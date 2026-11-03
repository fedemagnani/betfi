//! A small Solana JSON-RPC client over `reqwest`, at `confirmed`. Adapted from the sibling
//! crank's `rpc.rs`; program-agnostic. URLs are logged only through [`redact`] (D-016).

use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde_json::{Value, json};
use solana_hash::Hash;
use solana_pubkey::Pubkey;

/// Genesis hash of mainnet-beta, refused by [`RpcClient::refuse_mainnet`] (D-016).
pub const MAINNET_GENESIS: &str = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d";

/// An RPC failure. Never carries the URL (see [`redact`]).
#[derive(Debug)]
pub enum RpcError {
    /// The endpoint could not be reached.
    Transport(String),
    /// A JSON-RPC error object (message, plus simulation logs when the RPC returns them).
    Rpc(String),
    /// The response had an unexpected shape.
    Decode(String),
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "RPC unreachable: {e}"),
            Self::Rpc(e) => write!(f, "RPC error: {e}"),
            Self::Decode(e) => write!(f, "RPC response not understood: {e}"),
        }
    }
}

impl std::error::Error for RpcError {}

/// `url` without its query string, for logs and errors: hosted RPCs (Helius) carry the API key
/// there (`?api-key=…`), and it must never reach a log, a file or a commit.
pub fn redact(url: &str) -> String {
    match url.split_once('?') {
        Some((base, _)) => format!("{base}?…"),
        None => url.to_owned(),
    }
}

/// One account as returned with `encoding: base64`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawAccount {
    /// Owning program.
    pub owner: Pubkey,
    /// Balance.
    pub lamports: u64,
    /// Account data.
    pub data: Vec<u8>,
}

/// JSON-RPC client. Cheap to clone. `Debug` prints the redacted URL.
#[derive(Clone)]
pub struct RpcClient {
    url: String,
    http: reqwest::Client,
}

fn parse_account(v: &Value) -> Result<Option<RawAccount>, RpcError> {
    if v.is_null() {
        return Ok(None);
    }
    let bad = |what: &str| RpcError::Decode(what.to_owned());
    let owner = v["owner"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| bad("owner"))?;
    let lamports = v["lamports"].as_u64().ok_or_else(|| bad("lamports"))?;
    let data = &v["data"];
    if data[1] != "base64" {
        return Err(bad("encoding"));
    }
    let data = B64
        .decode(data[0].as_str().ok_or_else(|| bad("data"))?)
        .map_err(|e| RpcError::Decode(e.to_string()))?;
    Ok(Some(RawAccount {
        owner,
        lamports,
        data,
    }))
}

/// The `logs` array of a JSON-RPC error's `data` (preflight simulation), if any.
fn error_text(err: &Value) -> String {
    let mut s = err["message"].as_str().unwrap_or("").to_owned();
    if let Some(logs) = err["data"]["logs"].as_array() {
        for l in logs {
            s.push_str("\n  ");
            s.push_str(l.as_str().unwrap_or(""));
        }
    }
    if s.is_empty() { err.to_string() } else { s }
}

impl std::fmt::Debug for RpcClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RpcClient({})", redact(&self.url))
    }
}

impl RpcClient {
    /// A client for `url` (30 s timeout per call).
    pub fn new(url: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client");
        Self {
            url: url.into(),
            http,
        }
    }

