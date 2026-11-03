//! Typed REST client for `https://data.pascal.trade`.

use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use url::Url;

use crate::envelope::{
    Lenient, Page, Response, decode_lenient_response, decode_response, decode_with,
};
use crate::error::{ApiErrorCode, Error, PartialError, Result};
use crate::ratelimit::{RateLimiter, RateLimits, RetryPolicy};
use crate::types::{
    AccountBio, AccountHistory, AccountState, Books, BuilderStats, DepositAddress, Fill, Market,
    PositionHistory, PublicTrade, ServerTime, TradingKey, Transfer, Usernames,
};

/// Production read API.
pub const DEFAULT_BASE_URL: &str = "https://data.pascal.trade";

/// Maximum symbols per `/api/v1/trades` request.
pub const MAX_TRADES_SYMBOLS: usize = 120;
/// Maximum symbols per `/api/v1/books` request.
pub const MAX_BOOKS_SYMBOLS: usize = 50;
/// Maximum owners per `/api/v1/usernames` request.
pub const MAX_USERNAMES_OWNERS: usize = 100;
/// Maximum symbols in an account `symbols` filter (fills, position instances).
pub const MAX_FILTER_SYMBOLS: usize = 100;

/// Builder for [`Client`].
#[derive(Debug, Clone)]
pub struct ClientBuilder {
    base_url: String,
    timeout: Duration,
    user_agent: String,
    limiter: Option<Arc<RateLimiter>>,
    limits: RateLimits,
    retry: RetryPolicy,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        ClientBuilder {
            base_url: DEFAULT_BASE_URL.to_owned(),
            timeout: Duration::from_secs(60),
            user_agent: concat!("pascal-rs/", env!("CARGO_PKG_VERSION")).to_owned(),
            limiter: None,
            limits: RateLimits::default(),
            retry: RetryPolicy::default(),
        }
    }
}

impl ClientBuilder {
    /// Base URL (default [`DEFAULT_BASE_URL`]).
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Per-request timeout (default 60 s; the markets list is ~5 MB).
    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    /// `User-Agent` header.
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = ua.into();
        self
    }

    /// Rate limits for a limiter owned by this client (ignored when [`Self::rate_limiter`] is set).
    pub fn rate_limits(mut self, limits: RateLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Share an existing limiter, so several clients respect one per-host budget.
    pub fn rate_limiter(mut self, limiter: Arc<RateLimiter>) -> Self {
        self.limiter = Some(limiter);
        self
    }

    /// Retry policy for 429, 503 `HISTORY_NOT_READY` and other transient failures.
    pub fn retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Builds the client.
    pub fn build(self) -> Result<Client> {
        let base = Url::parse(&self.base_url)
            .map_err(|e| Error::InvalidArgument(format!("base url {:?}: {e}", self.base_url)))?;
        let host = base
            .host_str()
            .ok_or_else(|| Error::InvalidArgument(format!("base url {base} has no host")))?
            .to_owned();
        let http = reqwest::Client::builder()
            .timeout(self.timeout)
            .user_agent(self.user_agent)
            .gzip(true)
            .build()?;
        Ok(Client {
            inner: Arc::new(Inner {
                http,
                base,
                host,
                limiter: self
                    .limiter
                    .unwrap_or_else(|| Arc::new(RateLimiter::new(self.limits))),
                retry: self.retry,
            }),
        })
    }
}

/// Read client. Cheap to clone; clones share the HTTP pool and the rate limiter.
#[derive(Debug, Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    http: reqwest::Client,
    base: Url,
    host: String,
    limiter: Arc<RateLimiter>,
    retry: RetryPolicy,
}

/// Query parameters of a cursor-paginated endpoint. At most one of `before_cursor` and
/// `at_or_before_seq` may be set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageParams {
    /// Page size (1..=500 for most endpoints, default 100).
    pub limit: Option<u32>,
    /// The `next_cursor` of the previous page.
    pub before_cursor: Option<String>,
    /// Start at this sequence (inclusive), then continue with cursors.
    pub at_or_before_seq: Option<u64>,
}

