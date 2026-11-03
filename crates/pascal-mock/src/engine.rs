//! The mock exchange: replayed books at the clock, the mock's own resting orders, a matcher,
//! accounts, and resolutions.
//!
//! Everything lives behind one lock ([`crate::Mock`] wraps the engine in a `Mutex`). Every
//! request first calls [`Engine::advance`], which brings the state to the clock: due scripted
//! resolutions (and resolutions that appear on the tape) settle positions, GTT orders expire,
//! and on a new books snapshot the consumed-liquidity ledger resets and resting orders that the
//! new tape book crosses fill as makers.
//!
//! Matching (an incoming order against the tape book at the clock plus the mock's resting
//! orders): price priority, then time priority, with tape liquidity ahead of mock orders at the
//! same price (it was resting first). Fills happen at the resting price. Liquidity a fill takes
//! from the tape stays consumed until the next snapshot, so repeated IOCs cannot fill the same
//! level twice. Self-trades cancel the resting order (`SELF_TRADE`) and matching continues.
//!
//! Accounting follows Pascal's collateral rule: going long `q` at `p` locks `q × p`, going short
//! locks `q × (1 − p)`; reducing releases at the fill price; resolution at `r` pays `q × r` to a
//! long and `q × (1 − r)` to a short. Taker fee `size × p × (1 − p) × taker_fee_rate`, maker
//! rebate `… × taker_fee_rate × maker_rebate_share`, builder fee `… × builder_fee_rate`, each
//! rounded toward zero, all debited from the fill's `collateral_change_usd`; builder fees are
//! credited to the builder's free collateral.

use std::borrow::Cow;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pascal_rs::num::decimal_to_micros;
use pascal_rs::{
    AccountLifetimeStats, AccountRewards, AccountState, ApiErrorCode, Books, BuilderStats, Fill,
    Level, LifetimeStats, Liquidity, Market, MarketResolution, MarketUpdateType, MicroUsd, Order,
    OrderBook, OrderUpdate, OrderUpdateReason, Position, PositionResolution, PublicTrade, Side,
    TimeInForce,
};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::clock::MockClock;
use crate::events::MockEvent;
use crate::feed::{FeedCounts, LiveBook, LiveFeed, LiveMarket};
use crate::math::{MICRO, clamp_i64, fee_u, notional_u, rate_u, rebate_u, valid_price, weighted_u};
use crate::requests::{Builder, CancelOrder, CancelTarget, MassCancel, PlaceOrder, Reject};
use crate::script::{At, Script};
use crate::tape::Tape;
use crate::util::strip_nulls;

/// Owner reported as the counterparty of tape liquidity (`pascal-mock:tape-liquidity:00001`).
pub const TAPE_OWNER_BYTES: [u8; 32] = *b"pascal-mock:tape-liquidity:00001";
/// Pascal's resting-order limit per account and market.
pub const MAX_RESTING_PER_MARKET: usize = 50;
/// Minimum free collateral of a builder named on a new order (USD 100).
pub const BUILDER_MIN_COLLATERAL_U: i64 = 100 * MICRO;
const TRADE_ID_BASE: u64 = 9_000_000_000;
const ORDER_ID_BASE: u64 = 900_000_000_000;

/// Base58 of [`TAPE_OWNER_BYTES`].
pub fn tape_owner() -> String {
    bs58::encode(TAPE_OWNER_BYTES).into_string()
}

/// Engine settings.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Tape time the clock starts at, Unix ms.
    pub start_ms: u64,
    /// Tape ms per wall ms (0 freezes the clock).
    pub speed: f64,
    /// Free collateral of every new account, micro-USD.
    pub initial_collateral_u: MicroUsd,
    /// Scripted resolutions.
    pub script: Script,
}

/// Freshness of a response: envelope `seq` and `state_time_ms` / `server_time_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Envelope {
    /// Highest seq of the state served.
    pub seq: u64,
    /// Mock clock, Unix ms.
    pub time_ms: u64,
}

/// `data` of one successful `POST /api/v1/orders` element.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlaceResult {
    /// Order state after placement.
    pub order: Order,
    /// Fills during placement.
    pub fills: Vec<Fill>,
    /// Why the order closed or changed during placement, if it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_reason: Option<OrderUpdateReason>,
}

/// Clock state reported by the control endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ClockInfo {
    /// Mock clock, Unix ms.
    pub tape_ms: u64,
    /// Speed.
    pub speed: f64,
    /// Clock start.
    pub start_ms: u64,
    /// Time of the books snapshot in force, if any.
    pub book_snapshot_ms: Option<u64>,
    /// First and last books snapshot of the tape.
    pub tape_first_ms: u64,
    /// Last books snapshot.
    pub tape_last_ms: u64,
}

#[derive(Debug, Clone)]
struct OpenOrder {
    owner: String,
    order: Order,
    builder: Option<Builder>,
    /// Time priority (lower first).
    priority: u64,
}

#[derive(Debug, Clone, Default)]
struct PositionState {
    size: i64,
    /// Σ entry price × contracts of the open contracts (full-precision entry basis).
    entry_notional_u: i64,
    cum_inc_contracts: u64,
    cum_inc_notional_u: i64,
    cum_red_contracts: u64,
    cum_red_notional_u: i64,
    cum_fees_u: i64,
    realized_u: i64,
    seq: u64,
    update_seq: u64,
    open_ts: u64,
    update_ts: u64,
}

#[derive(Debug, Clone, Copy, Default)]
struct Lifetime {
    contracts: u64,
    weighted_u: i64,
    notional_u: i64,
    fees_u: i64,
}

impl Lifetime {
    fn of(m: &LifetimeStats) -> Lifetime {
        Lifetime {
            contracts: m.volume_contracts,
            weighted_u: m
                .weighted_volume
                .and_then(|w| decimal_to_micros(w).ok())
                .unwrap_or(0),
            notional_u: m.volume_notional_usd,
            fees_u: m.fees_paid_usd,
        }
    }

    fn msg(&self) -> LifetimeStats {
        LifetimeStats {
            volume_contracts: self.contracts,
            weighted_volume: Some(Decimal::new(self.weighted_u, 6)),
            volume_notional_usd: self.notional_u,
            fees_paid_usd: self.fees_u,
        }
    }
}

/// Fills attributed to an account as builder (`/builder-stats`).
#[derive(Debug, Clone, Copy, Default)]
struct BuilderTally {
    contracts: u128,
    notional_u: i64,
    fee_base_u: i64,
}

#[derive(Debug, Clone)]
struct Account {
    collateral_u: i64,
    positions: BTreeMap<String, PositionState>,
    fills: Vec<Fill>,
    resolutions: Vec<PositionResolution>,
    builder_fees_received_u: i64,
    builder: BuilderTally,
    maker: Lifetime,
    taker: Lifetime,
}

impl Account {
    fn new(collateral_u: i64) -> Self {
        Account {
            collateral_u,
            positions: BTreeMap::new(),
            fills: Vec::new(),
            resolutions: Vec::new(),
            builder_fees_received_u: 0,
            builder: BuilderTally::default(),
            maker: Lifetime::default(),
            taker: Lifetime::default(),
        }
    }

    fn rewards(&self) -> AccountRewards {
        AccountRewards {
            referral_rewards_received_usd: 0,
            builder_fees_received_usd: self.builder_fees_received_u,
            user_rewards_received_usd: 0,
        }
    }

    fn lifetime(&self) -> AccountLifetimeStats {
        AccountLifetimeStats {
            maker: self.maker.msg(),
            taker: self.taker.msg(),
        }
    }

    fn position_size(&self, symbol: &str) -> i64 {
        self.positions.get(symbol).map_or(0, |p| p.size)
    }
}

/// Market rules from `/markets`.
#[derive(Debug, Clone, Copy)]
struct Spec {
    taker_rate_u: i64,
    rebate_share_u: i64,
    tick_min_u: i64,
    tick_sig_figs: u8,
}

impl Spec {
    fn of(m: &Market) -> Spec {
        Spec {
            taker_rate_u: rate_u(m.taker_fee_rate).unwrap_or(0),
            rebate_share_u: rate_u(m.maker_rebate_share).unwrap_or(0),
            tick_min_u: m.tick_size_min,
            tick_sig_figs: m.tick_sig_figs,
        }
    }

    fn rebate(&self, size: u64, price_u: i64) -> i64 {
        rebate_u(size, price_u, self.taker_rate_u, self.rebate_share_u)
    }
}

/// One side of a fill to book.
struct FillArgs<'a> {
    owner: &'a str,
    symbol: &'a str,
    side: Side,
    price_u: i64,
    size: u64,
    liquidity: Liquidity,
    counterparty: &'a str,
    order_id: u64,
    client_order_id: u64,
    reduce_only: bool,
    builder: Option<&'a Builder>,
    spec: Spec,
    trade_id: u64,
    seq: u64,
}

/// One owner's changes since the last [`Engine::flush`].
#[derive(Debug, Default)]
struct Delta {
    fills: Vec<Fill>,
    orders: Vec<OrderUpdate>,
    resolutions: Vec<PositionResolution>,
    symbols: BTreeSet<String>,
    rewards: bool,
}

/// A source of liquidity met by an incoming order.
#[derive(Debug, Clone, Copy)]
enum Maker {
    Tape { price_u: i64, available: u64 },
    Mock { price_u: i64, id: u64 },
}

/// The mock exchange state.
pub struct Engine {
    tape: Arc<Tape>,
    clock: MockClock,
    start_ms: u64,
    now_ms: u64,
    seq: u64,
    tape_seq: u64,
    next_trade_id: u64,
    next_order_id: u64,
    book_idx: Option<usize>,
    /// Tape liquidity taken in the current snapshot: (symbol, side of the level, price) → size.
    consumed: HashMap<(String, Side, i64), u64>,
    orders: BTreeMap<u64, OpenOrder>,
    accounts: BTreeMap<String, Account>,
    resolutions: BTreeMap<String, MarketResolution>,
    /// Scheduled resolutions: (tape ms, symbol, price).
    pending: Vec<(u64, String, i64)>,
    /// Wall instant the engine was created (`wall+<dur>` times count from here, D-090).
    boot: Instant,
    /// Scheduled resolutions on the wall clock: (deadline, symbol, price).
    pending_wall: Vec<(Instant, String, i64)>,
    /// Per-owner changes not yet published as [`MockEvent::AccountUpdate`].
    dirty: BTreeMap<String, Delta>,
    trades: Vec<PublicTrade>,
    initial_collateral_u: i64,
    events: broadcast::Sender<MockEvent>,
    /// Live mode (L4-1): the upstream catalogue and books. `None` in replay; the tape is then
    /// the only source, and in live mode the tape is empty.
    live: Option<LiveFeed>,
}

