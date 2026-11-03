//! `PendingOpened` → the inception hedge (spec section 2 steps 4–5, section 5 flow 1).
//!
//! 1. Re-quote from fresh books ([`pricing::quote`] on the pending's legs and stake, at the
//!    books' `state_time_ms`). If the fresh prices no longer satisfy the user's floor (the C2
//!    payout with the **signed** `m₁` and, for legs the roll leaves unhedged, the **signed**
//!    `pᵢ` the program will use), do nothing: the pending expires and the crank refunds it.
//! 2. Per-leg max prices from the fresh prices and the user's floor ([`pricing::p_max`]).
//! 3. Caps: the hedger's free collateral covers the batch, and each symbol's net target stays
//!    within `Caps::max_target` of its depth.
//! 4. One batch of IOC limits on the inception hedged set (next leg to resolve plus every leg
//!    overlapping it), long = `BID` at `pᵢᵐᵃˣ`, short = `ASK` at `1 − pᵢᵐᵃˣ`, `Δᵢ` contracts,
//!    builder code on each.
//! 5. [`pricing::finalize`]: `fᵢ` (average fill, rounded up), `r`; unwind the excess over
//!    `⌈r·Δᵢ⌉`; emit `HedgeFilled`. With `r = 0` (or a floor the fills miss) unwind everything,
//!    emit `HedgeUnwound { pending: Some }` when anything had filled, then `RefundNeeded`.

use std::collections::BTreeMap;
use std::sync::Arc;

use pascal_rs::{AccountQuery, MarketSearch, OrderBook, TimeInForce};
use pricing::{LegFill, LegInput, PMaxLeg, PayoutLeg};

use super::Hedger;
use super::exec::How;
use super::targets::{self, pside, sigma};
use super::trader::{OrderSpec, order_for};
use crate::api::types::{FeedKind, LegStatus, Side, TicketFill, TicketStatus};
use crate::state::{
    Allocation, DeskEvent, FeedItem, HedgeFilled, LegState, PendingOpened, Symbol, TicketState,
};

/// One leg of the inception plan.
#[derive(Debug, Clone)]
pub struct PlanLeg {
    /// Index in the ticket.
    pub idx: u8,
    /// Symbol.
    pub symbol: Symbol,
    /// The user's side.
    pub side: Side,
    /// In the inception hedged set.
    pub hedged: bool,
    /// `Δᵢ` of the fresh quote, contracts.
    pub size: u64,
    /// `pᵢ` of the fresh quote (user's side).
    pub fresh_price_u: u64,
    /// `pᵢᵐᵃˣ`, user's side, on tick.
    pub p_max_u: u64,
    /// Taker fee rate, ppm.
    pub taker_fee_rate_ppm: u32,
    /// The book the quote was taken on.
    pub book: OrderBook,
}

/// The inception plan of a pending.
#[derive(Debug, Clone)]
pub struct Plan {
    /// Legs in ticket order.
    pub legs: Vec<PlanLeg>,
    /// Tape/engine time of the books, Unix ms.
    pub as_of_ms: u64,
}

impl Hedger {
    pub(super) async fn on_pending(&self, p: &PendingOpened) {
        let result = self.hedge_pending(p).await;
        metrics::counter!("hedge_inception_total", "result" => result).increment(1);
    }

