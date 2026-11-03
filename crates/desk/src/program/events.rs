//! `emit_cpi!` events (`programs/schedina/src/events.rs`). Each item of
//! [`crate::solana::listen::cpi_events`] is the event's 8-byte discriminator followed by its
//! borsh body; [`ProgramEvent::decode`] turns it into a typed event.

use solana_pubkey::Pubkey;

use super::accounts::{DecodeError, QuoteLeg, Reader};
use super::consts::MAX_LEGS;

/// Event discriminators (IDL `events[].discriminator`).
pub mod disc {
    /// `PendingOpened`.
    pub const PENDING_OPENED: [u8; 8] = [90, 227, 153, 200, 138, 86, 210, 203];
    /// `PendingRefunded`.
    pub const PENDING_REFUNDED: [u8; 8] = [253, 203, 132, 38, 212, 186, 173, 254];
    /// `TicketMinted`.
    pub const TICKET_MINTED: [u8; 8] = [22, 17, 212, 38, 91, 144, 104, 109];
    /// `LegResolved`.
    pub const LEG_RESOLVED: [u8; 8] = [83, 94, 20, 246, 252, 125, 188, 166];
    /// `TicketSettled`.
    pub const TICKET_SETTLED: [u8; 8] = [72, 40, 13, 3, 184, 159, 190, 211];
    /// `TicketCashedOut`.
    pub const TICKET_CASHED_OUT: [u8; 8] = [174, 14, 251, 89, 59, 107, 29, 83];
    /// `VaultFunded`.
    pub const VAULT_FUNDED: [u8; 8] = [192, 119, 245, 193, 55, 223, 195, 50];
}

/// `PendingOpened`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingOpenedEvent {
    /// Pending PDA.
    pub pending: Pubkey,
    /// Owner.
    pub user: Pubkey,
    /// Nonce.
    pub nonce: u64,
    /// Legs in ticket order.
    pub legs: Vec<QuoteLeg>,
    /// S.
    pub stake_u: u64,
    /// Copy fee.
    pub copy_fee_u: u64,
    /// Offered odds.
    pub odds_offered_u: u64,
    /// Floor.
    pub min_payout_per_unit_u: u64,
    /// Quote TTL.
    pub quote_expires_at: i64,
    /// Pending expiry.
    pub expires_at: i64,
    /// sha256 of the signed quote.
    pub quote_hash: [u8; 32],
    /// Own tip.
    pub is_tip: bool,
    /// Original of a copy.
    pub copied_from: Option<Pubkey>,
}

/// `TicketMinted`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketMintedEvent {
    /// Ticket PDA.
    pub ticket: Pubkey,
    /// Pending PDA.
    pub pending: Pubkey,
    /// Owner.
    pub user: Pubkey,
    /// Nonce.
    pub nonce: u64,
    /// `finalize`'s fills.
    pub fills: Vec<u64>,
    /// r.
    pub r_ppm: u32,
    /// rS.
    pub stake_u: u64,
    /// S − rS.
    pub refund_u: u64,
    /// P.
    pub payout_u: u64,
    /// P / rS.
    pub odds_u: u64,
    /// rS × m(n).
    pub fee_u: u64,
    /// Tipster.
    pub tipster: Option<Pubkey>,
    /// Original of a copy.
    pub copied_from: Option<Pubkey>,
    /// Vault locked after.
    pub vault_locked: u64,
    /// Vault free after.
    pub vault_free: u64,
}

/// A decoded program event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgramEvent {
    /// `open_pending` landed.
    PendingOpened(Box<PendingOpenedEvent>),
    /// `refund_pending` landed.
    PendingRefunded {
        /// Pending PDA.
        pending: Pubkey,
        /// Owner.
        user: Pubkey,
        /// Nonce.
        nonce: u64,
        /// Stake plus copy fee returned.
        amount_u: u64,
    },
    /// `finalize` landed.
    TicketMinted(Box<TicketMintedEvent>),
    /// `resolve_leg` landed.
    LegResolved {
        /// Ticket PDA.
        ticket: Pubkey,
        /// Leg index.
        leg_index: u8,
        /// Pascal r (long side).
        r_ppm: u32,
        /// The leg's factor.
        rtilde_ppm: u32,
        /// Signer.
        resolver: Pubkey,
    },
    /// `settle` landed.
    TicketSettled {
        /// Ticket PDA.
        ticket: Pubkey,
        /// Owner.
        user: Pubkey,
        /// P.
        payout_u: u64,
        /// Paid.
        paid_u: u64,
    },
    /// `cash_out` landed.
    TicketCashedOut {
        /// Ticket PDA.
        ticket: Pubkey,
        /// Owner.
        user: Pubkey,
        /// P.
        payout_u: u64,
        /// Paid.
        c_u: u64,
        /// Signed quote time.
        ts: i64,
    },
    /// `fund_vault` landed.
    VaultFunded {
        /// Funder.
        funder: Pubkey,
        /// Amount.
        amount_u: u64,
        /// Vault free after.
        vault_free: u64,
    },
    /// An event this desk does not act on (`ConfigUpdated`, `HouseWithdrawn`, `TipsterPaid`,
    /// or one 2.F adds), by discriminator.
    Other([u8; 8]),
}

