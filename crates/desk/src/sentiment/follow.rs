//! Following `GET /api/v1/trades` for the card's symbols: newest first, a few pages per pass,
//! incremental by `seq` (D-250).
//!
//! - Symbols go in chunks of at most [`pascal_rs::rest::MAX_TRADES_SYMBOLS`] (120), sorted so a
//!   chunk keeps its key while the card does not change.
//! - Each chunk remembers, per symbol, the highest trade `seq` it has read down from (`synced`).
//!   A pass pages from the newest trade until a page holds a trade at or below the lowest
//!   `synced` of the chunk (a symbol never read counts as 0: read its whole history), or the
//!   tape ends. The newest page is always read again, so a late trade just under the mark is
//!   still picked up, and [`TradeTape::ingest`] ignores what it has already seen.
//! - `seq` is the matching engine's sequence over every market, so the trade seqs of one
//!   market have gaps by design; nothing is inferred from a gap. Positions do not depend on
//!   reading every trade either: the latest trade's `prev` gives the absolute position.
//! - A pass reads at most `budget` pages. If the mark is not reached, the cursor is kept and the
//!   next pass continues there (a backfill), the chunk's markets stay `synced = false`, and the
//!   mark moves up only when the backfill ends. A failed page keeps the cursor too.
//!
//! The tape lock is taken only to ingest a page that has already arrived (D-043: never held
//! across an `.await`).

use std::collections::HashMap;

use async_trait::async_trait;
use pascal_rs::{Page, PageParams, PublicTrade, Response};

use super::tape::TradeTape;
use crate::state::{Locked, Symbol};

/// One page of `GET /api/v1/trades` (a trait so tests can page a fixture without HTTP).
#[async_trait]
pub trait TradeSource: Send + Sync {
    /// The page before `before_cursor` (the newest page without one), newest first.
    async fn page(
        &self,
        symbols: &[Symbol],
        limit: u32,
        before_cursor: Option<&str>,
    ) -> pascal_rs::Result<Response<Page<PublicTrade>>>;
}

#[async_trait]
impl TradeSource for pascal_rs::Client {
    async fn page(
        &self,
        symbols: &[Symbol],
        limit: u32,
        before_cursor: Option<&str>,
    ) -> pascal_rs::Result<Response<Page<PublicTrade>>> {
        let page = PageParams {
            limit: Some(limit),
            before_cursor: before_cursor.map(str::to_owned),
            at_or_before_seq: None,
        };
        self.trades(symbols, &page).await
    }
}

/// A backfill in progress for one chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Backfill {
    /// Next page's cursor.
    cursor: String,
    /// Stop once a trade at or below this seq is read.
    stop: u64,
    /// The chunk's mark once the backfill ends (highest seq of its first page).
    mark: u64,
}

/// What a pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PassStats {
    /// Pages fetched.
    pub pages: u32,
    /// New trades ingested.
    pub new_trades: u64,
    /// Chunks still backfilling.
    pub backfilling: usize,
}

/// The follower: per-symbol marks and per-chunk backfills. Owned by the sentiment task.
#[derive(Debug, Default)]
pub struct Follower {
    /// Highest `seq` each symbol has been read down from.
    synced: HashMap<Symbol, u64>,
    /// Backfills by chunk (the chunk's sorted symbols).
    backfills: HashMap<Vec<Symbol>, Backfill>,
    /// Page size.
    pub limit: u32,
    /// Most pages per chunk per pass.
    pub budget: u32,
}

/// Page size of a pass (Pascal allows 1..=500).
pub const PAGE_LIMIT: u32 = 500;
/// Most pages per chunk per pass.
pub const PAGE_BUDGET: u32 = 4;

impl Follower {
    /// A follower with the default page size and budget.
    pub fn new() -> Self {
        Follower {
            limit: PAGE_LIMIT,
            budget: PAGE_BUDGET,
            ..Follower::default()
        }
    }

