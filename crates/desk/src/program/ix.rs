//! Instruction builders, in the IDL's account order with its signer and writable flags
//! (checked by the tests against `programs/idl/schedina.json`). `remaining_accounts` follow
//! l2-contracts L2-4 so the same calls keep working once 2.F lands.

use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use super::accounts::{PendingAccount, QuoteLeg, TicketAccount};
use super::consts::QUOTE_TAG;
use super::{
    ASSOCIATED_TOKEN_PROGRAM_ID, ED25519_PROGRAM_ID, INSTRUCTIONS_SYSVAR_ID, Pdas,
    SYSTEM_PROGRAM_ID, TOKEN_PROGRAM_ID, associated_token_address,
};

/// Anchor instruction discriminators (IDL `instructions[].discriminator`).
pub mod disc {
    /// `init_config`.
    pub const INIT_CONFIG: [u8; 8] = [23, 235, 115, 232, 168, 96, 1, 231];
    /// `fund_vault`.
    pub const FUND_VAULT: [u8; 8] = [26, 33, 207, 242, 119, 108, 134, 73];
    /// `faucet_usdc` (devnet builds).
    pub const FAUCET_USDC: [u8; 8] = [190, 45, 226, 28, 94, 130, 98, 127];
    /// `open_pending`.
    pub const OPEN_PENDING: [u8; 8] = [33, 5, 249, 199, 254, 224, 125, 35];
    /// `finalize`.
    pub const FINALIZE: [u8; 8] = [171, 61, 218, 56, 127, 115, 12, 217];
    /// `refund_pending`.
    pub const REFUND_PENDING: [u8; 8] = [70, 207, 125, 172, 197, 218, 120, 112];
    /// `resolve_leg`.
    pub const RESOLVE_LEG: [u8; 8] = [178, 46, 116, 78, 252, 201, 13, 205];
    /// `settle`.
    pub const SETTLE: [u8; 8] = [175, 42, 185, 87, 144, 131, 102, 212];
    /// `cash_out`.
    pub const CASH_OUT: [u8; 8] = [1, 110, 57, 58, 159, 157, 243, 192];
    /// `pause`.
    pub const PAUSE: [u8; 8] = [211, 22, 221, 251, 74, 121, 193, 47];
}

/// `init_config`'s `Params` (borsh in this order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Params {
    /// Crank key.
    pub crank: Pubkey,
    /// Quote signer.
    pub quote_signer: Pubkey,
    /// Legs per ticket, 1..=5.
    pub max_legs: u8,
    /// Largest payout, micro-USDC.
    pub max_payout: u64,
    /// Smallest stake, micro-USDC.
    pub min_stake: u64,
    /// `withdraw_house` floor.
    pub reserve: u64,
    /// Copy fee, bps.
    pub copy_fee_bps: u16,
    /// Tipster share of the copy fee, bps.
    pub tipster_share_bps: u16,
}

impl Params {
    /// Borsh bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(32 * 2 + 1 + 24 + 4);
        v.extend_from_slice(self.crank.as_ref());
        v.extend_from_slice(self.quote_signer.as_ref());
        v.push(self.max_legs);
        v.extend_from_slice(&self.max_payout.to_le_bytes());
        v.extend_from_slice(&self.min_stake.to_le_bytes());
        v.extend_from_slice(&self.reserve.to_le_bytes());
        v.extend_from_slice(&self.copy_fee_bps.to_le_bytes());
        v.extend_from_slice(&self.tipster_share_bps.to_le_bytes());
        v
    }
}

/// The desk's signed quote (C3, D-050): borsh of `tag ‖ program id ‖ Vec<QuoteLeg> ‖ stake_u ‖
/// odds_offered_u ‖ quote_expires_at`, `77 + 85 n` bytes. The quoter (2.A) owns production
/// signing; the crank's tests and tools encode with this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuoteMsg {
    /// Program id (domain separation).
    pub program_id: Pubkey,
    /// Legs in ticket order.
    pub legs: Vec<QuoteLeg>,
    /// Stake.
    pub stake_u: u64,
    /// Offered odds.
    pub odds_offered_u: u64,
    /// Desk TTL, Unix s.
    pub quote_expires_at: i64,
}

impl QuoteMsg {
    /// The exact bytes the desk signs.
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(77 + 85 * self.legs.len());
        v.extend_from_slice(&QUOTE_TAG);
        v.extend_from_slice(self.program_id.as_ref());
        v.extend_from_slice(
            &u32::try_from(self.legs.len())
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );
        for l in &self.legs {
            l.write(&mut v);
        }
        v.extend_from_slice(&self.stake_u.to_le_bytes());
        v.extend_from_slice(&self.odds_offered_u.to_le_bytes());
        v.extend_from_slice(&self.quote_expires_at.to_le_bytes());
        v
    }
}

