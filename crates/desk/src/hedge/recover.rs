//! Start-up recovery and the 30 s reconciliation (spec section 5 "Internal state", section 12
//! runbook "Desk crash": restart, replay fills from `/accounts/{hedger}/fills` since the last
//! stored `seq`, recompute targets).

use std::collections::{BTreeSet, HashMap};

use pascal_rs::{AccountQuery, Fill, PageParams};

use super::Hedger;
use super::exec::{How, pro_rata};
use super::store::{self, FILLS_CURSOR};
use super::targets;
use super::trader::{self, Trader, signed};
use crate::api::types::{TicketStatus, Venue};
use crate::state::Symbol;

impl Hedger {
    /// Start-up (and bus-lag) recovery:
    ///
    /// 1. cancel resting orders a previous process left (their roll timers died with it);
    /// 2. replay `/fills` since the stored `seq` into `pascal_fills` / `fill_allocations`;
    /// 3. resolved symbols and allocations from the DB;
    /// 4. pending and open tickets from the DB, targets recomputed;
    /// 5. `held` from `/accounts/{hedger}` positions, else from the newest fill per symbol;
    /// 6. a `hedge_positions` row per symbol, then a rebalance sweep (10% and 2 contracts).
    pub async fn recover(&self) {
        let venue = self.trader.as_ref().map_or(Venue::Mock, Trader::venue);
        let db = &self.state.db;
        let account = match &self.trader {
            Some(t) => self.cancel_leftovers(t).await,
            None => None,
        };
        if let Some(t) = &self.trader {
            match self.replay_fills(t).await {
                Ok(n) => tracing::info!(fills = n, "fills replayed"),
                Err(e) => {
                    tracing::warn!(error = %e, "fills replay failed; held comes from /accounts")
                }
            }
        }
        match store::resolved_symbols(db).await {
            Ok(v) => self.resolved.write().extend(v),
            Err(e) => tracing::error!(error = %e, "resolved symbols read failed"),
        }
        match super::drift::load_prehedges(db).await {
            Ok(p) => *self.prehedged.write() = p,
            Err(e) => tracing::error!(error = %e, "pre-hedges read failed"),
        }
        match store::load_allocations(db, venue).await {
            Ok(mut a) => {
                let resolved = self.resolved.read().clone();
                a.retain(|_, (s, _)| !resolved.contains(s));
                *self.alloc.write() = a;
            }
            Err(e) => tracing::error!(error = %e, "allocations read failed"),
        }
        match store::load_tickets(db, None).await {
            Ok(stored) => {
                let alloc = self.alloc.read().clone();
                for st in stored {
                    let hedged = alloc.keys().any(|(t, _)| *t == st.ticket.ticket);
                    if st.ticket.status == TicketStatus::Pending && !hedged {
                        // Not hedged (yet): its PendingOpened, if queued, does the work.
                        continue;
                    }
                    self.install(st);
                }
            }
            Err(e) => tracing::error!(error = %e, "tickets read failed"),
        }
        let from_fills = store::held_from_fills(db, venue).await.unwrap_or_default();
        let held: HashMap<Symbol, i64> = match account {
            Some(positions) => {
                for (s, p) in &positions {
                    if from_fills.get(s).copied().unwrap_or(0) != *p {
                        tracing::warn!(symbol = %s, account = p, fills = from_fills.get(s), "held: /accounts and /fills disagree; /accounts wins");
                    }
                }
                positions
            }
            None => from_fills,
        };
        {
            let resolved = self.resolved.read().clone();
            let mut h = self.state.held.write();
            h.clear();
            h.extend(held.into_iter().filter(|(s, _)| !resolved.contains(s)));
        }
        let mut symbols: BTreeSet<Symbol> = targets::symbols_of(&self.state.tickets.read())
            .into_iter()
            .collect();
        symbols.extend(self.state.held.read().keys().cloned());
        for s in &symbols {
            self.publish(s, "recover").await;
        }
        tracing::info!(
            tickets = self.state.tickets.read().len(),
            symbols = symbols.len(),
            "hedge desk recovered"
        );
        for s in &symbols {
            self.trade_symbol(s, How::Sweep, "recover", None).await;
        }
        self.recovered
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// After bus lag (events lost): tickets the desk missed are loaded from the DB, tickets the
    /// DB no longer has as pending or open (settled, cashed out, refunded meanwhile) are closed
    /// and unwound, then `held` is reconciled. Unlike [`Hedger::recover`] it leaves this
    /// process's resting rolls alone.
    pub async fn resync(&self) {
        let stored = match store::load_tickets(&self.state.db, None).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "resync: tickets read failed");
                return;
            }
        };
        let ids: BTreeSet<String> = stored.iter().map(|s| s.ticket.ticket.clone()).collect();
        let alloc = self.alloc.read().clone();
        for st in stored {
            let known = self.state.tickets.read().contains_key(&st.ticket.ticket);
            let hedged = alloc.keys().any(|(t, _)| *t == st.ticket.ticket);
            if !known && (st.ticket.status == TicketStatus::Open || hedged) {
                self.install(st);
            }
        }
        let gone: Vec<String> = self
            .state
            .tickets
            .read()
            .keys()
            .filter(|t| !ids.contains(*t))
            .cloned()
            .collect();
        for t in gone {
            tracing::warn!(ticket = %t, "resync: ticket closed while the bus lagged");
            self.close_ticket(&t, "resync", None, false).await;
        }
        self.reconcile().await;
    }

    /// Cancels every resting order of the hedger and returns its positions (`None` when
    /// `/accounts` fails).
    async fn cancel_leftovers(&self, t: &Trader) -> Option<HashMap<Symbol, i64>> {
        let acct = match self
            .state
            .pascal
            .account(
                t.owner(),
                &AccountQuery {
                    symbol: None,
                    exclude_orders: false,
                },
            )
            .await
        {
            Ok(r) => r.data,
            Err(e) => {
                trader::count_error(&e);
                tracing::warn!(error = %e, "/accounts failed at start");
                return None;
            }
        };
        if let Some(c) = acct.collateral_usd {
            self.set_free_collateral(c);
        }
        for o in &acct.orders {
            match trader::cancel(&self.state.pascal, t, o.client_order_id).await {
                Ok(_) => {
                    tracing::info!(coid = o.client_order_id, symbol = %o.symbol, "leftover order cancelled")
                }
                Err(e) => {
                    tracing::warn!(error = %e, coid = o.client_order_id, "leftover cancel failed")
                }
            }
        }
        Some(
            acct.positions
                .iter()
                .filter(|p| p.size != 0)
                .map(|p| (p.symbol.clone(), p.size))
                .collect(),
        )
    }

    /// Replays `/accounts/{hedger}/fills` newer than the stored cursor (pages newest first),
    /// oldest first into the DB, allocating each fill over what its order served. `held` is
    /// not moved here (it is read from `/accounts` right after). Returns the new fills.
    pub async fn replay_fills(&self, t: &Trader) -> Result<usize, pascal_rs::Error> {
        let db = &self.state.db;
        let venue = t.venue();
        let cursor = crate::db::cursor(db, FILLS_CURSOR)
            .await
            .ok()
            .flatten()
            .and_then(|c| u64::try_from(c).ok())
            .unwrap_or(0);
        let mut page = PageParams::limit(self.cfg.fills_page);
        let mut collected: Vec<Fill> = Vec::new();
        loop {
            let r = self.state.pascal.fills(t.owner(), &[], &page).await?;
            let n = r.data.items.len();
            let mut reached = false;
            for f in r.data.items {
                if f.seq <= cursor {
                    reached = true;
                    break;
                }
                collected.push(f);
            }
            match r.data.next_cursor {
                Some(c) if !reached && n > 0 => {
                    page = PageParams {
                        limit: Some(self.cfg.fills_page),
                        before_cursor: Some(c),
                        at_or_before_seq: None,
                    };
                }
                _ => break,
            }
        }
        collected.sort_by_key(|f| f.seq);
        let mut new = 0;
        let mut max_seq = cursor;
        for f in &collected {
            max_seq = max_seq.max(f.seq);
            let permit = store::order_permit(db, f.client_order_id)
                .await
                .ok()
                .flatten();
            let inserted = match store::insert_fill(db, venue, f, permit.as_deref()).await {
                Ok(b) => b,
                Err(e) => {
                    tracing::error!(error = %e, trade_id = f.trade_id, "replayed fill write failed");
                    false
                }
            };
            if !inserted {
                continue;
            }
            new += 1;
            let mut needs = store::order_serves(db, f.client_order_id)
                .await
                .unwrap_or_default();
            if !needs.is_empty() {
                let done = store::allocated_for_order(db, venue, f.client_order_id)
                    .await
                    .unwrap_or_default();
                for n in &mut needs {
                    n.contracts -= done
                        .get(&(n.ticket.clone(), n.leg_idx))
                        .copied()
                        .unwrap_or(0);
                }
                // This fill's own allocation is not in `done` yet (inserted just now).
                let allocs = pro_rata(&mut needs, signed(f.side, f.fill_size));
                if let Err(e) = store::insert_allocations(db, venue, f.trade_id, &allocs).await {
                    tracing::error!(error = %e, "replayed allocation write failed");
                }
            }
            tracing::info!(
                trade_id = f.trade_id,
                seq = f.seq,
                coid = f.client_order_id,
                "fill replayed"
            );
        }
        if max_seq > cursor
            && let Err(e) =
                crate::db::set_cursor(db, FILLS_CURSOR, i64::try_from(max_seq).unwrap_or(i64::MAX))
                    .await
        {
            tracing::error!(error = %e, "fills cursor write failed");
        }
        Ok(new)
    }

    /// The 30 s reconciliation: `held` from `/accounts/{hedger}` (symbols with an order in
    /// flight are skipped), free collateral, then a rebalance sweep.
    pub async fn reconcile(&self) {
        let Some(t) = self.trader.clone() else {
            return;
        };
        let acct = match self
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
            Ok(r) => r.data,
            Err(e) => {
                trader::count_error(&e);
                tracing::warn!(error = %e, "reconcile: /accounts failed");
                return;
            }
        };
        if let Some(c) = acct.collateral_usd {
            self.set_free_collateral(c);
        }
        let positions: HashMap<Symbol, i64> = acct
            .positions
            .iter()
            .map(|p| (p.symbol.clone(), p.size))
            .collect();
        let mut symbols: BTreeSet<Symbol> = positions.keys().cloned().collect();
        symbols.extend(self.state.held.read().keys().cloned());
        symbols.extend(targets::symbols_of(&self.state.tickets.read()));
        for s in &symbols {
            if self.is_resolved(s) {
                continue;
            }
            let lock = self.symbol_lock(s);
            let Ok(_guard) = lock.try_lock() else {
                continue;
            };
            if self.resting(s) != 0 {
                continue;
            }
            let pascal = positions.get(s).copied().unwrap_or(0);
            let local = self.held(s);
            if pascal != local {
                tracing::warn!(symbol = %s, local, pascal, "held reconciled from /accounts");
                self.state.held.write().insert(s.clone(), pascal);
                drop(_guard);
                self.publish(s, "reconcile").await;
            }
        }
        for s in &symbols {
            self.trade_symbol(s, How::Sweep, "rebalance", None).await;
        }
    }
}
