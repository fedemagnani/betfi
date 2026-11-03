//! The hedge desk's database rows: `hedge_orders` / `hedge_order_serves` (0004, written before
//! an order is sent), `pascal_fills` and `fill_allocations` (0001, idempotent on
//! `(venue, trade_id)`), `hedge_positions` (target vs held), the `/fills` replay cursor, and the
//! ticket rows (written by 2.C) the desk rebuilds its state from at start.

use std::collections::HashMap;

use pascal_rs::{Fill, Liquidity};
use sqlx::{Row, SqlitePool};

use super::targets::status_rtilde;
use super::trader::{SignedOrder, side_str};
use crate::api::types::{LegStatus, Side, TicketStatus, Venue};
use crate::state::{Allocation, LegState, Symbol, TicketId, TicketState};

/// `cursors.name` of the last `/accounts/{hedger}/fills` seq stored.
pub const FILLS_CURSOR: &str = "hedge.fills_seq";

fn i(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn u(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

/// Writes a signed order and what it serves, before it is sent.
pub async fn insert_order(
    db: &SqlitePool,
    o: &SignedOrder,
    venue: Venue,
    builder_pubkey: &str,
    builder_fee_rate_u: u32,
    now_ms: u64,
) -> Result<(), sqlx::Error> {
    let s = &o.spec;
    let mut tx = db.begin().await?;
    sqlx::query(
        "INSERT INTO hedge_orders (client_order_id, venue, kind, reason, pending, symbol, side,
            price_u, size, tif, post_only, reduce_only, builder_pubkey, builder_fee_rate_u,
            request_json, permit_hex, signature, status, placed_ms, updated_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'signed', ?, ?)
         ON CONFLICT (client_order_id) DO NOTHING",
    )
    .bind(i(o.client_order_id))
    .bind(venue.as_str())
    .bind(s.kind)
    .bind(&s.reason)
    .bind(&s.pending)
    .bind(&s.symbol)
    .bind(side_str(s.side))
    .bind(s.price_u)
    .bind(i(s.size))
    .bind(format!("{:?}", s.tif).to_ascii_lowercase())
    .bind(s.post_only)
    .bind(s.reduce_only)
    .bind(builder_pubkey)
    .bind(i64::from(builder_fee_rate_u))
    .bind(&o.request_json)
    .bind(o.signed.permit_hex())
    .bind(o.signed.signature_base58())
    .bind(i(now_ms))
    .bind(i(now_ms))
    .execute(&mut *tx)
    .await?;
    for a in &s.serves {
        sqlx::query(
            "INSERT INTO hedge_order_serves (client_order_id, ticket, leg_idx, contracts)
             VALUES (?, ?, ?, ?)
             ON CONFLICT (client_order_id, ticket, leg_idx) DO UPDATE SET contracts = excluded.contracts",
        )
        .bind(i(o.client_order_id))
        .bind(&a.ticket)
        .bind(i64::from(a.leg_idx))
        .bind(a.contracts)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

/// The outcome of a sent order.
#[derive(Debug, Clone, Default)]
pub struct OrderOutcome {
    /// `placed`, `rejected`, `error` or `cancelled`.
    pub status: &'static str,
    /// Error text.
    pub error: Option<String>,
    /// Exchange id.
    pub order_id: Option<u64>,
    /// Contracts filled.
    pub size_filled: Option<u64>,
    /// Notional filled, micro-USD.
    pub notional_filled_u: Option<i64>,
}

/// Records how an order ended.
pub async fn update_order(
    db: &SqlitePool,
    client_order_id: u64,
    out: &OrderOutcome,
    now_ms: u64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE hedge_orders SET status = ?, error = COALESCE(?, error),
            order_id = COALESCE(?, order_id), size_filled = COALESCE(?, size_filled),
            notional_filled_u = COALESCE(?, notional_filled_u), updated_ms = ?
         WHERE client_order_id = ?",
    )
    .bind(out.status)
    .bind(&out.error)
    .bind(out.order_id.map(i))
    .bind(out.size_filled.map(i))
    .bind(out.notional_filled_u)
    .bind(i(now_ms))
    .bind(i(client_order_id))
    .execute(db)
    .await?;
    Ok(())
}

/// The signed permit (hex) of one of our orders.
pub async fn order_permit(
    db: &SqlitePool,
    client_order_id: u64,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT permit_hex FROM hedge_orders WHERE client_order_id = ?")
        .bind(i(client_order_id))
        .fetch_optional(db)
        .await
}

/// What an order served (the persisted `order_map` entry).
pub async fn order_serves(
    db: &SqlitePool,
    client_order_id: u64,
) -> Result<Vec<Allocation>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT ticket, leg_idx, contracts FROM hedge_order_serves WHERE client_order_id = ?
         ORDER BY ticket, leg_idx",
    )
    .bind(i(client_order_id))
    .fetch_all(db)
    .await?;
    Ok(rows
        .iter()
        .map(|r| Allocation {
            ticket: r.get("ticket"),
            leg_idx: u8::try_from(r.get::<i64, _>("leg_idx")).unwrap_or(0),
            contracts: r.get("contracts"),
        })
        .collect())
}

