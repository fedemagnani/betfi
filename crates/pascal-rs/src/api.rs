//! [`PascalApi`]: the one handle every other layer uses to talk to Pascal (or to
//! `pascal-mock`, which implements the same trait), and [`LiveClient`], its implementation on
//! the real read API, a configurable write API and the market-data socket.
//!
//! The trait is object-safe and used as `Arc<dyn PascalApi>`. It is written with
//! [`async_trait`]; an implementor writes `#[async_trait::async_trait] impl PascalApi for …`
//! with plain `async fn`s. The read methods carry the names, arguments and return types of
//! [`Client`]'s methods (symbol lists are `&[String]`, since a trait object cannot take the
//! generic `&[S]`).

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_trait::async_trait;
use futures_util::Stream;
use tokio::sync::mpsc;

use crate::envelope::{Lenient, Page, Response};
use crate::error::Result;
use crate::rest::{AccountQuery, Client, MarketSearch, PageParams, ResolvedQuery};
use crate::signing::{CancelOrderRequest, MassCancelRequest, PlaceOrderRequest};
use crate::trade::{MassCancelled, PlacedOrder, TradeClient, WriteResult};
use crate::types::{AccountState, Books, BuilderStats, Fill, Market, Order};
use crate::ws::{Channel, WsClient, WsConfig, WsEvent};

/// A stream of socket events: `WsEvent::Frame` carries the typed [`crate::WsFrame`]s, the
/// other variants report connects, drops and decode errors. Dropping the stream closes the
/// subscription.
pub type WsStream = Pin<Box<dyn Stream<Item = WsEvent> + Send>>;

/// Everything Schedina needs from Pascal, real or mocked.
#[async_trait]
pub trait PascalApi: Send + Sync {
    /// `GET /api/v1/markets` (rate-limited to one call per 30 s; cache it).
    async fn markets(&self) -> Result<Response<Vec<Market>>>;

    /// `GET /api/v1/markets`, decoded market by market. The default wraps
    /// [`PascalApi::markets`].
    async fn markets_lenient(&self) -> Result<Response<Lenient<Market>>> {
        Ok(self.markets().await?.map(all_decoded))
    }

    /// `POST /api/v1/markets/search` (exact symbols / events / games, resolved included).
    async fn search_markets(&self, body: &MarketSearch) -> Result<Response<Vec<Market>>>;

    /// `POST /api/v1/markets/search`, decoded market by market. The default wraps
    /// [`PascalApi::search_markets`].
    async fn search_markets_lenient(
        &self,
        body: &MarketSearch,
    ) -> Result<Response<Lenient<Market>>> {
        Ok(self.search_markets(body).await?.map(all_decoded))
    }

    /// `GET /api/v1/markets/resolved`.
    async fn resolved_markets(&self, q: ResolvedQuery) -> Result<Response<Vec<Market>>>;

    /// `GET /api/v1/markets/resolved`, decoded market by market. The default wraps
    /// [`PascalApi::resolved_markets`].
    async fn resolved_markets_lenient(
        &self,
        q: ResolvedQuery,
    ) -> Result<Response<Lenient<Market>>> {
        Ok(self.resolved_markets(q).await?.map(all_decoded))
    }

    /// `GET /api/v1/books` for 1..=50 symbols.
    async fn books(&self, symbols: &[String]) -> Result<Response<Books>>;

    /// `GET /api/v1/accounts/{owner}`.
    async fn account(&self, owner: &str, q: &AccountQuery) -> Result<Response<AccountState>>;

    /// `GET /api/v1/accounts/{owner}/fills`: one page, newest first.
    async fn fills(
        &self,
        owner: &str,
        symbols: &[String],
        page: &PageParams,
    ) -> Result<Response<Page<Fill>>>;

    /// `GET /api/v1/accounts/{owner}/builder-stats`.
    async fn builder_stats(&self, owner: &str) -> Result<Response<BuilderStats>>;

    /// `POST /api/v1/orders`: 1..=50 signed orders, one result per order, in order.
    async fn place_orders(
        &self,
        orders: &[PlaceOrderRequest],
    ) -> Result<Vec<WriteResult<PlacedOrder>>>;

    /// `POST /api/v1/cancels`: 1..=50 signed cancels.
    async fn cancel(&self, cancels: &[CancelOrderRequest]) -> Result<Vec<WriteResult<Order>>>;