impl PageParams {
    /// First page with the given size.
    pub fn limit(limit: u32) -> Self {
        PageParams {
            limit: Some(limit),
            ..Self::default()
        }
    }

    fn push(&self, q: &mut Vec<(String, String)>) -> Result<()> {
        if self.before_cursor.is_some() && self.at_or_before_seq.is_some() {
            return Err(Error::InvalidArgument(
                "set at most one of before_cursor and at_or_before_seq".into(),
            ));
        }
        if let Some(l) = self.limit {
            q.push(("limit".into(), l.to_string()));
        }
        if let Some(c) = &self.before_cursor {
            q.push(("before_cursor".into(), c.clone()));
        }
        if let Some(s) = self.at_or_before_seq {
            q.push(("at_or_before_seq".into(), s.to_string()));
        }
        Ok(())
    }
}

/// `lookback` of `/api/v1/markets/resolved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lookback {
    /// 4 hours.
    Hours4,
    /// 24 hours (the default).
    #[default]
    Hours24,
    /// 3 days.
    Days3,
}

impl Lookback {
    fn as_str(self) -> &'static str {
        match self {
            Lookback::Hours4 => "4h",
            Lookback::Hours24 => "24h",
            Lookback::Days3 => "3d",
        }
    }
}

/// `order_by` of `/api/v1/markets/resolved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResolvedOrderBy {
    /// Latest resolutions first (the default).
    #[default]
    ResolutionTime,
    /// Rolling 24 h contract volume.
    Volume24h,
    /// All-time contract volume.
    VolumeAllTime,
}

impl ResolvedOrderBy {
    fn as_str(self) -> &'static str {
        match self {
            ResolvedOrderBy::ResolutionTime => "resolution_time",
            ResolvedOrderBy::Volume24h => "volume_24h",
            ResolvedOrderBy::VolumeAllTime => "volume_all_time",
        }
    }
}

/// Query of `/api/v1/markets/resolved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedQuery {
    /// Lookback window.
    pub lookback: Lookback,
    /// Ordering.
    pub order_by: ResolvedOrderBy,
    /// 50, 100 or 200.
    pub limit: u32,
}

impl Default for ResolvedQuery {
    fn default() -> Self {
        ResolvedQuery {
            lookback: Lookback::Hours24,
            order_by: ResolvedOrderBy::ResolutionTime,
            limit: 50,
        }
    }
}

/// Body of `POST /api/v1/markets/search` (exact-match filters, union semantics).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MarketSearch {
    /// Symbols (≤ 500).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub symbols: Vec<String>,
    /// Event codes (≤ 50).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<String>,
    /// Game ids (≤ 50).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub games: Vec<String>,
}

/// Query of `/api/v1/accounts/{owner}`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountQuery {
    /// Restrict orders and positions to one market.
    pub symbol: Option<String>,
    /// Return an empty `orders` array (large market makers rest tens of thousands of orders).
    pub exclude_orders: bool,
}

/// `order_by` of position instances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PositionOrderBy {
    /// Close time (the default).
    #[default]
    CloseTime,
    /// Realized PnL.
    RealizedPnl,
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortOrder {
    /// Ascending.
    Asc,
    /// Descending (the default).
    #[default]
    Desc,
}

/// Query of `/api/v2/accounts/{owner}/position-instances`. A cursor must be followed with the
/// same query.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PositionInstancesQuery {
    /// Symbol filter (≤ 100, empty = all).
    pub symbols: Vec<String>,
    /// Ordering field.
    pub order_by: Option<PositionOrderBy>,
    /// Direction.
    pub sort_order: Option<SortOrder>,
    /// Page size (≤ 1000, default 1000).
    pub limit: Option<u32>,
}

