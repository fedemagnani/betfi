//! The desk's one Pascal handle (l1-contracts C5): `Arc<dyn PascalApi>`.
//!
//! Reads go to `PASCAL_DATA_URL`, the socket to `PASCAL_WS_URL`. Writes go to `PASCAL_TRADE_URL`
//! when it is set; on `dev`/`demo` it defaults to the data URL, because the mock serves
//! `/orders` next to its read API. On `live` with no `PASCAL_TRADE_URL`, the handle is
//! [`ReadOnly`]: every write fails locally and nothing leaves the process (D-007, mode M).

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_trait::async_trait;
use futures_util::Stream;
use pascal_rs::{
    AccountQuery, AccountState, Books, BuilderStats, CancelOrderRequest, Channel, Client, Error,
    Fill, Lenient, LiveClient, Market, MarketSearch, MassCancelRequest, MassCancelled, Order, Page,
    PageParams, PascalApi, PlaceOrderRequest, PlacedOrder, ResolvedQuery, Response, Result,
    TradeClient, WriteResult, WsClient, WsConfig, WsEvent, WsStream,
};
use tokio::sync::mpsc;

use crate::config::{Config, Profile};

/// The one Pascal handle type. Every Pascal call in the desk goes through it.
pub type Pascal = Arc<dyn PascalApi>;

/// The trade URL the desk writes to, if any: `PASCAL_TRADE_URL`, else the data URL on
/// `dev`/`demo` (the mock), else none (`live` is read-only unless a trade URL is configured).
///
/// The data-URL fallback never applies when the data URL is a Pascal host (D-143): `demo` pointed
/// at `https://data.pascal.trade` for real books must not POST signed orders to Pascal. The
/// handle is then read-only, like `live` without a trade URL. Only an explicit
/// `PASCAL_TRADE_URL` can name a Pascal host.
pub fn trade_url(config: &Config) -> Option<&str> {
    match (&config.pascal_trade_url, config.profile) {
        (Some(u), _) => Some(u.as_str()),
        (None, Profile::Dev | Profile::Demo) if !is_pascal_host(&config.pascal_data_url) => {
            Some(config.pascal_data_url.as_str())
        }
        (None, _) => None,
    }
}

/// Whether `url` is Pascal's domain or one of its subdomains. An unparseable URL counts as a
/// Pascal host, so the fallback fails closed.
pub fn is_pascal_host(url: &str) -> bool {
    match reqwest::Url::parse(url.trim()) {
        Ok(u) => u.host_str().is_none_or(|h| {
            let h = h.trim_end_matches('.').to_ascii_lowercase();
            h == "pascal.trade" || h.ends_with(".pascal.trade")
        }),
        Err(_) => true,
    }
}

/// Builds the [`Pascal`] handle from the desk's configuration.
pub fn pascal_client(config: &Config) -> Result<Pascal> {
    let read = Client::builder()
        .base_url(&config.pascal_data_url)
        .user_agent(concat!("schedina-desk/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let ws = WsConfig {
        url: config.pascal_ws_url.clone(),
        ..WsConfig::default()
    };
    Ok(match trade_url(config) {
        Some(url) => LiveClient::new(read, TradeClient::builder(url).build()?, ws).into_dyn(),
        None => Arc::new(ReadOnly { read, ws }),
    })
}

/// A Pascal handle without a write client: reads and the socket work, writes fail with
/// [`Error::InvalidArgument`] before any I/O.
pub struct ReadOnly {
    read: Client,
    ws: WsConfig,
}

fn writes_disabled<T>() -> Result<T> {
    Err(Error::InvalidArgument(
        "Pascal writes are disabled: no PASCAL_TRADE_URL on the live profile (D-007)".to_owned(),
    ))
}

#[async_trait]
impl PascalApi for ReadOnly {
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
        _orders: &[PlaceOrderRequest],
    ) -> Result<Vec<WriteResult<PlacedOrder>>> {
        writes_disabled()
    }
    async fn cancel(&self, _cancels: &[CancelOrderRequest]) -> Result<Vec<WriteResult<Order>>> {
        writes_disabled()
    }
    async fn mass_cancel(
        &self,
        _requests: &[MassCancelRequest],
    ) -> Result<Vec<WriteResult<MassCancelled>>> {
        writes_disabled()
    }
    async fn subscribe(&self, channels: Vec<Channel>) -> Result<WsStream> {
        let (client, events) = WsClient::spawn(self.ws.clone(), channels);
        Ok(Box::pin(WsEvents {
            _client: client,
            events,
        }))
    }
}

/// Same as `LiveClient`'s stream: owns the socket task (dropping it shuts the task down) and
/// yields its events.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn live_without_trade_url_refuses_writes_locally() {
        let mut config = Config::local("http://127.0.0.1:9", ":memory:");
        config.profile = Profile::Live;
        config.pascal_trade_url = None;
        assert_eq!(trade_url(&config), None);
        let p = pascal_client(&config).unwrap();
        let err = p.place_orders(&[]).await.unwrap_err();
        assert!(matches!(err, Error::InvalidArgument(_)), "{err:?}");
    }

    #[test]
    fn dev_and_demo_write_to_the_mock() {
        let mut config = Config::local("http://127.0.0.1:9000", ":memory:");
        for profile in [Profile::Dev, Profile::Demo] {
            config.profile = profile;
            config.pascal_trade_url = None;
            assert_eq!(trade_url(&config), Some("http://127.0.0.1:9000"));
        }
        config.pascal_trade_url = Some("http://127.0.0.1:9100".to_owned());
        assert_eq!(trade_url(&config), Some("http://127.0.0.1:9100"));
    }

    /// D-143: dev/demo reading real Pascal data never fall back to writing there.
    #[tokio::test]
    async fn dev_and_demo_on_pascal_data_are_read_only() {
        for data in [
            "https://data.pascal.trade",
            "https://DATA.Pascal.Trade./",
            "https://pascal.trade",
            "wss://anything.pascal.trade/ws",
            "not a url",
        ] {
            for profile in [Profile::Dev, Profile::Demo] {
                let mut config = Config::local("http://127.0.0.1:9", ":memory:");
                config.pascal_data_url = data.to_owned();
                config.profile = profile;
                config.pascal_trade_url = None;
                assert_eq!(trade_url(&config), None, "{data} on {profile:?}");
            }
        }
        // Look-alike hosts are not Pascal.
        for other in [
            "https://pascal.trade.evil.example",
            "https://notpascal.trade",
        ] {
            assert!(!is_pascal_host(other), "{other}");
        }
        // And the handle refuses the write before any I/O.
        let mut config = Config::local("http://127.0.0.1:9", ":memory:");
        config.pascal_data_url = "https://data.pascal.trade".to_owned();
        config.profile = Profile::Demo;
        let p = pascal_client(&config).unwrap();
        let err = p.place_orders(&[]).await.unwrap_err();
        assert!(matches!(err, Error::InvalidArgument(_)), "{err:?}");
    }
}
