//! HTTP surface: Pascal's REST routes (read and write API on one port) plus `/mock/*` control.
//!
//! Every response uses Pascal's envelope `{status, data, seq, state_time_ms, server_time_ms}`;
//! write responses add `server_receive_time_ms` and are arrays of per-item envelopes (HTTP 200
//! when any item succeeded). Errors carry Pascal's codes with these HTTP statuses: 400
//! `INVALID_JSON`, 401 `UNAUTHORIZED`, 404 `*_NOT_FOUND`, 429 `RATE_LIMITED`, 500
//! `INTERNAL_ERROR`, 502/503/504 for the transient codes, 422 for every other code.
//!
//! Error injection: `?inject=<CODE>` on any Pascal REST route, or `POST /mock/inject` for the
//! next requests (so an unmodified client can be tested). On `/orders`, `/cancels` and
//! `/mass-cancels` an item-level code (spec section 9's `INVALID_PRICE`, `WOULD_CROSS`,
//! `INSUFFICIENT_COLLATERAL`, and e.g. `UNAUTHORIZED`) fails every item of the batch;
//! request-level codes (`RATE_LIMITED`, `BACKPRESSURE`, `SERVICE_UNAVAILABLE`, …) return one
//! envelope. The code must be one of Pascal's (`ApiErrorCode`). `/ws` is never injected.
//!
//! Account reads added in 2.D: `GET /api/v1/accounts/{owner}/fills` (cursor-paginated like
//! Pascal: `limit` 1..=500, default 100; `before_cursor` or `at_or_before_seq`; `symbols`
//! filter) and `GET /api/v1/accounts/{owner}/builder-stats`. `GET /ws` is the WebSocket
//! ([`crate::ws`]).
//!
//! Live mode (L4-1, [`crate::live`]): the same routes. `/books`, `/orders` and the `/ws` `book`
//! channel subscribe the upstream book of each symbol they name; `/books` waits up to a second
//! for a book's first (or post-reconnect) snapshot. Until the first catalogue has loaded,
//! `/markets`, `/markets/search`, `/markets/text-search` and `/markets/resolved` answer
//! `NOT_READY` (503). `/api/v1/trades` is the upstream's tape, passed through read only and
//! rate-limited ([`LiveLink::trades`], L5-4). `/mock/clock` is read-only. `GET /mock/info` reports the mode and the
//! upstream's health in both modes.

// Handlers return early with a ready `Response` as the error; boxing it buys nothing here.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::Router;
use axum::body::{Bytes, to_bytes};
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use pascal_rs::{ApiErrorCode, MicroUsd};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::engine::{Engine, Envelope};
use crate::live::LiveLink;
use crate::requests::{CancelOrder, MassCancel, PermitVerifier, PlaceOrder, Reject, is_pubkey};
use crate::script::{At, parse_price};
use crate::util::strip_nulls;

/// Most orders or cancels per batch.
pub const MAX_BATCH: usize = 50;
/// Most mass cancels per batch.
pub const MAX_MASS_CANCEL_BATCH: usize = 10;

/// Pascal closes a WebSocket with no client message for this long.
pub const DEFAULT_WS_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// State shared by the handlers.
pub struct Shared {
    engine: Mutex<Engine>,
    verifier: Arc<dyn PermitVerifier>,
    injections: Mutex<Vec<Injection>>,
    ws_idle_timeout: Duration,
    live: Option<Arc<LiveLink>>,
}

/// The locked engine. Dropping it publishes the request's account deltas
/// ([`Engine::flush`]), so `account` frames always show the state a request left behind.
pub struct EngineGuard<'a>(MutexGuard<'a, Engine>);

impl Deref for EngineGuard<'_> {
    type Target = Engine;
    fn deref(&self) -> &Engine {
        &self.0
    }
}

impl DerefMut for EngineGuard<'_> {
    fn deref_mut(&mut self) -> &mut Engine {
        &mut self.0
    }
}

impl Drop for EngineGuard<'_> {
    fn drop(&mut self) {
        self.0.flush();
    }
}

