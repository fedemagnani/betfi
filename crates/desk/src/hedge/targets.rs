//! Section 3 targets from the desk's tickets, and book helpers for limit prices. Pure functions.
//!
//! - A **pending** ticket (hedged at inception, not minted yet) wants exactly what it kept: the
//!   inception fills minus the excess over `r·Δᵢ` (its legs' `target`, set by the inception).
//! - An **open** ticket wants `σ · hₖ,ᵢ · Δₖ,ᵢ(t)` per leg, with `Δᵢ = P · Πⱼ≠ᵢ qⱼ` where `qⱼ` is the
//!   leg's achieved fill price while unresolved (its quote price when it was left unhedged) and
//!   `r̃ⱼ` once resolved ([`pricing::delta_i_micro`], [`pricing::hedged_set`]). A won leg rolls the
//!   next leg to `S/(p₁…pₖ)`; a lost leg zeroes every delta of the ticket.
//! - Every other state (cashed out, settled, refunded) wants nothing.
//!
//! The symbol's net target is `Tᵢ = round(Σₖ σ hₖ,ᵢ Δₖ,ᵢ)` ([`pricing::net_target`]) over the
//! micro-contract deltas, so fifty copies of one ticket are one position.

use std::collections::{BTreeSet, HashMap};

use pascal_rs::{Level, OrderBook};
use pricing::{HedgeLeg, TargetEntry, TickSpec};

use crate::api::types::{LegStatus, Side, TicketStatus};
use crate::state::{Symbol, TicketId, TicketState};

/// 1.0 in micro-units.
pub const UNIT: u64 = 1_000_000;

/// `σ`: `+1` long, `−1` short.
pub fn sigma(side: Side) -> i64 {
    match side {
        Side::Long => 1,
        Side::Short => -1,
    }
}

/// The pricing crate's side.
pub fn pside(side: Side) -> pricing::Side {
    match side {
        Side::Long => pricing::Side::Long,
        Side::Short => pricing::Side::Short,
    }
}

/// `r̃` implied by a leg status when the resolution value is unknown: won 1, lost 0, half and
/// void 0.5 (spec section 10); `None` while open.
pub fn status_rtilde(status: LegStatus) -> Option<u64> {
    match status {
        LegStatus::Open => None,
        LegStatus::Won => Some(UNIT),
        LegStatus::Lost => Some(0),
        LegStatus::Half | LegStatus::Void => Some(UNIT / 2),
    }
}

/// The leg status of a resolution `r̃`.
pub fn rtilde_status(rtilde: u64) -> LegStatus {
    match rtilde {
        0 => LegStatus::Lost,
        r if r >= UNIT => LegStatus::Won,
        _ => LegStatus::Half,
    }
}

/// Known `r̃` per (ticket, leg).
pub type RTilde = HashMap<(TicketId, u8), u64>;

/// Leg indices per ticket that the pre-hedge trigger added to the hedged set (4.C, D-029).
pub type Prehedged = HashMap<TicketId, BTreeSet<u8>>;

/// Whether leg `idx` of `ticket` was pre-hedged.
pub fn is_prehedged(pre: &Prehedged, ticket: &str, idx: u8) -> bool {
    pre.get(ticket).is_some_and(|s| s.contains(&idx))
}

/// Per leg of `t`: the signed micro-contract delta it contributes to its symbol's net target
/// (`0` when the leg is not in the hedged set or resolved).
pub fn leg_micro(t: &TicketState, rtilde: &RTilde, game_length_ms: u64) -> Vec<i128> {
    leg_micro_with(t, rtilde, &Prehedged::new(), game_length_ms)
}

/// [`leg_micro`] with pre-hedged legs in the hedged set.
pub fn leg_micro_with(
    t: &TicketState,
    rtilde: &RTilde,
    pre: &Prehedged,
    game_length_ms: u64,
) -> Vec<i128> {
    match t.status {
        TicketStatus::Pending => t
            .legs
            .iter()
            .map(|l| i128::from(l.target) * i128::from(UNIT))
            .collect(),
        TicketStatus::Open => open_micro(t, rtilde, pre, game_length_ms),
        _ => vec![0; t.legs.len()],
    }
}

