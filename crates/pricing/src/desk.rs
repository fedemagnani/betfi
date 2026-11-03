//! Section 3: deltas, net targets, the hedged set, rebalance and pre-hedge triggers.

use serde::{Deserialize, Serialize};

use crate::{PricingError, Side, UNIT, UNIT_128, div_ceil, to_u64};

/// A leg's state for the delta formula: `qⱼ(t)` is the current price of the user's side while
/// unresolved, and `r̃ⱼ` once resolved (1 won, 0 lost, 0.5 half-void).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LegState {
    /// Not resolved; `price_u` is `pⱼ(t)`, the user's side price now (a mid, or the quote
    /// price at inception).
    Unresolved {
        /// `pⱼ(t)`, micro-units.
        price_u: u64,
    },
    /// Resolved at `r̃ⱼ` (see [`r_tilde`](crate::r_tilde)).
    Resolved {
        /// `r̃ⱼ`, ppm.
        r_tilde_ppm: u64,
    },
}

impl LegState {
    /// `qⱼ(t)`, ppm.
    pub fn q(self) -> u64 {
        match self {
            LegState::Unresolved { price_u } => price_u,
            LegState::Resolved { r_tilde_ppm } => r_tilde_ppm,
        }
        .min(UNIT)
    }

    /// Whether the leg is resolved.
    pub fn is_resolved(self) -> bool {
        matches!(self, LegState::Resolved { .. })
    }
}

/// `Δᵢ(t) = P · Πⱼ≠ᵢ qⱼ(t)` in **micro-contracts** (contracts × 10⁶), rounded up: one
/// `ceil(acc · qⱼ / 1e6)` per other leg in ticket order, so the result is at most `n − 1`
/// micro-contracts above the exact value. A resolved leg `i` needs no hedge: 0.
pub fn delta_i_micro(payout_u: u64, legs: &[LegState], i: usize) -> Result<u128, PricingError> {
    let me = legs.get(i).ok_or(PricingError::LegIndex(i))?;
    if me.is_resolved() {
        return Ok(0);
    }
    // P in micro-USDC is P·1e6 micro-contracts of 1-USD contracts.
    let mut acc = u128::from(payout_u);
    for (j, leg) in legs.iter().enumerate() {
        if j != i {
            acc = div_ceil(acc * u128::from(leg.q()), UNIT_128);
        }
    }
    Ok(acc)
}

/// `Δᵢ(t) = P · Πⱼ≠ᵢ qⱼ(t)` in contracts, rounded up (spec §3 "Delta per ticket").
///
/// A lost leg zeroes every other delta; a won leg multiplies the others by `1/pⱼ` relative to
/// before; with one leg left `Δ = P`. For a sequential ticket this gives the roll sizes:
/// `≈ S(1 − m)/p₁` at inception, `≈ S(1 − m)/(p₁p₂)` after leg 1 wins, `P` on the last leg.
pub fn delta_i(payout_u: u64, legs: &[LegState], i: usize) -> Result<u64, PricingError> {
    to_u64(div_ceil(delta_i_micro(payout_u, legs, i)?, UNIT_128))
}

/// One ticket's contribution to a leg's net target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetEntry {
    /// `hₖ,ᵢ`: whether this leg of this ticket is in the ticket's hedged set.
    pub hedged: bool,
    /// The ticket's side on the symbol (`σ`).
    pub side: Side,
    /// `Δₖ,ᵢ(t)` in micro-contracts ([`delta_i_micro`]).
    pub delta_micro: u128,
}

/// `Tᵢ(t) = round(Σₖ hₖ,ᵢ · σₖ,ᵢ · Δₖ,ᵢ(t))` in contracts of the symbol's long side (negative =
/// net short), rounded half away from zero (spec §3 "Net target per leg"). Opposite bets net;
/// fifty copies of one ticket are one position.
pub fn net_target(entries: &[TargetEntry]) -> i64 {
    let sum: i128 = entries
        .iter()
        .filter(|e| e.hedged)
        .map(|e| i128::from(e.side.sigma()) * i128::try_from(e.delta_micro).unwrap_or(i128::MAX))
        .fold(0_i128, i128::saturating_add);
    let half = i128::from(UNIT / 2);
    let unit = i128::from(UNIT);
    let rounded = (sum.abs() + half) / unit;
    let signed = if sum < 0 { -rounded } else { rounded };
    i64::try_from(signed).unwrap_or(if sum < 0 { i64::MIN } else { i64::MAX })
}

