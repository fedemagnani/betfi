//! Tape loader: `tapes/<night>/` as written by `scripts/record_tape.py` (see `tapes/README.md`).
//!
//! A tape directory holds `books.jsonl`, `trades.jsonl`, `search.jsonl` and `markets.jsonl`, one
//! HTTP response per line (`{"recv_ms", "method", "path", "http", "body"}`), plus
//! `markets_selected.json` one level up. [`Tape::load`] accepts the night directory (it then
//! uses `raw/` when present, else `sample/`) or the `raw/` / `sample/` directory itself.
//!
//! Every snapshot is stamped with the server's `body.state_time_ms` (falling back to `recv_ms`):
//! that is the tape time the mock clock runs on. Lines that are not JSON (a truncated last line
//! while the recorder is still appending), non-200 lines and error envelopes are skipped and
//! counted in [`LoadStats`], never fatal.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use pascal_rs::{Books, Market, OrderBook, PublicTrade};
use serde::Deserialize;
use serde_json::Value;

/// Why a tape could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum TapeError {
    /// No `books.jsonl` under the directory, `raw/` or `sample/`.
    #[error("no books.jsonl in {0} (nor in its raw/ or sample/ subdirectory)")]
    NoBooks(PathBuf),
    /// No market catalogue (`markets_selected.json`, `markets.jsonl` or `search.jsonl`).
    #[error("no market catalogue for tape {0}")]
    NoMarkets(PathBuf),
    /// File read failure.
    #[error("reading {path}: {source}")]
    Io {
        /// File.
        path: PathBuf,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
}

/// One `/books` response.
#[derive(Debug, Clone)]
pub struct BookSnapshot {
    /// Tape time, Unix ms.
    pub t_ms: u64,
    /// Envelope `seq`.
    pub seq: Option<u64>,
    /// Books by symbol, as served.
    pub books: BTreeMap<String, OrderBook>,
}

/// One market observation (from `markets_selected.json`, `markets.jsonl` or `search.jsonl`).
#[derive(Debug, Clone)]
pub struct MarketObs {
    /// Tape time of the response, Unix ms.
    pub t_ms: u64,
    /// The market object as served (verbatim JSON).
    pub raw: Value,
    /// The same, typed.
    pub market: Market,
}

/// A public trade first seen on the tape at `visible_ms`.
#[derive(Debug, Clone)]
pub struct TapeTrade {
    /// Tape time of the first `/trades` response that listed it.
    pub visible_ms: u64,
    /// Typed trade.
    pub trade: PublicTrade,
    /// As served.
    pub raw: Value,
}

/// Counters of what the loader kept and skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadStats {
    /// Lines that were not JSON (typically a truncated last line).
    pub bad_lines: usize,
    /// Non-200 lines and error envelopes.
    pub error_lines: usize,
    /// Market objects that `pascal-rs` could not decode.
    pub bad_markets: usize,
}

/// A loaded tape.
#[derive(Debug, Clone)]
pub struct Tape {
    /// Directory the `.jsonl` files were read from.
    pub dir: PathBuf,
    /// `/books` snapshots, by time.
    pub books: Vec<BookSnapshot>,
    /// Public trades, deduplicated by `trade_id`, by `seq` ascending.
    pub trades: Vec<TapeTrade>,
    /// Market observations per symbol, by time.
    pub markets: BTreeMap<String, Vec<MarketObs>>,
    /// Loader counters.
    pub stats: LoadStats,
}

#[derive(Deserialize)]
struct Line {
    recv_ms: u64,
    #[serde(default)]
    http: Option<u16>,
    body: Value,
}