/// Contracts already allocated per ticket leg out of one order's fills.
pub async fn allocated_for_order(
    db: &SqlitePool,
    venue: Venue,
    client_order_id: u64,
) -> Result<HashMap<(TicketId, u8), i64>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT a.ticket, a.leg_idx, SUM(a.contracts) AS c
         FROM fill_allocations a JOIN pascal_fills f
           ON f.venue = a.venue AND f.trade_id = a.trade_id
         WHERE f.venue = ? AND f.client_order_id = ?
         GROUP BY a.ticket, a.leg_idx",
    )
    .bind(venue.as_str())
    .bind(i(client_order_id))
    .fetch_all(db)
    .await?;
    Ok(rows
        .iter()
        .map(|r| {
            (
                (
                    r.get::<String, _>("ticket"),
                    u8::try_from(r.get::<i64, _>("leg_idx")).unwrap_or(0),
                ),
                r.get::<i64, _>("c"),
            )
        })
        .collect())
}

/// Inserts one fill; `true` when it is new (the caller then moves `held` and allocates it).
pub async fn insert_fill(
    db: &SqlitePool,
    venue: Venue,
    f: &Fill,
    signed_payload: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let raw = serde_json::to_string(f).unwrap_or_default();
    let r = sqlx::query(
        "INSERT INTO pascal_fills (venue, trade_id, symbol, side, price_u, size, liquidity, fee_u,
            builder_fee_u, collateral_change_u, realized_pnl_u, position_size_prev, order_id,
            client_order_id, reduce_only, seq, ts_ms, signed_payload, raw)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (venue, trade_id) DO NOTHING",
    )
    .bind(venue.as_str())
    .bind(i(f.trade_id))
    .bind(&f.symbol)
    .bind(side_str(f.side))
    .bind(f.fill_price)
    .bind(i(f.fill_size))
    .bind(match f.liquidity {
        Liquidity::Maker => "maker",
        Liquidity::Taker => "taker",
    })
    .bind(f.fee_usd)
    .bind(f.builder_fee_paid_usd.unwrap_or(0))
    .bind(f.collateral_change_usd)
    .bind(f.realized_pnl_usd)
    .bind(f.position_size_prev)
    .bind(i(f.order_id))
    .bind(i(f.client_order_id))
    .bind(f.reduce_only)
    .bind(i(f.seq))
    .bind(i(f.trade_ts_ms))
    .bind(signed_payload)
    .bind(raw)
    .execute(db)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// Writes one fill's split over ticket legs.
pub async fn insert_allocations(
    db: &SqlitePool,
    venue: Venue,
    trade_id: u64,
    allocs: &[Allocation],
) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    for a in allocs.iter().filter(|a| a.contracts != 0) {
        sqlx::query(
            "INSERT INTO fill_allocations (venue, trade_id, ticket, leg_idx, contracts)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (venue, trade_id, ticket, leg_idx) DO NOTHING",
        )
        .bind(venue.as_str())
        .bind(i(trade_id))
        .bind(&a.ticket)
        .bind(i64::from(a.leg_idx))
        .bind(a.contracts)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

/// Net allocated contracts per ticket leg, with the leg's symbol, over all fills of `venue`.
pub async fn load_allocations(
    db: &SqlitePool,
    venue: Venue,
) -> Result<HashMap<(TicketId, u8), (Symbol, i64)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT a.ticket, a.leg_idx, f.symbol, SUM(a.contracts) AS c
         FROM fill_allocations a JOIN pascal_fills f
           ON f.venue = a.venue AND f.trade_id = a.trade_id
         WHERE a.venue = ?
         GROUP BY a.ticket, a.leg_idx, f.symbol",
    )
    .bind(venue.as_str())
    .fetch_all(db)
    .await?;
    let mut out: HashMap<(TicketId, u8), (Symbol, i64)> = HashMap::new();
    for r in rows {
        let key = (
            r.get::<String, _>("ticket"),
            u8::try_from(r.get::<i64, _>("leg_idx")).unwrap_or(0),
        );
        let c: i64 = r.get("c");
        out.entry(key)
            .and_modify(|e| e.1 += c)
            .or_insert((r.get("symbol"), c));
    }
    out.retain(|_, (_, c)| *c != 0);
    Ok(out)
}

/// Whether the desk already sent the inception hedge of `pending` (idempotency across restarts
/// and redelivered events).
pub async fn inception_sent(db: &SqlitePool, pending: &str) -> Result<bool, sqlx::Error> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hedge_orders WHERE pending = ? AND kind = 'inception'",
    )
    .bind(pending)
    .fetch_one(db)
    .await?;
    Ok(n > 0)
}

/// One `hedge_positions` row (`INSERT OR REPLACE`: two rows in one ms keep the later one).
pub async fn insert_position(
    db: &SqlitePool,
    symbol: &str,
    ts_ms: u64,
    target: i64,
    held: i64,
    reason: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR REPLACE INTO hedge_positions (symbol, ts_ms, target, held, reason)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(symbol)
    .bind(i(ts_ms))
    .bind(target)
    .bind(held)
    .bind(reason)
    .execute(db)
    .await?;
    Ok(())
}

