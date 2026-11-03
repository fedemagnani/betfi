//! Chain state into the DB, then onto the bus (state.rs rule: DB write first, then emit).
//!
//! Every write is idempotent and keyed by the Ticket PDA (D-038): applying the same transaction
//! or account twice changes nothing and emits nothing. Bus events fire only on a state
//! transition the DB did not have yet (`pending → open`, a leg `open → resolved`,
//! `open → settled | cashed_out`, `pending → refunded`), so a replay, a resync after a
//! reconnect, or a restart never announces a change twice.
//!
//! 7.0c (l7-contracts L7-1): the slot of the `open_pending`, `cash_out` and `settle`
//! transactions, and `confirm_ms` = the indexer's clock when it applied the transaction minus
//! the block time (approximate: block time has whole-second resolution, and the gap includes
//! the listener's `getTransaction` round trip; `NULL` when the block time is unknown). Both are
//! written once, only from the transaction that made the transition (a resync never invents
//! them). Mint feed items carry `who`, `amount_u`, `team` and `odds_u`; a minted copy is kind
//! `copy`.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;
use solana_pubkey::Pubkey;
use sqlx::{Row, SqlitePool};

use crate::api::types::{FeedKind, LegStatus, QuoteLeg as ApiQuoteLeg, Side, TeamSide};
use crate::crank::tickets::load_ticket;
use crate::now_ms;
use crate::program::events::PendingOpenedEvent;
use crate::program::{
    self, LegStatus as ChainLegStatus, Pdas, PendingAccount, QuoteLeg, TicketAccount, TicketState,
    hex, sha256,
};
use crate::state::{AppState, DeskEvent, FeedItem, PendingLeg, PendingOpened};

fn i(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// What a pending carries, from its `PendingOpened` event or its account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingData {
    /// Pending PDA.
    pub pending: Pubkey,
    /// Owner.
    pub user: Pubkey,
    /// Nonce.
    pub nonce: u64,
    /// Legs, ticket order.
    pub legs: Vec<QuoteLeg>,
    /// S.
    pub stake_u: u64,
    /// Copy fee.
    pub copy_fee_u: u64,
    /// Offered odds.
    pub odds_offered_u: u64,
    /// Floor.
    pub min_payout_per_unit_u: u64,
    /// Quote TTL.
    pub quote_expires_at: i64,
    /// Pending expiry.
    pub expires_at: i64,
    /// sha256 of the signed quote.
    pub quote_hash: [u8; 32],
    /// Own tip.
    pub is_tip: bool,
    /// `Some(user)` for a tip.
    pub tipster: Option<Pubkey>,
    /// Original of a copy.
    pub copied_from: Option<Pubkey>,
    /// When it was opened, Unix s (block time or `created_at`).
    pub opened_at: Option<i64>,
}

impl PendingData {
    /// From the program's event (the tipster is the user for a tip, as `open_pending` stores).
    pub fn from_event(e: &PendingOpenedEvent, block_time: Option<i64>) -> Self {
        PendingData {
            pending: e.pending,
            user: e.user,
            nonce: e.nonce,
            legs: e.legs.clone(),
            stake_u: e.stake_u,
            copy_fee_u: e.copy_fee_u,
            odds_offered_u: e.odds_offered_u,
            min_payout_per_unit_u: e.min_payout_per_unit_u,
            quote_expires_at: e.quote_expires_at,
            expires_at: e.expires_at,
            quote_hash: e.quote_hash,
            is_tip: e.is_tip,
            tipster: e.is_tip.then_some(e.user),
            copied_from: e.copied_from,
            opened_at: block_time,
        }
    }

    /// From the account (resync).
    pub fn from_account(key: Pubkey, a: &PendingAccount) -> Self {
        PendingData {
            pending: key,
            user: a.user,
            nonce: a.nonce,
            legs: a.legs.clone(),
            stake_u: a.stake_u,
            copy_fee_u: a.copy_fee_u,
            odds_offered_u: a.odds_offered_u,
            min_payout_per_unit_u: a.min_payout_per_unit_u,
            quote_expires_at: a.quote_expires_at,
            expires_at: a.expires_at,
            quote_hash: a.quote_hash,
            is_tip: a.is_tip,
            tipster: a.tipster,
            copied_from: a.copied_from,
            opened_at: Some(a.created_at),
        }
    }
}

/// A leg named from its hashes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegName {
    /// Pascal symbol.
    pub symbol: String,
    /// Game id.
    pub game_id: String,
    /// Team picked.
    pub pick: TeamSide,
    /// Overlap flag of the quote (or `m1 = 50_000`).
    pub overlaps: bool,
}

/// The quote's own numbers, when the quote store has the row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuoteFacts {
    /// Offered payout at r = 1.
    pub payout_u: Option<u64>,
    /// Fair odds.
    pub odds_fair_u: Option<u64>,
    /// Margin in money.
    pub fee_u: Option<u64>,
}

