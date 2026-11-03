//! Live mode (L4-1): Pascal's real books and results from the free public read API, the mock's
//! own matcher for every order. **Nothing is ever forwarded**: orders, cancels, accounts, fills
//! and builder stats stay in the engine, and the only upstream is Pascal's read host
//! (`https://data.pascal.trade`, or a loopback fake in tests); [`LiveConfig::validate`] refuses
//! any other `*.pascal.trade` host.
//!
//! Three background tasks, each holding only a [`Weak`] handle on the mock (they stop when the
//! last [`crate::Mock`] is dropped, or on [`crate::Mock::stop_live`]):
//!
//! - **socket** ([`pascal_rs::WsClient`] on `wss://<upstream>/ws`): `market_update` from boot,
//!   then one `book` channel per symbol the desk asks about (`/books`, `/orders`, the mock's own
//!   `/ws` `book` channel), subscribed lazily. Snapshots and level updates go into the engine's
//!   feed ([`crate::Engine::live_book`]). A drop, a lag or a reconnect marks every book stale;
//!   each stays stale (no fills, `NOT_READY`) until its fresh snapshot lands. `WsClient`
//!   reconnects with exponential backoff and jitter (1 s → 60 s) and pings every 30 s. A
//!   `market_update` `resolved` entry resolves the market ([`crate::Engine::live_resolution`]),
//!   unless the mock already resolved it or has a resolution scheduled (a market resolves once).
//!   A book channel the upstream refuses is dropped and not asked for again for a minute.
//! - **rest** ([`pascal_rs::Client`], D-004 buckets: 5 req/s per host, burst 10, `/markets` at
//!   most once per 30 s): `GET /api/v1/markets` every 60 s (the catalogue `/markets`,
//!   `/markets/search` and `text-search` are served from), and `POST /markets/search` every 60 s
//!   for tracked unresolved symbols whose game started ≥ 2.5 h ago or that left `/markets` (the
//!   slow resolution backstop, D-012), plus symbols the desk searched that the mock did not know.
//! - **trades pass-through** (L5-4): `GET /api/v1/trades` is answered from the upstream's
//!   `GET /api/v1/trades` with the same query (symbols, limit, cursor), read only, through a
//!   client that shares the rest task's D-004 limiter, with its own bucket for the route
//!   ([`TRADES_ROUTE_LIMIT`]), at most [`TRADES_IN_FLIGHT`] calls in flight (more answer
//!   `RATE_LIMITED` at once), one retry and a [`TRADES_TIMEOUT`]. The mock's own fills are not
//!   mixed into the upstream tape (the cursors are the upstream's).
//! - **resolved pass-through** (31 Oct): `GET /api/v1/markets/resolved` also lists the
//!   upstream's resolved markets for symbols the mock does not track (read only, through the
//!   same limiter and in-flight cap, one upstream page per query reused for
//!   [`RESOLVED_CACHE`]). A desk that lost a ticket's leg names (a fresh database) names them
//!   again from it and resolves them; the mock's own resolutions always come first.
//! - **recorder** (`--record <dir>`, optional): the tape format of `tapes/README.md`
//!   ([`crate::record`]), so a live session replays later with `--tape <dir>`.
//!
//! Lock discipline: the engine mutex is taken briefly and never held across an `.await`;
//! [`LiveLink`]'s own small mutexes are never held while the engine lock is being taken.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use pascal_rs::ratelimit::{Backoff, BucketConfig, RateLimits, RetryPolicy};
use pascal_rs::ws::{FrameKind, WsConfig, WsEvent, WsMessage};
use pascal_rs::{Channel, Client, Market, MarketSearch, MarketUpdateType, WsClient};
use serde_json::Value;
use tokio::sync::{Notify, Semaphore, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::api::Shared;
use crate::record::{RecCmd, Recorder};

/// Pascal's public read host, the default upstream.
pub const DEFAULT_UPSTREAM: &str = "https://data.pascal.trade";
/// The only `*.pascal.trade` host live mode may talk to.
pub const READ_HOST: &str = "data.pascal.trade";
/// Fewest seconds between two `GET /markets` against a real upstream (D-004 allows 30; L4-1
/// asks for 60).
pub const MIN_MARKETS_EVERY: Duration = Duration::from_secs(60);
/// Fewest seconds between two backstop searches against a real upstream.
pub const MIN_SEARCH_EVERY: Duration = Duration::from_secs(30);
/// Most `POST /markets/search` symbols per call (Pascal's limit).
const SEARCH_CHUNK: usize = 500;
/// The trades route, as the rate limiter keys it.
pub const TRADES_ROUTE: &str = "/api/v1/trades";
/// Its own bucket against a real upstream, inside the per-host one: 1 call per second,
/// bursts of 3 (a desk following one card pages a few times every 15 s).
pub const TRADES_ROUTE_LIMIT: BucketConfig = BucketConfig {
    per_second: 1.0,
    burst: 3.0,
};
/// Most trades pass-throughs in flight; the next one answers `RATE_LIMITED` at once rather
/// than queueing on the limiter.
pub const TRADES_IN_FLIGHT: usize = 2;
/// A trades pass-through gives up after this long (`TIMEOUT`).
pub const TRADES_TIMEOUT: Duration = Duration::from_secs(8);

/// The resolved-markets route, as the rate limiter keys it.
pub const RESOLVED_ROUTE: &str = "/api/v1/markets/resolved";
/// Its own bucket against a real upstream: one call per 10 s, bursts of 2.
pub const RESOLVED_ROUTE_LIMIT: BucketConfig = BucketConfig {
    per_second: 0.1,
    burst: 2.0,
};
/// An upstream resolved page is reused this long for the same query (a desk sweeps every 20 s).
pub const RESOLVED_CACHE: Duration = Duration::from_secs(60);

/// Live-mode settings.
#[derive(Debug, Clone)]
pub struct LiveConfig {
    /// Upstream read API base URL (default [`DEFAULT_UPSTREAM`]).
    pub upstream: String,
    /// Upstream socket; default `<upstream with ws/wss>/ws`.
    pub ws_url: Option<String>,
    /// Record what arrives into this tape directory.
    pub record: Option<PathBuf>,
    /// Catalogue refresh cadence (default 60 s, at least [`MIN_MARKETS_EVERY`] upstream).
    pub markets_every: Duration,
    /// Backstop search cadence (default 60 s).
    pub search_every: Duration,
    /// A tracked market is searched upstream once its game started this long ago (2.5 h).
    pub search_after_start: Duration,
    /// `/books` waits up to this long for the first (or post-reconnect) snapshot of a
    /// requested book (default 1 s, under the desk's 1.5 s fetch timeout).
    pub ready_wait: Duration,
    /// Recording: books snapshot cadence (default 5 s, like `record_tape.py`).
    pub record_books_every: Duration,
    /// Read-API rate limits. Against a non-loopback upstream the D-004 defaults are enforced.
    pub rate_limits: RateLimits,
    /// Socket reconnect backoff (default 1 s doubling to 60 s, with jitter).
    pub ws_reconnect: Backoff,
    /// A book channel the upstream refused is not requested again for this long (60 s).
    pub retry_refused_after: Duration,
    /// Most upstream book subscriptions (default 200).
    pub max_books: usize,
}

impl LiveConfig {
    /// Defaults for `upstream`.
    pub fn new(upstream: impl Into<String>) -> Self {
        LiveConfig {
            upstream: upstream.into(),
            ws_url: None,
            record: None,
            markets_every: Duration::from_secs(60),
            search_every: Duration::from_secs(60),
            search_after_start: Duration::from_secs(150 * 60),
            ready_wait: Duration::from_secs(1),
            record_books_every: Duration::from_secs(5),
            rate_limits: RateLimits::default(),
            ws_reconnect: Backoff {
                base: Duration::from_secs(1),
                max: Duration::from_secs(60),
            },
            retry_refused_after: Duration::from_secs(60),
            max_books: 200,
        }
    }

    /// Checks the hosts and applies the floors that protect a real upstream. Returns the
    /// upstream host. Refuses any `*.pascal.trade` host other than [`READ_HOST`] (mode M, L3-1:
    /// the mock never calls a Pascal trade host).
    pub fn validate(&mut self) -> Result<String, String> {
        let host = check_host(&self.upstream, &["http", "https"])?;
        let ws = self.ws_url();
        let ws_host = check_host(&ws, &["ws", "wss"])?;
        if ws_host != host {
            return Err(format!(
                "the socket host {ws_host} differs from the upstream {host}"
            ));
        }
        if !is_loopback(&host) {
            if !self.upstream.starts_with("https://") || !ws.starts_with("wss://") {
                return Err(format!(
                    "{host}: a non-loopback upstream must use https:// and wss://"
                ));
            }
            self.rate_limits = RateLimits::default();
            self.rate_limits
                .per_route
                .insert(TRADES_ROUTE.to_owned(), TRADES_ROUTE_LIMIT);
            self.rate_limits
                .per_route
                .insert(RESOLVED_ROUTE.to_owned(), RESOLVED_ROUTE_LIMIT);
            self.markets_every = self.markets_every.max(MIN_MARKETS_EVERY);
            self.search_every = self.search_every.max(MIN_SEARCH_EVERY);
            self.ws_reconnect.base = self.ws_reconnect.base.max(Duration::from_secs(1));
            self.ws_reconnect.max = self.ws_reconnect.max.max(Duration::from_secs(30));
            self.retry_refused_after = self.retry_refused_after.max(Duration::from_secs(60));
        }
        Ok(host)
    }

    /// The socket URL.
    pub fn ws_url(&self) -> String {
        if let Some(u) = &self.ws_url {
            return u.clone();
        }
        let base = self.upstream.trim_end_matches('/');
        let base = if let Some(rest) = base.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = base.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            base.to_owned()
        };
        format!("{base}/ws")
    }
}

