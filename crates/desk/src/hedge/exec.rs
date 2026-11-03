//! Execution: send signed orders, book their fills (`pascal_fills`, `fill_allocations` pro-rata
//! over the ticket legs the order served, `held`), and bring a symbol to its net target with an
//! IOC or a post-only roll.

use std::collections::HashMap;

use pascal_rs::{Fill, OrderBook, PageParams, PlacedOrder, TimeInForce};
use pricing::TickSpec;

use super::Hedger;
use super::store::{self, OrderOutcome};
use super::targets;
use super::trader::{self, OrderSpec, SignedOrder, Trader, api_side, signed};
use crate::api::types::{TicketFill, Venue};
use crate::now_ms;
use crate::state::{Allocation, OrderRef, TicketId};

/// How [`Hedger::trade_symbol`] trades.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// One IOC to the target now (unwinds, excess, rebalances on events).
    Ioc,
    /// A roll: post-only one tick inside, then IOC after the roll timeout.
    Roll,
    /// The IOC that finishes a roll.
    RollIoc,
    /// IOC only when off target by more than 10% and 2 contracts (spec section 3 trigger).
    Sweep,
    /// The pre-hedge IOC to the new target (4.C): within the depth and collateral caps.
    Prehedge,
    /// The drift-rebalance IOC (4.C): as [`How::Sweep`], within the collateral cap.
    Drift,
}

/// A fill as the API shows it.
pub fn ticket_fill(f: &Fill, venue: Venue) -> TicketFill {
    TicketFill {
        trade_id: f.trade_id.to_string(),
        symbol: f.symbol.clone(),
        side: api_side(f.side),
        price_u: u64::try_from(f.fill_price).unwrap_or(0),
        size: f.fill_size,
        fee_u: f.fee_usd,
        builder_fee_u: f
            .builder_fee_paid_usd
            .and_then(|b| u64::try_from(b).ok())
            .unwrap_or(0),
        venue,
    }
}

/// Splits a fill of `fill` signed contracts over the remaining `needs` that point the same way,
/// pro-rata to their size (largest remainders get the leftover contracts), never giving a leg
/// more than it still needs. Updates `needs` in place.
pub fn pro_rata(needs: &mut [Allocation], fill: i64) -> Vec<Allocation> {
    let dir = fill.signum();
    if dir == 0 {
        return Vec::new();
    }
    let elig: Vec<usize> = (0..needs.len())
        .filter(|&k| needs[k].contracts.signum() == dir)
        .collect();
    let total: i128 = elig
        .iter()
        .map(|&k| i128::from(needs[k].contracts.unsigned_abs()))
        .sum();
    if total == 0 {
        return Vec::new();
    }
    let take = i128::from(fill.unsigned_abs()).min(total);
    let mut shares: Vec<(usize, i128, i128)> = elig
        .iter()
        .map(|&k| {
            let n = take * i128::from(needs[k].contracts.unsigned_abs());
            (k, n / total, n % total)
        })
        .collect();
    let mut left = take - shares.iter().map(|s| s.1).sum::<i128>();
    let mut order: Vec<usize> = (0..shares.len()).collect();
    order.sort_by(|&a, &b| shares[b].2.cmp(&shares[a].2).then(a.cmp(&b)));
    for i in order {
        if left == 0 {
            break;
        }
        shares[i].1 += 1;
        left -= 1;
    }
    shares
        .into_iter()
        .filter(|s| s.1 > 0)
        .map(|(k, share, _)| {
            let c = i64::try_from(share).unwrap_or(i64::MAX) * dir;
            needs[k].contracts -= c;
            Allocation {
                ticket: needs[k].ticket.clone(),
                leg_idx: needs[k].leg_idx,
                contracts: c,
            }
        })
        .collect()
}

/// The result of one order of a batch.
pub type Sent = (SignedOrder, Result<PlacedOrder, pascal_rs::Error>);

