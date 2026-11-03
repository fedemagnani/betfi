//! In-play drift: the `book` stream, the pre-hedge trigger and the drift rebalance (plan step
//! 4.C, spec section 3 "Rebalance triggers", l4-contracts L4-4).
//!
//! # The book task ([`Hedger::spawn_drift`])
//!
//! - **Subscriptions.** One socket ([`Pascal::subscribe`](pascal_rs::PascalApi::subscribe) on
//!   `PASCAL_WS_URL`) carries a `book` channel for every symbol with an unresolved leg on a
//!   pending or open ticket, soonest start first, at most `HEDGE_BOOK_MAX_SUBS`. The set is
//!   recomputed every evaluation tick; when it changes the old socket is dropped (which closes
//!   it) and a new one opens with the new set, at most once per [`RESUBSCRIBE_MIN`]. With no
//!   symbol wanted there is no socket. `pascal-rs` reconnects a dropped socket by itself and
//!   resubscribes with fresh snapshots.
//! - **Live view.** [`LiveView`](super::books::LiveView) rebuilds each book from its snapshot and
//!   diffs; every applied frame is published into `AppState::books` and the quoter's cache, so
//!   `/tickets` cash-out values see it (a quiet book ages out of the quoter's freshness rule as a
//!   REST book would; the stream never extends a book's life).
//! - **REST fallback.** A wanted symbol without a live book (socket down, stale after a reconnect
//!   until its snapshot lands, refused, or beyond the subscription cap) is read through the
//!   quoter's `/books` fetch (≤ 50 symbols per call, its back-off) at most every
//!   `HEDGE_BOOK_REST_MS`.
//!
//! # Triggers (every `HEDGE_DRIFT_EVAL_MS`, after start-up recovery, with hedger keys only)
//!
//! - **Pre-hedge (D-029).** An unresolved leg of an open ticket outside its hedged set whose
//!   user's-side mid ([`pricing::side_mid`]) is dearer than the signed quote price by more than
//!   5 points ([`pricing::prehedge_trigger`]). Confirmed on a fresh REST book, then written to
//!   `hedge_prehedges` (migration 0007) **before** any order, then added to the hedged set
//!   ([`pricing::hedged_set`] `prehedged`), and the ticket's symbols are traded to `Tᵢ` with an
//!   IOC (`kind = prehedge`). One-shot per ticket leg: a restart reloads the rows, so the leg
//!   stays hedged and the trigger never fires for it again.
//! - **Drift rebalance.** A symbol whose `held + resting` is off `Tᵢ` by more than 10% and 2
//!   contracts ([`pricing::needs_rebalance`]) gets one IOC back to target (`kind = drift`).
//!
//! **Debounce:** a trigger fires only once it has held on every evaluation for
//! `HEDGE_DRIFT_DEBOUNCE_MS` (default 1 s, i.e. ≥ 4 consecutive 250 ms checks; any check where
//! it does not hold, or where the book is unknown, restarts the wait), and a symbol gets at most
//! one drift order per `HEDGE_DRIFT_COOLDOWN_MS` (default 10 s). A symbol whose trading lock is
//! taken (an inception, roll or unwind in flight) is not evaluated for drift on that tick.
//!
//! **Caps:** both orders go through [`Hedger::trade_symbol`], so `Tᵢ` is clamped to
//! `Caps::max_target` of the visible depth; an order that opens exposure is also refused when the
//! hedger's known free collateral does not cover its worst-case cost plus fees
//! ([`Hedger::drift_collateral_ok`]).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use pascal_rs::{AccountQuery, Channel, OrderBook, WsEvent, WsMessage, WsStream};
use pricing::HedgeLeg;
use sqlx::{Row, SqlitePool};
use tokio::task::JoinHandle;

use super::Hedger;
use super::books::FrameMeta;
use super::exec::How;
use super::targets::{self, Prehedged, pside};
use crate::api::types::{LegStatus, Side, TicketStatus};
use crate::now_ms;
use crate::state::{Symbol, TicketId};