impl Engine {
    /// A fresh engine on `tape`; the clock starts at `cfg.start_ms`.
    pub fn new(tape: Arc<Tape>, cfg: EngineConfig) -> Engine {
        let (events, _) = broadcast::channel(4096);
        let boot = Instant::now();
        let mut pending: Vec<(u64, String, i64)> = Vec::new();
        let mut pending_wall: Vec<(Instant, String, i64)> = Vec::new();
        for r in &cfg.script.resolve {
            if !tape.markets.is_empty() && !tape.markets.contains_key(&r.symbol) {
                tracing::warn!(symbol = %r.symbol, "script resolves a symbol that is not on the tape");
            }
            match r.at.tape_ms(cfg.start_ms, cfg.start_ms) {
                Some(t) => pending.push((t, r.symbol.clone(), r.price_u)),
                None => {
                    let d = match r.at {
                        At::AfterBoot(d) => d,
                        _ => 0,
                    };
                    pending_wall.push((wall_deadline(boot, d), r.symbol.clone(), r.price_u));
                }
            }
        }
        pending.sort_by_key(|p| p.0);
        pending_wall.sort_by_key(|p| p.0);
        let tape_seq = tape.seq_at(cfg.start_ms);
        Engine {
            clock: MockClock::new(cfg.start_ms, cfg.speed),
            start_ms: cfg.start_ms,
            now_ms: cfg.start_ms,
            seq: tape_seq,
            tape_seq,
            next_trade_id: TRADE_ID_BASE,
            next_order_id: ORDER_ID_BASE,
            book_idx: None,
            consumed: HashMap::new(),
            orders: BTreeMap::new(),
            accounts: BTreeMap::new(),
            resolutions: BTreeMap::new(),
            pending,
            boot,
            pending_wall,
            dirty: BTreeMap::new(),
            trades: Vec::new(),
            initial_collateral_u: cfg.initial_collateral_u,
            events,
            tape,
            live: None,
        }
    }

    /// A live-mode engine (L4-1): no tape, an empty feed the upstream tasks fill
    /// ([`crate::live`]). The clock should be the wall clock (`start_ms` = now, `speed` 1).
    pub fn new_live(cfg: EngineConfig) -> Engine {
        let mut e = Engine::new(Arc::new(Tape::empty("live")), cfg);
        e.live = Some(LiveFeed::default());
        e
    }

    /// Whether the engine runs on the live feed.
    pub fn is_live(&self) -> bool {
        self.live.is_some()
    }

    /// Subscribes to the event bus.
    pub fn subscribe(&self) -> broadcast::Receiver<MockEvent> {
        self.events.subscribe()
    }

    /// The tape.
    pub fn tape(&self) -> &Tape {
        &self.tape
    }

    /// Mock time after the last [`advance`](Self::advance).
    pub fn now_ms(&self) -> u64 {
        self.now_ms
    }

    /// Envelope fields for a response produced now.
    pub fn envelope(&self) -> Envelope {
        Envelope {
            seq: self.seq.max(self.tape_seq),
            time_ms: self.now_ms,
        }
    }

    fn next_seq(&mut self) -> u64 {
        self.seq = self.seq.max(self.tape_seq) + 1;
        self.seq
    }

    fn emit(&mut self, e: MockEvent) {
        self.record(&e);
        // No subscriber is not an error.
        let _ = self.events.send(e);
    }

    /// Notes what `e` changed on an account, for the next [`flush`](Self::flush).
    fn record(&mut self, e: &MockEvent) {
        match e {
            MockEvent::Fill { owner, fill } => {
                let d = self.dirty.entry(owner.clone()).or_default();
                d.symbols.insert(fill.symbol.clone());
                d.fills.push(fill.clone());
            }
            MockEvent::Order { owner, update } => {
                self.dirty
                    .entry(owner.clone())
                    .or_default()
                    .orders
                    .push(update.clone());
            }
            MockEvent::PositionResolved { owner, resolution } => {
                let d = self.dirty.entry(owner.clone()).or_default();
                d.symbols.insert(resolution.symbol.clone());
                d.resolutions.push(resolution.clone());
            }
            MockEvent::BuilderFee { builder, .. } => {
                self.dirty.entry(builder.clone()).or_default().rewards = true;
            }
            MockEvent::MarketResolved { .. }
            | MockEvent::AccountUpdate { .. }
            | MockEvent::MarketUpdate { .. } => {}
        }
    }

    /// Publishes the changes recorded since the last call as one [`MockEvent::AccountUpdate`]
    /// per owner. The guard of [`crate::Shared::engine`] calls this when a request releases
    /// the engine, so every update shows a consistent state; standalone users call it
    /// themselves. Nothing is built while nobody subscribes.
    pub fn flush(&mut self) {
        if self.dirty.is_empty() {
            return;
        }
        let dirty = std::mem::take(&mut self.dirty);
        if self.events.receiver_count() == 0 {
            return;
        }
        let env = self.envelope();
        for (owner, d) in dirty {
            let Some(acct) = self.accounts.get(&owner) else {
                continue;
            };
            // Last state of each changed order, in update order.
            let mut last: BTreeMap<u64, Order> = BTreeMap::new();
            for u in &d.orders {
                last.insert(u.order.id, u.order.clone());
            }
            let mut orders: Vec<Order> = last.into_values().collect();
            orders.sort_by_key(|o| (o.update_seq, o.id));
            let positions = d
                .symbols
                .iter()
                .map(|s| match acct.positions.get(s) {
                    Some(p) => self.position_msg(s, p),
                    None => self.position_msg(s, &PositionState::default()),
                })
                .collect();
            let has_fills = !d.fills.is_empty();
            let update = AccountState {
                has_deposited: None,
                username: None,
                twitter_account_connection: None,
                collateral_usd: Some(acct.collateral_u),
                rewards: d.rewards.then(|| acct.rewards()),
                lifetime_stats: has_fills.then(|| acct.lifetime()),
                orders,
                positions,
                pending_deposits: Vec::new(),
                pending_withdrawals: Vec::new(),
                order_updates: (!d.orders.is_empty()).then_some(d.orders),
                fills: has_fills.then_some(d.fills),
                resolutions: (!d.resolutions.is_empty()).then_some(d.resolutions),
                closed_positions: None,
                transfers: None,
            };
            let _ = self.events.send(MockEvent::AccountUpdate {
                owner,
                update: Box::new(update),
                seq: env.seq,
                time_ms: env.time_ms,
            });
        }
    }

    // ---------- time ----------

    /// Brings the state to the clock (see the module docs). Idempotent.
    pub fn advance(&mut self) {
        let t = self.clock.now_ms().max(self.now_ms);
        self.now_ms = t;
        self.tape_seq = self.tape_seq.max(self.tape.seq_at(t));

        // 1. Resolutions: scripted (tape time, then wall clock), then those seen on the tape.
        //    In live mode a due resolution of a market the catalogue does not list yet waits
        //    for it (the catalogue loads after boot); on a tape it is dropped.
        let live = self.live.is_some();
        while let Some(i) = self
            .pending
            .iter()
            .position(|p| p.0 <= t && (!live || self.knows(&p.1)))
        {
            let (at, symbol, price) = self.pending.remove(i);
            if self.knows(&symbol) {
                self.resolve_now(&symbol, price, at, None);
            }
        }
        let wall = Instant::now();
        while let Some(i) = self
            .pending_wall
            .iter()
            .position(|p| p.0 <= wall && (!live || self.knows(&p.1)))
        {
            let (_, symbol, price) = self.pending_wall.remove(i);
            if self.knows(&symbol) {
                self.resolve_now(&symbol, price, t, None);
            }
        }
        let observed: Vec<(String, MarketResolution)> = self
            .tape
            .symbols()
            .filter(|s| !self.resolutions.contains_key(*s))
            .filter_map(|s| {
                let (m, _) = self.tape.market_at(s, t)?;
                Some((s.to_owned(), m.resolution?))
            })
            .collect();
        for (symbol, r) in observed {
            self.resolve_now(&symbol, r.price, r.market_resolution_ts_ms, Some(r.seq));
        }

        // 2. GTT expiry.
        let expired: Vec<u64> = self
            .orders
            .iter()
            .filter(|(_, o)| o.order.tif == TimeInForce::Gtt)
            .filter(|(_, o)| o.order.expires_ts_ms.is_some_and(|e| e <= t))
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            self.close_order(id, OrderUpdateReason::Expired);
        }