    /// Re-quotes `p` on fresh books and returns the plan, or why nothing will be hedged.
    pub async fn requote(&self, p: &PendingOpened) -> Result<Plan, String> {
        if p.legs.is_empty() {
            return Err("no legs".into());
        }
        let symbols: Vec<String> = p.legs.iter().map(|l| l.symbol.clone()).collect();
        let markets = self
            .state
            .pascal
            .search_markets(&MarketSearch {
                symbols: symbols.clone(),
                ..MarketSearch::default()
            })
            .await
            .map_err(|e| format!("markets: {e}"))?
            .data;
        let books = self
            .state
            .pascal
            .books(&symbols)
            .await
            .map_err(|e| format!("books: {e}"))?;
        let as_of_ms = books.state_time_ms.unwrap_or_else(crate::now_ms);
        let books: BTreeMap<String, OrderBook> = books.data.books;
        let mut inputs = Vec::with_capacity(p.legs.len());
        for leg in &p.legs {
            let m = markets
                .iter()
                .find(|m| m.symbol == leg.symbol)
                .ok_or_else(|| format!("{} not found", leg.symbol))?;
            inputs.push(LegInput::from_market(m, pside(leg.side)).map_err(|r| r.reason)?);
        }
        let params = &self.cfg.params;
        let q = pricing::quote(&inputs, &books, p.stake_u, as_of_ms, params)
            .map_err(|r| format!("fresh quote refused: {}", r.reason))?;
        // The program pays with the signed m1 and, on unhedged legs, the signed price.
        let worst: Vec<u64> = p
            .legs
            .iter()
            .zip(&q.legs)
            .map(|(signed, fresh)| {
                if fresh.hedged {
                    fresh.price_u
                } else {
                    signed.price_u
                }
            })
            .collect();
        let pay_legs: Vec<PayoutLeg> = p
            .legs
            .iter()
            .zip(&worst)
            .map(|(l, &f_u)| PayoutLeg {
                m1_ppm: l.m1_ppm,
                f_u,
            })
            .collect();
        let pay = pricing::payout(p.stake_u, 1_000_000, &pay_legs).map_err(|e| e.to_string())?;
        if !pricing::floor_ok(pay.payout_u, pay.rs_u, p.min_payout_per_unit_u) {
            return Err(format!(
                "fresh quote misses the floor: odds {} < {}",
                pay.odds_u, p.min_payout_per_unit_u
            ));
        }
        let pm_legs: Vec<PMaxLeg> = p
            .legs
            .iter()
            .zip(&worst)
            .zip(&inputs)
            .map(|((l, &p_u), input)| PMaxLeg {
                p_u,
                m1_ppm: l.m1_ppm,
                tick: input.tick,
            })
            .collect();
        let pm = pricing::p_max(
            &pm_legs,
            params.slippage_bps,
            p.min_payout_per_unit_u,
            p.stake_u,
            pricing::FLOOR_MIN_RS_U,
        )
        .map_err(|e| format!("p_max: {e}"))?;
        let legs = p
            .legs
            .iter()
            .enumerate()
            .map(|(i, l)| PlanLeg {
                idx: u8::try_from(i).unwrap_or(u8::MAX),
                symbol: l.symbol.clone(),
                side: l.side,
                hedged: q.legs[i].hedged,
                size: q.legs[i].size,
                fresh_price_u: q.legs[i].price_u,
                p_max_u: pm[i],
                taker_fee_rate_ppm: inputs[i].taker_fee_rate_ppm,
                book: books.get(&l.symbol).cloned().unwrap_or(OrderBook {
                    asks: Vec::new(),
                    bids: Vec::new(),
                    spec: None,
                }),
            })
            .collect();
        Ok(Plan { legs, as_of_ms })
    }