    /// The full URL (may carry an API key: log [`redact`]ed).
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Fails when the endpoint is mainnet-beta, by genesis hash (D-016). The crank calls this
    /// once before sending anything.
    pub async fn refuse_mainnet(&self) -> Result<(), RpcError> {
        if self.genesis_hash().await? == MAINNET_GENESIS {
            return Err(RpcError::Rpc(format!(
                "{} is mainnet-beta; refused (D-016)",
                redact(&self.url)
            )));
        }
        Ok(())
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let mut delay = Duration::from_millis(500);
        // Separate budgets: rate limiting must not use up the retries for a dropped connection
        // (a send's confirmation loop would then fail although the transaction landed).
        let (mut throttled, mut dropped) = (0, 0);
        loop {
            let resp = self.http.post(&self.url).json(&body).send().await;
            let resp = match resp {
                Ok(r) if r.status().as_u16() == 429 && throttled < 5 => {
                    throttled += 1;
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(8));
                    continue;
                }
                Ok(r) => r,
                // reqwest's Display includes the request URL: strip it (see `redact`).
                Err(e) if dropped < 2 => {
                    dropped += 1;
                    let e = e.without_url();
                    tracing::debug!(%e, "RPC transport error, retrying");
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                    continue;
                }
                Err(e) => return Err(RpcError::Transport(e.without_url().to_string())),
            };
            let v: Value = resp
                .json()
                .await
                .map_err(|e| RpcError::Decode(e.without_url().to_string()))?;
            if let Some(err) = v.get("error") {
                return Err(RpcError::Rpc(error_text(err)));
            }
            return v
                .get("result")
                .cloned()
                .ok_or_else(|| RpcError::Decode("no result".into()));
        }
    }

    /// `getMultipleAccounts`: one entry per key (`None` = no account) and the lowest context
    /// slot of the (100-key) chunks, so no row is tagged newer than its data.
    pub async fn get_multiple_accounts(
        &self,
        keys: &[Pubkey],
    ) -> Result<(u64, Vec<Option<RawAccount>>), RpcError> {
        let mut out = Vec::with_capacity(keys.len());
        let mut slot = u64::MAX;
        for chunk in keys.chunks(100) {
            let keys: Vec<String> = chunk.iter().map(ToString::to_string).collect();
            let result = self
                .call(
                    "getMultipleAccounts",
                    json!([keys, {"encoding": "base64", "commitment": "confirmed"}]),
                )
                .await?;
            slot = slot.min(result["context"]["slot"].as_u64().unwrap_or(0));
            let values = result["value"]
                .as_array()
                .ok_or_else(|| RpcError::Decode("value".into()))?;
            if values.len() != chunk.len() {
                return Err(RpcError::Decode("account count".into()));
            }
            for v in values {
                out.push(parse_account(v)?);
            }
        }
        Ok((if slot == u64::MAX { 0 } else { slot }, out))
    }

    /// One account (`None` = no account).
    pub async fn get_account(&self, key: &Pubkey) -> Result<Option<RawAccount>, RpcError> {
        Ok(self.get_multiple_accounts(&[*key]).await?.1.pop().flatten())
    }

    /// `getProgramAccounts` with `memcmp` filters `(offset, bytes)`, with the context slot.
    pub async fn get_program_accounts(
        &self,
        program: &Pubkey,
        memcmp: &[(usize, Vec<u8>)],
    ) -> Result<(u64, Vec<(Pubkey, RawAccount)>), RpcError> {
        let filters: Vec<Value> = memcmp
            .iter()
            .map(|(offset, bytes)| {
                json!({"memcmp": {"offset": offset, "bytes": bs58::encode(bytes).into_string()}})
            })
            .collect();
        let result = self
            .call(
                "getProgramAccounts",
                json!([program.to_string(), {
                    "encoding": "base64",
                    "commitment": "confirmed",
                    "filters": filters,
                    "withContext": true,
                }]),
            )
            .await?;
        let slot = result["context"]["slot"].as_u64().unwrap_or(0);
        let list = result["value"]
            .as_array()
            .ok_or_else(|| RpcError::Decode("program accounts".into()))?;
        let mut out = Vec::with_capacity(list.len());
        for item in list {
            let key: Pubkey = item["pubkey"]
                .as_str()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| RpcError::Decode("pubkey".into()))?;
            if let Some(acc) = parse_account(&item["account"])? {
                out.push((key, acc));
            }
        }
        Ok((slot, out))
    }

    /// `getLatestBlockhash`: the blockhash and its last valid block height.
    pub async fn latest_blockhash(&self) -> Result<(Hash, u64), RpcError> {
        let result = self
            .call("getLatestBlockhash", json!([{"commitment": "confirmed"}]))
            .await?;
        let hash: Hash = result["value"]["blockhash"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| RpcError::Decode("blockhash".into()))?;
        let height = result["value"]["lastValidBlockHeight"]
            .as_u64()
            .ok_or_else(|| RpcError::Decode("lastValidBlockHeight".into()))?;
        Ok((hash, height))
    }

    /// `getGenesisHash`.
    pub async fn genesis_hash(&self) -> Result<String, RpcError> {
        self.call("getGenesisHash", json!([]))
            .await?
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| RpcError::Decode("genesis hash".into()))
    }

    /// `getBlockHeight` at `confirmed`.
    pub async fn block_height(&self) -> Result<u64, RpcError> {
        self.call("getBlockHeight", json!([{"commitment": "confirmed"}]))
            .await?
            .as_u64()
            .ok_or_else(|| RpcError::Decode("block height".into()))
    }

    /// Unix time of the latest confirmed slot (the cluster's `Clock`, approximately).
    pub async fn cluster_time(&self) -> Result<i64, RpcError> {
        let slot = self
            .call("getSlot", json!([{"commitment": "confirmed"}]))
            .await?
            .as_u64()
            .ok_or_else(|| RpcError::Decode("slot".into()))?;
        // The newest confirmed slot may not have a block time yet (null, or "Block not
        // available"); step back a few slots instead of failing open-week / settle-week.
        let mut last = RpcError::Decode("block time".into());
        for s in (slot.saturating_sub(8)..=slot).rev() {
            match self.call("getBlockTime", json!([s])).await {
                Ok(v) if v.is_i64() => return Ok(v.as_i64().unwrap_or_default()),
                Ok(_) => {}
                Err(e @ RpcError::Rpc(_)) => last = e,
                Err(e) => return Err(e),
            }
        }
        Err(last)
    }

    /// `sendTransaction` (base64, with preflight at `confirmed`); returns the signature.
    pub async fn send_transaction(&self, wire: &[u8]) -> Result<String, RpcError> {
        self.call(
            "sendTransaction",
            json!([B64.encode(wire), {"encoding": "base64", "preflightCommitment": "confirmed"}]),
        )
        .await?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| RpcError::Decode("signature".into()))
    }

    /// `getSignatureStatuses` for one signature: `None` while unknown, else
    /// `(confirmationStatus, err)`.
    pub async fn signature_status(
        &self,
        signature: &str,
    ) -> Result<Option<(String, Option<Value>)>, RpcError> {
        let result = self
            .call(
                "getSignatureStatuses",
                json!([[signature], {"searchTransactionHistory": true}]),
            )
            .await?;
        let v = &result["value"][0];
        if v.is_null() {
            return Ok(None);
        }
        Ok(Some((
            v["confirmationStatus"]
                .as_str()
                .unwrap_or("processed")
                .to_owned(),
            v.get("err").filter(|e| !e.is_null()).cloned(),
        )))
    }

    /// `getTransaction` (JSON) at `confirmed`, or `None` if not yet available.
    pub async fn transaction(&self, signature: &str) -> Result<Option<Value>, RpcError> {
        let result = self
            .call(
                "getTransaction",
                json!([signature, {"encoding": "json", "commitment": "confirmed", "maxSupportedTransactionVersion": 0}]),
            )
            .await?;
        Ok((!result.is_null()).then_some(result))
    }
}

