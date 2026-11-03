//! Client for Pascal's signed write API: place, cancel and mass-cancel orders.
//!
//! [`TradeClient`] has **no default base URL**: it is always built with an explicit one
//! (`PASCAL_TRADE_URL`), which in Schedina's mode M is `pascal-mock`. Nothing in this crate's
//! library code knows the address of Pascal's production write API (decision D-007); only
//! `pascal-cli place-order --i-am-authorized` does.
//!
//! Requests are signed beforehand with [`crate::signing::TradingSigner`]; the client only
//! sends them. Each batch endpoint answers with one envelope per request, in request order,
//! and HTTP 200 if any succeeded ([`WriteResult`] per item); a request-level failure (rate
//! limit, malformed body) is one error envelope, returned as the call's `Err`.
//!
//! Retries are deliberately narrow, because a write that reached the matching engine must not
//! be sent twice: only a request-level `429` / `RATE_LIMITED` / `BACKPRESSURE` (rejected before
//! processing) and a failed TCP connect are retried. A timeout, a `5xx`, or
//! `WRITE_RESULT_UNAVAILABLE` is returned to the caller, who must reconcile from the account's
//! orders and fills before re-sending.

use std::sync::Arc;
use std::time::Duration;

use reqwest::Method;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::envelope::{Response, decode_with};
use crate::error::{ApiError, ApiErrorCode, Error, ErrorBody, Result};
use crate::num::opt_str_num;
use crate::ratelimit::{Backoff, BucketConfig, RateLimiter, RateLimits, RetryPolicy};
use crate::rest::build_url;
use crate::signing::{
    BuilderInfo, CancelOrderRequest, MassCancelRequest, PlaceOrderRequest, base58,
};
use crate::types::{Fill, Order, OrderUpdateReason};

/// `POST` path of place / replace orders.
pub const ORDERS_PATH: &str = "/api/v1/orders";
/// `POST` path of cancels.
pub const CANCELS_PATH: &str = "/api/v1/cancels";
/// `POST` path of mass cancels.
pub const MASS_CANCELS_PATH: &str = "/api/v1/mass-cancels";
/// Most orders per `POST /api/v1/orders`.
pub const MAX_ORDERS_PER_BATCH: usize = 50;
/// Most cancels per `POST /api/v1/cancels`.
pub const MAX_CANCELS_PER_BATCH: usize = 50;
/// Most mass cancels (one market each) per `POST /api/v1/mass-cancels`.
pub const MAX_MASS_CANCELS_PER_BATCH: usize = 10;

/// Outcome of one request of a batch: the decoded item with its envelope, or why it failed
/// (`Error::Api` with Pascal's code, or `Error::Decode` when a success item has an unexpected
/// shape). Use [`Error::api_code`] / [`ApiError::kind`] to branch on `INVALID_PRICE`,
/// `WOULD_CROSS`, `INSUFFICIENT_COLLATERAL`, `UNAUTHORIZED`, …
pub type WriteResult<T> = std::result::Result<Response<T>, Error>;

/// `data` of a successful place-order item.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PlacedOrder {
    /// The order after placement, including immediate fills.
    pub order: Order,
    /// Fills that happened during placement.
    #[serde(default)]
    pub fills: Vec<Fill>,
    /// Why the order changed during placement (e.g. `IOC_REMAINDER_CANCELED`).
    #[serde(default)]
    pub update_reason: Option<OrderUpdateReason>,
}

/// `data` of a successful mass-cancel item: the orders that were resting.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct MassCancelled {
    /// Cancelled orders.
    #[serde(default)]
    pub orders: Vec<Order>,
}

/// Limits for the write host. The docs allow 100 requests per second on the REST order
/// endpoints; the default stays at half that.
pub fn default_trade_limits() -> RateLimits {
    RateLimits {
        per_host: BucketConfig {
            per_second: 50.0,
            burst: 20.0,
        },
        per_route: Default::default(),
    }
}

/// Default write retry policy: 3 attempts, 100 ms doubling to 1 s, inside a 5 s receive window.
pub fn default_trade_retry() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 3,
        backoff: Backoff {
            base: Duration::from_millis(100),
            max: Duration::from_secs(1),
        },
    }
}

/// Which builder code [`TradeClient::place_orders`] insists on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BuilderPolicy {
    /// Every order must carry some `builder_info` (the default: Schedina routes every hedge
    /// with its builder code).
    #[default]
    Required,
    /// Every order must carry exactly this builder key and fee rate.
    Exactly(BuilderInfo),
    /// No check.
    Any,
}

