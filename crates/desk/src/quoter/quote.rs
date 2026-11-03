//! `POST /quote` and `POST /ticket/{id}/copy-quote` (C4), and the pure pieces they are built
//! from: catalogue market → [`pricing::LegInput`], [`pricing::Quote`] → signed
//! [`QuoteResponse`], and the L2-1 quote store.

use std::collections::BTreeSet;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use base64::Engine;
use pricing::{LegInput, RejectStatus, Rejection, Teams, TickSpec};
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;
use sqlx::SqlitePool;

use super::books::{books_for, to_i64};
use super::codec::{self, QuoteMsg, QuoteMsgLeg};
use super::{COPY_FEE_PPM, QUOTE_TTL_S};
use crate::api::ApiError;
use crate::api::types::{
    MarketKind, QuoteLeg, QuoteLegRequest, QuoteRequest, QuoteResponse, Side, TeamSide,
};
use crate::catalogue::{Catalogue, CatalogueMarket};
use crate::now_ms;
use crate::solana::signer::Keypair;
use crate::state::AppState;

/// `POST /ticket/{id}/copy-quote` body (spec section 5: `{stake_usdc}`, C1 units).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyQuoteRequest {
    /// Stake, micro-USDC.
    pub stake_u: u64,
}

/// `POST /quote`.
pub async fn post_quote(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<QuoteResponse>, ApiError> {
    let req: QuoteRequest = serde_json::from_slice(&body)
        .map_err(|e| ApiError::rejected(format!("invalid quote request: {e}")))?;
    let r = async {
        if req.token_in.is_some() {
            return Err(ApiError::rejected(
                "token_in is not supported yet: stake in USDC",
            ));
        }
        let title = tip_title(&req)?;
        if let Some(orig) = &req.copied_from {
            copies_enabled(&state)?;
            let legs = copy_source(&state, orig).await?;
            if legs != req.legs {
                return Err(ApiError::rejected(format!(
                    "the legs differ from those of the copied ticket {orig}"
                )));
            }
        }
        let r = quote(&state, &req.legs, req.stake_u, req.copied_from.as_deref()).await?;
        if let Some((wallet, title)) = title {
            store_title(&state.db, &r, &wallet, &title).await?;
        }
        Ok(r)
    }
    .await;
    count(&r);
    r.map(Json)
}

/// `POST /ticket/{id}/copy-quote` (on unless `DESK_COPY_QUOTE` turns it off, D-177): the
/// ticket's legs, quoted fresh for the caller's stake, with `copied_from` and the copy fee.
/// Refused up front (422) unless the ticket can still be copied ([`copy_refusal`]).
pub async fn post_copy_quote(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<QuoteResponse>, ApiError> {
    let r = async {
        copies_enabled(&state)?;
        let req: CopyQuoteRequest = serde_json::from_slice(&body)
            .map_err(|e| ApiError::rejected(format!("invalid copy-quote request: {e}")))?;
        let legs = copy_source(&state, &id).await?;
        quote(&state, &legs, req.stake_u, Some(&id)).await
    }
    .await;
    count(&r);
    r.map(Json)
}

fn count(r: &Result<QuoteResponse, ApiError>) {
    let result = match r {
        Ok(_) => "ok",
        Err(e) => match e.0.status {
            crate::api::types::ErrorStatus::Rejected => "rejected",
            crate::api::types::ErrorStatus::Unavailable => "unavailable",
            crate::api::types::ErrorStatus::NotFound => "not_found",
        },
    };
    metrics::counter!("desk_quotes_total", "result" => result).increment(1);
}

fn copies_enabled(state: &AppState) -> Result<(), ApiError> {
    if state.config.copy_quote {
        Ok(())
    } else {
        Err(ApiError::rejected("copying tickets is not enabled yet"))
    }
}

/// What the copy pre-check reads of the original ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopySource {
    /// `tickets.state`: `pending`, `open`, `settled`, `cashed_out` or `refunded`.
    pub state: String,
    /// The ticket's owner.
    pub user: String,
    /// `Ticket.tipster` (the user for an own tip).
    pub tipster: Option<String>,
    /// `Ticket.copied_from` (set on a copy).
    pub copied_from: Option<String>,
    /// The legs in ticket order.
    pub legs: Vec<CopySourceLeg>,
}

/// One leg of a [`CopySource`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopySourceLeg {
    /// Pascal symbol.
    pub symbol: String,
    /// `ticket_legs.side`: `long` or `short`.
    pub side: String,
    /// `ticket_legs.status`: `open` until `resolve_leg`.
    pub status: String,
}

