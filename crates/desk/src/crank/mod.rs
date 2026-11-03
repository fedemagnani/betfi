//! Crank (spec section 5): `finalize` on `HedgeFilled`, `refund_pending` on `RefundNeeded` or a
//! pending past `expires_at + 15 s`, the resolution poller (`POST /markets/search` from
//! `start_ts + 2.5 h`, `/markets/resolved` sweep, D-012) emitting `MarketResolved` and sending
//! `resolve_leg` then `settle`. Serves `GET /tickets/:user` and `GET /ticket/:id` from the DB
//! ([`tickets`]), with live cash-out values from [`AppState::cash_out_for`]. Signs and sends
//! through [`crate::solana`] with the key at `Config.crank_keypair_path` ([`chain`]).
//!
//! Exactly-once: every send first re-reads the account it acts on (a ticket that exists is never
//! finalized again, a resolved leg never resolved again), the program refuses duplicates anyway
//! (PDA `init`, `LegAlreadyResolved`, `TicketNotOpen`), and a refusal is reported as
//! `FinalizeRejected` only once the chain shows no ticket, so a finalize that landed despite a
//! transport error is never unwound by the hedge desk. One task per target at a time (in-flight
//! set); the `market_update` push path is the plan's cut rule (polling only).

pub mod chain;
mod resolve;
pub mod tickets;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::routing::get;
use solana_pubkey::Pubkey;

use crate::indexer::store::{self, TxFacts};
use crate::now_ms;
use crate::program::{Pdas, PendingAccount, TicketAccount, consts::REFUND_GRACE, ix};
use crate::solana::rpc::{RpcClient, redact};
use crate::solana::signer::Keypair;
use crate::state::{AppState, DeskEvent, HedgeFilled, Recv, recv};
use chain::{Chain, Outcome, record};

/// Adds `GET /tickets/{user}` and `GET /ticket/{id}`.
pub fn mount(router: Router<AppState>, _state: &AppState) -> Router<AppState> {
    router
        .route("/tickets/{user}", get(tickets::tickets))
        .route("/ticket/{id}", get(tickets::ticket))
}

/// Runs the crank until shutdown. Idle (routes only) when `DESK_CHAIN=0` or without a crank key.
pub async fn run(state: AppState) {
    if !state.config.chain_enabled {
        tracing::info!("chain disabled (DESK_CHAIN=0): crank idle");
        state.shutdown_requested().await;
        return;
    }
    let key = match Keypair::read(&state.config.crank_keypair_path) {
        Ok(k) => k,
        Err(e) => {
            tracing::error!(error = %e, "no crank key: the crank sends nothing");
            state.shutdown_requested().await;
            return;
        }
    };
    let rpc = RpcClient::new(&state.config.solana_rpc_url);
    tracing::info!(rpc = %redact(rpc.url()), crank = %key.pubkey(), "crank starting");
    let crank = Crank::new(
        state.clone(),
        Chain::new(rpc, Pdas::new(state.config.program_id), key),
    );
    crank.run().await;
}

/// The crank's shared state; cheap to clone into tasks.
#[derive(Clone)]
pub struct Crank {
    state: AppState,
    chain: Arc<Chain>,
    /// Targets with a task running (`finalize:<pending>`, `refund:<pending>`, ...).
    inflight: Arc<Mutex<HashSet<String>>>,
    /// Next attempt allowed for a target the timers retry, with its current backoff.
    backoff: Arc<Mutex<HashMap<String, (Instant, Duration)>>>,
    /// Latest Pascal clock seen (envelope `state_time_ms`), Unix ms.
    pascal_ms: Arc<AtomicU64>,
}

/// Removes its key from the in-flight set when the task ends.
struct InFlight {
    set: Arc<Mutex<HashSet<String>>>,
    key: String,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.set
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.key);
    }
}

fn now_s() -> i64 {
    i64::try_from(now_ms() / 1000).unwrap_or(i64::MAX)
}