#[derive(Debug, Clone, Deserialize)]
struct Injection {
    code: String,
    #[serde(default = "one")]
    times: u32,
    #[serde(default)]
    path: Option<String>,
}

fn one() -> u32 {
    1
}

impl Shared {
    /// Wraps an engine.
    pub fn new(engine: Engine, verifier: Arc<dyn PermitVerifier>) -> Self {
        Shared {
            engine: Mutex::new(engine),
            verifier,
            injections: Mutex::new(Vec::new()),
            ws_idle_timeout: DEFAULT_WS_IDLE_TIMEOUT,
            live: None,
        }
    }

    /// Attaches the live mode's link.
    pub fn with_live(mut self, link: Arc<LiveLink>) -> Self {
        self.live = Some(link);
        self
    }

    /// The live mode's link (`None` in replay).
    pub fn live(&self) -> Option<&Arc<LiveLink>> {
        self.live.as_ref()
    }

    /// Live mode before the first catalogue: the reply for catalogue routes.
    fn catalogue_pending(&self) -> Option<Response> {
        let link = self.live.as_ref()?;
        (!link.catalogue_loaded()).then(|| {
            fail_code(
                "NOT_READY",
                "pascal-mock live: the upstream catalogue has not loaded yet",
            )
        })
    }

    /// Sets the WebSocket idle timeout (default [`DEFAULT_WS_IDLE_TIMEOUT`]).
    pub fn with_ws_idle_timeout(mut self, d: Duration) -> Self {
        self.ws_idle_timeout = d;
        self
    }

    /// The WebSocket idle timeout.
    pub fn ws_idle_timeout(&self) -> Duration {
        self.ws_idle_timeout
    }

    /// The engine, brought to the clock.
    pub fn engine(&self) -> EngineGuard<'_> {
        let mut e = self.engine.lock().unwrap_or_else(|p| p.into_inner());
        e.advance();
        EngineGuard(e)
    }

    fn take_injection(&self, path: &str) -> Option<String> {
        let mut inj = self.injections.lock().unwrap_or_else(|p| p.into_inner());
        let i = inj
            .iter()
            .position(|j| j.path.as_deref().is_none_or(|p| path.starts_with(p)))?;
        let code = inj[i].code.clone();
        inj[i].times -= 1;
        if inj[i].times == 0 {
            inj.remove(i);
        }
        Some(code)
    }
}

/// The router over `shared`.
pub fn router(shared: Arc<Shared>) -> Router {
    Router::new()
        .route("/api/v1/time", get(time))
        .route("/api/v1/markets", get(markets))
        .route("/api/v1/markets/resolved", get(resolved))
        .route("/api/v1/markets/search", post(search))
        .route("/api/v1/markets/text-search", post(text_search))
        .route("/api/v1/books", get(books))
        .route("/api/v1/trades", get(trades))
        .route("/api/v1/accounts/{owner}", get(account))
        .route("/api/v1/accounts/{owner}/fills", get(fills))
        .route("/api/v1/accounts/{owner}/builder-stats", get(builder_stats))
        .route("/ws", get(crate::ws::upgrade))
        .route("/api/v1/orders", post(orders))
        .route("/api/v1/cancels", post(cancels))
        .route("/api/v1/mass-cancels", post(mass_cancels))
        .route("/mock/clock", get(clock_get).post(clock_set))
        .route("/mock/resolve", post(mock_resolve))
        .route("/mock/inject", post(mock_inject))
        .route("/mock/info", get(mock_info))
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(shared.clone(), inject))
        .with_state(shared)
}

type S = State<Arc<Shared>>;
type Q = Query<HashMap<String, String>>;

// ---------- envelopes ----------

