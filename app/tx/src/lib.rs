//! Schedina instructions as the app sends them (plan 2.E), built from the 1.C program's IDL
//! (`programs/idl/schedina.json`) and PDA seeds (`programs/schedina/src/constants.rs`).
//!
//! No web dependency: this crate is unit-tested on the host against
//! `fixtures/schedina/quote_vector.json` (byte-equal instruction data) and the IDL (account
//! order, signer and writable flags, PDA seeds), then linked into the wasm app.
//!
//! Transaction shape (L2-2, D-049): `[..., Ed25519Program verify, schedina instruction]`. The
//! verify carries exactly one signature with all three instruction-index fields `0xFFFF`; its
//! message is the desk's signed bytes, base64-decoded and copied untouched (never re-encoded).
//! This crate returns the last two instructions as one array, so they cannot be separated.
//!
//! Nothing here computes odds or payouts. [`QuoteMsgView`] and [`CashOutMsgView`] only read the
//! signed bytes back so the app can refuse to sign a message that differs from what it shows.

use std::fmt;

/// Sponsored transactions and the compile of every transaction the app sends (L8).
pub mod sponsor;
/// Pay in any token: `[swap, Ed25519 verify, open_pending]` (4.D).
pub mod swap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
pub use solana_instruction::{AccountMeta, Instruction};
pub use solana_pubkey::Pubkey;

/// The schedina program (`declare_id!` of 1.C, `programs/idl/schedina.json` `address`).
pub const PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("HXdZFi7tR669G1hXogh3uGGqLYsoZPCSuiutAviLNToW");
pub const ED25519_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("Ed25519SigVerify111111111111111111111111111");
pub const INSTRUCTIONS_SYSVAR_ID: Pubkey =
    Pubkey::from_str_const("Sysvar1nstructions1111111111111111111111111");
pub const TOKEN_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
pub const ATA_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
pub const SYSTEM_PROGRAM_ID: Pubkey = Pubkey::from_str_const("11111111111111111111111111111111");

/// Anchor discriminators, from the IDL.
pub const OPEN_PENDING_DISC: [u8; 8] = [33, 5, 249, 199, 254, 224, 125, 35];
pub const CASH_OUT_DISC: [u8; 8] = [1, 110, 57, 58, 159, 157, 243, 192];
pub const FAUCET_USDC_DISC: [u8; 8] = [190, 45, 226, 28, 94, 130, 98, 127];

/// Message tags (C3).
pub const QUOTE_TAG: &[u8; 17] = b"schedina/quote/v1";
pub const CASHOUT_TAG: &[u8; 19] = b"schedina/cashout/v1";
/// Quote message size: `77 + 85 n` (D-050).
pub const QUOTE_FIXED_LEN: usize = 77;
pub const QUOTE_LEG_LEN: usize = 85;
pub const CASHOUT_LEN: usize = 99;
/// Most legs the program's accounts hold (`constants::MAX_LEGS`).
pub const MAX_LEGS: usize = 5;

/// The Pending's `expires_at` is `now_s + PENDING_TTL_S` (L2-2; the program accepts
/// `now + 30 ..= now + 360`), where `now_s` is the **cluster** clock (the program compares it
/// with `Clock::unix_timestamp`), never the browser's: see [`clock_unix_timestamp`]. `now_s` is
/// read before the quote, so the wallet prompt has up to 315 s (was 45 of 30..=60, D-664).
pub const PENDING_TTL_S: i64 = 345;

/// The Clock sysvar, read by the app for the cluster's `unix_timestamp` (L3-4).
pub const CLOCK_SYSVAR_ID: Pubkey =
    Pubkey::from_str_const("SysvarC1ock11111111111111111111111111111111");
/// Clock sysvar account size: `slot u64 ‖ epoch_start_timestamp i64 ‖ epoch u64 ‖
/// leader_schedule_epoch u64 ‖ unix_timestamp i64` (bincode, little-endian).
pub const CLOCK_LEN: usize = 40;

