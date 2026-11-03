//! The mock's internal event bus: every state change, plus the ready-made payloads of the
//! WebSocket channels (`account` deltas, `market_update` entries).
//!
//! The engine publishes on a `tokio::sync::broadcast` channel while holding its lock, so events
//! arrive in `seq` order. Fine-grained events ([`MockEvent::Fill`], [`MockEvent::Order`], …) go
//! out as they happen; [`MockEvent::AccountUpdate`] coalesces one request's changes per owner and
//! goes out when the request releases the engine (`Engine::flush`). A lagging subscriber loses
//! events (`RecvError::Lagged`) and should resynchronise from `GET /api/v1/accounts/{owner}`
//! (the `/ws` session does this by sending fresh snapshots).

use pascal_rs::{
    AccountState, Fill, MarketResolution, MarketUpdateType, OrderUpdate, PositionResolution,
};
use serde_json::Value;

/// One state change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockEvent {
    /// A fill on `owner`'s account (taker or maker side).
    Fill {
        /// Account owner.
        owner: String,
        /// The fill, exactly as returned by `POST /orders`.
        fill: Fill,
    },
    /// An order was placed, changed, or closed.
    Order {
        /// Account owner.
        owner: String,
        /// Order state and reason.
        update: OrderUpdate,
    },
    /// A market resolved (scripted, control endpoint, or seen on the tape).
    MarketResolved {
        /// Market.
        symbol: String,
        /// Resolution.
        resolution: MarketResolution,
    },
    /// A position was settled by a resolution.
    PositionResolved {
        /// Account owner.
        owner: String,
        /// Settlement.
        resolution: PositionResolution,
    },
    /// Builder fees were credited to `builder`.
    BuilderFee {
        /// Builder owner.
        builder: String,
        /// Amount, micro-USD.
        amount_u: i64,
        /// Seq of the fill that paid it.
        seq: u64,
    },
    /// One request's changes to `owner`'s account, as the `account` channel's `update` frame
    /// carries them: `collateral_usd`, changed `positions` (size 0 when closed), changed
    /// `orders` and `order_updates`, `fills`, `resolutions`, `lifetime_stats` after fills,
    /// `rewards` after builder fees.
    AccountUpdate {
        /// Account owner.
        owner: String,
        /// The delta (`data` of the frame).
        update: Box<AccountState>,
        /// Envelope seq after the change.
        seq: u64,
        /// Mock clock, Unix ms.
        time_ms: u64,
    },
    /// One entry of the `market_update` channel.
    MarketUpdate {
        /// Why.
        update_type: MarketUpdateType,
        /// The market as served (with `resolution`; no `stats`, `best_bid`, `best_ask`).
        market: Value,
        /// Envelope seq.
        seq: u64,
        /// Mock clock, Unix ms.
        time_ms: u64,
    },
}
