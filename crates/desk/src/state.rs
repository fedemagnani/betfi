//! Shared state of the desk: the Pascal handle, the database, the catalogue cache, the spec
//! section 5 internal state (`books`, `tickets`, `held`, `order_map`) and the [`DeskEvent`] bus.
//!
//! # Locking rules
//!
//! - Every map sits behind a [`Locked`] (a `std::sync::RwLock`). Its guards are `!Send`, so a
//!   guard held across an `.await` does not compile inside a spawned task or an axum handler:
//!   copy what you need out, drop the guard, then await.
//! - When two locks are needed at once, take them in field order and release them together:
//!   `catalogue` → `books` → `tickets` → `held` → `order_map`. Prefer taking one at a time.
//! - Never send on the bus or touch the database while holding a guard.
//!
//! # The bus
//!
//! [`DeskEvent`]s go through one `tokio::sync::broadcast` channel ([`AppState::emit`],
//! [`AppState::subscribe`]). A slow receiver loses the oldest events and gets
//! `RecvError::Lagged(n)`: [`recv`] counts and logs it and hands [`Recv::Lagged`] to the caller,
//! which must resynchronise from the database or the chain (the stream only drops frames; the
//! app refetches `/tickets` on reconnect). Events are notifications, never the only record of a
//! state change: whoever emits one has already written it to the DB or the chain.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};

use sqlx::SqlitePool;
use tokio::sync::{broadcast, watch};

use crate::api::types::{
    CashOut, FeedKind, LegStatus, OrderSide, Side, Ticket, TicketFill, TicketStatus,
};
use crate::catalogue::Catalogue;
use crate::config::Config;
use crate::now_ms;

pub use crate::pascal::{Pascal, pascal_client};

/// Ticket PDA, base58 (the id used by the API and the DB).
pub type TicketId = String;
/// Pascal symbol.
pub type Symbol = String;

// --- the bus ------------------------------------------------------------------------------------