fn data(d: [u8; 8], args: &[&[u8]]) -> Vec<u8> {
    let mut v = d.to_vec();
    for a in args {
        v.extend_from_slice(a);
    }
    v
}

fn w(k: Pubkey) -> AccountMeta {
    AccountMeta::new(k, false)
}

fn r(k: Pubkey) -> AccountMeta {
    AccountMeta::new_readonly(k, false)
}

fn signer(k: Pubkey, writable: bool) -> AccountMeta {
    if writable {
        AccountMeta::new(k, true)
    } else {
        AccountMeta::new_readonly(k, true)
    }
}

/// `init_config(authority, params)`, signed by the program's upgrade authority (`deployer`,
/// who pays rent).
pub fn init_config(
    p: &Pdas,
    deployer: &Pubkey,
    authority: &Pubkey,
    params: &Params,
) -> Instruction {
    Instruction {
        program_id: p.program,
        accounts: vec![
            signer(*deployer, true),
            w(p.config()),
            w(p.vault()),
            w(p.usdc_mint()),
            w(p.vault_usdc()),
            w(p.escrow_usdc()),
            r(p.program),
            r(p.program_data()),
            r(TOKEN_PROGRAM_ID),
            r(SYSTEM_PROGRAM_ID),
        ],
        data: data(disc::INIT_CONFIG, &[authority.as_ref(), &params.encode()]),
    }
}

/// `fund_vault(amount)` from `funder_usdc` (a mock-USDC account `funder` owns).
pub fn fund_vault(p: &Pdas, funder: &Pubkey, funder_usdc: &Pubkey, amount: u64) -> Instruction {
    Instruction {
        program_id: p.program,
        accounts: vec![
            signer(*funder, false),
            r(p.config()),
            w(p.vault()),
            r(p.usdc_mint()),
            w(*funder_usdc),
            w(p.vault_usdc()),
            r(TOKEN_PROGRAM_ID),
            r(p.event_authority()),
            r(p.program),
        ],
        data: data(disc::FUND_VAULT, &[&amount.to_le_bytes()]),
    }
}

/// `faucet_usdc(amount)` into `owner`'s mock-USDC ATA (devnet builds only), paid by `caller`.
/// The rate limit is per recipient owner (`["faucet", owner]`).
pub fn faucet_usdc(p: &Pdas, caller: &Pubkey, owner: &Pubkey, amount: u64) -> Instruction {
    let mint = p.usdc_mint();
    Instruction {
        program_id: p.program,
        accounts: vec![
            signer(*caller, true),
            r(p.config()),
            w(mint),
            w(associated_token_address(owner, &mint)),
            w(p.faucet_claim(owner)),
            r(TOKEN_PROGRAM_ID),
            r(SYSTEM_PROGRAM_ID),
        ],
        data: data(disc::FAUCET_USDC, &[&amount.to_le_bytes()]),
    }
}

/// `open_pending(nonce, min_payout_per_unit_u, expires_at, is_tip, copied_from)` from `user`'s
/// mock-USDC ATA. Must come right after [`ed25519_verify`] of the signed quote (D-049). The app
/// (2.E) builds its own; this one serves the crank's tests and tools. `payer` pays the
/// Pending's rent and gets it back at close (L8): `user` itself, or the desk's sponsor key.
#[allow(clippy::too_many_arguments)]
pub fn open_pending(
    p: &Pdas,
    user: &Pubkey,
    payer: &Pubkey,
    nonce: u64,
    min_payout_per_unit_u: u64,
    expires_at: i64,
    is_tip: bool,
    copied_from: Option<&Pubkey>,
) -> Instruction {
    let mut args = Vec::with_capacity(8 + 8 + 8 + 1 + 33);
    args.extend_from_slice(&nonce.to_le_bytes());
    args.extend_from_slice(&min_payout_per_unit_u.to_le_bytes());
    args.extend_from_slice(&expires_at.to_le_bytes());
    args.push(u8::from(is_tip));
    match copied_from {
        None => args.push(0),
        Some(k) => {
            args.push(1);
            args.extend_from_slice(k.as_ref());
        }
    }
    Instruction {
        program_id: p.program,
        accounts: vec![
            signer(*user, false),
            r(p.config()),
            w(p.pending(user, nonce)),
            w(associated_token_address(user, &p.usdc_mint())),
            w(p.escrow_usdc()),
            r(INSTRUCTIONS_SYSVAR_ID),
            r(TOKEN_PROGRAM_ID),
            r(SYSTEM_PROGRAM_ID),
            signer(*payer, true),
            r(p.event_authority()),
            r(p.program),
        ],
        data: data(disc::OPEN_PENDING, &[&args]),
    }
}