    /// The section 3 caps for a plan: the hedger's free collateral covers the batch (cost +
    /// taker and builder fees at the max prices), and no symbol's net target exceeds
    /// `Caps::max_target` of its depth.
    pub async fn check_caps(&self, plan: &Plan) -> Result<(), String> {
        let params = &self.cfg.params;
        for l in plan.legs.iter().filter(|l| l.hedged) {
            let t_new = self.target(&l.symbol) + sigma(l.side) * l.size as i64;
            let depth = targets::opening_depth(&l.book, t_new, params.depth_window_u);
            let cap = params.caps.max_target(depth);
            if t_new.unsigned_abs() > cap {
                metrics::counter!("hedge_cap_clamped_total", "cap" => "max_target").increment(1);
                return Err(format!(
                    "max T cap on {}: |{t_new}| > {cap} (depth {depth})",
                    l.symbol
                ));
            }
        }
        let cost: u64 = plan
            .legs
            .iter()
            .filter(|l| l.hedged)
            .map(|l| {
                l.size.saturating_mul(l.p_max_u)
                    + pricing::taker_fee_u(l.size, l.p_max_u, l.taker_fee_rate_ppm)
                    + pricing::taker_fee_u(l.size, l.p_max_u, self.cfg.builder_fee_rate_u)
            })
            .sum();
        let free = match &self.trader {
            Some(t) => match self
                .state
                .pascal
                .account(
                    t.owner(),
                    &AccountQuery {
                        symbol: None,
                        exclude_orders: true,
                    },
                )
                .await
            {
                Ok(r) => {
                    if let Some(c) = r.data.collateral_usd {
                        self.set_free_collateral(c);
                    }
                    r.data.collateral_usd
                }
                Err(e) => {
                    super::trader::count_error(&e);
                    self.free_collateral()
                }
            },
            None => None,
        };
        match free {
            Some(f)
                if !params
                    .caps
                    .hedger_covers(u64::try_from(f).unwrap_or(0), cost) =>
            {
                metrics::counter!("hedge_cap_clamped_total", "cap" => "hedger_collateral")
                    .increment(1);
                Err(format!(
                    "hedger free collateral {f} < next hedge cost {cost}"
                ))
            }
            Some(_) => Ok(()),
            None => {
                tracing::warn!("hedger free collateral unknown; hedging anyway");
                Ok(())
            }
        }
    }

    async fn hedge_pending(&self, p: &PendingOpened) -> &'static str {
        let Some(trader) = self.trader.clone() else {
            tracing::warn!(pending = %p.pending, "no hedger keys: the pending will expire");
            return "disabled";
        };
        if self.state.is_paused() {
            tracing::warn!(pending = %p.pending, "desk paused: not hedging");
            return "paused";
        }
        match super::store::inception_sent(&self.state.db, &p.pending).await {
            Ok(true) => {
                tracing::info!(pending = %p.pending, "inception already sent; ignoring");
                return "duplicate";
            }
            Ok(false) => {}
            Err(e) => {
                tracing::error!(error = %e, "db read failed: not hedging");
                return "db_error";
            }
        }
        let plan = match self.requote(p).await {
            Ok(plan) => plan,
            Err(why) => {
                tracing::info!(pending = %p.pending, %why, "no hedge: the pending will expire");
                return "no_hedge";
            }
        };
        if let Err(why) = self.check_caps(&plan).await {
            tracing::warn!(pending = %p.pending, %why, "cap: no hedge, the pending will expire");
            return "cap";
        }

        // The pending ticket, wanting nothing yet.
        let ticket = TicketState {
            ticket: p.ticket.clone(),
            pending: p.pending.clone(),
            user: p.user.clone(),
            status: TicketStatus::Pending,
            stake_u: p.stake_u,
            rs_u: None,
            payout_u: 0,
            legs: p
                .legs
                .iter()
                .enumerate()
                .map(|(i, l)| LegState {
                    idx: u8::try_from(i).unwrap_or(u8::MAX),
                    symbol: l.symbol.clone(),
                    game_id: l.game_id.clone(),
                    side: l.side,
                    start_ms: l.start_ms,
                    p_quote_u: l.price_u,
                    p_fill_u: None,
                    m1_ppm: l.m1_ppm,
                    target: 0,
                    status: LegStatus::Open,
                })
                .collect(),
        };
        self.state.tickets.write().insert(p.ticket.clone(), ticket);

        // Lock the hedged symbols (sorted, so two batches cannot deadlock).
        let mut syms: Vec<Symbol> = plan
            .legs
            .iter()
            .filter(|l| l.hedged)
            .map(|l| l.symbol.clone())
            .collect();
        syms.sort();
        syms.dedup();
        let locks: Vec<_> = syms.iter().map(|s| self.symbol_lock(s)).collect();
        let mut guards = Vec::with_capacity(locks.len());
        for l in &locks {
            guards.push(l.lock().await);
        }