    /// `POST /api/v1/mass-cancels`: 1..=10 signed mass cancels, one market each.
    async fn mass_cancel(
        &self,
        requests: &[MassCancelRequest],
    ) -> Result<Vec<WriteResult<MassCancelled>>>;

    /// Subscribes to market-data channels (`book`, `account`, `market_update`, …). The stream
    /// reconnects and resubscribes by itself; drop it to unsubscribe.
    async fn subscribe(&self, channels: Vec<Channel>) -> Result<WsStream>;
}

fn all_decoded(v: Vec<Market>) -> Lenient<Market> {
    Lenient {
        items: v.into_iter().enumerate().collect(),
        failures: Vec::new(),
    }
}

/// [`PascalApi`] over HTTP and WebSocket: Pascal's read API, the write API at the trade
/// client's base URL (Pascal or `pascal-mock`), and the market-data socket.
#[derive(Debug, Clone)]
pub struct LiveClient {
    /// Read client (`PASCAL_DATA_URL`).
    pub read: Client,
    /// Write client (`PASCAL_TRADE_URL`).
    pub trade: TradeClient,
    /// Socket settings (`PASCAL_WS_URL` in `url`); each [`PascalApi::subscribe`] opens one
    /// connection with them.
    pub ws: WsConfig,
}

impl LiveClient {
    /// Bundles the three clients.
    pub fn new(read: Client, trade: TradeClient, ws: WsConfig) -> Self {
        LiveClient { read, trade, ws }
    }

    /// As a shared trait object.
    pub fn into_dyn(self) -> Arc<dyn PascalApi> {
        Arc::new(self)
    }
}

#[async_trait]
impl PascalApi for LiveClient {
    async fn markets(&self) -> Result<Response<Vec<Market>>> {
        self.read.markets().await
    }

    async fn markets_lenient(&self) -> Result<Response<Lenient<Market>>> {
        self.read.markets_lenient().await
    }

    async fn search_markets(&self, body: &MarketSearch) -> Result<Response<Vec<Market>>> {
        self.read.search_markets(body).await
    }

    async fn search_markets_lenient(
        &self,
        body: &MarketSearch,
    ) -> Result<Response<Lenient<Market>>> {
        self.read.search_markets_lenient(body).await
    }

    async fn resolved_markets(&self, q: ResolvedQuery) -> Result<Response<Vec<Market>>> {
        self.read.resolved_markets(q).await
    }

    async fn resolved_markets_lenient(
        &self,
        q: ResolvedQuery,
    ) -> Result<Response<Lenient<Market>>> {
        self.read.resolved_markets_lenient(q).await
    }

    async fn books(&self, symbols: &[String]) -> Result<Response<Books>> {
        self.read.books(symbols).await
    }

    async fn account(&self, owner: &str, q: &AccountQuery) -> Result<Response<AccountState>> {
        self.read.account(owner, q).await
    }

    async fn fills(
        &self,
        owner: &str,
        symbols: &[String],
        page: &PageParams,
    ) -> Result<Response<Page<Fill>>> {
        self.read.fills(owner, symbols, page).await
    }

    async fn builder_stats(&self, owner: &str) -> Result<Response<BuilderStats>> {
        self.read.builder_stats(owner).await
    }

    async fn place_orders(
        &self,
        orders: &[PlaceOrderRequest],
    ) -> Result<Vec<WriteResult<PlacedOrder>>> {
        self.trade.place_orders(orders).await
    }

    async fn cancel(&self, cancels: &[CancelOrderRequest]) -> Result<Vec<WriteResult<Order>>> {
        self.trade.cancel(cancels).await
    }

    async fn mass_cancel(
        &self,
        requests: &[MassCancelRequest],
    ) -> Result<Vec<WriteResult<MassCancelled>>> {
        self.trade.mass_cancel(requests).await
    }

    async fn subscribe(&self, channels: Vec<Channel>) -> Result<WsStream> {
        let (client, events) = WsClient::spawn(self.ws.clone(), channels);
        Ok(Box::pin(WsEvents {
            _client: client,
            events,
        }))
    }
}

/// Owns the socket task (dropping it shuts the task down) and yields its events.
struct WsEvents {
    _client: WsClient,
    events: mpsc::Receiver<WsEvent>,
}

impl Stream for WsEvents {
    type Item = WsEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<WsEvent>> {
        self.events.poll_recv(cx)
    }
}