        // 3. New snapshot: fresh liquidity, then resting orders the tape now crosses.
        let idx = self.tape.book_index_at(t);
        if idx != self.book_idx {
            self.book_idx = idx;
            self.consumed.clear();
            self.cross_resting_with_tape(None);
        }
    }

    /// Clock state.
    pub fn clock_info(&self) -> ClockInfo {
        ClockInfo {
            tape_ms: self.now_ms,
            speed: self.clock.speed(),
            start_ms: self.start_ms,
            book_snapshot_ms: self.book_idx.map(|i| self.tape.books[i].t_ms),
            tape_first_ms: self.tape.first_ms(),
            tape_last_ms: self.tape.last_ms(),
        }
    }

    /// Moves the clock: to `tape_ms` or by `advance_ms`, and/or to a new speed. The clock never
    /// goes backwards (state is not rewound).
    pub fn set_clock(
        &mut self,
        tape_ms: Option<u64>,
        advance_ms: Option<u64>,
        speed: Option<f64>,
    ) -> Result<(), Reject> {
        self.advance();
        if self.live.is_some() && (tape_ms.is_some() || advance_ms.is_some() || speed.is_some()) {
            return Err(Reject::new(
                ApiErrorCode::InvalidRequest,
                "live mode runs on the wall clock; /mock/clock is read-only",
            ));
        }
        let target = match (tape_ms, advance_ms) {
            (Some(_), Some(_)) => {
                return Err(Reject::new(
                    ApiErrorCode::InvalidRequest,
                    "give tape_ms or advance_ms, not both",
                ));
            }
            (Some(t), None) => Some(t),
            (None, Some(d)) => Some(self.now_ms.saturating_add(d)),
            (None, None) => None,
        };
        if let Some(t) = target {
            if t < self.now_ms {
                return Err(Reject::new(
                    ApiErrorCode::InvalidRequest,
                    format!("the mock clock cannot go back ({t} < {})", self.now_ms),
                ));
            }
            self.clock.set_time(t);
        }
        if let Some(s) = speed {
            self.clock.set_speed(s);
        }
        self.advance();
        Ok(())
    }

    // ---------- resolutions ----------

    /// Resolves `symbol` at `price_u`, now or at `at`. Returns the resolution if it happened
    /// now, `None` if scheduled. A market resolves once; a second resolution at another price
    /// is a `CONFLICT`.
    pub fn schedule_resolution(
        &mut self,
        symbol: &str,
        price_u: i64,
        at: At,
    ) -> Result<Option<MarketResolution>, Reject> {
        self.advance();
        if !self.knows(symbol) {
            return Err(Reject::new(
                ApiErrorCode::SymbolNotFound,
                format!("{symbol} is not on the tape"),
            ));
        }
        if !(0..=MICRO).contains(&price_u) {
            return Err(Reject::new(
                ApiErrorCode::InvalidRequest,
                "price must be in [0, 1]",
            ));
        }
        if let Some(r) = self.resolutions.get(symbol) {
            return if r.price == price_u {
                Ok(Some(r.clone()))
            } else {
                Err(Reject::new(
                    ApiErrorCode::Conflict,
                    format!("{symbol} already resolved at {}", r.price),
                ))
            };
        }
        let now = self.now_ms;
        let due = match (at, at.tape_ms(self.start_ms, now)) {
            (_, Some(at_ms)) if at_ms > now => {
                self.pending.push((at_ms, symbol.to_owned(), price_u));
                self.pending.sort_by_key(|p| p.0);
                false
            }
            (At::AfterBoot(d), None) => {
                let deadline = wall_deadline(self.boot, d);
                let due = deadline <= Instant::now();
                if !due {
                    self.pending_wall
                        .push((deadline, symbol.to_owned(), price_u));
                    self.pending_wall.sort_by_key(|p| p.0);
                }
                due
            }
            _ => true,
        };
        Ok(due.then(|| self.resolve_now(symbol, price_u, now, None)))
    }

    fn resolve_now(
        &mut self,
        symbol: &str,
        price_u: i64,
        ts_ms: u64,
        tape_seq: Option<u64>,
    ) -> MarketResolution {
        if let Some(r) = self.resolutions.get(symbol) {
            return r.clone();
        }
        let seq = self.next_seq();
        let resolution = MarketResolution {
            price: price_u,
            seq: tape_seq.unwrap_or(seq),
            market_resolution_ts_ms: ts_ms,
        };
        self.resolutions
            .insert(symbol.to_owned(), resolution.clone());
        tracing::info!(%symbol, price = price_u, ts_ms, "market resolved");

        let ids: Vec<u64> = self
            .orders
            .iter()
            .filter(|(_, o)| o.order.symbol == symbol)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.close_order(id, OrderUpdateReason::MarketResolved);
        }

        let mut settled = Vec::new();
        for (owner, acct) in &mut self.accounts {
            let Some(pos) = acct.positions.remove(symbol) else {
                continue;
            };
            if pos.size == 0 {
                continue;
            }
            let q = pos.size.unsigned_abs();
            let (payout, pnl) = if pos.size > 0 {
                let p = notional_u(q, price_u);
                (p, p - pos.entry_notional_u)
            } else {
                (
                    notional_u(q, MICRO - price_u),
                    pos.entry_notional_u - notional_u(q, price_u),
                )
            };
            acct.collateral_u += payout;
            let r = PositionResolution {
                symbol: symbol.to_owned(),
                resolution: price_u,
                size: pos.size,
                collateral_change_usd: payout,
                realized_pnl_usd: pnl,
                position_open_seq: Some(pos.seq),
                position_open_ts_ms: Some(pos.open_ts),
                seq,
                position_resolution_ts_ms: ts_ms,
            };
            acct.resolutions.push(r.clone());
            settled.push((owner.clone(), r));
        }
        self.emit(MockEvent::MarketResolved {
            symbol: symbol.to_owned(),
            resolution: resolution.clone(),
        });
        if let Some(mut market) = self.market_json(symbol) {
            if let Some(obj) = market.as_object_mut() {
                for key in ["stats", "best_bid", "best_ask"] {
                    obj.remove(key);
                }
            }
            let env = self.envelope();
            self.emit(MockEvent::MarketUpdate {
                update_type: MarketUpdateType::Resolved,
                market,
                seq: env.seq,
                time_ms: env.time_ms,
            });
        }
        for (owner, resolution) in settled {
            self.emit(MockEvent::PositionResolved { owner, resolution });
        }
        resolution
    }

    // ---------- market data ----------

    /// Whether the market is known: on the tape, or (live) in the catalogue or subscribed.
    pub fn knows(&self, symbol: &str) -> bool {
        match &self.live {
            Some(l) => l.markets.contains_key(symbol) || l.books.contains_key(symbol),
            None => self.tape.markets.contains_key(symbol),
        }
    }

    /// The market as observed now: the tape's latest observation, or the live catalogue entry.
    fn market_obs(&self, symbol: &str) -> Option<(Cow<'_, Market>, Cow<'_, Value>)> {
        match &self.live {
            Some(l) => l
                .markets
                .get(symbol)
                .map(|m| (Cow::Borrowed(&m.market), Cow::Borrowed(&m.raw))),
            None => self
                .tape
                .market_at(symbol, self.now_ms)
                .map(|(m, raw)| (Cow::Owned(m), Cow::Owned(raw))),
        }
    }

    /// Every market symbol of the catalogue (tape or live), ascending.
    fn catalogue_symbols(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        match &self.live {
            Some(l) => Box::new(l.markets.keys().map(String::as_str)),
            None => Box::new(self.tape.symbols()),
        }
    }

    fn spec(&self, symbol: &str) -> Option<Spec> {
        self.market_obs(symbol).map(|(m, _)| Spec::of(&m))
    }

    /// The market object as served now: the tape's latest observation (or the live catalogue
    /// entry), with `resolution`, `best_bid`, `best_ask` and `mark_price` brought to the mock's
    /// state. In live mode a market the mock holds no book for keeps the upstream's prices.
    fn market_json(&self, symbol: &str) -> Option<Value> {
        let mut raw = self.market_obs(symbol)?.1.into_owned();
        let has_book = self
            .live
            .as_ref()
            .is_none_or(|l| l.books.contains_key(symbol));
        let obj = raw.as_object_mut()?;
        match self.resolutions.get(symbol) {
            Some(r) => {
                obj.insert("resolution".into(), serde_json::to_value(r).ok()?);
                obj.insert(
                    "mark_price".into(),
                    pascal_rs::format_micros(r.price).into(),
                );
                obj.remove("best_bid");
                obj.remove("best_ask");
            }
            None if !has_book => {
                obj.remove("resolution");
            }
            None => {
                obj.remove("resolution");
                let book = self.book(symbol);
                for (key, level) in [
                    ("best_bid", book.bids.first()),
                    ("best_ask", book.asks.first()),
                ] {
                    match level {
                        Some(l) => {
                            obj.insert(key.into(), pascal_rs::format_micros(l.price).into());
                        }
                        None => {
                            obj.remove(key);
                        }
                    }
                }
                if let Some(m) = self.mark_u(symbol) {
                    obj.insert("mark_price".into(), pascal_rs::format_micros(m).into());
                }
            }
        }
        Some(raw)
    }

    /// `GET /api/v1/markets`: unresolved markets, by symbol (live: those the upstream lists).
    pub fn live_markets(&self) -> Vec<Value> {
        let listed = |s: &str| {
            self.live
                .as_ref()
                .is_none_or(|l| l.markets.get(s).is_some_and(|m| m.listed))
        };
        self.catalogue_symbols()
            .filter(|s| !self.resolutions.contains_key(*s) && listed(s))
            .filter_map(|s| self.market_json(s))
            .collect()
    }

    /// `GET /api/v1/markets/resolved`: markets resolved within `lookback_ms`, latest first.
    pub fn resolved_markets(&self, lookback_ms: u64, limit: usize) -> Vec<Value> {
        let floor = self.now_ms.saturating_sub(lookback_ms);
        let mut rs: Vec<(&String, &MarketResolution)> = self
            .resolutions
            .iter()
            .filter(|(_, r)| r.market_resolution_ts_ms >= floor)
            .collect();
        rs.sort_by_key(|(s, r)| (Reverse(r.market_resolution_ts_ms), *s));
        rs.into_iter()
            .take(limit)
            .filter_map(|(s, _)| self.market_json(s))
            .collect()
    }

    /// `POST /api/v1/markets/search`: union of exact symbol, event and game matches, by symbol,
    /// resolved markets included.
    pub fn search(&self, symbols: &[String], events: &[String], games: &[String]) -> Vec<Value> {
        self.catalogue_symbols()
            .filter(|s| {
                symbols.iter().any(|x| x == s)
                    || events
                        .iter()
                        .any(|e| s.split_once('.').is_some_and(|(ev, _)| ev == e))
                    || (!games.is_empty()
                        && self.market_obs(s).is_some_and(|(m, _)| {
                            m.display_attributes
                                .game_page
                                .as_ref()
                                .is_some_and(|g| games.contains(&g.game_id))
                        }))
            })
            .filter_map(|s| self.market_json(s))
            .collect()
    }

    /// `POST /api/v1/markets/text-search`: every query token (case-insensitive) must occur in
    /// the market's text (symbol, descriptions, game, outcome labels, teams, topic, tags); every
    /// requested tag must be among the market's tags. `status`: `live`, `resolved` or `all`.
    pub fn text_search(
        &self,
        query: &str,
        tags: &[String],
        status: &str,
        limit: usize,
    ) -> Vec<Value> {
        let tokens: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .map(str::to_lowercase)
            .collect();
        let mut out = Vec::new();
        for s in self.catalogue_symbols() {
            let resolved = self.resolutions.contains_key(s);
            if (status == "live" && resolved) || (status == "resolved" && !resolved) {
                continue;
            }
            let Some((m, _)) = self.market_obs(s) else {
                continue;
            };
            let d = &m.display_attributes;
            if !tags.iter().all(|t| d.tags.contains(t)) {
                continue;
            }
            let mut text = vec![
                m.symbol.clone(),
                d.event_description.clone(),
                d.market_description.clone(),
            ];
            text.extend(d.reverse_description.clone());
            text.extend(d.tags.iter().cloned());
            if let Some(g) = &d.game_page {
                text.push(g.game_description.clone());
                text.push(g.section_description.clone());
            }
            if let Some(t) = &d.topic {
                text.push(t.description.clone());
            }
            if let Some(l) = &d.outcome_labels {
                text.extend([
                    l.long.extended.clone(),
                    l.long.abbreviated.clone(),
                    l.short.extended.clone(),
                    l.short.abbreviated.clone(),
                ]);
            }
            if let Some(pascal_rs::MarketType::Sports(sp)) = &d.market_type {
                text.extend([sp.teams.home_name.clone(), sp.teams.away_name.clone()]);
            }
            let hay = text.join(" ").to_lowercase();
            if tokens.iter().all(|t| hay.contains(t.as_str())) {
                out.extend(self.market_json(s));
                if out.len() >= limit {
                    break;
                }
            }
        }
        out
    }

    /// The book of `symbol` now: unconsumed tape levels plus the mock's resting orders,
    /// aggregated by price (asks lowest first, bids highest first). Empty once resolved.
    pub fn book(&self, symbol: &str) -> OrderBook {
        let mut asks: BTreeMap<i64, u64> = BTreeMap::new();
        let mut bids: BTreeMap<i64, u64> = BTreeMap::new();
        if !self.resolutions.contains_key(symbol) {
            for (side, levels) in self.tape_levels(symbol) {
                let map = if side == Side::Ask {
                    &mut asks
                } else {
                    &mut bids
                };
                for l in levels {
                    *map.entry(l.price).or_default() += l.size;
                }
            }
            for o in self.orders.values().filter(|o| o.order.symbol == symbol) {
                let map = if o.order.side == Side::Ask {
                    &mut asks
                } else {
                    &mut bids
                };
                *map.entry(o.order.price).or_default() += o.order.size_remaining;
            }
        }
        let level = |(price, size): (i64, u64)| Level { price, size };
        OrderBook {
            asks: asks.into_iter().filter(|l| l.1 > 0).map(level).collect(),
            bids: bids
                .into_iter()
                .rev()
                .filter(|l| l.1 > 0)
                .map(level)
                .collect(),
            spec: None,
        }
    }

    /// `GET /api/v1/books`: books of the requested symbols that the tape knows (in its market
    /// catalogue or in the snapshot in force); unknown symbols are omitted.
    pub fn books(&self, symbols: &[String]) -> Books {
        let in_snapshot = |s: &String| {
            self.book_idx
                .is_some_and(|i| self.tape.books[i].books.contains_key(s))
        };
        Books {
            books: symbols
                .iter()
                .filter(|s| self.knows(s) || in_snapshot(s))
                .map(|s| (s.clone(), self.book(s)))
                .collect(),
        }
    }

    /// Unconsumed tape levels of the snapshot in force (live: of the upstream book, fresh or
    /// stale), best first: `[(Ask, asks), (Bid, bids)]`.
    fn tape_levels(&self, symbol: &str) -> [(Side, Vec<Level>); 2] {
        let live_book;
        let book = match &self.live {
            Some(l) => {
                live_book = l.books.get(symbol).map(live_order_book);
                live_book.as_ref()
            }
            None => self
                .book_idx
                .and_then(|i| self.tape.books[i].books.get(symbol)),
        };
        let Some(book) = book else {
            return [(Side::Ask, Vec::new()), (Side::Bid, Vec::new())];
        };
        let left = |side: Side, levels: &[Level]| -> Vec<Level> {
            let mut v: Vec<Level> = levels
                .iter()
                .map(|l| Level {
                    price: l.price,
                    size: l.size.saturating_sub(
                        self.consumed
                            .get(&(symbol.to_owned(), side, l.price))
                            .copied()
                            .unwrap_or(0),
                    ),
                })
                .filter(|l| l.size > 0)
                .collect();
            match side {
                Side::Ask => v.sort_by_key(|l| l.price),
                Side::Bid => v.sort_by_key(|l| Reverse(l.price)),
            }
            v
        };
        [
            (Side::Ask, left(Side::Ask, &book.asks)),
            (Side::Bid, left(Side::Bid, &book.bids)),
        ]
    }

    /// Mark price: the median of best bid, best ask and the tape's mark (Pascal's rule with
    /// the tape mark standing in for the last trade); the resolution price once resolved.
    pub fn mark_u(&self, symbol: &str) -> Option<i64> {
        if let Some(r) = self.resolutions.get(symbol) {
            return Some(r.price);
        }
        let tape_mark = self.market_obs(symbol).and_then(|(m, _)| m.mark_price);
        let book = self.book(symbol);
        match (book.bids.first(), book.asks.first(), tape_mark) {
            (Some(b), Some(a), Some(m)) => {
                let mut v = [b.price, a.price, m];
                v.sort_unstable();
                Some(v[1])
            }
            (Some(b), Some(a), None) => Some((b.price + a.price) / 2),
            (_, _, m) => m,
        }
    }

    /// `GET /api/v1/trades`: tape trades visible now plus the mock's own, newest first
    /// (by `seq`, then `trade_id`). Returns the page and the next cursor.
    pub fn trades(
        &self,
        symbols: &[String],
        limit: usize,
        before: Option<(u64, u64)>,
        at_or_before_seq: Option<u64>,
    ) -> (Vec<Value>, Option<String>) {
        let mut all: Vec<(u64, u64, Value)> = self
            .tape
            .trades
            .iter()
            .filter(|t| t.visible_ms <= self.now_ms && symbols.contains(&t.trade.symbol))
            .map(|t| (t.trade.seq, t.trade.trade_id, t.raw.clone()))
            .chain(
                self.trades
                    .iter()
                    .filter(|t| symbols.contains(&t.symbol))
                    .filter_map(|t| {
                        Some((
                            t.seq,
                            t.trade_id,
                            strip_nulls(serde_json::to_value(t).ok()?),
                        ))
                    }),
            )
            .filter(|(seq, id, _)| before.is_none_or(|b| (*seq, *id) < b))
            .filter(|(seq, _, _)| at_or_before_seq.is_none_or(|s| *seq <= s))
            .collect();
        all.sort_by_key(|(seq, id, _)| Reverse((*seq, *id)));
        let more = all.len() > limit;
        all.truncate(limit);
        let cursor = more
            .then(|| all.last().map(|(s, i, _)| format!("{s:020}:{i:020}")))
            .flatten();
        (all.into_iter().map(|(_, _, v)| v).collect(), cursor)
    }

    // ---------- live feed (L4-1) ----------

    /// Whether orders may match in `symbol` now: always on a tape; live, once the upstream
    /// book has a snapshot since the last socket drop.
    pub fn book_fresh(&self, symbol: &str) -> bool {
        self.live
            .as_ref()
            .is_none_or(|l| l.books.get(symbol).is_some_and(|b| b.fresh))
    }

    /// Whether the mock holds an upstream book entry for `symbol` (live; false on a tape).
    pub fn has_live_book(&self, symbol: &str) -> bool {
        self.live
            .as_ref()
            .is_some_and(|l| l.books.contains_key(symbol))
    }

    /// Live: how many upstream snapshots `symbol`'s book has had (it changes on every
    /// resubscription, e.g. after a reconnect), `None` on a tape or for an untracked symbol.
    /// The `/ws` `book` channel sends its subscribers a new snapshot when it changes.
    pub fn live_book_generation(&self, symbol: &str) -> Option<u64> {
        self.live
            .as_ref()
            .and_then(|l| l.books.get(symbol))
            .map(|b| b.snapshots)
    }

    /// Whether the `/ws` `book` channel may describe `symbol` now: always on a tape; live, once
    /// its upstream book is fresh (or the market resolved: its book is empty).
    pub fn book_servable(&self, symbol: &str) -> bool {
        self.book_fresh(symbol) || self.resolutions.contains_key(symbol)
    }

    /// The resolution of `symbol`, if it resolved.
    pub fn resolution(&self, symbol: &str) -> Option<&MarketResolution> {
        self.resolutions.get(symbol)
    }

    /// Notes that the mock asked the upstream for `symbol`'s book: an entry waiting for its
    /// first snapshot (stale). No-op on a tape or if the entry exists.
    pub fn live_track(&mut self, symbol: &str) {
        if let Some(l) = &mut self.live {
            l.books.entry(symbol.to_owned()).or_default();
        }
    }

    /// Forgets `symbol`'s upstream book (the upstream refused the channel, or the market
    /// resolved). Consumed liquidity of the symbol is dropped with it.
    pub fn live_untrack(&mut self, symbol: &str) {
        if let Some(l) = &mut self.live {
            l.books.remove(symbol);
            self.consumed.retain(|(s, _, _), _| s != symbol);
        }
    }

    /// Applies an upstream `book` frame. A snapshot replaces the book, marks it fresh and
    /// frees every level the mock had consumed in it; an update sets each listed level (size 0
    /// removes it) and frees that level only. Updates to a book that is not fresh are ignored
    /// (the reconnect's snapshot supersedes them). Resting mock orders the new levels cross
    /// fill as makers. Returns whether the frame was applied.
    pub fn live_book(
        &mut self,
        symbol: &str,
        snapshot: bool,
        book: &OrderBook,
        seq: Option<u64>,
    ) -> bool {
        self.advance();
        let Some(l) = &mut self.live else {
            return false;
        };
        let Some(b) = l.books.get_mut(symbol) else {
            // Not (or no longer) tracked: a frame racing an unsubscribe.
            return false;
        };
        if snapshot {
            b.replace(&book.asks, &book.bids);
            b.fresh = true;
            b.snapshots += 1;
            self.consumed.retain(|(s, _, _), _| s != symbol);
        } else {
            if !b.fresh {
                return false;
            }
            for (side, levels) in [(Side::Ask, &book.asks), (Side::Bid, &book.bids)] {
                for lv in levels {
                    b.set(side, lv.price, lv.size);
                    self.consumed.remove(&(symbol.to_owned(), side, lv.price));
                }
            }
        }
        if let Some(seq) = seq {
            self.tape_seq = self.tape_seq.max(seq);
        }
        if !self.resolutions.contains_key(symbol) {
            self.cross_resting_with_tape(Some(symbol));
        }
        true
    }

    /// Marks every upstream book stale (the socket dropped, lagged or reconnected). Returns how
    /// many were fresh.
    pub fn live_stale_all(&mut self) -> usize {
        let Some(l) = &mut self.live else {
            return 0;
        };
        let mut n = 0;
        for b in l.books.values_mut() {
            n += usize::from(b.fresh);
            b.fresh = false;
        }
        n
    }

    /// Replaces the catalogue with a full `GET /markets` refresh: listed markets are upserted,
    /// markets that left the list stay known (for `/markets/search`) but are no longer listed.
    /// Markets carrying a `resolution` resolve ([`Engine::live_resolution`]).
    pub fn live_catalogue(&mut self, items: Vec<(Market, Value)>, seq: Option<u64>) {
        self.advance();
        let now = self.now_ms;
        let Some(l) = &mut self.live else {
            return;
        };
        for m in l.markets.values_mut() {
            m.listed = false;
        }
        l.catalogue_ms = Some(now);
        if let Some(seq) = seq {
            self.tape_seq = self.tape_seq.max(seq);
        }
        self.live_upsert(items, true);
    }

    /// Upserts markets seen outside a full refresh (`/markets/search`, `market_update`); a new
    /// entry is listed when `listed`, an existing one keeps its flag unless `listed`.
    pub fn live_markets_seen(&mut self, items: Vec<(Market, Value)>, listed: bool) {
        self.advance();
        self.live_upsert(items, listed);
    }

    fn live_upsert(&mut self, items: Vec<(Market, Value)>, listed: bool) {
        let mut resolved = Vec::new();
        let Some(l) = &mut self.live else {
            return;
        };
        for (market, raw) in items {
            if let Some(r) = &market.resolution {
                resolved.push((market.symbol.clone(), r.clone()));
            }
            let symbol = market.symbol.clone();
            let keep = l.markets.get(&symbol).is_some_and(|m| m.listed);
            l.markets.insert(
                symbol,
                LiveMarket {
                    market,
                    raw,
                    listed: listed || keep,
                },
            );
        }
        for (symbol, r) in resolved {
            self.live_resolution(&symbol, r);
        }
    }

    /// An upstream resolution. A market resolves once: if the mock already resolved it
    /// (`/mock/resolve`, the script, or an earlier upstream report) or a scripted resolution is
    /// pending for it, the upstream result is ignored. Returns whether it resolved now.
    pub fn live_resolution(&mut self, symbol: &str, r: MarketResolution) -> bool {
        if let Some(mine) = self.resolutions.get(symbol) {
            if mine.price != r.price {
                tracing::info!(%symbol, mock = mine.price, upstream = r.price,
                    "upstream resolution ignored: the mock resolved this market first");
            }
            return false;
        }
        if self.pending.iter().any(|p| p.1 == symbol)
            || self.pending_wall.iter().any(|p| p.1 == symbol)
        {
            tracing::info!(%symbol, upstream = r.price,
                "upstream resolution ignored: a mock resolution is scheduled");
            return false;
        }
        self.tape_seq = self.tape_seq.max(r.seq);
        self.resolve_now(symbol, r.price, r.market_resolution_ts_ms, Some(r.seq));
        true
    }

    /// Book and catalogue counters (zeros on a tape, except the books of the snapshot in force).
    pub fn feed_counts(&self) -> FeedCounts {
        match &self.live {
            Some(l) => FeedCounts {
                books: l.books.len(),
                stale_books: l.books.values().filter(|b| !b.fresh).count(),
                markets: l.markets.len(),
                catalogue_ms: l.catalogue_ms,
            },
            None => FeedCounts {
                books: self.book_idx.map_or(0, |i| self.tape.books[i].books.len()),
                ..FeedCounts::default()
            },
        }
    }

    /// Tracked symbols that are unresolved and due for a resolution check upstream: started at
    /// least `after_start_ms` ago, no longer listed, or not in the catalogue at all.
    pub fn live_search_due(&self, after_start_ms: u64) -> Vec<String> {
        let Some(l) = &self.live else {
            return Vec::new();
        };
        l.books
            .keys()
            .filter(|s| !self.resolutions.contains_key(*s))
            .filter(|s| match l.markets.get(*s) {
                None => true,
                Some(m) => {
                    !m.listed
                        || m.market
                            .display_attributes
                            .expected_event_start_time_ms
                            .is_some_and(|t| t.saturating_add(after_start_ms) <= self.now_ms)
                }
            })
            .cloned()
            .collect()
    }

    /// The upstream's view for a recording: each tracked symbol's book as the upstream last
    /// sent it (fresh books only; a resolved market's book is empty, as Pascal serves it), and
    /// the highest upstream seq.
    pub fn live_record_books(&self) -> (BTreeMap<String, OrderBook>, u64) {
        let Some(l) = &self.live else {
            return (BTreeMap::new(), self.tape_seq);
        };
        let books = l
            .books
            .iter()
            .filter_map(|(s, b)| {
                if self.resolutions.contains_key(s) {
                    Some((s.clone(), empty_book()))
                } else if b.fresh {
                    Some((s.clone(), live_order_book(b)))
                } else {
                    None
                }
            })
            .collect();
        (books, self.tape_seq)
    }

    /// The upstream catalogue entries of `symbols` (verbatim), for a recording.
    pub fn live_markets_raw(&self, symbols: &[String]) -> Vec<Value> {
        let Some(l) = &self.live else {
            return Vec::new();
        };
        symbols
            .iter()
            .filter_map(|s| l.markets.get(s).map(|m| m.raw.clone()))
            .collect()
    }

    /// Symbols with an upstream book entry (live), ascending.
    pub fn live_books(&self) -> Vec<String> {
        self.live
            .as_ref()
            .map_or_else(Vec::new, |l| l.books.keys().cloned().collect())
    }

    // ---------- accounts ----------

    fn account_mut(&mut self, owner: &str) -> &mut Account {
        let initial = self.initial_collateral_u;
        self.accounts
            .entry(owner.to_owned())
            .or_insert_with(|| Account::new(initial))
    }

    /// `GET /api/v1/accounts/{owner}` (an unknown owner is a fresh account).
    pub fn account(
        &mut self,
        owner: &str,
        symbol: Option<&str>,
        exclude_orders: bool,
    ) -> AccountState {
        self.account_mut(owner);
        let acct = &self.accounts[owner];
        let keep = |s: &str| symbol.is_none_or(|x| x == s);
        let orders = if exclude_orders {
            Vec::new()
        } else {
            self.orders
                .values()
                .filter(|o| o.owner == owner && keep(&o.order.symbol))
                .map(|o| o.order.clone())
                .collect()
        };
        let positions = acct
            .positions
            .iter()
            .filter(|(s, p)| p.size != 0 && keep(s))
            .map(|(s, p)| self.position_msg(s, p))
            .collect();
        AccountState {
            has_deposited: Some(true),
            username: None,
            twitter_account_connection: None,
            collateral_usd: Some(acct.collateral_u),
            rewards: Some(acct.rewards()),
            lifetime_stats: Some(acct.lifetime()),
            orders,
            positions,
            pending_deposits: Vec::new(),
            pending_withdrawals: Vec::new(),
            order_updates: None,
            fills: None,
            resolutions: None,
            closed_positions: None,
            transfers: None,
        }
    }

    /// Every fill of `owner`, oldest first.
    pub fn account_fills(&self, owner: &str) -> &[Fill] {
        self.accounts.get(owner).map_or(&[], |a| &a.fills)
    }

    /// `GET /api/v1/accounts/{owner}/fills`: newest first, optionally only `symbols`, strictly
    /// older than `before` (a decoded cursor) or at or before `at_or_before_seq`. Returns the
    /// page and the cursor of its last item when more items follow. A fill's cursor is
    /// `<seq:020>:<n:010>` with `n` its ordinal in the account (the shape of Pascal's cursors).
    pub fn fills_page(
        &self,
        owner: &str,
        symbols: &[String],
        limit: usize,
        before: Option<(u64, u64)>,
        at_or_before_seq: Option<u64>,
    ) -> (Vec<Fill>, Option<String>) {
        let mut page: Vec<(u64, u64, &Fill)> = self
            .account_fills(owner)
            .iter()
            .enumerate()
            .rev()
            .map(|(n, f)| (f.seq, n as u64, f))
            .filter(|(_, _, f)| symbols.is_empty() || symbols.contains(&f.symbol))
            .filter(|(seq, n, _)| before.is_none_or(|b| (*seq, *n) < b))
            .filter(|(seq, _, _)| at_or_before_seq.is_none_or(|s| *seq <= s))
            .take(limit.saturating_add(1))
            .collect();
        let more = page.len() > limit;
        page.truncate(limit);
        let cursor = more
            .then(|| page.last().map(|(s, n, _)| fill_cursor(*s, *n)))
            .flatten();
        (
            page.into_iter().map(|(_, _, f)| f.clone()).collect(),
            cursor,
        )
    }

    /// `GET /api/v1/accounts/{owner}/builder-stats`: the fills that named `owner` as builder.
    /// Zeros for an account that never was one (Pascal answers the same).
    pub fn builder_stats(&self, owner: &str) -> BuilderStats {
        let (t, fees) = self
            .accounts
            .get(owner)
            .map_or((BuilderTally::default(), 0), |a| {
                (a.builder, a.builder_fees_received_u)
            });
        BuilderStats {
            builder_volume_contracts: t.contracts,
            builder_volume_notional_usd: t.notional_u,
            builder_fee_base_usd: t.fee_base_u,
            builder_fees_received_usd: fees,
        }
    }

    /// Replaces `owner`'s account with the state of a saved `GET /api/v1/accounts/{owner}`
    /// response (`--account-snapshot`, the mode-R demo): free collateral, positions with their
    /// entry basis and cumulative fields, lifetime totals, builder fees received. Resting
    /// orders are not restored (they rest on real Pascal, not in the mock's book); positions
    /// in markets that are not on the tape are kept but never resolve. Returns how many
    /// positions were loaded.
    pub fn boot_account(&mut self, owner: &str, state: &AccountState) -> usize {
        let mut acct = Account::new(state.collateral_usd.unwrap_or(self.initial_collateral_u));
        for p in state.positions.iter().filter(|p| p.size != 0) {
            let q = p.size.unsigned_abs();
            let entry = p
                .remaining_entry_notional_usd
                .or_else(|| p.average_entry_price.map(|a| notional_u(q, a)))
                .unwrap_or(0);
            if self.live.is_none() && !self.knows(&p.symbol) {
                tracing::warn!(%owner, symbol = %p.symbol, "snapshot position is not on the tape");
            }
            acct.positions.insert(
                p.symbol.clone(),
                PositionState {
                    size: p.size,
                    entry_notional_u: entry,
                    cum_inc_contracts: p.cumulative_increasing_contracts,
                    cum_inc_notional_u: p.cumulative_increasing_notional_usd,
                    cum_red_contracts: p.cumulative_reducing_contracts,
                    cum_red_notional_u: p.cumulative_reducing_notional_usd,
                    cum_fees_u: p.cumulative_fees_usd,
                    realized_u: p.realized_pnl_usd,
                    seq: p.seq,
                    update_seq: p.update_seq,
                    open_ts: p.open_ts_ms,
                    update_ts: p.update_ts_ms,
                },
            );
        }
        if let Some(r) = &state.rewards {
            acct.builder_fees_received_u = r.builder_fees_received_usd;
        }
        if let Some(l) = &state.lifetime_stats {
            acct.maker = Lifetime::of(&l.maker);
            acct.taker = Lifetime::of(&l.taker);
        }
        if !state.orders.is_empty() {
            tracing::warn!(%owner, orders = state.orders.len(), "snapshot resting orders are not restored");
        }
        let n = acct.positions.len();
        self.accounts.insert(owner.to_owned(), acct);
        n
    }

    fn position_msg(&self, symbol: &str, p: &PositionState) -> Position {
        let q = p.size.unsigned_abs();
        let avg = (q > 0).then(|| p.entry_notional_u / q as i64);
        let mark = self.mark_u(symbol).or(avg).unwrap_or(0);
        let unrealized = if p.size > 0 {
            notional_u(q, mark) - p.entry_notional_u
        } else {
            p.entry_notional_u - notional_u(q, mark)
        };
        Position {
            symbol: symbol.to_owned(),
            size: p.size,
            cumulative_increasing_contracts: p.cum_inc_contracts,
            cumulative_increasing_notional_usd: p.cum_inc_notional_u,
            cumulative_reducing_contracts: p.cum_red_contracts,
            cumulative_reducing_notional_usd: p.cum_red_notional_u,
            cumulative_fees_usd: p.cum_fees_u,
            realized_pnl_usd: p.realized_u,
            unrealized_pnl_usd: unrealized,
            mark_price: mark,
            average_entry_price: avg,
            remaining_entry_notional_usd: Some(p.entry_notional_u),
            seq: p.seq,
            update_seq: p.update_seq,
            open_ts_ms: p.open_ts,
            update_ts_ms: p.update_ts,
        }
    }

    // ---------- orders ----------

    /// Places one order (`POST /api/v1/orders` element). The caller has verified the permit.
    pub fn place(&mut self, req: &PlaceOrder) -> Result<PlaceResult, Reject> {
        let now = self.now_ms;
        let owner = req.auth.owner.clone();
        let symbol = req.symbol.as_str();
        let spec = self.spec(symbol).ok_or_else(|| {
            Reject::new(
                ApiErrorCode::SymbolNotFound,
                format!("unknown symbol {symbol}"),
            )
        })?;
        if self.resolutions.contains_key(symbol) {
            return Err(Reject::new(
                ApiErrorCode::MarketResolved,
                format!("{symbol} has resolved"),
            ));
        }
        if !self.book_fresh(symbol) {
            // Live mode: never match against a book the upstream has not re-sent since the
            // socket dropped (L4-1). Pascal's transient code; the desk treats it as a reject.
            return Err(Reject::new(
                ApiErrorCode::NotReady,
                format!(
                    "the book of {symbol} is stale (upstream not connected or snapshot pending); \
                     no fills until its fresh snapshot lands"
                ),
            ));
        }
        if !valid_price(req.price_u, spec.tick_min_u, spec.tick_sig_figs) {
            return Err(Reject::new(
                ApiErrorCode::InvalidPrice,
                format!(
                    "price {} is not on the tick grid of {symbol}",
                    pascal_rs::format_micros(req.price_u)
                ),
            ));
        }
        if req.expires_ts_ms.is_some_and(|e| e <= now) {
            return Err(Reject::new(
                ApiErrorCode::InvalidExpiration,
                "expires_ts_ms is in the past",
            ));
        }
        self.account_mut(&owner);

        // Replacement target.
        let replaced = match req.replace_client_order_id {
            None => None,
            Some(c) => match self.find_open(&owner, CancelTarget::ClientOrderId(c)) {
                Some(id) => {
                    let o = &self.orders[&id].order;
                    if o.symbol != symbol {
                        return Err(Reject::new(
                            ApiErrorCode::ReplacementOrderDiffMarket,
                            "replacement is in a different market",
                        ));
                    }
                    if o.side != req.side {
                        return Err(Reject::new(
                            ApiErrorCode::ReplacementOrderDiffSide,
                            "replacement is on a different side",
                        ));
                    }
                    Some(id)
                }
                None if req.allow_missing_replace => None,
                None => {
                    return Err(Reject::new(
                        ApiErrorCode::ReplacementOrderNotFound,
                        format!("client_order_id {c} is not an open order"),
                    ));
                }
            },
        };
        if let Some(dup) = self.find_open(&owner, CancelTarget::ClientOrderId(req.client_order_id))
            && Some(dup) != replaced
        {
            return Err(Reject::new(
                ApiErrorCode::DuplicateClientOrderId,
                format!("client_order_id {} is already open", req.client_order_id),
            ));
        }
        let size = match (req.replace_with_open_size, replaced) {
            (true, Some(id)) => self.orders[&id].order.size_remaining,
            _ => req.size,
        };
        if size == 0 {
            return Err(Reject::new(
                ApiErrorCode::InvalidSize,
                "size must be at least 1",
            ));
        }
        if let Some(b) = &req.builder {
            let initial = self.initial_collateral_u;
            let c = self
                .accounts
                .get(&b.pubkey)
                .map_or(initial, |a| a.collateral_u);
            if c < BUILDER_MIN_COLLATERAL_U {
                return Err(Reject::new(
                    ApiErrorCode::BuilderCollateralBelowMinimum,
                    "builder holds less than $100 of free collateral",
                ));
            }
        }
        let position = self.accounts[&owner].position_size(symbol);
        if req.reduce_only && reducible(position, req.side) == 0 {
            return Err(Reject::new(
                ApiErrorCode::ReduceOnlyRejected,
                "reduce-only order would increase the position",
            ));
        }
        let crosses = self.best_opposite(symbol, req.side).is_some_and(|m| {
            let p = match m {
                Maker::Tape { price_u, .. } | Maker::Mock { price_u, .. } => price_u,
            };
            crosses(req.side, req.price_u, p)
        });
        if req.post_only && crosses {
            return Err(Reject::new(
                ApiErrorCode::WouldCross,
                "post-only order would cross the book",
            ));
        }
        let required = required_collateral(
            position,
            req.side,
            size,
            req.price_u,
            spec.taker_rate_u + req.builder.as_ref().map_or(0, |b| b.fee_rate_u),
        );
        let free = self.accounts[&owner].collateral_u;
        if free < required {
            return Err(Reject::new(
                ApiErrorCode::InsufficientCollateral,
                format!(
                    "order needs {} of free collateral, account has {}",
                    pascal_rs::format_micros(required),
                    pascal_rs::format_micros(free)
                ),
            ));
        }
        let resting_here = self
            .orders
            .values()
            .filter(|o| {
                o.owner == owner && o.order.symbol == symbol && Some(o.order.id) != replaced
            })
            .count();
        let may_rest = req.tif != TimeInForce::Ioc;
        if may_rest && !crosses && resting_here >= MAX_RESTING_PER_MARKET {
            return Err(Reject::new(
                ApiErrorCode::RestingOrderLimitExceeded,
                format!("{MAX_RESTING_PER_MARKET} resting orders in {symbol} already"),
            ));
        }

        // Commit.
        let mut priority = None;
        if let Some(id) = replaced {
            let old = &self.orders[&id].order;
            if old.price == req.price_u && size <= old.size_remaining {
                priority = Some(self.orders[&id].priority);
            }
            self.close_order(id, OrderUpdateReason::Replaced);
        }
        let seq = self.next_seq();
        let id = self.next_order_id;
        self.next_order_id += 1;
        let mut order = Order {
            id,
            client_order_id: req.client_order_id,
            replace_client_order_id: req.replace_client_order_id,
            symbol: symbol.to_owned(),
            side: req.side,
            price: req.price_u,
            size_original: size,
            size_remaining: size,
            size_filled: 0,
            notional_filled: 0,
            tif: req.tif,
            order_type: req.order_type,
            post_only: req.post_only,
            reduce_only: req.reduce_only,
            expires_ts_ms: req.expires_ts_ms,
            builder_pubkey: req.builder.as_ref().map(|b| b.pubkey.clone()),
            builder_fee_rate: req.builder.as_ref().map(|b| Decimal::new(b.fee_rate_u, 6)),
            seq,
            update_seq: seq,
            place_ts_ms: now,
            update_ts_ms: now,
        };
        let fills = self.match_incoming(&owner, &mut order, req.builder.as_ref(), spec);

        let mut reason = None;
        if order.size_remaining > 0 {
            let reducible_now = reducible(self.accounts[&owner].position_size(symbol), order.side);
            if !may_rest {
                order.size_remaining = 0;
                reason = Some(OrderUpdateReason::IocRemainderCanceled);
            } else if order.reduce_only && reducible_now == 0 {
                order.size_remaining = 0;
                reason = Some(OrderUpdateReason::NoReduciblePosition);
            } else if resting_here >= MAX_RESTING_PER_MARKET {
                order.size_remaining = 0;
                reason = Some(OrderUpdateReason::RestingOrderLimit);
            } else if order.reduce_only && order.size_remaining > reducible_now {
                order.size_remaining = reducible_now;
                reason = Some(OrderUpdateReason::ReduceOnlyResize);
            }
            if order.size_remaining == 0 || reason == Some(OrderUpdateReason::ReduceOnlyResize) {
                order.update_seq = self.next_seq();
            }
        }
        if order.size_remaining > 0 {
            self.orders.insert(
                id,
                OpenOrder {
                    owner: owner.clone(),
                    order: order.clone(),
                    builder: req.builder.clone(),
                    priority: priority.unwrap_or(seq),
                },
            );
        }
        self.emit(MockEvent::Order {
            owner,
            update: OrderUpdate {
                order: order.clone(),
                update_reason: reason,
            },
        });
        Ok(PlaceResult {
            order,
            fills,
            update_reason: reason,
        })
    }

    /// Cancels one open order of the request's owner.
    pub fn cancel(&mut self, req: &CancelOrder) -> Result<Order, Reject> {
        let id = self.find_open(&req.auth.owner, req.target).ok_or_else(|| {
            let what = match req.target {
                CancelTarget::ClientOrderId(c) => format!("client_order_id {c}"),
                CancelTarget::OrderId(o) => format!("order_id {o}"),
            };
            Reject::new(
                ApiErrorCode::OrderNotFound,
                format!("{what} is not an open order"),
            )
        })?;
        Ok(self.close_order(id, OrderUpdateReason::UserCancel))
    }

    /// Cancels every resting order of the owner in one market.
    pub fn mass_cancel(&mut self, req: &MassCancel) -> Result<Vec<Order>, Reject> {
        if !self.knows(&req.symbol) {
            return Err(Reject::new(
                ApiErrorCode::SymbolNotFound,
                format!("unknown symbol {}", req.symbol),
            ));
        }
        let ids: Vec<u64> = self
            .orders
            .iter()
            .filter(|(_, o)| o.owner == req.auth.owner && o.order.symbol == req.symbol)
            .map(|(id, _)| *id)
            .collect();
        Ok(ids
            .into_iter()
            .map(|id| self.close_order(id, OrderUpdateReason::MassCancelled))
            .collect())
    }

    fn find_open(&self, owner: &str, target: CancelTarget) -> Option<u64> {
        self.orders
            .iter()
            .find(|(_, o)| {
                o.owner == owner
                    && match target {
                        CancelTarget::ClientOrderId(c) => o.order.client_order_id == c,
                        CancelTarget::OrderId(i) => o.order.id == i,
                    }
            })
            .map(|(id, _)| *id)
    }

    /// Removes an open order, returning its final state (size_remaining 0).
    fn close_order(&mut self, id: u64, reason: OrderUpdateReason) -> Order {
        let seq = self.next_seq();
        let now = self.now_ms;
        let mut o = self
            .orders
            .remove(&id)
            .expect("close_order on an open order");
        o.order.size_remaining = 0;
        o.order.update_seq = seq;
        o.order.update_ts_ms = now;
        self.emit(MockEvent::Order {
            owner: o.owner,
            update: OrderUpdate {
                order: o.order.clone(),
                update_reason: Some(reason),
            },
        });
        o.order
    }

    // ---------- matching ----------

    /// Best liquidity an order on `side` would meet: the tape level or the mock order with the
    /// best price; at equal prices the tape first, then mock orders by time priority.
    fn best_opposite(&self, symbol: &str, side: Side) -> Option<Maker> {
        let opp = opposite(side);
        let [(_, asks), (_, bids)] = self.tape_levels(symbol);
        let tape = match opp {
            Side::Ask => asks.first().copied(),
            Side::Bid => bids.first().copied(),
        }
        .map(|l| Maker::Tape {
            price_u: l.price,
            available: l.size,
        });
        let mock = self
            .orders
            .values()
            .filter(|o| o.order.symbol == symbol && o.order.side == opp)
            .min_by_key(|o| (better_key(opp, o.order.price), o.priority))
            .map(|o| Maker::Mock {
                price_u: o.order.price,
                id: o.order.id,
            });
        match (tape, mock) {
            (
                Some(t @ Maker::Tape { price_u: tp, .. }),
                Some(m @ Maker::Mock { price_u: mp, .. }),
            ) => {
                if better_key(opp, mp) < better_key(opp, tp) {
                    Some(m)
                } else {
                    Some(t)
                }
            }
            (t, m) => t.or(m),
        }
    }

    /// Matches an incoming order until it is filled, its limit stops crossing, or (reduce-only)
    /// nothing is left to reduce. Returns the taker's fills.
    fn match_incoming(
        &mut self,
        owner: &str,
        order: &mut Order,
        builder: Option<&Builder>,
        spec: Spec,
    ) -> Vec<Fill> {
        let symbol = order.symbol.clone();
        let mut fills = Vec::new();
        while order.size_remaining > 0 {
            let taker_prev = self.accounts[owner].position_size(&symbol);
            let cap = if order.reduce_only {
                reducible(taker_prev, order.side)
            } else {
                u64::MAX
            };
            if cap == 0 {
                break;
            }
            let Some(maker) = self.best_opposite(&symbol, order.side) else {
                break;
            };
            let (price_u, available) = match maker {
                Maker::Tape { price_u, available } => (price_u, available),
                Maker::Mock { price_u, id } => (price_u, self.orders[&id].order.size_remaining),
            };
            if !crosses(order.side, order.price, price_u) {
                break;
            }
            let mut qty = order.size_remaining.min(available).min(cap);

            // The maker side: tape liquidity, or a mock order (self-trade, reduce-only and
            // collateral checks cancel the maker and matching continues).
            let maker_owner;
            let mut maker_prev = None;
            let mut maker_ro = None;
            let mut maker_order = None;
            match maker {
                Maker::Tape { .. } => maker_owner = tape_owner(),
                Maker::Mock { id, .. } => {
                    let m = self.orders[&id].clone();
                    if m.owner == owner {
                        self.close_order(id, OrderUpdateReason::SelfTrade);
                        continue;
                    }
                    let prev = self.accounts[&m.owner].position_size(&symbol);
                    if m.order.reduce_only {
                        qty = qty.min(reducible(prev, m.order.side));
                        if qty == 0 {
                            self.close_order(id, OrderUpdateReason::NoReduciblePosition);
                            continue;
                        }
                    }
                    if !self.can_afford_maker(&m, qty, spec) {
                        self.close_order(id, OrderUpdateReason::Undercollateralized);
                        continue;
                    }
                    maker_owner = m.owner.clone();
                    maker_prev = Some(prev);
                    maker_ro = Some(m.order.reduce_only);
                    maker_order = Some(m);
                }
            }

            let trade_id = self.next_trade_id;
            self.next_trade_id += 1;
            let seq = self.next_seq();
            match &maker_order {
                None => {
                    *self
                        .consumed
                        .entry((symbol.clone(), opposite(order.side), price_u))
                        .or_default() += qty;
                }
                Some(m) => {
                    self.book_fill(FillArgs {
                        owner: &m.owner,
                        symbol: &symbol,
                        side: m.order.side,
                        price_u,
                        size: qty,
                        liquidity: Liquidity::Maker,
                        counterparty: owner,
                        order_id: m.order.id,
                        client_order_id: m.order.client_order_id,
                        reduce_only: m.order.reduce_only,
                        builder: m.builder.as_ref(),
                        spec,
                        trade_id,
                        seq,
                    });
                    self.after_maker_fill(m.order.id, qty, price_u, seq);
                }
            }
            let fill = self.book_fill(FillArgs {
                owner,
                symbol: &symbol,
                side: order.side,
                price_u,
                size: qty,
                liquidity: Liquidity::Taker,
                counterparty: &maker_owner,
                order_id: order.id,
                client_order_id: order.client_order_id,
                reduce_only: order.reduce_only,
                builder,
                spec,
                trade_id,
                seq,
            });
            order.size_remaining -= qty;
            order.size_filled += qty;
            order.notional_filled += notional_u(qty, price_u);
            order.update_seq = seq;
            order.update_ts_ms = self.now_ms;
            self.trades.push(PublicTrade {
                trade_id,
                symbol: symbol.clone(),
                taker_side: order.side,
                price: price_u,
                size: qty,
                maker: maker_owner,
                taker: owner.to_owned(),
                maker_reduce_only: maker_ro,
                taker_reduce_only: Some(order.reduce_only),
                maker_position_size_prev: maker_prev,
                taker_position_size_prev: Some(taker_prev),
                seq,
                trade_ts_ms: self.now_ms,
            });
            fills.push(fill);
        }
        fills
    }

    fn after_maker_fill(&mut self, id: u64, qty: u64, price_u: i64, seq: u64) {
        let now = self.now_ms;
        let Some(o) = self.orders.get_mut(&id) else {
            return;
        };
        o.order.size_remaining -= qty;
        o.order.size_filled += qty;
        o.order.notional_filled += notional_u(qty, price_u);
        o.order.update_seq = seq;
        o.order.update_ts_ms = now;
        let update = OrderUpdate {
            order: o.order.clone(),
            update_reason: None,
        };
        let owner = o.owner.clone();
        if o.order.size_remaining == 0 {
            self.orders.remove(&id);
        }
        self.emit(MockEvent::Order { owner, update });
    }

    fn can_afford_maker(&self, m: &OpenOrder, qty: u64, spec: Spec) -> bool {
        let acct = &self.accounts[&m.owner];
        let prev = acct.position_size(&m.order.symbol);
        let (_, _, pos_change) = position_change(prev, m.order.side, qty, m.order.price);
        let rebate = spec.rebate(qty, m.order.price);
        let builder = m
            .builder
            .as_ref()
            .map_or(0, |b| fee_u(qty, m.order.price, b.fee_rate_u));
        acct.collateral_u + pos_change + rebate - builder >= 0
    }

    /// Fills resting mock orders that the snapshot in force (live: the upstream book of `only`)
    /// crosses, best orders first, at the orders' own prices (they are the makers); the tape
    /// liquidity they take is consumed.
    fn cross_resting_with_tape(&mut self, only: Option<&str>) {
        let mut ids: Vec<(String, Side, i64, u64, u64)> = self
            .orders
            .values()
            .filter(|o| only.is_none_or(|s| o.order.symbol == s))
            .map(|o| {
                (
                    o.order.symbol.clone(),
                    o.order.side,
                    better_key(o.order.side, o.order.price),
                    o.priority,
                    o.order.id,
                )
            })
            .collect();
        ids.sort_by(|a, b| {
            (&a.0, a.1 == Side::Ask, a.2, a.3, a.4).cmp(&(&b.0, b.1 == Side::Ask, b.2, b.3, b.4))
        });
        for (symbol, side, _, _, id) in ids {
            let Some(spec) = self.spec(&symbol) else {
                continue;
            };
            while let Some(o) = self.orders.get(&id).cloned() {
                let levels = self.tape_levels(&symbol);
                let opp = if side == Side::Bid {
                    &levels[0].1
                } else {
                    &levels[1].1
                };
                let Some(level) = opp.first().copied() else {
                    break;
                };
                if !crosses(side, o.order.price, level.price) {
                    break;
                }
                let prev = self.accounts[&o.owner].position_size(&symbol);
                let cap = if o.order.reduce_only {
                    reducible(prev, side)
                } else {
                    u64::MAX
                };
                if cap == 0 {
                    self.close_order(id, OrderUpdateReason::NoReduciblePosition);
                    break;
                }
                let qty = o.order.size_remaining.min(level.size).min(cap);
                if !self.can_afford_maker(&o, qty, spec) {
                    self.close_order(id, OrderUpdateReason::Undercollateralized);
                    break;
                }
                *self
                    .consumed
                    .entry((symbol.clone(), opposite(side), level.price))
                    .or_default() += qty;
                let trade_id = self.next_trade_id;
                self.next_trade_id += 1;
                let seq = self.next_seq();
                let tape = tape_owner();
                self.book_fill(FillArgs {
                    owner: &o.owner,
                    symbol: &symbol,
                    side,
                    price_u: o.order.price,
                    size: qty,
                    liquidity: Liquidity::Maker,
                    counterparty: &tape,
                    order_id: id,
                    client_order_id: o.order.client_order_id,
                    reduce_only: o.order.reduce_only,
                    builder: o.builder.as_ref(),
                    spec,
                    trade_id,
                    seq,
                });
                self.after_maker_fill(id, qty, o.order.price, seq);
                self.trades.push(PublicTrade {
                    trade_id,
                    symbol: symbol.clone(),
                    taker_side: opposite(side),
                    price: o.order.price,
                    size: qty,
                    maker: o.owner.clone(),
                    taker: tape.clone(),
                    maker_reduce_only: Some(o.order.reduce_only),
                    taker_reduce_only: None,
                    maker_position_size_prev: Some(prev),
                    taker_position_size_prev: None,
                    seq,
                    trade_ts_ms: self.now_ms,
                });
            }
        }
    }

    /// Books one side of a trade on an account: position, collateral, fees, builder credit.
    fn book_fill(&mut self, a: FillArgs<'_>) -> Fill {
        let now = self.now_ms;
        let fee = match a.liquidity {
            Liquidity::Taker => fee_u(a.size, a.price_u, a.spec.taker_rate_u),
            Liquidity::Maker => -a.spec.rebate(a.size, a.price_u),
        };
        let builder_fee = a.builder.map(|b| fee_u(a.size, a.price_u, b.fee_rate_u));
        let acct = self.account_mut(a.owner);
        let pos = acct.positions.entry(a.symbol.to_owned()).or_default();
        let prev = pos.size;
        let (reducing, increasing, pos_change) = position_change(prev, a.side, a.size, a.price_u);
        let open_seq = if prev == 0 { a.seq } else { pos.seq };
        let open_ts = if prev == 0 { now } else { pos.open_ts };

        let mut realized = 0;
        if reducing > 0 {
            let abs = prev.unsigned_abs();
            let basis = clamp_i64(
                i128::from(pos.entry_notional_u) * i128::from(reducing) / i128::from(abs),
            );
            let exit = notional_u(reducing, a.price_u);
            realized = if prev > 0 { exit - basis } else { basis - exit };
            pos.entry_notional_u -= basis;
            pos.cum_red_contracts += reducing;
            pos.cum_red_notional_u += exit;
            pos.realized_u += realized;
            pos.size += if prev > 0 {
                -(reducing as i64)
            } else {
                reducing as i64
            };
            if pos.size == 0 {
                // Lifecycle ends on touching zero.
                *pos = PositionState::default();
            }
        }
        if increasing > 0 {
            if pos.size == 0 {
                *pos = PositionState {
                    seq: a.seq,
                    open_ts: now,
                    ..PositionState::default()
                };
            }
            let n = notional_u(increasing, a.price_u);
            pos.entry_notional_u += n;
            pos.cum_inc_contracts += increasing;
            pos.cum_inc_notional_u += n;
            pos.size += match a.side {
                Side::Bid => increasing as i64,
                Side::Ask => -(increasing as i64),
            };
        }
        pos.cum_fees_u += fee;
        pos.update_seq = a.seq;
        pos.update_ts = now;
        let collateral_change = pos_change - fee - builder_fee.unwrap_or(0);
        acct.collateral_u += collateral_change;
        let life = match a.liquidity {
            Liquidity::Taker => &mut acct.taker,
            Liquidity::Maker => &mut acct.maker,
        };
        life.contracts += a.size;
        life.weighted_u += weighted_u(a.size, a.price_u);
        life.notional_u += notional_u(a.size, a.price_u);
        life.fees_u += fee;

        let fill = Fill {
            trade_id: a.trade_id,
            symbol: a.symbol.to_owned(),
            side: a.side,
            reduce_only: Some(a.reduce_only),
            fill_price: a.price_u,
            fill_size: a.size,
            liquidity: a.liquidity,
            counterparty: a.counterparty.to_owned(),
            order_id: a.order_id,
            client_order_id: a.client_order_id,
            fee_usd: fee,
            collateral_change_usd: collateral_change,
            position_size_prev: prev,
            realized_pnl_usd: realized,
            position_open_seq: Some(open_seq),
            position_open_ts_ms: Some(open_ts),
            builder_pubkey: a.builder.map(|b| b.pubkey.clone()),
            builder_fee_paid_usd: builder_fee,
            seq: a.seq,
            trade_ts_ms: now,
        };
        acct.fills.push(fill.clone());
        if let (Some(b), Some(amount)) = (a.builder, builder_fee) {
            let ba = self.account_mut(&b.pubkey);
            ba.collateral_u += amount;
            ba.builder_fees_received_u += amount;
            ba.builder.contracts += u128::from(a.size);
            ba.builder.notional_u += notional_u(a.size, a.price_u);
            ba.builder.fee_base_u += fee_u(a.size, a.price_u, MICRO);
            self.emit(MockEvent::BuilderFee {
                builder: b.pubkey.clone(),
                amount_u: amount,
                seq: a.seq,
            });
        }
        self.emit(MockEvent::Fill {
            owner: a.owner.to_owned(),
            fill: fill.clone(),
        });
        fill
    }
}

