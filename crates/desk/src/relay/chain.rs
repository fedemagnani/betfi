//! What the relay asks of the chain, behind [`Chain`] so the pipeline is testable without a
//! validator: read lookup tables, simulate (rule 5), send. [`RpcChain`] is the JSON-RPC one;
//! errors never carry the RPC URL (it may hold an API key, D-016).

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde_json::{Value, json};
use solana_pubkey::Pubkey;

use crate::solana::rpc::{RpcClient, RpcError};
use crate::swap::ix::ALT_PROGRAM_ID;

/// A chain failure: the transaction is refused (422), or the chain cannot answer (503).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainError {
    /// Refused, in plain words.
    Refused(String),
    /// RPC down, or the sponsor cannot pay.
    Unavailable(String),
}

/// One simulation's outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Simulation {
    /// The failure in plain words (`None` = it succeeded).
    pub err: Option<String>,
    /// The sponsor's balance before, lamports.
    pub sponsor_before: u64,
    /// The sponsor's balance after, lamports.
    pub sponsor_after: u64,
}

impl Simulation {
    /// What the transaction costs the sponsor (fee plus rent), lamports.
    pub fn spend(&self) -> u64 {
        self.sponsor_before.saturating_sub(self.sponsor_after)
    }
}

/// The relay's chain.
#[async_trait]
pub trait Chain: Send + Sync {
    /// The addresses of each lookup table in `keys`; a missing table or another account is
    /// refused.
    async fn lookup_tables(
        &self,
        keys: &[Pubkey],
    ) -> Result<HashMap<Pubkey, Vec<Pubkey>>, ChainError>;
    /// Which of `keys` hold no account.
    async fn missing(&self, keys: &[Pubkey]) -> Result<Vec<Pubkey>, ChainError>;
    /// Simulates the wire transaction (signatures not verified), reading `sponsor`'s balance
    /// before and after.
    async fn simulate(&self, wire: &[u8], sponsor: &Pubkey) -> Result<Simulation, ChainError>;
    /// Sends the signed wire transaction (preflight on); its signature once the RPC accepted it.
    async fn send(&self, wire: &[u8]) -> Result<String, ChainError>;
}

/// [`Chain`] over JSON-RPC.
#[derive(Debug)]
pub struct RpcChain {
    rpc: RpcClient,
    http: reqwest::Client,
    /// Whether the endpoint was checked not to be mainnet (once, before the first send).
    not_mainnet: tokio::sync::OnceCell<()>,
}

impl RpcChain {
    /// A chain on `rpc`.
    pub fn new(rpc: RpcClient) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client");
        RpcChain {
            rpc,
            http,
            not_mainnet: tokio::sync::OnceCell::new(),
        }
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let v: Value = self
            .http
            .post(self.rpc.url())
            .json(&body)
            .send()
            .await
            .map_err(|e| RpcError::Transport(e.without_url().to_string()))?
            .json()
            .await
            .map_err(|e| RpcError::Decode(e.without_url().to_string()))?;
        if let Some(err) = v.get("error") {
            return Err(RpcError::Rpc(
                err["message"]
                    .as_str()
                    .map_or_else(|| err.to_string(), str::to_owned),
            ));
        }
        v.get("result")
            .cloned()
            .ok_or_else(|| RpcError::Decode("no result".into()))
    }
}

fn unavailable(e: &RpcError) -> ChainError {
    ChainError::Unavailable(format!("Solana RPC: {e}"))
}

#[async_trait]
impl Chain for RpcChain {
    async fn lookup_tables(
        &self,
        keys: &[Pubkey],
    ) -> Result<HashMap<Pubkey, Vec<Pubkey>>, ChainError> {
        let (_, accounts) = self
            .rpc
            .get_multiple_accounts(keys)
            .await
            .map_err(|e| unavailable(&e))?;
        let mut out = HashMap::with_capacity(keys.len());
        for (key, acc) in keys.iter().zip(accounts) {
            let acc = acc
                .ok_or_else(|| ChainError::Refused(format!("lookup table {key} does not exist")))?;
            if acc.owner != ALT_PROGRAM_ID {
                return Err(ChainError::Refused(format!("{key} is not a lookup table")));
            }
            let addresses = crate::swap::accounts::lookup_table_addresses(&acc.data)
                .map_err(|_| ChainError::Refused(format!("{key} is not a lookup table")))?;
            out.insert(*key, addresses);
        }
        Ok(out)
    }

    async fn missing(&self, keys: &[Pubkey]) -> Result<Vec<Pubkey>, ChainError> {
        let (_, accounts) = self
            .rpc
            .get_multiple_accounts(keys)
            .await
            .map_err(|e| unavailable(&e))?;
        Ok(keys
            .iter()
            .zip(accounts)
            .filter(|(_, a)| a.as_ref().is_none_or(|a| a.lamports == 0))
            .map(|(k, _)| *k)
            .collect())
    }

