//! HTTP/WebSocket API: the router, one `mount()` per service module, `/health`, `/metrics`,
//! `WS /stream`, and the C4 error bodies.

pub mod stream;
pub mod types;

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;
use crate::{catalogue, crank, hedge, indexer, now_ms, quoter};
use types::{ErrorBody, ErrorStatus};

/// The whole router. Each service module adds its routes in its own `mount()`.
pub fn router(state: AppState) -> Router {
    let r = Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics))
        .route("/stream", get(stream::stream));
    let r = catalogue::mount(r, &state);
    let r = quoter::mount(r, &state);
    let r = hedge::mount(r, &state);
    let r = crank::mount(r, &state);
    let r = indexer::mount(r, &state);
    let r = crate::swap::mount(r, &state);
    let r = crate::sentiment::mount(r, &state);
    let r = crate::profiles::mount(r, &state);
    let r = crate::fx::mount(r, &state);
    let r = crate::relay::mount(r, &state);
    r.layer(TraceLayer::new_for_http())
        // The app is served from another origin (dx dev server, static hosting).
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// `GET /health`: liveness plus the catalogue's freshness.
async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let cat = state.catalogue();
    Json(json!({
        "status": "ok",
        "profile": state.config.profile.as_str(),
        "now_ms": now_ms(),
        "paused": state.is_paused(),
        "catalogue": cat.as_ref().map(|c| json!({
            "as_of_ms": c.as_of_ms,
            "fetched_ms": c.fetched_ms,
            "markets": c.markets.len(),
        })),
    }))
}

/// `GET /metrics`: Prometheus text format.
async fn metrics() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        crate::metrics::render(),
    )
}

/// An API error with the C4 body: 422 `rejected`, 503 `unavailable`, 404 `not_found`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError(pub ErrorBody);

impl ApiError {
    /// 422: refused (correlation rule, bad parameters, ...).
    pub fn rejected(reason: impl Into<String>) -> Self {
        Self::with(ErrorStatus::Rejected, reason)
    }

    /// 503: cannot answer now.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::with(ErrorStatus::Unavailable, reason)
    }

    /// 404: unknown ticket or resource.
    pub fn not_found(reason: impl Into<String>) -> Self {
        Self::with(ErrorStatus::NotFound, reason)
    }

    fn with(status: ErrorStatus, reason: impl Into<String>) -> Self {
        ApiError(ErrorBody {
            status,
            reason: reason.into(),
        })
    }

    /// The HTTP status code.
    pub fn status_code(&self) -> StatusCode {
        match self.0.status {
            ErrorStatus::Rejected => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorStatus::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            ErrorStatus::NotFound => StatusCode::NOT_FOUND,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status_code(), Json(self.0)).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!(error = %e, "database error");
        ApiError::unavailable("database error")
    }
}
