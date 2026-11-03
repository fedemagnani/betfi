//! Hedge desk (spec section 3, plan step 2.B): the roll/ladder state machine driven by bus
//! events. It owns [`tickets`](crate::state::Shared::tickets), [`held`](crate::state::Shared::held)
//! and [`order_map`](crate::state::Shared::order_map); writes `pascal_fills`, `fill_allocations`,
//! `hedge_positions`, `hedge_orders` (0004) and logs every outbound order as its signed payload.
//!
//! # Events (l2-contracts L2-3)
//!
//! | Event | Action |
//! | --- | --- |
//! | `PendingOpened` | re-quote from fresh books ([`pricing::quote`]); floor missed → nothing (the pending expires and the crank refunds it); else one batch of IOC limits at the per-leg max prices on the inception hedged set; `fᵢ`, `r`; unwind the excess over `r·Δᵢ`; `HedgeFilled`, or (`r = 0`, floor) unwind + `HedgeUnwound` + `RefundNeeded` |
//! | `FinalizeRejected` | unwind the pending's hedge, `HedgeUnwound { pending: Some }`, `RefundNeeded` |
//! | `TicketMinted` | the ticket's targets switch from "what the inception kept" to the section 3 formula |
//! | `LegResolved` | won/half: roll the next leg to `S/(p₁…pₖ)` (post-only one tick inside, 20 s, then IOC); lost: unwind the ticket's share; `HedgeUnwound` |
//! | `TicketCashedOut` | unwind the ticket's share, `HedgeUnwound` |
//! | `TicketSettled`, `PendingRefunded` | drop the ticket (unwinding anything left, e.g. a pending the crank refunded on its timer) |
//! | `MarketResolved` | only marks the symbol resolved (Pascal settled the position): **`LegResolved` is the one trigger for rolls and unwinds** (D-070) |
//!
//! Every 30 s `held` is reconciled from `/accounts/{hedger}` and legs off target by more than 10%
//! and 2 contracts are rebalanced with an IOC. At start the desk replays
//! `/accounts/{hedger}/fills` since the last stored `seq`, rebuilds its tickets from the DB and
//! recomputes the targets ([`Hedger::recover`]).
//!
//! # Execution
//!
//! Inception orders are sized per ticket (`Δᵢ` of the fresh quote), so `r` is that ticket's own
//! fill ratio. Every later order is sized at the symbol level, `Tᵢ − held − resting`, so opposite
//! tickets net through `held` (D-071). Trading on a symbol is serialised by a per-symbol async
//! lock; the `std` guards of the shared maps are never held across an `.await`.
//!
//! [`flatten::flatten`] is the panic button, run by the `desk-flatten` binary (D-073).
//!
//! # In-play drift (4.C)
//!
//! [`drift`] runs beside the event loop: it keeps a live view of the `book` channel ([`books`])
//! for every symbol with an open leg, pre-hedges an unhedged later leg whose side got dearer by
//! more than 5 points since its quote (D-029, persisted in `hedge_prehedges`), and trades a
//! hedged symbol off target by more than 10% and 2 contracts back to `Tᵢ`, both debounced.

pub mod books;
pub mod drift;
pub mod exec;
pub mod flatten;
pub mod inception;
pub mod recover;
pub mod store;
pub mod targets;
pub mod trader;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

pub use drift::{DriftConfig, DriftCounts};
pub use exec::How;
pub use trader::{BUILDER_FEE_RATE_U, OrderSpec, SignedOrder, Trader};

use crate::api::types::{LegStatus, TicketFill, TicketStatus};
use crate::state::{
    AppState, DeskEvent, HedgeUnwound, Locked, Recv, Symbol, TicketId, TicketState, recv,
};
use targets::{Prehedged, RTilde};

/// Hedge desk settings: the pricing knobs plus the desk's timers. Read from the environment by
/// [`HedgeConfig::from_env`]; every variable is optional (L2-0: never a new required one).
#[derive(Debug, Clone)]
pub struct HedgeConfig {
    /// Section 2/3 knobs (slippage, game length, caps, rebalance thresholds).
    pub params: pricing::Params,
    /// Builder fee rate on every order, micro-units (1_000 = 0.1%).
    pub builder_fee_rate_u: u32,
    /// `HEDGE_ROLL_TIMEOUT_MS`: how long a post-only roll rests before the IOC (20 s).
    pub roll_timeout: Duration,
    /// `HEDGE_RECONCILE_MS`: `held` reconciliation and rebalance sweep period (30 s).
    pub reconcile_every: Duration,
    /// `HEDGE_POST_ONLY_ROLLS=0` sends rolls as IOC at once (the plan's cut rule).
    pub post_only_rolls: bool,
    /// Page size of the `/fills` replay.
    pub fills_page: u32,
    /// The `book` stream, pre-hedge and drift rebalance (4.C, `HEDGE_BOOK_*`, `HEDGE_DRIFT_*`).
    pub drift: DriftConfig,
}