/// Fewest time between two socket (re)openings for a subscription change.
pub const RESUBSCRIBE_MIN: Duration = Duration::from_secs(1);

/// Settings of the book task. Every variable is optional; unparsable values keep the default.
#[derive(Debug, Clone)]
pub struct DriftConfig {
    /// `HEDGE_BOOK_STREAM=0` disables the book task (no socket, no pre-hedge, no drift).
    pub enabled: bool,
    /// `HEDGE_BOOK_MAX_SUBS`: most `book` subscriptions on the socket (50); further symbols are
    /// read over REST.
    pub max_subscriptions: usize,
    /// `HEDGE_DRIFT_DEBOUNCE_MS`: how long a trigger must hold before it fires (1 s).
    pub debounce: Duration,
    /// `HEDGE_DRIFT_COOLDOWN_MS`: least time between two drift orders on one symbol (10 s).
    pub cooldown: Duration,
    /// `HEDGE_DRIFT_EVAL_MS`: trigger evaluation and subscription check period (250 ms).
    pub eval_every: Duration,
    /// `HEDGE_BOOK_REST_MS`: REST `/books` fallback period for symbols without a live book (2 s).
    pub rest_every: Duration,
}

impl Default for DriftConfig {
    fn default() -> Self {
        DriftConfig {
            enabled: true,
            max_subscriptions: 50,
            debounce: Duration::from_secs(1),
            cooldown: Duration::from_secs(10),
            eval_every: Duration::from_millis(250),
            rest_every: Duration::from_secs(2),
        }
    }
}

impl DriftConfig {
    /// From `get` (the process environment, or a map in tests).
    pub fn from_lookup(get: &dyn Fn(&str) -> Option<String>) -> Self {
        let mut c = DriftConfig::default();
        let num = |k: &str| {
            get(k).and_then(|v| {
                let v = v.trim().to_owned();
                if v.is_empty() {
                    return None;
                }
                v.parse::<u64>()
                    .map_err(|_| tracing::warn!(var = k, value = %v, "not a number; default kept"))
                    .ok()
            })
        };
        if let Some(v) = get("HEDGE_BOOK_STREAM") {
            c.enabled = !matches!(v.trim(), "0" | "false" | "no");
        }
        if let Some(v) = num("HEDGE_BOOK_MAX_SUBS") {
            c.max_subscriptions = usize::try_from(v).unwrap_or(usize::MAX);
        }
        if let Some(v) = num("HEDGE_DRIFT_DEBOUNCE_MS") {
            c.debounce = Duration::from_millis(v);
        }
        if let Some(v) = num("HEDGE_DRIFT_COOLDOWN_MS") {
            c.cooldown = Duration::from_millis(v);
        }
        if let Some(v) = num("HEDGE_DRIFT_EVAL_MS") {
            c.eval_every = Duration::from_millis(v.max(10));
        }
        if let Some(v) = num("HEDGE_BOOK_REST_MS") {
            c.rest_every = Duration::from_millis(v.max(100));
        }
        c
    }

    /// REST books at most this old stand in for a live book.
    fn rest_max_age_ms(&self) -> u64 {
        u64::try_from(self.rest_every.as_millis().saturating_mul(2))
            .unwrap_or(u64::MAX)
            .max(2_000)
    }
}

/// Counters of the book task (tests and `/debug/hedge`).
#[derive(Debug, Default)]
pub struct DriftStats {
    prehedges: AtomicU64,
    prehedge_orders: AtomicU64,
    drift_orders: AtomicU64,
    rest_polls: AtomicU64,
    subscriptions_opened: AtomicU64,
    disconnects: AtomicU64,
}