/// `boot + d` ms (saturating a century out).
fn wall_deadline(boot: Instant, d_ms: u64) -> Instant {
    boot.checked_add(Duration::from_millis(d_ms))
        .unwrap_or_else(|| boot + Duration::from_secs(100 * 365 * 86_400))
}

/// A `/fills` cursor.
fn fill_cursor(seq: u64, n: u64) -> String {
    format!("{seq:020}:{n:010}")
}

fn live_order_book(b: &LiveBook) -> OrderBook {
    OrderBook {
        asks: b.levels(Side::Ask),
        bids: b.levels(Side::Bid),
        spec: None,
    }
}

fn empty_book() -> OrderBook {
    OrderBook {
        asks: Vec::new(),
        bids: Vec::new(),
        spec: None,
    }
}

/// The other side.
pub fn opposite(side: Side) -> Side {
    match side {
        Side::Bid => Side::Ask,
        Side::Ask => Side::Bid,
    }
}

/// Sort key putting the best price of `side` first (asks: low; bids: high).
fn better_key(side: Side, price_u: i64) -> i64 {
    match side {
        Side::Ask => price_u,
        Side::Bid => -price_u,
    }
}

/// Whether an order on `side` with limit `limit_u` trades against resting price `resting_u`.
pub fn crosses(side: Side, limit_u: i64, resting_u: i64) -> bool {
    match side {
        Side::Bid => resting_u <= limit_u,
        Side::Ask => resting_u >= limit_u,
    }
}