/// Builder for [`TradeClient`].
#[derive(Debug, Clone)]
pub struct TradeClientBuilder {
    base_url: String,
    timeout: Duration,
    user_agent: String,
    limiter: Option<Arc<RateLimiter>>,
    limits: RateLimits,
    retry: RetryPolicy,
    builder_policy: BuilderPolicy,
}

impl TradeClientBuilder {
    /// Per-request timeout (default 10 s).
    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    /// `User-Agent` header.
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = ua.into();
        self
    }

    /// Limits for a limiter owned by this client (ignored when [`Self::rate_limiter`] is set).
    pub fn rate_limits(mut self, limits: RateLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Share an existing limiter.
    pub fn rate_limiter(mut self, limiter: Arc<RateLimiter>) -> Self {
        self.limiter = Some(limiter);
        self
    }

    /// Retry policy for request-level rate limits and connect failures.
    pub fn retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Builder-code check on every placed order (default [`BuilderPolicy::Required`]).
    pub fn builder_policy(mut self, policy: BuilderPolicy) -> Self {
        self.builder_policy = policy;
        self
    }

    /// Builds the client.
    pub fn build(self) -> Result<TradeClient> {
        let base = Url::parse(&self.base_url)
            .map_err(|e| Error::InvalidArgument(format!("trade url {:?}: {e}", self.base_url)))?;
        let host = base
            .host_str()
            .ok_or_else(|| Error::InvalidArgument(format!("trade url {base} has no host")))?
            .to_owned();
        let http = reqwest::Client::builder()
            .timeout(self.timeout)
            .user_agent(self.user_agent)
            .build()?;
        Ok(TradeClient {
            inner: Arc::new(Inner {
                http,
                base,
                host,
                limiter: self
                    .limiter
                    .unwrap_or_else(|| Arc::new(RateLimiter::new(self.limits))),
                retry: self.retry,
                builder_policy: self.builder_policy,
            }),
        })
    }
}

/// Write-API client. Cheap to clone.
#[derive(Debug, Clone)]
pub struct TradeClient {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    http: reqwest::Client,
    base: Url,
    host: String,
    limiter: Arc<RateLimiter>,
    retry: RetryPolicy,
    builder_policy: BuilderPolicy,
}

impl TradeClient {
    /// A builder for the write API at `base_url` (e.g. `http://127.0.0.1:8090` for the mock).
    pub fn builder(base_url: impl Into<String>) -> TradeClientBuilder {
        TradeClientBuilder {
            base_url: base_url.into(),
            timeout: Duration::from_secs(10),
            user_agent: concat!("pascal-rs/", env!("CARGO_PKG_VERSION")).to_owned(),
            limiter: None,
            limits: default_trade_limits(),
            retry: default_trade_retry(),
            builder_policy: BuilderPolicy::default(),
        }
    }