impl Tape {
    /// Loads a tape directory (see the module docs for the accepted layouts).
    pub fn load(dir: impl AsRef<Path>) -> Result<Tape, TapeError> {
        let given = dir.as_ref().to_path_buf();
        let files = [given.clone(), given.join("raw"), given.join("sample")]
            .into_iter()
            .find(|d| d.join("books.jsonl").is_file())
            .ok_or_else(|| TapeError::NoBooks(given.clone()))?;
        let mut stats = LoadStats::default();

        let mut books = Vec::new();
        for (t_ms, body) in read_lines(&files.join("books.jsonl"), &mut stats)? {
            match serde_json::from_value::<Books>(body["data"].clone()) {
                Ok(b) => books.push(BookSnapshot {
                    t_ms,
                    seq: str_u64(&body["seq"]),
                    books: b.books,
                }),
                Err(e) => {
                    tracing::warn!(t_ms, error = %e, "skipping undecodable books line");
                    stats.error_lines += 1;
                }
            }
        }
        books.sort_by_key(|b| b.t_ms);

        let mut trades = Vec::new();
        let mut seen = HashSet::new();
        let trades_path = files.join("trades.jsonl");
        if trades_path.is_file() {
            let mut lines = read_lines(&trades_path, &mut stats)?;
            lines.sort_by_key(|(t, _)| *t);
            for (t_ms, body) in lines {
                let items = body["data"]["items"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                for raw in items {
                    match serde_json::from_value::<PublicTrade>(raw.clone()) {
                        Ok(trade) if seen.insert(trade.trade_id) => trades.push(TapeTrade {
                            visible_ms: t_ms,
                            trade,
                            raw,
                        }),
                        Ok(_) => {}
                        Err(e) => tracing::warn!(error = %e, "skipping undecodable tape trade"),
                    }
                }
            }
        }
        trades.sort_by_key(|t| t.trade.seq);

        let mut markets: BTreeMap<String, Vec<MarketObs>> = BTreeMap::new();
        let mut add_markets = |t_ms: u64, data: &Value, stats: &mut LoadStats| {
            for raw in data.as_array().map(Vec::as_slice).unwrap_or(&[]) {
                match serde_json::from_value::<Market>(raw.clone()) {
                    Ok(market) => {
                        markets
                            .entry(market.symbol.clone())
                            .or_default()
                            .push(MarketObs {
                                t_ms,
                                raw: raw.clone(),
                                market,
                            })
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "skipping undecodable tape market");
                        stats.bad_markets += 1;
                    }
                }
            }
        };
        let selected = [
            given.join("markets_selected.json"),
            files.join("markets_selected.json"),
        ]
        .into_iter()
        .chain(files.parent().map(|p| p.join("markets_selected.json")))
        .find(|p| p.is_file());
        if let Some(path) = selected {
            let text = fs::read_to_string(&path).map_err(|source| TapeError::Io {
                path: path.clone(),
                source,
            })?;
            match serde_json::from_str::<Value>(&text) {
                Ok(body) => {
                    let t_ms = str_u64(&body["state_time_ms"]).unwrap_or(0);
                    add_markets(t_ms, &body["data"], &mut stats);
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "markets_selected.json is not JSON");
                    stats.bad_lines += 1;
                }
            }
        }
        for name in ["markets.jsonl", "search.jsonl"] {
            let path = files.join(name);
            if path.is_file() {
                for (t_ms, body) in read_lines(&path, &mut stats)? {
                    add_markets(t_ms, &body["data"], &mut stats);
                }
            }
        }
        if markets.is_empty() {
            return Err(TapeError::NoMarkets(given));
        }
        for obs in markets.values_mut() {
            obs.sort_by_key(|o| o.t_ms);
        }

        Ok(Tape {
            dir: files,
            books,
            trades,
            markets,
            stats,
        })
    }

    /// A tape with nothing on it (live mode keeps its market data in the engine's feed).
    pub fn empty(dir: impl Into<PathBuf>) -> Tape {
        Tape {
            dir: dir.into(),
            books: Vec::new(),
            trades: Vec::new(),
            markets: BTreeMap::new(),
            stats: LoadStats::default(),
        }
    }

    /// Time of the first books snapshot (else the first market observation).
    pub fn first_ms(&self) -> u64 {
        self.books.first().map(|b| b.t_ms).unwrap_or_else(|| {
            self.markets
                .values()
                .filter_map(|o| o.first().map(|o| o.t_ms))
                .min()
                .unwrap_or(0)
        })
    }

    /// Time of the last books snapshot.
    pub fn last_ms(&self) -> u64 {
        self.books
            .last()
            .map_or_else(|| self.first_ms(), |b| b.t_ms)
    }

    /// Index of the books snapshot at or before `t_ms`.
    pub fn book_index_at(&self, t_ms: u64) -> Option<usize> {
        self.books
            .partition_point(|b| b.t_ms <= t_ms)
            .checked_sub(1)
    }

    /// The market as observed at or before `t_ms`. Before its first observation the first one
    /// is returned with any `resolution` removed (a market is known from the tape's start).
    pub fn market_at(&self, symbol: &str, t_ms: u64) -> Option<(Market, Value)> {
        let obs = self.markets.get(symbol)?;
        match obs.partition_point(|o| o.t_ms <= t_ms).checked_sub(1) {
            Some(i) => Some((obs[i].market.clone(), obs[i].raw.clone())),
            None => {
                let first = obs.first()?;
                let mut market = first.market.clone();
                let mut raw = first.raw.clone();
                if market.resolution.take().is_some()
                    && let Some(o) = raw.as_object_mut()
                {
                    o.remove("resolution");
                }
                Some((market, raw))
            }
        }
    }

    /// Every symbol in the catalogue.
    pub fn symbols(&self) -> impl Iterator<Item = &str> {
        self.markets.keys().map(String::as_str)
    }

    /// The highest envelope `seq` seen on the tape at or before `t_ms` (0 if none).
    pub fn seq_at(&self, t_ms: u64) -> u64 {
        let books = self
            .book_index_at(t_ms)
            .and_then(|i| self.books[..=i].iter().rev().find_map(|b| b.seq))
            .unwrap_or(0);
        let trades = self
            .trades
            .iter()
            .filter(|t| t.visible_ms <= t_ms)
            .map(|t| t.trade.seq)
            .max()
            .unwrap_or(0);
        books.max(trades)
    }
}