impl PositionInstancesQuery {
    fn params(&self) -> Result<Vec<(String, String)>> {
        let mut q = Vec::new();
        push_symbols(&mut q, &self.symbols, MAX_FILTER_SYMBOLS, false)?;
        if let Some(o) = self.order_by {
            let v = match o {
                PositionOrderBy::CloseTime => "close_time",
                PositionOrderBy::RealizedPnl => "realized_pnl",
            };
            q.push(("order_by".into(), v.into()));
        }
        if let Some(s) = self.sort_order {
            let v = match s {
                SortOrder::Asc => "asc",
                SortOrder::Desc => "desc",
            };
            q.push(("sort_order".into(), v.into()));
        }
        if let Some(l) = self.limit {
            q.push(("limit".into(), l.to_string()));
        }
        Ok(q)
    }
}

fn push_symbols<S: AsRef<str>>(
    q: &mut Vec<(String, String)>,
    symbols: &[S],
    max: usize,
    required: bool,
) -> Result<()> {
    if symbols.len() > max {
        return Err(Error::InvalidArgument(format!(
            "{} symbols given, at most {max} allowed",
            symbols.len()
        )));
    }
    if symbols.is_empty() {
        if required {
            return Err(Error::InvalidArgument(
                "at least one symbol required".into(),
            ));
        }
        return Ok(());
    }
    let joined = symbols
        .iter()
        .map(AsRef::as_ref)
        .collect::<Vec<_>>()
        .join(",");
    q.push(("symbols".into(), joined));
    Ok(())
}

impl Client {
    /// A client for production with default limits and retries.
    pub fn new() -> Result<Self> {
        ClientBuilder::default().build()
    }

    /// A builder.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// The shared rate limiter (hand it to other clients via [`ClientBuilder::rate_limiter`]).
    pub fn rate_limiter(&self) -> Arc<RateLimiter> {
        self.inner.limiter.clone()
    }

