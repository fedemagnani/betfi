//! The relay's checks on a user-signed transaction, before the desk co-signs it (rules 1–4 of
//! [`super`]): pure functions over the decoded transaction and the lookup tables it uses, so
//! every refusal is testable without a chain.

use std::collections::{HashMap, HashSet};

use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use solana_message::VersionedMessage;
use solana_pubkey::Pubkey;
use solana_transaction::versioned::VersionedTransaction;

use crate::program::ix::disc;
use crate::program::{ASSOCIATED_TOKEN_PROGRAM_ID, ED25519_PROGRAM_ID, associated_token_address};
use crate::swap::ix::disc as amm_disc;

/// Compute budget program.
pub const COMPUTE_BUDGET_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("ComputeBudget111111111111111111111111111111");
/// SPL Memo program v2.
pub const MEMO_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");
/// The memo a sponsored `faucet_usdc` must carry, signed by the user (L8).
pub const FAUCET_MEMO: &[u8] = b"schedina:faucet";
/// Highest `SetComputeUnitPrice` the relay pays, micro-lamports per compute unit.
pub const MAX_CU_PRICE: u64 = 10_000;
/// Largest serialized transaction a validator accepts.
pub const PACKET_DATA_SIZE: usize = 1_232;

/// `open_pending`'s `payer` (L8).
const OPEN_PENDING_PAYER: usize = 8;
/// mock-amm `faucet`'s `payer`, its last account (L8).
const AMM_FAUCET_PAYER: usize = 8;
/// `faucet_usdc`'s `recipient`.
const FAUCET_RECIPIENT: usize = 3;
/// ATA `CreateIdempotent`'s wallet (owner).
const ATA_OWNER: usize = 2;

/// Why the relay will not co-sign, in plain words (the 422 `reason`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal(pub String);

impl Refusal {
    fn new(reason: impl Into<String>) -> Self {
        Refusal(reason.into())
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What the checks need to know about this deployment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// The sponsor key (fee payer, slot 0).
    pub sponsor: Pubkey,
    /// The schedina program.
    pub schedina: Pubkey,
    /// The mock AMM program.
    pub amm: Pubkey,
    /// Schedina's mock USDC mint (the sponsored faucet's recipient is the user's ATA of it).
    pub usdc_mint: Pubkey,
}

/// A transaction that passed the checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    /// The user (`static_account_keys[1]`).
    pub user: Pubkey,
    /// Token accounts an ATA `CreateIdempotent` funded by the sponsor would create (account 1).
    /// The relay pays each address at most once ([`super::limits::Limits`]): a user who closes
    /// the account keeps its rent, so re-creating it would farm the sponsor.
    pub sponsored_atas: Vec<Pubkey>,
}

/// The top-level instructions the relay knows (rule 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    ComputeBudget,
    Ed25519,
    Memo,
    CreateAta,
    OpenPending,
    CashOut,
    FaucetUsdc,
    Swap,
    AmmFaucet,
}

impl Kind {
    /// Account positions where the sponsor may stand (rule 3).
    fn sponsor_slots(self) -> &'static [usize] {
        match self {
            Kind::CreateAta | Kind::FaucetUsdc => &[0],
            Kind::OpenPending => &[OPEN_PENDING_PAYER],
            Kind::AmmFaucet => &[AMM_FAUCET_PAYER],
            _ => &[],
        }
    }

    /// The account that must be the user (rule 4), and the instruction's name.
    fn user_slot(self) -> Option<(usize, &'static str)> {
        match self {
            Kind::OpenPending => Some((0, "open_pending")),
            Kind::CashOut => Some((0, "cash_out")),
            Kind::Swap => Some((0, "swap")),
            Kind::AmmFaucet => Some((0, "the token faucet")),
            Kind::CreateAta => Some((ATA_OWNER, "the token account creation")),
            _ => None,
        }
    }
}

/// Decodes `POST /relay`'s `tx`: base64 of a wire `VersionedTransaction`, at most
/// [`PACKET_DATA_SIZE`] bytes, in its one canonical encoding (no trailing bytes), legacy or v0.
pub fn decode(b64: &str) -> Result<VersionedTransaction, Refusal> {
    // 1,232 bytes are 1,644 base64 characters; allow some whitespace, read no further.
    if b64.len() > 2 * PACKET_DATA_SIZE {
        return Err(Refusal::new("the transaction is too large"));
    }
    let wire = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|_| Refusal::new("tx is not base64"))?;
    if wire.len() > PACKET_DATA_SIZE {
        return Err(Refusal::new(format!(
            "the transaction is {} bytes, over {PACKET_DATA_SIZE}",
            wire.len()
        )));
    }
    let tx: VersionedTransaction =
        wincode::deserialize(&wire).map_err(|_| Refusal::new("tx is not a Solana transaction"))?;
    if wincode::serialize(&tx).ok().as_deref() != Some(&wire[..]) {
        return Err(Refusal::new("tx is not a Solana transaction"));
    }
    if matches!(tx.message, VersionedMessage::V1(_)) {
        return Err(Refusal::new("only legacy and v0 transactions are relayed"));
    }
    tx.sanitize()
        .map_err(|_| Refusal::new("the transaction is malformed"))?;
    Ok(tx)
}