impl Hedger {
    /// The market's tick parameters (catalogue, else NBA's).
    pub(super) fn tick(&self, symbol: &str) -> TickSpec {
        self.state
            .catalogue()
            .and_then(|c| {
                c.market(symbol).map(|m| TickSpec {
                    tick_size_min_u: u64::try_from(m.tick_size_min_u).unwrap_or(1).max(1),
                    tick_sig_figs: m.tick_sig_figs,
                })
            })
            .unwrap_or(TickSpec::NBA)
    }

    /// The current book of `symbol`.
    pub(super) async fn book(&self, symbol: &str) -> Option<OrderBook> {
        match self.state.pascal.books(&[symbol.to_owned()]).await {
            Ok(r) => r.data.books.get(symbol).cloned(),
            Err(e) => {
                trader::count_error(&e);
                tracing::warn!(error = %e, %symbol, "books failed");
                None
            }
        }
    }

    /// Signs, records (write-ahead) and sends a batch of up to 50 orders; returns each order
    /// with its result. A failed batch leaves no order resting, but one that may have reached
    /// the engine (timeout, 5xx) is reconciled from `/fills` (D-024).
    pub(super) async fn send_batch(&self, trader: &Trader, specs: Vec<OrderSpec>) -> Vec<Sent> {
        let mut signed_orders = Vec::with_capacity(specs.len());
        for spec in specs {
            let coid = self.state.next_client_order_id();
            match trader.sign_order(&spec, coid) {
                Ok(s) => signed_orders.push(s),
                Err(e) => tracing::error!(error = %e, symbol = %spec.symbol, "cannot sign order"),
            }
        }
        if signed_orders.is_empty() {
            return Vec::new();
        }
        let now = now_ms();
        let builder = pascal_rs::signing::base58(&trader.builder().pubkey);
        for o in &signed_orders {
            if let Err(e) = store::insert_order(
                &self.state.db,
                o,
                trader.venue(),
                &builder,
                trader.builder().fee_rate_u,
                now,
            )
            .await
            {
                tracing::error!(error = %e, coid = o.client_order_id, "hedge_orders write failed");
            }
            self.state.order_map.write().insert(
                o.client_order_id,
                OrderRef {
                    symbol: o.spec.symbol.clone(),
                    side: api_side(o.spec.side),
                    size: o.spec.size,
                    limit_u: u64::try_from(o.spec.price_u).unwrap_or(0),
                    placed_ms: now,
                    serves: o.spec.serves.clone(),
                },
            );
        }
        let results = trader::place(&self.state.pascal, &signed_orders).await;
        let mut out = Vec::with_capacity(signed_orders.len());
        match results {
            Err(e) => {
                tracing::error!(error = %e, orders = signed_orders.len(), "order batch failed");
                let ambiguous =
                    !matches!(e, pascal_rs::Error::InvalidArgument(_)) && e.api_code().is_none();
                for o in signed_orders {
                    self.record_outcome(
                        o.client_order_id,
                        OrderOutcome {
                            status: "error",
                            error: Some(e.to_string()),
                            ..OrderOutcome::default()
                        },
                    )
                    .await;
                    let r = if ambiguous {
                        self.reconcile_order(trader, &o).await
                    } else {
                        Err(pascal_rs::Error::InvalidArgument(e.to_string()))
                    };
                    out.push((o, r));
                }
            }
            Ok(items) => {
                let mut items = items.into_iter();
                for o in signed_orders {
                    let r = match items.next() {
                        Some(Ok(resp)) => Ok(resp.data),
                        Some(Err(e)) => Err(e),
                        None => Err(pascal_rs::Error::InvalidArgument(
                            "missing result for order".into(),
                        )),
                    };
                    let outcome = match &r {
                        Ok(p) => OrderOutcome {
                            status: "placed",
                            error: None,
                            order_id: Some(p.order.id),
                            size_filled: Some(p.order.size_filled),
                            notional_filled_u: Some(p.order.notional_filled),
                        },
                        Err(e) => {
                            if e.api_code() == Some("MARKET_RESOLVED") {
                                self.mark_resolved(&o.spec.symbol).await;
                            }
                            tracing::warn!(error = %e, coid = o.client_order_id, symbol = %o.spec.symbol, "order rejected");
                            OrderOutcome {
                                status: "rejected",
                                error: Some(e.to_string()),
                                ..OrderOutcome::default()
                            }
                        }
                    };
                    self.record_outcome(o.client_order_id, outcome).await;
                    out.push((o, r));
                }
            }
        }
        out
    }