    /// `GET` a path with query parameters and decode the envelope. Exposed for endpoints this
    /// crate does not wrap yet; rate limiting and retries apply.
    pub async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(String, String)],
    ) -> Result<Response<T>> {
        self.execute(Method::GET, path, query, None, decode_response::<T>)
            .await
    }

    /// `GET` a list endpoint and decode it item by item into a [`Lenient`].
    pub async fn get_lenient<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(String, String)],
    ) -> Result<Response<Lenient<T>>> {
        self.execute(Method::GET, path, query, None, |status, body, ctx| {
            decode_with(status, body, ctx, Lenient::from_value)
        })
        .await
    }

    async fn post<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<Response<T>> {
        let body = post_body(path, body)?;
        self.execute(Method::POST, path, &[], Some(body), decode_response::<T>)
            .await
    }

    async fn post_lenient<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<Response<Lenient<T>>> {
        let body = post_body(path, body)?;
        self.execute(
            Method::POST,
            path,
            &[],
            Some(body),
            decode_lenient_response::<T>,
        )
        .await
    }

    async fn execute<R>(
        &self,
        method: Method,
        path: &str,
        query: &[(String, String)],
        body: Option<Vec<u8>>,
        decode: impl Fn(u16, &[u8], &str) -> Result<R>,
    ) -> Result<R> {
        let inner = &self.inner;
        let (mut url, route) = build_url(&inner.base, path)?;
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        let policy = inner.retry;
        let mut attempt: u32 = 0;
        loop {
            inner.limiter.acquire(&inner.host, &route).await;
            let mut req = inner.http.request(method.clone(), url.clone());
            if let Some(b) = &body {
                req = req
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(b.clone());
            }
            let (result, retry_after) = match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let retry_after = parse_retry_after(&resp);
                    match resp.bytes().await {
                        Ok(bytes) => (decode(status.as_u16(), &bytes, url.as_str()), retry_after),
                        Err(e) => (Err(Error::Transport(e)), retry_after),
                    }
                }
                Err(e) => (Err(Error::Transport(e)), None),
            };
            let err = match result {
                Ok(r) => return Ok(r),
                Err(e) => e,
            };
            if !is_retryable(&err) {
                return Err(err);
            }
            attempt += 1;
            if attempt >= policy.max_attempts {
                return Err(Error::RetriesExhausted {
                    attempts: attempt,
                    last: Box::new(err),
                });
            }
            let delay = retry_after.unwrap_or_else(|| policy.backoff.delay(attempt - 1));
            if err.http_status() == Some(StatusCode::TOO_MANY_REQUESTS.as_u16())
                || err.api_code() == Some(ApiErrorCode::RateLimited.as_str())
            {
                // Everyone talking to this host slows down, not just this task.
                inner.limiter.host_bucket(&inner.host).pause_for(delay);
            }
            tracing::warn!(%url, attempt, ?delay, error = %err, "retrying Pascal request");
            tokio::time::sleep(delay).await;
        }
    }

    /// `GET /api/v1/time`.
    pub async fn time(&self) -> Result<Response<ServerTime>> {
        self.get("/api/v1/time", &[]).await
    }

    /// `GET /api/v1/markets`: every unresolved market (~3,000, ~5 MB). Rate-limited to one call
    /// per 30 s by default; cache the result. Fails if any market does not decode — see
    /// [`Self::markets_lenient`].
    pub async fn markets(&self) -> Result<Response<Vec<Market>>> {
        self.get("/api/v1/markets", &[]).await
    }

    /// `GET /api/v1/markets`, decoding market by market and reporting failures instead of
    /// failing the whole list.
    pub async fn markets_lenient(&self) -> Result<Response<Lenient<Market>>> {
        self.get_lenient("/api/v1/markets", &[]).await
    }

    /// `GET /api/v1/markets/resolved`.
    pub async fn resolved_markets(&self, q: ResolvedQuery) -> Result<Response<Vec<Market>>> {
        self.get("/api/v1/markets/resolved", &resolved_params(q)?)
            .await
    }

    /// `GET /api/v1/markets/resolved`, decoded market by market.
    pub async fn resolved_markets_lenient(
        &self,
        q: ResolvedQuery,
    ) -> Result<Response<Lenient<Market>>> {
        self.get_lenient("/api/v1/markets/resolved", &resolved_params(q)?)
            .await
    }

    /// `POST /api/v1/markets/search`: exact symbols / events / games, including resolved markets.
    pub async fn search_markets(&self, body: &MarketSearch) -> Result<Response<Vec<Market>>> {
        check_search(body)?;
        self.post("/api/v1/markets/search", body).await
    }

    /// `POST /api/v1/markets/search`, decoded market by market, so one market with a new enum
    /// value does not hide the rest of a 500-symbol batch.
    pub async fn search_markets_lenient(
        &self,
        body: &MarketSearch,
    ) -> Result<Response<Lenient<Market>>> {
        check_search(body)?;
        self.post_lenient("/api/v1/markets/search", body).await
    }

    /// `GET /api/v1/books` for 1..=50 symbols.
    pub async fn books<S: AsRef<str>>(&self, symbols: &[S]) -> Result<Response<Books>> {
        let mut q = Vec::new();
        push_symbols(&mut q, symbols, MAX_BOOKS_SYMBOLS, true)?;
        self.get("/api/v1/books", &q).await
    }

    /// `GET /api/v1/trades`: one page of the tape for 1..=120 symbols, newest first.
    pub async fn trades<S: AsRef<str>>(
        &self,
        symbols: &[S],
        page: &PageParams,
    ) -> Result<Response<Page<PublicTrade>>> {
        let mut q = Vec::new();
        push_symbols(&mut q, symbols, MAX_TRADES_SYMBOLS, true)?;
        page.push(&mut q)?;
        self.get("/api/v1/trades", &q).await
    }

    /// Pager over `/api/v1/trades`, newest first, following `next_cursor`.
    pub fn trades_pager<S: AsRef<str>>(
        &self,
        symbols: &[S],
        limit: Option<u32>,
    ) -> Result<Pager<PublicTrade>> {
        let mut q = Vec::new();
        push_symbols(&mut q, symbols, MAX_TRADES_SYMBOLS, true)?;
        PageParams {
            limit,
            ..PageParams::default()
        }
        .push(&mut q)?;
        Ok(Pager::new(self.clone(), "/api/v1/trades".into(), q))
    }

    /// `GET /api/v1/accounts/{owner}`.
    pub async fn account(&self, owner: &str, q: &AccountQuery) -> Result<Response<AccountState>> {
        let mut params = Vec::new();
        if let Some(s) = &q.symbol {
            params.push(("symbol".into(), s.clone()));
        }
        if q.exclude_orders {
            params.push(("exclude_orders".into(), "true".into()));
        }
        self.get(&account_path(owner, "")?, &params).await
    }

    /// `GET /api/v1/accounts/{owner}/history`: all four timeframes.
    pub async fn account_history(&self, owner: &str) -> Result<Response<AccountHistory>> {
        self.get(&account_path(owner, "/history")?, &[]).await
    }

    /// `GET /api/v1/accounts/{owner}/fills`: one page, newest first.
    pub async fn fills<S: AsRef<str>>(
        &self,
        owner: &str,
        symbols: &[S],
        page: &PageParams,
    ) -> Result<Response<Page<Fill>>> {
        let mut q = Vec::new();
        push_symbols(&mut q, symbols, MAX_FILTER_SYMBOLS, false)?;
        page.push(&mut q)?;
        self.get(&account_path(owner, "/fills")?, &q).await
    }

    /// Pager over an account's fills.
    pub fn fills_pager<S: AsRef<str>>(
        &self,
        owner: &str,
        symbols: &[S],
        limit: Option<u32>,
    ) -> Result<Pager<Fill>> {
        let mut q = Vec::new();
        push_symbols(&mut q, symbols, MAX_FILTER_SYMBOLS, false)?;
        PageParams {
            limit,
            ..PageParams::default()
        }
        .push(&mut q)?;
        Ok(Pager::new(self.clone(), account_path(owner, "/fills")?, q))
    }

    /// `GET /api/v1/accounts/{owner}/transfers`: one page, newest first.
    pub async fn transfers(
        &self,
        owner: &str,
        page: &PageParams,
    ) -> Result<Response<Page<Transfer>>> {
        let mut q = Vec::new();
        page.push(&mut q)?;
        self.get(&account_path(owner, "/transfers")?, &q).await
    }

    /// Pager over an account's transfers.
    pub fn transfers_pager(&self, owner: &str, limit: Option<u32>) -> Result<Pager<Transfer>> {
        let mut q = Vec::new();
        PageParams {
            limit,
            ..PageParams::default()
        }
        .push(&mut q)?;
        Ok(Pager::new(
            self.clone(),
            account_path(owner, "/transfers")?,
            q,
        ))
    }

    /// `GET /api/v2/accounts/{owner}/position-instances`: one page of closed positions.
    pub async fn position_instances(
        &self,
        owner: &str,
        q: &PositionInstancesQuery,
        before_cursor: Option<&str>,
    ) -> Result<Response<Page<PositionHistory>>> {
        let mut params = q.params()?;
        if let Some(c) = before_cursor {
            params.push(("before_cursor".into(), c.to_owned()));
        }
        self.get(&account_path_v2(owner, "/position-instances")?, &params)
            .await
    }

    /// Pager over closed positions (the cursor is followed with the same query).
    pub fn position_instances_pager(
        &self,
        owner: &str,
        q: &PositionInstancesQuery,
    ) -> Result<Pager<PositionHistory>> {
        Ok(Pager::new(
            self.clone(),
            account_path_v2(owner, "/position-instances")?,
            q.params()?,
        ))
    }

    /// `GET /api/v1/accounts/{owner}/trading-keys`.
    pub async fn trading_keys(&self, owner: &str) -> Result<Response<Vec<TradingKey>>> {
        self.get(&account_path(owner, "/trading-keys")?, &[]).await
    }

    /// `GET /api/v1/trading-keys/{pubkey}/accounts`: owners that use this trading key.
    pub async fn trading_key_accounts(&self, pubkey: &str) -> Result<Response<Vec<String>>> {
        check_base58(pubkey)?;
        self.get(&format!("/api/v1/trading-keys/{pubkey}/accounts"), &[])
            .await
    }

    /// `GET /api/v1/usernames` for 1..=100 owners (owners without a username are omitted).
    pub async fn usernames<S: AsRef<str>>(&self, owners: &[S]) -> Result<Response<Usernames>> {
        if owners.is_empty() || owners.len() > MAX_USERNAMES_OWNERS {
            return Err(Error::InvalidArgument(format!(
                "usernames takes 1..={MAX_USERNAMES_OWNERS} owners, got {}",
                owners.len()
            )));
        }
        let joined = owners
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<_>>()
            .join(",");
        self.get("/api/v1/usernames", &[("owners".into(), joined)])
            .await
    }

    /// `GET /api/v1/accounts/{owner}/bio`.
    pub async fn bio(&self, owner: &str) -> Result<Response<AccountBio>> {
        self.get(&account_path(owner, "/bio")?, &[]).await
    }

    /// `GET /api/v1/accounts/{owner}/builder-stats`.
    pub async fn builder_stats(&self, owner: &str) -> Result<Response<BuilderStats>> {
        self.get(&account_path(owner, "/builder-stats")?, &[]).await
    }

    /// `GET /api/v1/accounts/{owner}/deposit-address`.
    pub async fn deposit_address(&self, owner: &str) -> Result<Response<DepositAddress>> {
        self.get(&account_path(owner, "/deposit-address")?, &[])
            .await
    }
}

