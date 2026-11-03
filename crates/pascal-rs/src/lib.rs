//! Typed client for the Pascal exchange API: reads, market data, and signed writes.
//!
//! Pascal's read API (`https://data.pascal.trade`) and market-data socket
//! (`wss://data.pascal.trade/ws`) are public and unauthenticated. This crate wraps them, and
//! the signed write API, for the Schedina desk, crank and mock:
//!
//! - [`PascalApi`]: the object-safe trait (`Arc<dyn PascalApi>`) the desk talks to, with
//!   [`LiveClient`] (read + trade + socket) as its real implementation; `pascal-mock`
//!   implements it too.
//! - [`signing`]: permits and Ed25519 signatures for place-order (with builder code), cancel
//!   and mass-cancel, plus verification of signed requests ([`TradingSigner`]).
//! - [`TradeClient`]: `POST /orders`, `/cancels`, `/mass-cancels` on an explicit base URL (no
//!   default: in mode M it is the mock, decision D-007).
//! - [`tick`]: the per-market price grid (`tick_size_min`, `tick_sig_figs`) and snapping.
//!
//! - [`Client`]: one method per Pascal REST endpoint read here, returning [`Response<T>`] —
//!   the typed payload, the envelope's `seq` / `state_time_ms` / `server_time_ms`, and the raw
//!   body as a [`serde_json::Value`] for raw `jsonb` storage. Errors map Pascal's error envelope
//!   to [`ApiError`] with its `code`.
//! - [`Pager`]: `limit` + `before_cursor` pagination from `next_cursor`.
//! - [`ratelimit`]: a token bucket per host (and a slow one for the 5 MB markets list), plus
//!   exponential backoff with jitter on `429 RATE_LIMITED` and `503 HISTORY_NOT_READY`.
//! - [`WsClient`]: subscribe / unsubscribe, ping every 30 s, reconnect with backoff and
//!   resubscribe, typed [`WsFrame`]s for `all_trades`, `account`, `market_update`, `book`.
//! - [`num`]: Pascal's 6-d.p. decimal strings as micro-USD `i64` ([`MicroUsd`]).
//!
//! ```no_run
//! # async fn demo() -> pascal_rs::Result<()> {
//! let client = pascal_rs::Client::new()?;
//! let trades = client.trades(&["ISRAEL_PM_2026.NETANYAHU"], &pascal_rs::PageParams::limit(50)).await?;
//! for (trade, raw) in trades.data.items.iter().zip(trades.raw_items()) {
//!     println!("{} {} @ {} micro-USD (seq {}) raw={raw}", trade.symbol, trade.size, trade.price, trade.seq);
//! }
//! # Ok(()) }
//! ```

pub mod api;
pub mod envelope;
pub mod error;
pub mod num;
pub mod ratelimit;
pub mod rest;
pub mod signing;
pub mod tick;
pub mod trade;
pub mod types;
pub mod ws;

pub use api::{LiveClient, PascalApi, WsStream};
pub use envelope::{
    ItemFailure, Lenient, Page, Response, decode_lenient_response, decode_response,
};
pub use error::{ApiError, ApiErrorCode, Error, PartialError, Result};
pub use num::{MicroUsd, format_micros, parse_micros};
pub use rest::{
    AccountQuery, Client, ClientBuilder, Lookback, MarketSearch, PageParams, Pager,
    PositionInstancesQuery, PositionOrderBy, ResolvedOrderBy, ResolvedQuery, SortOrder,
};
pub use signing::{
    AuthParams, BuilderInfo, CancelOrder, CancelOrderRequest, MassCancel, MassCancelRequest,
    PermitError, PlaceOrder, PlaceOrderRequest, RequestAuth, Signed, TradingSigner, Verified,
};
pub use tick::{Rounding, TickError, TickRule};
pub use trade::{
    BuilderPolicy, MassCancelled, PlacedOrder, TradeClient, TradeClientBuilder, WriteResult,
};
pub use types::*;
pub use ws::{Channel, FrameKind, WsClient, WsConfig, WsEvent, WsFrame, WsMessage};
