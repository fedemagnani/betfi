//! `--record <dir>`: a live session written in the tape format of `tapes/README.md`, so that
//! `pascal-mock --tape <dir>` replays it later.
//!
//! ```text
//! <dir>/manifest.json           source, symbols, games (start_ms, …), resolutions, coverage
//! <dir>/markets_selected.json   the catalogue entries of every recorded symbol (a /markets body)
//! <dir>/raw/books.jsonl         GET /api/v1/books?symbols=…   every 5 s when a book changed
//! <dir>/raw/markets.jsonl       GET /api/v1/markets           each catalogue refresh, trimmed
//! <dir>/raw/search.jsonl        POST /api/v1/markets/search   backstop answers and pushed results
//! ```
//!
//! The recorded symbols are those the mock tracks (the desk asked for their books). Lines keep
//! the `{"recv_ms","method","path","http","body"}` shape and the upstream's `seq` /
//! `state_time_ms`. Books lines are the mock's copy of the upstream `book` channel (snapshot plus
//! updates), not REST responses, and carry `"source":"ws book"`: they hold the upstream levels
//! (never the mock's consumption or resting orders) of the fresh books only, so a book that was
//! stale is absent from the snapshots of that interval and a replay cannot fill on it either.
//! A resolution pushed on `market_update` becomes a `search.jsonl` line with
//! `"source":"ws market_update"`. `markets.jsonl` lines are filtered to the recorded symbols and
//! carry `"trimmed"`, as `record_tape.py` writes them.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Weak;
use std::time::Duration;

use pascal_rs::{Books, OrderBook};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::time::MissedTickBehavior;

use crate::api::Shared;
use crate::tape::str_u64;
use crate::util::strip_nulls;

pub(crate) enum RecCmd {
    Catalogue {
        seq: Option<u64>,
        state_time_ms: Option<u64>,
    },
    Search {
        markets: Vec<Value>,
        seq: Option<u64>,
        state_time_ms: Option<u64>,
    },
    Resolved {
        market: Value,
        seq: u64,
        state_time_ms: Option<u64>,
    },
    Stop(String, oneshot::Sender<()>),
}

#[derive(Debug, Clone, Copy, Default)]
struct FileStats {
    lines: u64,
    bytes: u64,
    first_ms: Option<u64>,
    last_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default)]
struct BookStats {
    first_ms: u64,
    last_ms: u64,
    snapshots: u64,
    nonempty: u64,
}

/// An open recording.
pub struct Recorder {
    dir: PathBuf,
    raw: PathBuf,
    upstream: String,
    started_ms: u64,
    /// Symbols in `markets_selected.json`, with their latest catalogue entry.
    selected: BTreeMap<String, Value>,
    last_books: Option<BTreeMap<String, OrderBook>>,
    last_seq: u64,
    files: BTreeMap<&'static str, FileStats>,
    book_stats: BTreeMap<String, BookStats>,
    resolutions: BTreeMap<String, Value>,
    warned: bool,
}

const BOOKS: &str = "books.jsonl";
const MARKETS: &str = "markets.jsonl";
const SEARCH: &str = "search.jsonl";

impl Recorder {
    /// Creates `<dir>/raw/`. Refuses a directory that already holds a recording.
    pub fn open(dir: &Path, upstream: &str, started_ms: u64) -> Result<Recorder, String> {
        let raw = dir.join("raw");
        if raw.join(BOOKS).exists() || dir.join(BOOKS).exists() {
            return Err(format!(
                "{} already holds a recording (books.jsonl); record into a new directory",
                dir.display()
            ));
        }
        fs::create_dir_all(&raw).map_err(|e| format!("creating {}: {e}", raw.display()))?;
        Ok(Recorder {
            dir: dir.to_path_buf(),
            raw,
            upstream: upstream.to_owned(),
            started_ms,
            selected: BTreeMap::new(),
            last_books: None,
            last_seq: 0,
            files: BTreeMap::new(),
            book_stats: BTreeMap::new(),
            resolutions: BTreeMap::new(),
            warned: false,
        })
    }

