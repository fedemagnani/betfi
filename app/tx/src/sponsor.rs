//! Sponsored transactions (L8, contract L8-2): the desk's sponsor key is the fee payer (static
//! key 0) and the rent payer, the user only signs (key 1), and the desk's `POST /relay` adds the
//! sponsor's signature and submits. A user never needs SOL.
//!
//! Every transaction the app sends is compiled here, self-paid or sponsored, legacy or v0, into
//! the wire format the wallet signs (`compact(n) ‖ n zeroed signatures ‖ message`, the bincode of
//! a `VersionedTransaction`). [`check_sponsored`] and [`check_self_paid`] hold the shape each
//! path needs before the wallet opens.
//!
//! Sizes (two compute-budget instructions, `copied_from`): a sponsored bet fits a legacy message
//! up to 4 legs; 5 legs need v0 with `AmmConfig.lookup_table` (1,300 B legacy, 1,150 B v0).

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use solana_message::{AddressLookupTableAccount, Message, VersionedMessage, v0};

use super::swap::PACKET_DATA_SIZE;
use super::{Instruction, Pubkey, TxError};

/// An address lookup table as read from chain: its key and its addresses.
pub type Table = (Pubkey, Vec<Pubkey>);

/// How a transaction is compiled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Format {
    /// A legacy message.
    Legacy,
    /// A v0 message, with the lookup table when there is one (only the addresses the
    /// instructions use are taken from it; signers and invoked programs stay static).
    V0(Option<Table>),
}

/// How the app asks for a transaction to be compiled; [`Compile::format`] picks the format once
/// the payer is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Compile {
    /// A legacy message (faucets, cash-out).
    Legacy,
    /// A bet: legacy when self-paid (as before L8); sponsored, v0 with the AMM's lookup table
    /// when it has one (a sponsored 5-leg bet does not fit a legacy message), else legacy.
    Bet(Option<Table>),
    /// Always v0, with the lookup table when there is one (the swap bet).
    V0(Option<Table>),
}

impl Compile {
    /// The format of this transaction, sponsored or self-paid.
    pub fn format(&self, sponsored: bool) -> Format {
        match self {
            Compile::Legacy => Format::Legacy,
            Compile::Bet(Some(t)) if sponsored => Format::V0(Some(t.clone())),
            Compile::Bet(_) => Format::Legacy,
            Compile::V0(t) => Format::V0(t.clone()),
        }
    }
}

/// Why a sponsored send did not go through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotSponsored {
    /// `503`: sponsoring is off or the desk's RPC is down. Nothing was sent.
    Unavailable,
    /// Over the packet limit with two signers. Nothing was signed.
    TooLarge,
    /// The wallet can only sign-and-send, not sign a slot of a transaction another key pays.
    CannotCoSign,
    /// `422`: the desk refused this transaction (its rules, or the RPC's preflight).
    Refused,
    /// No clean answer from the desk (network): it may have sent the transaction.
    NoAnswer,
}

/// A wallet holding less than this does not pay a transaction itself when sponsoring fails:
/// 0.01 SOL covers a fee and a Pending's or a token account's rent with room to spare.
pub const SELF_PAY_MIN_LAMPORTS: u64 = 10_000_000;

/// True when the wallet pays the transaction itself after `why`: only when nothing can have
/// been sent and the desk did not refuse it, and only with [`SELF_PAY_MIN_LAMPORTS`]. Otherwise
/// the error stands (L8-2: "Sponsored transactions are unavailable right now", the desk's reason).
pub fn self_pay_after(why: NotSponsored, lamports: u64) -> bool {
    matches!(
        why,
        NotSponsored::Unavailable | NotSponsored::TooLarge | NotSponsored::CannotCoSign
    ) && lamports >= SELF_PAY_MIN_LAMPORTS
}

/// Compute budget program.
pub const COMPUTE_BUDGET_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("ComputeBudget111111111111111111111111111111");
/// The priority fee a sponsored transaction carries, micro-lamports per compute unit: under the
/// relay's cap (desk `relay::rules::MAX_CU_PRICE`, 10,000).
pub const SPONSORED_CU_PRICE: u64 = 1_000;
/// The runtime's default compute units per instruction, and its per-transaction ceiling.
const DEFAULT_CU_PER_IX: u32 = 200_000;
const MAX_CU: u32 = 1_400_000;

/// `ixs` behind an explicit compute budget, for a sponsored transaction. A wallet that finds no
/// budget adds its own priority fee while signing (Phantom set 375,000 micro-lamports per unit),
/// which the relay refuses; one that finds a budget leaves it alone. The limit is the runtime's
/// default for these instructions (200,000 each, at most 1,400,000), so nothing gets fewer units
/// than it would without one. Instructions that already set a budget are returned as they are.
pub fn with_compute_budget(mut ixs: Vec<Instruction>) -> Vec<Instruction> {
    if ixs
        .iter()
        .any(|ix| ix.program_id == COMPUTE_BUDGET_PROGRAM_ID)
    {
        return ixs;
    }
    let n = u32::try_from(ixs.len()).unwrap_or(u32::MAX);
    let limit = n.saturating_mul(DEFAULT_CU_PER_IX).min(MAX_CU);
    let budget = |data: Vec<u8>| Instruction {
        program_id: COMPUTE_BUDGET_PROGRAM_ID,
        accounts: vec![],
        data,
    };
    let set_limit = budget([&[2u8][..], &limit.to_le_bytes()].concat());
    let set_price = budget([&[3u8][..], &SPONSORED_CU_PRICE.to_le_bytes()].concat());
    ixs.splice(0..0, [set_limit, set_price]);
    ixs
}

