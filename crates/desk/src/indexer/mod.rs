//! Indexer (spec section 5): `logsSubscribe` on the program through [`crate::solana::listen`],
//! re-reads the touched accounts and the `emit_cpi!` events ([`crate::solana::listen::cpi_events`],
//! D-005), upserts `tickets`, `ticket_legs`, `cashouts` and `feed_events`, and emits
//! `PendingOpened`, `TicketMinted`, `PendingRefunded`, `LegResolved`, `TicketSettled`,
//! `TicketCashedOut` and `TicketChanged` on the bus, each after its DB write ([`store`]).
//!
//! Account state is the source of truth: a `PendingOpened` event is applied from its own
//! (authenticated) bytes because `finalize` may already have closed the pending, but every
//! ticket event makes the indexer re-read the `Ticket` account and apply that. A full resync
//! (`getProgramAccounts` of every `Pending` and `Ticket`) runs at start, after every reconnect
//! and every `INDEXER_RESYNC_S`, so a dropped socket or a validator restart loses nothing.

pub mod config;
pub mod store;

use std::collections::HashSet;

use axum::Router;
use serde_json::Value;
use solana_pubkey::Pubkey;

use crate::program::accounts::disc;
use crate::program::{Pdas, PendingAccount, ProgramEvent, TicketAccount};
use crate::solana::listen::{self, Handler, cpi_events, ws_url_for};
use crate::solana::rpc::{RpcClient, redact};
use crate::state::AppState;
use crate::tipsters::chain as tipsters;
use store::{PendingData, TxFacts};

/// Adds this service's routes: tickets are served by the crank; the views built on the
/// indexed rows, `/tipsters` and `/house` (4.A), are mounted here.
pub fn mount(router: Router<AppState>, state: &AppState) -> Router<AppState> {
    let router = crate::tipsters::mount(router, state);
    crate::house::mount(router, state)
}

/// Runs the listener until shutdown (idle when `DESK_CHAIN=0`).
pub async fn run(state: AppState) {
    if !state.config.chain_enabled {
        tracing::info!("chain disabled (DESK_CHAIN=0): indexer idle");
        state.shutdown_requested().await;
        return;
    }
    let rpc = RpcClient::new(&state.config.solana_rpc_url);
    let ws = state
        .config
        .solana_ws_url
        .clone()
        .unwrap_or_else(|| ws_url_for(&state.config.solana_rpc_url));
    let program = state.config.program_id;
    tracing::info!(rpc = %redact(rpc.url()), ws = %redact(&ws), %program, "indexer starting");
    let indexer = Indexer::new(state.clone(), rpc.clone());
    let resync = state.config.chain_resync_every;
    listen::subscribe(&state, &indexer, &rpc, &ws, &program, resync).await;
}

/// The listener's [`Handler`]: chain → DB → bus.
#[derive(Clone)]
pub struct Indexer {
    state: AppState,
    rpc: RpcClient,
    pdas: Pdas,
}

impl Indexer {
    /// An indexer for `state.config.program_id`.
    pub fn new(state: AppState, rpc: RpcClient) -> Self {
        let pdas = Pdas::new(state.config.program_id);
        Indexer { state, rpc, pdas }
    }