/// Every account key a confirmed transaction touched (static keys plus loaded addresses).
pub fn transaction_keys(tx: &Value) -> Vec<Pubkey> {
    let mut keys: Vec<Pubkey> = tx["transaction"]["message"]["accountKeys"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|k| k.as_str()?.parse().ok())
        .collect();
    for kind in ["writable", "readonly"] {
        keys.extend(
            tx["meta"]["loadedAddresses"][kind]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|k| k.as_str()?.parse::<Pubkey>().ok()),
        );
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_accounts_and_errors() {
        let v = json!({"owner": "11111111111111111111111111111111", "lamports": 5, "data": ["AQID", "base64"]});
        let a = parse_account(&v).unwrap().unwrap();
        assert_eq!(a.data, vec![1, 2, 3]);
        assert_eq!(parse_account(&Value::Null).unwrap(), None);
        assert!(parse_account(&json!({"owner": "11111111111111111111111111111111", "lamports": 5, "data": ["x", "base58"]})).is_err());

        let e = json!({"code": -32002, "message": "simulation failed", "data": {"logs": ["Program log: a", "Program log: b"]}});
        assert_eq!(
            error_text(&e),
            "simulation failed\n  Program log: a\n  Program log: b"
        );
    }

    #[test]
    fn collects_transaction_keys() {
        let tx = json!({
            "transaction": {"message": {"accountKeys": ["11111111111111111111111111111111"]}},
            "meta": {"loadedAddresses": {"writable": ["TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"], "readonly": []}}
        });
        assert_eq!(transaction_keys(&tx).len(), 2);
    }

    #[test]
    fn redacts_api_keys() {
        assert_eq!(
            redact("https://devnet.helius-rpc.com/?api-key=secret"),
            "https://devnet.helius-rpc.com/?…"
        );
        assert_eq!(redact("http://127.0.0.1:8899"), "http://127.0.0.1:8899");
    }

    #[tokio::test]
    async fn transport_errors_omit_the_url() {
        // Nothing listens on port 9; the error must not carry the query string.
        let rpc = RpcClient::new("http://127.0.0.1:9/?api-key=secret");
        let e = rpc.call("getHealth", json!([])).await.unwrap_err();
        assert!(!e.to_string().contains("secret"), "{e}");
    }
}
