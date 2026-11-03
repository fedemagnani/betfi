//! Tipster reputation and copies (spec sections 4, 6 and 10; plan step 4.A).
//!
//! | Route | What |
//! | --- | --- |
//! | `GET /tipsters?sort=roi\|brier\|skin\|followers_roi&all=0\|1` | the board ([`metrics`]); default `roi`, filtered to `settled ≥ 5` and `skin ≥ 20 USDC` unless `all=1` |
//! | `GET /tipsters/{wallet}` | one tipster's row, cumulative ROI and Brier per settled tip, and their settled and open own tips as C4 tickets; 404 without a minted own tip |
//! | `GET /tipsters/{wallet}/copiers?ticket=<pda>` | the minted copies of one own tip, newest first, with the pool (7.0c) |
//!
//! 7.0c (l7-contracts L7-1): every tip `GET /tipsters/{wallet}` lists (open and settled) carries
//! `pool_u` (Σ rS of its minted copies, self-copies included like `copies`, D-171/D-332),
//! `copies` and `teams` ([`teams_of`]).
//!
//! Rows carry the 6.0c additions (l6-contracts L6-2): `handle` (from [`crate::profiles`]),
//! `verified`, `roi_30d_ppm` and `copiers` ([`metrics`]); tip tickets carry their `title`.
//!
//! Computed on request from the indexer's `tickets` / `ticket_legs` rows and the `tipster_paid`
//! table ([`store`], `migrations/0006_tipsters.sql`); nothing is cached, so a settle shows up on
//! the next request. The program's `TipsterProfile` keeps the same counts on-chain (settled,
//! won, staked, returned); the desk adds what needs per-leg data (Brier, edge), copies and
//! fees.

pub mod chain;
pub mod metrics;
pub mod store;

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use solana_pubkey::Pubkey;

use crate::api::ApiError;
use crate::api::types::{
    Copier, CopiersResponse, MarketKind, TeamSide, Ticket, TipsterResponse, TipsterRow,
    TipstersResponse,
};
use crate::crank::tickets::load_ticket;
use crate::now_ms;
use crate::state::AppState;
use metrics::{Scored, Sort};

/// Most tickets `/tipsters/{wallet}` lists per section.
pub const MAX_TICKETS: i64 = 200;
/// Most rows `/tipsters/{wallet}/copiers` lists (`pool_u` still counts every copy).
pub const MAX_COPIERS: i64 = 500;
/// The states of a minted copy.
const MINTED: &str = "('open', 'settled', 'cashed_out')";

/// Adds `GET /tipsters`, `GET /tipsters/{wallet}` and `GET /tipsters/{wallet}/copiers`.
pub fn mount(router: Router<AppState>, _state: &AppState) -> Router<AppState> {
    router
        .route("/tipsters", get(board))
        .route("/tipsters/{wallet}", get(tipster))
        .route("/tipsters/{wallet}/copiers", get(copiers))
}

/// `(copies, pool_u)` of `tip`: its minted copies and Σ of their stakes (rS).
pub async fn pool_of(db: &sqlx::SqlitePool, tip: &str) -> Result<(u32, u64), sqlx::Error> {
    let (n, sum): (i64, i64) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT COUNT(*), COALESCE(SUM(COALESCE(rs_u, stake_u)), 0) FROM tickets
         WHERE copied_from = ? AND state IN {MINTED}"
    )))
    .bind(tip)
    .fetch_one(db)
    .await?;
    Ok((
        u32::try_from(n).unwrap_or(u32::MAX),
        u64::try_from(sum).unwrap_or(0),
    ))
}

