//! v0 transactions with one address lookup table (`[swap, Ed25519 verify, open_pending]` does
//! not fit a legacy transaction once the quote has 4–5 legs), signed with desk keypairs. Used
//! by the set-up tooling and the localnet proof; the app builds its own (`app/tx/src/swap.rs`).

use std::time::Duration;

use serde_json::json;
use solana_hash::Hash;
use solana_instruction::Instruction;
use solana_message::{AddressLookupTableAccount, VersionedMessage, v0};
use solana_pubkey::Pubkey;
use solana_transaction::versioned::VersionedTransaction;

use crate::solana::rpc::{RpcClient, RpcError};
use crate::solana::signer::{Keypair, SendError};

/// Largest serialized transaction a validator accepts.
pub const PACKET_DATA_SIZE: usize = 1_232;

/// The signed v0 transaction of `ixs` (fee payer `signers[0]`) and its wire bytes. Every
/// account marked as a signer must be one of `signers`.
pub fn sign_v0(
    ixs: &[Instruction],
    signers: &[&Keypair],
    tables: &[AddressLookupTableAccount],
    blockhash: Hash,
) -> Result<(VersionedTransaction, Vec<u8>), SendError> {
    let payer = signers
        .first()
        .ok_or_else(|| SendError::Build("no signer".into()))?
        .pubkey();
    let msg = v0::Message::try_compile(&payer, ixs, tables, blockhash)
        .map_err(|e| SendError::Build(e.to_string()))?;
    let n = usize::from(msg.header.num_required_signatures);
    let message = VersionedMessage::V0(msg);
    let bytes = message.serialize();
    let mut signatures = Vec::with_capacity(n);
    for key in &message.static_account_keys()[..n] {
        let kp = signers
            .iter()
            .find(|k| k.pubkey() == *key)
            .ok_or_else(|| SendError::Build(format!("missing signer {key}")))?;
        signatures.push(kp.sign(&bytes));
    }
    let tx = VersionedTransaction {
        signatures,
        message,
    };
    let wire = wincode::serialize(&tx).map_err(|e| SendError::Build(e.to_string()))?;
    Ok((tx, wire))
}

/// Signs, sends and waits for `confirmed`, like [`crate::solana::signer::send`] for v0.
pub async fn send_v0(
    rpc: &RpcClient,
    ixs: &[Instruction],
    signers: &[&Keypair],
    tables: &[AddressLookupTableAccount],
) -> Result<String, SendError> {
    let (blockhash, last_valid) = rpc.latest_blockhash().await?;
    let (_, wire) = sign_v0(ixs, signers, tables, blockhash)?;
    if wire.len() > PACKET_DATA_SIZE {
        return Err(SendError::Build(format!(
            "transaction is {} bytes, over {PACKET_DATA_SIZE}",
            wire.len()
        )));
    }
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

/// `getSlot` at `finalized` (a slot certain to be in `SlotHashes`, for `CreateLookupTable`).
pub async fn finalized_slot(rpc: &RpcClient) -> Result<u64, RpcError> {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "getSlot", "params": [{"commitment": "finalized"}]});
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| RpcError::Transport(e.to_string()))?;
    let v: serde_json::Value = http
        .post(rpc.url())
        .json(&body)
        .send()
        .await
        .map_err(|e| RpcError::Transport(e.without_url().to_string()))?
        .json()
        .await
        .map_err(|e| RpcError::Decode(e.without_url().to_string()))?;
    v["result"]
        .as_u64()
        .ok_or_else(|| RpcError::Decode("slot".into()))
}

/// Reads lookup table `key` as `v0::Message::try_compile` wants it.
pub async fn lookup_table(
    rpc: &RpcClient,
    key: &Pubkey,
) -> Result<AddressLookupTableAccount, RpcError> {
    let acc = rpc
        .get_account(key)
        .await?
        .ok_or_else(|| RpcError::Decode(format!("lookup table {key} not found")))?;
    if acc.owner != super::ix::ALT_PROGRAM_ID {
        return Err(RpcError::Decode(format!("{key} is not a lookup table")));
    }
    let addresses = super::accounts::lookup_table_addresses(&acc.data)
        .map_err(|e| RpcError::Decode(e.0.into()))?;
    Ok(AddressLookupTableAccount {
        key: *key,
        addresses,
    })
}
