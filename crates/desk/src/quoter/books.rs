//! The quoter's book cache: REST `/books` (≤ 50 symbols per call), the freshness rule of spec
//! section 12's runbook, and the `book_snapshots` writer.
//!
//! - A quote prices from books at most [`FRESH_MS`] old, fetching them when the cache is older.
//! - When the fetch fails (Pascal `429`, an error, or no answer within [`FETCH_TIMEOUT`]), it
//!   prices from the cached books if every one is at most [`FALLBACK_MS`] old, else it answers
//!   `503 unavailable`. After a failure the desk leaves Pascal alone for [`BACKOFF_MS`] (the
//!   client already backs off inside one call; this stops every quote from waiting for it).
//! - Ages are measured on the desk's clock from the moment a book was received. The venue's own
//!   clock (`server_time_ms`, the tape time in mode M, D-033) is kept beside it and is the `now`
//!   the pricing rules (legs must not have started) are checked against.
//!
//! Every fetch also publishes the books into [`AppState::books`](crate::state::Shared::books)
//! for the other services.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;

use pascal_rs::OrderBook;

use super::QuoterState;
use crate::now_ms;
use crate::state::{AppState, LiveBook, Symbol};

/// A quote prices from books at most this old (desk clock), else it fetches.
pub const FRESH_MS: u64 = 1_000;
/// After a failed fetch, cached books at most this old still price a quote (spec 12 runbook).
pub const FALLBACK_MS: u64 = 10_000;
/// Longest wait for one `/books` call inside a request.
pub const FETCH_TIMEOUT: Duration = Duration::from_millis(1_500);
/// No `/books` call for this long after a failed one.
pub const BACKOFF_MS: u64 = 2_000;
/// Most symbols per `/books` call (Pascal's limit).
pub const MAX_SYMBOLS_PER_CALL: usize = 50;

/// One cached book.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedBook {
    /// The book (asks lowest first, bids highest first, as served).
    pub book: OrderBook,
    /// Envelope `seq`.
    pub seq: Option<u64>,
    /// Envelope `state_time_ms` (the venue's last matching round), else `venue_ms`.
    pub state_time_ms: u64,
    /// The venue's clock when it answered (`server_time_ms`, else `state_time_ms`, else the
    /// desk's receive time).
    pub venue_ms: u64,
    /// Desk clock when the response arrived, Unix ms.
    pub fetched_ms: u64,
}

impl CachedBook {
    /// The venue's clock now, extrapolated from this book by the desk's elapsed time.
    pub fn venue_now(&self, now_ms: u64) -> u64 {
        self.venue_ms
            .saturating_add(now_ms.saturating_sub(self.fetched_ms))
    }
}

/// Books for one request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegBooks {
    /// Book per symbol (only the symbols that met the age limit).
    pub books: BTreeMap<String, OrderBook>,
    /// The venue's clock now, estimated from the freshest book (`0` with no books).
    pub venue_now_ms: u64,
    /// Age of the oldest book used, ms.
    pub max_age_ms: u64,
}

impl QuoterState {
    /// The cached books of `symbols` at most `max_age_ms` old at `now_ms` (missing or older ones
    /// are left out).
    pub fn collect(&self, symbols: &BTreeSet<String>, now_ms: u64, max_age_ms: u64) -> LegBooks {
        let cache = self.books.read();
        let mut out = LegBooks::default();
        for s in symbols {
            let Some(c) = cache.get(s) else { continue };
            let age = now_ms.saturating_sub(c.fetched_ms);
            if age > max_age_ms {
                continue;
            }
            out.books.insert(s.clone(), c.book.clone());
            out.venue_now_ms = out.venue_now_ms.max(c.venue_now(now_ms));
            out.max_age_ms = out.max_age_ms.max(age);
        }
        out
    }

    /// A copy of one cached book.
    pub fn cached(&self, symbol: &str) -> Option<CachedBook> {
        self.books.read().get(symbol).cloned()
    }

    /// Whether `/books` calls are paused after a failure.
    pub fn backing_off(&self, now_ms: u64) -> bool {
        now_ms
            < self
                .backoff_until_ms
                .load(std::sync::atomic::Ordering::Relaxed)
    }