/// Why `t` cannot be copied, or `None` when it can. A copy must follow a minted, open own tip
/// (`tipster == user`, not itself a copy) whose legs are all unresolved; a leg in play is fine
/// (D-660: pricing decides whether it is still on offer, at live prices). `finalize` refuses
/// anything else (L2-4), so refusing here saves the copier's escrow and a hedge-then-unwind.
pub fn copy_refusal(ticket: &str, t: &CopySource) -> Option<String> {
    match t.state.as_str() {
        "open" => {}
        "pending" => return Some(format!("ticket {ticket} is not minted yet")),
        "cashed_out" => return Some(format!("ticket {ticket} was cashed out")),
        other => return Some(format!("ticket {ticket} is {other}, no longer open")),
    }
    if t.copied_from.is_some() {
        return Some(format!(
            "ticket {ticket} is itself a copy: copy the tipster's original"
        ));
    }
    if t.tipster.as_deref() != Some(t.user.as_str()) {
        return Some(format!(
            "ticket {ticket} is not a tip: only shared tips can be copied"
        ));
    }
    t.legs
        .iter()
        .any(|l| l.status != "open")
        .then(|| format!("ticket {ticket} has a leg already decided"))
}

/// The legs of `ticket` for a copy, after the copy pre-check (404 for an unknown ticket, 422 with
/// the reason for one that cannot be copied).
async fn copy_source(state: &AppState, ticket: &str) -> Result<Vec<QuoteLegRequest>, ApiError> {
    let src = read_copy_source(&state.db, ticket).await?;
    if let Some(reason) = copy_refusal(ticket, &src) {
        return Err(ApiError::rejected(reason));
    }
    src.legs
        .into_iter()
        .map(|l| {
            let side = match l.side.as_str() {
                "long" => Side::Long,
                "short" => Side::Short,
                other => {
                    tracing::error!(ticket, side = other, "bad side in ticket_legs");
                    return Err(ApiError::unavailable("ticket record is unreadable"));
                }
            };
            Ok(QuoteLegRequest {
                symbol: l.symbol,
                side,
            })
        })
        .collect()
}

/// The original ticket and its legs as on record (404 when there is no ticket or no leg).
async fn read_copy_source(db: &SqlitePool, ticket: &str) -> Result<CopySource, ApiError> {
    let not_found = || ApiError::not_found(format!("unknown ticket {ticket}"));
    let row: Option<(String, String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT state, user, tipster, copied_from FROM tickets WHERE ticket = ?")
            .bind(ticket)
            .fetch_optional(db)
            .await?;
    let (state, user, tipster, copied_from) = row.ok_or_else(not_found)?;
    let legs: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT symbol, side, status FROM ticket_legs WHERE ticket = ? ORDER BY idx",
    )
    .bind(ticket)
    .fetch_all(db)
    .await?;
    if legs.is_empty() {
        return Err(not_found());
    }
    Ok(CopySource {
        state,
        user,
        tipster,
        copied_from,
        legs: legs
            .into_iter()
            .map(|(symbol, side, status)| CopySourceLeg {
                symbol,
                side,
                status,
            })
            .collect(),
    })
}