    async fn record_outcome(&self, coid: u64, out: OrderOutcome) {
        if let Err(e) = store::update_order(&self.state.db, coid, &out, now_ms()).await {
            tracing::error!(error = %e, coid, "hedge_orders update failed");
        }
    }

    /// After an ambiguous write failure: the order's fills from `/fills`, as if it had answered.
    async fn reconcile_order(
        &self,
        trader: &Trader,
        o: &SignedOrder,
    ) -> Result<PlacedOrder, pascal_rs::Error> {
        let fills = self
            .order_fills(trader, &o.spec.symbol, o.client_order_id)
            .await
            .ok_or_else(|| {
                pascal_rs::Error::InvalidArgument("order outcome unknown; reconcile pending".into())
            })?;
        tracing::warn!(
            coid = o.client_order_id,
            fills = fills.len(),
            "ambiguous order reconciled from /fills"
        );
        let filled: u64 = fills.iter().map(|f| f.fill_size).sum();
        let notional: i64 = fills
            .iter()
            .map(|f| {
                f.fill_price
                    .saturating_mul(i64::try_from(f.fill_size).unwrap_or(0))
            })
            .sum();
        Ok(PlacedOrder {
            order: pascal_rs::Order {
                id: fills.first().map_or(0, |f| f.order_id),
                client_order_id: o.client_order_id,
                replace_client_order_id: None,
                symbol: o.spec.symbol.clone(),
                side: o.spec.side,
                price: o.spec.price_u,
                size_original: o.spec.size,
                size_remaining: 0,
                size_filled: filled,
                notional_filled: notional,
                tif: o.spec.tif,
                order_type: pascal_rs::OrderType::Limit,
                post_only: o.spec.post_only,
                reduce_only: o.spec.reduce_only,
                expires_ts_ms: None,
                builder_pubkey: None,
                builder_fee_rate: None,
                seq: 0,
                update_seq: 0,
                place_ts_ms: 0,
                update_ts_ms: 0,
            },
            fills,
            update_reason: None,
        })
    }

    /// One order's fills from `/accounts/{hedger}/fills` (newest page), or `None` when the
    /// endpoint fails.
    pub(super) async fn order_fills(
        &self,
        trader: &Trader,
        symbol: &str,
        coid: u64,
    ) -> Option<Vec<Fill>> {
        match self
            .state
            .pascal
            .fills(
                trader.owner(),
                &[symbol.to_owned()],
                &PageParams::limit(self.cfg.fills_page),
            )
            .await
        {
            Ok(r) => Some(
                r.data
                    .items
                    .into_iter()
                    .filter(|f| f.client_order_id == coid)
                    .collect(),
            ),
            Err(e) => {
                trader::count_error(&e);
                tracing::warn!(error = %e, "fills read failed");
                None
            }
        }
    }

