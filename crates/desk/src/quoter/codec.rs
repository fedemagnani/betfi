//! The two desk-signed messages of l1-contracts C3 (amended by D-049 / D-050), byte for byte as
//! `fixtures/schedina/quote_vector.json` and `fixtures/schedina/README.md` define them. Each is
//! the borsh encoding of the program's instruction argument (`QuoteMsg` of `open_pending`,
//! `CashOutMsg` of `cash_out`); integers are little-endian.
//!
//! - Quote, `77 + 85 n` bytes: `"schedina/quote/v1"` (17) ‖ program id (32) ‖ `n` as u32 (borsh
//!   `Vec` length) ‖ `n` legs of 85 bytes (`symbol_hash` 32, `game_id_hash` 32, `start_ts` i64,
//!   `side` u8, `price_u` u64, `m1_ppm` u32) ‖ `stake_u` u64 ‖ `odds_offered_u` u64 ‖
//!   `quote_expires_at` i64.
//! - Cash-out, 99 bytes: `"schedina/cashout/v1"` (19) ‖ program id (32) ‖ ticket PDA (32) ‖
//!   `c_u` u64 ‖ `ts` i64.
//!
//! The codec has no I/O and no clock; [`QuoteMsg::decode`] and [`CashOutMsg::decode`] reject
//! trailing bytes, as the program does.

use sha2::{Digest, Sha256};

/// Domain tag of the quote message.
pub const QUOTE_TAG: &[u8; 17] = b"schedina/quote/v1";
/// Domain tag of the cash-out message.
pub const CASHOUT_TAG: &[u8; 19] = b"schedina/cashout/v1";
/// Bytes of a quote message before the legs: tag, program id, `n`.
pub const QUOTE_HEADER_LEN: usize = 17 + 32 + 4;
/// Bytes of one quote leg.
pub const QUOTE_LEG_LEN: usize = 32 + 32 + 8 + 1 + 8 + 4;
/// Bytes of a quote message after the legs: stake, odds, expiry.
pub const QUOTE_TRAILER_LEN: usize = 8 + 8 + 8;
/// Bytes of a cash-out message.
pub const CASHOUT_LEN: usize = 19 + 32 + 32 + 8 + 8;

/// `side` byte of a long leg.
pub const SIDE_LONG: u8 = 0;
/// `side` byte of a short leg.
pub const SIDE_SHORT: u8 = 1;

/// sha256 of `bytes`.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// Lowercase hex of `bytes` (the `quote_hash`, `symbol_hash`, `game_id_hash` columns, L2-1).
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from(DIGITS[usize::from(b >> 4)]));
        s.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    s
}

/// The total length of a quote message with `n` legs: `77 + 85 n`.
pub const fn quote_len(n: usize) -> usize {
    QUOTE_HEADER_LEN + QUOTE_LEG_LEN * n + QUOTE_TRAILER_LEN
}

/// Why bytes did not decode.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodecError {
    /// The domain tag is not the expected one.
    #[error("wrong tag")]
    Tag,
    /// The length does not match the layout.
    #[error("expected {expected} bytes, got {got}")]
    Length {
        /// Expected length.
        expected: usize,
        /// Actual length.
        got: usize,
    },
    /// A `side` byte other than 0 or 1.
    #[error("invalid side byte {0}")]
    Side(u8),
}

/// One leg of a [`QuoteMsg`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuoteMsgLeg {
    /// sha256 of the Pascal symbol (UTF-8).
    pub symbol_hash: [u8; 32],
    /// sha256 of `display_attributes.game_page.game_id` (UTF-8).
    pub game_id_hash: [u8; 32],
    /// Expected start, Unix s.
    pub start_ts: i64,
    /// [`SIDE_LONG`] or [`SIDE_SHORT`].
    pub side: u8,
    /// Ask-side quote price `p_i`, micro-units.
    pub price_u: u64,
    /// Leg margin, ppm.
    pub m1_ppm: u32,
}

impl QuoteMsgLeg {
    /// A leg from the strings the desk knows (hashing them).
    pub fn new(
        symbol: &str,
        game_id: &str,
        start_ts: i64,
        side: u8,
        price_u: u64,
        m1_ppm: u32,
    ) -> Self {
        QuoteMsgLeg {
            symbol_hash: sha256(symbol.as_bytes()),
            game_id_hash: sha256(game_id.as_bytes()),
            start_ts,
            side,
            price_u,
            m1_ppm,
        }
    }
}

/// The quote message (`QuoteMsg`), signed by the quote signer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuoteMsg {
    /// Schedina program id.
    pub program_id: [u8; 32],
    /// Legs in ticket order.
    pub legs: Vec<QuoteMsgLeg>,
    /// Stake, micro-USDC.
    pub stake_u: u64,
    /// Offered odds, micro-units.
    pub odds_offered_u: u64,
    /// Desk TTL, Unix s (`open_pending` needs `now <= quote_expires_at`).
    pub quote_expires_at: i64,
}