/// Names a leg list (`(symbol_hash, game_id_hash, side, m1_ppm)` per leg) per L2-1: the quote
/// store row of `quote_hash` first (every hash must match), then the catalogue and the
/// `markets` table. `None` when any leg stays unnamed.
pub async fn name_legs(
    state: &AppState,
    quote_hash: &[u8; 32],
    legs: &[([u8; 32], [u8; 32], u8, u32)],
) -> Result<(Option<Vec<LegName>>, QuoteFacts), sqlx::Error> {
    let row = sqlx::query(
        "SELECT legs_json, payout_u, odds_fair_u, fee_u FROM quotes WHERE quote_hash = ?",
    )
    .bind(hex(quote_hash))
    .fetch_optional(&state.db)
    .await?;
    let mut facts = QuoteFacts::default();
    if let Some(row) = row {
        let get = |c: &str| {
            row.try_get::<Option<i64>, _>(c)
                .ok()
                .flatten()
                .and_then(|v| u64::try_from(v).ok())
        };
        facts = QuoteFacts {
            payout_u: get("payout_u"),
            odds_fair_u: get("odds_fair_u"),
            fee_u: get("fee_u"),
        };
        let parsed: Option<Vec<ApiQuoteLeg>> = row
            .try_get::<String, _>("legs_json")
            .ok()
            .and_then(|j| serde_json::from_str(&j).ok());
        if let Some(q) = parsed
            && q.len() == legs.len()
            && q.iter().zip(legs).all(|(ql, (sh, gh, side, _))| {
                sha256(ql.symbol.as_bytes()) == *sh
                    && sha256(ql.game_id.as_bytes()) == *gh
                    && (ql.side == Side::Short) == (*side == 1)
            })
        {
            let names = q
                .into_iter()
                .map(|ql| LegName {
                    symbol: ql.symbol,
                    game_id: ql.game_id,
                    pick: ql.pick,
                    overlaps: ql.overlaps,
                })
                .collect();
            return Ok((Some(names), facts));
        }
        tracing::warn!(
            quote_hash = %hex(quote_hash),
            "quote store row does not match the on-chain legs; naming from the catalogue"
        );
    }

    // Fallback: hash every known moneyline (catalogue cache, then the markets table, which also
    // keeps markets that left the live list).
    let mut symbols: HashMap<[u8; 32], (String, String, TeamSide)> = HashMap::new();
    if let Some(cat) = state.catalogue() {
        for m in cat.markets.values() {
            symbols.insert(
                sha256(m.symbol.as_bytes()),
                (m.symbol.clone(), m.game_id.clone(), m.long_team),
            );
        }
    }
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT symbol, game_id, long_team FROM markets")
            .fetch_all(&state.db)
            .await?;
    for (symbol, game_id, long_team) in rows {
        let long = if long_team == "home" {
            TeamSide::Home
        } else {
            TeamSide::Away
        };
        symbols
            .entry(sha256(symbol.as_bytes()))
            .or_insert((symbol, game_id, long));
    }
    let mut names = Vec::with_capacity(legs.len());
    for (sh, gh, side, m1) in legs {
        let Some((symbol, game_id, long_team)) = symbols.get(sh) else {
            return Ok((None, facts));
        };
        if sha256(game_id.as_bytes()) != *gh {
            return Ok((None, facts));
        }
        names.push(LegName {
            symbol: symbol.clone(),
            game_id: game_id.clone(),
            pick: if *side == 1 {
                long_team.other()
            } else {
                *long_team
            },
            overlaps: *m1 > 30_000,
        });
    }
    Ok((Some(names), facts))
}

fn side_str(side: u8) -> &'static str {
    if side == 1 { "short" } else { "long" }
}

fn pick_str(p: TeamSide) -> &'static str {
    match p {
        TeamSide::Away => "away",
        TeamSide::Home => "home",
    }
}

fn leg_status_str(s: ChainLegStatus) -> &'static str {
    match s {
        ChainLegStatus::Open => "open",
        ChainLegStatus::Won => "won",
        ChainLegStatus::Lost => "lost",
        ChainLegStatus::Partial => "half",
    }
}

fn api_leg_status(s: ChainLegStatus) -> LegStatus {
    match s {
        ChainLegStatus::Open => LegStatus::Open,
        ChainLegStatus::Won => LegStatus::Won,
        ChainLegStatus::Lost => LegStatus::Lost,
        ChainLegStatus::Partial => LegStatus::Half,
    }
}

fn usdc(u: u64) -> String {
    format!("{}.{:02}", u / 1_000_000, (u % 1_000_000) / 10_000)
}

fn short(id: &str) -> &str {
    id.get(..6).unwrap_or(id)
}

/// `observed − block time`, ms, or `None` when either is unknown (never negative).
pub fn confirm_ms(block_time_s: Option<i64>, observed_ms: Option<u64>) -> Option<u64> {
    let block_ms = u64::try_from(block_time_s?).ok()?.checked_mul(1000)?;
    Some(observed_ms?.saturating_sub(block_ms))
}

/// A short address for the ticker: `7xK9…94Lp` (first and last four characters).
pub fn short_address(wallet: &str) -> String {
    match (
        wallet.get(..4),
        wallet.len().checked_sub(4).and_then(|i| wallet.get(i..)),
    ) {
        (Some(a), Some(b)) if wallet.len() > 8 => format!("{a}…{b}"),
        _ => wallet.to_owned(),
    }
}