/// A copy of [`DriftStats`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct DriftCounts {
    /// Ticket legs pre-hedged (rows written).
    pub prehedges: u64,
    /// `prehedge` orders sent.
    pub prehedge_orders: u64,
    /// `drift` orders sent.
    pub drift_orders: u64,
    /// REST `/books` fallback reads.
    pub rest_polls: u64,
    /// Sockets opened for a subscription set.
    pub subscriptions_opened: u64,
    /// Socket drops and lags reported by `pascal-rs`.
    pub disconnects: u64,
}

impl DriftStats {
    fn bump(c: &AtomicU64) {
        c.fetch_add(1, Ordering::Relaxed);
    }

    /// A copy of the counters.
    pub fn counts(&self) -> DriftCounts {
        let g = |c: &AtomicU64| c.load(Ordering::Relaxed);
        DriftCounts {
            prehedges: g(&self.prehedges),
            prehedge_orders: g(&self.prehedge_orders),
            drift_orders: g(&self.drift_orders),
            rest_polls: g(&self.rest_polls),
            subscriptions_opened: g(&self.subscriptions_opened),
            disconnects: g(&self.disconnects),
        }
    }

    /// Counts an order of `kind` (`prehedge` / `drift`) sent on `symbol`.
    pub(super) fn order_sent(&self, kind: &str, symbol: &str) {
        match kind {
            "prehedge" => Self::bump(&self.prehedge_orders),
            "drift" => {
                Self::bump(&self.drift_orders);
                metrics::counter!("hedge_drift_rebalances_total", "symbol" => symbol.to_owned())
                    .increment(1);
            }
            _ => {}
        }
    }
}

/// Where a trigger's book came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Ws,
    Rest,
}

impl Source {
    fn as_str(self) -> &'static str {
        match self {
            Source::Ws => "ws",
            Source::Rest => "rest",
        }
    }
}

/// A leg whose pre-hedge trigger holds now.
#[derive(Debug, Clone)]
struct Candidate {
    ticket: TicketId,
    idx: u8,
    symbol: Symbol,
    side: Side,
    quote_u: u64,
    price_u: u64,
    source: Source,
}

/// The book task's own state (no locks: one task owns it).
#[derive(Default)]
struct Loop {
    /// Last socket opening.
    opened_at: Option<Instant>,
    /// Reopen even if the set did not change (a book frame failed to decode).
    force_reopen: bool,
    /// Last REST fallback read.
    last_rest: Option<Instant>,
    /// Since when each pre-hedge trigger holds.
    pre_since: HashMap<(TicketId, u8), Instant>,
    /// Since when each symbol is off target.
    drift_since: HashMap<Symbol, Instant>,
    /// Last drift order per symbol.
    last_drift: HashMap<Symbol, Instant>,
}

enum Step {
    Event(Option<WsEvent>),
    Tick,
}

async fn next_event(s: &mut Option<WsStream>) -> Option<WsEvent> {
    match s {
        Some(s) => s.next().await,
        None => std::future::pending().await,
    }
}

impl Hedger {
    /// Starts the book task (module docs), or nothing when `HEDGE_BOOK_STREAM=0`.
    pub fn spawn_drift(&self) -> Option<JoinHandle<()>> {
        if !self.cfg.drift.enabled {
            tracing::info!("book stream disabled (HEDGE_BOOK_STREAM=0): no pre-hedge, no drift");
            return None;
        }
        let h = self.clone();
        Some(tokio::spawn(async move { h.drift_loop().await }))
    }

    /// Counters of the book task.
    pub fn drift_counts(&self) -> DriftCounts {
        self.drift_stats.counts()
    }

    /// The live (socket) book of `symbol`, if it is live now.
    pub fn live_book(&self, symbol: &str) -> Option<OrderBook> {
        self.live.live(symbol).map(|b| b.book)
    }

    /// Whether start-up recovery has finished (tickets, pre-hedge marks and `held` reloaded).
    /// Drift and pre-hedge trading wait for it.
    pub fn is_recovered(&self) -> bool {
        self.recovered.load(Ordering::Acquire)
    }

    /// Socket and subscription status.
    pub fn stream_status(&self) -> super::books::ViewStatus {
        self.live.status()
    }

