//! `GET /tickets/:user` and `GET /ticket/:id` (C4 + its 29 Oct amendment, 404 per D-039), built
//! from the DB rows the indexer writes (`tickets`, `ticket_legs`) plus the hedge desk's fills
//! (`fill_allocations` × `pascal_fills`) and the catalogue's team names (`markets`). The live
//! cash-out value of an open ticket comes from [`AppState::cash_out_for`] (D-040), asked with a
//! [`TicketState`] built from the same rows (L2-3).

use axum::Json;
use axum::extract::{Path, State};
use solana_pubkey::Pubkey;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::api::ApiError;
use crate::api::types::{
    CashOut, LegStatus, OrderSide, QuoteLeg, Side, TeamSide, Ticket, TicketFill, TicketLeg,
    TicketStatus, TicketTx, TicketsResponse, TxConfirm, TxSlots, Venue,
};
use crate::state::{AppState, LegState, TicketState};

/// Most tickets `/tickets/:user` returns (newest first).
pub const MAX_TICKETS: i64 = 200;

/// `GET /tickets/:user`.
pub async fn tickets(
    State(state): State<AppState>,
    Path(user): Path<String>,
) -> Result<Json<TicketsResponse>, ApiError> {
    if user.parse::<Pubkey>().is_err() {
        return Err(ApiError::rejected(format!(
            "{user} is not a wallet address"
        )));
    }
    Ok(Json(TicketsResponse {
        tickets: tickets_of(&state, &user).await?,
        user,
    }))
}

/// `GET /ticket/:id` (the Ticket PDA).
pub async fn ticket(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Ticket>, ApiError> {
    match load_ticket(&state, &id).await? {
        Some(t) => Ok(Json(t)),
        None => Err(ApiError::not_found(format!("no ticket {id}"))),
    }
}

/// `user`'s tickets, newest first.
pub async fn tickets_of(state: &AppState, user: &str) -> Result<Vec<Ticket>, sqlx::Error> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT ticket FROM tickets WHERE user = ?
         ORDER BY COALESCE(opened_at, minted_at, 0) DESC, nonce DESC LIMIT ?",
    )
    .bind(user)
    .bind(MAX_TICKETS)
    .fetch_all(&state.db)
    .await?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(t) = load_ticket(state, &id).await? {
            out.push(t);
        }
    }
    Ok(out)
}

fn u(row: &SqliteRow, col: &str) -> u64 {
    row.try_get::<Option<i64>, _>(col)
        .ok()
        .flatten()
        .and_then(|v| u64::try_from(v).ok())
        .unwrap_or(0)
}

fn opt_u(row: &SqliteRow, col: &str) -> Option<u64> {
    row.try_get::<Option<i64>, _>(col)
        .ok()
        .flatten()
        .and_then(|v| u64::try_from(v).ok())
}

fn opt_i(row: &SqliteRow, col: &str) -> Option<i64> {
    row.try_get::<Option<i64>, _>(col).ok().flatten()
}

fn s(row: &SqliteRow, col: &str) -> String {
    row.try_get::<Option<String>, _>(col)
        .ok()
        .flatten()
        .unwrap_or_default()
}

fn opt_s(row: &SqliteRow, col: &str) -> Option<String> {
    row.try_get::<Option<String>, _>(col).ok().flatten()
}

/// `tickets.state` → C4 state.
pub fn status_of(s: &str) -> TicketStatus {
    match s {
        "open" => TicketStatus::Open,
        "settled" => TicketStatus::Settled,
        "cashed_out" => TicketStatus::CashedOut,
        "refunded" => TicketStatus::Refunded,
        _ => TicketStatus::Pending,
    }
}

/// `ticket_legs.status` → C4 leg status.
pub fn leg_status_of(s: &str) -> LegStatus {
    match s {
        "won" => LegStatus::Won,
        "lost" => LegStatus::Lost,
        "half" => LegStatus::Half,
        "void" => LegStatus::Void,
        _ => LegStatus::Open,
    }
}

