//! Program log listener, adapted from the sibling crank's `listen.rs` and made program-agnostic:
//! `logsSubscribe` on a program id; each successful transaction that mentions it is fetched with
//! `getTransaction` and handed to a [`Handler`], which re-reads the accounts it touched (account
//! state is the source of truth, D-005) and may decode the program's `emit_cpi!` events with
//! [`cpi_events`].
//!
//! [`Handler::resync`] runs at start, after every (re)connect and every `resync` interval, which
//! closes any gap a dropped socket leaves. [`poll`] skips the socket and resyncs on a timer (the
//! cut-rule mode). Both return when the desk shuts down.

use std::future::Future;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use solana_pubkey::Pubkey;
use tokio_tungstenite::tungstenite::Message;

use super::rpc::{RpcClient, redact, transaction_keys};
use crate::state::AppState;

/// What the listener does with the chain. Implemented by the indexer (2.C).
pub trait Handler: Clone + Send + Sync + 'static {
    /// A confirmed successful transaction that mentioned the program, as `getTransaction`
    /// returns it (JSON encoding). An error triggers a full [`Handler::resync`].
    fn on_transaction(
        &self,
        signature: &str,
        tx: &Value,
    ) -> impl Future<Output = Result<(), String>> + Send;

    /// Re-reads every program account (`getProgramAccounts`) and applies it.
    fn resync(&self) -> impl Future<Output = Result<(), String>> + Send;
}

/// Seeds of Anchor's event authority PDA.
pub const EVENT_AUTHORITY_SEED: &[u8] = b"__event_authority";
/// Anchor's `emit_cpi!` instruction tag (`EVENT_IX_TAG`, little-endian `0x1d9acb512ea545e4`).
pub const EVENT_IX_TAG: [u8; 8] = [0xe4, 0x45, 0xa5, 0x2e, 0x51, 0xcb, 0x9a, 0x1d];

/// The event authority PDA of `program`.
pub fn event_authority(program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[EVENT_AUTHORITY_SEED], program).0
}

/// The `emit_cpi!` events of a confirmed transaction (JSON encoding), in order: the data of every
/// inner instruction that `program` invoked on itself with its event authority as first account
/// and the event tag in front (D-005). Each item is the event's 8-byte discriminator followed by
/// its borsh body. Only the program can sign for its event authority, so a forged log or a
/// different program's instruction cannot produce an item.
pub fn cpi_events(tx: &Value, program: &Pubkey) -> Vec<Vec<u8>> {
    let keys = transaction_keys(tx);
    let authority = event_authority(program);
    let key_at = |v: &Value| {
        v.as_u64()
            .and_then(|i| keys.get(usize::try_from(i).ok()?))
            .copied()
    };
    let mut out = Vec::new();
    for group in tx["meta"]["innerInstructions"]
        .as_array()
        .into_iter()
        .flatten()
    {
        for ix in group["instructions"].as_array().into_iter().flatten() {
            if key_at(&ix["programIdIndex"]) != Some(*program) {
                continue;
            }
            if ix["accounts"].get(0).and_then(key_at) != Some(authority) {
                continue;
            }
            let Some(data) = ix["data"]
                .as_str()
                .and_then(|d| bs58::decode(d).into_vec().ok())
            else {
                continue;
            };
            if let Some(body) = data.strip_prefix(&EVENT_IX_TAG) {
                out.push(body.to_vec());
            }
        }
    }
    out
}

/// The WebSocket URL a JSON-RPC URL implies: `https` → `wss`; `http` → `ws` with the port
/// plus one (the test validator serves PubSub on RPC port + 1, 8899 → 8900).
pub fn ws_url_for(rpc_url: &str) -> String {
    if let Some(rest) = rpc_url.strip_prefix("https://") {
        return format!("wss://{rest}");
    }
    let Some(rest) = rpc_url.strip_prefix("http://") else {
        return rpc_url.to_owned();
    };
    let ws = format!("ws://{rest}");
    match ws.rsplit_once(':') {
        Some((head, port)) if port.trim_end_matches('/').parse::<u16>().is_ok() => {
            let p: u16 = port.trim_end_matches('/').parse().unwrap_or(0);
            format!("{head}:{}", p.wrapping_add(1))
        }
        _ => ws,
    }
}