    /// Pre-hedged ticket legs.
    pub fn prehedged_legs(&self) -> Vec<(TicketId, u8)> {
        let pre = self.prehedged.read();
        pre.iter()
            .flat_map(|(t, s)| s.iter().map(move |i| (t.clone(), *i)))
            .collect()
    }

    async fn drift_loop(self) {
        let state = self.state.clone();
        let mut st = Loop::default();
        let mut stream: Option<WsStream> = None;
        let every = self.cfg.drift.eval_every;
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tracing::info!(cfg = ?self.cfg.drift, "book stream task started");
        loop {
            let step = tokio::select! {
                _ = state.shutdown_requested() => break,
                ev = next_event(&mut stream) => Step::Event(ev),
                _ = tick.tick() => Step::Tick,
            };
            match step {
                Step::Event(Some(ev)) => self.on_ws_event(ev, &mut st),
                Step::Event(None) => {
                    tracing::warn!("book stream ended; reopening");
                    stream = None;
                    self.live.on_disconnected();
                }
                Step::Tick => {
                    tokio::select! {
                        _ = state.shutdown_requested() => break,
                        _ = self.resubscribe(&mut st, &mut stream) => {}
                    }
                    tokio::select! {
                        _ = state.shutdown_requested() => break,
                        _ = self.evaluate(&mut st) => {}
                    }
                }
            }
        }
        drop(stream);
        self.live.resubscribed(&BTreeSet::new());
        tracing::info!("book stream task stopped");
    }

    /// Every symbol with an unresolved leg on a pending or open ticket, soonest start first.
    fn wanted_symbols(&self) -> Vec<Symbol> {
        let mut first: BTreeMap<Symbol, u64> = BTreeMap::new();
        {
            let tickets = self.state.tickets.read();
            for t in tickets.values() {
                if !matches!(t.status, TicketStatus::Pending | TicketStatus::Open) {
                    continue;
                }
                for l in t.legs.iter().filter(|l| l.status == LegStatus::Open) {
                    first
                        .entry(l.symbol.clone())
                        .and_modify(|s| *s = (*s).min(l.start_ms))
                        .or_insert(l.start_ms);
                }
            }
        }
        let mut v: Vec<(u64, Symbol)> = first
            .into_iter()
            .filter(|(s, _)| !self.is_resolved(s))
            .map(|(s, t)| (t, s))
            .collect();
        v.sort();
        v.into_iter().map(|(_, s)| s).collect()
    }

    /// Opens a socket for the wanted set when it changed (or the socket is gone), dropping the
    /// old one first so at most one socket is open.
    async fn resubscribe(&self, st: &mut Loop, stream: &mut Option<WsStream>) {
        let mut wanted = self.wanted_symbols();
        wanted.truncate(self.cfg.drift.max_subscriptions);
        let wanted: BTreeSet<Symbol> = wanted.into_iter().collect();
        let current = self.live.subscribed();
        let gone = stream.is_none() && !wanted.is_empty();
        if wanted == current && !gone && !st.force_reopen {
            return;
        }
        if st.opened_at.is_some_and(|t| t.elapsed() < RESUBSCRIBE_MIN) {
            return;
        }
        st.force_reopen = false;
        // Close the old socket before the new one opens: no leaked subscription.
        *stream = None;
        self.live.resubscribed(&wanted);
        metrics::gauge!("hedge_book_subscriptions").set(wanted.len() as f64);
        if wanted.is_empty() {
            if !current.is_empty() {
                tracing::info!("no open legs: book socket closed");
            }
            return;
        }
        st.opened_at = Some(Instant::now());
        let channels: Vec<Channel> = wanted
            .iter()
            .map(|s| Channel::Book { symbol: s.clone() })
            .collect();
        match self.state.pascal.subscribe(channels).await {
            Ok(s) => {
                DriftStats::bump(&self.drift_stats.subscriptions_opened);
                tracing::info!(symbols = wanted.len(), "book socket opened");
                *stream = Some(s);
            }
            Err(e) => {
                tracing::warn!(error = %e, "book subscribe failed; REST fallback, retrying");
            }
        }
    }