/// The quote pipeline: catalogue → correlation rule → books → `pricing::quote` → sign →
/// store → response.
pub async fn quote(
    state: &AppState,
    legs: &[QuoteLegRequest],
    stake_u: u64,
    copied_from: Option<&str>,
) -> Result<QuoteResponse, ApiError> {
    if state.is_paused() {
        return Err(ApiError::unavailable(
            "the desk is paused: no new tickets right now",
        ));
    }
    let signer = state.quoter.signer(&state.config).map_err(|e| {
        tracing::error!(error = %e, "quote signer unavailable");
        ApiError::unavailable("the desk cannot sign quotes right now")
    })?;
    let cat = state
        .catalogue()
        .ok_or_else(|| ApiError::unavailable("the market list is not loaded yet"))?;
    // Tick and fee parameters come from the catalogue (D-009): refuse to price on a list as
    // stale as `/card` refuses to show.
    let cat_age_ms = now_ms().saturating_sub(cat.fetched_ms);
    if u128::from(cat_age_ms) > state.config.card_max_age.as_millis() {
        return Err(ApiError::unavailable(format!(
            "the market list is {} s old",
            cat_age_ms / 1000
        )));
    }
    let params = state.quoter.params;
    let inputs = legs
        .iter()
        .map(|l| leg_input(&cat, &l.symbol, l.side))
        .collect::<Result<Vec<_>, _>>()?;
    // A game the live feed reports over is no longer bet on, whatever its book says (31 Oct).
    if let Some(l) = inputs.iter().find(|l| state.scores.ended(&l.game_id)) {
        return Err(ApiError::rejected(format!(
            "game {} has finished: it can no longer be bet on",
            l.game_id
        )));
    }
    // Shape and correlation first, on the catalogue's venue clock, so a refused ticket costs no
    // Pascal call; `pricing::quote` checks again on the books' clock.
    let cat_now = cat.as_of_ms.saturating_add(cat_age_ms);
    pricing::check_correlation(&inputs, stake_u, cat_now, &params.caps).map_err(reject)?;

    let symbols: BTreeSet<String> = inputs.iter().map(|l| l.symbol.clone()).collect();
    let lb = books_for(state, &symbols)
        .await
        .map_err(|e| ApiError::unavailable(e.0))?;
    let venue_now = if lb.venue_now_ms > 0 {
        lb.venue_now_ms
    } else {
        cat_now
    };
    // An in-play price moves by the second: never price one from the fallback snapshot.
    if let Some(l) = inputs.iter().find(|l| l.start_ms <= venue_now)
        && lb.max_age_ms > super::books::FRESH_MS
    {
        return Err(ApiError::unavailable(format!(
            "live prices for game {} are not fresh enough right now; try again in a few seconds",
            l.game_id
        )));
    }
    let q = pricing::quote(&inputs, &lb.books, stake_u, venue_now, &params).map_err(reject)?;

    let copy_fee_u = copied_from.map_or(0, |_| copy_fee_u(stake_u));
    let expires = now_s().saturating_add(QUOTE_TTL_S);
    let mut signed = sign_quote(&q, copy_fee_u, expires, &state.config.program_id, &signer);
    describe_legs(&cat, &mut signed.response.legs);
    store_quote(&state.db, &signed, copied_from).await?;
    state.quoter.note_quoted(&symbols, now_ms());
    tracing::info!(
        quote_hash = %signed.quote_hash,
        n = q.n,
        stake_u,
        payout_u = q.payout_u,
        odds_offered_u = q.odds_offered_u,
        book_age_ms = lb.max_age_ms,
        "quote signed"
    );
    Ok(signed.response)
}

/// A C4 rejection from a pricing one: 422 `rejected` or 503 `unavailable`, with its reason.
pub fn reject(r: Rejection) -> ApiError {
    match r.status() {
        RejectStatus::Rejected => ApiError::rejected(r.reason),
        RejectStatus::Unavailable => ApiError::unavailable(r.reason),
    }
}

