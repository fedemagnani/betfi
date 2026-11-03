//! Cash-out quote (spec §2 "Cash-out quote", §10 "Cash-out terms").
//!
//! ```text
//! C(t) = P · Πⱼ∈resolved r̃ⱼ · Πⱼ∈unresolved b̃ⱼ · (1 − c),   b̃ⱼ = bidⱼ(Δⱼ) long, 1 − askⱼ(Δⱼ) short
//! ```
//!
//! Floored after every factor, in ticket order, then the fee `c` (2%). Offered only when no leg
//! is lost, at least one leg is unresolved, and every unresolved leg has executable depth on its
//! closing side (within the depth window) of at least `Δⱼ(t)`.

use pascal_rs::OrderBook;
use serde::{Deserialize, Serialize};

use crate::book::{close_price, executable_depth, side_mid};
use crate::desk::{LegState, delta_i};
use crate::payout::r_tilde;
use crate::rules::{Params, RejectCode, Rejection};
use crate::{Side, UNIT_128};

/// A leg's resolution status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LegStatus {
    /// Not resolved yet.
    Open,
    /// Resolved by Pascal at `r_u`, the long side's settlement price (1e6 = long won).
    Resolved {
        /// Pascal's `resolution.price`, micro-units.
        r_u: u64,
    },
}

/// One leg of a ticket being cashed out.
#[derive(Debug, Clone, Copy)]
pub struct CashOutLeg<'a> {
    /// Pascal symbol (for reasons).
    pub symbol: &'a str,
    /// The user's side.
    pub side: Side,
    /// Resolution status.
    pub status: LegStatus,
    /// The current book, needed for an open leg.
    pub book: Option<&'a OrderBook>,
}

/// A cash-out value and how it was built.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CashOut {
    /// `C`, micro-USDC (≤ `P`).
    pub c_u: u64,
    /// Per leg: `r̃ⱼ` (resolved) or `b̃ⱼ` (open), ppm.
    pub factors_ppm: Vec<u64>,
    /// Per leg: `Δⱼ(t)` in contracts (0 for resolved legs), the size the depth gate checked.
    pub sizes: Vec<u64>,
}

