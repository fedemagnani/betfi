//! Resolutions (spec section 10, D-012): `POST /markets/search {symbols}` for every open leg
//! from its `start + 2.5 h`, every `CRANK_RESOLVE_POLL_S`; `/markets/resolved` swept every
//! `CRANK_RESOLVED_SWEEP_S` as a backstop. A new resolution is written to `market_resolutions`
//! (and `markets`), then announced as `MarketResolved`; every open leg on the symbol gets
//! `resolve_leg(idx, r)`, and a ticket whose legs are all resolved gets `settle`. Never forced:
//! a market Pascal has not resolved stays open (the 0.5 exception is the authority's).

use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};

use pascal_rs::{Lookback, Market, MarketSearch, ResolvedOrderBy, ResolvedQuery};
use solana_pubkey::Pubkey;

use super::Crank;
use super::chain::{Outcome, record};
use crate::catalogue;
use crate::indexer::store;
use crate::now_ms;
use crate::program::{LegStatus, TicketState, ix, sha256};
use crate::state::DeskEvent;

/// Symbols per search request (Pascal's cap is 500).
const SEARCH_CHUNK: usize = 500;

fn i(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

impl Crank {
    /// The poller: search every `resolve_poll_every`, sweep every `resolved_sweep_every`, then
    /// push every known resolution and every settle-ready ticket on-chain.
    pub(super) async fn resolution_poller(self) {
        let cfg = &self.state.config;
        let mut last_sweep: Option<Instant> = None;
        loop {
            if let Err(e) = self.poll_once().await {
                tracing::warn!(error = %e, "resolution poll failed");
            }
            if last_sweep.is_none_or(|t| t.elapsed() >= cfg.resolved_sweep_every) {
                last_sweep = Some(Instant::now());
                if let Err(e) = self.sweep_resolved().await {
                    tracing::warn!(error = %e, "resolved sweep failed");
                }
            }
            if let Err(e) = self.push_resolutions().await {
                tracing::warn!(error = %e, "pushing resolutions failed");
            }
            if !self
                .sleep(cfg.resolve_poll_every.max(Duration::from_millis(100)))
                .await
            {
                return;
            }
        }
    }

    /// Open legs' symbols with the earliest start among them (`ticket_legs` of open tickets).
    async fn open_symbols(&self) -> Result<BTreeMap<String, u64>, String> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT l.symbol, MIN(l.start_ms) FROM ticket_legs l
             JOIN tickets t ON t.ticket = l.ticket
             WHERE t.state = 'open' AND l.status = 'open' AND l.symbol NOT LIKE 'sha256:%'
             GROUP BY l.symbol",
        )
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| e.to_string())?;
        Ok(rows
            .into_iter()
            .map(|(s, ms)| (s, u64::try_from(ms).unwrap_or(0)))
            .collect())
    }

    async fn known_resolutions(&self) -> Result<HashSet<String>, String> {
        let rows: Vec<String> = sqlx::query_scalar("SELECT symbol FROM market_resolutions")
            .fetch_all(&self.state.db)
            .await
            .map_err(|e| e.to_string())?;
        Ok(rows.into_iter().collect())
    }

    /// One `POST /markets/search` round over the eligible open legs.
    pub(super) async fn poll_once(&self) -> Result<(), String> {
        let open = self.open_symbols().await?;
        if open.is_empty() {
            return Ok(());
        }
        let known = self.known_resolutions().await?;
        let after = u64::try_from(self.state.config.resolve_after.as_millis()).unwrap_or(u64::MAX);
        let now = self.pascal_now_ms();
        let due: Vec<String> = open
            .iter()
            .filter(|(s, start)| !known.contains(*s) && start.saturating_add(after) <= now)
            .map(|(s, _)| s.clone())
            .collect();
        for chunk in due.chunks(SEARCH_CHUNK) {
            let body = MarketSearch {
                symbols: chunk.to_vec(),
                ..MarketSearch::default()
            };
            let resp = self
                .state
                .pascal
                .search_markets_lenient(&body)
                .await
                .map_err(|e| e.to_string())?;
            self.saw_pascal_time(resp.state_time_ms.or(resp.server_time_ms));
            for (_, m) in resp.data.items {
                self.observe(&m, "search").await?;
            }
        }
        Ok(())
    }

    /// The `/markets/resolved` backstop: resolutions of symbols with open legs.
    pub(super) async fn sweep_resolved(&self) -> Result<(), String> {
        let mut open = self.open_symbols().await?;
        let unnamed = store::has_unnamed(&self.state.db)
            .await
            .map_err(|e| e.to_string())?;
        if open.is_empty() && !unnamed {
            return Ok(());
        }
        let resp = self
            .state
            .pascal
            .resolved_markets_lenient(ResolvedQuery {
                lookback: Lookback::Days3,
                order_by: ResolvedOrderBy::ResolutionTime,
                limit: 200,
            })
            .await
            .map_err(|e| e.to_string())?;
        self.saw_pascal_time(resp.state_time_ms.or(resp.server_time_ms));
        // Legs known only by their hashes (a fresh database met their ticket on chain): named
        // from Pascal's resolved markets when they list theirs, then resolved like any other
        // (31 Oct). The by-volume page too: a busy game stays on it for the whole lookback,
        // when 200 newer resolutions of every sport push it off the first.
        let mut markets: Vec<(Market, serde_json::Value)> = Vec::new();
        let mut take = |r: &pascal_rs::Response<pascal_rs::Lenient<Market>>| {
            let raws = r.raw_items();
            for (i, m) in &r.data.items {
                markets.push((m.clone(), raws.get(*i).cloned().unwrap_or_default()));
            }
        };
        take(&resp);
        if unnamed {
            if let Ok(by_volume) = self
                .state
                .pascal
                .resolved_markets_lenient(ResolvedQuery {
                    lookback: Lookback::Days3,
                    order_by: ResolvedOrderBy::VolumeAllTime,
                    limit: 200,
                })
                .await
            {
                take(&by_volume);
            }
            let known: Vec<_> = markets
                .iter()
                .filter_map(|(m, raw)| catalogue::resolved_market(m).map(|cm| (cm, raw.clone())))
                .collect();
            let named = store::name_unnamed(&self.state, &known)
                .await
                .map_err(|e| e.to_string())?;
            if !named.is_empty() {
                open = self.open_symbols().await?;
            }
        }
        let mut seen = HashSet::new();
        for (m, _) in &markets {
            if open.contains_key(&m.symbol) && seen.insert(m.symbol.clone()) {
                self.observe(m, "resolved").await?;
            }
        }
        Ok(())
    }

    /// Records `m`'s resolution if it has one and it is new, then announces it.
    async fn observe(&self, m: &Market, source: &str) -> Result<(), String> {
        let Some(r) = &m.resolution else {
            return Ok(());
        };
        let price_u = u64::try_from(r.price.clamp(0, 1_000_000)).unwrap_or(0);
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO market_resolutions (symbol, price_u, resolved_ms, seq, source,
                seen_ms) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&m.symbol)
        .bind(i(price_u))
        .bind(i(r.market_resolution_ts_ms))
        .bind(i(r.seq))
        .bind(source)
        .bind(i(now_ms()))
        .execute(&self.state.db)
        .await
        .map_err(|e| e.to_string())?
        .rows_affected()
            == 1;
        if !inserted {
            return Ok(());
        }
        sqlx::query(
            "UPDATE markets SET resolved_price_u = ?, resolution_seq = ?, resolved_ms = ?
             WHERE symbol = ? AND resolved_price_u IS NULL",
        )
        .bind(i(price_u))
        .bind(i(r.seq))
        .bind(i(r.market_resolution_ts_ms))
        .bind(&m.symbol)
        .execute(&self.state.db)
        .await
        .map_err(|e| e.to_string())?;
        tracing::info!(symbol = %m.symbol, price_u, source, "market resolved");
        self.state.emit(DeskEvent::MarketResolved {
            symbol: m.symbol.clone(),
            resolution_u: price_u,
            resolved_ms: r.market_resolution_ts_ms,
        });
        Ok(())
    }

    /// Re-drives every known resolution whose legs are still open, and every ticket whose legs
    /// are all resolved (restarts, lost bus events, failed sends).
    async fn push_resolutions(&self) -> Result<(), String> {
        let todo: Vec<(String, i64)> = sqlx::query_as(
            "SELECT DISTINCT r.symbol, r.price_u FROM market_resolutions r
             JOIN ticket_legs l ON l.symbol = r.symbol AND l.status = 'open'
             JOIN tickets t ON t.ticket = l.ticket AND t.state = 'open'",
        )
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| e.to_string())?;
        for (symbol, price_u) in todo {
            if self.due(&format!("resolve:{symbol}")) {
                tokio::spawn(
                    self.clone()
                        .resolve_symbol(symbol, u64::try_from(price_u).unwrap_or(0)),
                );
            }
        }
        let ready: Vec<String> = sqlx::query_scalar(
            "SELECT t.ticket FROM tickets t WHERE t.state = 'open'
             AND NOT EXISTS (SELECT 1 FROM ticket_legs l WHERE l.ticket = t.ticket
                             AND l.status = 'open')",
        )
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| e.to_string())?;
        for ticket in ready {
            if self.due(&format!("settle:{ticket}")) {
                tokio::spawn(self.clone().maybe_settle(ticket));
            }
        }
        Ok(())
    }

    /// `resolve_leg(idx, r)` on every open leg of `symbol` (one transaction per ticket), then
    /// `settle` where that resolved the last leg.
    pub(super) async fn resolve_symbol(self, symbol: String, resolution_u: u64) {
        let key = format!("resolve:{symbol}");
        let Some(_guard) = self.claim(key.clone()) else {
            return;
        };
        let r_ppm = u32::try_from(resolution_u.min(1_000_000)).unwrap_or(0);
        let tickets: Result<Vec<String>, _> = sqlx::query_scalar(
            "SELECT DISTINCT l.ticket FROM ticket_legs l JOIN tickets t ON t.ticket = l.ticket
             WHERE l.symbol = ? AND l.status = 'open' AND t.state = 'open'",
        )
        .bind(&symbol)
        .fetch_all(&self.state.db)
        .await;
        let tickets = match tickets {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, %symbol, "cannot list legs to resolve");
                self.back_off(&key);
                return;
            }
        };
        let hash = sha256(symbol.as_bytes());
        let mut failed = false;
        for ticket in tickets {
            let Ok(ticket_key) = ticket.parse::<Pubkey>() else {
                continue;
            };
            // The chain decides which legs are still open and that they are this symbol.
            let t = match self.chain.ticket(&ticket_key).await {
                Ok(Some(t)) if t.state == TicketState::Open => t,
                Ok(_) => continue,
                Err(e) => {
                    tracing::warn!(%ticket, error = %e, "cannot read ticket to resolve");
                    failed = true;
                    continue;
                }
            };
            let legs: Vec<u8> = t
                .legs
                .iter()
                .enumerate()
                .filter(|(_, l)| l.status == LegStatus::Open && l.symbol_hash == hash)
                .filter_map(|(k, _)| u8::try_from(k).ok())
                .collect();
            if legs.is_empty() {
                continue;
            }
            let crank = self.chain.pubkey();
            let ixs: Vec<_> = legs
                .iter()
                .map(|&k| ix::resolve_leg(&self.chain.pdas, &crank, &ticket_key, k, r_ppm))
                .collect();
            let all_in = t.legs.iter().enumerate().all(|(k, l)| {
                l.status != LegStatus::Open || legs.contains(&u8::try_from(k).unwrap_or(u8::MAX))
            });
            match self.chain.send(&ixs).await {
                Outcome::Landed(sig) => {
                    for &k in &legs {
                        record(
                            &self.state.db,
                            "resolve",
                            &ticket,
                            Some(k),
                            "confirmed",
                            Some(&sig),
                            None,
                        )
                        .await;
                    }
                    tracing::info!(%ticket, %symbol, r_ppm, %sig, "resolve_leg confirmed");
                    if all_in {
                        tokio::spawn(self.clone().maybe_settle(ticket.clone()));
                    }
                }
                Outcome::Refused(err) => {
                    record(
                        &self.state.db,
                        "resolve",
                        &ticket,
                        legs.first().copied(),
                        "rejected",
                        None,
                        Some(&err),
                    )
                    .await;
                    tracing::warn!(%ticket, %symbol, error = %err, "resolve_leg refused");
                    failed = true;
                }
                Outcome::Transient(err) => {
                    tracing::warn!(%ticket, %symbol, error = %err, "resolve_leg not confirmed");
                    failed = true;
                }
            }
        }
        if failed {
            self.back_off(&key);
        } else {
            self.clear_backoff(&key);
        }
    }

    /// `settle` once every leg of `ticket` is resolved on-chain (permissionless; the crank pays).
    pub(super) async fn maybe_settle(self, ticket: String) {
        let key = format!("settle:{ticket}");
        let Some(_guard) = self.claim(key.clone()) else {
            return;
        };
        let Ok(ticket_key) = ticket.parse::<Pubkey>() else {
            return;
        };
        let t = match self.chain.ticket(&ticket_key).await {
            Ok(Some(t)) => t,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(%ticket, error = %e, "cannot read ticket to settle");
                self.back_off(&key);
                return;
            }
        };
        if t.state != TicketState::Open || t.legs.iter().any(|l| l.status == LegStatus::Open) {
            return;
        }
        let crank = self.chain.pubkey();
        let pdas = self.chain.pdas;
        let ixs = [
            ix::create_ata_idempotent(&crank, &t.user, &pdas.usdc_mint()),
            ix::settle(&pdas, &crank, &ticket_key, &t),
        ];
        match self.chain.send(&ixs).await {
            Outcome::Landed(sig) => {
                record(
                    &self.state.db,
                    "settle",
                    &ticket,
                    None,
                    "confirmed",
                    Some(&sig),
                    None,
                )
                .await;
                tracing::info!(%ticket, %sig, "settle confirmed");
                self.clear_backoff(&key);
            }
            Outcome::Refused(err) => {
                record(
                    &self.state.db,
                    "settle",
                    &ticket,
                    None,
                    "rejected",
                    None,
                    Some(&err),
                )
                .await;
                tracing::warn!(%ticket, error = %err, "settle refused");
                self.back_off(&key);
            }
            Outcome::Transient(err) => {
                tracing::warn!(%ticket, error = %err, "settle not confirmed");
                self.back_off(&key);
            }
        }
    }
}
