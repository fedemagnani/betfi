//! Depth walk over a Pascal L2 book (spec §2 "Inputs": `askᵢ(q)`, `bidᵢ(q)`).
//!
//! Levels are read defensively: a level with size 0 or a price outside `(0, 1)` is skipped, and
//! each side is sorted best-first before walking, so the result does not depend on the order the
//! venue (or the mock) sent.

use pascal_rs::{Level, OrderBook};

use crate::{Side, UNIT, div_ceil};

/// Width of the executable-depth window, micro-units: depth counts the contracts priced within
/// 0.02 of the best price on a side, inclusive (proposed decision D-1B-a).
///
/// D-021: NBA books are a market maker's ladder at about 0.01 spacing, so a 5-tick window
/// (0.005) often sees only the top level. Two cents see the best level plus the next one or two
/// rungs (e.g. ORL asks on 28 Oct: 16,474 contracts within 0.01, 66,474 within 0.02).
pub const DEPTH_WINDOW_U: u64 = 20_000;

/// Valid levels of one side as `(price_u, size)`, best first.
fn side_levels(levels: &[Level], best_is_lowest: bool) -> Vec<(u64, u64)> {
    let mut v: Vec<(u64, u64)> = levels
        .iter()
        .filter(|l| l.size > 0 && l.price > 0 && (l.price as u64) < UNIT)
        .map(|l| (l.price as u64, l.size))
        .collect();
    if best_is_lowest {
        v.sort_by_key(|&(p, _)| p);
    } else {
        v.sort_by_key(|&(p, _)| std::cmp::Reverse(p));
    }
    v
}

/// Σ price × size over the first `q` contracts of `levels` (best first), or `None` when the side
/// holds fewer than `q` contracts.
fn walk_notional(levels: &[(u64, u64)], q: u64) -> Option<u128> {
    let mut left = q;
    let mut notional: u128 = 0;
    for &(price, size) in levels {
        if left == 0 {
            break;
        }
        let take = size.min(left);
        notional += u128::from(price) * u128::from(take);
        left -= take;
    }
    (left == 0).then_some(notional)
}

/// `askᵢ(q)`: the depth-weighted average price to **buy** `q` contracts, walking asks from the
/// lowest, `ceil(Σ priceₖ·takenₖ / q)` (average fills round up, C1).
///
/// `None` when `q = 0` or the asks hold fewer than `q` contracts.
pub fn ask(book: &OrderBook, q: u64) -> Option<u64> {
    if q == 0 {
        return None;
    }
    let n = walk_notional(&side_levels(&book.asks, true), q)?;
    Some(div_ceil(n, u128::from(q)) as u64)
}

/// `bidᵢ(q)`: the depth-weighted average price to **sell** `q` contracts, walking bids from the
/// highest, `floor(Σ priceₖ·takenₖ / q)`.
///
/// Rounded down, the house's side: a short leg's price `1 − bid` and a long leg's cash-out
/// factor `bid` then both favour the house.
///
/// `None` when `q = 0` or the bids hold fewer than `q` contracts.
pub fn bid(book: &OrderBook, q: u64) -> Option<u64> {
    if q == 0 {
        return None;
    }
    let n = walk_notional(&side_levels(&book.bids, false), q)?;
    Some((n / u128::from(q)) as u64)
}

/// Best bid and best ask, if both sides have a valid level.
fn best_bid_ask(book: &OrderBook) -> Option<(u64, u64)> {
    let b = side_levels(&book.bids, false).first()?.0;
    let a = side_levels(&book.asks, true).first()?.0;
    Some((b, a))
}

/// Mid price of the long symbol, `floor((best bid + best ask) / 2)`; `None` on a one-sided book.
pub fn mid(book: &OrderBook) -> Option<u64> {
    best_bid_ask(book).map(|(b, a)| (b + a) / 2)
}

/// Indicative mid `p̃ᵢ` of the user's side (spec §2 step 2), rounded up: long `ceil((b + a)/2)`,
/// short `1 − floor((b + a)/2)`. A higher price means lower fair odds, the house's side.
pub fn side_mid(book: &OrderBook, side: Side) -> Option<u64> {
    let (b, a) = best_bid_ask(book)?;
    Some(match side {
        Side::Long => (b + a).div_ceil(2),
        Side::Short => UNIT - (b + a) / 2,
    })
}

/// Contracts on one side priced within `window_u` of that side's best price, inclusive (asks:
/// `price ≤ best + w`; bids: `price ≥ best − w`).
fn depth_within(levels: &[(u64, u64)], best_is_lowest: bool, window_u: u64) -> u64 {
    let Some(&(best, _)) = levels.first() else {
        return 0;
    };
    levels
        .iter()
        .take_while(|&&(p, _)| {
            if best_is_lowest {
                p <= best.saturating_add(window_u)
            } else {
                p >= best.saturating_sub(window_u)
            }
        })
        .map(|&(_, s)| s)
        .fold(0_u64, u64::saturating_add)
}