/// One ticket as C4 shows it, or `None` if the DB has no such ticket.
pub async fn load_ticket(state: &AppState, id: &str) -> Result<Option<Ticket>, sqlx::Error> {
    let Some(row) = sqlx::query("SELECT * FROM tickets WHERE ticket = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
    else {
        return Ok(None);
    };
    let legs = sqlx::query(
        "SELECT l.*, m.away AS away_name, m.home AS home_name
         FROM ticket_legs l LEFT JOIN markets m ON m.symbol = l.symbol
         WHERE l.ticket = ? ORDER BY l.idx",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    // Each fill that served this ticket, sized by this ticket's share of it and its fees
    // pro-rata (a netted order serves several tickets, spec section 6).
    let fills = sqlx::query(
        "SELECT f.venue, f.trade_id, f.symbol, f.side, f.price_u, f.size, f.fee_u,
                f.builder_fee_u, SUM(ABS(a.contracts)) AS mine
         FROM fill_allocations a
         JOIN pascal_fills f ON f.venue = a.venue AND f.trade_id = a.trade_id
         WHERE a.ticket = ?
         GROUP BY f.venue, f.trade_id ORDER BY f.ts_ms, f.trade_id",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    // Kind, line and label of each leg come from the signed quote the ticket was opened
    // with (L2-1 quote store); the chain and `ticket_legs` carry only symbols (D-424).
    let quoted: Vec<QuoteLeg> = match opt_s(&row, "quote_hash") {
        Some(h) => {
            sqlx::query_scalar::<_, String>("SELECT legs_json FROM quotes WHERE quote_hash = ?")
                .bind(h)
                .fetch_optional(&state.db)
                .await?
                .and_then(|j| serde_json::from_str(&j).ok())
                .unwrap_or_default()
        }
        None => Vec::new(),
    };
    let status = status_of(&s(&row, "state"));
    let resolve_leg = resolve_leg_sigs(&legs);
    let legs: Vec<TicketLeg> = legs
        .iter()
        .map(|l| {
            let idx = u(l, "idx");
            // The quote's legs are in ticket order; trust the match only on the same symbol.
            let q = usize::try_from(idx)
                .ok()
                .and_then(|i| quoted.get(i))
                .filter(|q| q.symbol == s(l, "symbol"));
            let side = if s(l, "side") == "short" {
                Side::Short
            } else {
                Side::Long
            };
            let pick = if s(l, "pick") == "home" {
                TeamSide::Home
            } else {
                TeamSide::Away
            };
            TicketLeg {
                idx: u8::try_from(idx).unwrap_or(u8::MAX),
                symbol: s(l, "symbol"),
                game_id: s(l, "game_id"),
                away: s(l, "away_name"),
                home: s(l, "home_name"),
                pick,
                side,
                start_ms: u(l, "start_ms"),
                p_quote_u: u(l, "p_quote_u"),
                p_fill_u: opt_u(l, "p_fill_u"),
                hedged: u(l, "hedged") != 0,
                status: leg_status_of(&s(l, "status")),
                resolution_u: opt_u(l, "resolution_u"),
                kind: q.and_then(|q| q.kind),
                line: q.and_then(|q| q.line.clone()),
                label: q.and_then(|q| q.label.clone()),
            }
        })
        .collect();
    let fills = fills
        .iter()
        .map(|f| {
            let size = u(f, "size");
            let mine = u(f, "mine").min(size);
            let share = |v: i64| -> i64 {
                if size == 0 {
                    return v;
                }
                let scaled = i128::from(v) * i128::from(mine) / i128::from(size);
                i64::try_from(scaled).unwrap_or(v)
            };
            let fee = opt_i(f, "fee_u").unwrap_or(0);
            let builder = opt_i(f, "builder_fee_u").unwrap_or(0);
            TicketFill {
                trade_id: opt_i(f, "trade_id").unwrap_or(0).to_string(),
                symbol: s(f, "symbol"),
                side: if s(f, "side") == "ask" {
                    OrderSide::Ask
                } else {
                    OrderSide::Bid
                },
                price_u: u(f, "price_u"),
                size: mine,
                fee_u: share(fee),
                builder_fee_u: u64::try_from(share(builder)).unwrap_or(0),
                venue: if s(f, "venue") == "pascal" {
                    Venue::Pascal
                } else {
                    Venue::Mock
                },
            }
        })
        .collect();

    let stake_u = opt_u(&row, "rs_u").unwrap_or_else(|| u(&row, "stake_u"));
    let mut t = Ticket {
        ticket: s(&row, "ticket"),
        user: s(&row, "user"),
        nonce: row
            .try_get::<i64, _>("nonce")
            .map(crate::db::nonce_from_db)
            .unwrap_or(0),
        state: status,
        n: u8::try_from(u(&row, "n")).unwrap_or(u8::MAX),
        stake_u,
        payout_u: u(&row, "payout_u"),
        odds_offered_u: opt_u(&row, "odds_u").unwrap_or_else(|| u(&row, "odds_offered_u")),
        odds_fair_u: u(&row, "odds_fair_u"),
        fee_u: u(&row, "fee_u"),
        minted_at: opt_i(&row, "minted_at"),
        settled_at: opt_i(&row, "settled_at"),
        paid_u: opt_u(&row, "paid_u"),
        cash_out: CashOut::unavailable(""),
        legs,
        fills,
        tx: TicketTx {
            open_pending: opt_s(&row, "tx_open_pending"),
            finalize: opt_s(&row, "tx_finalize"),
            settle: opt_s(&row, "tx_settle"),
            refund_pending: opt_s(&row, "tx_refund"),
            cash_out: opt_s(&row, "tx_cash_out"),
            resolve_leg,
            // L7-1 (7.0c): as the indexer applied each transaction (migration 0011).
            slot: TxSlots {
                open_pending: opt_u(&row, "slot_open_pending"),
                cash_out: opt_u(&row, "slot_cash_out"),
                settle: opt_u(&row, "slot_settle"),
            },
            confirm_ms: TxConfirm {
                open_pending: opt_u(&row, "confirm_open_pending_ms"),
                cash_out: opt_u(&row, "confirm_cash_out_ms"),
            },
        },
        tipster: opt_s(&row, "tipster"),
        copied_from: opt_s(&row, "copied_from"),
        // L6-2 (6.0c): the on-chain `Ticket.odds_u` once minted; 0 for a pending or refunded one.
        posted_odds_u: match status {
            TicketStatus::Open | TicketStatus::Settled | TicketStatus::CashedOut => {
                opt_u(&row, "odds_u").unwrap_or(0)
            }
            TicketStatus::Pending | TicketStatus::Refunded => 0,
        },
        title: crate::profiles::title_of(&state.db, id).await?,
        // Set by `GET /tipsters/{wallet}` on its tips (L7-1, [`crate::tipsters`]).
        pool_u: None,
        copies: None,
        teams: None,
    };
    t.cash_out = match status {
        TicketStatus::Open => state.cash_out_for(&ticket_state(&row, &t, &state.db).await?),
        TicketStatus::Pending => CashOut::unavailable("ticket not minted yet"),
        TicketStatus::Settled => CashOut::unavailable("ticket settled"),
        TicketStatus::CashedOut => CashOut::unavailable("ticket cashed out"),
        TicketStatus::Refunded => CashOut::unavailable("pending refunded"),
    };
    Ok(Some(t))
}

/// `Ticket.tx.resolve_leg` (l4-contracts L4-2): the legs' `resolve_leg` signatures in the order
/// the legs resolved (`resolved_ms`, then index), each once; legs resolved without a known
/// signature (found by a resync) are left out.
fn resolve_leg_sigs(legs: &[SqliteRow]) -> Vec<String> {
    let mut sigs: Vec<(i64, i64, String)> = legs
        .iter()
        .filter_map(|l| {
            let sig = opt_s(l, "tx_resolve")?;
            Some((
                opt_i(l, "resolved_ms").unwrap_or(i64::MAX),
                opt_i(l, "idx").unwrap_or(0),
                sig,
            ))
        })
        .collect();
    sigs.sort();
    let mut out: Vec<String> = Vec::with_capacity(sigs.len());
    for (_, _, sig) in sigs {
        if !out.contains(&sig) {
            out.push(sig);
        }
    }
    out
}

/// The [`TicketState`] the quoter values (L2-3), from the ticket's DB rows.
async fn ticket_state(
    row: &SqliteRow,
    t: &Ticket,
    db: &sqlx::SqlitePool,
) -> Result<TicketState, sqlx::Error> {
    let extra: Vec<(i64, i64, Option<i64>)> = sqlx::query_as(
        "SELECT idx, m1_ppm, delta_0 FROM ticket_legs WHERE ticket = ? ORDER BY idx",
    )
    .bind(&t.ticket)
    .fetch_all(db)
    .await?;
    let legs = t
        .legs
        .iter()
        .map(|l| {
            let (m1, delta) = extra
                .iter()
                .find(|(i, _, _)| *i == i64::from(l.idx))
                .map_or((0, 0), |(_, m, d)| (*m, d.unwrap_or(0)));
            LegState {
                idx: l.idx,
                symbol: l.symbol.clone(),
                game_id: l.game_id.clone(),
                side: l.side,
                start_ms: l.start_ms,
                p_quote_u: l.p_quote_u,
                p_fill_u: l.p_fill_u,
                m1_ppm: u32::try_from(m1).unwrap_or(0),
                target: delta,
                status: l.status,
            }
        })
        .collect();
    Ok(TicketState {
        ticket: t.ticket.clone(),
        pending: s(row, "pending"),
        user: t.user.clone(),
        status: t.state,
        stake_u: u(row, "stake_u"),
        rs_u: opt_u(row, "rs_u"),
        payout_u: t.payout_u,
        legs,
    })
}