/// Rule 1: exactly two required signatures, the sponsor as fee payer (slot 0), and the user's
/// signature (slot 1) valid over the message. Returns the user.
pub fn check_signers(tx: &VersionedTransaction, sponsor: &Pubkey) -> Result<Pubkey, Refusal> {
    let keys = tx.message.static_account_keys();
    if tx.message.header().num_required_signatures != 2 || tx.signatures.len() != 2 {
        return Err(Refusal::new(
            "the transaction must need exactly two signatures: the sponsor's and yours",
        ));
    }
    if keys.first() != Some(sponsor) {
        return Err(Refusal::new(format!(
            "the fee payer must be the sponsor {sponsor}"
        )));
    }
    let user = keys[1];
    let vk = VerifyingKey::from_bytes(&user.to_bytes())
        .map_err(|_| Refusal::new("the signer is not a wallet key"))?;
    let sig = Signature::from_bytes(tx.signatures[1].as_array());
    vk.verify_strict(&tx.message.serialize(), &sig)
        .map_err(|_| Refusal::new("your signature does not match the transaction"))?;
    Ok(user)
}

/// The lookup tables a v0 transaction loads addresses from.
pub fn lookup_tables(tx: &VersionedTransaction) -> Vec<Pubkey> {
    tx.message
        .address_table_lookups()
        .unwrap_or_default()
        .iter()
        .map(|l| l.account_key)
        .collect()
}

/// Every account of the transaction in index order: the static keys, then the writable and then
/// the read-only addresses loaded from `tables` (each table's addresses, by table key).
pub fn resolve_keys(
    tx: &VersionedTransaction,
    tables: &HashMap<Pubkey, Vec<Pubkey>>,
) -> Result<Vec<Pubkey>, Refusal> {
    let mut keys = tx.message.static_account_keys().to_vec();
    let lookups = tx.message.address_table_lookups().unwrap_or_default();
    let mut readonly = Vec::new();
    for l in lookups {
        let table = tables.get(&l.account_key).ok_or_else(|| {
            Refusal::new(format!("lookup table {} could not be read", l.account_key))
        })?;
        let pick = |i: &u8| {
            table.get(usize::from(*i)).copied().ok_or_else(|| {
                Refusal::new(format!("lookup table {} has no address {i}", l.account_key))
            })
        };
        for i in &l.writable_indexes {
            keys.push(pick(i)?);
        }
        for i in &l.readonly_indexes {
            readonly.push(pick(i)?);
        }
    }
    keys.extend(readonly);
    Ok(keys)
}