/// Events between the desk's services. Payloads that are more than a few words sit behind an
/// `Arc`, so a broadcast clone stays cheap. Each variant says who emits it and who consumes it;
/// `/stream` maps some of them to C4 frames ([`crate::api::stream::frame_for`]).
#[derive(Debug, Clone)]
pub enum DeskEvent {
    /// A `/markets` pass replaced the catalogue cache.
    /// Emitted by: catalogue (1.E). Consumed by: quoter (2.A) to re-read tick and fee
    /// parameters. Stream: nothing.
    CatalogueRefreshed {
        /// `state_time_ms` of the list.
        as_of_ms: u64,
        /// NBA moneylines in the cache.
        markets: usize,
    },
    /// `open_pending` landed: the stake is escrowed and the desk may hedge.
    /// Emitted by: indexer (2.C) after writing the `tickets` row in state `pending`.
    /// Consumed by: hedge (2.B) re-quotes and sends the inception IOC (or does nothing when the
    /// floor fails); crank (2.C) arms the refund timer. Stream: `progress` / `escrowed`.
    PendingOpened(Arc<PendingOpened>),
    /// The inception hedge filled (fully or partly, `r_ppm` ≤ 1e6).
    /// Emitted by: hedge (2.B) after writing `pascal_fills` and `fill_allocations`.
    /// Consumed by: crank (2.C) sends `finalize(pending, f_u, r_ppm)`. Stream: `progress` /
    /// `hedging` with the fills.
    HedgeFilled(Arc<HedgeFilled>),
    /// The desk flattened the hedge of a pending or ticket (finalize rejected, r = 0, lost leg,
    /// cash-out). Emitted by: hedge (2.B). Consumed by: indexer / feed writers. Stream: nothing.
    HedgeUnwound(Arc<HedgeUnwound>),
    /// The program refused `finalize` (floor breach, expired). Emitted by: crank (2.C).
    /// Consumed by: hedge (2.B) unwinds reduce-only, then emits [`DeskEvent::RefundNeeded`].
    /// Stream: nothing.
    FinalizeRejected {
        /// Pending PDA.
        pending: String,
        /// Owner.
        user: String,
        /// Program error text.
        error: String,
    },
    /// A pending must be refunded (no fills, r = 0, finalize rejected and unwound).
    /// Emitted by: hedge (2.B), or crank (2.C) itself for a pending past `expires_at + 15 s`.
    /// Consumed by: crank (2.C) sends `refund_pending`. Stream: nothing (the refund's
    /// [`DeskEvent::PendingRefunded`] is).
    RefundNeeded {
        /// Pending PDA.
        pending: String,
        /// Owner.
        user: String,
        /// Why.
        reason: String,
    },
    /// `refund_pending` landed. Emitted by: indexer (2.C). Consumed by: hedge (2.B) drops the
    /// pending. Stream: `progress` / `refunded`.
    PendingRefunded {
        /// Pending PDA.
        pending: String,
        /// Ticket PDA the pending would have minted.
        ticket: TicketId,
        /// Owner.
        user: String,
        /// Transaction signature.
        tx: String,
    },
    /// `finalize` landed: the ticket exists. Emitted by: indexer (2.C) after writing the row.
    /// Consumed by: hedge (2.B) registers the ticket's targets. Stream: `progress` / `minted`.
    TicketMinted {
        /// Pending PDA.
        pending: String,
        /// Ticket PDA.
        ticket: TicketId,
        /// Owner.
        user: String,
        /// Transaction signature.
        tx: String,
    },
    /// Pascal resolved a market (search poll, `market_update`, resolved sweep; D-012).
    /// Emitted by: crank's resolution poller (2.C). Consumed by: crank (2.C) sends
    /// `resolve_leg` for every open leg on the symbol; hedge (2.B) may act before the chain
    /// confirms. Stream: nothing.
    MarketResolved {
        /// Symbol.
        symbol: Symbol,
        /// `resolution.price` (1e6 = long side won).
        resolution_u: u64,
        /// `market_resolution_ts_ms`.
        resolved_ms: u64,
    },
    /// `resolve_leg` landed for one ticket leg. Emitted by: indexer (2.C). Consumed by: hedge
    /// (2.B) rolls on won, unwinds on lost; crank (2.C) settles once every leg is in. Stream:
    /// via [`DeskEvent::TicketChanged`].
    LegResolved {
        /// Ticket PDA.
        ticket: TicketId,
        /// Owner.
        user: String,
        /// Leg index.
        leg_idx: u8,
        /// Symbol.
        symbol: Symbol,
        /// r written on-chain.
        resolution_u: u64,
        /// The leg's status after r̃.
        status: LegStatus,
        /// Transaction signature.
        tx: String,
    },
    /// `settle` landed. Emitted by: indexer (2.C). Consumed by: hedge (2.B) drops the ticket's
    /// targets. Stream: via [`DeskEvent::TicketChanged`].
    TicketSettled {
        /// Ticket PDA.
        ticket: TicketId,
        /// Owner.
        user: String,
        /// Amount paid.
        paid_u: u64,
        /// Transaction signature.
        tx: String,
    },
    /// `cash_out` landed. Emitted by: indexer (2.C). Consumed by: hedge (2.B) unwinds the
    /// ticket's share. Stream: via [`DeskEvent::TicketChanged`].
    TicketCashedOut {
        /// Ticket PDA.
        ticket: TicketId,
        /// Owner.
        user: String,
        /// Amount paid.
        c_u: u64,
        /// Transaction signature.
        tx: String,
    },
    /// A ticket's API view changed (any of the above, after the DB write).
    /// Emitted by: indexer / crank (2.C). Consumed by: stream only (`ticket` frame).
    TicketChanged(Arc<Ticket>),
    /// One money-flow ticker item. Emitted by: whichever service caused it, after inserting
    /// the `feed_events` row. Consumed by: stream (`feed` frame).
    Feed(Arc<FeedItem>),
    /// Liveness tick. Emitted by: the heartbeat task (1.E) every `heartbeat_every` (≤ 10 s).
    /// Consumed by: stream (`heartbeat` frame).
    Heartbeat {
        /// Unix ms.
        ts_ms: u64,
    },
}