/// HTTP status for an error code.
pub fn http_status(code: &str) -> StatusCode {
    match code {
        "INVALID_JSON" => StatusCode::BAD_REQUEST,
        "UNAUTHORIZED" => StatusCode::UNAUTHORIZED,
        "RATE_LIMITED" => StatusCode::TOO_MANY_REQUESTS,
        "INTERNAL_ERROR" | "WRITE_RESULT_UNAVAILABLE" => StatusCode::INTERNAL_SERVER_ERROR,
        "BAD_GATEWAY" => StatusCode::BAD_GATEWAY,
        "SERVICE_UNAVAILABLE" | "BACKPRESSURE" | "NOT_READY" | "HISTORY_NOT_READY" => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        "TIMEOUT" => StatusCode::GATEWAY_TIMEOUT,
        c if c.ends_with("_NOT_FOUND") => StatusCode::NOT_FOUND,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    }
}

/// Codes that fail a whole write request rather than each item.
fn request_level(code: &str) -> bool {
    matches!(
        code,
        "RATE_LIMITED"
            | "BACKPRESSURE"
            | "SERVICE_UNAVAILABLE"
            | "NOT_READY"
            | "INVALID_JSON"
            | "TIMEOUT"
            | "BAD_GATEWAY"
            | "INTERNAL_ERROR"
            | "WRITE_RESULT_UNAVAILABLE"
    )
}

fn success_env(data: Value, env: Envelope, write: bool) -> Value {
    let mut v = json!({
        "status": "success",
        "data": data,
        "seq": env.seq.to_string(),
        "state_time_ms": env.time_ms.to_string(),
        "server_time_ms": env.time_ms.to_string(),
    });
    if write {
        v["server_receive_time_ms"] = env.time_ms.to_string().into();
    }
    v
}

fn error_env(code: &str, message: &str, env: Option<Envelope>, write: bool) -> Value {
    let mut v = json!({"status": "error", "data": {"code": code, "message": message}});
    if let Some(env) = env {
        v["seq"] = env.seq.to_string().into();
        v["state_time_ms"] = env.time_ms.to_string().into();
        v["server_time_ms"] = env.time_ms.to_string().into();
        if write {
            v["server_receive_time_ms"] = env.time_ms.to_string().into();
        }
    }
    v
}

fn reply(status: StatusCode, body: Value) -> Response {
    (status, axum::Json(body)).into_response()
}

fn ok(env: Envelope, data: impl Serialize) -> Response {
    match serde_json::to_value(data) {
        Ok(v) => reply(StatusCode::OK, success_env(strip_nulls(v), env, false)),
        Err(e) => fail_code("INTERNAL_ERROR", &e.to_string()),
    }
}

/// `data` already in its served form (raw tape JSON: nulls kept as recorded).
fn ok_raw(env: Envelope, data: Value) -> Response {
    reply(StatusCode::OK, success_env(data, env, false))
}

fn fail(r: Reject) -> Response {
    fail_code(r.code.as_str(), &r.message)
}

fn fail_code(code: &str, message: &str) -> Response {
    reply(http_status(code), error_env(code, message, None, false))
}

fn invalid(message: impl Into<String>) -> Response {
    fail(Reject::new(ApiErrorCode::InvalidRequest, message))
}

fn parse_json(body: &Bytes) -> Result<Value, Response> {
    serde_json::from_slice(body)
        .map_err(|e| fail_code("INVALID_JSON", &format!("invalid JSON body: {e}")))
}

// ---------- error injection ----------

async fn inject(State(shared): S, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_owned();
    if path.starts_with("/mock/") || path == "/ws" {
        return next.run(req).await;
    }
    let from_query = req.uri().query().and_then(|q| {
        q.split('&')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| *k == "inject")
            .map(|(_, v)| v.to_owned())
    });
    let Some(code) = from_query.or_else(|| shared.take_injection(&path)) else {
        return next.run(req).await;
    };
    if ApiErrorCode::parse(&code).is_none() {
        return invalid(format!("inject={code:?} is not a Pascal error code"));
    }
    let env = shared.engine().envelope();
    let message = format!("injected by pascal-mock: {code}");
    let batch = matches!(
        path.as_str(),
        "/api/v1/orders" | "/api/v1/cancels" | "/api/v1/mass-cancels"
    );
    if batch && !request_level(&code) {
        let body = to_bytes(req.into_body(), 4 << 20).await.unwrap_or_default();
        let n = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|v| v.as_array().map(Vec::len))
            .unwrap_or(1)
            .max(1);
        let items: Vec<Value> = (0..n)
            .map(|_| error_env(&code, &message, Some(env), true))
            .collect();
        return reply(http_status(&code), Value::Array(items));
    }
    reply(http_status(&code), error_env(&code, &message, None, batch))
}