    /// A client with default limits, retries and builder policy.
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        Self::builder(base_url).build()
    }

    /// The base URL requests go to.
    pub fn base_url(&self) -> &Url {
        &self.inner.base
    }

    /// `POST /api/v1/orders`: 1..=50 signed orders, one result per order in request order.
    pub async fn place_orders(
        &self,
        orders: &[PlaceOrderRequest],
    ) -> Result<Vec<WriteResult<PlacedOrder>>> {
        for (i, o) in orders.iter().enumerate() {
            self.check_builder(i, o)?;
        }
        self.post_batch(ORDERS_PATH, orders, MAX_ORDERS_PER_BATCH)
            .await
    }

    /// `POST /api/v1/cancels`: 1..=50 signed cancels; each success is the cancelled order.
    pub async fn cancel(&self, cancels: &[CancelOrderRequest]) -> Result<Vec<WriteResult<Order>>> {
        self.post_batch(CANCELS_PATH, cancels, MAX_CANCELS_PER_BATCH)
            .await
    }

    /// `POST /api/v1/mass-cancels`: 1..=10 signed mass cancels, one market each.
    pub async fn mass_cancel(
        &self,
        requests: &[MassCancelRequest],
    ) -> Result<Vec<WriteResult<MassCancelled>>> {
        self.post_batch(MASS_CANCELS_PATH, requests, MAX_MASS_CANCELS_PER_BATCH)
            .await
    }

    fn check_builder(&self, index: usize, o: &PlaceOrderRequest) -> Result<()> {
        let fail = |why: String| {
            Err(Error::InvalidArgument(format!(
                "order {index} (client_order_id {}): {why}",
                o.client_order_id
            )))
        };
        match (self.inner.builder_policy, &o.builder_info) {
            (BuilderPolicy::Any, _) => Ok(()),
            (_, None) => fail("missing builder_info".into()),
            (BuilderPolicy::Required, Some(_)) => Ok(()),
            (BuilderPolicy::Exactly(want), Some(got)) => {
                if got.builder_pubkey == base58(&want.pubkey)
                    && got.builder_fee_rate == i64::from(want.fee_rate_u)
                {
                    Ok(())
                } else {
                    fail(format!(
                        "builder_info {}/{} differs from the configured builder",
                        got.builder_pubkey, got.builder_fee_rate
                    ))
                }
            }
        }
    }

    async fn post_batch<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        items: &[B],
        max: usize,
    ) -> Result<Vec<WriteResult<T>>> {
        if items.is_empty() || items.len() > max {
            return Err(Error::InvalidArgument(format!(
                "{path} takes 1..={max} requests, got {}",
                items.len()
            )));
        }
        let body = serde_json::to_vec(items).map_err(|source| Error::Decode {
            context: format!("{path} request body"),
            source,
        })?;
        let inner = &self.inner;
        let (url, route) = build_url(&inner.base, path)?;
        let policy = inner.retry;
        let mut attempt: u32 = 0;
        loop {
            inner.limiter.acquire(&inner.host, &route).await;
            let req = inner
                .http
                .request(Method::POST, url.clone())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.clone());
            let (result, retry_after) = match req.send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let retry_after = retry_after(&resp);
                    match resp.bytes().await {
                        Ok(bytes) => (
                            decode_batch::<T>(status, &bytes, url.as_str(), items.len()),
                            retry_after,
                        ),
                        Err(e) => (Err(Error::Transport(e)), retry_after),
                    }
                }
                Err(e) => (Err(Error::Transport(e)), None),
            };
            let err = match result {
                Ok(r) => return Ok(r),
                Err(e) => e,
            };
            if !is_safe_to_resend(&err) {
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
            if err.http_status() == Some(429)
                || err.api_code() == Some(ApiErrorCode::RateLimited.as_str())
            {
                inner.limiter.host_bucket(&inner.host).pause_for(delay);
            }
            tracing::warn!(%url, attempt, ?delay, error = %err, "re-sending Pascal write request");
            tokio::time::sleep(delay).await;
        }
    }
}

fn retry_after(resp: &reqwest::Response) -> Option<Duration> {
    let secs: u64 = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(secs.min(60)))
}

/// Whether a failed write was certainly not processed, so the same signed batch may go again.
fn is_safe_to_resend(e: &Error) -> bool {
    match e {
        Error::Api(api) => {
            api.http_status == Some(429)
                || matches!(
                    api.kind(),
                    Some(ApiErrorCode::RateLimited | ApiErrorCode::Backpressure)
                )
        }
        Error::HttpStatus { status, .. } => *status == 429,
        Error::Transport(t) => t.is_connect(),
        _ => false,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Status {
    Success,
    Error,
}

#[derive(Deserialize)]
struct ItemHead {
    status: Status,
    #[serde(default, with = "opt_str_num")]
    seq: Option<u64>,
    #[serde(default, with = "opt_str_num")]
    state_time_ms: Option<u64>,
    #[serde(default, with = "opt_str_num")]
    server_time_ms: Option<u64>,
}

/// Decodes a batch response: an array of envelopes (one per request), or a single
/// request-level error envelope.
pub(crate) fn decode_batch<T: DeserializeOwned>(
    http_status: u16,
    body: &[u8],
    context: &str,
    expected: usize,
) -> Result<Vec<WriteResult<T>>> {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Array(items)) => {
            if items.len() != expected {
                return Err(Error::Decode {
                    context: context.to_owned(),
                    source: serde::de::Error::custom(format!(
                        "{} results for {expected} requests: {}",
                        items.len(),
                        truncate(&String::from_utf8_lossy(body))
                    )),
                });
            }
            Ok(items
                .into_iter()
                .enumerate()
                .map(|(i, item)| decode_item(http_status, item, &format!("{context} [{i}]")))
                .collect())
        }
        // Not an array: a request-level envelope (or a non-JSON error page).
        _ => decode_with(http_status, body, context, |_| {
            Err::<std::convert::Infallible, _>(serde::de::Error::custom(
                "expected an array of results",
            ))
        })
        .map(|r| match r.data {}),
    }
}