impl Default for HedgeConfig {
    fn default() -> Self {
        HedgeConfig {
            params: pricing::Params::default(),
            builder_fee_rate_u: BUILDER_FEE_RATE_U,
            roll_timeout: Duration::from_secs(20),
            reconcile_every: Duration::from_secs(30),
            post_only_rolls: true,
            fills_page: 100,
            drift: DriftConfig::default(),
        }
    }
}

impl HedgeConfig {
    /// From the process environment.
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// From `get` (tests pass a map). Unparsable values keep the default, with a warning.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let mut c = HedgeConfig::default();
        let ms = |k: &str| {
            get(k).and_then(|v| {
                let v = v.trim().to_owned();
                if v.is_empty() {
                    return None;
                }
                v.parse::<u64>()
                    .map_err(
                        |_| tracing::warn!(var = k, value = %v, "not a number of ms; default kept"),
                    )
                    .ok()
            })
        };
        if let Some(v) = ms("HEDGE_ROLL_TIMEOUT_MS") {
            c.roll_timeout = Duration::from_millis(v);
        }
        if let Some(v) = ms("HEDGE_RECONCILE_MS") {
            c.reconcile_every = Duration::from_millis(v.max(1_000));
        }
        if let Some(v) = get("HEDGE_POST_ONLY_ROLLS") {
            c.post_only_rolls = !matches!(v.trim(), "0" | "false" | "no");
        }
        c.drift = DriftConfig::from_lookup(&get);
        c
    }
}

/// The hedge desk. Cheap to clone.
#[derive(Clone)]
pub struct Hedger(Arc<Inner>);

impl Hedger {
    /// Rebuilds a handle from the weak reference in [`AppState::hedge`] (for read-only views
    /// outside this module, e.g. `/house`).
    pub fn from_inner(inner: Arc<Inner>) -> Self {
        Hedger(inner)
    }
}

impl std::ops::Deref for Hedger {
    type Target = Inner;

    fn deref(&self) -> &Inner {
        &self.0
    }
}

/// The hedge desk's state beyond [`AppState`].
pub struct Inner {
    /// Shared desk state.
    pub state: AppState,
    /// Settings.
    pub cfg: HedgeConfig,
    /// The hedger account; `None` when its keys are not configured (the desk then hedges
    /// nothing and every pending expires and is refunded).
    pub trader: Option<Trader>,
    locks: Mutex<HashMap<Symbol, Arc<tokio::sync::Mutex<()>>>>,
    /// Signed open contracts of resting post-only rolls per symbol.
    resting: Locked<HashMap<Symbol, i64>>,
    /// Symbols Pascal resolved (positions settled, no more orders).
    resolved: Locked<HashSet<Symbol>>,
    /// `r̃` of resolved ticket legs.
    rtilde: Locked<RTilde>,
    /// Net allocated contracts per ticket leg, with its symbol (`fill_allocations` summed).
    alloc: Locked<HashMap<(TicketId, u8), (Symbol, i64)>>,
    /// Last known free collateral of the hedger, micro-USD (`i64::MIN` = unknown).
    free_collateral_u: AtomicI64,
    /// Ticket legs the pre-hedge trigger added to the hedged set (`hedge_prehedges`, 4.C).
    prehedged: Locked<Prehedged>,
    /// The live `book` view (4.C).
    live: books::LiveView,
    /// Start-up recovery done: the book task may act.
    recovered: AtomicBool,
    /// Counters of the book task.
    drift_stats: drift::DriftStats,
}

/// Adds this service's routes: on `dev` and `demo` only, the read-only `GET /debug/hedge`
/// (the lifecycle script's hedge checks, plan 3.A). The panic button is the `desk-flatten`
/// binary, not a route (D-073).
pub fn mount(router: Router<AppState>, state: &AppState) -> Router<AppState> {
    if state.config.profile == crate::config::Profile::Live {
        return router;
    }
    router.route("/debug/hedge", axum::routing::get(debug_hedge))
}