/// Symbols the `markets` table knows as resolved.
pub async fn resolved_symbols(db: &SqlitePool) -> Result<Vec<Symbol>, sqlx::Error> {
    sqlx::query_scalar("SELECT symbol FROM markets WHERE resolved_price_u IS NOT NULL")
        .fetch_all(db)
        .await
}

/// The hedger's position per symbol as the newest stored fill left it
/// (`position_size_prev + signed size`), for symbols not known as resolved.
pub async fn held_from_fills(
    db: &SqlitePool,
    venue: Venue,
) -> Result<HashMap<Symbol, i64>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT f.symbol, f.side, f.size, f.position_size_prev
         FROM pascal_fills f
         JOIN (SELECT symbol, MAX(seq) AS seq FROM pascal_fills WHERE venue = ? GROUP BY symbol) m
           ON m.symbol = f.symbol AND m.seq = f.seq
         WHERE f.venue = ?
           AND f.symbol NOT IN (SELECT symbol FROM markets WHERE resolved_price_u IS NOT NULL)",
    )
    .bind(venue.as_str())
    .bind(venue.as_str())
    .fetch_all(db)
    .await?;
    let mut out = HashMap::new();
    for r in rows {
        let size: i64 = r.get("size");
        let prev: Option<i64> = r.get("position_size_prev");
        let s = if r.get::<String, _>("side") == "bid" {
            size
        } else {
            -size
        };
        out.insert(r.get::<String, _>("symbol"), prev.unwrap_or(0) + s);
    }
    Ok(out)
}

/// A ticket rebuilt from the `tickets` / `ticket_legs` rows (pending and open only), with the
/// `r̃` of each resolved leg.
#[derive(Debug, Clone)]
pub struct StoredTicket {
    /// The ticket (leg targets 0; the caller sets them).
    pub ticket: TicketState,
    /// `r̃` per resolved leg index.
    pub rtilde: Vec<(u8, u64)>,
}

/// Loads every pending or open ticket (or just `only`).
pub async fn load_tickets(
    db: &SqlitePool,
    only: Option<&str>,
) -> Result<Vec<StoredTicket>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT ticket, pending, user, state, stake_u, rs_u, payout_u FROM tickets
         WHERE state IN ('pending', 'open') AND (? IS NULL OR ticket = ?)",
    )
    .bind(only)
    .bind(only)
    .fetch_all(db)
    .await?;
    let mut out = Vec::new();
    for r in rows {
        let id: String = r.get("ticket");
        let legs = sqlx::query(
            "SELECT idx, symbol, game_id, side, start_ms, p_quote_u, p_fill_u, m1_ppm, status,
                    resolution_u
             FROM ticket_legs WHERE ticket = ? ORDER BY idx",
        )
        .bind(&id)
        .fetch_all(db)
        .await?;
        let mut rtilde = Vec::new();
        let legs: Vec<LegState> = legs
            .iter()
            .map(|l| {
                let side = if l.get::<String, _>("side") == "short" {
                    Side::Short
                } else {
                    Side::Long
                };
                let status = match l.get::<String, _>("status").as_str() {
                    "won" => LegStatus::Won,
                    "lost" => LegStatus::Lost,
                    "half" => LegStatus::Half,
                    "void" => LegStatus::Void,
                    _ => LegStatus::Open,
                };
                let idx = u8::try_from(l.get::<i64, _>("idx")).unwrap_or(0);
                if status != LegStatus::Open {
                    let r = l
                        .get::<Option<i64>, _>("resolution_u")
                        .map(|r| pricing::r_tilde(super::targets::pside(side), u(r)))
                        .or(status_rtilde(status))
                        .unwrap_or(0);
                    rtilde.push((idx, r));
                }
                LegState {
                    idx,
                    symbol: l.get("symbol"),
                    game_id: l.get("game_id"),
                    side,
                    start_ms: u(l.get("start_ms")),
                    p_quote_u: u(l.get("p_quote_u")),
                    p_fill_u: l
                        .get::<Option<i64>, _>("p_fill_u")
                        .map(u)
                        .filter(|&p| p > 0),
                    m1_ppm: u32::try_from(l.get::<i64, _>("m1_ppm")).unwrap_or(0),
                    target: 0,
                    status,
                }
            })
            .collect();
        let status = if r.get::<String, _>("state") == "open" {
            TicketStatus::Open
        } else {
            TicketStatus::Pending
        };
        out.push(StoredTicket {
            ticket: TicketState {
                ticket: id,
                pending: r.get("pending"),
                user: r.get("user"),
                status,
                stake_u: u(r.get("stake_u")),
                rs_u: r.get::<Option<i64>, _>("rs_u").map(u),
                payout_u: u(r.get("payout_u")),
                legs,
            },
            rtilde,
        });
    }
    Ok(out)
}
