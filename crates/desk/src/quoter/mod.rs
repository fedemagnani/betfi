//! Quoter (spec sections 2 and 5, plan step 2.A).
//!
//! | Route | What |
//! | --- | --- |
//! | `POST /quote` | C4 quote: legs from the catalogue, books from Pascal, `pricing::quote`, the C3 quote message signed by the quote signer, 10 s TTL |
//! | `GET /cashout-quote/{ticket}` | C4 cash-out quote: the ticket and its legs from the DB, closing-side prices, resolved legs at r̃, depth gate, the signed 99-byte cash-out message |
//! | `POST /ticket/{id}/copy-quote` | the original ticket's legs quoted fresh with `copied_from` and the 2% copy fee line; on by default for dev/demo and off for live, `DESK_COPY_QUOTE=0`/`false` turns it off (D-177); 422 unless the original is a minted, open own tip with every leg unresolved, in play included (D-660; `quote::copy_refusal`; `/quote` with `copied_from` runs the same check) |
//!
//! - **Store before answer** (l2-contracts L2-1): every signed quote is written to `quotes` and
//!   `quote_legs` before the response leaves, so the indexer can name the legs of a
//!   `PendingOpened` by `quote_hash`. Cash-out quotes are logged to `cashout_quotes`
//!   (`migrations/0003_quoter.sql`).
//! - **Books** ([`books`]): ≤ 1 s old or fetched; on a Pascal failure the last snapshot ≤ 10 s
//!   old, else `503 unavailable` (spec 12 runbook). Fetched books are also published to
//!   [`AppState::books`](crate::state::Shared::books).
//! - **Clocks**: pricing rules ("legs must not have started") use the venue's clock from the
//!   books envelope (tape time in mode M, D-033); `quote_expires_at` and the cash-out `ts` use
//!   the desk's wall clock, which is what the program's `Clock` checks them against.
//! - [`run`] registers the [`CashOutValuer`] (indicative values from cached books for
//!   `/tickets`, D-040) and writes `book_snapshots` every 10 s for the symbols with open
//!   interest, until shutdown.
//! - [`codec`] holds the two signed layouts, pinned to `fixtures/schedina/quote_vector.json`.
//!
//! State shared between the handlers and [`run`] is [`QuoterState`], reachable as
//! `AppState::quoter`.

pub mod books;
pub mod cashout;
pub mod codec;
pub mod quote;

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::Router;
use axum::routing::{get, post};

use crate::config::Config;
use crate::solana::signer::Keypair;
use crate::state::{AppState, CashOutValuer, Locked, Symbol};

pub use books::{CachedBook, LegBooks};
pub use cashout::Valuer;

/// Quote TTL, s (`quote_expires_at = now_s + TTL`; 10 s in L2-2, 5 min for the demo, D-664: a
/// wallet approval can take longer than 10 s, and a live price may move within it).
pub const QUOTE_TTL_S: i64 = 300;
/// A quoted symbol counts as open interest for the snapshot writer this long, ms.
pub const RECENT_QUOTE_MS: u64 = 5 * 60_000;
/// `book_snapshots` period.
pub const SNAPSHOT_EVERY: Duration = Duration::from_secs(10);
/// The indicative cash-out (`/tickets`) uses cached books at most this old, ms.
pub const VALUER_MAX_AGE_MS: u64 = 30_000;
/// Copy fee on a copied ticket, ppm of the stake (spec section 2: 2%).
pub const COPY_FEE_PPM: u64 = 20_000;

/// The quoter's own state: the book cache, recently quoted symbols, the Pascal back-off and the
/// quote-signer key. Lives in [`AppState::quoter`](crate::state::Shared::quoter).
#[derive(Default)]
pub struct QuoterState {
    books: Locked<HashMap<Symbol, CachedBook>>,
    recent: Locked<HashMap<Symbol, u64>>,
    backoff_until_ms: AtomicU64,
    signer: OnceLock<Arc<Keypair>>,
    /// Pricing knobs (spec defaults).
    pub params: pricing::Params,
}

impl std::fmt::Debug for QuoterState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuoterState")
            .field("books", &self.books.read().len())
            .field("signer", &self.signer.get())
            .finish_non_exhaustive()
    }
}

impl QuoterState {
    /// The quote signer, read from `config.quote_signer_keypair_path` on first use (a missing
    /// file is retried on the next call, so a key installed after start is picked up).
    pub fn signer(&self, config: &Config) -> Result<Arc<Keypair>, String> {
        if let Some(k) = self.signer.get() {
            return Ok(k.clone());
        }
        let k = Arc::new(Keypair::read(&config.quote_signer_keypair_path)?);
        Ok(self.signer.get_or_init(|| k).clone())
    }

    /// Records that `symbols` were just quoted (they get book snapshots for a while).
    pub fn note_quoted<'a>(&self, symbols: impl IntoIterator<Item = &'a String>, now_ms: u64) {
        let mut recent = self.recent.write();
        recent.retain(|_, t| now_ms.saturating_sub(*t) <= RECENT_QUOTE_MS);
        for s in symbols {
            recent.insert(s.clone(), now_ms);
        }
    }

    /// Symbols quoted in the last [`RECENT_QUOTE_MS`].
    pub fn recent_symbols(&self, now_ms: u64) -> BTreeSet<String> {
        self.recent
            .read()
            .iter()
            .filter(|(_, t)| now_ms.saturating_sub(**t) <= RECENT_QUOTE_MS)
            .map(|(s, _)| s.clone())
            .collect()
    }
}

/// Adds `POST /quote`, `GET /cashout-quote/{ticket}` and `POST /ticket/{id}/copy-quote`.
pub fn mount(router: Router<AppState>, _state: &AppState) -> Router<AppState> {
    router
        .route("/quote", post(quote::post_quote))
        .route("/cashout-quote/{ticket}", get(cashout::get_cashout_quote))
        .route("/ticket/{id}/copy-quote", post(quote::post_copy_quote))
}

/// Registers the cash-out valuer, then writes `book_snapshots` every [`SNAPSHOT_EVERY`] until
/// shutdown.
pub async fn run(state: AppState) {
    let valuer: Arc<dyn CashOutValuer> = Arc::new(Valuer::new(state.quoter.clone()));
    if state.cash_out.set(valuer).is_err() {
        tracing::warn!("a cash-out valuer was already registered; keeping it");
    }
    match state.quoter.signer(&state.config) {
        Ok(k) => tracing::info!(quote_signer = %k.pubkey(), "quoter ready"),
        Err(e) => {
            tracing::warn!(error = %e, "no quote signer yet: /quote answers 503 until it exists")
        }
    }
    let mut tick = tokio::time::interval(SNAPSHOT_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = state.shutdown_requested() => break,
            _ = tick.tick() => {}
        }
        tokio::select! {
            _ = state.shutdown_requested() => break,
            r = books::snapshot_once(&state) => match r {
                Ok(n) => {
                    metrics::counter!("desk_book_snapshots_total").increment(n as u64);
                    if n > 0 {
                        tracing::debug!(rows = n, "book snapshots written");
                    }
                }
                Err(e) => tracing::warn!(error = %e, "book snapshot pass failed"),
            },
        }
    }
    tracing::info!("quoter stopped");
}