impl Crank {
    /// A crank over `chain`.
    pub fn new(state: AppState, chain: Arc<Chain>) -> Self {
        Crank {
            state,
            chain,
            inflight: Arc::default(),
            backoff: Arc::default(),
            pascal_ms: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Claims `key` for one task, or `None` if a task already holds it.
    fn claim(&self, key: String) -> Option<InFlight> {
        let mut set = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
        set.insert(key.clone()).then(|| InFlight {
            set: self.inflight.clone(),
            key,
        })
    }

    /// Whether a timer may try `key` now.
    fn due(&self, key: &str) -> bool {
        let map = self.backoff.lock().unwrap_or_else(|e| e.into_inner());
        map.get(key).is_none_or(|(at, _)| Instant::now() >= *at)
    }

    /// Pushes `key`'s next timer attempt back (10 s, doubling, at most 5 min).
    fn back_off(&self, key: &str) {
        let mut map = self.backoff.lock().unwrap_or_else(|e| e.into_inner());
        let next = map.get(key).map_or(Duration::from_secs(10), |(_, d)| {
            (*d * 2).min(Duration::from_secs(300))
        });
        map.insert(key.to_owned(), (Instant::now() + next, next));
    }

    fn clear_backoff(&self, key: &str) {
        self.backoff
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
    }

    /// Applies the ticket a `finalize` just minted, as the indexer would (D-665): the listener
    /// learns of the transaction from the RPC's logs socket and `getTransaction`, which lagged
    /// tens of seconds on devnet, while the Ticket account reads at once. `apply_ticket` moves a
    /// row from `pending` only once, so the indexer's own pass later just backfills; a failed
    /// read here leaves the mint to it.
    async fn index_minted(&self, pdas: &Pdas, pending: &PendingAccount, sig: &str) {
        let key = pdas.ticket(&pending.user, pending.nonce);
        let read = self.chain.rpc.get_multiple_accounts(&[key]).await;
        let (slot, acc) = match read {
            Ok((slot, mut accs)) => (slot, accs.pop().flatten()),
            Err(e) => {
                tracing::warn!(ticket = %key, error = %e, "minted ticket not read; the indexer will apply it");
                return;
            }
        };
        let Some(acc) = acc else {
            tracing::warn!(ticket = %key, "minted ticket not visible yet; the indexer will apply it");
            return;
        };
        let t = match TicketAccount::decode(&acc.data, &acc.owner, &pdas.program) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(ticket = %key, error = %e, "minted ticket undecodable");
                return;
            }
        };
        let facts = TxFacts {
            sig: Some(sig.to_owned()),
            minted: true,
            observed_ms: Some(now_ms()),
            ..TxFacts::default()
        };
        if let Err(e) = store::apply_ticket(&self.state, pdas, &key, &t, slot, &facts).await {
            tracing::warn!(ticket = %key, error = %e, "minted ticket not applied; the indexer will");
        }
    }

    /// Pascal's clock as far as the desk has seen it (the mock reports tape time, D-033), never
    /// behind the wall clock.
    pub fn pascal_now_ms(&self) -> u64 {
        let cat = self.state.catalogue().map_or(0, |c| c.as_of_ms);
        now_ms()
            .max(cat)
            .max(self.pascal_ms.load(Ordering::Relaxed))
    }

    fn saw_pascal_time(&self, ms: Option<u64>) {
        if let Some(ms) = ms {
            self.pascal_ms.fetch_max(ms, Ordering::Relaxed);
        }
    }

    async fn sleep(&self, d: Duration) -> bool {
        tokio::select! {
            () = self.state.shutdown_requested() => false,
            () = tokio::time::sleep(d) => true,
        }
    }

    /// The bus loop plus the refund timer and the resolution poller.
    pub async fn run(self) {
        // Subscribe before anything awaits, so no event of this run is missed.
        let mut rx = self.state.subscribe();
        let timer = tokio::spawn(self.clone().refund_timer());
        let poller = tokio::spawn(self.clone().resolution_poller());
        loop {
            let event = tokio::select! {
                () = self.state.shutdown_requested() => break,
                r = recv(&mut rx, "crank") => r,
            };
            match event {
                Recv::Event(DeskEvent::HedgeFilled(h)) => {
                    tokio::spawn(self.clone().finalize(h));
                }
                Recv::Event(DeskEvent::RefundNeeded {
                    pending, reason, ..
                }) => {
                    tracing::info!(%pending, %reason, "refund asked");
                    tokio::spawn(self.clone().refund(pending));
                }
                Recv::Event(DeskEvent::MarketResolved {
                    symbol,
                    resolution_u,
                    ..
                }) => {
                    tokio::spawn(self.clone().resolve_symbol(symbol, resolution_u));
                }
                Recv::Event(DeskEvent::LegResolved { ticket, .. }) => {
                    tokio::spawn(self.clone().maybe_settle(ticket));
                }
                Recv::Event(_) => {}
                // The timer and the poller re-scan the DB on their next tick.
                Recv::Lagged(_) => {}
                Recv::Closed => break,
            }
        }
        timer.abort();
        poller.abort();
    }

    // --- finalize --------------------------------------------------------------------------