    /// Applies one confirmed transaction (JSON encoding, as `getTransaction` returns it).
    pub async fn apply_transaction(&self, signature: &str, tx: &Value) -> Result<(), String> {
        // L7-1: when the indexer saw it, for `confirm_ms` (approximate, see `store`).
        let observed_ms = Some(crate::now_ms());
        let slot = tx["slot"].as_u64().unwrap_or(0);
        let block_time = tx["blockTime"].as_i64();
        let program = &self.pdas.program;
        let mut tickets: Vec<Pubkey> = Vec::new();
        let mut facts: Vec<TxFacts> = Vec::new();
        let mut config_updated = false;
        let fact_for =
            |tickets: &mut Vec<Pubkey>, facts: &mut Vec<TxFacts>, t: Pubkey| match tickets
                .iter()
                .position(|k| *k == t)
            {
                Some(i) => i,
                None => {
                    tickets.push(t);
                    facts.push(TxFacts {
                        sig: Some(signature.to_owned()),
                        block_time,
                        slot: Some(slot),
                        observed_ms,
                        ..TxFacts::default()
                    });
                    tickets.len() - 1
                }
            };
        for raw in cpi_events(tx, program) {
            let event = match ProgramEvent::decode(&raw) {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(signature, error = %e, "undecodable program event");
                    continue;
                }
            };
            match event {
                ProgramEvent::PendingOpened(e) => {
                    let p = PendingData::from_event(&e, block_time);
                    store::apply_pending_seen(
                        &self.state,
                        &self.pdas,
                        &p,
                        slot,
                        Some(signature),
                        observed_ms,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                }
                ProgramEvent::PendingRefunded { pending, .. } => {
                    store::apply_refund(&self.state, &pending, slot, Some(signature))
                        .await
                        .map_err(|e| e.to_string())?;
                }
                ProgramEvent::TicketMinted(e) => {
                    let i = fact_for(&mut tickets, &mut facts, e.ticket);
                    facts[i].minted = true;
                }
                ProgramEvent::LegResolved {
                    ticket, leg_index, ..
                } => {
                    let i = fact_for(&mut tickets, &mut facts, ticket);
                    facts[i].resolved.push(leg_index);
                }
                ProgramEvent::TicketSettled { ticket, .. } => {
                    let i = fact_for(&mut tickets, &mut facts, ticket);
                    facts[i].settled = true;
                }
                ProgramEvent::TicketCashedOut {
                    ticket, c_u, ts, ..
                } => {
                    let i = fact_for(&mut tickets, &mut facts, ticket);
                    facts[i].cashed_out = Some((c_u, ts));
                }
                // 4.A: the tipster's copy-fee share, and the pause mirror (D-121).
                ProgramEvent::Other(tipsters::disc::TIPSTER_PAID) => {
                    match tipsters::TipsterPaid::decode(&raw) {
                        Ok(p) => crate::tipsters::store::record_paid(
                            &self.state.db,
                            &p.ticket,
                            &p.tipster,
                            p.amount_u,
                            Some(signature),
                        )
                        .await
                        .map_err(|e| e.to_string())?,
                        Err(e) => tracing::warn!(signature, error = %e, "undecodable TipsterPaid"),
                    }
                }
                ProgramEvent::Other(tipsters::disc::CONFIG_UPDATED) => config_updated = true,
                ProgramEvent::VaultFunded { .. } | ProgramEvent::Other(_) => {}
            }
        }
        if config_updated {
            // Not fatal: the next resync (≤ INDEXER_RESYNC_S) reads the config again.
            if let Err(e) = config::mirror(&self.state, &self.rpc, &self.pdas).await {
                tracing::warn!(signature, error = %e, "cannot mirror the config after ConfigUpdated");
            }
        }
        if tickets.is_empty() {
            return Ok(());
        }
        // Re-read the tickets the transaction touched (D-005).
        let (ctx_slot, accounts) = self
            .rpc
            .get_multiple_accounts(&tickets)
            .await
            .map_err(|e| e.to_string())?;
        for ((key, acc), f) in tickets.iter().zip(accounts).zip(&facts) {
            let Some(acc) = acc else {
                return Err(format!("ticket {key} of {signature} not found"));
            };
            let t = TicketAccount::decode(&acc.data, &acc.owner, program)
                .map_err(|e| format!("ticket {key}: {e}"))?;
            store::apply_ticket(&self.state, &self.pdas, key, &t, slot.max(ctx_slot), f)
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Re-reads every `Pending` and `Ticket` of the program and applies them; a pending the DB
    /// holds that is neither on-chain nor minted was refunded while the listener was away.
    pub async fn full_resync(&self) -> Result<(), String> {
        let program = self.pdas.program;
        // Config first: the pause mirror (D-121), and the tipster share the copies' derived
        // `TipsterPaid` rows need. Not fatal: a config read must never stop ticket indexing.
        if let Err(e) = config::mirror(&self.state, &self.rpc, &self.pdas).await {
            tracing::warn!(error = %e, "cannot mirror the config; keeping the last one");
        }
        let (pend_slot, pendings) = self
            .rpc
            .get_program_accounts(&program, &[(0, disc::PENDING.to_vec())])
            .await
            .map_err(|e| e.to_string())?;
        // Tickets after pendings: a finalize between the two reads shows up as a ticket.
        let (ticket_slot, tickets) = self
            .rpc
            .get_program_accounts(&program, &[(0, disc::TICKET.to_vec())])
            .await
            .map_err(|e| e.to_string())?;
        let mut live: HashSet<String> = HashSet::new();
        for (key, acc) in &pendings {
            match PendingAccount::decode(&acc.data, &acc.owner, &program) {
                Ok(p) => {
                    live.insert(key.to_string());
                    let data = PendingData::from_account(*key, &p);
                    store::apply_pending(&self.state, &self.pdas, &data, pend_slot, None)
                        .await
                        .map_err(|e| e.to_string())?;
                }
                Err(e) => tracing::warn!(pending = %key, error = %e, "undecodable pending"),
            }
        }
        for (key, acc) in &tickets {
            match TicketAccount::decode(&acc.data, &acc.owner, &program) {
                Ok(t) => {
                    live.insert(self.pdas.pending(&t.user, t.nonce).to_string());
                    store::apply_ticket(
                        &self.state,
                        &self.pdas,
                        key,
                        &t,
                        ticket_slot,
                        &TxFacts::default(),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                }
                Err(e) => tracing::warn!(ticket = %key, error = %e, "undecodable ticket"),
            }
        }
        let rows = store::pending_rows(&self.state.db)
            .await
            .map_err(|e| e.to_string())?;
        for (pending, row_slot) in rows {
            // Only when the read is at least as new as the row: a pending opened after the
            // read is not missing.
            if live.contains(&pending) || row_slot > pend_slot {
                continue;
            }
            let Ok(key) = pending.parse::<Pubkey>() else {
                continue;
            };
            store::apply_refund(&self.state, &key, pend_slot, None)
                .await
                .map_err(|e| e.to_string())?;
        }
        tracing::debug!(
            pendings = pendings.len(),
            tickets = tickets.len(),
            "chain resync done"
        );
        Ok(())
    }
}

impl Handler for Indexer {
    async fn on_transaction(&self, signature: &str, tx: &Value) -> Result<(), String> {
        self.apply_transaction(signature, tx).await
    }

    async fn resync(&self) -> Result<(), String> {
        self.full_resync().await
    }
}