impl DeskEvent {
    /// Short name for logs and metrics.
    pub fn kind(&self) -> &'static str {
        match self {
            DeskEvent::CatalogueRefreshed { .. } => "catalogue_refreshed",
            DeskEvent::PendingOpened(_) => "pending_opened",
            DeskEvent::HedgeFilled(_) => "hedge_filled",
            DeskEvent::HedgeUnwound(_) => "hedge_unwound",
            DeskEvent::FinalizeRejected { .. } => "finalize_rejected",
            DeskEvent::RefundNeeded { .. } => "refund_needed",
            DeskEvent::PendingRefunded { .. } => "pending_refunded",
            DeskEvent::TicketMinted { .. } => "ticket_minted",
            DeskEvent::MarketResolved { .. } => "market_resolved",
            DeskEvent::LegResolved { .. } => "leg_resolved",
            DeskEvent::TicketSettled { .. } => "ticket_settled",
            DeskEvent::TicketCashedOut { .. } => "ticket_cashed_out",
            DeskEvent::TicketChanged(_) => "ticket_changed",
            DeskEvent::Feed(_) => "feed",
            DeskEvent::Heartbeat { .. } => "heartbeat",
        }
    }

    /// The wallet the event concerns, for `/stream` subscriptions (`None` = everyone's).
    pub fn user(&self) -> Option<&str> {
        match self {
            DeskEvent::PendingOpened(p) => Some(&p.user),
            DeskEvent::HedgeFilled(h) => Some(&h.user),
            DeskEvent::HedgeUnwound(h) => Some(&h.user),
            DeskEvent::FinalizeRejected { user, .. }
            | DeskEvent::RefundNeeded { user, .. }
            | DeskEvent::PendingRefunded { user, .. }
            | DeskEvent::TicketMinted { user, .. }
            | DeskEvent::LegResolved { user, .. }
            | DeskEvent::TicketSettled { user, .. }
            | DeskEvent::TicketCashedOut { user, .. } => Some(user),
            DeskEvent::TicketChanged(t) => Some(&t.user),
            DeskEvent::CatalogueRefreshed { .. }
            | DeskEvent::MarketResolved { .. }
            | DeskEvent::Feed(_)
            | DeskEvent::Heartbeat { .. } => None,
        }
    }
}

/// Payload of [`DeskEvent::PendingOpened`]: the `Pending` account plus the strings its event
/// carries (symbols and game ids are hashed on-chain, D-008).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingOpened {
    /// Pending PDA.
    pub pending: String,
    /// The Ticket PDA it will mint (same user and nonce).
    pub ticket: TicketId,
    /// Owner.
    pub user: String,
    /// Nonce of the PDA seeds.
    pub nonce: u64,
    /// Escrowed stake.
    pub stake_u: u64,
    /// Escrowed copy fee (v1), 0 otherwise.
    pub copy_fee_u: u64,
    /// Offered odds in the signed quote.
    pub odds_offered_u: u64,
    /// The user's floor.
    pub min_payout_per_unit_u: u64,
    /// The Pending's expiry, Unix s.
    pub expires_at: i64,
    /// Legs in ticket order.
    pub legs: Vec<PendingLeg>,
    /// `open_pending` signature.
    pub tx: String,
    /// Slot it landed in.
    pub slot: u64,
}

/// One leg of a pending as signed in the quote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingLeg {
    /// Symbol.
    pub symbol: Symbol,
    /// Game id.
    pub game_id: String,
    /// Side.
    pub side: Side,
    /// `start_ts` × 1000.
    pub start_ms: u64,
    /// Ask-side quote price `p_i`.
    pub price_u: u64,
    /// Leg margin.
    pub m1_ppm: u32,
}