    /// `finalize(f_u, r_ppm)` for a filled hedge (L2-3). A refusal becomes `FinalizeRejected`;
    /// a transport failure is retried while `now < expires_at`.
    pub async fn finalize(self, h: Arc<HedgeFilled>) {
        let Some(_guard) = self.claim(format!("finalize:{}", h.pending)) else {
            return;
        };
        let reject = |error: String| {
            let this = self.clone();
            let h = h.clone();
            async move {
                record(
                    &this.state.db,
                    "finalize",
                    &h.pending,
                    None,
                    "rejected",
                    None,
                    Some(&error),
                )
                .await;
                tracing::warn!(pending = %h.pending, %error, "finalize rejected");
                this.state.emit(DeskEvent::FinalizeRejected {
                    pending: h.pending.clone(),
                    user: h.user.clone(),
                    error,
                });
            }
        };
        let Ok(pending_key) = h.pending.parse::<Pubkey>() else {
            reject(format!("{} is not a pubkey", h.pending)).await;
            return;
        };
        let pdas = self.chain.pdas;
        let mut expires_at: Option<i64> = None;
        let mut last = String::new();
        let mut reads = 0u32;
        loop {
            if self.state.is_shutting_down() {
                return;
            }
            // Re-read: a ticket that exists is never finalized twice.
            let read = self.read_pending_and_ticket(&pending_key).await;
            let (pending, ticket_exists) = match read {
                Ok(v) => v,
                Err(e) => {
                    reads += 1;
                    last = e;
                    if expires_at.is_some_and(|x| now_s() >= x) || reads > 60 {
                        reject(format!("chain unreachable until expiry: {last}")).await;
                        return;
                    }
                    if !self.sleep(Duration::from_secs(1)).await {
                        return;
                    }
                    continue;
                }
            };
            if ticket_exists {
                if !last.is_empty() {
                    // An attempt whose confirmation was lost landed after all.
                    record(
                        &self.state.db,
                        "finalize",
                        &h.pending,
                        None,
                        "confirmed",
                        None,
                        Some("landed; confirmation lost"),
                    )
                    .await;
                }
                return;
            }
            let Some(pending) = pending else {
                reject("pending not found (refunded, or never opened)".into()).await;
                return;
            };
            expires_at = Some(pending.expires_at);
            if now_s() >= pending.expires_at {
                let why = if last.is_empty() {
                    "PendingExpired (hedge filled after expires_at)".to_owned()
                } else {
                    format!("PendingExpired after transient failures: {last}")
                };
                reject(why).await;
                return;
            }
            if h.f_u.len() != pending.legs.len() {
                reject(format!(
                    "{} fills for {} legs",
                    h.f_u.len(),
                    pending.legs.len()
                ))
                .await;
                return;
            }
            let mint = pdas.usdc_mint();
            let crank = self.chain.pubkey();
            let mut ixs = vec![ix::create_ata_idempotent(&crank, &pending.user, &mint)];
            let copy_tipster = match pending.copied_from {
                Some(orig) => match self.chain.ticket(&orig).await {
                    Ok(Some(o)) => {
                        // The copy's tipster is the original's user (2.F).
                        let tipster = o.user;
                        ixs.push(ix::create_ata_idempotent(&crank, &tipster, &mint));
                        Some(tipster)
                    }
                    Ok(None) => {
                        reject(format!("original ticket {orig} not found")).await;
                        return;
                    }
                    Err(e) => {
                        last = e;
                        if !self.sleep(Duration::from_secs(1)).await {
                            return;
                        }
                        continue;
                    }
                },
                None => None,
            };
            let remaining = ix::finalize_remaining(&pdas, &pending, copy_tipster.as_ref());
            ixs.push(ix::finalize(
                &pdas,
                &crank,
                &pending_key,
                &pending,
                &h.f_u,
                h.r_ppm,
                remaining,
            ));
            match self.chain.send(&ixs).await {
                Outcome::Landed(sig) => {
                    record(
                        &self.state.db,
                        "finalize",
                        &h.pending,
                        None,
                        "confirmed",
                        Some(&sig),
                        None,
                    )
                    .await;
                    tracing::info!(pending = %h.pending, %sig, "finalize confirmed");
                    self.index_minted(&pdas, &pending, &sig).await;
                    return;
                }
                Outcome::Refused(err) => {
                    // An earlier attempt may have landed after all.
                    if matches!(
                        self.read_pending_and_ticket(&pending_key).await,
                        Ok((_, true))
                    ) {
                        return;
                    }
                    reject(err).await;
                    return;
                }
                Outcome::Transient(err) => {
                    tracing::warn!(pending = %h.pending, error = %err, "finalize not confirmed; retrying");
                    last = err;
                    if !self.sleep(Duration::from_millis(500)).await {
                        return;
                    }
                }
            }
        }
    }