    /// Books an order's fills: each new one goes into `pascal_fills`, moves `held`, and is split
    /// over what the order still needs into `fill_allocations`. Returns every fill.
    pub(super) async fn apply_fills(
        &self,
        coid: u64,
        serves: &[Allocation],
        fills: &[Fill],
    ) -> Vec<TicketFill> {
        let venue = self.trader.as_ref().map_or(Venue::Mock, Trader::venue);
        let db = &self.state.db;
        let permit = store::order_permit(db, coid).await.ok().flatten();
        let mut needs: Vec<Allocation> = serves.to_vec();
        if !needs.is_empty() {
            let done = store::allocated_for_order(db, venue, coid)
                .await
                .unwrap_or_default();
            for n in &mut needs {
                n.contracts -= done
                    .get(&(n.ticket.clone(), n.leg_idx))
                    .copied()
                    .unwrap_or(0);
            }
        }
        let mut out = Vec::with_capacity(fills.len());
        for f in fills {
            let new = match store::insert_fill(db, venue, f, permit.as_deref()).await {
                Ok(n) => n,
                Err(e) => {
                    tracing::error!(error = %e, trade_id = f.trade_id, "pascal_fills write failed");
                    true
                }
            };
            if new {
                let s = signed(f.side, f.fill_size);
                self.add_held(&f.symbol, s);
                let allocs = pro_rata(&mut needs, s);
                if let Err(e) = store::insert_allocations(db, venue, f.trade_id, &allocs).await {
                    tracing::error!(error = %e, trade_id = f.trade_id, "fill_allocations write failed");
                }
                let mut alloc = self.alloc.write();
                for a in &allocs {
                    let e = alloc
                        .entry((a.ticket.clone(), a.leg_idx))
                        .or_insert((f.symbol.clone(), 0));
                    e.1 += a.contracts;
                }
                alloc.retain(|_, (_, c)| *c != 0);
            }
            tracing::info!(
                trade_id = f.trade_id, coid, symbol = %f.symbol, side = ?f.side,
                price = f.fill_price, size = f.fill_size, fee = f.fee_usd,
                builder_fee = f.builder_fee_paid_usd, venue = venue.as_str(), new, "fill"
            );
            out.push(ticket_fill(f, venue));
        }
        out
    }

    /// What each ticket leg on `symbol` still needs: its target minus what is allocated to it.
    fn needs(&self, symbol: &str) -> Vec<Allocation> {
        let rt = self.rtilde_snapshot();
        let pre = self.prehedged_snapshot();
        let legs = {
            let tickets = self.state.tickets.read();
            targets::symbol_target_with(&tickets, symbol, &rt, &pre, self.cfg.params.game_length_ms)
                .1
        };
        let mut by_leg: HashMap<(TicketId, u8), (i64, i64)> = legs
            .into_iter()
            .map(|(t, i, target)| ((t, i), (target, 0)))
            .collect();
        for ((t, i), (s, c)) in self.alloc.read().iter() {
            if s == symbol {
                by_leg.entry((t.clone(), *i)).or_insert((0, 0)).1 = *c;
            }
        }
        let mut v: Vec<Allocation> = by_leg
            .into_iter()
            .filter(|(_, (t, a))| t != a)
            .map(|((ticket, leg_idx), (t, a))| Allocation {
                ticket,
                leg_idx,
                contracts: t - a,
            })
            .collect();
        v.sort_by(|a, b| (&a.ticket, a.leg_idx).cmp(&(&b.ticket, b.leg_idx)));
        v
    }

