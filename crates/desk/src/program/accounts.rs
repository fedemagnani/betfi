//! Account decoders: Anchor discriminator, then the borsh body in field order
//! (`programs/schedina/src/state.rs`). `Option<Pubkey>` is borsh's 1-byte tag plus 32 bytes when
//! `Some`, so the fields after one move; the decoders read sequentially and never at fixed
//! offsets past the first option.

use solana_pubkey::Pubkey;

use super::consts::MAX_LEGS;

/// Anchor account discriminators (IDL `accounts[].discriminator`).
pub mod disc {
    /// `Config`.
    pub const CONFIG: [u8; 8] = [155, 12, 170, 224, 30, 250, 204, 130];
    /// `Vault`.
    pub const VAULT: [u8; 8] = [211, 8, 232, 43, 2, 152, 117, 119];
    /// `Pending`.
    pub const PENDING: [u8; 8] = [36, 180, 147, 170, 72, 175, 173, 126];
    /// `Ticket`.
    pub const TICKET: [u8; 8] = [41, 228, 24, 165, 78, 90, 235, 200];
    /// `FaucetClaim` (devnet builds).
    pub const FAUCET_CLAIM: [u8; 8] = [88, 39, 189, 221, 15, 215, 24, 248];
}

/// Why bytes are not the expected account or event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// Not owned by the program.
    WrongOwner,
    /// Another account type.
    WrongDiscriminator,
    /// Ran out of bytes.
    TooShort,
    /// A value no valid account holds (enum variant, option tag, `n`).
    Invalid(&'static str),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongOwner => f.write_str("account not owned by the program"),
            Self::WrongDiscriminator => f.write_str("discriminator mismatch"),
            Self::TooShort => f.write_str("data too short"),
            Self::Invalid(what) => write!(f, "invalid {what}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Sequential borsh reader.
pub(crate) struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    /// Reads `data` after checking its 8-byte discriminator.
    pub(crate) fn with_disc(data: &'a [u8], d: &[u8; 8]) -> Result<Self, DecodeError> {
        match data.get(..8) {
            None => Err(DecodeError::TooShort),
            Some(h) if h != d => Err(DecodeError::WrongDiscriminator),
            Some(_) => Ok(Reader { data, at: 8 }),
        }
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let end = self.at.checked_add(N).ok_or(DecodeError::TooShort)?;
        let s = self.data.get(self.at..end).ok_or(DecodeError::TooShort)?;
        self.at = end;
        let mut b = [0u8; N];
        b.copy_from_slice(s);
        Ok(b)
    }
    pub(crate) fn bytes32(&mut self) -> Result<[u8; 32], DecodeError> {
        self.take::<32>()
    }
    pub(crate) fn pubkey(&mut self) -> Result<Pubkey, DecodeError> {
        Ok(Pubkey::new_from_array(self.take::<32>()?))
    }
    pub(crate) fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take::<1>()?[0])
    }
    pub(crate) fn bool(&mut self) -> Result<bool, DecodeError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DecodeError::Invalid("bool")),
        }
    }
    pub(crate) fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.take()?))
    }
    pub(crate) fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.take()?))
    }
    pub(crate) fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.take()?))
    }
    pub(crate) fn i64(&mut self) -> Result<i64, DecodeError> {
        Ok(i64::from_le_bytes(self.take()?))
    }
    pub(crate) fn opt_pubkey(&mut self) -> Result<Option<Pubkey>, DecodeError> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.pubkey()?)),
            _ => Err(DecodeError::Invalid("option tag")),
        }
    }
    /// A borsh `Vec` length (u32), bounded so a corrupt length cannot allocate much.
    pub(crate) fn len(&mut self, max: usize) -> Result<usize, DecodeError> {
        let n = usize::try_from(self.u32()?).map_err(|_| DecodeError::Invalid("length"))?;
        if n > max {
            return Err(DecodeError::Invalid("length"));
        }
        Ok(n)
    }
    pub(crate) fn skip(&mut self, n: usize) -> Result<(), DecodeError> {
        let end = self.at.checked_add(n).ok_or(DecodeError::TooShort)?;
        if end > self.data.len() {
            return Err(DecodeError::TooShort);
        }
        self.at = end;
        Ok(())
    }
}