fn decode_item<T: DeserializeOwned>(http_status: u16, raw: Value, context: &str) -> WriteResult<T> {
    let decode_err = |source| Error::Decode {
        context: format!("{context}: {}", truncate(&raw.to_string())),
        source,
    };
    let head = ItemHead::deserialize(&raw).map_err(decode_err)?;
    match head.status {
        Status::Error => {
            let body = ErrorBody::deserialize(&raw["data"]).map_err(decode_err)?;
            Err(Error::Api(ApiError {
                http_status: Some(http_status),
                code: body.code,
                message: body.message,
                seq: head.seq,
                server_time_ms: head.server_time_ms,
            }))
        }
        Status::Success => {
            let data = T::deserialize(&raw["data"]).map_err(decode_err)?;
            Ok(Response {
                data,
                seq: head.seq,
                state_time_ms: head.state_time_ms,
                server_time_ms: head.server_time_ms,
                raw,
            })
        }
    }
}

fn truncate(s: &str) -> String {
    match s.char_indices().nth(300) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORDER: &str = r#"{"id":"987654321","client_order_id":"42","symbol":"A.B","side":"BID","price":"0.550000","size_original":"10","size_remaining":"0","size_filled":"3","notional_filled":"1.650000","tif":"IOC","type":"LIMIT","post_only":false,"reduce_only":false,"builder_pubkey":"J2xccRtuG43drESLYznHhLhQkLTdfepcKYbiQ9BsJVaf","builder_fee_rate":"0.001000","seq":"67880","update_seq":"67891","place_ts_ms":"1731536000050","update_ts_ms":"1731536000050"}"#;

    #[test]
    fn batch_items_decode_independently() {
        let body = format!(
            r#"[{{"status":"success","data":{{"order":{ORDER},"fills":[],"update_reason":"IOC_REMAINDER_CANCELED"}},"seq":"67891","server_time_ms":"1"}},
               {{"status":"error","data":{{"code":"WOULD_CROSS","message":"x"}},"seq":"67891"}},
               {{"status":"error","data":{{"code":"SOMETHING_NEW","message":"y"}}}},
               {{"status":"success","data":{{"order":{{"id":"x"}}}}}}]"#
        );
        let r = decode_batch::<PlacedOrder>(200, body.as_bytes(), "t", 4).unwrap();
        let ok = r[0].as_ref().unwrap();
        assert_eq!(ok.data.order.client_order_id, 42);
        assert_eq!(
            ok.data.update_reason,
            Some(OrderUpdateReason::IocRemainderCanceled)
        );
        assert_eq!(ok.seq, Some(67891));
        assert_eq!(r[1].as_ref().unwrap_err().api_code(), Some("WOULD_CROSS"));
        match r[2].as_ref().unwrap_err() {
            Error::Api(a) => {
                assert_eq!(a.kind(), None);
                assert_eq!(a.code, "SOMETHING_NEW");
            }
            e => panic!("{e}"),
        }
        assert!(matches!(r[3], Err(Error::Decode { .. })));
        // Count mismatch.
        assert!(decode_batch::<PlacedOrder>(200, body.as_bytes(), "t", 3).is_err());
    }

    #[test]
    fn request_level_errors() {
        let e = decode_batch::<Order>(
            429,
            br#"{"status":"error","data":{"code":"RATE_LIMITED","message":"slow"}}"#,
            "t",
            1,
        )
        .unwrap_err();
        assert_eq!(e.api_code(), Some("RATE_LIMITED"));
        assert!(is_safe_to_resend(&e));
        let e = decode_batch::<Order>(502, b"<html>bad gateway</html>", "t", 1).unwrap_err();
        assert!(matches!(e, Error::HttpStatus { status: 502, .. }));
        assert!(!is_safe_to_resend(&e));
        let e =
            decode_batch::<Order>(200, br#"{"status":"success","data":{}}"#, "t", 1).unwrap_err();
        assert!(matches!(e, Error::Decode { .. }));
        let e = decode_batch::<Order>(
            503,
            br#"{"status":"error","data":{"code":"WRITE_RESULT_UNAVAILABLE","message":"?"}}"#,
            "t",
            1,
        )
        .unwrap_err();
        assert!(!is_safe_to_resend(&e));
    }
}