/// The host of `url`, if its scheme is allowed and it is not a Pascal host other than the read
/// host.
fn check_host(url: &str, schemes: &[&str]) -> Result<String, String> {
    let u = url::Url::parse(url).map_err(|e| format!("{url:?} is not a URL: {e}"))?;
    if !schemes.contains(&u.scheme()) {
        return Err(format!("{url:?}: scheme must be one of {schemes:?}"));
    }
    let host = u
        .host_str()
        .ok_or_else(|| format!("{url:?} has no host"))?
        .to_ascii_lowercase();
    let host = host.trim_end_matches('.').to_owned();
    let domain = pascal_domain();
    let pascal = host == domain || host.ends_with(&format!(".{domain}"));
    if pascal && host != READ_HOST {
        return Err(format!(
            "{host} is not Pascal's read host: live mode reads {READ_HOST} only and never \
             calls Pascal's write API (mode M)"
        ));
    }
    Ok(host)
}

/// Pascal's domain, the read host's parent: every other host under it may carry Pascal's write
/// API, so live mode refuses them all.
fn pascal_domain() -> &'static str {
    READ_HOST.split_once('.').map_or(READ_HOST, |(_, d)| d)
}

/// Whether `host` is this machine.
pub fn is_loopback(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    h == "localhost"
        || h.parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

pub(crate) enum WsCmd {
    Subscribe(Vec<String>),
    Unsubscribe(Vec<String>),
    Stop,
}

pub(crate) enum RestCmd {
    Stop,
}

/// The live mode's handle, shared by the HTTP handlers and the background tasks.
pub struct LiveLink {
    host: String,
    ready_wait: Duration,
    retry_refused_after: Duration,
    max_books: usize,
    recording: Option<PathBuf>,
    ws_cmd: mpsc::UnboundedSender<WsCmd>,
    rest_cmd: mpsc::UnboundedSender<RestCmd>,
    rec_cmd: Option<mpsc::UnboundedSender<RecCmd>>,
    ws_up: AtomicBool,
    catalogue_ok: AtomicBool,
    /// Local Unix ms of the last good catalogue refresh (0 = never).
    catalogue_ms: AtomicU64,
    /// Book channels the upstream refused, and when.
    refused: Mutex<HashMap<String, Instant>>,
    /// Symbols searched for (and not known) since the last backstop pass.
    wanted: Mutex<BTreeSet<String>>,
    /// Woken when a book gets a snapshot or the catalogue loads.
    changed: Notify,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    /// The upstream read client of the trades pass-through, set by the rest task (it shares
    /// that task's limiter). `None` until the task has started.
    reads: OnceLock<Client>,
    /// Trades pass-throughs in flight ([`TRADES_IN_FLIGHT`]).
    trades_slots: Semaphore,
    /// The upstream's resolved markets per query string, and when they were read.
    resolved_cache: Mutex<HashMap<String, (Instant, Vec<Value>)>>,
}

/// Why a trades pass-through failed: a Pascal error code (the mock answers it with the code's
/// HTTP status) and a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassError {
    /// Pascal error code (`RATE_LIMITED`, `NOT_READY`, `TIMEOUT`, `BAD_GATEWAY`, an upstream
    /// code such as `INVALID_REQUEST`).
    pub code: String,
    /// Message.
    pub message: String,
}