/// Payload of [`DeskEvent::HedgeFilled`]: what `finalize` needs (C3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HedgeFilled {
    /// Pending PDA.
    pub pending: String,
    /// Ticket PDA.
    pub ticket: TicketId,
    /// Owner.
    pub user: String,
    /// One per leg: achieved average fill price (rounded up), `0` = leg not hedged.
    pub f_u: Vec<u64>,
    /// Filled fraction of the hedged set, `(0, 1e6]`.
    pub r_ppm: u32,
    /// The fills, for the stream and the receipt.
    pub fills: Vec<TicketFill>,
}

/// Payload of [`DeskEvent::HedgeUnwound`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HedgeUnwound {
    /// Pending PDA when the unwind precedes a refund, else `None`.
    pub pending: Option<String>,
    /// Ticket PDA.
    pub ticket: TicketId,
    /// Owner.
    pub user: String,
    /// Why (`finalize_rejected`, `lost_leg`, `cash_out`, ...).
    pub reason: String,
    /// The unwinding fills.
    pub fills: Vec<TicketFill>,
}

/// A money-flow ticker item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedItem {
    /// Kind.
    pub kind: FeedKind,
    /// Display text.
    pub text: String,
    /// Ticket it concerns.
    pub ticket: Option<TicketId>,
    /// Unix ms.
    pub ts_ms: u64,
    /// Who placed it (handle, else a short address); `mint` and `copy` only (L7-1, 7.0c).
    pub who: Option<String>,
    /// Stake, micro-USDC; `mint` and `copy` only.
    pub amount_u: Option<u64>,
    /// The first leg's pick label; `mint` and `copy` only.
    pub team: Option<String>,
    /// The ticket's odds; `mint` and `copy` only.
    pub odds_u: Option<u64>,
}

/// What a bus receiver got.
#[derive(Debug)]
pub enum Recv {
    /// The next event.
    Event(DeskEvent),
    /// `n` events were dropped because this receiver was too slow; resynchronise.
    Lagged(u64),
    /// The bus is gone (shutdown).
    Closed,
}

/// Receives the next event for `consumer`, counting and logging lag.
pub async fn recv(rx: &mut broadcast::Receiver<DeskEvent>, consumer: &'static str) -> Recv {
    match rx.recv().await {
        Ok(e) => Recv::Event(e),
        Err(broadcast::error::RecvError::Lagged(n)) => {
            tracing::warn!(consumer, dropped = n, "bus receiver lagged");
            metrics::counter!("desk_bus_lagged_total", "consumer" => consumer).increment(n);
            Recv::Lagged(n)
        }
        Err(broadcast::error::RecvError::Closed) => Recv::Closed,
    }
}

// --- the section 5 internal state ---------------------------------------------------------------

/// A live order book (REST in the MVP, the `book` channel in v1). Written by the quoter (2.A).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveBook {
    /// Bids, best first.
    pub bids: Vec<pascal_rs::Level>,
    /// Asks, best first.
    pub asks: Vec<pascal_rs::Level>,
    /// Envelope `seq` of the response.
    pub seq: Option<u64>,
    /// Envelope `state_time_ms` (or receive time).
    pub as_of_ms: u64,
}

/// The desk's view of one ticket (pending or open) with per-leg targets. Written by the hedge
/// desk (2.B) from bus events; the DB row (2.C) stays the record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketState {
    /// Ticket PDA.
    pub ticket: TicketId,
    /// Pending PDA.
    pub pending: String,
    /// Owner.
    pub user: String,
    /// Lifecycle state.
    pub status: TicketStatus,
    /// Stake S.
    pub stake_u: u64,
    /// r·S once finalized.
    pub rs_u: Option<u64>,
    /// Payout P.
    pub payout_u: u64,
    /// Legs.
    pub legs: Vec<LegState>,
}

/// One leg of a [`TicketState`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegState {
    /// Index.
    pub idx: u8,
    /// Symbol.
    pub symbol: Symbol,
    /// Game id.
    pub game_id: String,
    /// Side.
    pub side: Side,
    /// Expected start, Unix ms.
    pub start_ms: u64,
    /// Quote price.
    pub p_quote_u: u64,
    /// Fill price once hedged.
    pub p_fill_u: Option<u64>,
    /// Leg margin.
    pub m1_ppm: u32,
    /// Signed contracts the desk wants held for this leg now (Tᵢ, section 3).
    pub target: i64,
    /// Status.
    pub status: LegStatus,
}