/// The sponsored message of `ixs`: behind [`with_compute_budget`] when that still fits the
/// packet, else as it is (a first-time user's 5-leg swap bet with its create-ATA is 1,262 B with
/// the budget; such a bet relays from a wallet that adds no budget of its own, as before).
pub fn compile_sponsored(
    sponsor: &Pubkey,
    ixs: Vec<Instruction>,
    format: &Format,
    blockhash: [u8; 32],
) -> Result<VersionedMessage, TxError> {
    let budgeted = compile_message(
        sponsor,
        &with_compute_budget(ixs.clone()),
        format,
        blockhash,
    )?;
    if wire(&budgeted).is_ok() {
        return Ok(budgeted);
    }
    compile_message(sponsor, &ixs, format, blockhash)
}

/// The message of `ixs` paid by `payer` on `blockhash`, in `format`.
pub fn compile_message(
    payer: &Pubkey,
    ixs: &[Instruction],
    format: &Format,
    blockhash: [u8; 32],
) -> Result<VersionedMessage, TxError> {
    let hash = solana_hash::Hash::new_from_array(blockhash);
    match format {
        Format::Legacy => Ok(VersionedMessage::Legacy(Message::new_with_blockhash(
            ixs,
            Some(payer),
            &hash,
        ))),
        Format::V0(table) => {
            let tables: Vec<AddressLookupTableAccount> = table
                .iter()
                .map(|(key, addresses)| AddressLookupTableAccount {
                    key: *key,
                    addresses: addresses.clone(),
                })
                .collect();
            v0::Message::try_compile(payer, ixs, &tables, hash)
                .map(VersionedMessage::V0)
                .map_err(|e| TxError::Message(format!("cannot compile the transaction: {e}")))
        }
    }
}

/// The unsigned transaction of `msg` in the wire format the wallet signs: `compact(n) ‖ n
/// zeroed signatures ‖ message` (a v0 message starts with `0x80`). Refuses a transaction over
/// [`PACKET_DATA_SIZE`].
pub fn wire(msg: &VersionedMessage) -> Result<Vec<u8>, TxError> {
    let n = msg.header().num_required_signatures;
    if n >= 0x80 {
        return Err(TxError::Message("too many signers".into()));
    }
    let body = msg.serialize();
    let mut out = Vec::with_capacity(1 + 64 * usize::from(n) + body.len());
    out.push(n);
    out.resize(1 + 64 * usize::from(n), 0);
    out.extend_from_slice(&body);
    if out.len() > PACKET_DATA_SIZE {
        return Err(TxError::TooLarge(out.len()));
    }
    Ok(out)
}

/// [`compile_message`] then [`wire`].
pub fn compile(
    payer: &Pubkey,
    ixs: &[Instruction],
    format: &Format,
    blockhash: [u8; 32],
) -> Result<Vec<u8>, TxError> {
    wire(&compile_message(payer, ixs, format, blockhash)?)
}

/// True when static key `i` of `msg` is a read-only signer.
pub fn is_readonly_signer(msg: &VersionedMessage, i: usize) -> bool {
    let h = msg.header();
    let required = usize::from(h.num_required_signatures);
    let readonly = usize::from(h.num_readonly_signed_accounts);
    i < required && i >= required - readonly
}

/// What the desk relays (L8-2): exactly two signers, the sponsor as fee payer (static key 0) and
/// the user (key 1), and the user a read-only signer, so the transaction cannot move the user's
/// SOL (every user meta the IDLs name is read-only: `open_pending`, `cash_out`, the mock AMM's
/// `swap` and `faucet`, the faucet memo).
pub fn check_sponsored(
    msg: &VersionedMessage,
    sponsor: &Pubkey,
    user: &Pubkey,
) -> Result<(), TxError> {
    let bad = |m: &str| Err(TxError::Message(m.to_string()));
    if sponsor == user {
        return bad("the sponsor is the user");
    }
    let keys = msg.static_account_keys();
    if msg.header().num_required_signatures != 2 || keys.len() < 2 {
        return bad("a sponsored transaction has exactly two signers");
    }
    if keys[0] != *sponsor {
        return bad("the sponsor is not the fee payer");
    }
    if keys[1] != *user {
        return bad("the wallet is not the second signer");
    }
    if !is_readonly_signer(msg, 1) {
        return bad("the transaction could spend the wallet's SOL");
    }
    Ok(())
}

/// What the app sends through its own RPC: the user is the fee payer and the only signer (the
/// node would refuse a missing signature).
pub fn check_self_paid(msg: &VersionedMessage, user: &Pubkey) -> Result<(), TxError> {
    let keys = msg.static_account_keys();
    if msg.header().num_required_signatures != 1 || keys.first() != Some(user) {
        return Err(TxError::Message(
            "the wallet is not the fee payer and only signer".into(),
        ));
    }
    Ok(())
}

/// The `tx` field of `POST /relay`: standard base64 of the wire bytes.
pub fn relay_tx_b64(wire: &[u8]) -> String {
    STANDARD.encode(wire)
}
