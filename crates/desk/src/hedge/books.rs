//! The hedge desk's live view of the `book` channel (plan step 4.C, spec section 1 `book`).
//!
//! Pascal (and the mock, 2.D) sends a `snapshot` per subscribed symbol, then `update` frames with
//! the changed levels only (size 0 = level removed). [`LiveView`] rebuilds each book from them.
//!
//! A book is **live** only while the socket is connected and the book's snapshot arrived on the
//! current connection (`epoch`): after a drop, or after a reconnect until the fresh snapshot
//! lands, the book is stale and the caller falls back to REST `/books`. An update for a book
//! without a snapshot on the current connection is ignored (it is a diff against a state we do
//! not have).
//!
//! The view is a hedge-private [`Locked`] map; [`LiveView::apply`] returns the copies to publish
//! (into [`AppState::books`](crate::state::Shared::books) and the quoter's cache) so the caller
//! takes those locks one at a time, after this one is released (D-043).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use pascal_rs::{FrameKind, Level, OrderBook};

use crate::quoter::CachedBook;
use crate::state::{LiveBook, Locked, Symbol};

/// One rebuilt book.
#[derive(Debug, Clone, Default)]
struct Entry {
    /// Price → size, asks.
    asks: BTreeMap<i64, u64>,
    /// Price → size, bids.
    bids: BTreeMap<i64, u64>,
    /// Connection the snapshot came on.
    epoch: u64,
    /// Envelope `seq` of the last frame.
    seq: Option<u64>,
    /// Envelope `state_time_ms` of the last frame.
    state_time_ms: Option<u64>,
    /// The venue's clock at the last frame (`server_time_ms`, else `state_time_ms`).
    venue_ms: u64,
    /// Desk clock when the last frame arrived, Unix ms.
    recv_ms: u64,
}

impl Entry {
    fn book(&self) -> OrderBook {
        let lv = |(&price, &size): (&i64, &u64)| Level { price, size };
        OrderBook {
            asks: self.asks.iter().map(lv).collect(),
            bids: self.bids.iter().rev().map(lv).collect(),
            spec: None,
        }
    }
}

#[derive(Debug, Default)]
struct View {
    books: HashMap<Symbol, Entry>,
    /// Socket connected (a `Connected` event and no `Disconnected` since).
    connected: bool,
    /// Connection counter: bumped on every `Connected`.
    epoch: u64,
    /// Symbols on the current subscription.
    subscribed: BTreeSet<Symbol>,
    /// Symbols the server refused (error frame): REST only until the next resubscription.
    refused: BTreeSet<Symbol>,
}

/// What a frame changed, ready to publish.
#[derive(Debug, Clone)]
pub struct Published {
    /// For [`AppState::books`](crate::state::Shared::books).
    pub live: LiveBook,
    /// For the quoter's cache (the cash-out valuer reads it).
    pub cached: CachedBook,
}

/// Envelope fields of a frame, and when the desk received it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameMeta {
    /// Envelope `seq`.
    pub seq: Option<u64>,
    /// Envelope `state_time_ms`.
    pub state_time_ms: Option<u64>,
    /// Envelope `server_time_ms`.
    pub server_time_ms: Option<u64>,
    /// Desk clock, Unix ms.
    pub recv_ms: u64,
}

/// A live book as the hedge desk reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewBook {
    /// The book.
    pub book: OrderBook,
    /// Desk clock of the last frame, Unix ms.
    pub recv_ms: u64,
}

/// Socket and subscription status (`/debug/hedge`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ViewStatus {
    /// Socket connected.
    pub connected: bool,
    /// Connections so far.
    pub epoch: u64,
    /// Subscribed symbols.
    pub subscribed: usize,
    /// Of those, books live now.
    pub live: usize,
    /// Symbols refused by the server.
    pub refused: Vec<Symbol>,
}

/// The live `book` view. See the module docs.
#[derive(Debug, Default)]
pub struct LiveView(Locked<View>);

impl LiveView {
    /// A `Connected` event: a new connection whose snapshots follow.
    pub fn on_connected(&self) {
        let mut v = self.0.write();
        v.connected = true;
        v.epoch += 1;
    }

    /// The socket dropped (or lagged): every book is stale until its next snapshot.
    pub fn on_disconnected(&self) {
        self.0.write().connected = false;
    }

    /// A new subscription set (a new socket): books of symbols no longer wanted are dropped, and
    /// the socket counts as down until it reports `Connected`.
    pub fn resubscribed(&self, symbols: &BTreeSet<Symbol>) {
        let mut v = self.0.write();
        v.connected = false;
        v.books.retain(|s, _| symbols.contains(s));
        v.subscribed = symbols.clone();
        v.refused.clear();
    }

    /// The server refused `symbol` (error frame).
    pub fn refuse(&self, symbol: &str) {
        let mut v = self.0.write();
        v.refused.insert(symbol.to_owned());
        v.books.remove(symbol);
    }