/// `(slot, unix_timestamp)` of a Clock sysvar account's data. The timestamp is what the
/// program's `Clock::get()` sees, so the Pending's `expires_at` is taken from it rather than
/// from the browser clock (a skewed laptop or a restarted validator gave `BadExpiry`, L2).
pub fn clock_unix_timestamp(data: &[u8]) -> Result<(u64, i64), TxError> {
    if data.len() != CLOCK_LEN {
        return Err(TxError::Message(format!(
            "clock sysvar is {} bytes, expected {CLOCK_LEN}",
            data.len()
        )));
    }
    let slot = u64::from_le_bytes(data[0..8].try_into().expect("8 bytes"));
    let ts = i64::from_le_bytes(data[32..40].try_into().expect("8 bytes"));
    if ts <= 0 {
        return Err(TxError::Message("clock sysvar has no timestamp".into()));
    }
    Ok((slot, ts))
}

/// [`clock_unix_timestamp`] of the base64 `data[0]` that `getAccountInfo` returns with
/// `"encoding": "base64"`.
pub fn clock_unix_timestamp_b64(b64: &str) -> Result<(u64, i64), TxError> {
    let data = STANDARD
        .decode(b64)
        .map_err(|_| TxError::Base64("clock sysvar data"))?;
    clock_unix_timestamp(&data)
}

/// Why an instruction could not be built. Every variant means "do not sign".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxError {
    /// A base64 field is not standard base64 with padding.
    Base64(&'static str),
    /// A base58 key does not parse.
    Pubkey(&'static str),
    /// The signature is not 64 bytes.
    SignatureLength(usize),
    /// The signed message is not a quote / cash-out of this program, or disagrees with the
    /// JSON fields shown to the user.
    Message(String),
    /// The transaction is this many bytes, over the 1,232-byte packet limit.
    TooLarge(usize),
}

impl fmt::Display for TxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TxError::Base64(what) => write!(f, "{what} is not valid base64"),
            TxError::Pubkey(what) => write!(f, "{what} is not a valid address"),
            TxError::SignatureLength(n) => write!(f, "signature is {n} bytes, expected 64"),
            TxError::Message(m) => write!(f, "signed message rejected: {m}"),
            TxError::TooLarge(n) => write!(
                f,
                "transaction is {n} bytes, over the {}-byte limit",
                swap::PACKET_DATA_SIZE
            ),
        }
    }
}

impl std::error::Error for TxError {}

// ------------------------------------------------------------------------------------ PDAs

/// Program-derived addresses of the 1.C program (`constants.rs`). Nonces are u64 **LE**.
pub mod pda {
    use super::{ATA_PROGRAM_ID, PROGRAM_ID, Pubkey, TOKEN_PROGRAM_ID};

    fn find(seeds: &[&[u8]]) -> Pubkey {
        Pubkey::find_program_address(seeds, &PROGRAM_ID).0
    }

    /// `["config"]`.
    pub fn config() -> Pubkey {
        find(&[b"config"])
    }
    /// `["vault"]`.
    pub fn vault() -> Pubkey {
        find(&[b"vault"])
    }
    /// `["vault_usdc"]`: the house token account (`Vault.usdc_ata`, set by `init_config`).
    pub fn vault_usdc() -> Pubkey {
        find(&[b"vault_usdc"])
    }
    /// `["escrow_usdc"]`.
    pub fn escrow_usdc() -> Pubkey {
        find(&[b"escrow_usdc"])
    }
    /// `["mock", "USDC"]`: the mock USDC mint, `Config.usdc_mint` (D-052).
    pub fn usdc_mint() -> Pubkey {
        find(&[b"mock", b"USDC"])
    }
    /// `["pending", user, nonce u64 LE]`.
    pub fn pending(user: &Pubkey, nonce: u64) -> Pubkey {
        find(&[b"pending", user.as_ref(), &nonce.to_le_bytes()])
    }
    /// `["ticket", user, nonce u64 LE]` (same nonce as its Pending).
    pub fn ticket(user: &Pubkey, nonce: u64) -> Pubkey {
        find(&[b"ticket", user.as_ref(), &nonce.to_le_bytes()])
    }
    /// `["tipster", wallet]` (L2-4).
    pub fn tipster(user: &Pubkey) -> Pubkey {
        find(&[b"tipster", user.as_ref()])
    }
    /// `["faucet", owner]`.
    pub fn faucet_claim(owner: &Pubkey) -> Pubkey {
        find(&[b"faucet", owner.as_ref()])
    }
    /// Anchor `#[event_cpi]` authority, `["__event_authority"]`.
    pub fn event_authority() -> Pubkey {
        find(&[b"__event_authority"])
    }
    /// Associated token account of `owner` for `mint` (classic SPL Token).
    pub fn ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(
            &[owner.as_ref(), TOKEN_PROGRAM_ID.as_ref(), mint.as_ref()],
            &ATA_PROGRAM_ID,
        )
        .0
    }
    /// The user's mock-USDC account the app uses for stakes, payouts and the faucet.
    pub fn user_usdc(user: &Pubkey) -> Pubkey {
        ata(user, &usdc_mint())
    }
}

