//! Events for the desk's chain listener (spec section 4, "Events").
//! Emitted with `emit_cpi!` (D-005): they arrive as a self-CPI inner instruction
//! whose data is `EVENT_IX_TAG_LE ‖ event discriminator ‖ borsh(event)`, so log
//! truncation cannot drop them. Vault totals are the values after the instruction.

use anchor_lang::prelude::*;

use crate::messages::QuoteLeg;

/// `set_params` and `pause`: the config after the change.
#[event]
pub struct ConfigUpdated {
    pub crank: Pubkey,
    pub quote_signer: Pubkey,
    pub max_legs: u8,
    pub max_payout: u64,
    pub min_stake: u64,
    pub reserve: u64,
    pub copy_fee_bps: u16,
    pub tipster_share_bps: u16,
    pub paused: bool,
}

#[event]
pub struct VaultFunded {
    pub funder: Pubkey,
    pub amount_u: u64,
    pub vault_free: u64,
}

#[event]
pub struct HouseWithdrawn {
    pub destination: Pubkey,
    pub amount_u: u64,
    pub vault_free: u64,
}

/// What starts the hedge.
#[event]
pub struct PendingOpened {
    pub pending: Pubkey,
    pub user: Pubkey,
    pub nonce: u64,
    pub legs: Vec<QuoteLeg>,
    pub stake_u: u64,
    pub copy_fee_u: u64,
    pub odds_offered_u: u64,
    pub min_payout_per_unit_u: u64,
    pub quote_expires_at: i64,
    pub expires_at: i64,
    pub quote_hash: [u8; 32],
    pub is_tip: bool,
    pub copied_from: Option<Pubkey>,
}

#[event]
pub struct PendingRefunded {
    pub pending: Pubkey,
    pub user: Pubkey,
    pub nonce: u64,
    /// Stake plus copy fee returned.
    pub amount_u: u64,
}

#[event]
pub struct TicketMinted {
    pub ticket: Pubkey,
    pub pending: Pubkey,
    pub user: Pubkey,
    pub nonce: u64,
    /// The crank's `fills` argument (0 = unhedged leg).
    pub fills: Vec<u64>,
    pub r_ppm: u32,
    pub stake_u: u64,
    pub refund_u: u64,
    pub payout_u: u64,
    pub odds_u: u64,
    pub fee_u: u64,
    pub tipster: Option<Pubkey>,
    pub copied_from: Option<Pubkey>,
    pub vault_locked: u64,
    pub vault_free: u64,
}

#[event]
pub struct LegResolved {
    pub ticket: Pubkey,
    pub leg_index: u8,
    /// Pascal's resolution price of the symbol (long side).
    pub r_ppm: u32,
    /// The ticket's factor for this leg (`r` long, `1e6 - r` short).
    pub rtilde_ppm: u32,
    pub resolver: Pubkey,
}

#[event]
pub struct TicketSettled {
    pub ticket: Pubkey,
    pub user: Pubkey,
    pub payout_u: u64,
    pub paid_u: u64,
    pub vault_locked: u64,
    pub vault_free: u64,
}

#[event]
pub struct TicketCashedOut {
    pub ticket: Pubkey,
    pub user: Pubkey,
    pub payout_u: u64,
    pub c_u: u64,
    pub ts: i64,
    pub vault_locked: u64,
    pub vault_free: u64,
}

/// `finalize` of a copy: the tipster's share of the copy fee, paid to their USDC account.
#[event]
pub struct TipsterPaid {
    pub tipster: Pubkey,
    pub ticket: Pubkey,
    pub amount_u: u64,
}