async fn mock_inject(State(shared): S, body: Bytes) -> Response {
    let inj: Injection = match parse_json(&body).and_then(|v| {
        serde_json::from_value(v).map_err(|e| invalid(format!("deserialize_failed: {e}")))
    }) {
        Ok(i) => i,
        Err(r) => return r,
    };
    if inj.times == 0 {
        return invalid("times must be ≥ 1");
    }
    if ApiErrorCode::parse(&inj.code).is_none() {
        return invalid(format!("{:?} is not a Pascal error code", inj.code));
    }
    let echo = json!({"code": inj.code, "times": inj.times, "path": inj.path});
    shared
        .injections
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push(inj);
    let env = shared.engine().envelope();
    ok(env, echo)
}

// ---------- read API ----------

async fn not_found() -> Response {
    reply(
        StatusCode::NOT_FOUND,
        error_env("INVALID_REQUEST", "pascal-mock: no such route", None, false),
    )
}

async fn time(State(shared): S) -> Response {
    let env = shared.engine().envelope();
    ok(env, json!({"server_time_ms": env.time_ms.to_string()}))
}

async fn markets(State(shared): S) -> Response {
    if let Some(r) = shared.catalogue_pending() {
        return r;
    }
    let e = shared.engine();
    ok_raw(e.envelope(), Value::Array(e.live_markets()))
}

async fn resolved(State(shared): S, Query(q): Q) -> Response {
    let lookback_ms = match q.get("lookback").map_or("24h", String::as_str) {
        "4h" => 4 * 3_600_000,
        "24h" => 24 * 3_600_000,
        "3d" => 72 * 3_600_000,
        other => return invalid(format!("lookback={other}: expected 4h, 24h or 3d")),
    };
    if let Some(o) = q.get("order_by")
        && !matches!(
            o.as_str(),
            "resolution_time" | "volume_24h" | "volume_all_time"
        )
    {
        return invalid(format!("order_by={o}"));
    }
    let limit = match q.get("limit").map_or("50", String::as_str) {
        "50" => 50,
        "100" => 100,
        "200" => 200,
        other => return invalid(format!("limit={other}: expected 50, 100 or 200")),
    };
    if let Some(r) = shared.catalogue_pending() {
        return r;
    }
    let (env, mut items) = {
        let e = shared.engine();
        (e.envelope(), e.resolved_markets(lookback_ms, limit))
    };
    // Live: then the upstream's, for symbols the mock does not track (its own come first, and
    // a market it tracks resolves only through it). An upstream failure leaves the mock's list.
    if let Some(link) = shared.live() {
        let query = [
            (
                "lookback".to_owned(),
                q.get("lookback").cloned().unwrap_or_else(|| "24h".into()),
            ),
            (
                "order_by".to_owned(),
                q.get("order_by")
                    .cloned()
                    .unwrap_or_else(|| "resolution_time".into()),
            ),
            ("limit".to_owned(), limit.to_string()),
        ];
        if let Ok(up) = link.resolved(&query).await {
            let e = shared.engine();
            let mut seen: std::collections::HashSet<String> = items
                .iter()
                .filter_map(|m| m["symbol"].as_str().map(str::to_owned))
                .collect();
            for m in up {
                let Some(sym) = m["symbol"].as_str().map(str::to_owned) else {
                    continue;
                };
                if items.len() >= limit {
                    break;
                }
                if !e.knows(&sym) && seen.insert(sym) {
                    items.push(m);
                }
            }
        }
    }
    ok_raw(env, Value::Array(items))
}

#[derive(Deserialize)]
struct SearchBody {
    #[serde(default)]
    symbols: Vec<String>,
    #[serde(default)]
    events: Vec<String>,
    #[serde(default)]
    games: Vec<String>,
}