/// Normalises an API path to `/a/b` (one leading slash, no trailing slash, no empty segments).
/// The result is the rate-limit route key.
pub(crate) fn normalize_path(path: &str) -> Result<String> {
    let path = path.split(['?', '#']).next().unwrap_or("");
    if path.contains("..") {
        return Err(Error::InvalidArgument(format!(
            "path {path:?} contains '..'"
        )));
    }
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    Ok(format!("/{}", segs.join("/")))
}

/// Appends a normalised API path to the base URL, keeping any path prefix of the base
/// (`https://proxy/pascal` + `/api/v1/time` → `https://proxy/pascal/api/v1/time`).
/// Returns the URL and the route key used for per-route rate limits.
pub(crate) fn build_url(base: &Url, path: &str) -> Result<(Url, String)> {
    let route = normalize_path(path)?;
    let mut url = base.clone();
    let prefix = base.path().trim_end_matches('/');
    url.set_path(&format!("{prefix}{route}"));
    url.set_query(None);
    url.set_fragment(None);
    Ok((url, route))
}

fn resolved_params(q: ResolvedQuery) -> Result<Vec<(String, String)>> {
    if ![50, 100, 200].contains(&q.limit) {
        return Err(Error::InvalidArgument(format!(
            "resolved markets limit must be 50, 100 or 200, got {}",
            q.limit
        )));
    }
    Ok(vec![
        ("lookback".into(), q.lookback.as_str().into()),
        ("order_by".into(), q.order_by.as_str().into()),
        ("limit".into(), q.limit.to_string()),
    ])
}

