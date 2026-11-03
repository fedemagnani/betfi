//! Section 2 step 5: finalize at execution.
//!
//! ```text
//! fᵢ = ceil(notional_filledᵢ / size_filledᵢ)      hedged legs (average fill, rounded up)
//! fᵢ = pᵢ                                          legs left unhedged by the roll policy
//! r  = minᵢ∈hedged floor(size_filledᵢ · 1e6 / Δᵢ)  capped at 1e6
//! P_final, rS, refund: C2 with these fᵢ and r;  mint only if P_final/(rS) ≥ min_payout_per_unit
//! ```

use serde::{Deserialize, Serialize};

use crate::payout::{PayoutLeg, floor_ok, payout};
use crate::{PricingError, Side, UNIT, UNIT_128, div_ceil, to_u64};

/// One leg's hedge result at inception.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegFill {
    /// Whether the leg was in the inception hedged set (an order was sent).
    pub hedged: bool,
    /// `Δᵢ`, the contracts the inception order asked for (ignored when not hedged).
    pub delta: u64,
    /// Contracts filled.
    pub filled: u64,
    /// Σ price × size of the fills **in the user's side's terms**, micro-USDC (use
    /// [`side_notional_u`] for a short leg's `ASK` fills on the long symbol).
    pub notional_u: u64,
    /// `pᵢ`, the signed quote price (the leg's `fᵢ` when unhedged).
    pub quote_price_u: u64,
    /// `m₁,ᵢ` from the signed quote.
    pub m1_ppm: u32,
}

/// The user's side notional of `filled` contracts whose fills on the long symbol total
/// `long_notional_u`: unchanged for a long leg; for a short leg the desk **sold** the long side
/// at `x`, i.e. bought the user's side at `1 − x`, so the notional is `filled · 1e6 − Σ x`.
pub fn side_notional_u(side: Side, filled: u64, long_notional_u: u64) -> u64 {
    match side {
        Side::Long => long_notional_u,
        Side::Short => u64::try_from(
            (u128::from(filled) * UNIT_128).saturating_sub(u128::from(long_notional_u)),
        )
        .unwrap_or(u64::MAX),
    }
}

/// The result of [`finalize()`]: the arguments of the program's `finalize(fills, r_ppm)` (C3) and
/// the ticket it mints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finalized {
    /// C3's `fills`: `fᵢ` for a hedged leg, `0` for an unhedged one (the program then uses the
    /// stored `pᵢ`).
    pub fills_u: Vec<u64>,
    /// `fᵢ` as used in the payout (unhedged legs at `pᵢ`).
    pub f_u: Vec<u64>,
    /// `r`, ppm, in `(0, 1e6]`.
    pub r_ppm: u32,
    /// `rS`, micro-USDC.
    pub rs_u: u64,
    /// `P_final`, micro-USDC.
    pub payout_u: u64,
    /// `P_final / (rS)`, micro-units.
    pub odds_u: u64,
    /// `(1 − r)S`, micro-USDC.
    pub refund_u: u64,
    /// The program's mint check `P_final/(rS) ≥ min_payout_per_unit`. When false the desk
    /// unwinds and the pending is refunded.
    pub floor_ok: bool,
    /// Contracts filled beyond `⌈r·Δᵢ⌉` per hedged leg, to unwind at once (0 for unhedged legs).
    pub excess: Vec<u64>,
}