fn check_owner(owner: &Pubkey, program: &Pubkey) -> Result<(), DecodeError> {
    if owner == program {
        Ok(())
    } else {
        Err(DecodeError::WrongOwner)
    }
}

/// `Config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigAccount {
    /// Authority.
    pub authority: Pubkey,
    /// Crank key.
    pub crank: Pubkey,
    /// Quote signer.
    pub quote_signer: Pubkey,
    /// Mock USDC mint.
    pub usdc_mint: Pubkey,
    /// Copy fee, bps.
    pub copy_fee_bps: u16,
    /// Tipster share, bps.
    pub tipster_share_bps: u16,
    /// Legs per ticket.
    pub max_legs: u8,
    /// Largest payout.
    pub max_payout: u64,
    /// Smallest stake.
    pub min_stake: u64,
    /// Withdraw floor.
    pub reserve: u64,
    /// Paused.
    pub paused: bool,
}

impl ConfigAccount {
    /// Decodes a `Config` owned by `program`.
    pub fn decode(data: &[u8], owner: &Pubkey, program: &Pubkey) -> Result<Self, DecodeError> {
        check_owner(owner, program)?;
        let mut r = Reader::with_disc(data, &disc::CONFIG)?;
        Ok(ConfigAccount {
            authority: r.pubkey()?,
            crank: r.pubkey()?,
            quote_signer: r.pubkey()?,
            usdc_mint: r.pubkey()?,
            copy_fee_bps: r.u16()?,
            tipster_share_bps: r.u16()?,
            max_legs: r.u8()?,
            max_payout: r.u64()?,
            min_stake: r.u64()?,
            reserve: r.u64()?,
            paused: r.bool()?,
        })
    }
}

/// `Vault`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultAccount {
    /// House token account.
    pub usdc_ata: Pubkey,
    /// Escrow token account.
    pub escrow_ata: Pubkey,
    /// Σ payout of open tickets.
    pub locked: u64,
    /// House money.
    pub free: u64,
    /// v2.
    pub lp_shares: u64,
}

impl VaultAccount {
    /// Decodes a `Vault` owned by `program`.
    pub fn decode(data: &[u8], owner: &Pubkey, program: &Pubkey) -> Result<Self, DecodeError> {
        check_owner(owner, program)?;
        let mut r = Reader::with_disc(data, &disc::VAULT)?;
        Ok(VaultAccount {
            usdc_ata: r.pubkey()?,
            escrow_ata: r.pubkey()?,
            locked: r.u64()?,
            free: r.u64()?,
            lp_shares: r.u64()?,
        })
    }
}

/// One quoted leg (`messages::QuoteLeg`, 85 bytes).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QuoteLeg {
    /// sha256(symbol).
    pub symbol_hash: [u8; 32],
    /// sha256(game_id).
    pub game_id_hash: [u8; 32],
    /// Unix s.
    pub start_ts: i64,
    /// 0 long, 1 short.
    pub side: u8,
    /// Quote price.
    pub price_u: u64,
    /// Leg margin.
    pub m1_ppm: u32,
}

impl QuoteLeg {
    pub(crate) fn read(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(QuoteLeg {
            symbol_hash: r.bytes32()?,
            game_id_hash: r.bytes32()?,
            start_ts: r.i64()?,
            side: r.u8()?,
            price_u: r.u64()?,
            m1_ppm: r.u32()?,
        })
    }

    /// Borsh bytes (85).
    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.symbol_hash);
        out.extend_from_slice(&self.game_id_hash);
        out.extend_from_slice(&self.start_ts.to_le_bytes());
        out.push(self.side);
        out.extend_from_slice(&self.price_u.to_le_bytes());
        out.extend_from_slice(&self.m1_ppm.to_le_bytes());
    }
}