    async fn simulate(&self, wire: &[u8], sponsor: &Pubkey) -> Result<Simulation, ChainError> {
        let sponsor_s = sponsor.to_string();
        let result = self
            .call(
                "simulateTransaction",
                json!([B64.encode(wire), {
                    "encoding": "base64",
                    "sigVerify": false,
                    "replaceRecentBlockhash": false,
                    "commitment": "confirmed",
                    "accounts": {"encoding": "base64", "addresses": [sponsor_s]},
                }]),
            )
            .await
            .map_err(|e| match e {
                // The node refused to simulate it at all (bad lookup table, malformed, ...).
                RpcError::Rpc(msg) => ChainError::Refused(format!("the network refused it: {msg}")),
                e => unavailable(&e),
            })?;
        let v = &result["value"];
        let logs: Vec<&str> = v["logs"]
            .as_array()
            .map(|l| l.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let err = v.get("err").filter(|e| !e.is_null());
        if let Some(e) = err
            && sponsor_cannot_pay(e)
        {
            return Err(ChainError::Unavailable(
                "the sponsor is out of SOL; try again later".into(),
            ));
        }
        let err = err.map(|e| plain_error(e, &logs));
        let after = v["accounts"][0]["lamports"]
            .as_u64()
            .or_else(|| v["postBalances"][0].as_u64());
        let before = match v["preBalances"][0].as_u64() {
            Some(b) => Some(b),
            None => self
                .rpc
                .get_account(sponsor)
                .await
                .map_err(|e| unavailable(&e))?
                .map(|a| a.lamports),
        };
        // A failed simulation may not report balances; it is refused for its error anyway.
        let (sponsor_before, sponsor_after) = match (before, after, &err) {
            (Some(b), Some(a), _) => (b, a),
            (_, _, Some(_)) => (0, 0),
            _ => {
                return Err(ChainError::Unavailable(
                    "the simulation did not report the sponsor's balance".into(),
                ));
            }
        };
        Ok(Simulation {
            err,
            sponsor_before,
            sponsor_after,
        })
    }

    async fn send(&self, wire: &[u8]) -> Result<String, ChainError> {
        if self
            .not_mainnet
            .get_or_try_init(|| async { self.rpc.refuse_mainnet().await })
            .await
            .is_err()
        {
            return Err(ChainError::Unavailable(
                "the Solana RPC could not be checked (or is mainnet); nothing sent".into(),
            ));
        }
        self.rpc.send_transaction(wire).await.map_err(|e| match e {
            RpcError::Rpc(msg) => ChainError::Refused(plain_text(&msg)),
            e => unavailable(&e),
        })
    }
}

/// Whether a simulation error means the fee payer (the sponsor) cannot pay: a desk problem, not
/// the user's.
fn sponsor_cannot_pay(err: &Value) -> bool {
    matches!(
        err.as_str(),
        Some("AccountNotFound" | "InsufficientFundsForFee" | "InsufficientFundsForRent")
    ) || err["InsufficientFundsForRent"]["account_index"].as_u64() == Some(0)
}

/// A simulation error in plain words: Anchor's `Error Message:` from the logs when there is one,
/// else the program's own words, else the runtime's error name.
pub fn plain_error(err: &Value, logs: &[&str]) -> String {
    let joined = logs.join("\n");
    if let Some(m) = anchor_message(&joined) {
        return format!("the program refused it: {m}");
    }
    match err.as_str() {
        Some("BlockhashNotFound") => {
            "the transaction's blockhash expired: sign it again".to_owned()
        }
        Some("AlreadyProcessed") => "this transaction was already sent".to_owned(),
        Some(other) => format!("the network refused it: {other}"),
        None => match err["InstructionError"].as_array() {
            Some(a) => format!(
                "instruction {} failed: {}",
                a.first().and_then(Value::as_u64).map_or(0, |i| i + 1),
                a.get(1).map_or_else(String::new, |e| match e.as_str() {
                    Some(s) => s.to_owned(),
                    None => e.to_string(),
                })
            ),
            None => format!("the network refused it: {err}"),
        },
    }
}

/// `sendTransaction`'s error text (preflight) in plain words.
fn plain_text(msg: &str) -> String {
    match anchor_message(msg) {
        Some(m) => format!("the program refused it: {m}"),
        None => format!(
            "the network refused it: {}",
            msg.lines().next().unwrap_or(msg).trim()
        ),
    }
}

/// `the desk quote has expired` from `... Error Message: the desk quote has expired.`
fn anchor_message(text: &str) -> Option<String> {
    let rest = text.split("Error Message: ").nth(1)?;
    let line = rest.lines().next()?.trim().trim_end_matches('.');
    (!line.is_empty()).then(|| line.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_words() {
        let logs = [
            "Program log: Instruction: FaucetUsdc",
            "Program log: AnchorError thrown in programs/schedina/src/instructions/faucet_usdc.rs:56. Error Code: FaucetCooldown. Error Number: 6043. Error Message: faucet cooldown: one claim per hour.",
        ];
        let err = json!({"InstructionError": [1, {"Custom": 6043}]});
        assert_eq!(
            plain_error(&err, &logs),
            "the program refused it: faucet cooldown: one claim per hour"
        );
        assert_eq!(
            plain_error(&err, &[]),
            r#"instruction 2 failed: {"Custom":6043}"#
        );
        assert_eq!(
            plain_error(&json!("BlockhashNotFound"), &[]),
            "the transaction's blockhash expired: sign it again"
        );
        assert!(sponsor_cannot_pay(&json!("InsufficientFundsForFee")));
        assert!(sponsor_cannot_pay(
            &json!({"InsufficientFundsForRent": {"account_index": 0}})
        ));
        assert!(!sponsor_cannot_pay(
            &json!({"InsufficientFundsForRent": {"account_index": 3}})
        ));
        assert!(!sponsor_cannot_pay(&err));
        assert_eq!(
            plain_text("Transaction simulation failed: Blockhash not found"),
            "the network refused it: Transaction simulation failed: Blockhash not found"
        );
    }
}