/// Contracts an order on `side` can reduce given the signed position.
pub fn reducible(position: i64, side: Side) -> u64 {
    match side {
        Side::Bid if position < 0 => position.unsigned_abs(),
        Side::Ask if position > 0 => position.unsigned_abs(),
        _ => 0,
    }
}

/// Splits a fill into reducing and increasing contracts and returns the position collateral
/// change (before fees): increasing a long costs `p`, a short `1 − p`; reducing a long returns
/// `p`, a short `1 − p`, per contract.
pub fn position_change(prev: i64, side: Side, size: u64, price_u: i64) -> (u64, u64, i64) {
    let reducing = size.min(reducible(prev, side));
    let increasing = size - reducing;
    let (inc_cost, red_return) = match side {
        Side::Bid => (
            notional_u(increasing, price_u),
            notional_u(reducing, MICRO - price_u),
        ),
        Side::Ask => (
            notional_u(increasing, MICRO - price_u),
            notional_u(reducing, price_u),
        ),
    };
    (reducing, increasing, red_return - inc_cost)
}

/// Worst-case free collateral an order needs: the increasing part locked at the limit price
/// plus fees at the fill price maximising `p (1 − p)` within the limit, minus what the reducing
/// part releases at the limit. `fee_rate_u` is taker + builder.
pub fn required_collateral(prev: i64, side: Side, size: u64, limit_u: i64, fee_rate_u: i64) -> i64 {
    let (_, _, change) = position_change(prev, side, size, limit_u);
    let half = MICRO / 2;
    let worst_p = match side {
        Side::Bid => limit_u.min(half),
        Side::Ask => limit_u.max(half),
    };
    (fee_u(size, worst_p, fee_rate_u) - change).max(0)
}
