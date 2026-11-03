//! Accounts (spec section 4): `Config`, `Vault`, `Pending`, `Ticket`, and
//! `TipsterProfile` (step 2.F; logic in `tipster`). `MockMint` is
//! the SPL mint at `["mock", "USDC"]` (mint authority `Config`), not a data account.
//! Every field the spec lists is present now, including the copy-fee and tipster
//! fields, so step 2.F does not move the layout. Each account carries zeroed
//! `reserved` bytes for later fields.

use anchor_lang::prelude::*;

use crate::constants::MAX_LEGS;
use crate::messages::QuoteLeg;

/// Global parameters. PDA `["config"]`.
#[account]
#[derive(InitSpace)]
pub struct Config {
    /// Offline key: `set_params`, `pause`, `withdraw_house`, the 0.5 resolution exception.
    pub authority: Pubkey,
    /// Backend key: `finalize`, `resolve_leg`.
    pub crank: Pubkey,
    /// Backend key whose ed25519 signature `open_pending` and `cash_out` verify.
    pub quote_signer: Pubkey,
    /// Mock USDC, PDA `["mock", "USDC"]`, 6 decimals, mint authority = this `Config`.
    pub usdc_mint: Pubkey,
    /// Copy fee in bps of stake (2.F; 200 = 2%).
    pub copy_fee_bps: u16,
    /// Tipster share of the copy fee in bps (2.F; 8_000 = 80%).
    pub tipster_share_bps: u16,
    /// Legs per ticket, `1..=MAX_LEGS`.
    pub max_legs: u8,
    /// Largest ticket payout, micro-USDC.
    pub max_payout: u64,
    /// Smallest stake, micro-USDC.
    pub min_stake: u64,
    /// `withdraw_house` never takes `Vault.free` below this.
    pub reserve: u64,
    /// Blocks `open_pending` and `finalize` (exits stay open).
    pub paused: bool,
    pub bump: u8,
    pub usdc_mint_bump: u8,
    pub reserved: [u8; 64],
}

/// House accounting. PDA `["vault"]`. Invariants: `locked == Σ payout_u` over `Open`
/// tickets; `free + locked ==` balance of `usdc_ata`.
#[account]
#[derive(InitSpace)]
pub struct Vault {
    /// House token account, PDA `["vault_usdc"]` (owner `Config`).
    pub usdc_ata: Pubkey,
    /// Escrow token account for pending stakes, PDA `["escrow_usdc"]` (owner `Config`);
    /// its balance is `Σ (stake_u + copy_fee_u)` over live `Pending` accounts.
    pub escrow_ata: Pubkey,
    /// Full payout of every open ticket.
    pub locked: u64,
    /// Unencumbered house money.
    pub free: u64,
    /// v2 (LP vault); always 0.
    pub lp_shares: u64,
    pub bump: u8,
    pub reserved: [u8; 64],
}

/// A user's escrowed request for a ticket, alive 30-60 s. PDA
/// `["pending", user, nonce u64 LE]`. Closed (rent to [`Pending::rent_receiver`]) by
/// `finalize` or `refund_pending`.
#[account]
#[derive(InitSpace)]
pub struct Pending {
    pub user: Pubkey,
    pub nonce: u64,
    /// Escrowed stake (`quote.stake_u`).
    pub stake_u: u64,
    /// Escrowed copy fee (2.F; 0 in the MVP).
    pub copy_fee_u: u64,
    pub n: u8,
    /// Legs `0..n` as signed by the desk; the rest zeroed.
    pub legs: [QuoteLeg; MAX_LEGS],
    /// `quote.odds_offered_u`.
    pub odds_offered_u: u64,
    /// The user's floor: `finalize` needs `payout_u * 1e6 >= min_payout_per_unit_u * rs_u`.
    pub min_payout_per_unit_u: u64,
    pub quote_expires_at: i64,
    /// sha256 of the signed quote bytes.
    pub quote_hash: [u8; 32],
    /// The desk's ed25519 signature over those bytes.
    pub quote_sig: [u8; 64],
    /// `finalize` needs `now < expires_at`; `refund_pending` needs `now >= expires_at + 15`.
    pub expires_at: i64,
    pub created_at: i64,
    pub is_tip: bool,
    /// `Some(user)` when `is_tip`.
    pub tipster: Option<Pubkey>,
    /// Original ticket of a copy (2.F; always `None` in the MVP).
    pub copied_from: Option<Pubkey>,
    pub bump: u8,
    /// Who paid this account's rent (`open_pending`'s `payer`: the user, or the desk's
    /// sponsor key, L8). Takes the 32 bytes that were `reserved`, so the layout is unchanged;
    /// all zeros on a Pending opened before L8.
    pub rent_payer: Pubkey,
}