impl PassError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        PassError {
            code: code.to_owned(),
            message: message.into(),
        }
    }
}

/// Channel ends handed to [`spawn`].
pub(crate) struct LiveTasks {
    cfg: LiveConfig,
    ws_rx: mpsc::UnboundedReceiver<WsCmd>,
    rest_rx: mpsc::UnboundedReceiver<RestCmd>,
    rec: Option<(Recorder, mpsc::UnboundedReceiver<RecCmd>)>,
}

impl LiveLink {
    /// A link and the receiving ends for its tasks. `cfg` must be validated; `recorder` is
    /// the opened tape directory when recording.
    pub(crate) fn new(
        cfg: LiveConfig,
        host: String,
        recorder: Option<Recorder>,
    ) -> (Arc<LiveLink>, LiveTasks) {
        let (ws_cmd, ws_rx) = mpsc::unbounded_channel();
        let (rest_cmd, rest_rx) = mpsc::unbounded_channel();
        let (rec_cmd, rec) = match recorder {
            Some(r) => {
                let (tx, rx) = mpsc::unbounded_channel();
                (Some(tx), Some((r, rx)))
            }
            None => (None, None),
        };
        let link = Arc::new(LiveLink {
            host,
            ready_wait: cfg.ready_wait,
            retry_refused_after: cfg.retry_refused_after,
            max_books: cfg.max_books,
            recording: cfg.record.clone(),
            ws_cmd,
            rest_cmd,
            rec_cmd,
            ws_up: AtomicBool::new(false),
            catalogue_ok: AtomicBool::new(false),
            catalogue_ms: AtomicU64::new(0),
            refused: Mutex::new(HashMap::new()),
            wanted: Mutex::new(BTreeSet::new()),
            changed: Notify::new(),
            tasks: Mutex::new(Vec::new()),
            reads: OnceLock::new(),
            trades_slots: Semaphore::new(TRADES_IN_FLIGHT),
            resolved_cache: Mutex::new(HashMap::new()),
        });
        (
            link,
            LiveTasks {
                cfg,
                ws_rx,
                rest_rx,
                rec,
            },
        )
    }