/// `finalize`'s `remaining_accounts` (L2-4): `[tipster_profile (w)]` for an own tip,
/// `[orig_ticket (r), tipster_usdc (w)]` for a copy of `orig` whose tipster is `tipster`,
/// otherwise none.
pub fn finalize_remaining(
    p: &Pdas,
    pending: &PendingAccount,
    copy_tipster: Option<&Pubkey>,
) -> Vec<AccountMeta> {
    match (pending.copied_from, copy_tipster) {
        (Some(orig), Some(tipster)) => vec![
            r(orig),
            w(associated_token_address(tipster, &p.usdc_mint())),
        ],
        _ if pending.is_tip => vec![w(p.tipster(&pending.user))],
        _ => Vec::new(),
    }
}

/// `finalize(fills, r_ppm)` of `pending` (PDA `pending_key`), signed and paid by the crank.
/// The refund goes to the user's mock-USDC ATA (prepend [`create_ata_idempotent`]).
pub fn finalize(
    p: &Pdas,
    crank: &Pubkey,
    pending_key: &Pubkey,
    pending: &PendingAccount,
    fills: &[u64],
    r_ppm: u32,
    remaining: Vec<AccountMeta>,
) -> Instruction {
    let mint = p.usdc_mint();
    let mut args = Vec::with_capacity(4 + 8 * fills.len() + 4);
    args.extend_from_slice(&u32::try_from(fills.len()).unwrap_or(u32::MAX).to_le_bytes());
    for f in fills {
        args.extend_from_slice(&f.to_le_bytes());
    }
    args.extend_from_slice(&r_ppm.to_le_bytes());
    let mut accounts = vec![
        signer(*crank, true),
        r(p.config()),
        w(p.vault()),
        w(*pending_key),
        w(p.ticket(&pending.user, pending.nonce)),
        w(pending.rent_receiver()),
        w(associated_token_address(&pending.user, &mint)),
        r(mint),
        w(p.escrow_usdc()),
        w(p.vault_usdc()),
        r(TOKEN_PROGRAM_ID),
        r(SYSTEM_PROGRAM_ID),
        r(p.event_authority()),
        r(p.program),
    ];
    accounts.extend(remaining);
    Instruction {
        program_id: p.program,
        accounts,
        data: data(disc::FINALIZE, &[&args]),
    }
}

/// `refund_pending()` of `user`'s pending `nonce`, signed by `caller` (anyone). The escrow goes
/// back to the user's mock-USDC ATA (prepend [`create_ata_idempotent`]); the rent to `rent_to`,
/// the Pending's [`PendingAccount::rent_receiver`] (L8).
///
/// [`PendingAccount::rent_receiver`]: super::accounts::PendingAccount::rent_receiver
pub fn refund_pending(
    p: &Pdas,
    caller: &Pubkey,
    user: &Pubkey,
    nonce: u64,
    rent_to: &Pubkey,
) -> Instruction {
    let mint = p.usdc_mint();
    Instruction {
        program_id: p.program,
        accounts: vec![
            signer(*caller, false),
            r(p.config()),
            w(p.pending(user, nonce)),
            w(*rent_to),
            w(associated_token_address(user, &mint)),
            r(mint),
            w(p.escrow_usdc()),
            r(TOKEN_PROGRAM_ID),
            r(p.event_authority()),
            r(p.program),
        ],
        data: data(disc::REFUND_PENDING, &[]),
    }
}

/// `resolve_leg(leg_index, r_ppm)` on `ticket`, signed by the crank (or the authority).
pub fn resolve_leg(
    p: &Pdas,
    resolver: &Pubkey,
    ticket: &Pubkey,
    leg_index: u8,
    r_ppm: u32,
) -> Instruction {
    Instruction {
        program_id: p.program,
        accounts: vec![
            signer(*resolver, false),
            r(p.config()),
            w(*ticket),
            r(p.event_authority()),
            r(p.program),
        ],
        data: data(disc::RESOLVE_LEG, &[&[leg_index], &r_ppm.to_le_bytes()]),
    }
}