    async fn read_pending_and_ticket(
        &self,
        pending_key: &Pubkey,
    ) -> Result<(Option<crate::program::PendingAccount>, bool), String> {
        let pending = self.chain.pending(pending_key).await?;
        let ticket_key = match &pending {
            Some(p) => self.chain.pdas.ticket(&p.user, p.nonce),
            None => {
                // Without the pending, find the user and nonce in the DB to look for a ticket.
                let row: Option<(String, i64)> =
                    sqlx::query_as("SELECT user, nonce FROM tickets WHERE pending = ?")
                        .bind(pending_key.to_string())
                        .fetch_optional(&self.state.db)
                        .await
                        .map_err(|e| e.to_string())?;
                match row.and_then(|(u, n)| {
                    Some((u.parse::<Pubkey>().ok()?, crate::db::nonce_from_db(n)))
                }) {
                    Some((user, nonce)) => self.chain.pdas.ticket(&user, nonce),
                    None => return Ok((None, false)),
                }
            }
        };
        let exists = self
            .chain
            .accounts(&[ticket_key])
            .await?
            .pop()
            .flatten()
            .is_some();
        Ok((pending, exists))
    }

    // --- refund ----------------------------------------------------------------------------

    /// `refund_pending` once `now ≥ expires_at + 15 s` (the program's grace).
    pub async fn refund(self, pending: String) {
        let key = format!("refund:{pending}");
        let Some(_guard) = self.claim(key.clone()) else {
            return;
        };
        let Ok(pending_key) = pending.parse::<Pubkey>() else {
            return;
        };
        let mut sent = false;
        for attempt in 0..10u32 {
            let p = match self.chain.pending(&pending_key).await {
                Ok(Some(p)) => p,
                Ok(None) => {
                    // Finalized or refunded already (the indexer records which), possibly by an
                    // earlier attempt of ours whose confirmation was lost.
                    if sent {
                        record(
                            &self.state.db,
                            "refund",
                            &pending,
                            None,
                            "confirmed",
                            None,
                            Some("pending gone after an unconfirmed send"),
                        )
                        .await;
                    }
                    self.clear_backoff(&key);
                    return;
                }
                Err(e) => {
                    tracing::warn!(%pending, error = %e, "cannot read pending for refund");
                    if !self.sleep(Duration::from_secs(2)).await {
                        return;
                    }
                    continue;
                }
            };
            let opens = p.expires_at.saturating_add(REFUND_GRACE);
            let wait = opens.saturating_sub(now_s());
            if wait > 0
                && !self
                    .sleep(Duration::from_secs(u64::try_from(wait).unwrap_or(0)))
                    .await
            {
                return;
            }
            let pdas = self.chain.pdas;
            let crank = self.chain.pubkey();
            let ixs = [
                ix::create_ata_idempotent(&crank, &p.user, &pdas.usdc_mint()),
                ix::refund_pending(&pdas, &crank, &p.user, p.nonce, &p.rent_receiver()),
            ];
            match self.chain.send(&ixs).await {
                Outcome::Landed(sig) => {
                    record(
                        &self.state.db,
                        "refund",
                        &pending,
                        None,
                        "confirmed",
                        Some(&sig),
                        None,
                    )
                    .await;
                    tracing::info!(%pending, %sig, "refund confirmed");
                    self.clear_backoff(&key);
                    return;
                }
                Outcome::Refused(err) => {
                    // The cluster clock may trail ours by a few seconds (RefundTooEarly).
                    tracing::warn!(%pending, error = %err, attempt, "refund refused");
                    if err.contains("RefundTooEarly") && attempt < 9 {
                        if !self.sleep(Duration::from_secs(2)).await {
                            return;
                        }
                        continue;
                    }
                    record(
                        &self.state.db,
                        "refund",
                        &pending,
                        None,
                        "rejected",
                        None,
                        Some(&err),
                    )
                    .await;
                    self.back_off(&key);
                    return;
                }
                Outcome::Transient(err) => {
                    sent = true;
                    tracing::warn!(%pending, error = %err, "refund not confirmed; retrying");
                    if !self.sleep(Duration::from_secs(1)).await {
                        return;
                    }
                }
            }
        }
        record(
            &self.state.db,
            "refund",
            &pending,
            None,
            "failed",
            None,
            Some("gave up after 10 attempts"),
        )
        .await;
        self.back_off(&key);
    }

    /// Every `crank_tick`: pendings still `pending` in the DB at `expires_at + 15 s` are
    /// refunded (spec flow 4), whether or not the hedge desk asked.
    async fn refund_timer(self) {
        loop {
            if !self.sleep(self.state.config.crank_tick).await {
                return;
            }
            let due: Result<Vec<String>, _> = sqlx::query_scalar(
                "SELECT pending FROM tickets WHERE state = 'pending' AND expires_at IS NOT NULL
                 AND expires_at + ? <= ?",
            )
            .bind(REFUND_GRACE)
            .bind(now_s())
            .fetch_all(&self.state.db)
            .await;
            match due {
                Ok(list) => {
                    for pending in list {
                        if self.due(&format!("refund:{pending}")) {
                            tokio::spawn(self.clone().refund(pending));
                        }
                    }
                }
                Err(e) => tracing::warn!(error = %e, "refund timer cannot read the DB"),
            }
        }
    }
}