// --------------------------------------------------------------------- signed desk messages

/// A desk-signed message as carried on the wire (C4: `quote_signer` base58, `quote_msg` and
/// `quote_sig` standard base64 with padding), decoded once and kept as raw bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedMsg {
    pub signer: Pubkey,
    pub msg: Vec<u8>,
    pub sig: [u8; 64],
}

impl SignedMsg {
    pub fn from_wire(signer_b58: &str, msg_b64: &str, sig_b64: &str) -> Result<Self, TxError> {
        let signer: Pubkey = signer_b58
            .parse()
            .map_err(|_| TxError::Pubkey("quote_signer"))?;
        let msg = STANDARD
            .decode(msg_b64)
            .map_err(|_| TxError::Base64("quote_msg"))?;
        let sig_bytes = STANDARD
            .decode(sig_b64)
            .map_err(|_| TxError::Base64("quote_sig"))?;
        let sig: [u8; 64] = sig_bytes
            .as_slice()
            .try_into()
            .map_err(|_| TxError::SignatureLength(sig_bytes.len()))?;
        Ok(SignedMsg { signer, msg, sig })
    }
}

fn u32_at(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(d[at..at + 4].try_into().expect("4 bytes"))
}
fn u64_at(d: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(d[at..at + 8].try_into().expect("8 bytes"))
}
fn i64_at(d: &[u8], at: usize) -> i64 {
    i64::from_le_bytes(d[at..at + 8].try_into().expect("8 bytes"))
}

/// The fields of a signed quote message the app compares with what it shows. Read-only view
/// of the bytes (D-050 layout); the bytes themselves go into the transaction untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuoteMsgView {
    pub n: usize,
    pub legs: Vec<QuoteLegView>,
    pub stake_u: u64,
    pub odds_offered_u: u64,
    pub quote_expires_at: i64,
}

/// One signed leg: the hashes and side the program stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuoteLegView {
    pub symbol_hash: [u8; 32],
    pub game_id_hash: [u8; 32],
    /// 0 long, 1 short.
    pub side: u8,
    /// The game's start, Unix seconds, as signed.
    pub start_ts: i64,
}

impl QuoteMsgView {
    /// Checks tag, program id, `77 + 85 n` length, `1 <= n <= 5`.
    pub fn parse(msg: &[u8]) -> Result<Self, TxError> {
        let bad = |m: &str| Err(TxError::Message(m.to_string()));
        if msg.len() < QUOTE_FIXED_LEN || &msg[..17] != QUOTE_TAG {
            return bad("not a schedina quote");
        }
        if msg[17..49] != PROGRAM_ID.to_bytes() {
            return bad("quote signed for another program");
        }
        let n = u32_at(msg, 49) as usize;
        if n == 0 || n > MAX_LEGS {
            return bad("quote leg count out of range");
        }
        if msg.len() != QUOTE_FIXED_LEN + QUOTE_LEG_LEN * n {
            return bad("quote length does not match its leg count");
        }
        let legs = (0..n)
            .map(|i| {
                let at = 53 + QUOTE_LEG_LEN * i;
                QuoteLegView {
                    symbol_hash: msg[at..at + 32].try_into().expect("32 bytes"),
                    game_id_hash: msg[at + 32..at + 64].try_into().expect("32 bytes"),
                    side: msg[at + 72],
                    start_ts: i64_at(msg, at + 64),
                }
            })
            .collect();
        let tail = 53 + QUOTE_LEG_LEN * n;
        Ok(QuoteMsgView {
            n,
            legs,
            stake_u: u64_at(msg, tail),
            odds_offered_u: u64_at(msg, tail + 8),
            quote_expires_at: i64_at(msg, tail + 16),
        })
    }
}