    /// The upstream host.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Socket connected and the catalogue loaded and current.
    pub fn upstream_ok(&self) -> bool {
        self.ws_up.load(Ordering::Relaxed) && self.catalogue_ok.load(Ordering::Relaxed)
    }

    /// Whether the upstream socket is connected.
    pub fn socket_up(&self) -> bool {
        self.ws_up.load(Ordering::Relaxed)
    }

    /// Whether a catalogue was ever loaded.
    pub fn catalogue_loaded(&self) -> bool {
        self.catalogue_ms.load(Ordering::Relaxed) > 0
    }

    /// Local Unix ms of the last good catalogue refresh.
    pub fn catalogue_ms(&self) -> Option<u64> {
        Some(self.catalogue_ms.load(Ordering::Relaxed)).filter(|&t| t > 0)
    }

    /// The tape directory being recorded into.
    pub fn recording(&self) -> Option<&PathBuf> {
        self.recording.as_ref()
    }

    /// Subscribes the upstream `book` channel of each symbol not tracked yet (known to the
    /// catalogue, unresolved, not refused within the last minute), up to the book cap.
    pub fn track(&self, shared: &Shared, symbols: &[String]) {
        let now = Instant::now();
        // Copy the refusals first: this lock is never held while the engine lock is taken.
        let refused: Vec<String> = {
            let mut r = self.refused.lock().unwrap_or_else(|p| p.into_inner());
            r.retain(|_, at| now.duration_since(*at) < self.retry_refused_after);
            r.keys().cloned().collect()
        };
        let mut new = Vec::new();
        {
            let mut e = shared.engine();
            let loaded = e.feed_counts().catalogue_ms.is_some();
            let mut books = e.feed_counts().books;
            for s in symbols {
                if e.has_live_book(s)
                    || e.resolution(s).is_some()
                    || refused.contains(s)
                    || (loaded && !e.knows(s))
                    || new.contains(s)
                {
                    continue;
                }
                if books >= self.max_books {
                    tracing::warn!(symbol = %s, max = self.max_books, "live: book cap reached; not subscribing");
                    break;
                }
                e.live_track(s);
                books += 1;
                new.push(s.clone());
            }
        }
        if !new.is_empty() {
            tracing::info!(symbols = ?new, "live: subscribing upstream books");
            let _ = self.ws_cmd.send(WsCmd::Subscribe(new));
        }
    }