/// One leg's input to [`hedged_set`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HedgeLeg {
    /// Start, Unix ms.
    pub start_ms: u64,
    /// Resolved legs are never in the set.
    pub resolved: bool,
    /// Activated by the pre-hedge trigger.
    pub prehedged: bool,
}

/// The hedged set `hₖ,ᵢ` of one ticket (spec §3): among unresolved legs, the next leg to resolve
/// (earliest start), every pre-hedged leg, and, repeatedly, every leg whose game window
/// (`|startᵢ − startⱼ| < game_length`) overlaps a leg already in the set.
///
/// The closure is transitive (proposed decision D-1B-b): with A 00:00, B 02:00, C 04:00 and a
/// 2.5 h game, C joins through B at inception, because A may still be running when C tips off and
/// no roll into C would be possible then.
pub fn hedged_set(legs: &[HedgeLeg], game_length_ms: u64) -> Vec<bool> {
    let mut set = vec![false; legs.len()];
    let next = legs
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.resolved)
        .min_by_key(|(_, l)| l.start_ms)
        .map(|(i, _)| i);
    let Some(next) = next else {
        return set;
    };
    set[next] = true;
    for (i, l) in legs.iter().enumerate() {
        if l.prehedged && !l.resolved {
            set[i] = true;
        }
    }
    loop {
        let mut grew = false;
        for i in 0..legs.len() {
            if set[i] || legs[i].resolved {
                continue;
            }
            let joins = (0..legs.len())
                .any(|k| set[k] && legs[k].start_ms.abs_diff(legs[i].start_ms) < game_length_ms);
            if joins {
                set[i] = true;
                grew = true;
            }
        }
        if !grew {
            return set;
        }
    }
}

/// Rebalance trigger (spec §3): the hedged leg is off target by more than `rel_ppm` of `|Tᵢ|`
/// (10%) **and** by more than `min_contracts` (2).
pub fn needs_rebalance(held: i64, target: i64, rel_ppm: u32, min_contracts: u64) -> bool {
    let diff = held.abs_diff(target);
    diff > min_contracts
        && u128::from(diff) * UNIT_128 > u128::from(target.unsigned_abs()) * u128::from(rel_ppm)
}