/// The fields of a signed cash-out message (99 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CashOutMsgView {
    pub ticket: Pubkey,
    pub c_u: u64,
    pub ts: i64,
}

impl CashOutMsgView {
    /// Checks length, tag and program id.
    pub fn parse(msg: &[u8]) -> Result<Self, TxError> {
        let bad = |m: &str| Err(TxError::Message(m.to_string()));
        if msg.len() != CASHOUT_LEN || &msg[..19] != CASHOUT_TAG {
            return bad("not a schedina cash-out quote");
        }
        if msg[19..51] != PROGRAM_ID.to_bytes() {
            return bad("cash-out quote signed for another program");
        }
        let ticket = Pubkey::new_from_array(msg[51..83].try_into().expect("32 bytes"));
        Ok(CashOutMsgView {
            ticket,
            c_u: u64_at(msg, 83),
            ts: i64_at(msg, 91),
        })
    }
}

// ----------------------------------------------------------------------------- instructions

/// `Ed25519Program` verify of one signature, everything inside the instruction itself (all
/// three instruction indices `0xFFFF`), as solana's `new_ed25519_instruction_with_signature`
/// builds it: `[1, 0, sig_off, 0xFFFF, pk_off, 0xFFFF, msg_off, msg_len, 0xFFFF] ‖ pubkey ‖
/// signature ‖ message`.
pub fn ed25519_verify_ix(signed: &SignedMsg) -> Result<Instruction, TxError> {
    const HEADER: u16 = 16;
    const PK_OFF: u16 = HEADER;
    const SIG_OFF: u16 = PK_OFF + 32;
    const MSG_OFF: u16 = SIG_OFF + 64;
    let msg_len = u16::try_from(signed.msg.len())
        .ok()
        .filter(|l| usize::from(MSG_OFF) + usize::from(*l) <= usize::from(u16::MAX))
        .ok_or_else(|| TxError::Message("message too long".into()))?;
    let mut data = Vec::with_capacity(usize::from(MSG_OFF) + signed.msg.len());
    data.extend_from_slice(&[1, 0]);
    for v in [
        SIG_OFF,
        u16::MAX,
        PK_OFF,
        u16::MAX,
        MSG_OFF,
        msg_len,
        u16::MAX,
    ] {
        data.extend_from_slice(&v.to_le_bytes());
    }
    data.extend_from_slice(signed.signer.as_ref());
    data.extend_from_slice(&signed.sig);
    data.extend_from_slice(&signed.msg);
    Ok(Instruction {
        program_id: ED25519_PROGRAM_ID,
        accounts: Vec::new(),
        data,
    })
}

/// `open_pending` arguments (the quote itself is the verify instruction's message).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenPendingArgs {
    pub nonce: u64,
    pub min_payout_per_unit_u: u64,
    pub expires_at: i64,
    pub is_tip: bool,
    pub copied_from: Option<Pubkey>,
}