/// `Pending`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAccount {
    /// Owner.
    pub user: Pubkey,
    /// PDA nonce.
    pub nonce: u64,
    /// Escrowed stake S.
    pub stake_u: u64,
    /// Escrowed copy fee.
    pub copy_fee_u: u64,
    /// Legs `0..n` (signed quote order).
    pub legs: Vec<QuoteLeg>,
    /// Offered odds.
    pub odds_offered_u: u64,
    /// The user's floor.
    pub min_payout_per_unit_u: u64,
    /// Quote TTL.
    pub quote_expires_at: i64,
    /// sha256 of the signed quote bytes.
    pub quote_hash: [u8; 32],
    /// `finalize` needs `now < expires_at`.
    pub expires_at: i64,
    /// Creation time.
    pub created_at: i64,
    /// Own tip.
    pub is_tip: bool,
    /// `Some(user)` when `is_tip`.
    pub tipster: Option<Pubkey>,
    /// Original ticket of a copy.
    pub copied_from: Option<Pubkey>,
    /// Who paid the rent (L8; zero on a Pending opened before L8).
    pub rent_payer: Pubkey,
}

impl PendingAccount {
    /// Where the rent goes when the Pending closes (`finalize`, `refund_pending`): the program's
    /// `Pending::rent_receiver`, `rent_payer` or, for a pre-L8 Pending, `user`.
    pub fn rent_receiver(&self) -> Pubkey {
        if self.rent_payer == Pubkey::default() {
            self.user
        } else {
            self.rent_payer
        }
    }

    /// Decodes a `Pending` owned by `program`.
    pub fn decode(data: &[u8], owner: &Pubkey, program: &Pubkey) -> Result<Self, DecodeError> {
        check_owner(owner, program)?;
        let mut r = Reader::with_disc(data, &disc::PENDING)?;
        let user = r.pubkey()?;
        let nonce = r.u64()?;
        let stake_u = r.u64()?;
        let copy_fee_u = r.u64()?;
        let n = usize::from(r.u8()?);
        if n == 0 || n > MAX_LEGS {
            return Err(DecodeError::Invalid("n"));
        }
        let mut legs = Vec::with_capacity(n);
        for i in 0..MAX_LEGS {
            let l = QuoteLeg::read(&mut r)?;
            if i < n {
                legs.push(l);
            }
        }
        let odds_offered_u = r.u64()?;
        let min_payout_per_unit_u = r.u64()?;
        let quote_expires_at = r.i64()?;
        let quote_hash = r.bytes32()?;
        r.skip(64)?; // quote_sig
        let expires_at = r.i64()?;
        let created_at = r.i64()?;
        let is_tip = r.bool()?;
        let tipster = r.opt_pubkey()?;
        let copied_from = r.opt_pubkey()?;
        r.skip(1)?; // bump
        Ok(PendingAccount {
            user,
            nonce,
            stake_u,
            copy_fee_u,
            legs,
            odds_offered_u,
            min_payout_per_unit_u,
            quote_expires_at,
            quote_hash,
            expires_at,
            created_at,
            is_tip,
            tipster,
            copied_from,
            rent_payer: r.pubkey()?,
        })
    }
}

/// `LegStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegStatus {
    /// Unresolved.
    Open,
    /// r̃ = 1.
    Won,
    /// r̃ = 0.
    Lost,
    /// 0 < r̃ < 1.
    Partial,
}

impl LegStatus {
    fn from_u8(v: u8) -> Result<Self, DecodeError> {
        Ok(match v {
            0 => LegStatus::Open,
            1 => LegStatus::Won,
            2 => LegStatus::Lost,
            3 => LegStatus::Partial,
            _ => return Err(DecodeError::Invalid("leg status")),
        })
    }
}

/// A minted leg (`state::TicketLeg`, 99 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TicketLeg {
    /// sha256(symbol).
    pub symbol_hash: [u8; 32],
    /// sha256(game_id).
    pub game_id_hash: [u8; 32],
    /// Unix s.
    pub start_ts: i64,
    /// 0 long, 1 short.
    pub side: u8,
    /// Quote price.
    pub price_u: u64,
    /// Payout price (fill, or `price_u` if unhedged).
    pub fill_u: u64,
    /// Leg margin.
    pub m1_ppm: u32,
    /// Hedged at finalize.
    pub hedged: bool,
    /// Status.
    pub status: LegStatus,
    /// Pascal's r (long side), 0 while open.
    pub resolution_ppm: u32,
}

