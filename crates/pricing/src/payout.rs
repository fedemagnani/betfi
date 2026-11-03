//! Margin, payout, odds, floor and settlement: contract C2, bit for bit with the program (1.C).
//!
//! ```text
//! rs_u     = floor(stake_u * r_ppm / 1e6)
//! acc      = rs_u
//! for each leg i, in ticket order:  acc = floor(acc * (1e6 - m1_ppm[i]) / f_u[i])
//! payout_u = acc
//! odds_u   = floor(payout_u * 1e6 / rs_u)
//! refund_u = stake_u - rs_u
//! floor ok  <=>  payout_u * 1e6 >= min_payout_per_unit_u * rs_u
//! settle:  acc = payout_u; for each leg: acc = floor(acc * rtilde_ppm[i] / 1e6); paid_u = acc
//! ```
//!
//! The vectors are `fixtures/schedina/payout_vectors.json` (tested in `tests/vectors.rs`).

use serde::{Deserialize, Serialize};

use crate::rules::MarginTiers;
use crate::{PricingError, Side, UNIT, UNIT_128, to_u64};

/// One leg's inputs to [`payout()`]: its margin tier and its price.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayoutLeg {
    /// `m₁,ᵢ`, ppm (30_000 or 50_000).
    pub m1_ppm: u32,
    /// `fᵢ`, micro-units: the achieved average fill of a hedged leg, the quote price `pᵢ` of an
    /// unhedged one (or of every leg at quote time), a mid for fair odds.
    pub f_u: u64,
}

/// The C2 result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payout {
    /// `rS`, micro-USDC: the part of the stake that became a bet.
    pub rs_u: u64,
    /// `P`, micro-USDC.
    pub payout_u: u64,
    /// `P / (rS)`, micro-units.
    pub odds_u: u64,
    /// `(1 − r)S`, micro-USDC.
    pub refund_u: u64,
}

/// `m₁,ᵢ`, ppm: the sequential tier for a leg that overlaps no other leg, the overlap tier
/// otherwise (spec §2 step 3; 3% / 5% by default).
pub fn m1_ppm(overlaps: bool, tiers: &MarginTiers) -> u32 {
    if overlaps {
        tiers.overlap_ppm
    } else {
        tiers.sequential_ppm
    }
}

/// `m(n) = 1 − Πᵢ (1 − m₁,ᵢ)`, ppm, rounded up (the product `Π(1 − m₁)` is floored).
///
/// For display and the books (`fee = rS × m(n)`); the payout itself applies each `1 − m₁,ᵢ` leg
/// by leg as C2 says, never this rounded total.
pub fn m_n_ppm(m1s: &[u32]) -> u32 {
    let mut keep: u128 = UNIT_128;
    for &m in m1s {
        let k = UNIT_128.saturating_sub(u128::from(m));
        keep = keep * k / UNIT_128;
    }
    (UNIT_128 - keep) as u32
}

/// C2: `rS`, `P = rS · Πᵢ (1 − m₁,ᵢ) / fᵢ` (floored after every leg, in ticket order), odds and
/// refund.
///
/// Errors: `r_ppm` or a `m1_ppm` above `1e6`, an `f_u` outside `(0, 1e6]`, no legs, `rS = 0`,
/// or a payout above `u64::MAX` (only with prices near the minimum tick on many legs).
pub fn payout(stake_u: u64, r_ppm: u32, legs: &[PayoutLeg]) -> Result<Payout, PricingError> {
    if legs.is_empty() {
        return Err(PricingError::LegCount(0));
    }
    if u64::from(r_ppm) > UNIT {
        return Err(PricingError::InvalidPrice(u64::from(r_ppm)));
    }
    let rs = u128::from(stake_u) * u128::from(r_ppm) / UNIT_128;
    if rs == 0 {
        return Err(PricingError::ZeroStake);
    }
    let mut acc = rs;
    for leg in legs {
        if leg.f_u == 0 || leg.f_u > UNIT {
            return Err(PricingError::InvalidPrice(leg.f_u));
        }
        if u64::from(leg.m1_ppm) > UNIT {
            return Err(PricingError::InvalidPrice(u64::from(leg.m1_ppm)));
        }
        let keep = UNIT_128 - u128::from(leg.m1_ppm);
        acc = acc.checked_mul(keep).ok_or(PricingError::Overflow)? / u128::from(leg.f_u);
    }
    let rs_u = to_u64(rs)?;
    let payout_u = to_u64(acc)?;
    Ok(Payout {
        rs_u,
        payout_u,
        odds_u: odds_u(payout_u, rs_u)?,
        refund_u: stake_u - rs_u,
    })
}

/// `odds = P / (rS)`, micro-units, rounded down.
pub fn odds_u(payout_u: u64, rs_u: u64) -> Result<u64, PricingError> {
    if rs_u == 0 {
        return Err(PricingError::ZeroStake);
    }
    to_u64(u128::from(payout_u) * UNIT_128 / u128::from(rs_u))
}

/// The program's mint check: `P / (rS) ≥ min_payout_per_unit`, without division:
/// `payout_u · 1e6 ≥ min_payout_per_unit_u · rs_u`.
pub fn floor_ok(payout_u: u64, rs_u: u64, min_payout_per_unit_u: u64) -> bool {
    u128::from(payout_u) * UNIT_128 >= u128::from(min_payout_per_unit_u) * u128::from(rs_u)
}

/// `r̃ᵢ` (spec §10): Pascal's resolution price `r` of the market's long side for a long leg,
/// `1 − r` for a short leg. `r_u` above `1e6` is clamped to `1e6`.
pub fn r_tilde(side: Side, r_u: u64) -> u64 {
    let r = r_u.min(UNIT);
    match side {
        Side::Long => r,
        Side::Short => UNIT - r,
    }
}