/// Rules 1–4 over the decoded transaction and the addresses of the lookup tables it uses.
pub fn check(
    tx: &VersionedTransaction,
    policy: &Policy,
    tables: &HashMap<Pubkey, Vec<Pubkey>>,
) -> Result<Checked, Refusal> {
    let user = check_signers(tx, &policy.sponsor)?;
    let keys = resolve_keys(tx, tables)?;
    let n_static = tx.message.static_account_keys().len();
    if keys[n_static..].contains(&policy.sponsor) {
        return Err(Refusal::new("a lookup table holds the sponsor key"));
    }
    let mut seen = HashSet::with_capacity(keys.len());
    if !keys.iter().all(|k| seen.insert(*k)) {
        return Err(Refusal::new("an account is listed twice"));
    }

    let user_ata_usdc = associated_token_address(&user, &policy.usdc_mint);
    let mut sponsored_atas = Vec::new();
    let mut sponsored_faucet = false;
    let mut faucet_memo = false;
    for (n, ix) in tx.message.instructions().iter().enumerate() {
        let at = |i: &u8| {
            keys.get(usize::from(*i))
                .copied()
                .ok_or_else(|| Refusal::new("an instruction names a missing account"))
        };
        let program = at(&ix.program_id_index)?;
        let accounts = ix.accounts.iter().map(at).collect::<Result<Vec<_>, _>>()?;
        let kind = classify(policy, &program, &ix.data)
            .map_err(|what| Refusal::new(format!("instruction {}: {what}", n + 1)))?;

        // Rule 3: the sponsor only where it pays.
        for (i, a) in accounts.iter().enumerate() {
            if *a == policy.sponsor && !kind.sponsor_slots().contains(&i) {
                return Err(Refusal::new(format!(
                    "instruction {}: the sponsor may only pay fees and rent",
                    n + 1
                )));
            }
        }
        // Rule 4: the user's own instructions.
        if let Some((slot, name)) = kind.user_slot() {
            match accounts.get(slot) {
                Some(a) if *a == user => {}
                Some(_) => {
                    return Err(Refusal::new(format!(
                        "instruction {}: {name} must be for the signing wallet",
                        n + 1
                    )));
                }
                None => {
                    return Err(Refusal::new(format!(
                        "instruction {}: too few accounts",
                        n + 1
                    )));
                }
            }
        }
        match kind {
            Kind::FaucetUsdc if accounts.first() == Some(&policy.sponsor) => {
                if accounts.get(FAUCET_RECIPIENT) != Some(&user_ata_usdc) {
                    return Err(Refusal::new(format!(
                        "instruction {}: a sponsored faucet pays only your own USDC account",
                        n + 1
                    )));
                }
                sponsored_faucet = true;
            }
            Kind::Memo if ix.data == FAUCET_MEMO && accounts.contains(&user) => {
                faucet_memo = true;
            }
            Kind::CreateAta if accounts.first() == Some(&policy.sponsor) => {
                if let Some(ata) = accounts.get(1) {
                    sponsored_atas.push(*ata);
                }
            }
            _ => {}
        }
    }
    if sponsored_faucet && !faucet_memo {
        return Err(Refusal::new(
            "a sponsored faucet claim must carry the memo \"schedina:faucet\" signed by you",
        ));
    }
    Ok(Checked {
        user,
        sponsored_atas,
    })
}

/// Rule 2: the program allowlist, by program and instruction data.
fn classify(policy: &Policy, program: &Pubkey, data: &[u8]) -> Result<Kind, String> {
    let d8 = data.get(..8);
    if *program == COMPUTE_BUDGET_PROGRAM_ID {
        return match data {
            [2, rest @ ..] if rest.len() == 4 => Ok(Kind::ComputeBudget),
            [3, rest @ ..] if rest.len() == 8 => {
                let price = u64::from_le_bytes(rest.try_into().unwrap_or_default());
                if price <= MAX_CU_PRICE {
                    Ok(Kind::ComputeBudget)
                } else {
                    Err(format!(
                        "priority fee of {price} micro-lamports per unit is over {MAX_CU_PRICE}"
                    ))
                }
            }
            _ => Err("only compute unit limit and price are allowed".into()),
        };
    }
    if *program == ED25519_PROGRAM_ID {
        return Ok(Kind::Ed25519);
    }
    if *program == MEMO_PROGRAM_ID {
        return Ok(Kind::Memo);
    }
    if *program == ASSOCIATED_TOKEN_PROGRAM_ID {
        return if data == [1] {
            Ok(Kind::CreateAta)
        } else {
            Err("only creating a token account (idempotent) is allowed".into())
        };
    }
    if *program == policy.schedina {
        let name = match d8.and_then(|d| <[u8; 8]>::try_from(d).ok()) {
            Some(disc::OPEN_PENDING) => return Ok(Kind::OpenPending),
            Some(disc::CASH_OUT) => return Ok(Kind::CashOut),
            Some(disc::FAUCET_USDC) => return Ok(Kind::FaucetUsdc),
            Some(disc::FINALIZE) => "finalize",
            Some(disc::REFUND_PENDING) => "refund_pending",
            Some(disc::RESOLVE_LEG) => "resolve_leg",
            Some(disc::SETTLE) => "settle",
            Some(disc::INIT_CONFIG) => "init_config",
            Some(disc::FUND_VAULT) => "fund_vault",
            Some(disc::PAUSE) => "pause",
            _ => "this schedina instruction",
        };
        return Err(format!("{name} is not relayed"));
    }
    if *program == policy.amm {
        return match d8.and_then(|d| <[u8; 8]>::try_from(d).ok()) {
            Some(amm_disc::SWAP) => Ok(Kind::Swap),
            Some(amm_disc::FAUCET) => Ok(Kind::AmmFaucet),
            _ => Err("only swap and the token faucet of the mock AMM are relayed".into()),
        };
    }
    if *program == crate::program::SYSTEM_PROGRAM_ID {
        return Err("system program instructions are not relayed".into());
    }
    Err(format!("program {program} is not relayed"))
}