impl QuoteMsg {
    /// The borsh bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(quote_len(self.legs.len()));
        out.extend_from_slice(QUOTE_TAG);
        out.extend_from_slice(&self.program_id);
        // n ≤ 5 in practice; a u32 always holds a Vec length the desk can build.
        out.extend_from_slice(&(self.legs.len() as u32).to_le_bytes());
        for l in &self.legs {
            out.extend_from_slice(&l.symbol_hash);
            out.extend_from_slice(&l.game_id_hash);
            out.extend_from_slice(&l.start_ts.to_le_bytes());
            out.push(l.side);
            out.extend_from_slice(&l.price_u.to_le_bytes());
            out.extend_from_slice(&l.m1_ppm.to_le_bytes());
        }
        out.extend_from_slice(&self.stake_u.to_le_bytes());
        out.extend_from_slice(&self.odds_offered_u.to_le_bytes());
        out.extend_from_slice(&self.quote_expires_at.to_le_bytes());
        out
    }

    /// Decodes a whole message (no trailing bytes).
    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        let mut r = Reader(bytes);
        if r.take::<17>()? != *QUOTE_TAG {
            return Err(CodecError::Tag);
        }
        let program_id = r.take::<32>()?;
        let n = u32::from_le_bytes(r.take()?) as usize;
        let expected = quote_len(n.min(bytes.len()));
        if bytes.len() != expected {
            return Err(CodecError::Length {
                expected,
                got: bytes.len(),
            });
        }
        let mut legs = Vec::with_capacity(n);
        for _ in 0..n {
            let symbol_hash = r.take()?;
            let game_id_hash = r.take()?;
            let start_ts = i64::from_le_bytes(r.take()?);
            let [side] = r.take::<1>()?;
            if side > SIDE_SHORT {
                return Err(CodecError::Side(side));
            }
            legs.push(QuoteMsgLeg {
                symbol_hash,
                game_id_hash,
                start_ts,
                side,
                price_u: u64::from_le_bytes(r.take()?),
                m1_ppm: u32::from_le_bytes(r.take()?),
            });
        }
        Ok(QuoteMsg {
            program_id,
            legs,
            stake_u: u64::from_le_bytes(r.take()?),
            odds_offered_u: u64::from_le_bytes(r.take()?),
            quote_expires_at: i64::from_le_bytes(r.take()?),
        })
    }
}

/// The cash-out message (`CashOutMsg`), signed by the quote signer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CashOutMsg {
    /// Schedina program id.
    pub program_id: [u8; 32],
    /// Ticket PDA.
    pub ticket: [u8; 32],
    /// Amount paid, micro-USDC (≤ `Ticket.payout_u`).
    pub c_u: u64,
    /// Desk signing time, Unix s (valid while `|now − ts| ≤ 30`).
    pub ts: i64,
}

impl CashOutMsg {
    /// The 99 borsh bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(CASHOUT_LEN);
        out.extend_from_slice(CASHOUT_TAG);
        out.extend_from_slice(&self.program_id);
        out.extend_from_slice(&self.ticket);
        out.extend_from_slice(&self.c_u.to_le_bytes());
        out.extend_from_slice(&self.ts.to_le_bytes());
        out
    }

    /// Decodes a whole message (no trailing bytes).
    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        if bytes.len() != CASHOUT_LEN {
            return Err(CodecError::Length {
                expected: CASHOUT_LEN,
                got: bytes.len(),
            });
        }
        let mut r = Reader(bytes);
        if r.take::<19>()? != *CASHOUT_TAG {
            return Err(CodecError::Tag);
        }
        Ok(CashOutMsg {
            program_id: r.take()?,
            ticket: r.take()?,
            c_u: u64::from_le_bytes(r.take()?),
            ts: i64::from_le_bytes(r.take()?),
        })
    }
}

/// A cursor over the message bytes.
struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], CodecError> {
        let Some((head, rest)) = self.0.split_first_chunk::<N>() else {
            return Err(CodecError::Length {
                expected: N,
                got: self.0.len(),
            });
        };
        self.0 = rest;
        Ok(*head)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths() {
        assert_eq!(quote_len(0), 77);
        assert_eq!(quote_len(1), 77 + 85);
        assert_eq!(QUOTE_LEG_LEN, 85);
        assert_eq!(CASHOUT_LEN, 99);
    }

    #[test]
    fn round_trip_and_rejections() {
        let m = QuoteMsg {
            program_id: [3; 32],
            legs: vec![QuoteMsgLeg::new("A.B", "g", 1, SIDE_SHORT, 500_000, 30_000)],
            stake_u: 5_000_000,
            odds_offered_u: 1_940_000,
            quote_expires_at: 99,
        };
        let b = m.encode();
        assert_eq!(b.len(), quote_len(1));
        assert_eq!(QuoteMsg::decode(&b), Ok(m));
        let mut long = b.clone();
        long.push(0);
        assert!(matches!(
            QuoteMsg::decode(&long),
            Err(CodecError::Length { .. })
        ));
        let mut bad_side = b.clone();
        bad_side[QUOTE_HEADER_LEN + 72] = 2;
        assert_eq!(QuoteMsg::decode(&bad_side), Err(CodecError::Side(2)));
        let mut bad_tag = b;
        bad_tag[0] = b'x';
        assert_eq!(QuoteMsg::decode(&bad_tag), Err(CodecError::Tag));
        // A huge n in a short buffer is a length error, not an allocation.
        let mut huge = QUOTE_TAG.to_vec();
        huge.extend_from_slice(&[0; 32]);
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            QuoteMsg::decode(&huge),
            Err(CodecError::Length { .. })
        ));

        let c = CashOutMsg {
            program_id: [1; 32],
            ticket: [2; 32],
            c_u: 7,
            ts: -1,
        };
        let b = c.encode();
        assert_eq!(b.len(), CASHOUT_LEN);
        assert_eq!(CashOutMsg::decode(&b), Ok(c));
        assert!(CashOutMsg::decode(&b[1..]).is_err());
        assert_eq!(hex(&[0x00, 0xab, 0x7f]), "00ab7f");
    }
}