    fn on_ws_event(&self, ev: WsEvent, st: &mut Loop) {
        match ev {
            WsEvent::Connected { channels } => {
                self.live.on_connected();
                tracing::info!(channels, "book socket connected");
            }
            WsEvent::Frame(f) => {
                let meta = FrameMeta {
                    seq: f.seq,
                    state_time_ms: f.state_time_ms,
                    server_time_ms: f.server_time_ms,
                    recv_ms: now_ms(),
                };
                match f.message {
                    WsMessage::Book { symbol, kind, book } => {
                        let Some(p) = self.live.apply(&symbol, kind, &book, meta) else {
                            return;
                        };
                        // One lock at a time, the hedge-private view's already released.
                        self.state.books.write().insert(symbol.clone(), p.live);
                        let fetched = HashMap::from([(symbol.clone(), p.cached)]);
                        self.state
                            .quoter
                            .store(std::slice::from_ref(&symbol), fetched);
                    }
                    WsMessage::Error {
                        channel: Some(Channel::Book { symbol }),
                        error,
                        ..
                    } => {
                        tracing::warn!(%symbol, code = %error.code, message = %error.message, "book subscription refused; REST fallback");
                        self.live.refuse(&symbol);
                    }
                    _ => {}
                }
            }
            WsEvent::DecodeError { error, text } => {
                tracing::warn!(%error, "undecodable socket frame");
                if text.contains("\"book\"") {
                    // A lost diff leaves the rebuilt book wrong: start over with snapshots.
                    self.live.on_disconnected();
                    st.force_reopen = true;
                }
            }
            WsEvent::Lagged { waited } => {
                DriftStats::bump(&self.drift_stats.disconnects);
                self.live.on_disconnected();
                tracing::warn!(
                    ?waited,
                    "book socket lagged; snapshots follow the reconnect"
                );
            }
            WsEvent::Disconnected { reason, retry_in } => {
                DriftStats::bump(&self.drift_stats.disconnects);
                metrics::counter!("hedge_book_disconnects_total").increment(1);
                self.live.on_disconnected();
                tracing::warn!(%reason, ?retry_in, "book socket down; REST fallback");
            }
        }
    }

    /// Books for the triggers: live ones, else REST (fetched at most every `rest_every`).
    async fn trigger_books(
        &self,
        wanted: &[Symbol],
        st: &mut Loop,
    ) -> HashMap<Symbol, (OrderBook, Source)> {
        let mut out = HashMap::new();
        let mut missing = Vec::new();
        for s in wanted {
            match self.live.live(s) {
                Some(b) => {
                    out.insert(s.clone(), (b.book, Source::Ws));
                }
                None => missing.push(s.clone()),
            }
        }
        if missing.is_empty() {
            return out;
        }
        let due = st
            .last_rest
            .is_none_or(|t| t.elapsed() >= self.cfg.drift.rest_every);
        if due && !self.state.quoter.backing_off(now_ms()) {
            st.last_rest = Some(Instant::now());
            DriftStats::bump(&self.drift_stats.rest_polls);
            metrics::counter!("hedge_book_rest_fallback_total").increment(1);
            if let Err(e) = crate::quoter::books::fetch(
                &self.state,
                &missing,
                crate::quoter::books::FETCH_TIMEOUT,
            )
            .await
            {
                tracing::debug!(error = %e, "REST books fallback failed");
            }
        }
        let now = now_ms();
        let max_age = self.cfg.drift.rest_max_age_ms();
        for s in missing {
            if let Some(c) = self.state.quoter.cached(&s)
                && now.saturating_sub(c.fetched_ms) <= max_age
            {
                out.insert(s, (c.book, Source::Rest));
            }
        }
        out
    }