/// The ticker's `who` and `team` for a ticket: the owner's handle (else a short address), and the
/// first leg's pick label from the signed quote (else the picked team's name, else the symbol).
pub async fn feed_who_team(
    db: &SqlitePool,
    ticket: &str,
    user: &str,
) -> Result<(String, Option<String>), sqlx::Error> {
    let handle: Option<String> = sqlx::query_scalar("SELECT handle FROM profiles WHERE wallet = ?")
        .bind(user)
        .fetch_optional(db)
        .await?;
    let who = handle.unwrap_or_else(|| short_address(user));
    /// `(symbol, pick, away name, home name, the quote's legs_json)` of the first leg.
    type FirstLeg = (
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let leg: Option<FirstLeg> = sqlx::query_as(
        "SELECT l.symbol, l.pick, m.away, m.home, q.legs_json
               FROM ticket_legs l
               JOIN tickets t ON t.ticket = l.ticket
               LEFT JOIN markets m ON m.symbol = l.symbol
               LEFT JOIN quotes q ON q.quote_hash = t.quote_hash
              WHERE l.ticket = ? ORDER BY l.idx LIMIT 1",
    )
    .bind(ticket)
    .fetch_optional(db)
    .await?;
    let team = leg.map(|(symbol, pick, away, home, legs_json)| {
        let label = legs_json
            .and_then(|j| serde_json::from_str::<Vec<ApiQuoteLeg>>(&j).ok())
            .and_then(|q| q.into_iter().next())
            .filter(|q| q.symbol == symbol)
            .and_then(|q| q.label);
        let name = if pick == "home" { home } else { away };
        label.or(name).unwrap_or(symbol)
    });
    Ok((who, team))
}

/// Inserts a feed row (once per `dedupe` key) and returns the item to emit, if new.
async fn feed(
    db: &SqlitePool,
    kind: FeedKind,
    text: String,
    ticket: &str,
    dedupe: String,
    payload: serde_json::Value,
) -> Result<Option<FeedItem>, sqlx::Error> {
    let ts_ms = now_ms();
    let done = sqlx::query(
        "INSERT OR IGNORE INTO feed_events (kind, text, ticket, payload, ts_ms, dedupe)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(kind.as_str())
    .bind(&text)
    .bind(ticket)
    .bind(payload.to_string())
    .bind(i(ts_ms))
    .bind(dedupe)
    .execute(db)
    .await?;
    Ok((done.rows_affected() == 1).then(|| FeedItem {
        kind,
        text,
        ticket: Some(ticket.to_owned()),
        ts_ms,
        who: None,
        amount_u: None,
        team: None,
        odds_u: None,
    }))
}

/// Whether some ticket's legs are still known only by their chain hashes.
pub async fn has_unnamed(db: &SqlitePool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM tickets WHERE names_resolved = 0)")
        .fetch_one(db)
        .await
}

/// Names the legs of tickets indexed with hashes only (met on chain without their quote, e.g.
/// by a desk on a fresh database; L2-1 left them `sha256:…`) from `markets` (31 Oct: Pascal's
/// resolved list, with each market's JSON). A ticket whose every leg matches a market by symbol
/// and game hash gets their symbols, games and picks; the matched markets are remembered (so
/// the legs show their teams), and each named ticket is announced again. A ticket with a leg
/// still unknown is left as it is. Returns the tickets named.
pub async fn name_unnamed(
    state: &AppState,
    markets: &[(crate::catalogue::CatalogueMarket, serde_json::Value)],
) -> Result<Vec<String>, sqlx::Error> {
    type Leg = (String, i64, String, Option<String>, Option<String>);
    let legs: Vec<Leg> = sqlx::query_as(
        "SELECT l.ticket, l.idx, l.side, l.symbol_hash, l.game_id_hash FROM ticket_legs l
           JOIN tickets t ON t.ticket = l.ticket
          WHERE t.names_resolved = 0 ORDER BY l.ticket, l.idx",
    )
    .fetch_all(&state.db)
    .await?;
    let by_hash: HashMap<String, usize> = markets
        .iter()
        .enumerate()
        .map(|(i, (m, _))| (hex(&sha256(m.symbol.as_bytes())), i))
        .collect();
    // ticket → [(idx, market, pick)], or None once a leg does not match.
    type Plan = Vec<(i64, usize, TeamSide)>;
    let mut plans: Vec<(String, Option<Plan>)> = Vec::new();
    for (ticket, idx, side, sh, gh) in legs {
        if plans.last().is_none_or(|(t, _)| *t != ticket) {
            plans.push((ticket.clone(), Some(Vec::new())));
        }
        let Some((_, Some(plan))) = plans.last_mut() else {
            continue;
        };
        let hit = sh
            .as_ref()
            .and_then(|h| by_hash.get(h))
            .copied()
            .filter(|i| {
                gh.as_deref() == Some(hex(&sha256(markets[*i].0.game_id.as_bytes())).as_str())
            });
        match hit {
            Some(i) => {
                let long = markets[i].0.long_team;
                plan.push((idx, i, if side == "short" { long.other() } else { long }));
            }
            None => plans.last_mut().expect("pushed above").1 = None,
        }
    }
    let plans: Vec<(String, Plan)> = plans
        .into_iter()
        .filter_map(|(t, p)| p.filter(|p| !p.is_empty()).map(|p| (t, p)))
        .collect();
    if plans.is_empty() {
        return Ok(Vec::new());
    }
    let used: Vec<(crate::catalogue::CatalogueMarket, serde_json::Value)> = {
        let mut ids: Vec<usize> = plans
            .iter()
            .flat_map(|(_, p)| p.iter().map(|x| x.1))
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter().map(|i| markets[i].clone()).collect()
    };
    crate::catalogue::remember(state, &used).await?;
    let mut tx = state.db.begin().await?;
    for (ticket, plan) in &plans {
        for (idx, i, pick) in plan {
            let m = &markets[*i].0;
            sqlx::query(
                "UPDATE ticket_legs SET symbol = ?, game_id = ?, pick = ? WHERE ticket = ? AND idx = ?",
            )
            .bind(&m.symbol)
            .bind(&m.game_id)
            .bind(pick_str(*pick))
            .bind(ticket)
            .bind(idx)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("UPDATE tickets SET names_resolved = 1, updated_ms = ? WHERE ticket = ?")
            .bind(i(now_ms()))
            .bind(ticket)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    let named: Vec<String> = plans.into_iter().map(|(t, _)| t).collect();
    for t in &named {
        tracing::info!(ticket = %t, "named a ticket's legs from Pascal's resolved markets");
        emit_ticket(state, t).await?;
    }
    Ok(named)
}

/// Emits the ticket's current API view (`TicketChanged`).
pub async fn emit_ticket(state: &AppState, ticket: &str) -> Result<(), sqlx::Error> {
    if let Some(t) = load_ticket(state, ticket).await? {
        state.emit(DeskEvent::TicketChanged(Arc::new(t)));
    }
    Ok(())
}

/// `open_pending` landed (event) or a live pending was found (resync): inserts the `pending`
/// row and its legs if new, then emits `PendingOpened` (only when every leg is named, L2-1) and
/// `TicketChanged`. A known pending only gets its missing open signature filled in.
pub async fn apply_pending(
    state: &AppState,
    pdas: &Pdas,
    p: &PendingData,
    slot: u64,
    tx: Option<&str>,
) -> Result<(), sqlx::Error> {
    apply_pending_seen(state, pdas, p, slot, tx, None).await
}

/// [`apply_pending`] from a transaction the indexer applied at `observed_ms` (its own clock):
/// with `tx`, also records the slot and `confirm_ms` of `open_pending` (L7-1), before any emit.
pub async fn apply_pending_seen(
    state: &AppState,
    pdas: &Pdas,
    p: &PendingData,
    slot: u64,
    tx: Option<&str>,
    observed_ms: Option<u64>,
) -> Result<(), sqlx::Error> {
    let ticket = pdas.ticket(&p.user, p.nonce).to_string();
    let pending = p.pending.to_string();
    let known: Option<String> = sqlx::query_scalar("SELECT state FROM tickets WHERE ticket = ?")
        .bind(&ticket)
        .fetch_optional(&state.db)
        .await?;
    if known.is_some() {
        if let Some(tx) = tx {
            sqlx::query(
                "UPDATE tickets SET tx_open_pending = ? WHERE ticket = ? AND tx_open_pending IS NULL",
            )
            .bind(tx)
            .bind(&ticket)
            .execute(&state.db)
            .await?;
            record_open_seen(&state.db, &ticket, tx, slot, p.opened_at, observed_ms).await?;
        }
        return Ok(());
    }
    let Some(names) = insert_row(state, pdas, p, slot, tx).await? else {
        return Ok(());
    };
    if let Some(tx) = tx {
        record_open_seen(&state.db, &ticket, tx, slot, p.opened_at, observed_ms).await?;
    }
    match names {
        Some(names) => {
            let legs = p
                .legs
                .iter()
                .zip(names)
                .map(|(l, n)| PendingLeg {
                    symbol: n.symbol,
                    game_id: n.game_id,
                    side: if l.side == 1 { Side::Short } else { Side::Long },
                    start_ms: u64::try_from(l.start_ts).unwrap_or(0).saturating_mul(1000),
                    price_u: l.price_u,
                    m1_ppm: l.m1_ppm,
                })
                .collect();
            state.emit(DeskEvent::PendingOpened(Arc::new(PendingOpened {
                pending,
                ticket: ticket.clone(),
                user: p.user.to_string(),
                nonce: p.nonce,
                stake_u: p.stake_u,
                copy_fee_u: p.copy_fee_u,
                odds_offered_u: p.odds_offered_u,
                min_payout_per_unit_u: p.min_payout_per_unit_u,
                expires_at: p.expires_at,
                legs,
                tx: tx.unwrap_or_default().to_owned(),
                slot,
            })));
        }
        None => tracing::error!(
            %pending,
            quote_hash = %hex(&p.quote_hash),
            "cannot name the legs of a pending from the quote store or the catalogue; \
             not announcing it (it will be refunded after expiry)"
        ),
    }
    emit_ticket(state, &ticket).await
}

/// The slot and confirm time of `open_pending` `tx` (L7-1), once, and only on the row whose
/// open signature it is.
async fn record_open_seen(
    db: &SqlitePool,
    ticket: &str,
    tx: &str,
    slot: u64,
    block_time: Option<i64>,
    observed_ms: Option<u64>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE tickets SET slot_open_pending = COALESCE(slot_open_pending, ?),
            confirm_open_pending_ms = COALESCE(confirm_open_pending_ms, ?)
         WHERE ticket = ? AND tx_open_pending = ?",
    )
    .bind((slot > 0).then(|| i(slot)))
    .bind(confirm_ms(block_time, observed_ms).map(i))
    .bind(ticket)
    .bind(tx)
    .execute(db)
    .await?;
    Ok(())
}

/// `refund_pending` landed (event) or a pending vanished without a ticket (resync).
pub async fn apply_refund(
    state: &AppState,
    pending: &Pubkey,
    slot: u64,
    tx: Option<&str>,
) -> Result<(), sqlx::Error> {
    let row: Option<(String, String, String)> = sqlx::query_as(
        "UPDATE tickets SET state = 'refunded', tx_refund = COALESCE(tx_refund, ?),
            slot = MAX(COALESCE(slot, 0), ?), updated_ms = ?
         WHERE pending = ? AND state = 'pending' RETURNING ticket, user, pending",
    )
    .bind(tx)
    .bind(i(slot))
    .bind(i(now_ms()))
    .bind(pending.to_string())
    .fetch_optional(&state.db)
    .await?;
    match row {
        Some((ticket, user, pending)) => {
            state.emit(DeskEvent::PendingRefunded {
                pending,
                ticket: ticket.clone(),
                user,
                tx: tx.unwrap_or_default().to_owned(),
            });
            emit_ticket(state, &ticket).await
        }
        None => {
            if let Some(tx) = tx {
                sqlx::query(
                    "UPDATE tickets SET tx_refund = ? WHERE pending = ? AND state = 'refunded'
                     AND tx_refund IS NULL",
                )
                .bind(tx)
                .bind(pending.to_string())
                .execute(&state.db)
                .await?;
            }
            Ok(())
        }
    }
}

/// What one transaction did to a ticket, so each transition gets its own signature (a resync
/// passes [`TxFacts::default`]: transitions it finds get no signature).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TxFacts {
    /// The signature.
    pub sig: Option<String>,
    /// It carried `TicketMinted`.
    pub minted: bool,
    /// Legs it carried `LegResolved` for.
    pub resolved: Vec<u8>,
    /// It carried `TicketSettled`.
    pub settled: bool,
    /// It carried `TicketCashedOut` (`c_u`, `ts`).
    pub cashed_out: Option<(u64, i64)>,
    /// Block time.
    pub block_time: Option<i64>,
    /// The transaction's own slot (L7-1; `None` on a resync).
    pub slot: Option<u64>,
    /// The indexer's clock when it applied the transaction, Unix ms (L7-1).
    pub observed_ms: Option<u64>,
}

impl TxFacts {
    fn sig_if(&self, yes: bool) -> Option<&str> {
        if yes { self.sig.as_deref() } else { None }
    }

    /// The transaction's slot, when it did the transition (`yes`) and is known.
    fn slot_if(&self, yes: bool) -> Option<i64> {
        self.sig_if(yes)?;
        self.slot.filter(|s| *s > 0).map(i)
    }

    /// Its confirm time, when it did the transition and both clocks are known.
    fn confirm_if(&self, yes: bool) -> Option<i64> {
        self.sig_if(yes)?;
        confirm_ms(self.block_time, self.observed_ms).map(i)
    }
}

/// A `Ticket` account as read from the chain: brings its DB row up to date and emits one bus
/// event per transition the DB did not have yet, then `TicketChanged`.
pub async fn apply_ticket(
    state: &AppState,
    pdas: &Pdas,
    key: &Pubkey,
    t: &TicketAccount,
    slot: u64,
    facts: &TxFacts,
) -> Result<(), sqlx::Error> {
    let ticket = key.to_string();
    let user = t.user.to_string();
    let pending = pdas.pending(&t.user, t.nonce).to_string();
    let row: Option<String> = sqlx::query_scalar("SELECT state FROM tickets WHERE ticket = ?")
        .bind(&ticket)
        .fetch_optional(&state.db)
        .await?;
    let now = i(now_ms());
    let mut events: Vec<DeskEvent> = Vec::new();
    let mut feeds: Vec<FeedItem> = Vec::new();

    let db_state = match row {
        Some(s) => s,
        None => {
            // The DB never saw the pending (fresh DB, or open_pending and finalize both landed
            // while the listener was away): create the row from the account.
            insert_from_account(state, pdas, key, t, slot).await?;
            "pending".to_owned()
        }
    };

    // pending → open.
    if db_state == "pending" || db_state == "refunded" {
        let mut dbtx = state.db.begin().await?;
        let changed = sqlx::query(
            "UPDATE tickets SET state = 'open', rs_u = ?, r_ppm = ?, payout_u = ?, odds_u = ?,
                chain_fee_u = ?, refund_u = ?, minted_at = ?, tipster = ?, copied_from = ?,
                tx_finalize = COALESCE(tx_finalize, ?), slot = MAX(COALESCE(slot, 0), ?),
                updated_ms = ?
             WHERE ticket = ? AND state IN ('pending', 'refunded')",
        )
        .bind(i(t.stake_u))
        .bind(i64::from(t.r_ppm))
        .bind(i(t.payout_u))
        .bind(i(t.odds_u))
        .bind(i(t.fee_u))
        .bind(i(t.refund_u))
        .bind(t.minted_at)
        .bind(t.tipster.map(|k| k.to_string()))
        .bind(t.copied_from.map(|k| k.to_string()))
        .bind(facts.sig_if(facts.minted))
        .bind(i(slot))
        .bind(now)
        .bind(&ticket)
        .execute(&mut *dbtx)
        .await?
        .rows_affected()
            == 1;
        for (idx, l) in t.legs.iter().enumerate() {
            sqlx::query(
                "UPDATE ticket_legs SET p_fill_u = ?, hedged = ? WHERE ticket = ? AND idx = ?",
            )
            .bind(i(l.fill_u))
            .bind(i64::from(l.hedged))
            .bind(&ticket)
            .bind(i64::try_from(idx).unwrap_or(0))
            .execute(&mut *dbtx)
            .await?;
        }
        dbtx.commit().await?;
        if changed {
            let n = t.legs.len();
            // L7-1: "<who> placed <amount> on <team> @ <odds>"; a minted copy is kind `copy`.
            // Display only: a failed read never blocks the transition (the row is committed).
            let (who, team) = feed_who_team(&state.db, &ticket, &user)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, ticket, "feed: cannot read who and team");
                    (short_address(&user), None)
                });
            let (kind, text) = match t.copied_from {
                Some(of) => (
                    FeedKind::Copy,
                    format!(
                        "Copy of {} minted: {n} legs, {} USDC to win {} USDC",
                        short(&of.to_string()),
                        usdc(t.stake_u),
                        usdc(t.payout_u)
                    ),
                ),
                None => (
                    FeedKind::Mint,
                    format!(
                        "Schedina {} minted: {n} legs, {} USDC to win {} USDC",
                        short(&ticket),
                        usdc(t.stake_u),
                        usdc(t.payout_u)
                    ),
                ),
            };
            if let Some(f) = feed(
                &state.db,
                kind,
                text,
                &ticket,
                format!("mint:{ticket}"),
                json!({"ticket": ticket, "stake_u": t.stake_u, "payout_u": t.payout_u,
                    "r_ppm": t.r_ppm, "who": who, "team": team, "odds_u": t.odds_u,
                    "copied_from": t.copied_from.map(|k| k.to_string())}),
            )
            .await?
            {
                feeds.push(FeedItem {
                    who: Some(who),
                    amount_u: Some(t.stake_u),
                    team,
                    odds_u: Some(t.odds_u),
                    ..f
                });
            }
            events.push(DeskEvent::TicketMinted {
                pending: pending.clone(),
                ticket: ticket.clone(),
                user: user.clone(),
                tx: facts.sig_if(facts.minted).unwrap_or_default().to_owned(),
            });
        }
    } else if facts.minted {
        backfill(&state.db, "tx_finalize", &ticket, facts.sig.as_deref()).await?;
    }

    // Legs open → resolved.
    for (idx, l) in t.legs.iter().enumerate() {
        if l.status == ChainLegStatus::Open {
            continue;
        }
        let idx8 = u8::try_from(idx).unwrap_or(u8::MAX);
        let mine = facts.resolved.contains(&idx8);
        let symbol: Option<String> = sqlx::query_scalar(
            "UPDATE ticket_legs SET status = ?, resolution_u = ?, resolved_ms = ?,
                tx_resolve = COALESCE(tx_resolve, ?)
             WHERE ticket = ? AND idx = ? AND status = 'open' RETURNING symbol",
        )
        .bind(leg_status_str(l.status))
        .bind(i64::from(l.resolution_ppm))
        .bind(facts.block_time.map_or(now, |s| s.saturating_mul(1000)))
        .bind(facts.sig_if(mine))
        .bind(&ticket)
        .bind(i64::from(idx8))
        .fetch_optional(&state.db)
        .await?;
        match symbol {
            Some(symbol) => {
                let word = match l.status {
                    ChainLegStatus::Won => "won",
                    ChainLegStatus::Lost => "lost",
                    _ => "half-void",
                };
                if let Some(f) = feed(
                    &state.db,
                    FeedKind::Resolve,
                    format!("{symbol}: leg {} of {} {word}", idx + 1, short(&ticket)),
                    &ticket,
                    format!("resolve:{ticket}:{idx}"),
                    json!({"ticket": ticket, "leg_idx": idx, "r_ppm": l.resolution_ppm}),
                )
                .await?
                {
                    feeds.push(f);
                }
                events.push(DeskEvent::LegResolved {
                    ticket: ticket.clone(),
                    user: user.clone(),
                    leg_idx: idx8,
                    symbol,
                    resolution_u: u64::from(l.resolution_ppm),
                    status: api_leg_status(l.status),
                    tx: facts.sig_if(mine).unwrap_or_default().to_owned(),
                });
            }
            None if mine => {
                sqlx::query(
                    "UPDATE ticket_legs SET tx_resolve = ? WHERE ticket = ? AND idx = ?
                     AND tx_resolve IS NULL",
                )
                .bind(facts.sig.as_deref())
                .bind(&ticket)
                .bind(i64::from(idx8))
                .execute(&state.db)
                .await?;
            }
            None => {}
        }
    }

    // open → settled | cashed_out.
    match t.state {
        TicketState::Open => {}
        TicketState::Settled => {
            let changed = sqlx::query(
                "UPDATE tickets SET state = 'settled', paid_u = ?, settled_at = ?,
                    tx_settle = COALESCE(tx_settle, ?), slot = MAX(COALESCE(slot, 0), ?),
                    slot_settle = COALESCE(slot_settle, ?), updated_ms = ?
                 WHERE ticket = ? AND state = 'open'",
            )
            .bind(i(t.paid_u))
            .bind(t.resolved_at)
            .bind(facts.sig_if(facts.settled))
            .bind(i(slot))
            .bind(facts.slot_if(facts.settled))
            .bind(now)
            .bind(&ticket)
            .execute(&state.db)
            .await?
            .rows_affected()
                == 1;
            if changed {
                if let Some(f) = feed(
                    &state.db,
                    FeedKind::Settle,
                    format!(
                        "Schedina {} settled: paid {} USDC",
                        short(&ticket),
                        usdc(t.paid_u)
                    ),
                    &ticket,
                    format!("settle:{ticket}"),
                    json!({"ticket": ticket, "paid_u": t.paid_u, "payout_u": t.payout_u}),
                )
                .await?
                {
                    feeds.push(f);
                }
                events.push(DeskEvent::TicketSettled {
                    ticket: ticket.clone(),
                    user: user.clone(),
                    paid_u: t.paid_u,
                    tx: facts.sig_if(facts.settled).unwrap_or_default().to_owned(),
                });
            } else if facts.settled {
                backfill(&state.db, "tx_settle", &ticket, facts.sig.as_deref()).await?;
                record_seen(&state.db, &ticket, "tx_settle", facts, None).await?;
            }
        }
        TicketState::CashedOut => {
            let mut dbtx = state.db.begin().await?;
            let changed = sqlx::query(
                "UPDATE tickets SET state = 'cashed_out', paid_u = ?, settled_at = ?,
                    tx_cash_out = COALESCE(tx_cash_out, ?), slot = MAX(COALESCE(slot, 0), ?),
                    slot_cash_out = COALESCE(slot_cash_out, ?),
                    confirm_cash_out_ms = COALESCE(confirm_cash_out_ms, ?), updated_ms = ?
                 WHERE ticket = ? AND state = 'open'",
            )
            .bind(i(t.paid_u))
            .bind(t.resolved_at)
            .bind(facts.sig_if(facts.cashed_out.is_some()))
            .bind(i(slot))
            .bind(facts.slot_if(facts.cashed_out.is_some()))
            .bind(facts.confirm_if(facts.cashed_out.is_some()))
            .bind(now)
            .bind(&ticket)
            .execute(&mut *dbtx)
            .await?
            .rows_affected()
                == 1;
            let unresolved = t
                .legs
                .iter()
                .filter(|l| l.status == ChainLegStatus::Open)
                .count();
            sqlx::query(
                "INSERT INTO cashouts (ticket, c_u, unresolved_legs, ts, tx, updated_ms)
                 VALUES (?, ?, ?, ?, ?, ?)
                 ON CONFLICT (ticket) DO UPDATE SET tx = COALESCE(cashouts.tx, excluded.tx),
                    updated_ms = excluded.updated_ms",
            )
            .bind(&ticket)
            .bind(i(t.paid_u))
            .bind(i64::try_from(unresolved).unwrap_or(0))
            .bind(facts.cashed_out.map_or(t.resolved_at, |(_, ts)| ts))
            .bind(facts.sig_if(facts.cashed_out.is_some()))
            .bind(now)
            .execute(&mut *dbtx)
            .await?;
            dbtx.commit().await?;
            if changed {
                if let Some(f) = feed(
                    &state.db,
                    FeedKind::Cashout,
                    format!(
                        "Schedina {} cashed out: {} USDC",
                        short(&ticket),
                        usdc(t.paid_u)
                    ),
                    &ticket,
                    format!("cashout:{ticket}"),
                    json!({"ticket": ticket, "c_u": t.paid_u, "payout_u": t.payout_u}),
                )
                .await?
                {
                    feeds.push(f);
                }
                events.push(DeskEvent::TicketCashedOut {
                    ticket: ticket.clone(),
                    user: user.clone(),
                    c_u: t.paid_u,
                    tx: facts
                        .sig_if(facts.cashed_out.is_some())
                        .unwrap_or_default()
                        .to_owned(),
                });
            } else if facts.cashed_out.is_some() {
                backfill(&state.db, "tx_cash_out", &ticket, facts.sig.as_deref()).await?;
                record_seen(
                    &state.db,
                    &ticket,
                    "tx_cash_out",
                    facts,
                    Some("confirm_cash_out_ms"),
                )
                .await?;
            }
        }
    }

    // 4.A: a copy's charged fee and its derived `TipsterPaid` row (resync), before any emit.
    crate::tipsters::store::on_ticket(&state.db, key, t).await?;

    let changed = !events.is_empty();
    for e in events {
        state.emit(e);
    }
    for f in feeds {
        state.emit(DeskEvent::Feed(Arc::new(f)));
    }
    if changed {
        emit_ticket(state, &ticket).await?;
    }
    Ok(())
}