/// The short code of each leg's pick, in leg order: on an NBA symbol the catalogue's
/// abbreviation of the picked team (`Over`/`Under` → `OVR`/`UND` on a total), on any other the
/// symbol's team code (after its last `.`), falling back to the abbreviation.
pub async fn teams_of(db: &sqlx::SqlitePool, t: &Ticket) -> Result<Vec<String>, sqlx::Error> {
    // One read for every leg of the ticket.
    let abbrs: std::collections::HashMap<String, (String, String)> =
        sqlx::query_as::<_, (String, String, String)>(
            "SELECT symbol, away_abbr, home_abbr FROM markets
          WHERE symbol IN (SELECT symbol FROM ticket_legs WHERE ticket = ?)",
        )
        .bind(&t.ticket)
        .fetch_all(db)
        .await?
        .into_iter()
        .map(|(s, a, h)| (s, (a, h)))
        .collect();
    let mut out = Vec::with_capacity(t.legs.len());
    for l in &t.legs {
        let picked = abbrs
            .get(&l.symbol)
            .map(|(a, h)| if l.pick == TeamSide::Home { h } else { a }.clone());
        let suffix = l
            .symbol
            .rsplit_once('.')
            .map(|(_, c)| c.to_owned())
            .filter(|c| !c.is_empty());
        let code = if l.symbol.starts_with("NBA_") {
            if l.kind == Some(MarketKind::Total) {
                let over = l
                    .label
                    .as_deref()
                    .is_some_and(|x| x.to_ascii_lowercase().starts_with("over"));
                Some(if over { "OVR" } else { "UND" }.to_owned())
            } else {
                picked.or(suffix)
            }
        } else {
            suffix.or(picked)
        };
        // A leg not named yet (its `sha256:` id, D-573) has no code to show.
        let unnamed = l.symbol.starts_with("sha256:");
        out.push(code.filter(|_| !unnamed).unwrap_or_else(|| {
            if unnamed {
                "?".into()
            } else {
                l.symbol.clone()
            }
        }));
    }
    Ok(out)
}

/// Sets a tip's `pool_u`, `copies` and `teams` (L7-1).
pub async fn decorate_tip(db: &sqlx::SqlitePool, t: &mut Ticket) -> Result<(), sqlx::Error> {
    let (copies, pool_u) = pool_of(db, &t.ticket).await?;
    t.copies = Some(copies);
    t.pool_u = Some(pool_u);
    t.teams = Some(teams_of(db, t).await?);
    Ok(())
}

/// The copiers of `wallet`'s own tip `tip`, or `None` when `tip` is not one.
pub async fn copiers_of(
    db: &sqlx::SqlitePool,
    wallet: &str,
    tip: &str,
) -> Result<Option<CopiersResponse>, sqlx::Error> {
    let own: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM tickets WHERE ticket = ? AND user = ? AND tipster = user
         AND copied_from IS NULL",
    )
    .bind(tip)
    .bind(wallet)
    .fetch_optional(db)
    .await?;
    if own.is_none() {
        return Ok(None);
    }
    let rows: Vec<(String, i64, Option<String>)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT user, COALESCE(rs_u, stake_u), tx_open_pending FROM tickets
         WHERE copied_from = ? AND state IN {MINTED}
         ORDER BY COALESCE(minted_at, opened_at, 0) DESC, nonce DESC LIMIT ?"
    )))
    .bind(tip)
    .bind(MAX_COPIERS)
    .fetch_all(db)
    .await?;
    let wallets: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
    let handles = crate::profiles::handles_of(db, &wallets).await?;
    let (_, pool_u) = pool_of(db, tip).await?;
    Ok(Some(CopiersResponse {
        ticket: tip.to_owned(),
        copiers: rows
            .into_iter()
            .map(|(w, stake, tx)| Copier {
                handle: handles.get(&w).cloned(),
                wallet: w,
                stake_u: u64::try_from(stake).unwrap_or(0),
                tx,
            })
            .collect(),
        pool_u,
    }))
}

/// `GET /tipsters/{wallet}/copiers` query.
#[derive(Debug, Default, Deserialize)]
pub struct CopiersQuery {
    /// The own tip's Ticket PDA.
    #[serde(default)]
    pub ticket: Option<String>,
}

/// `GET /tipsters/{wallet}/copiers?ticket=<pda>`: 422 for a bad wallet or ticket, 404 when the
/// ticket is not an own tip of the wallet.
async fn copiers(
    State(state): State<AppState>,
    Path(wallet): Path<String>,
    Query(q): Query<CopiersQuery>,
) -> Result<Json<CopiersResponse>, ApiError> {
    if wallet.parse::<Pubkey>().is_err() {
        return Err(ApiError::rejected(format!(
            "{wallet} is not a wallet address"
        )));
    }
    let Some(tip) = q.ticket.filter(|t| t.parse::<Pubkey>().is_ok()) else {
        return Err(ApiError::rejected("ticket=<Ticket PDA> is required"));
    };
    match copiers_of(&state.db, &wallet, &tip).await? {
        Some(c) => Ok(Json(c)),
        None => Err(ApiError::not_found(format!(
            "{tip} is not a tip of {wallet}"
        ))),
    }
}