fn post_body<B: Serialize>(path: &str, body: &B) -> Result<Vec<u8>> {
    serde_json::to_vec(body).map_err(|source| Error::Decode {
        context: format!("{path} request body"),
        source,
    })
}

fn check_search(body: &MarketSearch) -> Result<()> {
    if body.symbols.is_empty() && body.events.is_empty() && body.games.is_empty() {
        return Err(Error::InvalidArgument(
            "market search needs at least one filter".into(),
        ));
    }
    Ok(())
}

fn check_base58(s: &str) -> Result<()> {
    let ok = (32..=44).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() && !matches!(b, b'0' | b'O' | b'I' | b'l'));
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "{s:?} is not a base58 public key"
        )))
    }
}

fn account_path(owner: &str, suffix: &str) -> Result<String> {
    check_base58(owner)?;
    Ok(format!("/api/v1/accounts/{owner}{suffix}"))
}

fn account_path_v2(owner: &str, suffix: &str) -> Result<String> {
    check_base58(owner)?;
    Ok(format!("/api/v2/accounts/{owner}{suffix}"))
}

fn parse_retry_after(resp: &reqwest::Response) -> Option<Duration> {
    let secs: u64 = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(secs.min(300)))
}

/// Whether a failed request should be retried unchanged.
fn is_retryable(e: &Error) -> bool {
    match e {
        Error::Api(api) => {
            api.kind().is_some_and(ApiErrorCode::is_transient)
                || matches!(api.http_status, Some(429 | 502 | 503 | 504))
        }
        Error::HttpStatus { status, .. } => matches!(status, 429 | 500 | 502 | 503 | 504),
        Error::Transport(t) => t.is_timeout() || t.is_connect() || t.is_request() || t.is_body(),
        _ => false,
    }
}