    fn back_off(&self, now_ms: u64) {
        self.backoff_until_ms.store(
            now_ms.saturating_add(BACKOFF_MS),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// Stores fetched books, dropping requested symbols the venue did not return (no book is
    /// better than a stale one for a market that may have closed).
    pub fn store(&self, requested: &[String], fetched: HashMap<Symbol, CachedBook>) {
        let mut cache = self.books.write();
        for s in requested {
            if !fetched.contains_key(s) {
                cache.remove(s);
            }
        }
        cache.extend(fetched);
    }
}

/// Why books could not be had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BooksUnavailable(pub String);

/// The books of `symbols` for a quote or a cash-out quote, per the freshness rule above.
///
/// On success every symbol the venue returned is in the result; a symbol it did not return is
/// absent (pricing then answers "no book for …").
pub async fn books_for(
    state: &AppState,
    symbols: &BTreeSet<String>,
) -> Result<LegBooks, BooksUnavailable> {
    let q = &state.quoter;
    let fresh = q.collect(symbols, now_ms(), FRESH_MS);
    if fresh.books.len() == symbols.len() {
        return Ok(fresh);
    }
    let why = if q.backing_off(now_ms()) {
        "Pascal is backing off after an error".to_owned()
    } else {
        let list: Vec<String> = symbols.iter().cloned().collect();
        match fetch(state, &list, FETCH_TIMEOUT).await {
            Ok(()) => return Ok(q.collect(symbols, now_ms(), FRESH_MS)),
            Err(e) => e,
        }
    };
    let stale = q.collect(symbols, now_ms(), FALLBACK_MS);
    if stale.books.len() == symbols.len() {
        metrics::counter!("desk_quoter_books_fallback_total").increment(1);
        tracing::info!(age_ms = stale.max_age_ms, %why, "pricing from the last book snapshot");
        return Ok(stale);
    }
    Err(BooksUnavailable(format!(
        "live prices are unavailable right now ({why}); try again in a few seconds"
    )))
}

/// Fetches `symbols` (in calls of at most [`MAX_SYMBOLS_PER_CALL`], each bounded by
/// `timeout`), caches the books and publishes them to `AppState::books`. A failure starts the
/// back-off and returns its reason; books of the calls that succeeded are kept.
pub async fn fetch(state: &AppState, symbols: &[String], timeout: Duration) -> Result<(), String> {
    for chunk in symbols.chunks(MAX_SYMBOLS_PER_CALL) {
        let started = std::time::Instant::now();
        let res = tokio::time::timeout(timeout, state.pascal.books(chunk)).await;
        metrics::histogram!("desk_quoter_books_seconds").record(started.elapsed().as_secs_f64());
        let resp = match res {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                let why = if e.http_status() == Some(429) || e.api_code() == Some("RATE_LIMITED") {
                    "Pascal rate limit (429)".to_owned()
                } else {
                    format!("Pascal error: {e}")
                };
                return Err(failed(state, why));
            }
            Err(_) => return Err(failed(state, "Pascal did not answer in time".to_owned())),
        };
        metrics::counter!("desk_quoter_books_fetch_total", "result" => "ok").increment(1);
        let fetched_ms = now_ms();
        let state_time_ms = resp.state_time_ms;
        let venue_ms = resp.server_time_ms.or(state_time_ms).unwrap_or(fetched_ms);
        let mut cached = HashMap::new();
        let mut live = Vec::new();
        for (symbol, book) in resp.data.books {
            if !chunk.contains(&symbol) {
                continue;
            }
            live.push((
                symbol.clone(),
                LiveBook {
                    bids: book.bids.clone(),
                    asks: book.asks.clone(),
                    seq: resp.seq,
                    as_of_ms: state_time_ms.unwrap_or(fetched_ms),
                },
            ));
            cached.insert(
                symbol,
                CachedBook {
                    book,
                    seq: resp.seq,
                    state_time_ms: state_time_ms.unwrap_or(venue_ms),
                    venue_ms,
                    fetched_ms,
                },
            );
        }
        state.quoter.store(chunk, cached);
        let mut books = state.books.write();
        for s in chunk {
            books.remove(s);
        }
        books.extend(live);
    }
    Ok(())
}

fn failed(state: &AppState, why: String) -> String {
    metrics::counter!("desk_quoter_books_fetch_total", "result" => "error").increment(1);
    tracing::warn!(%why, "books fetch failed; backing off");
    state.quoter.back_off(now_ms());
    why
}

// --- book_snapshots ------------------------------------------------------------------------------

/// Symbols with open interest: legs still open on pending or open tickets, plus the symbols
/// quoted in the last [`super::RECENT_QUOTE_MS`].
pub async fn open_interest_symbols(state: &AppState) -> Result<BTreeSet<String>, sqlx::Error> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT l.symbol FROM ticket_legs l JOIN tickets t ON t.ticket = l.ticket
         WHERE t.state IN ('pending', 'open') AND l.status = 'open'",
    )
    .fetch_all(&state.db)
    .await?;
    let mut out: BTreeSet<String> = rows.into_iter().collect();
    out.extend(state.quoter.recent_symbols(now_ms()));
    Ok(out)
}