/// What one outbound order serves (`order_map`: `client_order_id` → this), so its fills can be
/// split pro-rata into `fill_allocations`. Written by the hedge desk (2.B).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderRef {
    /// Symbol.
    pub symbol: Symbol,
    /// Side.
    pub side: OrderSide,
    /// Contracts asked.
    pub size: u64,
    /// Limit price.
    pub limit_u: u64,
    /// Unix ms sent.
    pub placed_ms: u64,
    /// Ticket legs served, with the contracts each asked for.
    pub serves: Vec<Allocation>,
}

/// One ticket leg's share of an order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Allocation {
    /// Ticket PDA.
    pub ticket: TicketId,
    /// Leg index.
    pub leg_idx: u8,
    /// Signed contracts.
    pub contracts: i64,
}

/// Live cash-out valuation, provided by the quoter (2.A) and asked by `/tickets` (2.C) without
/// a bus round trip. Unset until the quoter registers itself in [`AppState::cash_out`].
pub trait CashOutValuer: Send + Sync {
    /// The indicative cash-out of `ticket` from the cached books (no I/O, no await).
    fn cash_out(&self, ticket: &TicketState) -> CashOut;
}

/// An `RwLock` whose guards ignore poisoning (a panicked writer leaves plain data behind, never
/// a broken invariant across maps, since nothing spans two locks).
#[derive(Debug, Default)]
pub struct Locked<T>(RwLock<T>);

impl<T> Locked<T> {
    /// Wraps `value`.
    pub fn new(value: T) -> Self {
        Locked(RwLock::new(value))
    }

    /// Shared access. Do not hold across `.await`.
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        self.0.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Exclusive access. Do not hold across `.await`.
    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        self.0.write().unwrap_or_else(|e| e.into_inner())
    }
}

// --- AppState -----------------------------------------------------------------------------------

/// Cheap-to-clone handle on the shared state; derefs to [`Shared`].
#[derive(Clone)]
pub struct AppState(Arc<Shared>);

impl std::ops::Deref for AppState {
    type Target = Shared;

    fn deref(&self) -> &Shared {
        &self.0
    }
}

/// The shared state. Fields are added by L2 steps, never changed.
pub struct Shared {
    /// Configuration.
    pub config: Config,
    /// Pascal (or the mock).
    pub pascal: Pascal,
    /// SQLite pool.
    pub db: SqlitePool,
    /// Latest good `/markets` pass, NBA moneylines only (catalogue). `None` until the first.
    pub catalogue: Locked<Option<Arc<Catalogue>>>,
    /// `book`: live books by symbol (quoter, 2.A).
    pub books: Locked<HashMap<Symbol, LiveBook>>,
    /// `tickets`: per-ticket deltas and status (hedge, 2.B).
    pub tickets: Locked<HashMap<TicketId, TicketState>>,
    /// `held`: the hedger's net signed position per symbol, reconciled every 30 s (hedge, 2.B).
    pub held: Locked<HashMap<Symbol, i64>>,
    /// `order_map`: `client_order_id` → what the order serves (hedge, 2.B).
    pub order_map: Locked<HashMap<u64, OrderRef>>,
    /// Desk-wide `client_order_id` sequence ([`AppState::next_client_order_id`]).
    client_order_ids: AtomicU64,
    /// Stop minting (Pascal down, hedger under-collateralised; spec 12 runbook).
    pub paused: AtomicBool,
    /// Cash-out valuation registered by the quoter (2.A).
    pub cash_out: OnceLock<Arc<dyn CashOutValuer>>,
    /// The quoter's book cache, back-off and signer (2.A).
    pub quoter: Arc<crate::quoter::QuoterState>,
    /// The running hedge desk, registered by [`crate::hedge::run`] (weak: no cycle), read by
    /// the `dev`/`demo` route `GET /debug/hedge` (3.A).
    pub hedge: OnceLock<std::sync::Weak<crate::hedge::Inner>>,
    /// Outcome sentiment: the followed trade tape (5.0c, L5-4). Its lock is never held with
    /// another one.
    pub sentiment: Arc<crate::sentiment::Sentiment>,
    /// `PUT /profile` write limits per IP and per wallet (6.0c). Its lock is never held with
    /// another one.
    pub profiles: Arc<crate::profiles::Limiter>,
    /// The cached SOL/USD price `/fx` serves (7.0c). Its lock is never held with another one.
    pub sol: Arc<crate::fx::sol::SolCache>,
    /// Live scores and finished games (31 Oct). Its lock is never held with another one.
    pub scores: Arc<crate::scores::ScoreCache>,
    /// The sponsored-transaction relay (L8), decided once by [`crate::relay::mount`]. Its limits'
    /// lock is never held with another one.
    pub relay: OnceLock<crate::relay::Status>,
    bus: broadcast::Sender<DeskEvent>,
    shutdown: watch::Sender<bool>,
}