/// Follows `next_cursor` → `before_cursor` over a paginated endpoint.
///
/// [`Pager::next_page`] returns pages until the API omits `next_cursor`. A failed call (after the
/// client's own retries) leaves the pager where it was, so calling `next_page` again retries the
/// same page. The cursor can be saved with [`Pager::cursor`] and restored with
/// [`Pager::resume_from`] to continue a backfill later.
#[derive(Debug)]
pub struct Pager<T> {
    client: Client,
    path: String,
    params: Vec<(String, String)>,
    cursor: Option<String>,
    first_seq: Option<u64>,
    pages: u32,
    done: bool,
    _item: PhantomData<fn() -> T>,
}

impl<T: DeserializeOwned> Pager<T> {
    fn new(client: Client, path: String, params: Vec<(String, String)>) -> Self {
        Pager {
            client,
            path,
            params,
            cursor: None,
            first_seq: None,
            pages: 0,
            done: false,
            _item: PhantomData,
        }
    }

    /// Starts (or continues) from a saved `next_cursor`.
    pub fn resume_from(mut self, before_cursor: impl Into<String>) -> Self {
        self.cursor = Some(before_cursor.into());
        self
    }

    /// The cursor of the next page to fetch (`None` before the first page or when done).
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    /// Envelope `seq` of the first page: together the pages cover items up to this sequence.
    pub fn first_seq(&self) -> Option<u64> {
        self.first_seq
    }

    /// Pages fetched so far.
    pub fn pages_fetched(&self) -> u32 {
        self.pages
    }

    /// True once the last page has been returned.
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Fetches the next page, or `None` when the previous page was the last.
    pub async fn next_page(&mut self) -> Result<Option<Response<Page<T>>>> {
        if self.done {
            return Ok(None);
        }
        let mut q = self.params.clone();
        if let Some(c) = &self.cursor {
            q.push(("before_cursor".into(), c.clone()));
        }
        let resp: Response<Page<T>> = self.client.get(&self.path, &q).await?;
        self.pages += 1;
        if self.first_seq.is_none() {
            self.first_seq = resp.seq;
        }
        let next = resp.data.next_cursor.clone();
        // Stop on the last page, an empty page, or a cursor that did not advance.
        self.done = next.is_none() || resp.data.items.is_empty() || next == self.cursor;
        self.cursor = if self.done { None } else { next };
        Ok(Some(resp))
    }