impl OpenPendingArgs {
    /// Borsh: `nonce u64 ‖ min_payout_per_unit_u u64 ‖ expires_at i64 ‖ is_tip u8 ‖
    /// copied_from Option<Pubkey>` after the discriminator.
    pub fn data(&self) -> Vec<u8> {
        let mut d = Vec::with_capacity(8 + 25 + 33);
        d.extend_from_slice(&OPEN_PENDING_DISC);
        d.extend_from_slice(&self.nonce.to_le_bytes());
        d.extend_from_slice(&self.min_payout_per_unit_u.to_le_bytes());
        d.extend_from_slice(&self.expires_at.to_le_bytes());
        d.push(u8::from(self.is_tip));
        match &self.copied_from {
            None => d.push(0),
            Some(k) => {
                d.push(1);
                d.extend_from_slice(k.as_ref());
            }
        }
        d
    }
}

/// `open_pending` with the IDL's account list, in order. `user_usdc` is the user's mock-USDC
/// token account (normally [`pda::user_usdc`]). `payer` pays the Pending's rent and gets it back
/// at close (L8): `user` itself, or the desk's sponsor key.
pub fn open_pending_ix(
    user: &Pubkey,
    payer: &Pubkey,
    user_usdc: &Pubkey,
    args: &OpenPendingArgs,
) -> Instruction {
    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(*user, true),
            AccountMeta::new_readonly(pda::config(), false),
            AccountMeta::new(pda::pending(user, args.nonce), false),
            AccountMeta::new(*user_usdc, false),
            AccountMeta::new(pda::escrow_usdc(), false),
            AccountMeta::new_readonly(INSTRUCTIONS_SYSVAR_ID, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(pda::event_authority(), false),
            AccountMeta::new_readonly(PROGRAM_ID, false),
        ],
        data: args.data(),
    }
}

/// One leg as the user saw it: the desk's symbol and game id strings and the side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownLeg {
    pub symbol: String,
    pub game_id: String,
    /// 0 long, 1 short.
    pub side: u8,
}

/// What the user saw before tapping "Gioca": the signed message must say the same.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownQuote {
    pub legs: Vec<ShownLeg>,
    pub stake_u: u64,
    pub odds_offered_u: u64,
    pub quote_expires_at: i64,
    pub min_payout_per_unit_u: u64,
}

/// What kind of schedina an `open_pending` opens (the instruction's `is_tip` and
/// `copied_from` arguments).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpenOptions {
    /// Share it as a tip: others may copy it and it counts on the tipster's record. Legs in
    /// play are fine (D-660); the program refuses (`TipNotAllowed`) only a tip that copies.
    pub is_tip: bool,
    /// The original Ticket PDA when this schedina copies a tip. The program checks the copy
    /// against the original at `finalize` (L2-4), not here.
    pub copied_from: Option<Pubkey>,
}

/// Largest nonce the app draws: 2^53 - 1, so every JavaScript consumer of the desk's JSON
/// (numbers are IEEE doubles) reads it exactly (L3 merge notes).
pub const NONCE_MAX: u64 = (1 << 53) - 1;

/// An `open_pending` nonce from 8 random bytes, limited to 53 bits ([`NONCE_MAX`]). 53 bits
/// keep a collision between one wallet's nonces negligible; a reused nonce can only be
/// refunded (D-054).
pub fn nonce_from_entropy(bytes: [u8; 8]) -> u64 {
    u64::from_le_bytes(bytes) & NONCE_MAX
}

fn sha256(s: &str) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(s.as_bytes()).into()
}