/// `GET /debug/hedge`: per symbol, signed contracts (+ long) as the running hedge desk knows
/// them now: `held`, the net target `Tᵢ` it trades to, and resting post-only contracts.
/// 503 when no hedge desk runs in this process.
async fn debug_hedge(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<axum::Json<serde_json::Value>, crate::api::ApiError> {
    let h = state
        .hedge
        .get()
        .and_then(std::sync::Weak::upgrade)
        .map(Hedger)
        .ok_or_else(|| crate::api::ApiError::unavailable("hedge desk not running"))?;
    Ok(axum::Json(h.debug_view()))
}

/// Runs the hedge desk until shutdown, with [`HedgeConfig::from_env`] and the hedger keys of
/// the desk configuration.
pub async fn run(state: AppState) {
    let rx = state.subscribe();
    let h = Hedger::new(state.clone(), HedgeConfig::from_env());
    let _ = state.hedge.set(Arc::downgrade(&h.0));
    let drift = h.spawn_drift();
    h.run_loop(rx).await;
    if let Some(d) = drift {
        let _ = d.await;
    }
}

impl Hedger {
    /// A hedge desk trading with the configured hedger keys (or none when they are missing).
    pub fn new(state: AppState, cfg: HedgeConfig) -> Self {
        let trader = match Trader::from_config(&state.config, cfg.builder_fee_rate_u) {
            Ok(t) => {
                tracing::info!(trader = ?t, "hedge desk armed");
                Some(t)
            }
            Err(e) => {
                tracing::error!(error = %e, "hedge desk has no hedger keys: nothing will be hedged");
                None
            }
        };
        Self::with_trader(state, cfg, trader)
    }

    /// A hedge desk with an explicit trader (tests).
    pub fn with_trader(state: AppState, cfg: HedgeConfig, trader: Option<Trader>) -> Self {
        Hedger(Arc::new(Inner {
            state,
            cfg,
            trader,
            locks: Mutex::new(HashMap::new()),
            resting: Locked::default(),
            resolved: Locked::default(),
            rtilde: Locked::default(),
            alloc: Locked::default(),
            free_collateral_u: AtomicI64::new(i64::MIN),
            prehedged: Locked::default(),
            live: books::LiveView::default(),
            recovered: AtomicBool::new(false),
            drift_stats: drift::DriftStats::default(),
        }))
    }

    /// Subscribes to the bus now (so no event emitted after this call is missed) and runs the
    /// desk in a task.
    pub fn spawn(self) -> JoinHandle<()> {
        let rx = self.state.subscribe();
        tokio::spawn(self.run_loop(rx))
    }

    /// Recovery, then events and the reconcile timer until shutdown.
    pub async fn run_loop(self, mut rx: broadcast::Receiver<DeskEvent>) {
        let state = self.state.clone();
        tokio::select! {
            _ = state.shutdown_requested() => return,
            _ = self.recover() => {}
        }
        let every = self.cfg.reconcile_every;
        let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = state.shutdown_requested() => break,
                _ = tick.tick() => tokio::select! {
                    _ = state.shutdown_requested() => break,
                    _ = self.reconcile() => {}
                },
                r = recv(&mut rx, "hedge") => match r {
                    Recv::Event(e) => tokio::select! {
                        _ = state.shutdown_requested() => break,
                        _ = self.on_event(e) => {}
                    },
                    Recv::Lagged(_) => tokio::select! {
                        _ = state.shutdown_requested() => break,
                        _ = self.resync() => {}
                    },
                    Recv::Closed => break,
                },
            }
        }
        tracing::info!("hedge desk stopped");
    }

    /// Handles one bus event (see the module table).
    pub async fn on_event(&self, e: DeskEvent) {
        match e {
            DeskEvent::PendingOpened(p) => self.on_pending(&p).await,
            DeskEvent::FinalizeRejected {
                pending,
                user,
                error,
            } => self.on_finalize_rejected(&pending, &user, &error).await,
            DeskEvent::TicketMinted { ticket, .. } => self.on_minted(&ticket).await,
            DeskEvent::LegResolved {
                ticket,
                leg_idx,
                symbol,
                resolution_u,
                status,
                ..
            } => {
                self.on_leg_resolved(&ticket, leg_idx, &symbol, resolution_u, status)
                    .await
            }
            DeskEvent::MarketResolved { symbol, .. } => self.mark_resolved(&symbol).await,
            DeskEvent::TicketCashedOut { ticket, .. } => {
                self.close_ticket(&ticket, "cash_out", None, true).await;
            }
            DeskEvent::TicketSettled { ticket, .. } => {
                self.close_ticket(&ticket, "settled", None, false).await;
            }
            DeskEvent::PendingRefunded {
                pending, ticket, ..
            } => {
                self.close_ticket(&ticket, "refunded", Some(&pending), false)
                    .await;
            }
            _ => {}
        }
    }

    // --- small accessors -----------------------------------------------------------------------

    fn symbol_lock(&self, symbol: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut m = self.locks.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(symbol.to_owned()).or_default().clone()
    }

    /// Whether Pascal resolved `symbol`.
    pub fn is_resolved(&self, symbol: &str) -> bool {
        self.resolved.read().contains(symbol)
    }

    /// The hedger's net position on `symbol` as the desk knows it.
    pub fn held(&self, symbol: &str) -> i64 {
        self.state.held.read().get(symbol).copied().unwrap_or(0)
    }

    fn resting(&self, symbol: &str) -> i64 {
        self.resting.read().get(symbol).copied().unwrap_or(0)
    }

    fn add_held(&self, symbol: &str, delta: i64) {
        let now = {
            let mut h = self.state.held.write();
            let e = h.entry(symbol.to_owned()).or_insert(0);
            *e += delta;
            *e
        };
        metrics::gauge!("leg_held_contracts", "symbol" => symbol.to_owned()).set(now as f64);
    }

    fn set_free_collateral(&self, u: i64) {
        self.free_collateral_u.store(u, Ordering::Relaxed);
        metrics::gauge!("hedger_free_collateral_usd").set(u as f64 / 1e6);
    }

    fn free_collateral(&self) -> Option<i64> {
        let v = self.free_collateral_u.load(Ordering::Relaxed);
        (v != i64::MIN).then_some(v)
    }

    fn rtilde_snapshot(&self) -> RTilde {
        self.rtilde.read().clone()
    }

    fn prehedged_snapshot(&self) -> Prehedged {
        self.prehedged.read().clone()
    }

    /// The net target `Tᵢ` of `symbol` now.
    pub fn target(&self, symbol: &str) -> i64 {
        let rt = self.rtilde_snapshot();
        let pre = self.prehedged_snapshot();
        let tickets = self.state.tickets.read();
        targets::symbol_target_with(&tickets, symbol, &rt, &pre, self.cfg.params.game_length_ms).0
    }

    /// Recomputes the formula targets of one ticket (open tickets only).
    fn refresh_ticket(&self, ticket: &str) {
        let rt = self.rtilde_snapshot();
        let pre = self.prehedged_snapshot();
        let mut tickets = self.state.tickets.write();
        if let Some(t) = tickets.get_mut(ticket) {
            targets::refresh_with(t, &rt, &pre, self.cfg.params.game_length_ms);
        }
    }

    /// Writes a `hedge_positions` row and the target/held gauges for `symbol`.
    async fn publish(&self, symbol: &str, reason: &str) {
        let target = self.target(symbol);
        let held = self.held(symbol);
        metrics::gauge!("leg_target_contracts", "symbol" => symbol.to_owned()).set(target as f64);
        metrics::gauge!("leg_held_contracts", "symbol" => symbol.to_owned()).set(held as f64);
        if let Err(e) = store::insert_position(
            &self.state.db,
            symbol,
            crate::now_ms(),
            target,
            held,
            reason,
        )
        .await
        {
            tracing::error!(error = %e, %symbol, "hedge_positions write failed");
        }
    }

    // --- lifecycle events ------------------------------------------------------------------------

    /// Loads `ticket` from the DB into `tickets` when the desk does not know it yet.
    async fn ensure_ticket(&self, ticket: &str) -> bool {
        if self.state.tickets.read().contains_key(ticket) {
            return true;
        }
        match store::load_tickets(&self.state.db, Some(ticket)).await {
            Ok(mut v) if !v.is_empty() => {
                let st = v.remove(0);
                self.install(st);
                true
            }
            Ok(_) => false,
            Err(e) => {
                tracing::error!(error = %e, %ticket, "ticket load failed");
                false
            }
        }
    }

    /// Puts a stored ticket into `tickets` (pending: targets from its allocations; open: the
    /// formula).
    fn install(&self, st: store::StoredTicket) {
        let mut t = st.ticket;
        {
            let mut rt = self.rtilde.write();
            for (idx, r) in st.rtilde {
                rt.insert((t.ticket.clone(), idx), r);
            }
        }
        if t.status == TicketStatus::Pending {
            let alloc = self.alloc.read();
            for l in &mut t.legs {
                l.target = alloc.get(&(t.ticket.clone(), l.idx)).map_or(0, |(_, c)| *c);
            }
        } else {
            let rt = self.rtilde_snapshot();
            let pre = self.prehedged_snapshot();
            targets::refresh_with(&mut t, &rt, &pre, self.cfg.params.game_length_ms);
        }
        self.state.tickets.write().insert(t.ticket.clone(), t);
    }

    async fn on_minted(&self, ticket: &str) {
        // The indexer wrote the final row (payout, rS, fills) before emitting.
        let row = store::load_tickets(&self.state.db, Some(ticket))
            .await
            .unwrap_or_else(|e| {
                tracing::error!(error = %e, %ticket, "ticket load failed");
                Vec::new()
            })
            .pop();
        let known = {
            let mut tickets = self.state.tickets.write();
            match tickets.get_mut(ticket) {
                Some(t) => {
                    t.status = TicketStatus::Open;
                    if let Some(r) = row
                        .as_ref()
                        .filter(|r| r.ticket.status == TicketStatus::Open)
                    {
                        t.payout_u = r.ticket.payout_u;
                        t.rs_u = r.ticket.rs_u.or(t.rs_u);
                        for (l, rl) in t.legs.iter_mut().zip(&r.ticket.legs) {
                            if rl.p_fill_u.is_some() {
                                l.p_fill_u = rl.p_fill_u;
                            }
                        }
                    }
                    true
                }
                None => false,
            }
        };
        if !known {
            match row {
                Some(mut r) => {
                    r.ticket.status = TicketStatus::Open;
                    self.install(r);
                }
                None => {
                    tracing::warn!(%ticket, "minted ticket unknown to the desk and the DB");
                    return;
                }
            }
        }
        self.refresh_ticket(ticket);
        tracing::info!(%ticket, "ticket minted: targets registered");
        for s in self.ticket_symbols(ticket) {
            self.trade_symbol(&s, How::Sweep, "minted", None).await;
            self.publish(&s, "minted").await;
        }
    }

    /// Symbols of a ticket's legs plus any symbol it still has allocated contracts on.
    fn ticket_symbols(&self, ticket: &str) -> Vec<Symbol> {
        let mut v: Vec<Symbol> = self
            .state
            .tickets
            .read()
            .get(ticket)
            .map(|t| t.legs.iter().map(|l| l.symbol.clone()).collect())
            .unwrap_or_default();
        v.extend(
            self.alloc
                .read()
                .iter()
                .filter(|((t, _), (_, c))| t == ticket && *c != 0)
                .map(|(_, (s, _))| s.clone()),
        );
        v.sort();
        v.dedup();
        v
    }

    fn has_exposure(&self, ticket: &str) -> bool {
        let targets = self
            .state
            .tickets
            .read()
            .get(ticket)
            .is_some_and(|t| t.legs.iter().any(|l| l.target != 0));
        if targets {
            return true;
        }
        let resolved = self.resolved.read().clone();
        self.alloc
            .read()
            .iter()
            .any(|((t, _), (s, c))| t == ticket && *c != 0 && !resolved.contains(s))
    }

    /// Pascal resolved `symbol`: its position is settled, so `held` is 0 there, allocations on
    /// it are spent and no order can go to it. No roll or unwind here: that is `LegResolved`'s.
    pub async fn mark_resolved(&self, symbol: &str) {
        if !self.resolved.write().insert(symbol.to_owned()) {
            return;
        }
        self.state.held.write().insert(symbol.to_owned(), 0);
        self.resting.write().remove(symbol);
        self.alloc.write().retain(|_, (s, _)| s != symbol);
        tracing::info!(%symbol, "symbol resolved: position settled by Pascal");
        self.publish(symbol, "resolved").await;
    }

    async fn on_leg_resolved(
        &self,
        ticket: &str,
        leg_idx: u8,
        symbol: &str,
        resolution_u: u64,
        status: LegStatus,
    ) {
        self.mark_resolved(symbol).await;
        if !self.ensure_ticket(ticket).await {
            tracing::warn!(%ticket, leg_idx, "leg resolved on a ticket the desk does not know");
            return;
        }
        let before: Vec<(Symbol, i64)> = {
            let mut tickets = self.state.tickets.write();
            let Some(t) = tickets.get_mut(ticket) else {
                return;
            };
            let Some(leg) = t.legs.iter_mut().find(|l| l.idx == leg_idx) else {
                tracing::warn!(%ticket, leg_idx, "no such leg");
                return;
            };
            let rt = pricing::r_tilde(targets::pside(leg.side), resolution_u);
            leg.status = if status == LegStatus::Open {
                targets::rtilde_status(rt)
            } else {
                status
            };
            self.rtilde.write().insert((ticket.to_owned(), leg_idx), rt);
            t.legs
                .iter()
                .map(|l| (l.symbol.clone(), l.target))
                .collect()
        };
        let had = self.has_exposure(ticket);
        self.refresh_ticket(ticket);
        let after: Vec<i64> = self
            .state
            .tickets
            .read()
            .get(ticket)
            .map(|t| t.legs.iter().map(|l| l.target).collect())
            .unwrap_or_default();
        let lost = self
            .rtilde
            .read()
            .get(&(ticket.to_owned(), leg_idx))
            .is_some_and(|&r| r == 0);
        let reason = if lost { "lost_leg" } else { "roll" };
        tracing::info!(%ticket, leg_idx, %symbol, resolution_u, reason, "leg resolved");
        let mut fills = Vec::new();
        for ((sym, old), new) in before.iter().zip(&after) {
            if sym == symbol || self.is_resolved(sym) {
                continue;
            }
            let how = if !lost && new.unsigned_abs() > old.unsigned_abs() {
                How::Roll
            } else {
                How::Ioc
            };
            fills.extend(self.trade_symbol(sym, how, reason, None).await);
        }
        if lost && (had || !fills.is_empty()) {
            self.emit_unwound(ticket, None, reason, fills).await;
        }
    }

    /// Drops a ticket's targets, unwinds whatever it still holds and forgets it.
    async fn close_ticket(
        &self,
        ticket: &str,
        reason: &str,
        pending: Option<&str>,
        always_emit: bool,
    ) -> Vec<TicketFill> {
        let had = self.has_exposure(ticket);
        let symbols = self.ticket_symbols(ticket);
        {
            let mut tickets = self.state.tickets.write();
            if let Some(t) = tickets.get_mut(ticket) {
                t.status = match reason {
                    "cash_out" => TicketStatus::CashedOut,
                    "settled" => TicketStatus::Settled,
                    _ => TicketStatus::Refunded,
                };
                for l in &mut t.legs {
                    l.target = 0;
                }
            }
        }
        let mut fills = Vec::new();
        for s in &symbols {
            fills.extend(self.trade_symbol(s, How::Ioc, reason, pending).await);
        }
        let user = self.state.tickets.write().remove(ticket).map(|t| t.user);
        self.prehedged.write().remove(ticket);
        if always_emit || had || !fills.is_empty() {
            let user = match user {
                Some(u) => u,
                None => self.user_of(ticket).await,
            };
            self.emit_unwound_as(ticket, &user, pending, reason, fills.clone());
        }
        fills
    }

    async fn user_of(&self, ticket: &str) -> String {
        sqlx::query_scalar("SELECT user FROM tickets WHERE ticket = ?")
            .bind(ticket)
            .fetch_optional(&self.state.db)
            .await
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    async fn emit_unwound(
        &self,
        ticket: &str,
        pending: Option<&str>,
        reason: &str,
        fills: Vec<TicketFill>,
    ) {
        let user = self
            .state
            .tickets
            .read()
            .get(ticket)
            .map(|t| t.user.clone());
        let user = match user {
            Some(u) => u,
            None => self.user_of(ticket).await,
        };
        self.emit_unwound_as(ticket, &user, pending, reason, fills);
    }

    fn emit_unwound_as(
        &self,
        ticket: &str,
        user: &str,
        pending: Option<&str>,
        reason: &str,
        fills: Vec<TicketFill>,
    ) {
        tracing::info!(%ticket, ?pending, reason, fills = fills.len(), "hedge unwound");
        self.state
            .emit(DeskEvent::HedgeUnwound(Arc::new(HedgeUnwound {
                pending: pending.map(str::to_owned),
                ticket: ticket.to_owned(),
                user: user.to_owned(),
                reason: reason.to_owned(),
                fills,
            })));
    }

    async fn on_finalize_rejected(&self, pending: &str, user: &str, error: &str) {
        let ticket = self
            .state
            .tickets
            .read()
            .values()
            .find(|t| t.pending == pending)
            .map(|t| t.ticket.clone());
        let ticket = match ticket {
            Some(t) => Some(t),
            None => sqlx::query_scalar("SELECT ticket FROM tickets WHERE pending = ?")
                .bind(pending)
                .fetch_optional(&self.state.db)
                .await
                .ok()
                .flatten(),
        };
        tracing::warn!(%pending, ?ticket, %error, "finalize rejected: unwinding");
        metrics::counter!("quote_floor_breaches_total").increment(1);
        let ticket = ticket.unwrap_or_default();
        self.close_ticket(&ticket, "finalize_rejected", Some(pending), true)
            .await;
        self.state.emit(DeskEvent::RefundNeeded {
            pending: pending.to_owned(),
            user: user.to_owned(),
            reason: format!("finalize rejected: {error}"),
        });
    }

    /// `held`, `Tᵢ` and resting contracts per symbol (`GET /debug/hedge`).
    pub fn debug_view(&self) -> serde_json::Value {
        let mut symbols: std::collections::BTreeSet<Symbol> =
            targets::symbols_of(&self.state.tickets.read())
                .into_iter()
                .collect();
        symbols.extend(self.state.held.read().keys().cloned());
        symbols.extend(self.resting.read().keys().cloned());
        let mut held = serde_json::Map::new();
        let mut target = serde_json::Map::new();
        let mut resting = serde_json::Map::new();
        for s in symbols {
            held.insert(s.clone(), self.held(&s).into());
            target.insert(s.clone(), self.target(&s).into());
            let r = self.resting(&s);
            resting.insert(s, r.into());
        }
        let prehedged: Vec<serde_json::Value> = self
            .prehedged_legs()
            .into_iter()
            .map(|(t, i)| serde_json::json!({ "ticket": t, "leg": i }))
            .collect();
        let mut books = serde_json::Map::new();
        for s in self.live.subscribed() {
            let b = self.live.live(&s);
            let best = |v: Option<&pascal_rs::Level>| v.map(|l| l.price);
            books.insert(
                s,
                serde_json::json!({
                    "live": b.is_some(),
                    "bid_u": b.as_ref().and_then(|b| best(b.book.bids.first())),
                    "ask_u": b.as_ref().and_then(|b| best(b.book.asks.first())),
                    "recv_ms": b.as_ref().map(|b| b.recv_ms),
                }),
            );
        }
        let st = self.live.status();
        serde_json::json!({
            "held": held,
            "target": target,
            "resting": resting,
            "prehedged": prehedged,
            "books": books,
            "stream": {
                "enabled": self.cfg.drift.enabled,
                "connected": st.connected,
                "connections": st.epoch,
                "subscribed": st.subscribed,
                "live": st.live,
                "refused": st.refused,
                "counts": self.drift_counts(),
            },
        })
    }

    /// The tickets the desk holds now (tests and diagnostics).
    pub fn tickets(&self) -> Vec<TicketState> {
        self.state.tickets.read().values().cloned().collect()
    }

    /// Net allocated contracts per ticket leg (tests and diagnostics).
    pub fn allocations(&self) -> HashMap<(TicketId, u8), (Symbol, i64)> {
        self.alloc.read().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_from_env_keeps_defaults() {
        let c = HedgeConfig::from_lookup(|k| match k {
            "HEDGE_ROLL_TIMEOUT_MS" => Some("250".into()),
            "HEDGE_RECONCILE_MS" => Some("x".into()),
            "HEDGE_POST_ONLY_ROLLS" => Some("0".into()),
            _ => None,
        });
        assert_eq!(c.roll_timeout, Duration::from_millis(250));
        assert_eq!(c.reconcile_every, Duration::from_secs(30));
        assert!(!c.post_only_rolls);
        assert_eq!(c.builder_fee_rate_u, 1_000);
    }
}