    async fn evaluate(&self, st: &mut Loop) {
        if self.trader.is_none() || !self.recovered.load(Ordering::Acquire) {
            return;
        }
        let wanted = self.wanted_symbols();
        let books = self.trigger_books(&wanted, st).await;
        let now = Instant::now();
        let fired = self.prehedge_due(&books, st, now);
        if !fired.is_empty() {
            self.fire_prehedges(fired, st).await;
        }
        self.drift_due_and_trade(st, now).await;
    }

    /// Legs whose pre-hedge trigger held for the debounce window.
    fn prehedge_due(
        &self,
        books: &HashMap<Symbol, (OrderBook, Source)>,
        st: &mut Loop,
        now: Instant,
    ) -> Vec<Candidate> {
        let pre: Prehedged = self.prehedged.read().clone();
        let gl = self.cfg.params.game_length_ms;
        let move_u = self.cfg.params.prehedge_move_u;
        let mut hits = Vec::new();
        {
            let tickets = self.state.tickets.read();
            // A lost leg zeroes every delta of its ticket: nothing left to pre-hedge.
            for t in tickets.values().filter(|t| {
                t.status == TicketStatus::Open && t.legs.iter().all(|l| l.status != LegStatus::Lost)
            }) {
                let hl: Vec<HedgeLeg> = t
                    .legs
                    .iter()
                    .map(|l| HedgeLeg {
                        start_ms: l.start_ms,
                        resolved: l.status != LegStatus::Open,
                        prehedged: targets::is_prehedged(&pre, &t.ticket, l.idx),
                    })
                    .collect();
                let set = pricing::hedged_set(&hl, gl);
                for (l, in_set) in t.legs.iter().zip(set) {
                    if in_set || l.status != LegStatus::Open {
                        continue;
                    }
                    let Some((book, source)) = books.get(&l.symbol) else {
                        continue;
                    };
                    let Some(price_u) = pricing::side_mid(book, pside(l.side)) else {
                        continue;
                    };
                    if pricing::prehedge_trigger(l.p_quote_u, price_u, move_u) {
                        hits.push(Candidate {
                            ticket: t.ticket.clone(),
                            idx: l.idx,
                            symbol: l.symbol.clone(),
                            side: l.side,
                            quote_u: l.p_quote_u,
                            price_u,
                            source: *source,
                        });
                    }
                }
            }
        }
        hits.retain(|c| !self.is_resolved(&c.symbol));
        let holding: HashSet<(TicketId, u8)> =
            hits.iter().map(|c| (c.ticket.clone(), c.idx)).collect();
        st.pre_since.retain(|k, _| holding.contains(k));
        hits.into_iter()
            .filter(|c| {
                let since = *st.pre_since.entry((c.ticket.clone(), c.idx)).or_insert(now);
                now.duration_since(since) >= self.cfg.drift.debounce
            })
            .collect()
    }