    /// Waits (up to the configured `ready_wait`) until every requested book the mock tracks is
    /// fresh, while the socket is up. Never holds the engine lock while waiting.
    pub async fn wait_fresh(&self, shared: &Shared, symbols: &[String]) {
        let deadline = tokio::time::Instant::now() + self.ready_wait;
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.socket_up() {
                return;
            }
            let waiting = {
                let e = shared.engine();
                symbols
                    .iter()
                    .any(|s| e.has_live_book(s) && !e.book_fresh(s) && e.resolution(s).is_none())
            };
            if !waiting {
                return;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return;
            }
        }
    }

    /// Waits until the first catalogue loaded (or `timeout`). Returns whether it did.
    pub async fn wait_catalogue(&self, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.catalogue_loaded() {
                return true;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return self.catalogue_loaded();
            }
        }
    }

    /// Stops following `symbol`'s upstream book (the mock resolved the market itself). The
    /// entry stays, so `/books` keeps answering the resolved market's empty book.
    pub fn unfollow(&self, symbol: &str) {
        let _ = self
            .ws_cmd
            .send(WsCmd::Unsubscribe(vec![symbol.to_owned()]));
    }

    /// Queues symbols for the next upstream search (desk searches the mock does not know).
    pub fn want_search(&self, symbols: Vec<String>) {
        if symbols.is_empty() {
            return;
        }
        let mut w = self.wanted.lock().unwrap_or_else(|p| p.into_inner());
        for s in symbols {
            if w.len() >= SEARCH_CHUNK {
                break;
            }
            w.insert(s);
        }
    }

    /// Stops the tasks: closes the socket, finalises the recording's manifest with
    /// `stop_reason`. Idempotent.
    pub async fn stop(&self, stop_reason: &str) {
        let _ = self.ws_cmd.send(WsCmd::Stop);
        let _ = self.rest_cmd.send(RestCmd::Stop);
        if let Some(tx) = &self.rec_cmd {
            let (ack, done) = oneshot::channel();
            if tx.send(RecCmd::Stop(stop_reason.to_owned(), ack)).is_ok() {
                let _ = tokio::time::timeout(Duration::from_secs(10), done).await;
            }
        }
        let tasks: Vec<JoinHandle<()>> =
            std::mem::take(&mut *self.tasks.lock().unwrap_or_else(|p| p.into_inner()));
        // The recording is final; a task still inside an upstream request (a /markets fetch
        // can take seconds) is not waited for.
        for mut t in tasks {
            if tokio::time::timeout(Duration::from_secs(1), &mut t)
                .await
                .is_err()
            {
                t.abort();
            }
        }
    }

    /// `GET /api/v1/trades` from the upstream with `query` (already checked by the handler:
    /// symbols, limit, at most one of the cursors). Returns the upstream's body as served.
    /// Read only; never retried more than once; refused at once when [`TRADES_IN_FLIGHT`]
    /// calls are already waiting on the upstream.
    pub async fn trades(&self, query: &[(String, String)]) -> Result<Value, PassError> {
        let Some(client) = self.reads.get() else {
            return Err(PassError::new(
                "NOT_READY",
                "pascal-mock live: the upstream client has not started yet",
            ));
        };
        let Ok(_slot) = self.trades_slots.try_acquire() else {
            return Err(PassError::new(
                "RATE_LIMITED",
                "pascal-mock live: too many trades reads in flight to the upstream",
            ));
        };
        let call = client.get::<Value>(TRADES_ROUTE, query);
        match tokio::time::timeout(TRADES_TIMEOUT, call).await {
            Err(_) => Err(PassError::new(
                "TIMEOUT",
                "pascal-mock live: the upstream trades read timed out",
            )),
            Ok(Ok(resp)) => Ok(resp.raw),
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "live: upstream trades read failed");
                Err(match e.api_code() {
                    Some(code) => PassError::new(code, e.to_string()),
                    None => PassError::new("BAD_GATEWAY", e.to_string()),
                })
            }
        }
    }

    /// `GET /api/v1/markets/resolved` from the upstream with `query` (checked by the handler),
    /// its `data` items. Read only; one upstream call per query per [`RESOLVED_CACHE`]; refused
    /// at once when [`TRADES_IN_FLIGHT`] reads are already waiting on the upstream.
    pub async fn resolved(&self, query: &[(String, String)]) -> Result<Vec<Value>, PassError> {
        let key = query
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        if let Some((at, items)) = self
            .resolved_cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&key)
            && at.elapsed() < RESOLVED_CACHE
        {
            return Ok(items.clone());
        }
        let Some(client) = self.reads.get() else {
            return Err(PassError::new(
                "NOT_READY",
                "pascal-mock live: the upstream client has not started yet",
            ));
        };
        let Ok(_slot) = self.trades_slots.try_acquire() else {
            return Err(PassError::new(
                "RATE_LIMITED",
                "pascal-mock live: too many reads in flight to the upstream",
            ));
        };
        let call = client.get::<Value>(RESOLVED_ROUTE, query);
        let raw = match tokio::time::timeout(TRADES_TIMEOUT, call).await {
            Err(_) => {
                return Err(PassError::new(
                    "TIMEOUT",
                    "pascal-mock live: the upstream resolved read timed out",
                ));
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "live: upstream resolved read failed");
                return Err(PassError::new("BAD_GATEWAY", e.to_string()));
            }
            Ok(Ok(resp)) => resp.raw,
        };
        let items = match raw.get("data").unwrap_or(&raw) {
            Value::Array(a) => a.clone(),
            _ => Vec::new(),
        };
        self.resolved_cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key, (Instant::now(), items.clone()));
        Ok(items)
    }

    fn record(&self, cmd: RecCmd) {
        if let Some(tx) = &self.rec_cmd {
            let _ = tx.send(cmd);
        }
    }
}

impl Drop for LiveLink {
    fn drop(&mut self) {
        for t in self
            .tasks
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .drain(..)
        {
            t.abort();
        }
    }
}

/// Starts the tasks on the current Tokio runtime.
pub(crate) fn spawn(shared: &Arc<Shared>, link: &Arc<LiveLink>, t: LiveTasks) {
    let LiveTasks {
        cfg,
        ws_rx,
        rest_rx,
        rec,
    } = t;
    let mut tasks = vec![
        tokio::spawn(ws_task(Arc::downgrade(shared), cfg.clone(), ws_rx)),
        tokio::spawn(rest_task(Arc::downgrade(shared), cfg.clone(), rest_rx)),
    ];
    if let Some((recorder, rx)) = rec {
        tasks.push(tokio::spawn(crate::record::run(
            Arc::downgrade(shared),
            recorder,
            rx,
            cfg.record_books_every,
        )));
    }
    link.tasks
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .extend(tasks);
}