    /// Applies one `book` frame. Returns what to publish, or `None` when the frame was ignored
    /// (an update with no snapshot on this connection, an unsubscribed symbol, or the socket
    /// marked down).
    pub fn apply(
        &self,
        symbol: &str,
        kind: FrameKind,
        book: &OrderBook,
        m: FrameMeta,
    ) -> Option<Published> {
        let FrameMeta {
            seq,
            state_time_ms,
            server_time_ms,
            recv_ms,
        } = m;
        let mut v = self.0.write();
        if !v.connected || !v.subscribed.contains(symbol) {
            return None;
        }
        let epoch = v.epoch;
        let e = match kind {
            FrameKind::Snapshot => {
                let e = v.books.entry(symbol.to_owned()).or_default();
                e.asks.clear();
                e.bids.clear();
                e.epoch = epoch;
                e
            }
            FrameKind::Update => match v.books.get_mut(symbol) {
                Some(e) if e.epoch == epoch => e,
                _ => return None,
            },
        };
        for (side, levels) in [(&mut e.asks, &book.asks), (&mut e.bids, &book.bids)] {
            for l in levels {
                if l.size == 0 {
                    side.remove(&l.price);
                } else if l.price > 0 {
                    side.insert(l.price, l.size);
                }
            }
        }
        e.seq = seq;
        e.state_time_ms = state_time_ms;
        e.venue_ms = server_time_ms.or(state_time_ms).unwrap_or(recv_ms);
        e.recv_ms = recv_ms;
        let b = e.book();
        Some(Published {
            live: LiveBook {
                bids: b.bids.clone(),
                asks: b.asks.clone(),
                seq,
                as_of_ms: state_time_ms.unwrap_or(recv_ms),
            },
            cached: CachedBook {
                book: b,
                seq,
                state_time_ms: state_time_ms.unwrap_or(e.venue_ms),
                venue_ms: e.venue_ms,
                fetched_ms: recv_ms,
            },
        })
    }

    /// The live book of `symbol`: connected and snapshotted on this connection, else `None`.
    pub fn live(&self, symbol: &str) -> Option<ViewBook> {
        let v = self.0.read();
        if !v.connected {
            return None;
        }
        v.books
            .get(symbol)
            .filter(|e| e.epoch == v.epoch)
            .map(|e| ViewBook {
                book: e.book(),
                recv_ms: e.recv_ms,
            })
    }

    /// Symbols on the current subscription.
    pub fn subscribed(&self) -> BTreeSet<Symbol> {
        self.0.read().subscribed.clone()
    }

    /// Socket and subscription status.
    pub fn status(&self) -> ViewStatus {
        let v = self.0.read();
        ViewStatus {
            connected: v.connected,
            epoch: v.epoch,
            subscribed: v.subscribed.len(),
            live: if v.connected {
                v.books.values().filter(|e| e.epoch == v.epoch).count()
            } else {
                0
            },
            refused: v.refused.iter().cloned().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn at(recv_ms: u64) -> FrameMeta {
        FrameMeta {
            recv_ms,
            ..FrameMeta::default()
        }
    }

    fn levels(v: &[Level]) -> Vec<(i64, u64)> {
        v.iter().map(|l| (l.price, l.size)).collect()
    }

    #[test]
    fn snapshot_then_diffs_and_staleness() {
        let v = LiveView::default();
        let syms: BTreeSet<Symbol> = ["A".to_owned()].into();
        v.resubscribed(&syms);
        let snap = book(
            &[(710_000, 5), (700_000, 10)],
            &[(690_000, 3), (680_000, 7)],
        );
        // Not connected yet: ignored.
        assert!(v.apply("A", FrameKind::Snapshot, &snap, at(1)).is_none());
        v.on_connected();
        // An update before the snapshot is a diff against nothing: ignored.
        assert!(v.apply("A", FrameKind::Update, &snap, at(1)).is_none());
        let p = v
            .apply(
                "A",
                FrameKind::Snapshot,
                &snap,
                FrameMeta {
                    seq: Some(9),
                    state_time_ms: Some(100),
                    server_time_ms: Some(101),
                    recv_ms: 2,
                },
            )
            .unwrap();
        assert_eq!(levels(&p.live.asks), [(700_000, 10), (710_000, 5)]);
        assert_eq!(levels(&p.live.bids), [(690_000, 3), (680_000, 7)]);
        assert_eq!((p.cached.venue_ms, p.cached.fetched_ms), (101, 2));
        // Diff: 700k removed, 720k added, bid 690k resized.
        let d = book(&[(700_000, 0), (720_000, 4)], &[(690_000, 1)]);
        v.apply("A", FrameKind::Update, &d, at(3)).unwrap();
        let live = v.live("A").unwrap();
        assert_eq!(levels(&live.book.asks), [(710_000, 5), (720_000, 4)]);
        assert_eq!(levels(&live.book.bids), [(690_000, 1), (680_000, 7)]);
        assert_eq!(live.recv_ms, 3);
        assert_eq!(v.status().live, 1);
        // Drop: stale. Reconnect: still stale until the new snapshot; updates ignored.
        v.on_disconnected();
        assert!(v.live("A").is_none());
        v.on_connected();
        assert!(v.live("A").is_none());
        assert!(v.apply("A", FrameKind::Update, &d, at(4)).is_none());
        v.apply("A", FrameKind::Snapshot, &snap, at(5)).unwrap();
        assert_eq!(
            levels(&v.live("A").unwrap().book.asks),
            [(700_000, 10), (710_000, 5)]
        );
        // Unsubscribed symbols are ignored and dropped.
        assert!(v.apply("B", FrameKind::Snapshot, &snap, at(6)).is_none());
        v.resubscribed(&BTreeSet::new());
        assert!(v.live("A").is_none());
        assert_eq!(v.status().subscribed, 0);
    }
}