/// `C(t)` for a ticket of payout `payout_u` (spec §2), or the reason cash-out is unavailable.
///
/// `Δⱼ(t) = P · Πₖ≠ⱼ qₖ(t)` uses `r̃ₖ` for resolved legs and the side mid for open ones. The
/// resolved-leg factor matters for a half-void leg (`r̃ = 0.5`): without it a cash-out would pay
/// twice the ticket's value.
pub fn cash_out(
    payout_u: u64,
    legs: &[CashOutLeg<'_>],
    params: &Params,
) -> Result<CashOut, Rejection> {
    use RejectCode::*;
    let mut states = Vec::with_capacity(legs.len());
    for leg in legs {
        states.push(match leg.status {
            LegStatus::Resolved { r_u } => {
                let rt = r_tilde(leg.side, r_u);
                if rt == 0 {
                    return Err(Rejection::new(
                        LegLost,
                        format!("{} is lost: the schedina pays nothing", leg.symbol),
                    ));
                }
                LegState::Resolved { r_tilde_ppm: rt }
            }
            LegStatus::Open => {
                let book = leg.book.ok_or_else(|| {
                    Rejection::new(
                        NoBook,
                        format!("cash-out unavailable: no book for {}", leg.symbol),
                    )
                })?;
                let mid = side_mid(book, leg.side).ok_or_else(|| {
                    Rejection::new(
                        OneSidedBook,
                        format!("cash-out unavailable: {} has no two-sided book", leg.symbol),
                    )
                })?;
                LegState::Unresolved { price_u: mid }
            }
        });
    }
    if states.iter().all(|s| s.is_resolved()) {
        return Err(Rejection::new(
            AllResolved,
            "every leg is resolved: the schedina settles instead",
        ));
    }
    let unpriceable = |e| Rejection::new(Unpriceable, format!("cash-out cannot be priced ({e})"));
    let mut factors = Vec::with_capacity(legs.len());
    let mut sizes = Vec::with_capacity(legs.len());
    for (j, (leg, state)) in legs.iter().zip(&states).enumerate() {
        match (state, leg.book) {
            (LegState::Resolved { r_tilde_ppm }, _) => {
                factors.push(*r_tilde_ppm);
                sizes.push(0);
            }
            (LegState::Unresolved { .. }, Some(book)) => {
                let q = delta_i(payout_u, &states, j).map_err(unpriceable)?.max(1);
                let depth = executable_depth(book, leg.side, params.depth_window_u);
                let b = (q <= depth)
                    .then(|| close_price(book, leg.side, q))
                    .flatten()
                    .ok_or_else(|| {
                        Rejection::new(
                            NotEnoughDepth,
                            format!(
                                "cash-out unavailable: not enough depth to close {q} contracts on {}",
                                leg.symbol
                            ),
                        )
                    })?;
                factors.push(b);
                sizes.push(q);
            }
            (LegState::Unresolved { .. }, None) => {
                unreachable!("open legs were checked for a book")
            }
        }
    }
    let mut acc = u128::from(payout_u);
    for &f in &factors {
        acc = acc * u128::from(f) / UNIT_128;
    }
    let fee_keep = UNIT_128.saturating_sub(u128::from(params.cash_out_fee_ppm));
    let c_u = (acc * fee_keep / UNIT_128) as u64;
    Ok(CashOut {
        c_u,
        factors_ppm: factors,
        sizes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::tests::book;

    #[test]
    fn half_void_case() {
        // 3 legs at 0.50, P = 68.59. Leg 0 resolved; legs 1 and 2 open with bids at 0.48.
        let p = 68_590_000;
        let b = book(&[(520_000, 1_000)], &[(480_000, 1_000)]);
        let params = Params::default();
        let open = |s| CashOutLeg {
            symbol: s,
            side: Side::Long,
            status: LegStatus::Open,
            book: Some(&b),
        };
        let resolved = |r_u| CashOutLeg {
            symbol: "A",
            side: Side::Long,
            status: LegStatus::Resolved { r_u },
            book: None,
        };
        let won = cash_out(p, &[resolved(1_000_000), open("B"), open("C")], &params).unwrap();
        // 68.59 · 0.48 · 0.48 · 0.98 = 15.48707328
        assert_eq!(won.factors_ppm, vec![1_000_000, 480_000, 480_000]);
        assert_eq!(won.c_u, 15_487_073);
        // Δ for leg 1 = P · 1 · 0.50 = 34.295 -> 35 contracts.
        assert_eq!(won.sizes, vec![0, 35, 35]);
        let half = cash_out(p, &[resolved(500_000), open("B"), open("C")], &params).unwrap();
        // 68.59 · 0.5 · 0.48 · 0.48 · 0.98 = 7.74353664: half the won case, up to flooring.
        assert_eq!(half.c_u, 7_743_536);
        assert!(won.c_u.abs_diff(2 * half.c_u) <= 1);
        // Δ = P · 0.5 · 0.5 = 17.1475 -> 18.
        assert_eq!(half.sizes, vec![0, 18, 18]);
        // A short leg resolved at 0.5 is a half-void too.
        let short_half = CashOutLeg {
            side: Side::Short,
            ..resolved(500_000)
        };
        assert_eq!(
            cash_out(p, &[short_half, open("B"), open("C")], &params)
                .unwrap()
                .c_u,
            half.c_u
        );
        // Lost leg: not offered.
        let lost = cash_out(p, &[resolved(0), open("B"), open("C")], &params).unwrap_err();
        assert_eq!(lost.code, RejectCode::LegLost);
        let all = cash_out(p, &[resolved(1_000_000)], &params).unwrap_err();
        assert_eq!(all.code, RejectCode::AllResolved);
    }

    #[test]
    fn depth_gate_and_short_side() {
        let p = 20_000_000;
        let params = Params::default();
        // Short leg closes on the asks: 1 − ask.
        let b = book(&[(300_000, 100), (330_000, 1_000)], &[(280_000, 1)]);
        let leg = CashOutLeg {
            symbol: "X",
            side: Side::Short,
            status: LegStatus::Open,
            book: Some(&b),
        };
        let c = cash_out(p, &[leg], &params).unwrap();
        // One leg: Δ = P = 20 contracts, ask(20) = 0.30 -> b̃ = 0.70; C = 20 · 0.7 · 0.98.
        assert_eq!(c.sizes, vec![20]);
        assert_eq!(c.c_u, 13_720_000);
        // The 0.33 level is outside the 0.02 window: 101 contracts are not executable.
        let c = cash_out(101_000_000, &[leg], &params).unwrap_err();
        assert_eq!(c.code, RejectCode::NotEnoughDepth);
        let none = CashOutLeg { book: None, ..leg };
        assert_eq!(
            cash_out(p, &[none], &params).unwrap_err().code,
            RejectCode::NoBook
        );
    }
}