/// Step 5: `fᵢ`, `r`, then `P_final = rS · Πᵢ (1 − m₁,ᵢ)/fᵢ` per C2, the refund and the floor
/// check. Legs left unhedged take their quote price `pᵢ` as `fᵢ` and are excluded from `r`.
///
/// Errors: no hedged leg, a hedged leg with `Δᵢ = 0` or with no fill (`r = 0`), an average fill
/// outside `(0, 1]`, or the C2 errors.
pub fn finalize(
    legs: &[LegFill],
    stake_u: u64,
    min_payout_per_unit_u: u64,
) -> Result<Finalized, PricingError> {
    if legs.is_empty() {
        return Err(PricingError::LegCount(0));
    }
    let mut r: Option<u64> = None;
    let mut f_u = Vec::with_capacity(legs.len());
    let mut fills_u = Vec::with_capacity(legs.len());
    for (i, leg) in legs.iter().enumerate() {
        if !leg.hedged {
            f_u.push(leg.quote_price_u);
            fills_u.push(0);
            continue;
        }
        if leg.delta == 0 {
            return Err(PricingError::ZeroDelta(i));
        }
        let ri = (u128::from(leg.filled) * UNIT_128 / u128::from(leg.delta)).min(UNIT_128) as u64;
        if ri == 0 {
            return Err(PricingError::NoFill(i));
        }
        let f = to_u64(div_ceil(u128::from(leg.notional_u), u128::from(leg.filled)))?;
        if f == 0 || f > UNIT {
            return Err(PricingError::InvalidPrice(f));
        }
        r = Some(r.map_or(ri, |r| r.min(ri)));
        f_u.push(f);
        fills_u.push(f);
    }
    let r_ppm = r.ok_or(PricingError::NoHedgedLeg)? as u32;
    let pl: Vec<PayoutLeg> = legs
        .iter()
        .zip(&f_u)
        .map(|(l, &f)| PayoutLeg {
            m1_ppm: l.m1_ppm,
            f_u: f,
        })
        .collect();
    let p = payout(stake_u, r_ppm, &pl)?;
    let excess = legs
        .iter()
        .map(|l| {
            if !l.hedged {
                return 0;
            }
            let keep = div_ceil(u128::from(l.delta) * u128::from(r_ppm), UNIT_128) as u64;
            l.filled.saturating_sub(keep)
        })
        .collect();
    Ok(Finalized {
        fills_u,
        f_u,
        r_ppm,
        rs_u: p.rs_u,
        payout_u: p.payout_u,
        odds_u: p.odds_u,
        refund_u: p.refund_u,
        floor_ok: floor_ok(p.payout_u, p.rs_u, min_payout_per_unit_u),
        excess,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hedged(delta: u64, filled: u64, notional_u: u64, p: u64) -> LegFill {
        LegFill {
            hedged: true,
            delta,
            filled,
            notional_u,
            quote_price_u: p,
            m1_ppm: 50_000,
        }
    }

    #[test]
    fn full_fill_matches_quote() {
        // 3 overlapping legs at 0.50, all filled at 0.50: the C2 vector.
        let legs = [hedged(20, 20, 10_000_000, 500_000); 3];
        let f = finalize(&legs, 10_000_000, 6_653_230).unwrap();
        assert_eq!(f.r_ppm, 1_000_000);
        assert_eq!(f.payout_u, 68_590_000);
        assert_eq!(f.odds_u, 6_859_000);
        assert_eq!(f.refund_u, 0);
        assert!(f.floor_ok);
        assert_eq!(f.fills_u, vec![500_000; 3]);
        assert_eq!(f.excess, vec![0; 3]);
    }

    #[test]
    fn partial_fill_and_unhedged_leg() {
        // Leg 0 fills 12 of 20 at 0.50 and 0.51 (avg 0.5025 -> f), leg 1 fills 20 of 20, leg 2
        // is left to the roll at its quote price 0.40.
        let legs = [
            hedged(20, 12, 6 * 500_000 + 6 * 510_000, 500_000),
            hedged(20, 20, 10_000_000, 500_000),
            LegFill {
                hedged: false,
                delta: 0,
                filled: 0,
                notional_u: 0,
                quote_price_u: 400_000,
                m1_ppm: 30_000,
            },
        ];
        let f = finalize(&legs, 10_000_000, 0).unwrap();
        assert_eq!(f.r_ppm, 600_000);
        assert_eq!(f.rs_u, 6_000_000);
        assert_eq!(f.refund_u, 4_000_000);
        assert_eq!(f.f_u, vec![505_000, 500_000, 400_000]);
        assert_eq!(f.fills_u, vec![505_000, 500_000, 0]);
        // 6 · 0.95/0.505 = 11.287128 -> · 0.95/0.5 = 21.445543 -> · 0.97/0.4 = 52.005441
        assert_eq!(f.payout_u, 52_005_441);
        // Leg 1 holds 20 but the ticket needs ⌈0.6 · 20⌉ = 12: unwind 8.
        assert_eq!(f.excess, vec![0, 8, 0]);
    }

    #[test]
    fn floor_failure_and_errors() {
        // Filled above the max price: floor fails, the pending is refunded.
        let legs = [hedged(20, 20, 20 * 520_000, 500_000); 3];
        assert!(!finalize(&legs, 10_000_000, 6_653_230).unwrap().floor_ok);
        let none = [hedged(20, 0, 0, 500_000)];
        assert_eq!(finalize(&none, 10_000_000, 0), Err(PricingError::NoFill(0)));
        let mut unhedged = hedged(20, 0, 0, 500_000);
        unhedged.hedged = false;
        assert_eq!(
            finalize(&[unhedged], 10_000_000, 0),
            Err(PricingError::NoHedgedLeg)
        );
        assert_eq!(
            finalize(&[hedged(0, 1, 1, 1)], 10_000_000, 0),
            Err(PricingError::ZeroDelta(0))
        );
        // Over-fill counts as r = 1, the extra is excess.
        let over = finalize(&[hedged(10, 12, 12 * 500_000, 500_000)], 10_000_000, 0).unwrap();
        assert_eq!(over.r_ppm, 1_000_000);
        assert_eq!(over.excess, vec![2]);
    }

    #[test]
    fn short_leg_notional() {
        // Selling 10 long at 0.371 buys the short side at 0.629.
        assert_eq!(side_notional_u(Side::Short, 10, 3_710_000), 6_290_000);
        assert_eq!(side_notional_u(Side::Long, 10, 3_710_000), 3_710_000);
    }
}