/// One `book_snapshots` pass: fetches the books of every symbol with open interest and writes
/// one row per book. Returns the number of rows written.
pub async fn snapshot_once(state: &AppState) -> Result<usize, String> {
    let symbols = open_interest_symbols(state)
        .await
        .map_err(|e| format!("db: {e}"))?;
    if symbols.is_empty() {
        return Ok(0);
    }
    if state.quoter.backing_off(now_ms()) {
        return Err("backing off after a failed /books call".to_owned());
    }
    let list: Vec<String> = symbols.iter().cloned().collect();
    // The writer is not in a request, so it may wait longer than a quote does.
    fetch(state, &list, Duration::from_secs(5)).await?;
    // The books this pass fetched (a symbol the venue did not return was dropped from the cache).
    let rows: Vec<(&String, CachedBook)> = list
        .iter()
        .filter_map(|s| state.quoter.cached(s).map(|c| (s, c)))
        .collect();
    let window = state.quoter.params.depth_window_u;
    let mut tx = state.db.begin().await.map_err(|e| format!("db: {e}"))?;
    let mut n = 0;
    for (symbol, c) in &rows {
        let best_bid = c
            .book
            .bids
            .iter()
            .filter(|l| l.size > 0)
            .map(|l| l.price)
            .max();
        let best_ask = c
            .book
            .asks
            .iter()
            .filter(|l| l.size > 0)
            .map(|l| l.price)
            .min();
        let bid_depth =
            pricing::executable_depth(&c.book, pricing::Side::Long, window).min(i64::MAX as u64);
        let ask_depth =
            pricing::opening_depth(&c.book, pricing::Side::Long, window).min(i64::MAX as u64);
        let raw = serde_json::json!({"bids": c.book.bids, "asks": c.book.asks}).to_string();
        let r = sqlx::query(
            "INSERT INTO book_snapshots (symbol, ts_ms, best_bid_u, best_ask_u, bid_depth,
                ask_depth, depth_window_u, seq, raw, state_time_ms)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (symbol, ts_ms) DO NOTHING",
        )
        .bind(symbol.as_str())
        .bind(to_i64(c.fetched_ms))
        .bind(best_bid)
        .bind(best_ask)
        .bind(to_i64(bid_depth))
        .bind(to_i64(ask_depth))
        .bind(to_i64(window))
        .bind(c.seq.map(to_i64))
        .bind(raw)
        .bind(to_i64(c.state_time_ms))
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("db: {e}"))?;
        n += r.rows_affected() as usize;
    }
    tx.commit().await.map_err(|e| format!("db: {e}"))?;
    Ok(n)
}

/// `u64` → SQLite `INTEGER`, saturating.
pub(crate) fn to_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use pascal_rs::Level;

    use super::*;

    fn cb(fetched_ms: u64, venue_ms: u64) -> CachedBook {
        CachedBook {
            book: OrderBook {
                asks: vec![Level {
                    price: 600_000,
                    size: 10,
                }],
                bids: vec![Level {
                    price: 590_000,
                    size: 10,
                }],
                spec: None,
            },
            seq: Some(1),
            state_time_ms: venue_ms,
            venue_ms,
            fetched_ms,
        }
    }

    #[test]
    fn freshness_windows() {
        let q = QuoterState::default();
        let a = "A".to_owned();
        let b = "B".to_owned();
        let mut m = HashMap::new();
        m.insert(a.clone(), cb(100_000, 5_000_000));
        m.insert(b.clone(), cb(95_000, 4_000_000));
        q.store(&[a.clone(), b.clone()], m);
        let both: BTreeSet<String> = [a.clone(), b.clone()].into();

        // At 100.5 s: A is fresh, B is 5.5 s old.
        let fresh = q.collect(&both, 100_500, FRESH_MS);
        assert_eq!(fresh.books.len(), 1);
        let stale = q.collect(&both, 100_500, FALLBACK_MS);
        assert_eq!(stale.books.len(), 2);
        assert_eq!(stale.max_age_ms, 5_500);
        // Venue clock: the freshest extrapolation (A: 5,000,000 + 500).
        assert_eq!(stale.venue_now_ms, 5_000_500);
        // At 105.001 s B is past the 10 s fallback window.
        assert_eq!(q.collect(&both, 105_001, FALLBACK_MS).books.len(), 1);
        assert_eq!(q.collect(&both, 105_000, FALLBACK_MS).books.len(), 2);

        // A refetch that does not return B drops it.
        let mut m = HashMap::new();
        m.insert(a.clone(), cb(106_000, 5_006_000));
        q.store(&[a.clone(), b.clone()], m);
        assert!(q.cached(&b).is_none());

        assert!(!q.backing_off(0));
        q.back_off(1_000);
        assert!(q.backing_off(2_999));
        assert!(!q.backing_off(3_000));
    }
}