fn open_micro(t: &TicketState, rtilde: &RTilde, pre: &Prehedged, game_length_ms: u64) -> Vec<i128> {
    let states: Vec<pricing::LegState> = t
        .legs
        .iter()
        .map(|l| match l.status {
            LegStatus::Open => pricing::LegState::Unresolved {
                price_u: l.p_fill_u.filter(|&f| f > 0).unwrap_or(l.p_quote_u),
            },
            s => pricing::LegState::Resolved {
                r_tilde_ppm: rtilde
                    .get(&(t.ticket.clone(), l.idx))
                    .copied()
                    .or(status_rtilde(s))
                    .unwrap_or(0),
            },
        })
        .collect();
    let hl: Vec<HedgeLeg> = t
        .legs
        .iter()
        .map(|l| HedgeLeg {
            start_ms: l.start_ms,
            resolved: l.status != LegStatus::Open,
            prehedged: is_prehedged(pre, &t.ticket, l.idx),
        })
        .collect();
    let set = pricing::hedged_set(&hl, game_length_ms);
    t.legs
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if !set[i] {
                return 0;
            }
            let micro = pricing::delta_i_micro(t.payout_u, &states, i).unwrap_or(0);
            i128::try_from(micro).unwrap_or(i128::MAX) * i128::from(sigma(l.side))
        })
        .collect()
}

/// Signed contracts from signed micro-contracts, rounded **up** in magnitude (C1: contract
/// sizes round up): a ticket leg's own target.
pub fn contracts_up(micro: i128) -> i64 {
    let unit = i128::from(UNIT);
    let mag = (micro.abs() + unit - 1) / unit;
    let v = i64::try_from(mag).unwrap_or(i64::MAX);
    if micro < 0 { -v } else { v }
}

/// Sets each leg's `target` of an open ticket from the formula; pending tickets keep theirs,
/// closed ones go to 0. Returns whether any target changed.
pub fn refresh(t: &mut TicketState, rtilde: &RTilde, game_length_ms: u64) -> bool {
    refresh_with(t, rtilde, &Prehedged::new(), game_length_ms)
}

/// [`refresh`] with pre-hedged legs in the hedged set.
pub fn refresh_with(
    t: &mut TicketState,
    rtilde: &RTilde,
    pre: &Prehedged,
    game_length_ms: u64,
) -> bool {
    if t.status == TicketStatus::Pending {
        return false;
    }
    let micro = leg_micro_with(t, rtilde, pre, game_length_ms);
    let mut changed = false;
    for (l, m) in t.legs.iter_mut().zip(micro) {
        let c = contracts_up(m);
        changed |= l.target != c;
        l.target = c;
    }
    changed
}

/// The net target `Tᵢ` of `symbol` over all tickets, and each ticket leg's own target on it.
pub fn symbol_target(
    tickets: &HashMap<TicketId, TicketState>,
    symbol: &str,
    rtilde: &RTilde,
    game_length_ms: u64,
) -> (i64, Vec<(TicketId, u8, i64)>) {
    symbol_target_with(tickets, symbol, rtilde, &Prehedged::new(), game_length_ms)
}

/// [`symbol_target`] with pre-hedged legs in the hedged set.
pub fn symbol_target_with(
    tickets: &HashMap<TicketId, TicketState>,
    symbol: &str,
    rtilde: &RTilde,
    pre: &Prehedged,
    game_length_ms: u64,
) -> (i64, Vec<(TicketId, u8, i64)>) {
    let mut entries = Vec::new();
    let mut legs = Vec::new();
    for t in tickets.values() {
        if !t.legs.iter().any(|l| l.symbol == symbol) {
            continue;
        }
        let micro = leg_micro_with(t, rtilde, pre, game_length_ms);
        for (l, m) in t.legs.iter().zip(micro) {
            if l.symbol != symbol {
                continue;
            }
            entries.push(TargetEntry {
                hedged: m != 0,
                side: if m < 0 {
                    pricing::Side::Short
                } else {
                    pricing::Side::Long
                },
                delta_micro: m.unsigned_abs(),
            });
            legs.push((t.ticket.clone(), l.idx, l.target));
        }
    }
    (pricing::net_target(&entries), legs)
}

