//! Schedina desk (spec section 5): one axum + tokio binary running the catalogue, quoter, hedge
//! desk, crank and indexer behind a single HTTP/WebSocket API.
//!
//! Step 1.E froze the shell so that 2.A, 2.B and 2.C can work in parallel inside this crate:
//!
//! | Module          | Owner | What it holds                                                    |
//! | --------------- | ----- | ---------------------------------------------------------------- |
//! | [`config`]      | 1.E   | configuration from the environment                               |
//! | [`state`]       | 1.E   | [`AppState`], the [`Pascal`] handle, the [`DeskEvent`] bus        |
//! | [`db`]          | 1.E   | SQLite pool and migrations (`migrations/0001_desk.sql`)          |
//! | [`api`]         | 1.E   | router, C4 JSON types, errors, `/health`, `/metrics`, `/stream`  |
//! | [`catalogue`]   | 1.E   | `/markets` every 60 s, `GET /card`                               |
//! | [`solana`]      | 1.E   | keypair signer, JSON-RPC client, program log listener            |
//! | [`quoter`]      | 2.A   | `POST /quote`, `/cashout-quote`, book snapshots                  |
//! | [`hedge`]       | 2.B   | the section 3 hedge state machine                                |
//! | [`crank`]       | 2.C   | finalize, refund, resolve, settle; `/tickets`, `/ticket`         |
//! | [`indexer`]     | 2.C   | program events into the DB and onto the bus                      |
//! | [`sentiment`]   | 5.0c  | `GET /sentiment`: who backs which team (Pascal trades, DB)       |
//! | [`profiles`]    | 6.0c  | `PUT /profile`, `GET /profile/{wallet}`: signed handles, titles  |
//! | [`fx`]          | 6.0c  | `GET /fx`: the daily ECB EUR/USD rate, cached, with a fallback   |
//! | [`relay`]       | 8.B   | `GET`/`POST /relay`: the sponsor co-signs users' transactions    |
//!
//! Every service module exposes `pub fn mount(router, state) -> Router` and
//! `pub async fn run(state)`. Shared edits to `state.rs`, `api/types.rs` or the migrations are
//! made only by adding (a field, a variant, a migration file), never by changing a line.

pub mod api;
pub mod catalogue;
pub mod config;
pub mod crank;
pub mod db;
pub mod fx;
pub mod hedge;
pub mod house;
pub mod indexer;
pub mod metrics;
pub mod pascal;
pub mod profiles;
pub mod program;
pub mod quoter;
pub mod relay;
pub mod scores;
pub mod sentiment;
pub mod solana;
pub mod state;
pub mod swap;
pub mod tipsters;

mod app;

pub use app::{BoxError, Running, start};
pub use config::{Config, Profile};
pub use state::{AppState, DeskEvent, Pascal};

/// Unix milliseconds now.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}