/// `TicketState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TicketState {
    /// Payout locked.
    Open,
    /// Paid by `settle`.
    Settled,
    /// Paid by `cash_out`.
    CashedOut,
}

/// `Ticket`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketAccount {
    /// Owner.
    pub user: Pubkey,
    /// PDA nonce.
    pub nonce: u64,
    /// Legs `0..n`.
    pub legs: Vec<TicketLeg>,
    /// rS.
    pub stake_u: u64,
    /// S − rS.
    pub refund_u: u64,
    /// P.
    pub payout_u: u64,
    /// P / rS.
    pub odds_u: u64,
    /// Signed offered odds.
    pub odds_offered_u: u64,
    /// Floor.
    pub min_payout_per_unit_u: u64,
    /// Fill ratio.
    pub r_ppm: u32,
    /// rS × m(n).
    pub fee_u: u64,
    /// Copy fee.
    pub copy_fee_u: u64,
    /// State.
    pub state: TicketState,
    /// Tipster.
    pub tipster: Option<Pubkey>,
    /// Original of a copy.
    pub copied_from: Option<Pubkey>,
    /// sha256 of the signed quote.
    pub quote_hash: [u8; 32],
    /// Unix s.
    pub minted_at: i64,
    /// Unix s when it left `Open`, else 0.
    pub resolved_at: i64,
    /// Paid at settle or cash-out, else 0.
    pub paid_u: u64,
}

impl TicketAccount {
    /// Byte offset of `state`: every field before it has a fixed size.
    pub const STATE_OFFSET: usize = 8 + 32 + 8 + 1 + MAX_LEGS * 99 + 8 * 6 + 4 + 8 * 2;

    /// Decodes a `Ticket` owned by `program`.
    pub fn decode(data: &[u8], owner: &Pubkey, program: &Pubkey) -> Result<Self, DecodeError> {
        check_owner(owner, program)?;
        let mut r = Reader::with_disc(data, &disc::TICKET)?;
        let user = r.pubkey()?;
        let nonce = r.u64()?;
        let n = usize::from(r.u8()?);
        if n == 0 || n > MAX_LEGS {
            return Err(DecodeError::Invalid("n"));
        }
        let mut legs = Vec::with_capacity(n);
        for i in 0..MAX_LEGS {
            let l = TicketLeg {
                symbol_hash: r.bytes32()?,
                game_id_hash: r.bytes32()?,
                start_ts: r.i64()?,
                side: r.u8()?,
                price_u: r.u64()?,
                fill_u: r.u64()?,
                m1_ppm: r.u32()?,
                hedged: r.bool()?,
                status: LegStatus::from_u8(r.u8()?)?,
                resolution_ppm: r.u32()?,
            };
            if i < n {
                legs.push(l);
            }
        }
        Ok(TicketAccount {
            user,
            nonce,
            legs,
            stake_u: r.u64()?,
            refund_u: r.u64()?,
            payout_u: r.u64()?,
            odds_u: r.u64()?,
            odds_offered_u: r.u64()?,
            min_payout_per_unit_u: r.u64()?,
            r_ppm: r.u32()?,
            fee_u: r.u64()?,
            copy_fee_u: r.u64()?,
            state: match r.u8()? {
                0 => TicketState::Open,
                1 => TicketState::Settled,
                2 => TicketState::CashedOut,
                _ => return Err(DecodeError::Invalid("ticket state")),
            },
            tipster: r.opt_pubkey()?,
            copied_from: r.opt_pubkey()?,
            quote_hash: r.bytes32()?,
            minted_at: r.i64()?,
            resolved_at: r.i64()?,
            paid_u: r.u64()?,
        })
    }

    /// Whether `settle`/`cash_out` must pass the tipster profile (l2-contracts L2-4).
    pub fn is_own_tip(&self) -> bool {
        self.tipster == Some(self.user) && self.copied_from.is_none()
    }
}
