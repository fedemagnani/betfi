//! The panic button (spec section 3 "Desk panic", section 12 runbook): one `mass-cancel` per
//! market in batches of 10, then a reduce-only IOC closing every position at the price that
//! walks the closing side of the book for its size. Run by the `desk-flatten` binary (D-073);
//! stop the desk first, or it re-hedges its tickets on the next event.

use std::collections::BTreeSet;

use pascal_rs::{AccountQuery, TimeInForce};
use serde::Serialize;
use sqlx::SqlitePool;

use super::exec::ticket_fill;
use super::store;
use super::targets::walk_limit;
use super::trader::{self, OrderSpec, Trader, signed};
use crate::api::types::TicketFill;
use crate::now_ms;
use crate::state::Pascal;

/// What a flatten did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FlattenReport {
    /// Markets mass-cancelled.
    pub markets_cancelled: Vec<String>,
    /// Orders the mass cancels removed.
    pub orders_cancelled: usize,
    /// Positions found, `(symbol, signed contracts)`.
    pub positions: Vec<(String, i64)>,
    /// Closing fills.
    pub fills: Vec<TicketFill>,
    /// Positions left after the closes (empty book side, rejections), from the fills.
    pub left: Vec<(String, i64)>,
    /// Errors met on the way.
    pub errors: Vec<String>,
}

/// Flattens the hedger account: cancels every resting order, closes every position
/// reduce-only. Each close is signed with the builder code and recorded in `hedge_orders` /
/// `pascal_fills` when `db` is given.
pub async fn flatten(
    pascal: &Pascal,
    trader: &Trader,
    db: Option<&SqlitePool>,
) -> Result<FlattenReport, pascal_rs::Error> {
    let mut report = FlattenReport::default();
    let acct = pascal
        .account(
            trader.owner(),
            &AccountQuery {
                symbol: None,
                exclude_orders: false,
            },
        )
        .await?
        .data;
    let mut markets: BTreeSet<String> = acct.orders.iter().map(|o| o.symbol.clone()).collect();
    report.positions = acct
        .positions
        .iter()
        .filter(|p| p.size != 0)
        .map(|p| (p.symbol.clone(), p.size))
        .collect();
    markets.extend(report.positions.iter().map(|(s, _)| s.clone()));
    let markets: Vec<String> = markets.into_iter().collect();
    for chunk in markets.chunks(10) {
        match trader::mass_cancel(pascal, trader, chunk).await {
            Ok(items) => {
                for (s, r) in chunk.iter().zip(items) {
                    match r {
                        Ok(m) => {
                            report.orders_cancelled += m.data.orders.len();
                            report.markets_cancelled.push(s.clone());
                        }
                        Err(e) => report.errors.push(format!("mass-cancel {s}: {e}")),
                    }
                }
            }
            Err(e) => report.errors.push(format!("mass-cancel batch: {e}")),
        }
    }
    let symbols: Vec<String> = report.positions.iter().map(|(s, _)| s.clone()).collect();
    let mut specs = Vec::new();
    for chunk in symbols.chunks(50) {
        let books = match pascal.books(chunk).await {
            Ok(b) => b.data.books,
            Err(e) => {
                report.errors.push(format!("books: {e}"));
                continue;
            }
        };
        for (s, size) in report.positions.iter().filter(|(s, _)| chunk.contains(s)) {
            let side = if *size > 0 {
                pascal_rs::Side::Ask
            } else {
                pascal_rs::Side::Bid
            };
            let n = size.unsigned_abs();
            match books.get(s).and_then(|b| walk_limit(b, side, n)) {
                Some(price_u) => specs.push(OrderSpec {
                    symbol: s.clone(),
                    side,
                    price_u,
                    size: n,
                    tif: TimeInForce::Ioc,
                    post_only: false,
                    reduce_only: true,
                    kind: "flatten",
                    reason: "flatten".into(),
                    pending: None,
                    serves: Vec::new(),
                }),
                None => report.errors.push(format!("{s}: empty closing side")),
            }
        }
    }
    let mut left: std::collections::BTreeMap<String, i64> =
        report.positions.iter().cloned().collect();
    let builder = pascal_rs::signing::base58(&trader.builder().pubkey);
    // Counter from the clock: the desk's AppState counter is not available here.
    let base = now_ms().saturating_mul(1000).saturating_add(500);
    for (k, chunk) in specs.chunks(50).enumerate() {
        let mut signed_orders = Vec::new();
        for (j, spec) in chunk.iter().enumerate() {
            let coid = base + (k * 50 + j) as u64;
            match trader.sign_order(spec, coid) {
                Ok(o) => signed_orders.push(o),
                Err(e) => report.errors.push(format!("sign {}: {e}", spec.symbol)),
            }
        }
        if let Some(db) = db {
            for o in &signed_orders {
                if let Err(e) = store::insert_order(
                    db,
                    o,
                    trader.venue(),
                    &builder,
                    trader.builder().fee_rate_u,
                    now_ms(),
                )
                .await
                {
                    report.errors.push(format!("hedge_orders: {e}"));
                }
            }
        }
        if signed_orders.is_empty() {
            continue;
        }
        match trader::place(pascal, &signed_orders).await {
            Ok(items) => {
                for (o, r) in signed_orders.iter().zip(items) {
                    match r {
                        Ok(placed) => {
                            for f in &placed.data.fills {
                                if let Some(db) = db
                                    && let Err(e) = store::insert_fill(
                                        db,
                                        trader.venue(),
                                        f,
                                        Some(&o.signed.permit_hex()),
                                    )
                                    .await
                                {
                                    report.errors.push(format!("pascal_fills: {e}"));
                                }
                                *left.entry(f.symbol.clone()).or_insert(0) +=
                                    signed(f.side, f.fill_size);
                                report.fills.push(ticket_fill(f, trader.venue()));
                            }
                        }
                        Err(e) => report.errors.push(format!("close {}: {e}", o.spec.symbol)),
                    }
                }
            }
            Err(e) => report.errors.push(format!("close batch: {e}")),
        }
    }
    report.left = left.into_iter().filter(|(_, v)| *v != 0).collect();
    tracing::warn!(
        markets = report.markets_cancelled.len(),
        orders = report.orders_cancelled,
        fills = report.fills.len(),
        left = ?report.left,
        errors = ?report.errors,
        "flatten done"
    );
    Ok(report)
}