    /// Collects items while `keep(item)` is true, stopping at the first item for which it is
    /// false, at the last page, or after `max_pages` pages. Useful for "everything newer than
    /// a timestamp" on the newest-first tape.
    ///
    /// On error the items collected so far are returned inside [`PartialError`], and the pager
    /// still points at the page that failed: store the items, then call `collect_while` again
    /// (or save [`Pager::cursor`]) to resume without refetching. `?` converts a
    /// `PartialError` into [`Error`], dropping the items.
    pub async fn collect_while(
        &mut self,
        mut keep: impl FnMut(&T) -> bool,
        max_pages: u32,
    ) -> Result<Vec<T>, PartialError<T>> {
        let mut out = Vec::new();
        let mut fetched = 0;
        while fetched < max_pages {
            let page = match self.next_page().await {
                Ok(Some(page)) => page,
                Ok(None) => break,
                Err(error) => return Err(PartialError { items: out, error }),
            };
            fetched += 1;
            for item in page.data.items {
                if !keep(&item) {
                    self.done = true;
                    self.cursor = None;
                    return Ok(out);
                }
                out.push(item);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_are_validated() {
        let mut q = Vec::new();
        let many: Vec<String> = (0..121).map(|i| format!("S{i}.X")).collect();
        assert!(push_symbols(&mut q, &many, MAX_TRADES_SYMBOLS, true).is_err());
        assert!(push_symbols::<&str>(&mut q, &[], MAX_TRADES_SYMBOLS, true).is_err());
        assert!(push_symbols::<&str>(&mut q, &[], MAX_FILTER_SYMBOLS, false).is_ok());
        assert!(q.is_empty());
        push_symbols(&mut q, &["A.B", "C.D"], MAX_TRADES_SYMBOLS, true).unwrap();
        assert_eq!(q, vec![("symbols".to_string(), "A.B,C.D".to_string())]);
    }

    #[test]
    fn owner_is_validated() {
        assert!(account_path("7ATdPM8uGMu3qiRSCpc6hUrSZ5VxFSUyYLfRipABG5bC", "/bio").is_ok());
        assert!(account_path("../markets", "").is_err());
        assert!(account_path("7ATdPM8uGMu3qiRSCpc6hUrSZ5VxFSUyYLfRipABG5b0", "").is_err());
    }

    #[test]
    fn paths_are_normalised_and_prefix_kept() {
        let base = Url::parse("https://data.pascal.trade").unwrap();
        let (url, route) = build_url(&base, "/api/v1/time").unwrap();
        assert_eq!(url.as_str(), "https://data.pascal.trade/api/v1/time");
        assert_eq!(route, "/api/v1/time");
        // Missing / doubled / trailing slashes all map to the markets-list bucket.
        for p in ["api/v1/markets", "/api/v1/markets/", "//api//v1/markets"] {
            let (url, route) = build_url(&base, p).unwrap();
            assert_eq!(route, crate::ratelimit::MARKETS_LIST_ROUTE, "{p}");
            assert_eq!(url.path(), "/api/v1/markets");
        }
        // A base path prefix is preserved, with or without a trailing slash.
        for b in ["http://127.0.0.1:9/pascal", "http://127.0.0.1:9/pascal/"] {
            let base = Url::parse(b).unwrap();
            let (url, route) = build_url(&base, "/api/v1/markets").unwrap();
            assert_eq!(url.as_str(), "http://127.0.0.1:9/pascal/api/v1/markets");
            assert_eq!(route, "/api/v1/markets");
        }
        assert!(build_url(&base, "/api/../markets").is_err());
    }

    #[test]
    fn page_params_reject_two_starts() {
        let p = PageParams {
            limit: None,
            before_cursor: Some("x".into()),
            at_or_before_seq: Some(1),
        };
        assert!(p.push(&mut Vec::new()).is_err());
    }

    #[test]
    fn retryable_classification() {
        let api = |status, code: &str| {
            Error::Api(crate::error::ApiError {
                http_status: Some(status),
                code: code.into(),
                message: String::new(),
                seq: None,
                server_time_ms: None,
            })
        };
        assert!(is_retryable(&api(429, "RATE_LIMITED")));
        assert!(is_retryable(&api(503, "HISTORY_NOT_READY")));
        assert!(!is_retryable(&api(422, "INVALID_REQUEST")));
        assert!(!is_retryable(&api(404, "ACCOUNT_NOT_FOUND")));
        assert!(is_retryable(&Error::HttpStatus {
            status: 429,
            url: String::new(),
            body: String::new()
        }));
    }
}