/// Copy fee: 2% of the stake, rounded up (house side, C1).
pub fn copy_fee_u(stake_u: u64) -> u64 {
    let fee = (u128::from(stake_u) * u128::from(COPY_FEE_PPM)).div_ceil(1_000_000);
    u64::try_from(fee).unwrap_or(u64::MAX)
}

/// The desk's wall clock, Unix s (the program's `Clock` checks TTLs against it).
pub fn now_s() -> i64 {
    to_i64(now_ms() / 1000)
}

/// The API side as pricing's.
pub fn pricing_side(side: Side) -> pricing::Side {
    match side {
        Side::Long => pricing::Side::Long,
        Side::Short => pricing::Side::Short,
    }
}

fn api_side(side: pricing::Side) -> Side {
    match side {
        pricing::Side::Long => Side::Long,
        pricing::Side::Short => Side::Short,
    }
}

fn pick(t: TeamSide) -> pricing::Pick {
    match t {
        TeamSide::Away => pricing::Pick::Away,
        TeamSide::Home => pricing::Pick::Home,
    }
}

fn team_side(p: pricing::Pick) -> TeamSide {
    match p {
        pricing::Pick::Away => TeamSide::Away,
        pricing::Pick::Home => TeamSide::Home,
    }
}

/// The pricing input for `side` of the catalogue's market `symbol` (422 when the symbol is not
/// an NBA winner, spread or total, or a soccer 1-X-2 outcome, in the catalogue).
pub fn leg_input(cat: &Catalogue, symbol: &str, side: Side) -> Result<LegInput, ApiError> {
    let m = cat.market(symbol).ok_or_else(|| {
        ApiError::rejected(format!(
            "{symbol} is not on the card (only NBA winner, spread and total markets and soccer 1X2 results can be played)"
        ))
    })?;
    Ok(leg_input_of(m, side))
}

/// [`leg_input`] for a market already looked up. Catalogue markets are NBA two-way winners,
/// half-point spreads and totals (D-008, 6.0b) and soccer 1-X-2 outcomes (7.0b) by
/// construction. A soccer outcome is a [`pricing::LegMarket::ThreeWay`] (long only; its draw's
/// `long_pick` is the home placeholder, proposed D-461), and its team abbreviations are
/// prefixed with the league (`EPL:AVL`) so the same-day team rule never matches a soccer club
/// with an NBA team of the same three letters.
pub fn leg_input_of(m: &CatalogueMarket, side: Side) -> LegInput {
    let three_way = m.is_three_way();
    let abbr = |a: &str| {
        if three_way && !a.is_empty() {
            format!("{}:{a}", m.league.as_str())
        } else {
            a.to_owned()
        }
    };
    LegInput {
        symbol: m.symbol.clone(),
        side: pricing_side(side),
        game_id: m.game_id.clone(),
        start_ms: m.start_ms,
        market: match m.kind {
            MarketKind::Winner if three_way => pricing::LegMarket::ThreeWay,
            MarketKind::Winner => pricing::LegMarket::Winner,
            MarketKind::Spread => pricing::LegMarket::Spread,
            MarketKind::Total => pricing::LegMarket::Total,
        },
        long_pick: pick(m.long_team),
        teams: Teams {
            away_subject_id: m.away.subject_id.clone(),
            home_subject_id: m.home.subject_id.clone(),
            away_abbr: abbr(&m.away.abbr),
            home_abbr: abbr(&m.home.abbr),
        },
        tick: TickSpec {
            tick_size_min_u: u64::try_from(m.tick_size_min_u).unwrap_or(1).max(1),
            tick_sig_figs: m.tick_sig_figs,
        },
        taker_fee_rate_ppm: m.taker_fee_rate_ppm,
    }
}

/// Adds each leg's market kind, line and card label from the catalogue (6.0b). Nothing here is
/// signed: the C3 message is unchanged.
pub fn describe_legs(cat: &Catalogue, legs: &mut [QuoteLeg]) {
    for l in legs {
        if let Some(m) = cat.market(&l.symbol) {
            l.kind = Some(m.kind);
            l.line = m.line();
            l.label = Some(m.label(l.side));
        }
    }
}

