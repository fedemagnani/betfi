//! Keypair files (the Solana CLI's JSON array of 64 bytes, kept outside the repo, D-015) and
//! signing, sending and confirming legacy transactions. Adapted from the sibling crank's
//! `signer.rs`; program-agnostic (the instructions come from the caller).

use std::path::Path;
use std::time::Duration;

use ed25519_dalek::{Signer as _, SigningKey};
use solana_instruction::Instruction;
use solana_message::Message;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_transaction::Transaction;

use super::rpc::{RpcClient, RpcError};

/// A signing key read from a keypair file. The key bytes never leave this struct: `Debug`
/// prints only the public key.
pub struct Keypair {
    key: SigningKey,
}

impl std::fmt::Debug for Keypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Keypair({})", self.pubkey())
    }
}

impl Keypair {
    /// Reads a Solana CLI keypair file (`[u8; 64]` as JSON: signing key ‖ public key).
    pub fn read(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("keypair {}: {e}", path.display()))?;
        let bytes: Vec<u8> = serde_json::from_str(&text)
            .map_err(|_| format!("keypair {}: not a JSON byte array", path.display()))?;
        Self::from_bytes(&bytes).map_err(|e| format!("keypair {}: {e}", path.display()))
    }

    /// A keypair from its 64 bytes (signing key ‖ public key).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let arr: [u8; 64] = bytes
            .try_into()
            .map_err(|_| "expected 64 bytes".to_owned())?;
        let key = SigningKey::from_keypair_bytes(&arr)
            .map_err(|_| "public half does not match the signing key".to_owned())?;
        Ok(Self { key })
    }

    /// The public key.
    pub fn pubkey(&self) -> Pubkey {
        Pubkey::new_from_array(self.key.verifying_key().to_bytes())
    }

    /// Ed25519 signature over `msg` (transactions, and the desk-signed quote and cash-out
    /// messages of C3, which the program checks against `Config.quote_signer`).
    pub fn sign(&self, msg: &[u8]) -> Signature {
        Signature::from(self.key.sign(msg).to_bytes())
    }
}

/// Why a transaction was not confirmed.
#[derive(Debug)]
pub enum SendError {
    /// The RPC failed.
    Rpc(RpcError),
    /// The transaction landed and failed.
    Failed {
        /// Signature.
        signature: String,
        /// The transaction error.
        err: String,
    },
    /// Not confirmed before its blockhash expired.
    Expired {
        /// Signature.
        signature: String,
    },
    /// The transaction could not be built or signed.
    Build(String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rpc(e) => e.fmt(f),
            Self::Failed { signature, err } => write!(f, "transaction {signature} failed: {err}"),
            Self::Expired { signature } => {
                write!(
                    f,
                    "transaction {signature} not confirmed before its blockhash expired"
                )
            }
            Self::Build(e) => write!(f, "cannot build transaction: {e}"),
        }
    }
}

impl std::error::Error for SendError {}

impl From<RpcError> for SendError {
    fn from(e: RpcError) -> Self {
        Self::Rpc(e)
    }
}

/// Signs `ixs` with `signers` (the first pays), sends it and waits for `confirmed`.
/// Every account marked as a signer must be one of `signers`.
pub async fn send(
    rpc: &RpcClient,
    ixs: &[Instruction],
    signers: &[&Keypair],
) -> Result<String, SendError> {
    let payer = signers
        .first()
        .ok_or_else(|| SendError::Build("no signer".into()))?
        .pubkey();
    let (blockhash, last_valid) = rpc.latest_blockhash().await?;
    let message = Message::new_with_blockhash(ixs, Some(&payer), &blockhash);
    let mut tx = Transaction::new_unsigned(message);
    let msg = tx.message_data();
    let n = usize::from(tx.message.header.num_required_signatures);
    for (i, key) in tx.message.account_keys[..n].iter().enumerate() {
        let kp = signers
            .iter()
            .find(|k| k.pubkey() == *key)
            .ok_or_else(|| SendError::Build(format!("missing signer {key}")))?;
        tx.signatures[i] = kp.sign(&msg);
    }
    let wire = wincode::serialize(&tx).map_err(|e| SendError::Build(e.to_string()))?;
    let signature = rpc.send_transaction(&wire).await?;
    loop {
        if let Some((status, err)) = rpc.signature_status(&signature).await? {
            if let Some(err) = err {
                return Err(SendError::Failed {
                    signature,
                    err: err.to_string(),
                });
            }
            if status == "confirmed" || status == "finalized" {
                return Ok(signature);
            }
        } else if rpc.block_height().await? > last_valid {
            return Err(SendError::Expired { signature });
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_cli_keypair_bytes() {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let mut bytes = [7u8; 32].to_vec();
        bytes.extend_from_slice(&sk.verifying_key().to_bytes());
        let kp = Keypair::from_bytes(&bytes).unwrap();
        assert_eq!(kp.pubkey().to_bytes(), sk.verifying_key().to_bytes());
        assert!(!format!("{kp:?}").contains("[7"));
        bytes[40] ^= 1;
        assert!(Keypair::from_bytes(&bytes).is_err());
        assert!(Keypair::from_bytes(&bytes[..63]).is_err());
    }
}