async fn search(State(shared): S, body: Bytes) -> Response {
    let b: SearchBody = match parse_json(&body).and_then(|v| {
        serde_json::from_value(v).map_err(|e| invalid(format!("deserialize_failed: {e}")))
    }) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if b.symbols.is_empty() && b.events.is_empty() && b.games.is_empty() {
        return invalid("at least one of symbols, events, games is required");
    }
    if b.symbols.len() > 500 || b.events.len() > 50 || b.games.len() > 50 {
        return invalid("at most 500 symbols, 50 events and 50 games");
    }
    if let Some(r) = shared.catalogue_pending() {
        return r;
    }
    let e = shared.engine();
    let found = e.search(&b.symbols, &b.events, &b.games);
    let env = e.envelope();
    if let Some(link) = shared.live() {
        // Unknown symbols are looked up upstream on the next backstop pass.
        let unknown: Vec<String> = b.symbols.iter().filter(|s| !e.knows(s)).cloned().collect();
        drop(e);
        link.want_search(unknown);
    }
    ok_raw(env, Value::Array(found))
}

#[derive(Deserialize)]
struct TextSearchBody {
    query: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    order_by: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn text_search(State(shared): S, body: Bytes) -> Response {
    let b: TextSearchBody = match parse_json(&body).and_then(|v| {
        serde_json::from_value(v).map_err(|e| invalid(format!("deserialize_failed: {e}")))
    }) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let q = b.query.trim();
    if q.is_empty() || q.chars().count() > 32 {
        return invalid("query must be 1 to 32 characters after trimming");
    }
    if b.tags.len() > 5 {
        return invalid("at most 5 tags");
    }
    let status = b.status.as_deref().unwrap_or("all");
    if !matches!(status, "live" | "resolved" | "all") {
        return invalid(format!("status={status}"));
    }
    if let Some(o) = &b.order_by
        && !matches!(o.as_str(), "volume_24h" | "volume_all_time")
    {
        return invalid(format!("order_by={o}"));
    }
    let limit = b.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return invalid("limit must be in 1..=100");
    }
    if let Some(r) = shared.catalogue_pending() {
        return r;
    }
    let e = shared.engine();
    ok_raw(
        e.envelope(),
        Value::Array(e.text_search(q, &b.tags, status, limit)),
    )
}