    /// Brings `symbol` to its net target `Tᵢ` (capped at `Caps::max_target` of the depth): one
    /// IOC for `Tᵢ − held − resting`, reduce-only when it only shrinks the position, or a post-
    /// only roll. Writes `hedge_positions`. Returns the fills.
    pub async fn trade_symbol(
        &self,
        symbol: &str,
        how: How,
        reason: &str,
        pending: Option<&str>,
    ) -> Vec<TicketFill> {
        let Some(trader) = self.trader.clone() else {
            return Vec::new();
        };
        if self.is_resolved(symbol) {
            return Vec::new();
        }
        let lock = self.symbol_lock(symbol);
        let _guard = lock.lock().await;
        let held = self.held(symbol);
        let pos = held + self.resting(symbol);
        let mut target = self.target(symbol);
        if target == pos {
            return Vec::new();
        }
        let params = &self.cfg.params;
        if matches!(how, How::Sweep | How::Drift)
            && !pricing::needs_rebalance(
                pos,
                target,
                params.rebalance_rel_ppm,
                params.rebalance_min_contracts,
            )
        {
            return Vec::new();
        }
        let Some(book) = self.book(symbol).await else {
            return Vec::new();
        };
        if target != 0 && (target.signum() != pos.signum() || target.abs() > pos.abs()) {
            let depth = targets::opening_depth(&book, target, params.depth_window_u);
            let cap = i64::try_from(params.caps.max_target(depth)).unwrap_or(i64::MAX);
            if target.abs() > cap {
                tracing::warn!(%symbol, target, cap, depth, "max T cap: target clamped");
                metrics::counter!("hedge_cap_clamped_total", "cap" => "max_target").increment(1);
                target = target.signum() * cap;
            }
        }
        let diff = target - pos;
        if diff == 0 {
            return Vec::new();
        }
        let side = if diff > 0 {
            pascal_rs::Side::Bid
        } else {
            pascal_rs::Side::Ask
        };
        let size = diff.unsigned_abs();
        let reduce_only = held != 0 && held.signum() != diff.signum() && diff.abs() <= held.abs();
        let serves: Vec<Allocation> = self
            .needs(symbol)
            .into_iter()
            .filter(|a| a.contracts.signum() == diff.signum())
            .collect();
        tracing::info!(%symbol, ?how, reason, target, held, pos, diff, reduce_only, "rebalance");

        if how == How::Roll
            && self.cfg.post_only_rolls
            && !reduce_only
            && let Some(price) = targets::post_only_price(&book, side, self.tick(symbol))
        {
            let spec = OrderSpec {
                symbol: symbol.to_owned(),
                side,
                price_u: price,
                size,
                tif: TimeInForce::Gtc,
                post_only: true,
                reduce_only: false,
                kind: "roll_post",
                reason: reason.to_owned(),
                pending: pending.map(str::to_owned),
                serves: serves.clone(),
            };
            let mut sent = self.send_batch(&trader, vec![spec]).await;
            if let Some((o, Ok(placed))) = sent.pop() {
                let fills = self
                    .apply_fills(o.client_order_id, &o.spec.serves, &placed.fills)
                    .await;
                let open = signed(side, placed.order.size_remaining);
                if open != 0 {
                    *self.resting.write().entry(symbol.to_owned()).or_insert(0) += open;
                    self.spawn_roll_finisher(
                        symbol.to_owned(),
                        o.client_order_id,
                        open,
                        o.spec.serves.clone(),
                        reason.to_owned(),
                        pending.map(str::to_owned),
                    );
                } else {
                    self.state.order_map.write().remove(&o.client_order_id);
                }
                drop(_guard);
                self.publish(symbol, reason).await;
                return fills;
            }
            // Rejected (e.g. WOULD_CROSS after the book moved): fall through to the IOC.
        }

        let Some(limit) = targets::walk_limit(&book, side, size) else {
            tracing::warn!(%symbol, ?side, size, "empty book side: cannot trade");
            drop(_guard);
            self.publish(symbol, reason).await;
            return Vec::new();
        };
        if matches!(how, How::Prehedge | How::Drift)
            && !reduce_only
            && !self.drift_collateral_ok(symbol, side, size, limit).await
        {
            drop(_guard);
            self.publish(symbol, reason).await;
            return Vec::new();
        }
        let kind = match how {
            How::RollIoc | How::Roll => "roll_ioc",
            How::Sweep => "rebalance",
            How::Prehedge => "prehedge",
            How::Drift => "drift",
            How::Ioc if reason == "excess" => "excess",
            How::Ioc if reduce_only => "unwind",
            How::Ioc => "rebalance",
        };
        let spec = OrderSpec {
            symbol: symbol.to_owned(),
            side,
            price_u: limit,
            size,
            tif: TimeInForce::Ioc,
            post_only: false,
            reduce_only,
            kind,
            reason: reason.to_owned(),
            pending: pending.map(str::to_owned),
            serves,
        };
        self.drift_stats.order_sent(kind, symbol);
        let mut fills = Vec::new();
        for (o, r) in self.send_batch(&trader, vec![spec]).await {
            if let Ok(placed) = r {
                fills.extend(
                    self.apply_fills(o.client_order_id, &o.spec.serves, &placed.fills)
                        .await,
                );
            }
            self.state.order_map.write().remove(&o.client_order_id);
        }
        drop(_guard);
        self.publish(symbol, reason).await;
        fills
    }