fn link_of(shared: &Shared) -> Option<Arc<LiveLink>> {
    shared.live().cloned()
}

// ---------- socket ----------

async fn ws_task(weak: Weak<Shared>, cfg: LiveConfig, mut cmds: mpsc::UnboundedReceiver<WsCmd>) {
    let ws_cfg = WsConfig {
        url: cfg.ws_url(),
        reconnect: cfg.ws_reconnect,
        ..WsConfig::default()
    };
    let (client, mut events) = WsClient::spawn(ws_cfg, [Channel::MarketUpdate]);
    let mut alive = tokio::time::interval(Duration::from_secs(1));
    alive.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            ev = events.recv() => {
                let Some(ev) = ev else { break };
                let Some(shared) = weak.upgrade() else { break };
                on_ws_event(&shared, &client, ev);
            }
            cmd = cmds.recv() => match cmd {
                None | Some(WsCmd::Stop) => break,
                Some(WsCmd::Subscribe(s)) => {
                    let _ = client.subscribe(s.into_iter().map(|symbol| Channel::Book { symbol }));
                }
                Some(WsCmd::Unsubscribe(s)) => {
                    let _ = client.unsubscribe(s.into_iter().map(|symbol| Channel::Book { symbol }));
                }
            },
            _ = alive.tick() => if weak.strong_count() == 0 { break },
        }
    }
    if let Some(shared) = weak.upgrade()
        && let Some(link) = link_of(&shared)
    {
        link.ws_up.store(false, Ordering::Relaxed);
    }
    client.shutdown().await;
}

fn on_ws_event(shared: &Shared, client: &WsClient, ev: WsEvent) {
    let Some(link) = link_of(shared) else {
        return;
    };
    match ev {
        WsEvent::Connected { channels } => {
            // Books from an earlier session stay stale until this session's snapshots.
            let stale = shared.engine().live_stale_all();
            link.ws_up.store(true, Ordering::Relaxed);
            tracing::info!(channels, stale, "live: upstream socket connected");
        }
        WsEvent::Disconnected { reason, retry_in } => {
            link.ws_up.store(false, Ordering::Relaxed);
            let stale = shared.engine().live_stale_all();
            tracing::warn!(%reason, ?retry_in, stale, "live: upstream socket dropped; books stale until their snapshots");
            link.changed.notify_waiters();
        }
        WsEvent::Lagged { waited } => {
            link.ws_up.store(false, Ordering::Relaxed);
            let stale = shared.engine().live_stale_all();
            tracing::warn!(
                ?waited,
                stale,
                "live: fell behind the upstream socket; resyncing"
            );
            link.changed.notify_waiters();
        }
        WsEvent::DecodeError { error, text } => {
            let cut: String = text.chars().take(300).collect();
            tracing::warn!(%error, frame = %cut, "live: undecodable upstream frame");
        }
        WsEvent::Frame(f) => match f.message {
            WsMessage::Book { symbol, kind, book } => {
                let snapshot = kind == FrameKind::Snapshot;
                let applied = shared.engine().live_book(&symbol, snapshot, &book, f.seq);
                if snapshot && applied {
                    tracing::debug!(%symbol, asks = book.asks.len(), bids = book.bids.len(), "live: book snapshot");
                    link.changed.notify_waiters();
                }
            }
            WsMessage::MarketUpdate { updates, .. } => {
                for fail in &updates.failures {
                    tracing::warn!(index = fail.index, error = %fail.error, "live: undecodable market_update entry");
                }
                for (i, u) in updates.items {
                    let raw = f.raw["data"][i]["market"].clone();
                    let symbol = u.market.symbol.clone();
                    let resolution = u.market.resolution.clone();
                    let resolved_update = u.update_type == MarketUpdateType::Resolved;
                    if resolved_update && resolution.is_none() {
                        tracing::warn!(%symbol, "live: resolved market_update without a resolution");
                    }
                    let (was, now, tracked) = {
                        let mut e = shared.engine();
                        let was = e.resolution(&symbol).cloned();
                        e.live_markets_seen(vec![(u.market, raw.clone())], !resolved_update);
                        (
                            was,
                            e.resolution(&symbol).cloned(),
                            e.has_live_book(&symbol),
                        )
                    };
                    if let Some(r) = &resolution {
                        if tracked {
                            link.record(RecCmd::Resolved {
                                market: raw,
                                seq: f.seq.unwrap_or(r.seq),
                                state_time_ms: f.state_time_ms,
                            });
                        }
                        if was.is_none() && now.is_some() {
                            tracing::info!(%symbol, price = r.price, "live: upstream resolution");
                        }
                        if tracked {
                            let _ = client.unsubscribe([Channel::Book {
                                symbol: symbol.clone(),
                            }]);
                        }
                    }
                }
            }
            WsMessage::Error { channel, error, .. } => match channel {
                Some(Channel::Book { symbol }) => {
                    link.refused
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .insert(symbol.clone(), Instant::now());
                    shared.engine().live_untrack(&symbol);
                    tracing::warn!(%symbol, code = %error.code, message = %error.message,
                        "live: upstream refused the book channel");
                    link.changed.notify_waiters();
                }
                other => {
                    tracing::warn!(channel = ?other, code = %error.code, message = %error.message,
                        "live: upstream error frame");
                }
            },
            WsMessage::Pong
            | WsMessage::Account { .. }
            | WsMessage::AllTrades { .. }
            | WsMessage::Trades { .. } => {}
        },
    }
}