/// `[Ed25519 verify, open_pending]` for a desk quote. Refuses a message that is not a quote of
/// this program or disagrees with `shown` (each leg's symbol and game-id hashes and side, in
/// order; stake, offered odds, expiry), and a floor of zero or above the offered odds (the
/// program's `FloorAboveOffered`), and what the program would (`TipNotAllowed`): a tip
/// that copies. `payer` pays the Pending's rent ([`open_pending_ix`]).
pub fn open_pending_ixs(
    user: &Pubkey,
    payer: &Pubkey,
    signed: &SignedMsg,
    shown: &ShownQuote,
    nonce: u64,
    now_s: i64,
    opts: OpenOptions,
) -> Result<[Instruction; 2], TxError> {
    let view = QuoteMsgView::parse(&signed.msg)?;
    let same_legs = view.legs.len() == shown.legs.len()
        && view.legs.iter().zip(&shown.legs).all(|(v, s)| {
            v.side == s.side
                && v.symbol_hash == sha256(&s.symbol)
                && v.game_id_hash == sha256(&s.game_id)
        });
    let same = same_legs
        && view.stake_u == shown.stake_u
        && view.odds_offered_u == shown.odds_offered_u
        && view.quote_expires_at == shown.quote_expires_at;
    if !same {
        return Err(TxError::Message(
            "the signed quote differs from the one shown".into(),
        ));
    }
    if shown.min_payout_per_unit_u == 0 || shown.min_payout_per_unit_u > view.odds_offered_u {
        return Err(TxError::Message("minimum payout out of range".into()));
    }
    if opts.is_tip && opts.copied_from.is_some() {
        return Err(TxError::Message("a copy cannot be shared as a tip".into()));
    }
    if nonce > NONCE_MAX {
        return Err(TxError::Message("nonce above 53 bits".into()));
    }
    let args = OpenPendingArgs {
        nonce,
        min_payout_per_unit_u: shown.min_payout_per_unit_u,
        expires_at: now_s + PENDING_TTL_S,
        is_tip: opts.is_tip,
        copied_from: opts.copied_from,
    };
    Ok([
        ed25519_verify_ix(signed)?,
        open_pending_ix(user, payer, &pda::user_usdc(user), &args),
    ])
}

/// A copy's legs against its original's, as the program's `finalize` compares them
/// (`CopyMismatch`, D-114): the same count, and every copy leg matches an original leg in
/// symbol, game id and side (game ids are unique within a ticket, so this is set equality).
pub fn copy_matches(original: &[ShownLeg], copy: &[ShownLeg]) -> bool {
    original.len() == copy.len() && copy.iter().all(|c| original.contains(c))
}

/// L2-4: a ticket is an own tip when `tipster == user` and `copied_from` is null; then
/// `settle` / `cash_out` take the `["tipster", user]` profile as the first remaining account.
pub fn is_own_tip(user: &Pubkey, tipster: Option<&Pubkey>, copied_from: Option<&Pubkey>) -> bool {
    tipster == Some(user) && copied_from.is_none()
}

/// `cash_out(quote: CashOutMsg)`: data is the discriminator followed by the 99 signed bytes
/// (borsh of `CashOutMsg` is the message itself). `ticket` is the Ticket PDA; with `own_tip`
/// the tipster profile is appended as the first remaining account (writable).
pub fn cash_out_ix(user: &Pubkey, ticket: &Pubkey, msg: &[u8], own_tip: bool) -> Instruction {
    let mut data = Vec::with_capacity(8 + msg.len());
    data.extend_from_slice(&CASH_OUT_DISC);
    data.extend_from_slice(msg);
    let mint = pda::usdc_mint();
    let mut accounts = vec![
        AccountMeta::new_readonly(*user, true),
        AccountMeta::new_readonly(pda::config(), false),
        AccountMeta::new(pda::vault(), false),
        AccountMeta::new(*ticket, false),
        AccountMeta::new(pda::ata(user, &mint), false),
        AccountMeta::new_readonly(mint, false),
        AccountMeta::new(pda::vault_usdc(), false),
        AccountMeta::new_readonly(INSTRUCTIONS_SYSVAR_ID, false),
        AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
        AccountMeta::new_readonly(pda::event_authority(), false),
        AccountMeta::new_readonly(PROGRAM_ID, false),
    ];
    if own_tip {
        accounts.push(AccountMeta::new(pda::tipster(user), false));
    }
    Instruction {
        program_id: PROGRAM_ID,
        accounts,
        data,
    }
}

/// The ticket a cash-out is for, as the app knows it (C4 `Ticket`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CashOutTicket {
    /// `Ticket.user`; must be the connected wallet.
    pub user: Pubkey,
    pub nonce: u64,
    /// The Ticket PDA as the desk reported it.
    pub ticket: Pubkey,
    pub own_tip: bool,
}