        let specs: Vec<OrderSpec> = plan
            .legs
            .iter()
            .filter(|l| l.hedged)
            .map(|l| {
                let (side, price_u) = order_for(l.side, l.p_max_u);
                OrderSpec {
                    symbol: l.symbol.clone(),
                    side,
                    price_u,
                    size: l.size,
                    tif: TimeInForce::Ioc,
                    post_only: false,
                    reduce_only: false,
                    kind: "inception",
                    reason: "pending_opened".into(),
                    pending: Some(p.pending.clone()),
                    serves: vec![Allocation {
                        ticket: p.ticket.clone(),
                        leg_idx: l.idx,
                        contracts: sigma(l.side) * l.size as i64,
                    }],
                }
            })
            .collect();
        let sent = self.send_batch(&trader, specs).await;
        let mut fills: Vec<TicketFill> = Vec::new();
        // Per leg: contracts filled and Σ price × size on the long symbol.
        let mut per_leg: BTreeMap<u8, (u64, u64)> = BTreeMap::new();
        for (o, r) in sent {
            let idx = o.spec.serves.first().map_or(0, |a| a.leg_idx);
            if let Ok(placed) = r {
                for f in &placed.fills {
                    let e = per_leg.entry(idx).or_insert((0, 0));
                    e.0 += f.fill_size;
                    e.1 += u64::try_from(f.fill_price).unwrap_or(0) * f.fill_size;
                }
                fills.extend(
                    self.apply_fills(o.client_order_id, &o.spec.serves, &placed.fills)
                        .await,
                );
            }
            self.state.order_map.write().remove(&o.client_order_id);
        }
        // What the ticket holds now, before the finalize maths.
        self.set_targets(&p.ticket, |idx, side| {
            sigma(side) * per_leg.get(&idx).map_or(0, |x| x.0) as i64
        });
        drop(guards);

        let leg_fills: Vec<LegFill> = plan
            .legs
            .iter()
            .zip(&p.legs)
            .map(|(l, signed)| {
                let (filled, long_notional) = per_leg.get(&l.idx).copied().unwrap_or((0, 0));
                LegFill {
                    hedged: l.hedged,
                    delta: l.size,
                    filled,
                    notional_u: pricing::side_notional_u(pside(l.side), filled, long_notional),
                    quote_price_u: signed.price_u,
                    m1_ppm: signed.m1_ppm,
                }
            })
            .collect();
        let fin = pricing::finalize(&leg_fills, p.stake_u, p.min_payout_per_unit_u);
        let fin = match fin {
            Ok(f) if f.floor_ok => f,
            other => {
                let why = match other {
                    Ok(_) => "fills miss the floor".to_owned(),
                    Err(e) => format!("{e}"),
                };
                tracing::warn!(pending = %p.pending, %why, "inception hedge unusable: unwinding, refund");
                self.unwind_pending(p, &syms, "r_zero", !fills.is_empty())
                    .await;
                self.state.emit(DeskEvent::RefundNeeded {
                    pending: p.pending.clone(),
                    user: p.user.clone(),
                    reason: format!("hedge not filled: {why}"),
                });
                return "refund";
            }
        };