/// Executable depth on the side that **opens** (hedges) a leg: asks for a long leg (the desk
/// bids), bids for a short leg (the desk asks), within `window_u` of the best price.
pub fn opening_depth(book: &OrderBook, side: Side, window_u: u64) -> u64 {
    match side {
        Side::Long => depth_within(&side_levels(&book.asks, true), true, window_u),
        Side::Short => depth_within(&side_levels(&book.bids, false), false, window_u),
    }
}

/// Executable depth on the side that **closes** a leg (spec §2 cash-out, §10): bids for a long
/// leg, asks for a short one, within `window_u` of the best price.
pub fn executable_depth(book: &OrderBook, side: Side, window_u: u64) -> u64 {
    match side {
        Side::Long => depth_within(&side_levels(&book.bids, false), false, window_u),
        Side::Short => depth_within(&side_levels(&book.asks, true), true, window_u),
    }
}

/// `b̃ⱼ` of the cash-out formula: what the user's side of `q` contracts sells for now, `bid(q)`
/// for a long leg and `1 − ask(q)` for a short one (both round down).
pub fn close_price(book: &OrderBook, side: Side, q: u64) -> Option<u64> {
    match side {
        Side::Long => bid(book, q),
        Side::Short => ask(book, q).map(|a| UNIT - a),
    }
}

/// Sum of `size` over valid levels (test helper and diagnostics).
#[cfg(test)]
pub(crate) fn total(levels: &[Level]) -> u64 {
    side_levels(levels, true).iter().map(|&(_, s)| s).sum()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn lv(price: i64, size: u64) -> Level {
        Level { price, size }
    }

    pub(crate) fn book(asks: &[(i64, u64)], bids: &[(i64, u64)]) -> OrderBook {
        OrderBook {
            asks: asks.iter().map(|&(p, s)| lv(p, s)).collect(),
            bids: bids.iter().map(|&(p, s)| lv(p, s)).collect(),
            spec: None,
        }
    }

    #[test]
    fn walk_exact_level_boundaries() {
        let b = book(
            &[(500_000, 10), (510_000, 10)],
            &[(490_000, 10), (480_000, 5)],
        );
        assert_eq!(ask(&b, 10), Some(500_000));
        // 10 at 0.50 + 1 at 0.51 = 5.51 / 11 = 0.500909.. -> up
        assert_eq!(ask(&b, 11), Some(500_910));
        assert_eq!(ask(&b, 20), Some(505_000));
        assert_eq!(ask(&b, 21), None);
        assert_eq!(ask(&b, 0), None);
        assert_eq!(bid(&b, 10), Some(490_000));
        // 10 at 0.49 + 1 at 0.48 = 5.38 / 11 = 0.489090.. -> down
        assert_eq!(bid(&b, 11), Some(489_090));
        assert_eq!(bid(&b, 15), Some(486_666));
        assert_eq!(bid(&b, 16), None);
    }

    #[test]
    fn unsorted_and_invalid_levels() {
        let b = book(
            &[
                (510_000, 10),
                (0, 99),
                (500_000, 10),
                (1_000_000, 5),
                (505_000, 0),
            ],
            &[(480_000, 5), (490_000, 10), (-1, 7)],
        );
        assert_eq!(ask(&b, 10), Some(500_000));
        assert_eq!(bid(&b, 10), Some(490_000));
        assert_eq!(total(&b.asks), 20);
        assert_eq!(mid(&b), Some(495_000));
    }

    #[test]
    fn mids_round_up_per_side() {
        let b = book(&[(500_000, 1)], &[(489_999, 1)]);
        assert_eq!(mid(&b), Some(494_999));
        assert_eq!(side_mid(&b, Side::Long), Some(495_000));
        assert_eq!(side_mid(&b, Side::Short), Some(505_001));
        assert_eq!(side_mid(&book(&[], &[(1, 1)]), Side::Long), None);
    }

    #[test]
    fn depth_window_is_inclusive() {
        let b = book(
            &[
                (379_000, 16_454),
                (385_000, 20),
                (391_000, 50_000),
                (401_000, 35_304),
            ],
            &[(371_000, 50_000), (360_000, 20), (351_000, 7), (350_000, 3)],
        );
        assert_eq!(opening_depth(&b, Side::Long, 10_000), 16_474);
        assert_eq!(opening_depth(&b, Side::Long, DEPTH_WINDOW_U), 66_474);
        assert_eq!(executable_depth(&b, Side::Short, DEPTH_WINDOW_U), 66_474);
        // bids within 0.02 of 0.371: down to 0.351 inclusive.
        assert_eq!(executable_depth(&b, Side::Long, DEPTH_WINDOW_U), 50_027);
        assert_eq!(opening_depth(&b, Side::Short, DEPTH_WINDOW_U), 50_027);
        assert_eq!(
            opening_depth(&book(&[], &[]), Side::Long, DEPTH_WINDOW_U),
            0
        );
    }

    #[test]
    fn close_price_sides() {
        let b = book(&[(510_000, 10)], &[(490_000, 10)]);
        assert_eq!(close_price(&b, Side::Long, 5), Some(490_000));
        assert_eq!(close_price(&b, Side::Short, 5), Some(490_000));
        assert_eq!(close_price(&b, Side::Short, 11), None);
    }
}