    fn append(&mut self, file: &'static str, line: &Value, state_ms: u64) {
        let mut text = line.to_string();
        text.push('\n');
        let res = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.raw.join(file))
            .and_then(|mut f| f.write_all(text.as_bytes()));
        match res {
            Ok(()) => {
                let s = self.files.entry(file).or_default();
                s.lines += 1;
                s.bytes += text.len() as u64;
                s.first_ms.get_or_insert(state_ms);
                s.last_ms = Some(state_ms);
            }
            Err(e) if !self.warned => {
                self.warned = true;
                tracing::warn!(file, error = %e, "record: write failed (further failures are not logged)");
            }
            Err(_) => {}
        }
    }

    fn line(method: &str, path: String, body: Value, source: Option<&str>, now: u64) -> Value {
        let mut l =
            json!({"recv_ms": now, "method": method, "path": path, "http": 200, "body": body});
        if let Some(s) = source {
            l["source"] = s.into();
        }
        l
    }

    fn body(data: Value, seq: u64, state_ms: u64) -> Value {
        json!({
            "status": "success",
            "data": data,
            "seq": seq.to_string(),
            "state_time_ms": state_ms.to_string(),
            "server_time_ms": state_ms.to_string(),
        })
    }

    /// Writes a books snapshot if the tracked books changed; picks up newly tracked symbols.
    fn tick(&mut self, shared: &Shared) {
        let (books, seq, now, tracked, markets) = {
            let e = shared.engine();
            let (books, seq) = e.live_record_books();
            let tracked = e.live_books();
            let new = tracked.iter().any(|s| !self.selected.contains_key(s));
            let markets = new.then(|| e.live_markets_raw(&tracked));
            (books, seq, e.now_ms(), tracked, markets)
        };
        self.last_seq = self.last_seq.max(seq);
        if let Some(markets) = markets {
            self.select(&tracked, markets, now);
        }
        if books.is_empty() || self.last_books.as_ref() == Some(&books) {
            return;
        }
        for (s, b) in &books {
            let st = self.book_stats.entry(s.clone()).or_insert(BookStats {
                first_ms: now,
                ..BookStats::default()
            });
            st.last_ms = now;
            st.snapshots += 1;
            st.nonempty += u64::from(!b.asks.is_empty() || !b.bids.is_empty());
        }
        let symbols: Vec<&str> = books.keys().map(String::as_str).collect();
        let path = format!("/api/v1/books?symbols={}", symbols.join(","));
        let data = serde_json::to_value(Books {
            books: books.clone(),
        })
        .map(strip_nulls)
        .unwrap_or(Value::Null);
        let line = Self::line(
            "GET",
            path,
            Self::body(data, self.last_seq, now),
            Some("ws book"),
            now,
        );
        self.append(BOOKS, &line, now);
        self.last_books = Some(books);
    }

    /// New symbols: rewrites `markets_selected.json` (stamped at the recording's start, so the
    /// catalogue covers the whole tape) and writes a `markets.jsonl` line.
    fn select(&mut self, tracked: &[String], markets: Vec<Value>, now: u64) {
        for m in markets {
            if let Some(s) = m["symbol"].as_str() {
                self.selected.insert(s.to_owned(), m.clone());
            }
        }
        for s in tracked {
            if !self.selected.contains_key(s) {
                tracing::debug!(symbol = %s, "record: tracked symbol not in the catalogue yet");
            }
        }
        let data: Vec<Value> = self.selected.values().cloned().collect();
        let body = Self::body(Value::Array(data.clone()), self.last_seq, self.started_ms);
        let path = self.dir.join("markets_selected.json");
        if let Err(e) = fs::write(&path, body.to_string()) {
            tracing::warn!(path = %path.display(), error = %e, "record: cannot write markets_selected.json");
        }
        self.markets_line(data, self.last_seq, now);
    }

    fn markets_line(&mut self, data: Vec<Value>, seq: u64, now: u64) {
        // A market recorded as resolved is left out: a later unresolved copy (an upstream that
        // still lists it) would hide the result from the replay, which reads the latest
        // observation at its clock.
        let data: Vec<Value> = data
            .into_iter()
            .filter(|m| {
                m["symbol"]
                    .as_str()
                    .is_some_and(|s| !self.resolutions.contains_key(s))
            })
            .collect();
        if data.is_empty() {
            return;
        }
        let mut line = Self::line(
            "GET",
            "/api/v1/markets".into(),
            Self::body(Value::Array(data), seq, now),
            None,
            now,
        );
        line["trimmed"] = "data filtered to the recorded symbols".into();
        self.append(MARKETS, &line, now);
    }

    fn search_line(&mut self, markets: Vec<Value>, seq: u64, state_ms: u64, source: Option<&str>) {
        let markets: Vec<Value> = markets
            .into_iter()
            .filter(|m| {
                m["symbol"]
                    .as_str()
                    .is_some_and(|s| self.selected.contains_key(s))
            })
            .collect();
        if markets.is_empty() {
            return;
        }
        for m in &markets {
            if let (Some(s), Some(r)) = (m["symbol"].as_str(), m.get("resolution"))
                && r.is_object()
            {
                self.resolutions
                    .entry(s.to_owned())
                    .or_insert_with(|| r.clone());
            }
        }
        let symbols: Vec<&str> = markets
            .iter()
            .filter_map(|m| m["symbol"].as_str())
            .collect();
        let path = "/api/v1/markets/search".to_owned();
        let mut line = Self::line(
            "POST",
            path,
            Self::body(Value::Array(markets.clone()), seq, state_ms),
            source,
            state_ms,
        );
        line["request"] = json!({ "symbols": symbols });
        self.append(SEARCH, &line, state_ms);
    }

    fn handle(&mut self, weak: &Weak<Shared>, cmd: RecCmd) {
        let now = crate::util::wall_ms();
        match cmd {
            RecCmd::Catalogue { seq, state_time_ms } => {
                let Some(shared) = weak.upgrade() else {
                    return;
                };
                let symbols: Vec<String> = self.selected.keys().cloned().collect();
                let markets = shared.engine().live_markets_raw(&symbols);
                drop(shared);
                for m in &markets {
                    if let Some(s) = m["symbol"].as_str() {
                        self.selected.insert(s.to_owned(), m.clone());
                    }
                }
                let seq = seq.unwrap_or(self.last_seq);
                self.last_seq = self.last_seq.max(seq);
                self.markets_line(markets, seq, state_time_ms.unwrap_or(now));
                self.write_manifest("recording", now);
            }
            RecCmd::Search {
                markets,
                seq,
                state_time_ms,
            } => {
                let seq = seq.unwrap_or(self.last_seq);
                self.search_line(markets, seq, state_time_ms.unwrap_or(now), None);
            }
            RecCmd::Resolved {
                market,
                seq,
                state_time_ms,
            } => {
                self.search_line(
                    vec![market],
                    seq,
                    state_time_ms.unwrap_or(now),
                    Some("ws market_update"),
                );
                self.write_manifest("recording", now);
            }
            RecCmd::Stop(..) => {}
        }
    }

    /// `manifest.json`, in the spirit of `record_tape.py`'s.
    fn write_manifest(&self, stop_reason: &str, now: u64) {
        let games: Vec<Value> = self
            .selected
            .iter()
            .map(|(symbol, m)| {
                let d = &m["display_attributes"];
                let teams = &d["market_type"]["teams"];
                let mut g = json!({
                    "symbol": symbol,
                    "game_id": d["game_page"]["game_id"],
                    "event_description": d["event_description"],
                    "long": d["outcome_labels"]["long"]["abbreviated"],
                    "short": d["outcome_labels"]["short"]["abbreviated"],
                    "away": teams["away_name"],
                    "home": teams["home_name"],
                    "start_ms": str_u64(&d["expected_event_start_time_ms"]),
                    "expected_resolution_ms": str_u64(&d["expected_resolution_time_ms"]),
                });
                if let Some(r) = self.resolutions.get(symbol) {
                    g["resolution"] = r.clone();
                }
                if let Some(b) = self.book_stats.get(symbol) {
                    g["books"] = json!({
                        "snapshots": b.snapshots, "nonempty": b.nonempty,
                        "first_state_time_ms": b.first_ms, "last_state_time_ms": b.last_ms,
                    });
                }
                g
            })
            .collect();
        let game_ids: BTreeSet<&str> = self
            .selected
            .values()
            .filter_map(|m| m["display_attributes"]["game_page"]["game_id"].as_str())
            .collect();
        let raw_files: BTreeMap<&str, Value> = self
            .files
            .iter()
            .map(|(name, s)| {
                (
                    *name,
                    json!({"lines": s.lines, "bytes": s.bytes,
                        "first_state_time_ms": s.first_ms, "last_state_time_ms": s.last_ms}),
                )
            })
            .collect();
        let first = self.files.values().filter_map(|s| s.first_ms).min();
        let last = self.files.values().filter_map(|s| s.last_ms).max();
        let resolutions: BTreeMap<&String, &Value> = self
            .resolutions
            .iter()
            .map(|(s, r)| (s, &r["price"]))
            .collect();
        let manifest = json!({
            "tape": self.dir.file_name().and_then(|n| n.to_str()),
            "source": "pascal-mock --live (upstream book and market_update sockets, /markets, /markets/search)",
            "base_url": format!("https://{}", self.upstream),
            "upstream": self.upstream,
            "selection_time_ms": self.started_ms,
            "symbols": self.selected.keys().collect::<Vec<_>>(),
            "game_ids": game_ids,
            "games": games,
            "resolutions": resolutions,
            "raw_files": raw_files,
            "first_state_time_ms": first,
            "last_state_time_ms": last,
            "recorder": {"started_ms": self.started_ms, "updated_ms": now, "stop_reason": stop_reason},
        });
        let path = self.dir.join("manifest.json");
        let text = serde_json::to_string_pretty(&manifest).unwrap_or_default();
        if let Err(e) = fs::write(&path, text + "\n") {
            tracing::warn!(path = %path.display(), error = %e, "record: cannot write manifest.json");
        }
    }
}