    /// One pass over `symbols`: every chunk in turn. Stops at the first failed page (its chunk
    /// keeps its cursor) and returns the error after ingesting what arrived.
    pub async fn pass(
        &mut self,
        src: &dyn TradeSource,
        symbols: &[Symbol],
        tape: &Locked<TradeTape>,
    ) -> Result<PassStats, pascal_rs::Error> {
        let mut sorted = symbols.to_vec();
        sorted.sort();
        sorted.dedup();
        let chunks: Vec<Vec<Symbol>> = sorted
            .chunks(pascal_rs::rest::MAX_TRADES_SYMBOLS)
            .map(<[Symbol]>::to_vec)
            .collect();
        // A chunk that changed starts over: its old backfill is dropped (marks stay).
        self.backfills.retain(|k, _| chunks.contains(k));
        let mut stats = PassStats::default();
        for chunk in chunks {
            self.chunk(src, chunk, tape, &mut stats).await?;
        }
        stats.backfilling = self.backfills.len();
        Ok(stats)
    }

    async fn chunk(
        &mut self,
        src: &dyn TradeSource,
        chunk: Vec<Symbol>,
        tape: &Locked<TradeTape>,
        stats: &mut PassStats,
    ) -> Result<(), pascal_rs::Error> {
        let (mut cursor, stop, mut mark) = match self.backfills.remove(&chunk) {
            Some(b) => (Some(b.cursor), b.stop, Some(b.mark)),
            None => {
                let stop = chunk
                    .iter()
                    .map(|s| self.synced.get(s).copied().unwrap_or(0))
                    .min()
                    .unwrap_or(0);
                (None, stop, None)
            }
        };
        let limit = self.limit.clamp(1, 500);
        let mut pages = 0;
        loop {
            if pages >= self.budget.max(1) {
                // Out of budget: continue here next pass.
                if let Some(c) = cursor {
                    self.backfills.insert(
                        chunk.clone(),
                        Backfill {
                            cursor: c,
                            stop,
                            mark: mark.unwrap_or(stop),
                        },
                    );
                }
                tape.write().set_synced(&chunk, false);
                return Ok(());
            }
            let resp = match src.page(&chunk, limit, cursor.as_deref()).await {
                Ok(r) => r,
                Err(e) => {
                    if let Some(c) = cursor {
                        self.backfills.insert(
                            chunk.clone(),
                            Backfill {
                                cursor: c,
                                stop,
                                mark: mark.unwrap_or(stop),
                            },
                        );
                        tape.write().set_synced(&chunk, false);
                    }
                    return Err(e);
                }
            };
            pages += 1;
            stats.pages += 1;
            let items = &resp.data.items;
            let top = items.iter().map(|t| t.seq).max();
            // The first page of a fresh pass sets the mark the chunk moves to when it ends.
            if mark.is_none() {
                mark = Some(top.unwrap_or(stop).max(stop));
            }
            {
                let mut t = tape.write();
                for trade in items {
                    // Only the chunk's own symbols (a server answering more is not trusted).
                    if chunk.contains(&trade.symbol) && t.ingest(trade) {
                        stats.new_trades += 1;
                    }
                }
            }
            let reached = stop > 0 && items.iter().any(|t| t.seq <= stop);
            let next = resp.data.next_cursor.clone();
            let end = items.is_empty() || next.is_none() || next == cursor;
            if reached || end {
                let m = mark.unwrap_or(stop);
                for s in &chunk {
                    let e = self.synced.entry(s.clone()).or_insert(0);
                    *e = (*e).max(m);
                }
                tape.write().set_synced(&chunk, true);
                return Ok(());
            }
            cursor = next;
        }
    }

    /// Forgets the marks of symbols not in `keep` (with [`TradeTape::retain`]: a symbol that
    /// comes back is read from its first trade again).
    pub fn retain(&mut self, keep: &[Symbol]) {
        self.synced.retain(|s, _| keep.contains(s));
    }

    /// The mark of `symbol` (tests and logs).
    pub fn synced(&self, symbol: &str) -> Option<u64> {
        self.synced.get(symbol).copied()
    }

    /// Whether a chunk is backfilling.
    pub fn backfilling(&self) -> bool {
        !self.backfills.is_empty()
    }
}