/// A `logsNotification`'s signature, if the transaction succeeded.
fn successful_signature(msg: &Value) -> Option<String> {
    if msg["method"] != "logsNotification" {
        return None;
    }
    let v = &msg["params"]["result"]["value"];
    if !v["err"].is_null() {
        return None;
    }
    v["signature"].as_str().map(str::to_owned)
}

/// Fetches `signature` and hands it to the handler; falls back to a full resync if the
/// transaction cannot be fetched.
async fn apply<H: Handler>(handler: &H, rpc: &RpcClient, signature: &str) -> Result<(), String> {
    for _ in 0..20 {
        if let Some(tx) = rpc
            .transaction(signature)
            .await
            .map_err(|e| e.to_string())?
        {
            return handler.on_transaction(signature, &tx).await;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    tracing::warn!(signature, "transaction not found; full resync");
    handler.resync().await
}

/// Resyncs every `every` until shutdown.
pub async fn poll<H: Handler>(state: &AppState, handler: &H, every: Duration) {
    loop {
        tokio::select! {
            _ = state.shutdown_requested() => return,
            r = handler.resync() => if let Err(e) = r {
                tracing::warn!(error = %e, "resync failed");
            },
        }
        tokio::select! {
            _ = state.shutdown_requested() => return,
            _ = tokio::time::sleep(every) => {}
        }
    }
}

/// Subscribes to `program`'s logs and applies them until shutdown, reconnecting with backoff.
pub async fn subscribe<H: Handler>(
    state: &AppState,
    handler: &H,
    rpc: &RpcClient,
    ws_url: &str,
    program: &Pubkey,
    resync: Duration,
) {
    let mut backoff = Duration::from_secs(1);
    loop {
        let started = tokio::time::Instant::now();
        tokio::select! {
            _ = state.shutdown_requested() => return,
            r = session(handler, rpc, ws_url, program, resync) => if let Err(e) = r {
                tracing::warn!(error = %e, "listener session ended");
            },
        }
        // A session that ran for a while was healthy: start the backoff over.
        if started.elapsed() > Duration::from_secs(60) {
            backoff = Duration::from_secs(1);
        }
        tokio::select! {
            _ = state.shutdown_requested() => return,
            _ = tokio::time::sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

/// Work for the worker of one listener session.
enum Job {
    Apply(String),
    Resync,
}

async fn session<H: Handler>(
    handler: &H,
    rpc: &RpcClient,
    ws_url: &str,
    program: &Pubkey,
    resync: Duration,
) -> Result<(), String> {
    let (socket, _) = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::connect_async(ws_url),
    )
    .await
    .map_err(|_| format!("connect to {} timed out", redact(ws_url)))?
    // tungstenite errors do not carry the URL; the redacted one is added here.
    .map_err(|e| format!("connect to {}: {e}", redact(ws_url)))?;
    let (mut sink, mut stream) = socket.split();
    let sub = json!({
        "jsonrpc": "2.0", "id": 1, "method": "logsSubscribe",
        "params": [{"mentions": [program.to_string()]}, {"commitment": "confirmed"}],
    });
    sink.send(Message::text(sub.to_string()))
        .await
        .map_err(|e| e.to_string())?;
    tracing::info!(ws_url = %redact(ws_url), %program, "logsSubscribe sent");
    // Catch up on whatever happened while disconnected.
    handler.resync().await?;

    // Updates run on a worker, in order: `apply` can wait seconds for getTransaction and a
    // resync reads every account, and neither may stall reading the socket or pinging.
    let (jobs, mut queue) = tokio::sync::mpsc::unbounded_channel::<Job>();
    let worker = {
        let (handler, rpc) = (handler.clone(), rpc.clone());
        tokio::spawn(async move {
            while let Some(job) = queue.recv().await {
                let done = match job {
                    Job::Apply(sig) => match apply(&handler, &rpc, &sig).await {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            tracing::warn!(error = %e, signature = sig, "update failed; full resync");
                            handler.resync().await
                        }
                    },
                    Job::Resync => handler.resync().await,
                };
                // The next resync tick retries.
                if let Err(e) = done {
                    tracing::warn!(error = %e, "resync failed");
                }
            }
        })
    };
    let ended = async {
        let mut ping = tokio::time::interval(Duration::from_secs(30));
        let mut resync_tick =
            tokio::time::interval_at(tokio::time::Instant::now() + resync, resync);
        loop {
            tokio::select! {
                msg = stream.next() => {
                    let msg = match msg {
                        None => return Err("socket closed".to_owned()),
                        Some(Err(e)) => return Err(e.to_string()),
                        Some(Ok(m)) => m,
                    };
                    let text = match msg {
                        Message::Text(t) => t.to_string(),
                        Message::Close(_) => return Err("socket closed by server".into()),
                        _ => continue,
                    };
                    let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
                    if v.get("error").is_some() {
                        // A refused subscribe is the endpoint's, not the network's: only resyncs
                        // (on every retry) see the chain until SOLANA_WS_URL names another socket.
                        let hint = if v["id"] == 1 { " (logsSubscribe refused; set SOLANA_WS_URL to a socket that serves it)" } else { "" };
                        return Err(format!("subscription error: {}{hint}", v["error"]));
                    }
                    if v["id"] == 1 && v["result"].is_u64() {
                        tracing::info!(subscription = %v["result"], "subscribed");
                    }
                    if let Some(sig) = successful_signature(&v) {
                        let _ = jobs.send(Job::Apply(sig));
                    }
                }
                _ = ping.tick() => {
                    sink.send(Message::Ping(Vec::new().into())).await.map_err(|e| e.to_string())?;
                }
                _ = resync_tick.tick() => {
                    let _ = jobs.send(Job::Resync);
                }
            }
        }
    }
    .await;
    worker.abort();
    ended
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_urls() {
        assert_eq!(ws_url_for("http://127.0.0.1:8899"), "ws://127.0.0.1:8900");
        assert_eq!(
            ws_url_for("https://api.devnet.solana.com"),
            "wss://api.devnet.solana.com"
        );
        assert_eq!(
            ws_url_for("https://devnet.helius-rpc.com/?api-key=abc"),
            "wss://devnet.helius-rpc.com/?api-key=abc"
        );
    }

    #[test]
    fn picks_successful_notifications() {
        let ok = json!({"method": "logsNotification", "params": {"result": {"context": {"slot": 5}, "value": {"signature": "S", "err": null, "logs": []}}}});
        assert_eq!(successful_signature(&ok).as_deref(), Some("S"));
        let failed = json!({"method": "logsNotification", "params": {"result": {"value": {"signature": "S", "err": {"InstructionError": [0, "Custom"]}}}}});
        assert_eq!(successful_signature(&failed), None);
        assert_eq!(successful_signature(&json!({"id": 1, "result": 7})), None);
    }

    #[test]
    fn extracts_cpi_events() {
        let program: Pubkey = crate::config::DEFAULT_PROGRAM_ID.parse().unwrap();
        let other = Pubkey::new_from_array([9; 32]);
        let authority = event_authority(&program);
        let event = [EVENT_IX_TAG.as_slice(), &[1, 2, 3, 4, 5, 6, 7, 8, 42]].concat();
        let enc = |b: &[u8]| bs58::encode(b).into_string();
        let tx = json!({
            "transaction": {"message": {"accountKeys": [
                other.to_string(), program.to_string(), authority.to_string()
            ]}},
            "meta": {"loadedAddresses": {"writable": [], "readonly": []}, "innerInstructions": [
                {"index": 0, "instructions": [
                    // The program's self-CPI with its event authority: kept.
                    {"programIdIndex": 1, "accounts": [2], "data": enc(&event)},
                    // Same bytes from another program: ignored.
                    {"programIdIndex": 0, "accounts": [2], "data": enc(&event)},
                    // Without the event authority: ignored.
                    {"programIdIndex": 1, "accounts": [0], "data": enc(&event)},
                    // Not an event: ignored.
                    {"programIdIndex": 1, "accounts": [2], "data": enc(&[0u8; 12])}
                ]}
            ]}
        });
        assert_eq!(
            cpi_events(&tx, &program),
            vec![vec![1, 2, 3, 4, 5, 6, 7, 8, 42]]
        );
    }
}
