//! `GET /house` (l4-contracts L4-2, plan step 4.A): the house's books at a glance.
//!
//! - **vault**: a fresh read of the `Config` and `Vault` accounts (3 s budget); when the read
//!   fails, the last `chain_config` row the indexer wrote; zeros with the chain off.
//! - **tickets**: rows per state in `tickets`.
//! - **hedger**: the venue (D-007), the hedger's free collateral from a fresh `/accounts` read
//!   (0 when unknown), and per symbol the net position (`AppState::held`) against the hedge
//!   desk's net target, as `GET /debug/hedge` shows them.
//! - **fees**: `margin_u` = Σ `Ticket.fee_u` (`rS × m(n)`, what `finalize` records for the books)
//!   of minted tickets; Pascal and builder fees summed over `pascal_fills`; `copy_fees_u` = the
//!   house's part of the copy fees, i.e. the fees charged on minted copies minus Σ `TipsterPaid`
//!   (D-172).

use std::collections::BTreeSet;
use std::time::Duration;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use pascal_rs::AccountQuery;
use sqlx::SqlitePool;

use crate::api::ApiError;
use crate::api::types::{
    HouseFees, HouseHedger, HousePosition, HouseResponse, HouseTickets, HouseVault,
};
use crate::hedge::targets;
use crate::now_ms;
use crate::program::{ConfigAccount, Pdas, VaultAccount};
use crate::solana::rpc::RpcClient;
use crate::state::AppState;

/// Budget of each fresh read (chain, Pascal account).
pub const READ_BUDGET: Duration = Duration::from_secs(3);

/// Adds `GET /house`.
pub fn mount(router: Router<AppState>, _state: &AppState) -> Router<AppState> {
    router.route("/house", get(house))
}

/// `GET /house`.
async fn house(State(state): State<AppState>) -> Result<Json<HouseResponse>, ApiError> {
    Ok(Json(house_of(&state).await?))
}

/// The `/house` view now.
pub async fn house_of(state: &AppState) -> Result<HouseResponse, sqlx::Error> {
    let vault = vault(state).await?;
    let tickets = ticket_counts(&state.db).await?;
    let fees = fees(&state.db).await?;
    let hedger = hedger(state).await;
    Ok(HouseResponse {
        as_of_ms: now_ms(),
        profile: state.config.profile.as_str().to_owned(),
        vault,
        tickets,
        hedger,
        fees,
    })
}

fn u(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

async fn vault(state: &AppState) -> Result<HouseVault, sqlx::Error> {
    if !state.config.chain_enabled {
        return Ok(HouseVault::default());
    }
    let pdas = Pdas::new(state.config.program_id);
    let rpc = RpcClient::new(&state.config.solana_rpc_url);
    let read = tokio::time::timeout(
        READ_BUDGET,
        rpc.get_multiple_accounts(&[pdas.config(), pdas.vault()]),
    )
    .await;
    if let Ok(Ok((_, accs))) = read
        && let [Some(c), Some(v)] = accs.as_slice()
        && let Ok(c) = ConfigAccount::decode(&c.data, &c.owner, &pdas.program)
        && let Ok(v) = VaultAccount::decode(&v.data, &v.owner, &pdas.program)
    {
        return Ok(HouseVault {
            free_u: v.free,
            locked_u: v.locked,
            reserve_u: c.reserve,
        });
    }
    tracing::debug!("house: fresh vault read failed; using the indexer's last read");
    let row: Option<(Option<i64>, Option<i64>, i64)> = sqlx::query_as(
        "SELECT vault_free_u, vault_locked_u, reserve_u FROM chain_config WHERE id = 1",
    )
    .fetch_optional(&state.db)
    .await?;
    Ok(
        row.map_or_else(HouseVault::default, |(f, l, r)| HouseVault {
            free_u: f.map_or(0, u),
            locked_u: l.map_or(0, u),
            reserve_u: u(r),
        }),
    )
}

async fn ticket_counts(db: &SqlitePool) -> Result<HouseTickets, sqlx::Error> {
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT state, COUNT(*) FROM tickets GROUP BY state")
            .fetch_all(db)
            .await?;
    let mut t = HouseTickets::default();
    for (state, n) in rows {
        let n = u(n);
        match state.as_str() {
            "pending" => t.pending = n,
            "open" => t.open = n,
            "settled" => t.settled = n,
            "cashed_out" => t.cashed_out = n,
            "refunded" => t.refunded = n,
            _ => {}
        }
    }
    Ok(t)
}

async fn fees(db: &SqlitePool) -> Result<HouseFees, sqlx::Error> {
    let (margin, copy_charged): (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(chain_fee_u), 0),
                COALESCE(SUM(CASE WHEN copied_from IS NOT NULL THEN copy_fee_u ELSE 0 END), 0)
         FROM tickets WHERE state IN ('open', 'settled', 'cashed_out')",
    )
    .fetch_one(db)
    .await?;
    let (pascal, builder): (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(fee_u), 0), COALESCE(SUM(builder_fee_u), 0) FROM pascal_fills",
    )
    .fetch_one(db)
    .await?;
    let tipsters: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(amount_u), 0) FROM tipster_paid")
        .fetch_one(db)
        .await?;
    Ok(HouseFees {
        margin_u: u(margin),
        pascal_fees_u: pascal,
        builder_fees_u: u(builder),
        copy_fees_u: u(copy_charged.saturating_sub(tipsters)),
    })
}

async fn hedger(state: &AppState) -> HouseHedger {
    let free_collateral_u = match &state.config.hedger_owner {
        Some(owner) => {
            let q = AccountQuery {
                symbol: None,
                exclude_orders: true,
            };
            match tokio::time::timeout(READ_BUDGET, state.pascal.account(owner, &q)).await {
                Ok(Ok(r)) => r.data.collateral_usd.unwrap_or(0),
                Ok(Err(e)) => {
                    tracing::debug!(error = %e, "house: hedger account read failed");
                    0
                }
                Err(_) => 0,
            }
        }
        None => 0,
    };
    HouseHedger {
        venue: crate::hedge::trader::venue_of(&state.config),
        free_collateral_u,
        positions: positions(state),
    }
}

/// Held against target per symbol. The target is the hedge desk's own net target
/// (`Hedger::target`: resolved legs at r̃ and 4.C's pre-hedge marks), the same number
/// `/debug/hedge` reports. One lock at a time (D-043): the hedge desk takes its own, then held.
fn positions(state: &AppState) -> Vec<HousePosition> {
    let hedger = state
        .hedge
        .get()
        .and_then(std::sync::Weak::upgrade)
        .map(crate::hedge::Hedger::from_inner);
    let mut targets_by: Vec<(String, i64)> = Vec::new();
    if let Some(h) = hedger {
        let symbols = targets::symbols_of(&state.tickets.read());
        for s in symbols {
            let t = h.target(&s);
            targets_by.push((s, t));
        }
    }
    let held: Vec<(String, i64)> = state
        .held
        .read()
        .iter()
        .map(|(s, h)| (s.clone(), *h))
        .collect();
    let symbols: BTreeSet<&String> = targets_by.iter().chain(&held).map(|(s, _)| s).collect();
    symbols
        .into_iter()
        .map(|s| HousePosition {
            symbol: s.clone(),
            held: held.iter().find(|(x, _)| x == s).map_or(0, |(_, h)| *h),
            target: targets_by
                .iter()
                .find(|(x, _)| x == s)
                .map_or(0, |(_, t)| *t),
        })
        .collect()
}