fn symbols_param(q: &HashMap<String, String>, max: usize) -> Result<Vec<String>, Response> {
    let symbols: Vec<String> = q
        .get("symbols")
        .map(|s| {
            s.split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if symbols.is_empty() {
        return Err(invalid("deserialize_failed: missing field `symbols`"));
    }
    if symbols.len() > max {
        return Err(invalid(format!("{} symbols, at most {max}", symbols.len())));
    }
    Ok(symbols)
}

async fn books(State(shared): S, Query(q): Q) -> Response {
    let symbols = match symbols_param(&q, 50) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Some(link) = shared.live() {
        link.track(&shared, &symbols);
        link.wait_fresh(&shared, &symbols).await;
    }
    let e = shared.engine();
    ok(e.envelope(), e.books(&symbols))
}

async fn trades(State(shared): S, Query(q): Q) -> Response {
    let symbols = match symbols_param(&q, 120) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let limit = match q.get("limit").map(|l| l.parse::<usize>()) {
        None => 100,
        Some(Ok(l)) if (1..=500).contains(&l) => l,
        Some(_) => return invalid("invalid request: limit must be in 1..=500"),
    };
    if q.contains_key("before_cursor") && q.contains_key("at_or_before_seq") {
        return invalid("give at most one of before_cursor and at_or_before_seq");
    }
    let at_or_before = match q.get("at_or_before_seq").map(|s| s.parse::<u64>()) {
        None => None,
        Some(Ok(s)) => Some(s),
        Some(Err(_)) => return invalid("at_or_before_seq must be a u64"),
    };
    if let Some(link) = shared.live() {
        return trades_upstream(link, symbols, limit, q.get("before_cursor"), at_or_before).await;
    }
    let before = match q.get("before_cursor").map(|c| (c, parse_cursor(c))) {
        None => None,
        Some((_, Some(b))) => Some(b),
        Some((c, None)) => return invalid(format!("before_cursor {c:?} is not a cursor")),
    };
    let e = shared.engine();
    let (items, next) = e.trades(&symbols, limit, before, at_or_before);
    let mut data = json!({"items": items});
    if let Some(c) = next {
        data["next_cursor"] = c.into();
    }
    ok_raw(e.envelope(), data)
}

/// Live mode (L5-4): the upstream's own page, with the checked query passed on (an upstream
/// cursor is opaque: only its length and characters are checked here).
async fn trades_upstream(
    link: &Arc<LiveLink>,
    symbols: Vec<String>,
    limit: usize,
    before: Option<&String>,
    at_or_before: Option<u64>,
) -> Response {
    let mut query = vec![
        ("symbols".to_owned(), symbols.join(",")),
        ("limit".to_owned(), limit.to_string()),
    ];
    if let Some(c) = before {
        let ok = !c.is_empty()
            && c.len() <= 256
            && c.chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "-_:.=+/".contains(ch));
        if !ok {
            return invalid(format!("before_cursor {c:?} is not a cursor"));
        }
        query.push(("before_cursor".to_owned(), c.clone()));
    }
    if let Some(s) = at_or_before {
        query.push(("at_or_before_seq".to_owned(), s.to_string()));
    }
    match link.trades(&query).await {
        Ok(body) => reply(StatusCode::OK, body),
        Err(e) => fail_code(&e.code, &e.message),
    }
}

async fn account(State(shared): S, Path(owner): Path<String>, Query(q): Q) -> Response {
    if !is_pubkey(&owner) {
        return invalid(format!("{owner:?} is not a base58 public key"));
    }
    let exclude = match q.get("exclude_orders").map(String::as_str) {
        None | Some("false") => false,
        Some("true") => true,
        Some(o) => return invalid(format!("exclude_orders={o}")),
    };
    let mut e = shared.engine();
    let state = e.account(&owner, q.get("symbol").map(String::as_str), exclude);
    ok(e.envelope(), state)
}

/// `before_cursor` of a paginated endpoint: `<seq>:<n>`.
fn parse_cursor(c: &str) -> Option<(u64, u64)> {
    let (s, i) = c.split_once(':')?;
    Some((s.parse().ok()?, i.parse().ok()?))
}

async fn fills(State(shared): S, Path(owner): Path<String>, Query(q): Q) -> Response {
    if !is_pubkey(&owner) {
        return invalid(format!("{owner:?} is not a base58 public key"));
    }
    let symbols: Vec<String> = q
        .get("symbols")
        .map(|s| {
            s.split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if symbols.len() > 100 {
        return invalid(format!("{} symbols, at most 100", symbols.len()));
    }
    let limit = match q.get("limit").map(|l| l.parse::<usize>()) {
        None => 100,
        Some(Ok(l)) if (1..=500).contains(&l) => l,
        Some(_) => return invalid("invalid request: limit must be in 1..=500"),
    };
    if q.contains_key("before_cursor") && q.contains_key("at_or_before_seq") {
        return invalid("give at most one of before_cursor and at_or_before_seq");
    }
    let before = match q.get("before_cursor").map(|c| (c, parse_cursor(c))) {
        None => None,
        Some((_, Some(b))) => Some(b),
        Some((c, None)) => return invalid(format!("before_cursor {c:?} is not a cursor")),
    };
    let at_or_before = match q.get("at_or_before_seq").map(|s| s.parse::<u64>()) {
        None => None,
        Some(Ok(s)) => Some(s),
        Some(Err(_)) => return invalid("at_or_before_seq must be a u64"),
    };
    let e = shared.engine();
    let (items, next) = e.fills_page(&owner, &symbols, limit, before, at_or_before);
    let mut data = json!({ "items": items });
    if let Some(c) = next {
        data["next_cursor"] = c.into();
    }
    ok(e.envelope(), data)
}

async fn builder_stats(State(shared): S, Path(owner): Path<String>) -> Response {
    if !is_pubkey(&owner) {
        return invalid(format!("{owner:?} is not a base58 public key"));
    }
    let e = shared.engine();
    ok(e.envelope(), e.builder_stats(&owner))
}

// ---------- write API ----------

fn batch_items(body: &Bytes, max: usize) -> Result<Vec<Value>, Response> {
    let v = parse_json(body)?;
    let Value::Array(items) = v else {
        return Err(invalid("request body must be a JSON array"));
    };
    if items.is_empty() || items.len() > max {
        return Err(invalid(format!(
            "batch of {} requests, expected 1..={max}",
            items.len()
        )));
    }
    Ok(items)
}

/// Serialises per-item results into the batch response.
fn batch_reply(results: Vec<(Result<Value, Reject>, Envelope)>) -> Response {
    let any_ok = results.iter().any(|(r, _)| r.is_ok());
    let first_err = results
        .iter()
        .find_map(|(r, _)| r.as_ref().err().map(|e| e.code));
    let items: Vec<Value> = results
        .into_iter()
        .map(|(r, env)| match r {
            Ok(data) => success_env(strip_nulls(data), env, true),
            Err(e) => error_env(e.code.as_str(), &e.message, Some(env), true),
        })
        .collect();
    let status = match (any_ok, first_err) {
        (false, Some(code)) => http_status(code.as_str()),
        _ => StatusCode::OK,
    };
    reply(status, Value::Array(items))
}

fn unauthorized(why: String) -> Reject {
    Reject::new(ApiErrorCode::Unauthorized, why)
}

async fn orders(State(shared): S, body: Bytes) -> Response {
    let items = match batch_items(&body, MAX_BATCH) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let parsed: Vec<Result<PlaceOrder, Reject>> = items
        .iter()
        .map(|v| {
            let o = PlaceOrder::parse(v)?;
            shared.verifier.verify_place(&o, v).map_err(unauthorized)?;
            Ok(o)
        })
        .collect();
    if let Some(link) = shared.live() {
        // An order in a market the mock does not follow yet subscribes its book (this order
        // is refused NOT_READY; the next one fills once the snapshot landed).
        let symbols: Vec<String> = parsed
            .iter()
            .filter_map(|o| o.as_ref().ok().map(|o| o.symbol.clone()))
            .collect();
        link.track(&shared, &symbols);
    }
    let mut e = shared.engine();
    let mut results: Vec<Option<(Result<Value, Reject>, Envelope)>> = vec![None; items.len()];
    // Matching rounds process post-only placements before other orders.
    let order: Vec<usize> = (0..items.len())
        .filter(|&i| matches!(&parsed[i], Ok(o) if o.post_only))
        .chain((0..items.len()).filter(|&i| !matches!(&parsed[i], Ok(o) if o.post_only)))
        .collect();
    for i in order {
        let r = match &parsed[i] {
            Err(r) => Err(r.clone()),
            Ok(o) => e.place(o).and_then(|res| {
                tracing::info!(
                    owner = %o.auth.owner, symbol = %o.symbol, side = ?o.side,
                    price = o.price_u, size = o.size, tif = ?o.tif,
                    filled = res.order.size_filled, fills = res.fills.len(),
                    "order"
                );
                serde_json::to_value(&res)
                    .map_err(|err| Reject::new(ApiErrorCode::InternalError, err.to_string()))
            }),
        };
        results[i] = Some((r, e.envelope()));
    }
    drop(e);
    batch_reply(results.into_iter().flatten().collect())
}

async fn cancels(State(shared): S, body: Bytes) -> Response {
    let items = match batch_items(&body, MAX_BATCH) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let mut e = shared.engine();
    let results = items
        .iter()
        .map(|v| {
            let r = CancelOrder::parse(v)
                .and_then(|c| {
                    shared.verifier.verify_cancel(&c, v).map_err(unauthorized)?;
                    e.cancel(&c)
                })
                .and_then(|o| {
                    serde_json::to_value(o)
                        .map_err(|err| Reject::new(ApiErrorCode::InternalError, err.to_string()))
                });
            (r, e.envelope())
        })
        .collect();
    drop(e);
    batch_reply(results)
}

async fn mass_cancels(State(shared): S, body: Bytes) -> Response {
    let items = match batch_items(&body, MAX_MASS_CANCEL_BATCH) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let mut e = shared.engine();
    let results = items
        .iter()
        .map(|v| {
            let r = MassCancel::parse(v)
                .and_then(|m| {
                    shared
                        .verifier
                        .verify_mass_cancel(&m, v)
                        .map_err(unauthorized)?;
                    e.mass_cancel(&m)
                })
                .and_then(|orders| {
                    serde_json::to_value(json!({ "orders": orders }))
                        .map_err(|err| Reject::new(ApiErrorCode::InternalError, err.to_string()))
                });
            (r, e.envelope())
        })
        .collect();
    drop(e);
    batch_reply(results)
}

// ---------- control ----------

/// `GET /mock/info` (L4-1): `{"mode","upstream","upstream_ok","books","stale_books"}` plus
/// `markets`, `catalogue_age_ms`, `socket_connected`, `recording` (live) or `tape` (replay).
/// A plain object, not a Pascal envelope.
async fn mock_info(State(shared): S) -> Response {
    let (counts, now) = {
        let e = shared.engine();
        (e.feed_counts(), e.now_ms())
    };
    let data = match shared.live() {
        Some(l) => json!({
            "mode": "live",
            "upstream": l.host(),
            "upstream_ok": l.upstream_ok(),
            "books": counts.books,
            "stale_books": counts.stale_books,
            "markets": counts.markets,
            "catalogue_age_ms": l.catalogue_ms().map(|t| now.saturating_sub(t)),
            "socket_connected": l.socket_up(),
            "recording": l.recording().map(|p| p.display().to_string()),
        }),
        None => json!({
            "mode": "replay",
            "upstream": "",
            "upstream_ok": false,
            "books": counts.books,
            "stale_books": 0,
            "tape": shared.engine().tape().dir.display().to_string(),
        }),
    };
    reply(StatusCode::OK, data)
}

async fn clock_get(State(shared): S) -> Response {
    let e = shared.engine();
    ok(e.envelope(), e.clock_info())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClockBody {
    #[serde(default)]
    tape_ms: Option<u64>,
    #[serde(default)]
    advance_ms: Option<u64>,
    #[serde(default)]
    speed: Option<f64>,
}

async fn clock_set(State(shared): S, body: Bytes) -> Response {
    let b: ClockBody = match parse_json(&body).and_then(|v| {
        serde_json::from_value(v).map_err(|e| invalid(format!("deserialize_failed: {e}")))
    }) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let mut e = shared.engine();
    if let Err(r) = e.set_clock(b.tape_ms, b.advance_ms, b.speed) {
        return fail(r);
    }
    ok(e.envelope(), e.clock_info())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveBody {
    symbol: String,
    price: Value,
    #[serde(default)]
    at: Option<Value>,
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

async fn mock_resolve(State(shared): S, body: Bytes) -> Response {
    let b: ResolveBody = match parse_json(&body).and_then(|v| {
        serde_json::from_value(v).map_err(|e| invalid(format!("deserialize_failed: {e}")))
    }) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let price: MicroUsd = match parse_price(&text(&b.price)) {
        Ok(p) => p,
        Err(err) => return invalid(err.to_string()),
    };
    let at = match b.at.as_ref().map(|a| At::parse(&text(a))) {
        None => At::Now,
        Some(Ok(a)) => a,
        Some(Err(err)) => return invalid(err.to_string()),
    };
    let mut e = shared.engine();
    match e.schedule_resolution(&b.symbol, price, at) {
        Ok(r) => {
            let followed = r.is_some() && e.has_live_book(&b.symbol);
            let data = json!({"symbol": b.symbol, "scheduled": r.is_none(), "resolution": r});
            let resp = ok(e.envelope(), data);
            drop(e);
            if followed && let Some(link) = shared.live() {
                link.unfollow(&b.symbol);
            }
            resp
        }
        Err(r) => fail(r),
    }
}