        // Keep ⌈r·Δᵢ⌉, unwind the rest at once (inventory cap zero).
        {
            let mut tickets = self.state.tickets.write();
            if let Some(t) = tickets.get_mut(&p.ticket) {
                t.rs_u = Some(fin.rs_u);
                t.payout_u = fin.payout_u;
                for (l, (lf, f)) in t.legs.iter_mut().zip(leg_fills.iter().zip(&fin.f_u)) {
                    if lf.hedged {
                        l.p_fill_u = Some(*f);
                    }
                }
            }
        }
        let excess = fin.excess.clone();
        self.set_targets(&p.ticket, |idx, side| {
            let i = usize::from(idx);
            let filled = leg_fills.get(i).map_or(0, |l| l.filled);
            sigma(side) * filled.saturating_sub(excess.get(i).copied().unwrap_or(0)) as i64
        });
        for (l, &x) in plan.legs.iter().zip(&fin.excess) {
            if x > 0 {
                self.trade_symbol(&l.symbol, How::Ioc, "excess", Some(&p.pending))
                    .await;
            }
        }
        for s in &syms {
            self.publish(s, "inception").await;
        }
        self.feed_fills(p, &fills).await;
        tracing::info!(
            pending = %p.pending, ticket = %p.ticket, r_ppm = fin.r_ppm, f_u = ?fin.fills_u,
            payout_u = fin.payout_u, rs_u = fin.rs_u, "inception hedged"
        );
        self.state
            .emit(DeskEvent::HedgeFilled(Arc::new(HedgeFilled {
                pending: p.pending.clone(),
                ticket: p.ticket.clone(),
                user: p.user.clone(),
                f_u: fin.fills_u,
                r_ppm: fin.r_ppm,
                fills,
            })));
        "hedged"
    }

    fn set_targets(&self, ticket: &str, f: impl Fn(u8, Side) -> i64) {
        let mut tickets = self.state.tickets.write();
        if let Some(t) = tickets.get_mut(ticket) {
            for l in &mut t.legs {
                l.target = f(l.idx, l.side);
            }
        }
    }

    /// Unwinds everything a pending holds and forgets it (r = 0, floor missed).
    async fn unwind_pending(&self, p: &PendingOpened, syms: &[Symbol], reason: &str, emit: bool) {
        self.set_targets(&p.ticket, |_, _| 0);
        let mut fills = Vec::new();
        for s in syms {
            fills.extend(
                self.trade_symbol(s, How::Ioc, reason, Some(&p.pending))
                    .await,
            );
        }
        self.state.tickets.write().remove(&p.ticket);
        if emit || !fills.is_empty() {
            self.emit_unwound_as(&p.ticket, &p.user, Some(&p.pending), reason, fills);
        }
    }

    /// One `fill` feed item per inception fill (spec section 5: the money-flow ticker).
    async fn feed_fills(&self, p: &PendingOpened, fills: &[TicketFill]) {
        for f in fills {
            let label = match f.venue {
                crate::api::types::Venue::Mock => "mock-filled",
                crate::api::types::Venue::Pascal => "filled on Pascal",
            };
            let text = format!(
                "Hedge {} {} {} @ {}.{:06} ({label}, builder fee {}.{:06})",
                if f.side == crate::api::types::OrderSide::Bid {
                    "bought"
                } else {
                    "sold"
                },
                f.size,
                f.symbol,
                f.price_u / 1_000_000,
                f.price_u % 1_000_000,
                f.builder_fee_u / 1_000_000,
                f.builder_fee_u % 1_000_000,
            );
            let ts_ms = crate::now_ms();
            let r = sqlx::query(
                "INSERT INTO feed_events (kind, text, ticket, payload, ts_ms) VALUES ('fill', ?, ?, ?, ?)",
            )
            .bind(&text)
            .bind(&p.ticket)
            .bind(serde_json::to_string(f).unwrap_or_default())
            .bind(i64::try_from(ts_ms).unwrap_or(i64::MAX))
            .execute(&self.state.db)
            .await;
            match r {
                Ok(_) => self.state.emit(DeskEvent::Feed(Arc::new(FeedItem {
                    kind: FeedKind::Fill,
                    text,
                    ticket: Some(p.ticket.clone()),
                    ts_ms,
                    who: None,
                    amount_u: None,
                    team: None,
                    odds_u: None,
                }))),
                Err(e) => tracing::error!(error = %e, "feed_events write failed"),
            }
        }
    }
}