/// `[Ed25519 verify, cash_out]`. Refuses if the desk's ticket address is not
/// `["ticket", user, nonce]`, if the signed message is for another ticket or program, or if
/// its amount differs from the one shown (`shown_c_u`).
pub fn cash_out_ixs(
    t: &CashOutTicket,
    signed: &SignedMsg,
    shown_c_u: u64,
) -> Result<[Instruction; 2], TxError> {
    if pda::ticket(&t.user, t.nonce) != t.ticket {
        return Err(TxError::Message(
            "ticket address does not match its owner and number".into(),
        ));
    }
    let view = CashOutMsgView::parse(&signed.msg)?;
    if view.ticket != t.ticket {
        return Err(TxError::Message(
            "cash-out quote is for another ticket".into(),
        ));
    }
    if view.c_u != shown_c_u {
        return Err(TxError::Message(
            "the signed amount differs from the one shown".into(),
        ));
    }
    Ok([
        ed25519_verify_ix(signed)?,
        cash_out_ix(&t.user, &t.ticket, &signed.msg, t.own_tip),
    ])
}

/// Associated Token Account program `CreateIdempotent` (instruction 1): creates `owner`'s ATA
/// for `mint` if absent, paid by `payer`.
pub fn create_ata_idempotent_ix(payer: &Pubkey, owner: &Pubkey, mint: &Pubkey) -> Instruction {
    Instruction {
        program_id: ATA_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(pda::ata(owner, mint), false),
            AccountMeta::new_readonly(*owner, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
        ],
        data: vec![1],
    }
}

/// `faucet_usdc(amount)` into `recipient` (a mock-USDC token account owned by
/// `recipient_owner`). The program has it only in `devnet` builds.
pub fn faucet_usdc_ix(
    caller: &Pubkey,
    recipient: &Pubkey,
    recipient_owner: &Pubkey,
    amount: u64,
) -> Instruction {
    let mut data = FAUCET_USDC_DISC.to_vec();
    data.extend_from_slice(&amount.to_le_bytes());
    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*caller, true),
            AccountMeta::new_readonly(pda::config(), false),
            AccountMeta::new(pda::usdc_mint(), false),
            AccountMeta::new(*recipient, false),
            AccountMeta::new(pda::faucet_claim(recipient_owner), false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        ],
        data,
    }
}

/// Mock USDC per faucet tap: €1,000 (the program allows up to 10,000 a claim, with no cooldown
/// since D-661).
pub const FAUCET_AMOUNT_U: u64 = 1_000_000_000;

/// SPL Memo program v2.
pub const MEMO_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");

/// The memo a sponsored faucet claim carries, signed by the user (L8).
pub const FAUCET_MEMO: &[u8] = b"schedina:faucet";

/// SPL Memo `memo`, which fails unless every account in `signers` signed the transaction.
pub fn memo_ix(memo: &[u8], signers: &[Pubkey]) -> Instruction {
    Instruction {
        program_id: MEMO_PROGRAM_ID,
        accounts: signers
            .iter()
            .map(|k| AccountMeta::new_readonly(*k, true))
            .collect(),
        data: memo.to_vec(),
    }
}

/// `[create the user's mock-USDC account if absent, faucet_usdc(amount)]`, both paid by `payer`
/// (the faucet's `caller`; the claim is keyed on the user, the account's owner). When `payer`
/// is not the user (L8: the desk's sponsor), a [`FAUCET_MEMO`] signed by the user follows, so
/// the user still signs their own claim.
pub fn faucet_ixs(user: &Pubkey, payer: &Pubkey, amount: u64) -> Vec<Instruction> {
    let mint = pda::usdc_mint();
    let mut ixs = vec![
        create_ata_idempotent_ix(payer, user, &mint),
        faucet_usdc_ix(payer, &pda::ata(user, &mint), user, amount),
    ];
    if payer != user {
        ixs.push(memo_ix(FAUCET_MEMO, &[*user]));
    }
    ixs
}