    fn spawn_roll_finisher(
        &self,
        symbol: String,
        coid: u64,
        open: i64,
        serves: Vec<Allocation>,
        reason: String,
        pending: Option<String>,
    ) {
        let h = self.clone();
        let timeout = self.cfg.roll_timeout;
        tokio::spawn(async move {
            tokio::select! {
                _ = h.state.shutdown_requested() => return,
                _ = tokio::time::sleep(timeout) => {}
            }
            h.finish_roll(&symbol, coid, open, &serves, &reason, pending.as_deref())
                .await;
        });
    }

    /// The roll timed out: cancel the post-only order, book what it filled, IOC the rest.
    async fn finish_roll(
        &self,
        symbol: &str,
        coid: u64,
        open: i64,
        serves: &[Allocation],
        reason: &str,
        pending: Option<&str>,
    ) {
        let Some(trader) = self.trader.clone() else {
            return;
        };
        {
            let lock = self.symbol_lock(symbol);
            let _guard = lock.lock().await;
            let cancelled = trader::cancel(&self.state.pascal, &trader, coid).await;
            let fills = self.order_fills(&trader, symbol, coid).await;
            match (&fills, &cancelled) {
                (Some(f), _) => {
                    self.apply_fills(coid, serves, f).await;
                }
                (None, Ok(o)) if o.size_filled > 0 => {
                    // No /fills: move `held` by what the order says; reconcile corrects it.
                    self.add_held(symbol, signed(o.side, o.size_filled));
                }
                _ => {}
            }
            let outcome = match &cancelled {
                Ok(o) => OrderOutcome {
                    status: "cancelled",
                    error: None,
                    order_id: Some(o.id),
                    size_filled: Some(o.size_filled),
                    notional_filled_u: Some(o.notional_filled),
                },
                Err(e) => OrderOutcome {
                    status: "cancelled",
                    error: Some(e.to_string()),
                    ..OrderOutcome::default()
                },
            };
            self.record_outcome(coid, outcome).await;
            {
                let mut r = self.resting.write();
                if let Some(v) = r.get_mut(symbol) {
                    *v -= open;
                    if *v == 0 {
                        r.remove(symbol);
                    }
                }
            }
            self.state.order_map.write().remove(&coid);
            tracing::info!(%symbol, coid, "roll timed out: post-only cancelled, IOC for the rest");
        }
        self.trade_symbol(symbol, How::RollIoc, reason, pending)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn need(t: &str, i: u8, c: i64) -> Allocation {
        Allocation {
            ticket: t.into(),
            leg_idx: i,
            contracts: c,
        }
    }

    #[test]
    fn pro_rata_splits_and_caps() {
        let mut n = vec![need("a", 0, 10), need("b", 0, 5), need("c", 0, -3)];
        let a = pro_rata(&mut n, 9);
        let total: i64 = a.iter().map(|x| x.contracts).sum();
        assert_eq!(total, 9);
        assert_eq!(a[0].contracts, 6);
        assert_eq!(a[1].contracts, 3);
        // Remaining needs shrink; an over-fill is capped at what is still needed.
        let b = pro_rata(&mut n, 100);
        assert_eq!(b.iter().map(|x| x.contracts).sum::<i64>(), 6);
        assert!(n.iter().take(2).all(|x| x.contracts == 0));
        // The opposite direction only goes to legs that need it.
        let c = pro_rata(&mut n, -2);
        assert_eq!(c, vec![need("c", 0, -2)]);
        assert!(pro_rata(&mut [], 5).is_empty());
    }
}