/// Reads a `.jsonl` tape file into `(tape_ms, body)` pairs of successful responses.
fn read_lines(path: &Path, stats: &mut LoadStats) -> Result<Vec<(u64, Value)>, TapeError> {
    let text = fs::read_to_string(path).map_err(|source| TapeError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let line: Line = match serde_json::from_str(line) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(path = %path.display(), line = n + 1, error = %e, "skipping unparsable tape line");
                stats.bad_lines += 1;
                continue;
            }
        };
        if line.http.is_some_and(|h| h != 200) || line.body["status"] != "success" {
            stats.error_lines += 1;
            continue;
        }
        let t_ms = str_u64(&line.body["state_time_ms"]).unwrap_or(line.recv_ms);
        out.push((t_ms, line.body));
    }
    Ok(out)
}

/// A JSON string (or number) holding a `u64`.
pub(crate) fn str_u64(v: &Value) -> Option<u64> {
    match v {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_u64(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tapes/nba-2026-10-28/sample")
    }

    #[test]
    fn loads_the_sample_tape() {
        let tape = Tape::load(sample_dir()).unwrap();
        assert_eq!(tape.books.len(), 2);
        assert_eq!(tape.markets.len(), 5);
        assert!(tape.trades.len() >= 3);
        assert_eq!(tape.stats, LoadStats::default());
        assert_eq!(tape.first_ms(), 1_793_212_987_458);
        assert_eq!(tape.book_index_at(tape.first_ms() - 1), None);
        assert_eq!(tape.book_index_at(1_793_213_017_475), Some(0));
        assert_eq!(tape.book_index_at(1_793_213_017_476), Some(1));
        assert_eq!(tape.book_index_at(u64::MAX), Some(1));
        let (m, raw) = tape.market_at("NBA_26OCT28_PHIIND_ML.PHI", 0).unwrap();
        assert_eq!(m.tick_sig_figs, 3);
        assert_eq!(raw["symbol"], "NBA_26OCT28_PHIIND_ML.PHI");
        assert!(tape.seq_at(u64::MAX) >= 12_216_637_677);
    }

    #[test]
    fn night_directory_and_truncated_lines() {
        let dir = std::env::temp_dir().join(format!("pascal-mock-tape-{}", std::process::id()));
        let sample = dir.join("sample");
        fs::create_dir_all(&sample).unwrap();
        for f in [
            "books.jsonl",
            "markets.jsonl",
            "search.jsonl",
            "trades.jsonl",
        ] {
            fs::copy(sample_dir().join(f), sample.join(f)).unwrap();
        }
        // A recorder killed mid-write leaves half a line.
        let mut books = fs::read_to_string(sample.join("books.jsonl")).unwrap();
        books.push_str(r#"{"recv_ms":1793213047635,"method":"GET","path":"/api/v1/bo"#);
        fs::write(sample.join("books.jsonl"), books).unwrap();
        let tape = Tape::load(&dir).unwrap();
        assert_eq!(tape.dir, sample);
        assert_eq!(tape.books.len(), 2);
        assert_eq!(tape.stats.bad_lines, 1);
        fs::remove_dir_all(&dir).ok();
        assert!(matches!(Tape::load(&dir), Err(TapeError::NoBooks(_))));
    }
}