/// `settle()` of `ticket` (PDA `ticket_key`), signed by `caller` (anyone). Pays the user's
/// mock-USDC ATA (prepend [`create_ata_idempotent`]). An own tip passes its tipster profile
/// (L2-4).
pub fn settle(
    p: &Pdas,
    caller: &Pubkey,
    ticket_key: &Pubkey,
    ticket: &TicketAccount,
) -> Instruction {
    let mint = p.usdc_mint();
    let mut accounts = vec![
        signer(*caller, false),
        r(p.config()),
        w(p.vault()),
        w(*ticket_key),
        w(associated_token_address(&ticket.user, &mint)),
        r(mint),
        w(p.vault_usdc()),
        r(TOKEN_PROGRAM_ID),
        r(p.event_authority()),
        r(p.program),
    ];
    if ticket.is_own_tip() {
        accounts.push(w(p.tipster(&ticket.user)));
    }
    Instruction {
        program_id: p.program,
        accounts,
        data: data(disc::SETTLE, &[]),
    }
}

/// `cash_out(quote)` of `ticket` (PDA `ticket_key`), signed by the ticket's user. `quote_msg` is
/// the desk's signed 99-byte cash-out message (`CashOutMsg`, C3), copied untouched from
/// `GET /cashout-quote` (L2-2): its borsh bytes are the instruction's argument. Must come right
/// after [`ed25519_verify`] of the same bytes. Pays the user's mock-USDC ATA; an own tip passes
/// its tipster profile (L2-4).
pub fn cash_out(
    p: &Pdas,
    ticket_key: &Pubkey,
    ticket: &TicketAccount,
    quote_msg: &[u8],
) -> Instruction {
    let mint = p.usdc_mint();
    let mut accounts = vec![
        signer(ticket.user, false),
        r(p.config()),
        w(p.vault()),
        w(*ticket_key),
        w(associated_token_address(&ticket.user, &mint)),
        r(mint),
        w(p.vault_usdc()),
        r(INSTRUCTIONS_SYSVAR_ID),
        r(TOKEN_PROGRAM_ID),
        r(p.event_authority()),
        r(p.program),
    ];
    if ticket.is_own_tip() {
        accounts.push(w(p.tipster(&ticket.user)));
    }
    Instruction {
        program_id: p.program,
        accounts,
        data: data(disc::CASH_OUT, &[quote_msg]),
    }
}

/// `pause(paused)`, signed by `Config.authority`: `true` blocks `open_pending` and `finalize`;
/// refunds, resolutions, settles and cash-outs stay open (D-052). Runbook "Pascal down".
pub fn pause(p: &Pdas, authority: &Pubkey, paused: bool) -> Instruction {
    Instruction {
        program_id: p.program,
        accounts: vec![
            signer(*authority, false),
            w(p.config()),
            r(p.event_authority()),
            r(p.program),
        ],
        data: data(disc::PAUSE, &[&[u8::from(paused)]]),
    }
}

/// Associated Token Account `CreateIdempotent` of `owner`'s account for `mint`, paid by `payer`.
pub fn create_ata_idempotent(payer: &Pubkey, owner: &Pubkey, mint: &Pubkey) -> Instruction {
    Instruction {
        program_id: ASSOCIATED_TOKEN_PROGRAM_ID,
        accounts: vec![
            signer(*payer, true),
            w(associated_token_address(owner, mint)),
            r(*owner),
            r(*mint),
            r(SYSTEM_PROGRAM_ID),
            r(TOKEN_PROGRAM_ID),
        ],
        data: vec![1],
    }
}

/// The `Ed25519Program` verify instruction the program accepts (D-049): one signature, and
/// public key, signature and message all inside it (every instruction index `0xFFFF`), as
/// solana's `new_ed25519_instruction_with_signature` lays it out.
pub fn ed25519_verify(msg: &[u8], sig: &[u8; 64], pubkey: &Pubkey) -> Instruction {
    const START: u16 = 16;
    let pk_off = START;
    let sig_off = pk_off + 32;
    let msg_off = sig_off + 64;
    let mut d = Vec::with_capacity(usize::from(msg_off) + msg.len());
    d.extend_from_slice(&[1, 0]);
    for v in [
        sig_off,
        u16::MAX,
        pk_off,
        u16::MAX,
        msg_off,
        u16::try_from(msg.len()).unwrap_or(u16::MAX),
        u16::MAX,
    ] {
        d.extend_from_slice(&v.to_le_bytes());
    }
    d.extend_from_slice(pubkey.as_ref());
    d.extend_from_slice(sig);
    d.extend_from_slice(msg);
    Instruction {
        program_id: ED25519_PROGRAM_ID,
        accounts: vec![],
        data: d,
    }
}