/// The program's custom error for an Anchor error code (6000 + index), in plain words, for
/// messages the user may see after a failed simulation. `None` for codes the user cannot act on.
pub fn program_error_words(code: u32) -> Option<&'static str> {
    Some(match code {
        6005 => "Schedina is paused right now",
        6008..=6012 => "the desk signature did not check out",
        6015 => "the quote expired before the transaction landed",
        6017 => "two picks on the same game",
        6019 => "stake below the minimum",
        6020 => "the minimum payout is above the offered odds",
        6021 => "payout above the house limit",
        // The app takes `now` from the cluster clock (L3-4), so only a slow landing is left.
        6022 => "the transaction took more than 15 seconds to land: tap Gioca again",
        // Since D-660 only a copy shared as a tip (a leg in play is fine).
        6023 => "a copy cannot be shared as a tip",
        6032 => "this schedina is no longer open",
        // Own tips pass `["tipster", user]` on cash-out (L2-4); refused without it.
        6044 => "the tipster record of this tip did not check out",
        6037 => "the cash-out offer is for another schedina",
        6038 => "the cash-out offer expired",
        6039 => "the cash-out offer is above the payout",
        6040 => "a pick is lost: no cash-out",
        // A program deployed before D-661 (no cooldown) still answers it.
        6043 => "the faucet gives test USDC once an hour",
        // Anchor's InstructionFallbackNotFound: `faucet_usdc` exists only in `devnet` builds.
        101 => "this deployment has no test USDC faucet",
        // Anchor's AccountNotInitialized: the user has no mock-USDC account yet.
        3012 => "no test USDC yet: tap \u{201c}Get test USDC\u{201d}",
        _ => return None,
    })
}

/// The custom program error code in an RPC or wallet error text, as the node reports a failed
/// simulation: `custom program error: 0x177f`, `{"Custom":6015}` or `Custom(6015)`.
pub fn custom_error_code(text: &str) -> Option<u32> {
    const HEX: &str = "custom program error: 0x";
    if let Some(i) = text.find(HEX) {
        let hex: String = text[i + HEX.len()..]
            .chars()
            .take_while(char::is_ascii_hexdigit)
            .collect();
        return u32::from_str_radix(&hex, 16).ok();
    }
    for pat in ["\"Custom\":", "Custom(", "Custom: "] {
        if let Some(i) = text.find(pat) {
            let dec: String = text[i + pat.len()..]
                .trim_start()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if let Ok(code) = dec.parse() {
                return Some(code);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_custom_error_codes() {
        assert_eq!(
            custom_error_code(
                "simulation failed: Error processing Instruction 1: custom program error: 0x177f"
            ),
            Some(6015)
        );
        assert_eq!(
            custom_error_code(r#"{"InstructionError":[1,{"Custom":6022}]}"#),
            Some(6022)
        );
        assert_eq!(
            custom_error_code("InstructionError(1, Custom(6040))"),
            Some(6040)
        );
        assert_eq!(custom_error_code("blockhash not found"), None);
        assert_eq!(
            program_error_words(6015),
            Some("the quote expired before the transaction landed")
        );
        assert_eq!(program_error_words(6000), None);
    }

    /// A real devnet Clock sysvar (`getAccountInfo`, base64, `processed`, 29 Oct 2026, context
    /// slot 516236249), decoded with Python's `struct.unpack('<QqQQq', …)`.
    #[test]
    fn reads_the_cluster_clock() {
        let b64 = "2SPFHgAAAABeBOFqAAAAAKoEAAAAAAAAqwQAAAAAAAAllOJqAAAAAA==";
        assert_eq!(
            clock_unix_timestamp_b64(b64),
            Ok((516_236_249, 1_793_233_957))
        );
        assert!(clock_unix_timestamp(&[0u8; 39]).is_err());
        assert!(clock_unix_timestamp(&[0u8; CLOCK_LEN]).is_err());
        assert_eq!(
            clock_unix_timestamp_b64("not base64!"),
            Err(TxError::Base64("clock sysvar data"))
        );
        assert_eq!(
            CLOCK_SYSVAR_ID.to_string(),
            "SysvarC1ock11111111111111111111111111111111"
        );
    }
}