/// A signed quote: the response plus what the quote store keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedQuote {
    /// The C4 response.
    pub response: QuoteResponse,
    /// The C3 message bytes.
    pub msg: Vec<u8>,
    /// The ed25519 signature.
    pub sig: [u8; 64],
    /// Lowercase hex sha256 of `msg` (`Pending.quote_hash`).
    pub quote_hash: String,
}

/// Signs `q` with `signer` for program `program_id`, expiring at `quote_expires_at` (Unix s).
pub fn sign_quote(
    q: &pricing::Quote,
    copy_fee_u: u64,
    quote_expires_at: i64,
    program_id: &Pubkey,
    signer: &Keypair,
) -> SignedQuote {
    let msg = QuoteMsg {
        program_id: program_id.to_bytes(),
        legs: q
            .legs
            .iter()
            .map(|l| {
                QuoteMsgLeg::new(
                    &l.symbol,
                    &l.game_id,
                    // Pascal start times are whole seconds; a stray millisecond rounds down.
                    to_i64(l.start_ms / 1000),
                    match l.side {
                        pricing::Side::Long => codec::SIDE_LONG,
                        pricing::Side::Short => codec::SIDE_SHORT,
                    },
                    l.price_u,
                    l.m1_ppm,
                )
            })
            .collect(),
        stake_u: q.stake_u,
        odds_offered_u: q.odds_offered_u,
        quote_expires_at,
    }
    .encode();
    let sig: [u8; 64] = signer.sign(&msg).into();
    let b64 = base64::engine::general_purpose::STANDARD;
    let response = QuoteResponse {
        n: q.n,
        stake_u: q.stake_u,
        legs: q
            .legs
            .iter()
            .map(|l| QuoteLeg {
                symbol: l.symbol.clone(),
                game_id: l.game_id.clone(),
                side: api_side(l.side),
                pick: team_side(l.pick),
                start_ms: l.start_ms,
                price_u: l.price_u,
                p_max_u: l.p_max_u,
                m1_ppm: l.m1_ppm,
                overlaps: l.overlaps,
                in_play: l.in_play,
                kind: None,
                line: None,
                label: None,
            })
            .collect(),
        odds_fair_u: q.odds_fair_u,
        odds_offered_u: q.odds_offered_u,
        payout_u: q.payout_u,
        fee_u: q.fee_u,
        pascal_fees_est_u: q.pascal_fees_est_u,
        copy_fee_u,
        slippage_bps: q.slippage_bps,
        min_payout_per_unit_u: q.min_payout_per_unit_u,
        quote_expires_at,
        quote_signer: signer.pubkey().to_string(),
        quote_msg: b64.encode(&msg),
        quote_sig: b64.encode(sig),
    };
    SignedQuote {
        response,
        quote_hash: codec::hex(&codec::sha256(&msg)),
        msg,
        sig,
    }
}

/// The `(wallet, trimmed title)` a quote request names its tip with (L9), checked as
/// `PUT /profile` checks a title; `None` without a title.
fn tip_title(req: &QuoteRequest) -> Result<Option<(String, String)>, ApiError> {
    let Some(title) = &req.title else {
        return Ok(None);
    };
    if req.copied_from.is_some() {
        return Err(ApiError::rejected(
            "a copy takes no title: only own tips are named",
        ));
    }
    let wallet = req
        .wallet
        .as_deref()
        .ok_or_else(|| ApiError::rejected("a title needs the wallet that will sign"))?;
    let wallet = wallet
        .parse::<Pubkey>()
        .map_err(|_| ApiError::rejected(format!("wallet {wallet} is not a base58 public key")))?;
    let title = crate::profiles::clean_title(title).map_err(ApiError::rejected)?;
    Ok(Some((wallet.to_string(), title)))
}

