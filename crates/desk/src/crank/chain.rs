//! The crank's chain handle: its key, the RPC, the mainnet guard (D-016), sending with an
//! outcome the callers can act on, and the `crank_ops` log.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

use crate::now_ms;
use crate::program::{Pdas, PendingAccount, TicketAccount};
use crate::solana::rpc::{RawAccount, RpcClient, RpcError};
use crate::solana::signer::{Keypair, SendError, send};

/// What became of a transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Confirmed; the signature.
    Landed(String),
    /// The program refused it (simulation or on-chain): the program's error, e.g.
    /// `PendingExpired`.
    Refused(String),
    /// Transport, blockhash expiry, RPC trouble: it may be retried.
    Transient(String),
}

/// Anchor's error name in a refusal's logs (`Error Code: PendingExpired.`), else the first line.
pub fn refusal_reason(text: &str) -> String {
    text.split("Error Code: ")
        .nth(1)
        .and_then(|rest| rest.split(['.', ' ']).next())
        .filter(|s| !s.is_empty())
        .map_or_else(
            || text.lines().next().unwrap_or(text).trim().to_owned(),
            str::to_owned,
        )
}

/// Classifies a send result: an instruction error (in preflight simulation or on-chain) is the
/// program refusing; everything else is transient.
pub fn classify(r: Result<String, SendError>) -> Outcome {
    match r {
        Ok(sig) => Outcome::Landed(sig),
        Err(SendError::Failed { err, .. }) if err.contains("InstructionError") => {
            Outcome::Refused(refusal_reason(&err))
        }
        Err(SendError::Rpc(RpcError::Rpc(msg)))
            if msg.contains("custom program error")
                || msg.contains("Error processing Instruction")
                || msg.contains("InstructionError")
                || msg.contains("AnchorError") =>
        {
            Outcome::Refused(refusal_reason(&msg))
        }
        Err(e) => Outcome::Transient(e.to_string()),
    }
}

/// Signs as the crank and sends.
pub struct Chain {
    /// RPC (URL logged only redacted).
    pub rpc: RpcClient,
    /// PDAs.
    pub pdas: Pdas,
    key: Keypair,
    /// 0 unchecked, 1 not mainnet, 2 mainnet (never send).
    cluster: AtomicU8,
}

impl Chain {
    /// A handle signing with `key`.
    pub fn new(rpc: RpcClient, pdas: Pdas, key: Keypair) -> Arc<Self> {
        Arc::new(Chain {
            rpc,
            pdas,
            key,
            cluster: AtomicU8::new(0),
        })
    }

    /// The crank's public key (fee payer and signer).
    pub fn pubkey(&self) -> Pubkey {
        self.key.pubkey()
    }

    /// Checks the genesis hash once (D-016); a mainnet endpoint disables every send.
    async fn guard(&self) -> Result<(), String> {
        match self.cluster.load(Ordering::Relaxed) {
            1 => Ok(()),
            2 => Err("mainnet-beta endpoint: the crank never sends (D-016)".into()),
            _ => match self.rpc.refuse_mainnet().await {
                Ok(()) => {
                    self.cluster.store(1, Ordering::Relaxed);
                    Ok(())
                }
                Err(RpcError::Rpc(e)) => {
                    self.cluster.store(2, Ordering::Relaxed);
                    tracing::error!(error = %e, "crank disabled");
                    Err(e)
                }
                Err(e) => Err(e.to_string()),
            },
        }
    }

    /// Signs `ixs` as the crank (fee payer), sends and waits for `confirmed`.
    pub async fn send(&self, ixs: &[Instruction]) -> Outcome {
        if let Err(e) = self.guard().await {
            return Outcome::Transient(e);
        }
        classify(send(&self.rpc, ixs, &[&self.key]).await)
    }

    /// Raw accounts (`None` = no account).
    pub async fn accounts(&self, keys: &[Pubkey]) -> Result<Vec<Option<RawAccount>>, String> {
        Ok(self
            .rpc
            .get_multiple_accounts(keys)
            .await
            .map_err(|e| e.to_string())?
            .1)
    }

    /// The pending at `key`, if it exists.
    pub async fn pending(&self, key: &Pubkey) -> Result<Option<PendingAccount>, String> {
        let acc = self.accounts(&[*key]).await?.pop().flatten();
        acc.map(|a| {
            PendingAccount::decode(&a.data, &a.owner, &self.pdas.program).map_err(|e| e.to_string())
        })
        .transpose()
    }

    /// The ticket at `key`, if it exists.
    pub async fn ticket(&self, key: &Pubkey) -> Result<Option<TicketAccount>, String> {
        let acc = self.accounts(&[*key]).await?.pop().flatten();
        acc.map(|a| {
            TicketAccount::decode(&a.data, &a.owner, &self.pdas.program).map_err(|e| e.to_string())
        })
        .transpose()
    }
}

/// Appends to `crank_ops` (DB first, then any bus event).
pub async fn record(
    db: &sqlx::SqlitePool,
    op: &str,
    target: &str,
    leg_idx: Option<u8>,
    status: &str,
    signature: Option<&str>,
    error: Option<&str>,
) {
    let r = sqlx::query(
        "INSERT INTO crank_ops (op, target, leg_idx, status, signature, error, ts_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(op)
    .bind(target)
    .bind(leg_idx.map(i64::from))
    .bind(status)
    .bind(signature)
    .bind(error)
    .bind(i64::try_from(now_ms()).unwrap_or(i64::MAX))
    .execute(db)
    .await;
    if let Err(e) = r {
        tracing::error!(error = %e, op, target, "cannot record crank op");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_refusals_and_transients() {
        let sim = "Transaction simulation failed: Error processing Instruction 1: custom program error: 0x1772\n  Program log: AnchorError occurred. Error Code: PendingExpired. Error Number: 6002.";
        assert_eq!(
            classify(Err(SendError::Rpc(RpcError::Rpc(sim.into())))),
            Outcome::Refused("PendingExpired".into())
        );
        assert_eq!(
            classify(Err(SendError::Failed {
                signature: "s".into(),
                err: r#"{"InstructionError":[1,{"Custom":6001}]}"#.into()
            })),
            Outcome::Refused(r#"{"InstructionError":[1,{"Custom":6001}]}"#.into())
        );
        assert!(matches!(
            classify(Err(SendError::Rpc(RpcError::Rpc(
                "Transaction simulation failed: Blockhash not found".into()
            )))),
            Outcome::Transient(_)
        ));
        assert!(matches!(
            classify(Err(SendError::Rpc(RpcError::Transport("down".into())))),
            Outcome::Transient(_)
        ));
        assert!(matches!(
            classify(Err(SendError::Expired {
                signature: "s".into()
            })),
            Outcome::Transient(_)
        ));
        assert_eq!(classify(Ok("sig".into())), Outcome::Landed("sig".into()));
    }
}