impl AppState {
    /// Builds the state around an open pool and a Pascal handle.
    pub fn new(config: Config, pascal: Pascal, db: SqlitePool) -> Self {
        let (bus, _) = broadcast::channel(config.bus_capacity.max(16));
        let (shutdown, _) = watch::channel(false);
        AppState(Arc::new(Shared {
            config,
            pascal,
            db,
            catalogue: Locked::default(),
            books: Locked::default(),
            tickets: Locked::default(),
            held: Locked::default(),
            order_map: Locked::default(),
            // Unique across restarts without a DB read: ms clock × 1000 leaves room for 1,000
            // orders per ms of downtime.
            client_order_ids: AtomicU64::new(now_ms().saturating_mul(1000)),
            paused: AtomicBool::new(false),
            cash_out: OnceLock::new(),
            quoter: Arc::default(),
            hedge: OnceLock::new(),
            sentiment: Arc::default(),
            profiles: Arc::default(),
            sol: Arc::default(),
            scores: Arc::default(),
            relay: OnceLock::new(),
            bus,
            shutdown,
        }))
    }

    /// Publishes `event` to every subscriber (none listening is fine).
    pub fn emit(&self, event: DeskEvent) {
        if !matches!(event, DeskEvent::Heartbeat { .. }) {
            tracing::info!(event = event.kind(), user = event.user(), detail = ?event, "bus");
        }
        metrics::counter!("desk_bus_events_total", "event" => event.kind()).increment(1);
        let _ = self.bus.send(event);
    }

    /// A new receiver, seeing events emitted from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<DeskEvent> {
        self.bus.subscribe()
    }

    /// The latest catalogue, if one has loaded.
    pub fn catalogue(&self) -> Option<Arc<Catalogue>> {
        self.catalogue.read().clone()
    }

    /// The next desk-wide `client_order_id` (section 1 idempotency).
    pub fn next_client_order_id(&self) -> u64 {
        self.client_order_ids.fetch_add(1, Ordering::Relaxed)
    }

    /// Whether minting is paused.
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// The quoter's cash-out value for `ticket`, or unavailable while no quoter is registered.
    pub fn cash_out_for(&self, ticket: &TicketState) -> CashOut {
        match self.cash_out.get() {
            Some(v) => v.cash_out(ticket),
            None => CashOut::unavailable("quoter not running"),
        }
    }

    /// Asks every service to stop (idempotent).
    pub fn shutdown(&self) {
        self.shutdown.send_replace(true);
    }

    /// Whether shutdown has been asked for.
    pub fn is_shutting_down(&self) -> bool {
        *self.shutdown.borrow()
    }

    /// Resolves once shutdown has been asked for. Services `select!` on it.
    pub async fn shutdown_requested(&self) {
        let mut rx = self.shutdown.subscribe();
        // `wait_for` checks the current value first, so a late subscriber does not hang.
        let _ = rx.wait_for(|stop| *stop).await;
    }
}