// ---------- REST: catalogue and search backstop ----------

async fn rest_task(
    weak: Weak<Shared>,
    cfg: LiveConfig,
    mut cmds: mpsc::UnboundedReceiver<RestCmd>,
) {
    let client = match Client::builder()
        .base_url(cfg.upstream.clone())
        .rate_limits(cfg.rate_limits.clone())
        .user_agent(concat!("pascal-mock-live/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "live: cannot build the upstream client");
            return;
        }
    };
    // The trades pass-through's client: same host and limiter (one D-004 budget for the whole
    // mock), one retry at most, so a desk's read never waits minutes behind a backoff.
    if let Some(link) = weak.upgrade().as_deref().and_then(link_of) {
        match Client::builder()
            .base_url(cfg.upstream.clone())
            .rate_limiter(client.rate_limiter())
            .retry(RetryPolicy {
                max_attempts: 2,
                backoff: Backoff {
                    base: Duration::from_millis(500),
                    max: Duration::from_secs(2),
                },
            })
            .user_agent(concat!("pascal-mock-live/", env!("CARGO_PKG_VERSION")))
            .build()
        {
            Ok(reads) => {
                let _ = link.reads.set(reads);
            }
            Err(e) => tracing::error!(error = %e, "live: cannot build the trades client"),
        }
    }
    let mut markets = tokio::time::interval(cfg.markets_every);
    markets.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut search = tokio::time::interval_at(
        tokio::time::Instant::now() + cfg.search_every,
        cfg.search_every,
    );
    search.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut alive = tokio::time::interval(Duration::from_secs(1));
    alive.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let after_start_ms = u64::try_from(cfg.search_after_start.as_millis()).unwrap_or(u64::MAX);
    let mut last_ok: Option<Instant> = None;
    loop {
        tokio::select! {
            _ = markets.tick() => {
                if !refresh_catalogue(&weak, &client, &mut last_ok, cfg.markets_every).await {
                    break;
                }
            }
            _ = search.tick() => {
                if !backstop(&weak, &client, after_start_ms).await {
                    break;
                }
            }
            cmd = cmds.recv() => match cmd {
                None | Some(RestCmd::Stop) => break,
            },
            _ = alive.tick() => if weak.strong_count() == 0 { break },
        }
    }
}