/// The recorder task.
pub(crate) async fn run(
    weak: Weak<Shared>,
    mut rec: Recorder,
    mut cmds: mpsc::UnboundedReceiver<RecCmd>,
    every: Duration,
) {
    tracing::info!(dir = %rec.dir.display(), "record: writing the live session as a tape");
    let mut tick = tokio::time::interval(every);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut alive = tokio::time::interval(Duration::from_secs(1));
    alive.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let reason = loop {
        tokio::select! {
            _ = tick.tick() => {
                let Some(shared) = weak.upgrade() else { break "mock dropped".to_owned() };
                rec.tick(&shared);
            }
            cmd = cmds.recv() => match cmd {
                None => break "mock dropped".to_owned(),
                Some(RecCmd::Stop(reason, ack)) => {
                    if let Some(shared) = weak.upgrade() {
                        rec.tick(&shared);
                    }
                    rec.write_manifest(&reason, crate::util::wall_ms());
                    tracing::info!(dir = %rec.dir.display(), %reason, "record: tape finalised");
                    let _ = ack.send(());
                    return;
                }
                Some(c) => rec.handle(&weak, c),
            },
            _ = alive.tick() => if weak.strong_count() == 0 { break "mock dropped".to_owned() },
        }
    };
    rec.write_manifest(&reason, crate::util::wall_ms());
}