impl Pending {
    /// Where the rent goes back when this Pending closes: `rent_payer`, or `user` for a
    /// Pending opened before L8 (zero `rent_payer`). A sponsored user never receives the
    /// sponsor's rent.
    pub fn rent_receiver(&self) -> Pubkey {
        if self.rent_payer == Pubkey::default() {
            self.user
        } else {
            self.rent_payer
        }
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegStatus {
    Open,
    /// `rtilde == 1e6`.
    Won,
    /// `rtilde == 0`.
    Lost,
    /// `0 < rtilde < 1e6` (half-void and other scalar resolutions).
    Partial,
}

/// A minted leg.
#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TicketLeg {
    pub symbol_hash: [u8; 32],
    pub game_id_hash: [u8; 32],
    pub start_ts: i64,
    /// 0 long, 1 short.
    pub side: u8,
    /// Signed quote price `p_i`.
    pub price_u: u64,
    /// Price used in the payout: the fill for a hedged leg, `price_u` otherwise.
    pub fill_u: u64,
    pub m1_ppm: u32,
    /// `true` when `finalize` got a non-zero fill for this leg.
    pub hedged: bool,
    pub status: LegStatus,
    /// Pascal's resolution `r` (long side), micro-units; 0 while `Open`.
    pub resolution_ppm: u32,
}

impl Default for TicketLeg {
    fn default() -> Self {
        Self {
            symbol_hash: [0; 32],
            game_id_hash: [0; 32],
            start_ts: 0,
            side: 0,
            price_u: 0,
            fill_u: 0,
            m1_ppm: 0,
            hedged: false,
            status: LegStatus::Open,
            resolution_ppm: 0,
        }
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, InitSpace, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TicketState {
    /// `payout_u` is locked in the vault.
    Open,
    /// Paid `P × Π r̃` by `settle`.
    Settled,
    /// Paid `C` by `cash_out`.
    CashedOut,
}

/// A minted parlay. PDA `["ticket", user, nonce u64 LE]`. Immutable except
/// `legs[].status`, `legs[].resolution_ppm`, `state`, `paid_u`, `resolved_at`. Never closed.
#[account]
#[derive(InitSpace)]
pub struct Ticket {
    pub user: Pubkey,
    pub nonce: u64,
    pub n: u8,
    pub legs: [TicketLeg; MAX_LEGS],
    /// Stake behind the ticket, `rS` (C2 `rs_u`).
    pub stake_u: u64,
    /// `S - rS`, returned at `finalize`.
    pub refund_u: u64,
    /// Locked payout `P` (C2 `payout_u`).
    pub payout_u: u64,
    /// `floor(P * 1e6 / rS)`.
    pub odds_u: u64,
    /// The desk's signed offered odds.
    pub odds_offered_u: u64,
    pub min_payout_per_unit_u: u64,
    /// Fill ratio `r`.
    pub r_ppm: u32,
    /// `rS × m(n)`, for the books (no transfer: the margin is inside `P`).
    pub fee_u: u64,
    /// Copy fee charged (2.F; 0 in the MVP).
    pub copy_fee_u: u64,
    pub state: TicketState,
    pub tipster: Option<Pubkey>,
    pub copied_from: Option<Pubkey>,
    pub quote_hash: [u8; 32],
    pub minted_at: i64,
    /// When the ticket left `Open` (settle or cash-out); 0 before.
    pub resolved_at: i64,
    /// Amount paid out at settle or cash-out; 0 before.
    pub paid_u: u64,
    pub bump: u8,
    pub reserved: [u8; 64],
}

/// A tipster's own-tip record (2.F). PDA `["tipster", wallet]`. Own tips only
/// (`ticket.tipster == Some(ticket.user)`, `copied_from == None`): `finalize` adds a
/// ticket to `open_*`; `settle`/`cash_out` move it to `settled_count`, `won` (returned
/// more than staked), `settled_staked` and `returned` (paid or `C`). Stakes are the
/// ticket's `rS`. `fees_earned` stays 0: copies never touch a profile, so copy-fee
/// income is the sum of `TipsterPaid` events.
#[account]
#[derive(InitSpace)]
pub struct TipsterProfile {
    pub wallet: Pubkey,
    pub open_count: u32,
    pub open_staked: u64,
    pub settled_count: u32,
    pub won: u32,
    pub settled_staked: u64,
    pub returned: u64,
    pub fees_earned: u64,
    pub bump: u8,
    pub reserved: [u8; 32],
}

/// Last faucet claim per recipient owner (devnet builds only; no cooldown since D-661). PDA
/// `["faucet", owner]`.
#[cfg(feature = "devnet")]
#[account]
#[derive(InitSpace)]
pub struct FaucetClaim {
    pub owner: Pubkey,
    pub last_claim_ts: i64,
    pub bump: u8,
}