/// A transition the DB already had, seen again in its own transaction (a resync applied it
/// first): its slot (and confirm time) once, on the row whose `sig_column` is this signature.
async fn record_seen(
    db: &SqlitePool,
    ticket: &str,
    sig_column: &'static str,
    facts: &TxFacts,
    confirm_column: Option<&'static str>,
) -> Result<(), sqlx::Error> {
    let Some(sig) = facts.sig.as_deref() else {
        return Ok(());
    };
    // Every column name is one of a few literals, never input.
    let slot_column = match sig_column {
        "tx_settle" => "slot_settle",
        _ => "slot_cash_out",
    };
    let confirm = confirm_column.map_or_else(String::new, |c| format!(", {c} = COALESCE({c}, ?)"));
    let sql = format!(
        "UPDATE tickets SET {slot_column} = COALESCE({slot_column}, ?){confirm}
         WHERE ticket = ? AND {sig_column} = ?"
    );
    let mut q = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(facts.slot_if(true));
    if confirm_column.is_some() {
        q = q.bind(facts.confirm_if(true));
    }
    q.bind(ticket).bind(sig).execute(db).await?;
    Ok(())
}

async fn backfill(
    db: &SqlitePool,
    column: &'static str,
    ticket: &str,
    sig: Option<&str>,
) -> Result<(), sqlx::Error> {
    let Some(sig) = sig else { return Ok(()) };
    // `column` is one of three literals, never input.
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE tickets SET {column} = ? WHERE ticket = ? AND {column} IS NULL"
    )))
    .bind(sig)
    .bind(ticket)
    .execute(db)
    .await?;
    Ok(())
}