/// Settlement (spec §10, C2): `paid = P · Πᵢ r̃ᵢ`, floored after every leg, in ticket order.
/// An `r̃` above `1e6` is an error.
pub fn settle(payout_u: u64, rtilde_ppm: &[u64]) -> Result<u64, PricingError> {
    let mut acc = u128::from(payout_u);
    for &rt in rtilde_ppm {
        if rt > UNIT {
            return Err(PricingError::InvalidPrice(rt));
        }
        acc = acc * u128::from(rt) / UNIT_128;
    }
    to_u64(acc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legs(m1: u32, f: u64, n: usize) -> Vec<PayoutLeg> {
        vec![PayoutLeg { m1_ppm: m1, f_u: f }; n]
    }

    /// Spec §2 step 3 table: m(n) to 0.1% and odds at 0.50 to 2 d.p., both rounded half-up as the
    /// table prints them (27.4795 shows as 27.48).
    #[test]
    fn spec_margin_table() {
        let rows: [(usize, u32, u32, u64, u64, u64); 5] = [
            // n, m seq (per mille), m overlap, fair, offered seq, offered overlap (centi-odds)
            (1, 30, 50, 200, 194, 190),
            (2, 59, 98, 400, 376, 361),
            (3, 87, 143, 800, 730, 686),
            (4, 115, 185, 1600, 1416, 1303),
            (5, 141, 226, 3200, 2748, 2476),
        ];
        let tiers = MarginTiers::default();
        for (n, m_seq, m_ovl, fair, off_seq, off_ovl) in rows {
            let seq = vec![m1_ppm(false, &tiers); n];
            let ovl = vec![m1_ppm(true, &tiers); n];
            // Rounded half-up to 0.1%.
            assert_eq!((m_n_ppm(&seq) + 500) / 1000, m_seq, "m seq n={n}");
            assert_eq!((m_n_ppm(&ovl) + 500) / 1000, m_ovl, "m ovl n={n}");
            let f = payout(1_000_000, 1_000_000, &legs(0, 500_000, n)).unwrap();
            assert_eq!((f.odds_u + 5_000) / 10_000, fair);
            let s = payout(1_000_000, 1_000_000, &legs(30_000, 500_000, n)).unwrap();
            assert_eq!((s.odds_u + 5_000) / 10_000, off_seq, "offered seq n={n}");
            let o = payout(1_000_000, 1_000_000, &legs(50_000, 500_000, n)).unwrap();
            assert_eq!((o.odds_u + 5_000) / 10_000, off_ovl, "offered ovl n={n}");
        }
    }

    /// Spec §8 fee stack: the margin on a 10 USDC, 3-leg ticket is 0.87 sequential and 1.43
    /// overlapping (`fee = rS × m(n)`).
    #[test]
    fn fee_stack_example() {
        let stake = 10_000_000_u64;
        let fee = |m: u32| u128::from(stake) * u128::from(m_n_ppm(&[m; 3])) / UNIT_128;
        // 0.87327 and 1.42625 USDC, shown to the cent.
        assert_eq!(fee(30_000), 873_270);
        assert_eq!(fee(50_000), 1_426_250);
        assert_eq!((fee(30_000) + 5_000) / 10_000, 87);
        assert_eq!((fee(50_000) + 5_000) / 10_000, 143);
    }

    #[test]
    fn payout_errors() {
        assert_eq!(payout(1, 1_000_000, &[]), Err(PricingError::LegCount(0)));
        assert_eq!(
            payout(10, 1_000_000, &legs(0, 0, 1)),
            Err(PricingError::InvalidPrice(0))
        );
        assert_eq!(
            payout(10, 1_000_001, &legs(0, 1, 1)),
            Err(PricingError::InvalidPrice(1_000_001))
        );
        assert_eq!(
            payout(1, 1, &legs(0, 500_000, 1)),
            Err(PricingError::ZeroStake)
        );
    }

    /// Worst cases: 5 legs at the minimum tick overflow u64 cleanly instead of wrapping; the
    /// largest stake at prices near 1 stays exact.
    #[test]
    fn payout_overflow_worst_cases() {
        assert_eq!(
            payout(5_000_000, 1_000_000, &legs(0, 1_000, 5)),
            Err(PricingError::Overflow)
        );
        assert_eq!(
            payout(u64::MAX, 1_000_000, &legs(0, 1_000, 5)),
            Err(PricingError::Overflow)
        );
        // 4 legs at the minimum tick with 5% margin on 5 USDC: 5e6 · 950^4 ... fits.
        let p = payout(5_000_000, 1_000_000, &legs(50_000, 1_000, 4)).unwrap();
        assert_eq!(p.payout_u, 4_072_531_250_000_000_000);
        let q = payout(u64::MAX, 1_000_000, &legs(50_000, 999_000, 5)).unwrap();
        assert!(q.payout_u < u64::MAX);
        assert_eq!(settle(u64::MAX, &[UNIT; 5]).unwrap(), u64::MAX);
        assert_eq!(settle(u64::MAX, &[UNIT / 2]).unwrap(), u64::MAX / 2);
    }

    #[test]
    fn r_tilde_sides() {
        assert_eq!(r_tilde(Side::Long, 1_000_000), 1_000_000);
        assert_eq!(r_tilde(Side::Short, 1_000_000), 0);
        assert_eq!(r_tilde(Side::Short, 0), 1_000_000);
        assert_eq!(r_tilde(Side::Short, 500_000), 500_000);
        assert_eq!(r_tilde(Side::Long, 2_000_000), 1_000_000);
    }
}