fn now_s() -> i64 {
    i64::try_from(now_ms() / 1000).unwrap_or(i64::MAX)
}

/// `GET /tipsters` query.
#[derive(Debug, Default, Deserialize)]
pub struct BoardQuery {
    /// `roi` (default), `brier`, `skin`, `followers_roi`.
    #[serde(default)]
    pub sort: Option<String>,
    /// `1`: everyone with a minted own tip; `0` (default): the filtered board.
    #[serde(default)]
    pub all: Option<String>,
}

/// The board as `GET /tipsters` serves it.
pub async fn board_of(
    state: &AppState,
    sort: Sort,
    all: bool,
) -> Result<TipstersResponse, sqlx::Error> {
    let now = now_s();
    let mut rows: Vec<Scored> = store::books(&state.db, None)
        .await?
        .iter()
        .map(|b| metrics::score(b, now))
        .filter(|s| all || metrics::on_board(&s.row))
        .collect();
    metrics::sort_board(&mut rows, sort);
    let wallets: Vec<String> = rows.iter().map(|s| s.row.wallet.clone()).collect();
    let handles = crate::profiles::handles_of(&state.db, &wallets).await?;
    Ok(TipstersResponse {
        as_of_ms: now_ms(),
        tipsters: rows
            .into_iter()
            .map(|s| TipsterRow {
                handle: handles.get(&s.row.wallet).cloned(),
                ..s.row
            })
            .collect(),
    })
}

/// `GET /tipsters`.
async fn board(
    State(state): State<AppState>,
    Query(q): Query<BoardQuery>,
) -> Result<Json<TipstersResponse>, ApiError> {
    let sort: Sort = q
        .sort
        .as_deref()
        .unwrap_or("")
        .parse()
        .map_err(ApiError::rejected)?;
    let all = match q.all.as_deref() {
        None | Some("" | "0") => false,
        Some("1") => true,
        Some(other) => return Err(ApiError::rejected(format!("all={other}: expected 0 or 1"))),
    };
    Ok(Json(board_of(&state, sort, all).await?))
}

/// One tipster as `GET /tipsters/{wallet}` serves it, or `None` without a minted own tip.
pub async fn tipster_of(
    state: &AppState,
    wallet: &str,
) -> Result<Option<TipsterResponse>, sqlx::Error> {
    let Some(book) = store::books(&state.db, Some(wallet)).await?.pop() else {
        return Ok(None);
    };
    let mut scored = metrics::score(&book, now_s());
    scored.row.handle = crate::profiles::handles_of(&state.db, &[wallet.to_owned()])
        .await?
        .remove(wallet);
    let mut settled = Vec::new();
    for id in store::own_tip_ids(&state.db, wallet, true, MAX_TICKETS).await? {
        settled.extend(load_ticket(state, &id).await?);
    }
    let mut open = Vec::new();
    for id in store::own_tip_ids(&state.db, wallet, false, MAX_TICKETS).await? {
        open.extend(load_ticket(state, &id).await?);
    }
    for t in settled.iter_mut().chain(open.iter_mut()) {
        decorate_tip(&state.db, t).await?;
    }
    Ok(Some(TipsterResponse {
        tipster: scored.row,
        history: scored.history,
        settled,
        open,
    }))
}

/// `GET /tipsters/{wallet}`.
async fn tipster(
    State(state): State<AppState>,
    Path(wallet): Path<String>,
) -> Result<Json<TipsterResponse>, ApiError> {
    if wallet.parse::<Pubkey>().is_err() {
        return Err(ApiError::rejected(format!(
            "{wallet} is not a wallet address"
        )));
    }
    match tipster_of(&state, &wallet).await? {
        Some(t) => Ok(Json(t)),
        None => Err(ApiError::not_found(format!("{wallet} has no tips"))),
    }
}