/// Every symbol any ticket has a leg on.
pub fn symbols_of(tickets: &HashMap<TicketId, TicketState>) -> Vec<Symbol> {
    let mut v: Vec<Symbol> = tickets
        .values()
        .flat_map(|t| t.legs.iter().map(|l| l.symbol.clone()))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Valid levels of one side, best first.
fn levels(book: &OrderBook, asks: bool) -> Vec<(i64, u64)> {
    let side: &[Level] = if asks { &book.asks } else { &book.bids };
    let mut v: Vec<(i64, u64)> = side
        .iter()
        .filter(|l| l.size > 0 && l.price > 0 && l.price < UNIT as i64)
        .map(|l| (l.price, l.size))
        .collect();
    if asks {
        v.sort_by_key(|&(p, _)| p);
    } else {
        v.sort_by_key(|&(p, _)| std::cmp::Reverse(p));
    }
    v
}

/// The marketable IOC limit for `size` contracts on `side` (a bid walks the asks, an ask the
/// bids): the price of the level where the walk reaches `size`, or the last level when the book
/// holds less (the IOC then fills partly). `None` on an empty side.
pub fn walk_limit(book: &OrderBook, side: pascal_rs::Side, size: u64) -> Option<i64> {
    let lv = levels(book, side == pascal_rs::Side::Bid);
    let mut left = size;
    let mut last = None;
    for (p, s) in lv {
        last = Some(p);
        if s >= left {
            return last;
        }
        left -= s;
    }
    last
}

/// The post-only roll price: one tick inside the opposite best (a bid one tick under the best
/// ask, an ask one tick over the best bid), so it rests at the touch without crossing. `None`
/// on an empty opposite side.
pub fn post_only_price(book: &OrderBook, side: pascal_rs::Side, tick: TickSpec) -> Option<i64> {
    let bid = side == pascal_rs::Side::Bid;
    let (best, _) = *levels(book, bid).first()?;
    let best = u64::try_from(best).ok()?;
    let p = if bid {
        tick.snap_down(best.checked_sub(tick.tick_at(best))?)?
    } else {
        tick.snap_up(best + tick.tick_at(best))?
    };
    i64::try_from(p).ok()
}

/// Contracts on the side that **opens** a net position of sign `target` (asks for a long
/// target, bids for a short one) within `window_u` of the best price: the depth `Caps::max_target`
/// is taken of (D-027).
pub fn opening_depth(book: &OrderBook, target: i64, window_u: u64) -> u64 {
    let side = if target >= 0 {
        pricing::Side::Long
    } else {
        pricing::Side::Short
    };
    pricing::opening_depth(book, side, window_u)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::LegState;

    fn book(asks: &[(i64, u64)], bids: &[(i64, u64)]) -> OrderBook {
        let lv = |v: &[(i64, u64)]| {
            v.iter()
                .map(|&(price, size)| Level { price, size })
                .collect()
        };
        OrderBook {
            asks: lv(asks),
            bids: lv(bids),
            spec: None,
        }
    }

    fn leg(idx: u8, symbol: &str, side: Side, start_ms: u64, p: u64) -> LegState {
        LegState {
            idx,
            symbol: symbol.into(),
            game_id: format!("g{idx}"),
            side,
            start_ms,
            p_quote_u: p,
            p_fill_u: None,
            m1_ppm: 30_000,
            target: 0,
            status: LegStatus::Open,
        }
    }

    fn ticket(id: &str, payout_u: u64, legs: Vec<LegState>) -> TicketState {
        TicketState {
            ticket: id.into(),
            pending: format!("p-{id}"),
            user: "u".into(),
            status: TicketStatus::Open,
            stake_u: 10_000_000,
            rs_u: Some(10_000_000),
            payout_u,
            legs,
        }
    }

    const H: u64 = 3_600_000;
    const GL: u64 = 9_000_000;

    #[test]
    fn sequential_rolls_and_losses() {
        let mut t = ticket(
            "t",
            73_013_840,
            vec![
                leg(0, "A", Side::Long, 0, 500_000),
                leg(1, "B", Side::Short, 5 * H, 400_000),
                leg(2, "C", Side::Long, 10 * H, 625_000),
            ],
        );
        let rt = RTilde::new();
        assert!(refresh(&mut t, &rt, GL));
        assert_eq!(
            t.legs.iter().map(|l| l.target).collect::<Vec<_>>(),
            [19, 0, 0]
        );
        // Leg 0 won: roll into B (short) at S(1-m)³/(p1 p2) = 45.63 -> 46 contracts short.
        t.legs[0].status = LegStatus::Won;
        refresh(&mut t, &rt, GL);
        assert_eq!(
            t.legs.iter().map(|l| l.target).collect::<Vec<_>>(),
            [0, -46, 0]
        );
        let mut tickets = HashMap::new();
        tickets.insert("t".to_owned(), t.clone());
        assert_eq!(symbol_target(&tickets, "B", &rt, GL).0, -46);
        // Leg 1 lost: everything goes to 0.
        t.legs[1].status = LegStatus::Lost;
        refresh(&mut t, &rt, GL);
        assert!(t.legs.iter().all(|l| l.target == 0));
    }

    #[test]
    fn overlapping_ladder_and_netting() {
        let a = ticket(
            "a",
            40_000_000,
            vec![
                leg(0, "A", Side::Long, 0, 500_000),
                leg(1, "B", Side::Long, H, 500_000),
            ],
        );
        let mut b = ticket(
            "b",
            40_000_000,
            vec![
                leg(0, "A", Side::Short, 0, 500_000),
                leg(1, "C", Side::Long, H, 500_000),
            ],
        );
        let rt = RTilde::new();
        let mut tickets = HashMap::new();
        tickets.insert("a".to_owned(), a);
        assert_eq!(symbol_target(&tickets, "A", &rt, GL).0, 20);
        assert_eq!(symbol_target(&tickets, "B", &rt, GL).0, 20);
        refresh(&mut b, &rt, GL);
        tickets.insert("b".to_owned(), b);
        // Opposite bets net.
        assert_eq!(symbol_target(&tickets, "A", &rt, GL).0, 0);
        let (_, legs) = symbol_target(&tickets, "A", &rt, GL);
        assert_eq!(legs.len(), 2);
    }

    #[test]
    fn prehedged_leg_joins_the_set() {
        let mut t = ticket(
            "t",
            73_013_840,
            vec![
                leg(0, "A", Side::Long, 0, 500_000),
                leg(1, "B", Side::Short, 5 * H, 400_000),
                leg(2, "C", Side::Long, 10 * H, 625_000),
            ],
        );
        let rt = RTilde::new();
        let mut pre = Prehedged::new();
        pre.entry("t".to_owned()).or_default().insert(2);
        assert!(refresh_with(&mut t, &rt, &pre, GL));
        // C joins at P · p1 · p2 = 73.01384 · 0.5 · 0.4 = 14.6 -> 15; A keeps its 19.
        assert_eq!(
            t.legs.iter().map(|l| l.target).collect::<Vec<_>>(),
            [19, 0, 15]
        );
        let mut tickets = HashMap::new();
        tickets.insert("t".to_owned(), t.clone());
        assert_eq!(symbol_target_with(&tickets, "C", &rt, &pre, GL).0, 15);
        assert_eq!(symbol_target(&tickets, "C", &rt, GL).0, 0);
        // Once resolved it leaves the set like any leg.
        t.legs[2].status = LegStatus::Won;
        refresh_with(&mut t, &rt, &pre, GL);
        assert_eq!(t.legs[2].target, 0);
    }

    #[test]
    fn half_resolution_scales() {
        let mut t = ticket(
            "t",
            40_000_000,
            vec![
                leg(0, "A", Side::Long, 0, 500_000),
                leg(1, "B", Side::Long, 5 * H, 500_000),
            ],
        );
        t.legs[0].status = LegStatus::Half;
        let mut rt = RTilde::new();
        rt.insert(("t".to_owned(), 0), 500_000);
        refresh(&mut t, &rt, GL);
        assert_eq!(t.legs[1].target, 20);
    }

    #[test]
    fn limits_and_depth() {
        let b = book(
            &[(700_000, 5), (710_000, 10)],
            &[(690_000, 3), (680_000, 50)],
        );
        assert_eq!(walk_limit(&b, pascal_rs::Side::Bid, 5), Some(700_000));
        assert_eq!(walk_limit(&b, pascal_rs::Side::Bid, 6), Some(710_000));
        assert_eq!(walk_limit(&b, pascal_rs::Side::Bid, 99), Some(710_000));
        assert_eq!(walk_limit(&b, pascal_rs::Side::Ask, 4), Some(680_000));
        assert_eq!(
            post_only_price(&b, pascal_rs::Side::Bid, TickSpec::NBA),
            Some(699_000)
        );
        assert_eq!(
            post_only_price(&b, pascal_rs::Side::Ask, TickSpec::NBA),
            Some(691_000)
        );
        assert_eq!(opening_depth(&b, 5, 20_000), 15);
        assert_eq!(opening_depth(&b, -5, 20_000), 53);
        assert_eq!(walk_limit(&book(&[], &[]), pascal_rs::Side::Bid, 1), None);
        assert_eq!(contracts_up(-1_000_001), -2);
        assert_eq!(contracts_up(2_000_000), 2);
    }
}