/// Creates the `pending` row of a ticket the DB never saw, from the `Ticket` account (stake
/// `S = rS + refund`); [`apply_ticket`] then moves it on. Legs are named per L2-1.
async fn insert_from_account(
    state: &AppState,
    pdas: &Pdas,
    key: &Pubkey,
    t: &TicketAccount,
    slot: u64,
) -> Result<(), sqlx::Error> {
    let legs = t
        .legs
        .iter()
        .map(|l| program::QuoteLeg {
            symbol_hash: l.symbol_hash,
            game_id_hash: l.game_id_hash,
            start_ts: l.start_ts,
            side: l.side,
            price_u: l.price_u,
            m1_ppm: l.m1_ppm,
        })
        .collect();
    let p = PendingData {
        pending: pdas.pending(&t.user, t.nonce),
        user: t.user,
        nonce: t.nonce,
        legs,
        stake_u: t.stake_u.saturating_add(t.refund_u),
        copy_fee_u: t.copy_fee_u,
        odds_offered_u: t.odds_offered_u,
        min_payout_per_unit_u: t.min_payout_per_unit_u,
        quote_expires_at: 0,
        expires_at: 0,
        quote_hash: t.quote_hash,
        is_tip: t.tipster == Some(t.user) && t.copied_from.is_none(),
        tipster: t.tipster,
        copied_from: t.copied_from,
        opened_at: Some(t.minted_at),
    };
    debug_assert_eq!(pdas.ticket(&t.user, t.nonce), *key);
    // Inserted without announcing it: it is already a ticket, and `apply_ticket` emits the
    // mint. Unnamed legs keep their hashes (logged).
    if let Some(None) = insert_row(state, pdas, &p, slot, None).await? {
        tracing::error!(ticket = %key, "cannot name the legs of a ticket; indexed with hashes");
    }
    Ok(())
}