impl ProgramEvent {
    /// Decodes one `cpi_events` item (discriminator ‖ borsh).
    pub fn decode(data: &[u8]) -> Result<Self, DecodeError> {
        let d: [u8; 8] = data
            .get(..8)
            .and_then(|h| h.try_into().ok())
            .ok_or(DecodeError::TooShort)?;
        let mut r = Reader::with_disc(data, &d)?;
        Ok(match d {
            disc::PENDING_OPENED => {
                let pending = r.pubkey()?;
                let user = r.pubkey()?;
                let nonce = r.u64()?;
                let n = r.len(MAX_LEGS)?;
                let mut legs = Vec::with_capacity(n);
                for _ in 0..n {
                    legs.push(QuoteLeg::read(&mut r)?);
                }
                ProgramEvent::PendingOpened(Box::new(PendingOpenedEvent {
                    pending,
                    user,
                    nonce,
                    legs,
                    stake_u: r.u64()?,
                    copy_fee_u: r.u64()?,
                    odds_offered_u: r.u64()?,
                    min_payout_per_unit_u: r.u64()?,
                    quote_expires_at: r.i64()?,
                    expires_at: r.i64()?,
                    quote_hash: r.bytes32()?,
                    is_tip: r.bool()?,
                    copied_from: r.opt_pubkey()?,
                }))
            }
            disc::PENDING_REFUNDED => ProgramEvent::PendingRefunded {
                pending: r.pubkey()?,
                user: r.pubkey()?,
                nonce: r.u64()?,
                amount_u: r.u64()?,
            },
            disc::TICKET_MINTED => {
                let ticket = r.pubkey()?;
                let pending = r.pubkey()?;
                let user = r.pubkey()?;
                let nonce = r.u64()?;
                let n = r.len(MAX_LEGS)?;
                let mut fills = Vec::with_capacity(n);
                for _ in 0..n {
                    fills.push(r.u64()?);
                }
                ProgramEvent::TicketMinted(Box::new(TicketMintedEvent {
                    ticket,
                    pending,
                    user,
                    nonce,
                    fills,
                    r_ppm: r.u32()?,
                    stake_u: r.u64()?,
                    refund_u: r.u64()?,
                    payout_u: r.u64()?,
                    odds_u: r.u64()?,
                    fee_u: r.u64()?,
                    tipster: r.opt_pubkey()?,
                    copied_from: r.opt_pubkey()?,
                    vault_locked: r.u64()?,
                    vault_free: r.u64()?,
                }))
            }
            disc::LEG_RESOLVED => ProgramEvent::LegResolved {
                ticket: r.pubkey()?,
                leg_index: r.u8()?,
                r_ppm: r.u32()?,
                rtilde_ppm: r.u32()?,
                resolver: r.pubkey()?,
            },
            disc::TICKET_SETTLED => ProgramEvent::TicketSettled {
                ticket: r.pubkey()?,
                user: r.pubkey()?,
                payout_u: r.u64()?,
                paid_u: r.u64()?,
            },
            disc::TICKET_CASHED_OUT => ProgramEvent::TicketCashedOut {
                ticket: r.pubkey()?,
                user: r.pubkey()?,
                payout_u: r.u64()?,
                c_u: r.u64()?,
                ts: r.i64()?,
            },
            disc::VAULT_FUNDED => ProgramEvent::VaultFunded {
                funder: r.pubkey()?,
                amount_u: r.u64()?,
                vault_free: r.u64()?,
            },
            other => ProgramEvent::Other(other),
        })
    }
}