    /// Confirms each due pre-hedge on a fresh REST book, writes it, adds the legs to the hedged
    /// set, then trades the touched tickets' symbols to target.
    async fn fire_prehedges(&self, fired: Vec<Candidate>, st: &mut Loop) {
        let move_u = self.cfg.params.prehedge_move_u;
        let mut confirmed: Vec<(TicketId, u8)> = Vec::new();
        for c in fired {
            let key = (c.ticket.clone(), c.idx);
            st.pre_since.remove(&key);
            let fresh = self
                .book(&c.symbol)
                .await
                .and_then(|b| pricing::side_mid(&b, pside(c.side)));
            match fresh {
                Some(p) if pricing::prehedge_trigger(c.quote_u, p, move_u) => {}
                other => {
                    tracing::info!(ticket = %c.ticket, leg = c.idx, symbol = %c.symbol, fresh = ?other, "pre-hedge not confirmed on a fresh book");
                    continue;
                }
            }
            if let Err(e) = insert_prehedge(&self.state.db, &c, now_ms()).await {
                tracing::error!(error = %e, ticket = %c.ticket, leg = c.idx, "hedge_prehedges write failed: not pre-hedging");
                continue;
            }
            DriftStats::bump(&self.drift_stats.prehedges);
            metrics::counter!("hedge_prehedges_total", "symbol" => c.symbol.clone()).increment(1);
            tracing::info!(
                ticket = %c.ticket, leg = c.idx, symbol = %c.symbol, side = ?c.side,
                quote_u = c.quote_u, price_u = c.price_u, source = c.source.as_str(),
                "pre-hedge: leg joins the hedged set"
            );
            confirmed.push((c.ticket, c.idx));
        }
        if confirmed.is_empty() {
            return;
        }
        // Trade only the symbols whose target the new hedged set moves (the pre-hedged legs and
        // any leg that joins through an overlap); a symbol within its rounding of the target
        // (D-123) is left alone. Targets before the marks, then the marks, then after.
        let mut symbols: BTreeSet<Symbol> = BTreeSet::new();
        for (t, _) in &confirmed {
            symbols.extend(self.ticket_symbols(t));
        }
        let before: Vec<(Symbol, i64)> = symbols
            .iter()
            .map(|s| (s.clone(), self.target(s)))
            .collect();
        {
            let mut pre = self.prehedged.write();
            for (t, i) in &confirmed {
                pre.entry(t.clone()).or_default().insert(*i);
            }
        }
        for (t, _) in &confirmed {
            self.refresh_ticket(t);
        }
        for (s, old) in before {
            if self.target(&s) != old {
                self.trade_symbol(&s, How::Prehedge, "prehedge", None).await;
            }
        }
    }

    /// Symbols off target past the debounce and the cooldown get one drift IOC each.
    async fn drift_due_and_trade(&self, st: &mut Loop, now: Instant) {
        let mut symbols: BTreeSet<Symbol> = targets::symbols_of(&self.state.tickets.read())
            .into_iter()
            .collect();
        symbols.extend(self.state.held.read().keys().cloned());
        let params = &self.cfg.params;
        let mut due = Vec::new();
        for s in &symbols {
            if self.is_resolved(s) {
                st.drift_since.remove(s);
                continue;
            }
            let busy = self.symbol_lock(s).try_lock().is_err();
            let pos = self.held(s) + self.resting(s);
            let target = self.target(s);
            let off = pricing::needs_rebalance(
                pos,
                target,
                params.rebalance_rel_ppm,
                params.rebalance_min_contracts,
            );
            if busy || !off {
                st.drift_since.remove(s);
                continue;
            }
            let since = *st.drift_since.entry(s.clone()).or_insert(now);
            let cooling = st
                .last_drift
                .get(s)
                .is_some_and(|t| now.duration_since(*t) < self.cfg.drift.cooldown);
            if now.duration_since(since) >= self.cfg.drift.debounce && !cooling {
                due.push(s.clone());
            }
        }
        st.drift_since.retain(|s, _| symbols.contains(s));
        st.last_drift
            .retain(|_, t| now.duration_since(*t) < self.cfg.drift.cooldown);
        for s in due {
            tracing::info!(symbol = %s, "drift: off target, rebalancing");
            st.last_drift.insert(s.clone(), now);
            st.drift_since.remove(&s);
            self.trade_symbol(&s, How::Drift, "drift", None).await;
        }
    }