/// Inserts the `pending` row and its legs. `None` when the row already existed, else
/// `Some(names)` (`names` is `None` when a leg could not be named, L2-1).
async fn insert_row(
    state: &AppState,
    pdas: &Pdas,
    p: &PendingData,
    slot: u64,
    tx: Option<&str>,
) -> Result<Option<Option<Vec<LegName>>>, sqlx::Error> {
    let ticket = pdas.ticket(&p.user, p.nonce).to_string();
    let hashes: Vec<_> = p
        .legs
        .iter()
        .map(|l| (l.symbol_hash, l.game_id_hash, l.side, l.m1_ppm))
        .collect();
    let (names, facts) = name_legs(state, &p.quote_hash, &hashes).await?;
    // The offered payout at r = 1 (C2 with f = p), unless the quote store has it.
    let payout_u = facts.payout_u.or_else(|| {
        let legs: Vec<pricing::PayoutLeg> = p
            .legs
            .iter()
            .map(|l| pricing::PayoutLeg {
                m1_ppm: l.m1_ppm,
                f_u: l.price_u,
            })
            .collect();
        pricing::payout(p.stake_u, 1_000_000, &legs)
            .ok()
            .map(|x| x.payout_u)
    });
    let mut dbtx = state.db.begin().await?;
    let inserted = sqlx::query(
        "INSERT INTO tickets (ticket, pending, user, nonce, state, n, stake_u, payout_u,
            odds_offered_u, odds_fair_u, fee_u, min_payout_per_unit_u, copy_fee_u, tipster,
            copied_from, quote_expires_at, expires_at, opened_at, tx_open_pending, slot,
            updated_ms, quote_hash, is_tip, names_resolved)
         VALUES (?, ?, ?, ?, 'pending', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT DO NOTHING",
    )
    .bind(&ticket)
    .bind(p.pending.to_string())
    .bind(p.user.to_string())
    .bind(crate::db::nonce_to_db(p.nonce))
    .bind(i64::try_from(p.legs.len()).unwrap_or(0))
    .bind(i(p.stake_u))
    .bind(i(payout_u.unwrap_or(0)))
    .bind(i(p.odds_offered_u))
    .bind(facts.odds_fair_u.map(i))
    .bind(facts.fee_u.map(i))
    .bind(i(p.min_payout_per_unit_u))
    .bind(i(p.copy_fee_u))
    .bind(p.tipster.map(|k| k.to_string()))
    .bind(p.copied_from.map(|k| k.to_string()))
    // 0 = unknown (a row created from a Ticket account): NULL, so no timer looks at it.
    .bind((p.quote_expires_at != 0).then_some(p.quote_expires_at))
    .bind((p.expires_at != 0).then_some(p.expires_at))
    .bind(p.opened_at)
    .bind(tx)
    .bind(i(slot))
    .bind(i(now_ms()))
    .bind(hex(&p.quote_hash))
    .bind(i64::from(p.is_tip))
    .bind(i64::from(names.is_some()))
    .execute(&mut *dbtx)
    .await?
    .rows_affected()
        == 1;
    if !inserted {
        dbtx.rollback().await?;
        return Ok(None);
    }
    for (idx, l) in p.legs.iter().enumerate() {
        let name = names.as_ref().and_then(|n| n.get(idx));
        let hashed = |h: &[u8; 32]| format!("sha256:{}", hex(h));
        // Without a name, D-021's default: the moneyline symbol is long the away team.
        let pick = name.map_or(
            if l.side == 1 {
                TeamSide::Home
            } else {
                TeamSide::Away
            },
            |n| n.pick,
        );
        sqlx::query(
            "INSERT INTO ticket_legs (ticket, idx, symbol, game_id, side, pick, start_ms,
                p_quote_u, m1_ppm, overlaps, symbol_hash, game_id_hash)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
        )
        .bind(&ticket)
        .bind(i64::try_from(idx).unwrap_or(0))
        .bind(name.map_or_else(|| hashed(&l.symbol_hash), |n| n.symbol.clone()))
        .bind(name.map_or_else(|| hashed(&l.game_id_hash), |n| n.game_id.clone()))
        .bind(side_str(l.side))
        .bind(pick_str(pick))
        .bind(l.start_ts.saturating_mul(1000))
        .bind(i(l.price_u))
        .bind(i64::from(l.m1_ppm))
        .bind(i64::from(name.is_some_and(|n| n.overlaps)))
        .bind(hex(&l.symbol_hash))
        .bind(hex(&l.game_id_hash))
        .execute(&mut *dbtx)
        .await?;
    }
    // An own tip named in the drawer (L9): the title its wallet sent with this quote. The
    // program accepted the tip only before every game, so the title rule holds.
    if p.is_tip && p.copied_from.is_none() && p.tipster == Some(p.user) {
        sqlx::query(
            "INSERT INTO tip_titles (ticket, wallet, title, set_at)
             SELECT ?, wallet, title, ? FROM quote_titles WHERE quote_hash = ? AND wallet = ?
             ON CONFLICT (ticket) DO NOTHING",
        )
        .bind(&ticket)
        .bind(i64::try_from(now_ms() / 1000).unwrap_or(i64::MAX))
        .bind(hex(&p.quote_hash))
        .bind(p.user.to_string())
        .execute(&mut *dbtx)
        .await?;
    }
    dbtx.commit().await?;
    Ok(Some(names))
}

/// Pendings the DB holds as `pending`, with the slot of their last change: resync marks those
/// absent from the chain as refunded.
pub async fn pending_rows(db: &SqlitePool) -> Result<Vec<(String, u64)>, sqlx::Error> {
    let rows: Vec<(String, Option<i64>)> =
        sqlx::query_as("SELECT pending, slot FROM tickets WHERE state = 'pending'")
            .fetch_all(db)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(p, s)| (p, s.and_then(|s| u64::try_from(s).ok()).unwrap_or(0)))
        .collect())
}