/// Pre-hedge trigger (spec §3, v1): an unhedged later leg whose price moved more than
/// `move_u` (5 points = 50_000) against the desk since the quote.
///
/// "Against" is read as the user's side getting **dearer** (proposed decision D-1B-c): the roll
/// must then buy that leg at a higher price than the quote priced in, which is the drift risk
/// the trigger exists for; a cheaper leg only helps the desk.
pub fn prehedge_trigger(quote_price_u: u64, current_price_u: u64, move_u: u64) -> bool {
    current_price_u > quote_price_u.saturating_add(move_u)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payout::{PayoutLeg, payout};

    const H: u64 = 3_600_000;
    const L: u64 = 9_000_000;

    fn open(p: u64) -> LegState {
        LegState::Unresolved { price_u: p }
    }
    fn won() -> LegState {
        LegState::Resolved {
            r_tilde_ppm: 1_000_000,
        }
    }

    /// A 3-leg sequential ticket rolls `S/p₁` → `S/(p₁p₂)` → `P` (each × (1 − m) and rounded up).
    #[test]
    fn sequential_roll_sizes() {
        let (p1, p2, p3) = (500_000, 400_000, 625_000);
        let s = 10_000_000_u64;
        let p = payout(
            s,
            1_000_000,
            &[
                PayoutLeg {
                    m1_ppm: 30_000,
                    f_u: p1,
                },
                PayoutLeg {
                    m1_ppm: 30_000,
                    f_u: p2,
                },
                PayoutLeg {
                    m1_ppm: 30_000,
                    f_u: p3,
                },
            ],
        )
        .unwrap()
        .payout_u;
        // P = 10 · 0.97³ / (0.5 · 0.4 · 0.625) = 73.01384 USDC.
        assert_eq!(p, 73_013_840);
        let inception = [open(p1), open(p2), open(p3)];
        let after1 = [won(), open(p2), open(p3)];
        let after2 = [won(), won(), open(p3)];
        // 73.01384 · 0.4 · 0.625 = 18.25346 -> 19 ≈ S(1−m)/p₁ = 18.25346.
        assert_eq!(delta_i(p, &inception, 0).unwrap(), 19);
        // 73.01384 · 0.625 = 45.6336 -> 46 ≈ S(1−m)/(p₁p₂) = 45.6336.
        assert_eq!(delta_i(p, &after1, 1).unwrap(), 46);
        // Last leg: Δ = P = 73.01384 -> 74.
        assert_eq!(delta_i(p, &after2, 2).unwrap(), 74);
        // Exact relation to the stake for the first two.
        let keep = 912_673_u128; // 0.97³
        assert_eq!(
            delta_i(p, &inception, 0).unwrap() as u128,
            (u128::from(s) * keep / 1_000_000).div_ceil(u128::from(p1))
        );
        assert_eq!(
            delta_i(p, &after1, 1).unwrap() as u128,
            (u128::from(s) * keep / 1_000_000)
                .div_ceil(u128::from(p1) * u128::from(p2) / 1_000_000)
        );
        // Resolved legs and lost legs.
        assert_eq!(delta_i(p, &after1, 0).unwrap(), 0);
        let lost = [LegState::Resolved { r_tilde_ppm: 0 }, open(p2), open(p3)];
        assert_eq!(delta_i(p, &lost, 1).unwrap(), 0);
        assert_eq!(delta_i(p, &lost, 7), Err(PricingError::LegIndex(7)));
    }

    #[test]
    fn hedged_set_sequential_and_overlapping() {
        let leg = |start_ms, resolved| HedgeLeg {
            start_ms,
            resolved,
            prehedged: false,
        };
        // Sequential: only the first.
        let seq = [leg(0, false), leg(5 * H, false), leg(10 * H, false)];
        assert_eq!(hedged_set(&seq, L), vec![true, false, false]);
        // After the first resolves, the second.
        let seq2 = [leg(0, true), leg(5 * H, false), leg(10 * H, false)];
        assert_eq!(hedged_set(&seq2, L), vec![false, true, false]);
        // Same-night overlap: all three.
        let night = [leg(0, false), leg(H / 2, false), leg(H, false)];
        assert_eq!(hedged_set(&night, L), vec![true; 3]);
        // Chain A–B–C (C overlaps only B): transitive.
        let chain = [leg(0, false), leg(2 * H, false), leg(4 * H, false)];
        assert_eq!(hedged_set(&chain, L), vec![true; 3]);
        // Ticket order differs from start order.
        let unordered = [leg(10 * H, false), leg(0, false)];
        assert_eq!(hedged_set(&unordered, L), vec![false, true]);
        // Pre-hedged later leg.
        let pre = [
            leg(0, false),
            HedgeLeg {
                start_ms: 10 * H,
                resolved: false,
                prehedged: true,
            },
        ];
        assert_eq!(hedged_set(&pre, L), vec![true, true]);
        assert_eq!(hedged_set(&[leg(0, true)], L), vec![false]);
    }

    #[test]
    fn net_target_nets_and_rounds() {
        let e = |hedged, side, delta_micro| TargetEntry {
            hedged,
            side,
            delta_micro,
        };
        assert_eq!(
            net_target(&[
                e(true, Side::Long, 20_400_000),
                e(true, Side::Short, 5_000_000),
                e(false, Side::Long, 99_000_000),
            ]),
            15
        );
        // 50 copies of 0.5 contracts are 25 contracts, not 50.
        assert_eq!(net_target(&vec![e(true, Side::Long, 500_000); 50]), 25);
        assert_eq!(net_target(&[e(true, Side::Short, 2_500_000)]), -3);
        assert_eq!(net_target(&[e(true, Side::Short, 2_499_999)]), -2);
        assert_eq!(net_target(&[]), 0);
    }

    #[test]
    fn rebalance_rule() {
        // Off by 3 on 20: 15% and > 2 -> yes.
        assert!(needs_rebalance(17, 20, 100_000, 2));
        // Off by 3 on 40: 7.5% -> no.
        assert!(!needs_rebalance(37, 40, 100_000, 2));
        // Off by 2 on 5: 40% but not > 2 contracts -> no.
        assert!(!needs_rebalance(3, 5, 100_000, 2));
        // Exactly 10%: not more than -> no.
        assert!(!needs_rebalance(27, 30, 100_000, 2));
        assert!(needs_rebalance(26, 30, 100_000, 2));
        // Target 0, holding 3: unwind.
        assert!(needs_rebalance(3, 0, 100_000, 2));
        assert!(needs_rebalance(-10, 10, 100_000, 2));
    }

    #[test]
    fn prehedge() {
        assert!(!prehedge_trigger(400_000, 450_000, 50_000));
        assert!(prehedge_trigger(400_000, 450_001, 50_000));
        assert!(!prehedge_trigger(400_000, 300_000, 50_000));
    }
}