/// Keeps `title` for the own tip `wallet` may open with quote `r` (`quote_titles`). The same
/// bytes quoted again for the same wallet keep the latest title.
async fn store_title(
    db: &SqlitePool,
    r: &QuoteResponse,
    wallet: &str,
    title: &str,
) -> Result<(), ApiError> {
    let msg = base64::engine::general_purpose::STANDARD
        .decode(&r.quote_msg)
        .map_err(|e| ApiError::unavailable(format!("quote_msg is not base64: {e}")))?;
    sqlx::query(
        "INSERT INTO quote_titles (quote_hash, wallet, title, created_ms) VALUES (?, ?, ?, ?)
         ON CONFLICT (quote_hash, wallet) DO UPDATE SET title = excluded.title,
            created_ms = excluded.created_ms",
    )
    .bind(codec::hex(&codec::sha256(&msg)))
    .bind(wallet)
    .bind(title)
    .bind(to_i64(now_ms()))
    .execute(db)
    .await?;
    Ok(())
}

/// Writes the `quotes` row and one `quote_legs` row per leg (L2-1), in one transaction. The
/// same bytes signed twice (same legs, prices and expiry second) are one row.
pub async fn store_quote(
    db: &SqlitePool,
    s: &SignedQuote,
    copied_from: Option<&str>,
) -> Result<(), sqlx::Error> {
    let r = &s.response;
    let legs_json = serde_json::to_string(&r.legs).unwrap_or_else(|_| "[]".to_owned());
    let mut tx = db.begin().await?;
    sqlx::query(
        "INSERT INTO quotes (quote_hash, msg, sig, quote_signer, n, stake_u, odds_offered_u,
            odds_fair_u, payout_u, fee_u, min_payout_per_unit_u, quote_expires_at, copied_from,
            legs_json, created_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (quote_hash) DO NOTHING",
    )
    .bind(&s.quote_hash)
    .bind(&s.msg)
    .bind(&s.sig[..])
    .bind(&r.quote_signer)
    .bind(i64::from(r.n))
    .bind(to_i64(r.stake_u))
    .bind(to_i64(r.odds_offered_u))
    .bind(to_i64(r.odds_fair_u))
    .bind(to_i64(r.payout_u))
    .bind(to_i64(r.fee_u))
    .bind(to_i64(r.min_payout_per_unit_u))
    .bind(r.quote_expires_at)
    .bind(copied_from)
    .bind(legs_json)
    .bind(to_i64(now_ms()))
    .execute(&mut *tx)
    .await?;
    for (idx, l) in r.legs.iter().enumerate() {
        sqlx::query(
            "INSERT INTO quote_legs (quote_hash, idx, symbol, symbol_hash, game_id, game_id_hash)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (quote_hash, idx) DO NOTHING",
        )
        .bind(&s.quote_hash)
        .bind(idx as i64)
        .bind(&l.symbol)
        .bind(codec::hex(&codec::sha256(l.symbol.as_bytes())))
        .bind(&l.game_id)
        .bind(codec::hex(&codec::sha256(l.game_id.as_bytes())))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_fee_rounds_up() {
        assert_eq!(copy_fee_u(10_000_000), 200_000);
        assert_eq!(copy_fee_u(10_000_001), 200_001);
        assert_eq!(copy_fee_u(0), 0);
    }

    #[test]
    fn a_live_tip_can_be_copied_until_a_leg_is_decided() {
        // Nothing here reads a start: a leg in play is the quote's business (D-660).
        let leg = |status: &str| CopySourceLeg {
            symbol: "S".into(),
            side: "long".into(),
            status: status.into(),
        };
        let tip = |legs| CopySource {
            state: "open".into(),
            user: "U".into(),
            tipster: Some("U".into()),
            copied_from: None,
            legs,
        };
        assert_eq!(
            copy_refusal("T", &tip(vec![leg("open"), leg("open")])),
            None
        );
        let decided = copy_refusal("T", &tip(vec![leg("won"), leg("open")])).unwrap();
        assert!(decided.contains("already decided"), "{decided}");
    }
}
