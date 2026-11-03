//! Live market data held by the engine in live mode (L4-1): Pascal's catalogue as last fetched
//! and Pascal's books as the upstream `book` channel last described them.
//!
//! A book is **fresh** from its upstream snapshot until the socket drops (or lags, or the
//! upstream refuses the channel); it is then **stale** until the next snapshot. The matcher
//! never fills against a stale book ([`crate::Engine::place`] refuses with `NOT_READY`); `/books`
//! keeps serving its last levels so quotes do not flicker during a short reconnect.
//!
//! These are the upstream's levels. What the mock serves is these minus the liquidity its own
//! fills consumed plus its resting orders (D-034, D-036): the engine keeps the consumed ledger
//! and drops a level's entry when the upstream next updates that level (or sends a snapshot).

use std::collections::BTreeMap;

use pascal_rs::{Level, Market, Side};
use serde_json::Value;

/// One catalogue entry.
#[derive(Debug, Clone)]
pub(crate) struct LiveMarket {
    /// Typed.
    pub market: Market,
    /// As served by the upstream (verbatim JSON).
    pub raw: Value,
    /// Listed by the last `GET /markets` refresh (a market that left the list is kept for
    /// `/markets/search`, but no longer served by `/markets`).
    pub listed: bool,
}

/// One upstream book.
#[derive(Debug, Clone, Default)]
pub(crate) struct LiveBook {
    /// Price → size.
    pub asks: BTreeMap<i64, u64>,
    /// Price → size.
    pub bids: BTreeMap<i64, u64>,
    /// Snapshot received since the last socket drop.
    pub fresh: bool,
    /// Snapshots received.
    pub snapshots: u64,
}

impl LiveBook {
    /// Levels of one side, best first.
    pub fn levels(&self, side: Side) -> Vec<Level> {
        let level = |(&price, &size): (&i64, &u64)| Level { price, size };
        match side {
            Side::Ask => self.asks.iter().map(level).collect(),
            Side::Bid => self.bids.iter().rev().map(level).collect(),
        }
    }

    /// Replaces both sides.
    pub fn replace(&mut self, asks: &[Level], bids: &[Level]) {
        self.asks = asks
            .iter()
            .filter(|l| l.size > 0)
            .map(|l| (l.price, l.size))
            .collect();
        self.bids = bids
            .iter()
            .filter(|l| l.size > 0)
            .map(|l| (l.price, l.size))
            .collect();
    }

    /// Sets one level (size 0 removes it).
    pub fn set(&mut self, side: Side, price: i64, size: u64) {
        let map = match side {
            Side::Ask => &mut self.asks,
            Side::Bid => &mut self.bids,
        };
        if size == 0 {
            map.remove(&price);
        } else {
            map.insert(price, size);
        }
    }
}

/// The live feed: catalogue and books.
#[derive(Debug, Clone, Default)]
pub struct LiveFeed {
    pub(crate) markets: BTreeMap<String, LiveMarket>,
    pub(crate) books: BTreeMap<String, LiveBook>,
    /// Local time of the last full catalogue refresh, Unix ms.
    pub(crate) catalogue_ms: Option<u64>,
}

/// Counters for `GET /mock/info`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FeedCounts {
    /// Books the mock holds (subscribed symbols that got at least one snapshot, or are waiting
    /// for their first).
    pub books: usize,
    /// Books that are not fresh (no fills on them).
    pub stale_books: usize,
    /// Catalogue entries.
    pub markets: usize,
    /// Local time of the last catalogue refresh.
    pub catalogue_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_best_first_and_zero_removes() {
        let mut b = LiveBook::default();
        b.replace(
            &[
                Level {
                    price: 570_000,
                    size: 20,
                },
                Level {
                    price: 560_000,
                    size: 12,
                },
            ],
            &[
                Level {
                    price: 530_000,
                    size: 7,
                },
                Level {
                    price: 540_000,
                    size: 15,
                },
            ],
        );
        assert_eq!(b.levels(Side::Ask)[0].price, 560_000);
        assert_eq!(b.levels(Side::Bid)[0].price, 540_000);
        b.set(Side::Ask, 560_000, 0);
        b.set(Side::Bid, 545_000, 3);
        assert_eq!(b.levels(Side::Ask)[0].price, 570_000);
        assert_eq!(b.levels(Side::Bid)[0].price, 545_000);
    }
}