    /// Hedger cap (spec section 3) for a pre-hedge or drift order that opens exposure: the
    /// known free collateral covers `size` at `limit_u` plus taker and builder fees. Unknown
    /// collateral lets the order go (as the inception does).
    pub(super) async fn drift_collateral_ok(
        &self,
        symbol: &str,
        side: pascal_rs::Side,
        size: u64,
        limit_u: i64,
    ) -> bool {
        let Some(t) = &self.trader else {
            return false;
        };
        let p = u64::try_from(limit_u).unwrap_or(0).min(targets::UNIT);
        let per = match side {
            pascal_rs::Side::Bid => p,
            pascal_rs::Side::Ask => targets::UNIT - p,
        };
        let taker_ppm = self
            .state
            .catalogue()
            .and_then(|c| c.market(symbol).map(|m| m.taker_fee_rate_ppm))
            .unwrap_or(20_000);
        let cost = size.saturating_mul(per)
            + pricing::taker_fee_u(size, p, taker_ppm)
            + pricing::taker_fee_u(size, p, self.cfg.builder_fee_rate_u);
        let free = match self
            .state
            .pascal
            .account(
                t.owner(),
                &AccountQuery {
                    symbol: None,
                    exclude_orders: true,
                },
            )
            .await
        {
            Ok(r) => {
                if let Some(c) = r.data.collateral_usd {
                    self.set_free_collateral(c);
                }
                r.data.collateral_usd
            }
            Err(e) => {
                super::trader::count_error(&e);
                self.free_collateral()
            }
        };
        match free {
            Some(f)
                if !self
                    .cfg
                    .params
                    .caps
                    .hedger_covers(u64::try_from(f).unwrap_or(0), cost) =>
            {
                metrics::counter!("hedge_cap_clamped_total", "cap" => "hedger_collateral")
                    .increment(1);
                tracing::warn!(%symbol, size, cost, free = f, "hedger free collateral short: no drift order");
                false
            }
            _ => true,
        }
    }
}

/// Writes one pre-hedge row (`INSERT OR IGNORE`: a leg is pre-hedged once).
async fn insert_prehedge(db: &SqlitePool, c: &Candidate, ts_ms: u64) -> Result<(), sqlx::Error> {
    let i = |v: u64| i64::try_from(v).unwrap_or(i64::MAX);
    sqlx::query(
        "INSERT OR IGNORE INTO hedge_prehedges
            (ticket, leg_idx, symbol, side, quote_price_u, trigger_price_u, source, ts_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&c.ticket)
    .bind(i64::from(c.idx))
    .bind(&c.symbol)
    .bind(match c.side {
        Side::Long => "long",
        Side::Short => "short",
    })
    .bind(i(c.quote_u))
    .bind(i(c.price_u))
    .bind(c.source.as_str())
    .bind(i(ts_ms))
    .execute(db)
    .await?;
    Ok(())
}

/// Pre-hedged legs of the tickets the DB still has open (start-up recovery).
pub async fn load_prehedges(db: &SqlitePool) -> Result<Prehedged, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT p.ticket, p.leg_idx FROM hedge_prehedges p
         JOIN tickets t ON t.ticket = p.ticket
         WHERE t.state = 'open'",
    )
    .fetch_all(db)
    .await?;
    let mut out = Prehedged::new();
    for r in rows {
        let idx = u8::try_from(r.get::<i64, _>("leg_idx")).unwrap_or(u8::MAX);
        out.entry(r.get::<String, _>("ticket"))
            .or_default()
            .insert(idx);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_from_env() {
        let c = DriftConfig::from_lookup(&|k| match k {
            "HEDGE_BOOK_STREAM" => Some("1".into()),
            "HEDGE_BOOK_MAX_SUBS" => Some("7".into()),
            "HEDGE_DRIFT_DEBOUNCE_MS" => Some("300".into()),
            "HEDGE_DRIFT_COOLDOWN_MS" => Some("x".into()),
            "HEDGE_BOOK_REST_MS" => Some("5".into()),
            _ => None,
        });
        assert!(c.enabled);
        assert_eq!(c.max_subscriptions, 7);
        assert_eq!(c.debounce, Duration::from_millis(300));
        assert_eq!(c.cooldown, Duration::from_secs(10));
        assert_eq!(c.eval_every, Duration::from_millis(250));
        assert_eq!(c.rest_every, Duration::from_millis(100));
        assert_eq!(c.rest_max_age_ms(), 2_000);
        let off = DriftConfig::from_lookup(&|k| (k == "HEDGE_BOOK_STREAM").then(|| "0".into()));
        assert!(!off.enabled);
    }
}