/// Splits a market list response into typed markets with their raw JSON.
fn with_raw(
    items: Vec<(usize, Market)>,
    raw: &Value,
    failures: usize,
    what: &str,
) -> Vec<(Market, Value)> {
    if failures > 0 {
        tracing::warn!(
            failures,
            what,
            "live: upstream markets that did not decode were skipped"
        );
    }
    let list = raw["data"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    items
        .into_iter()
        .filter_map(|(i, m)| list.get(i).map(|r| (m, r.clone())))
        .collect()
}

/// One `GET /markets`. Returns false when the mock is gone.
async fn refresh_catalogue(
    weak: &Weak<Shared>,
    client: &Client,
    last_ok: &mut Option<Instant>,
    every: Duration,
) -> bool {
    let started = Instant::now();
    let res = client.markets_lenient().await;
    let Some(shared) = weak.upgrade() else {
        return false;
    };
    let Some(link) = link_of(&shared) else {
        return false;
    };
    match res {
        Ok(resp) => {
            let failures = resp.data.failures.len();
            let items = with_raw(resp.data.items, &resp.raw, failures, "markets");
            let n = items.len();
            let (seq, state_ms) = (resp.seq, resp.state_time_ms);
            let now_ms = {
                let mut e = shared.engine();
                e.live_catalogue(items, seq);
                e.now_ms()
            };
            *last_ok = Some(Instant::now());
            link.catalogue_ms.store(now_ms.max(1), Ordering::Relaxed);
            link.catalogue_ok.store(true, Ordering::Relaxed);
            link.changed.notify_waiters();
            link.record(RecCmd::Catalogue {
                seq,
                state_time_ms: state_ms,
            });
            tracing::info!(
                markets = n,
                ms = started.elapsed().as_millis() as u64,
                "live: catalogue refreshed"
            );
        }
        Err(e) => {
            // One failed refresh keeps the cache; three missed cadences mark the upstream down.
            let down = last_ok.is_none_or(|t| t.elapsed() > every * 3);
            if down {
                link.catalogue_ok.store(false, Ordering::Relaxed);
            }
            tracing::warn!(error = %e, down, "live: catalogue refresh failed");
        }
    }
    true
}

/// One backstop pass. Returns false when the mock is gone.
async fn backstop(weak: &Weak<Shared>, client: &Client, after_start_ms: u64) -> bool {
    let (mut due, link) = {
        let Some(shared) = weak.upgrade() else {
            return false;
        };
        let Some(link) = link_of(&shared) else {
            return false;
        };
        let due = shared.engine().live_search_due(after_start_ms);
        (due, link)
    };
    let wanted: Vec<String> =
        std::mem::take(&mut *link.wanted.lock().unwrap_or_else(|p| p.into_inner()))
            .into_iter()
            .collect();
    for w in wanted {
        if !due.contains(&w) {
            due.push(w);
        }
    }
    drop(link);
    for chunk in due.chunks(SEARCH_CHUNK) {
        let body = MarketSearch {
            symbols: chunk.to_vec(),
            ..MarketSearch::default()
        };
        let res = client.search_markets_lenient(&body).await;
        let Some(shared) = weak.upgrade() else {
            return false;
        };
        match res {
            Ok(resp) => {
                let failures = resp.data.failures.len();
                let items = with_raw(resp.data.items, &resp.raw, failures, "search");
                let raws: Vec<Value> = items.iter().map(|(_, r)| r.clone()).collect();
                let resolved = items.iter().filter(|(m, _)| m.resolution.is_some()).count();
                shared.engine().live_markets_seen(items, false);
                tracing::info!(
                    asked = chunk.len(),
                    found = raws.len(),
                    resolved,
                    "live: backstop search"
                );
                if let Some(link) = link_of(&shared) {
                    link.record(RecCmd::Search {
                        markets: raws,
                        seq: resp.seq,
                        state_time_ms: resp.state_time_ms,
                    });
                }
            }
            Err(e) => tracing::warn!(error = %e, "live: backstop search failed"),
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts() {
        let mut c = LiveConfig::new(DEFAULT_UPSTREAM);
        assert_eq!(c.validate().unwrap(), READ_HOST);
        assert_eq!(c.ws_url(), "wss://data.pascal.trade/ws");
        // Every other host under Pascal's domain is refused (built from the read host, so this
        // file names no write-capable host, D-142).
        let d = pascal_domain();
        for bad in [
            format!("https://{d}"),
            format!("https://api.{d}"),
            format!("https://DATA2.{d}"),
            format!("wss://x.{d}"),
            format!("ftp://{READ_HOST}"),
            format!("http://{READ_HOST}"),
            "not a url".to_owned(),
        ] {
            assert!(LiveConfig::new(&bad).validate().is_err(), "{bad}");
        }
        assert!(
            LiveConfig::new(format!("https://{READ_HOST}."))
                .validate()
                .is_ok()
        );
        let mut c = LiveConfig::new(DEFAULT_UPSTREAM);
        c.ws_url = Some("wss://elsewhere.example/ws".into());
        assert!(c.validate().is_err());
        let mut c = LiveConfig::new("http://127.0.0.1:9");
        c.markets_every = Duration::from_millis(100);
        assert_eq!(c.validate().unwrap(), "127.0.0.1");
        assert_eq!(c.markets_every, Duration::from_millis(100));
        assert_eq!(c.ws_url(), "ws://127.0.0.1:9/ws");
    }

    #[test]
    fn a_real_upstream_gets_the_d004_floors() {
        let mut c = LiveConfig::new(DEFAULT_UPSTREAM);
        c.markets_every = Duration::from_secs(1);
        c.search_every = Duration::from_secs(1);
        c.rate_limits.per_host.per_second = 1_000.0;
        c.ws_reconnect.base = Duration::from_millis(1);
        c.validate().unwrap();
        assert_eq!(c.markets_every, MIN_MARKETS_EVERY);
        assert_eq!(c.search_every, MIN_SEARCH_EVERY);
        let mut want = RateLimits::default();
        want.per_route
            .insert(TRADES_ROUTE.to_owned(), TRADES_ROUTE_LIMIT);
        want.per_route
            .insert(RESOLVED_ROUTE.to_owned(), RESOLVED_ROUTE_LIMIT);
        assert_eq!(c.rate_limits, want);
        assert_eq!(c.ws_reconnect.base, Duration::from_secs(1));
        assert!(is_loopback("localhost") && is_loopback("[::1]") && !is_loopback("10.0.0.1"));
    }
}
