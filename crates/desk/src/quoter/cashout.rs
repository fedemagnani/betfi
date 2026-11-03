//! `GET /cashout-quote/{ticket}` (C4, spec section 2 "Cash-out quote", flow 3) and the
//! indicative [`Valuer`] behind `/tickets` (D-040).
//!
//! `C = P · Π r̃ⱼ (resolved) · Π b̃ⱼ (open) · (1 − c)` by `pricing::cash_out`, which also gates on
//! the closing side's depth. The ticket and its legs come from the DB (`tickets`,
//! `ticket_legs`; L2-3), never from `AppState::tickets`. Every refusal of an existing ticket is
//! `503 unavailable` with the reason (C4 lists only that), an unknown ticket is `404`.

use std::collections::BTreeSet;
use std::str::FromStr;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use base64::Engine;
use pricing::{CashOutLeg, LegStatus as PLegStatus, Params, Rejection};
use solana_pubkey::Pubkey;

use super::books::{books_for, to_i64};
use super::codec::{self, CashOutMsg};
use super::quote::{now_s, pricing_side};
use super::{QuoterState, VALUER_MAX_AGE_MS};
use crate::api::ApiError;
use crate::api::types::{CashOut, CashoutQuoteResponse, LegStatus, Side, TicketStatus};
use crate::now_ms;
use crate::solana::signer::Keypair;
use crate::state::{AppState, CashOutValuer, TicketState};

/// `r` (long side, micro-units) of a resolved leg: the recorded `resolution_u` when there is
/// one, else the value its status implies (won / lost / half or void at 0.5). `None` while the
/// leg is open.
pub fn resolved_r_u(side: Side, status: LegStatus, resolution_u: Option<u64>) -> Option<u64> {
    if let Some(r) = resolution_u {
        return Some(r.min(1_000_000));
    }
    let r_tilde = match status {
        LegStatus::Open => return None,
        LegStatus::Won => 1_000_000,
        LegStatus::Lost => 0,
        LegStatus::Half | LegStatus::Void => 500_000,
    };
    Some(match side {
        Side::Long => r_tilde,
        Side::Short => 1_000_000 - r_tilde,
    })
}

/// One leg as the cash-out needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CashLeg {
    /// Symbol.
    pub symbol: String,
    /// Side.
    pub side: Side,
    /// `Some(r)` once resolved.
    pub r_u: Option<u64>,
}

/// `pricing::cash_out` over `legs` with the books in `books` (a missing book makes an open leg
/// unavailable).
pub fn value(
    payout_u: u64,
    legs: &[CashLeg],
    books: &std::collections::BTreeMap<String, pascal_rs::OrderBook>,
    params: &Params,
) -> Result<pricing::CashOut, Rejection> {
    let legs: Vec<CashOutLeg<'_>> = legs
        .iter()
        .map(|l| CashOutLeg {
            symbol: &l.symbol,
            side: pricing_side(l.side),
            status: match l.r_u {
                Some(r_u) => PLegStatus::Resolved { r_u },
                None => PLegStatus::Open,
            },
            book: books.get(&l.symbol),
        })
        .collect();
    pricing::cash_out(payout_u, &legs, params)
}

/// The indicative cash-out of `/tickets`: cached books only (no I/O), at most
/// [`VALUER_MAX_AGE_MS`] old.
pub struct Valuer {
    q: Arc<QuoterState>,
}

impl Valuer {
    /// A valuer over the quoter's book cache.
    pub fn new(q: Arc<QuoterState>) -> Self {
        Valuer { q }
    }
}

impl CashOutValuer for Valuer {
    fn cash_out(&self, t: &TicketState) -> CashOut {
        if t.status != TicketStatus::Open {
            return CashOut::unavailable(format!("the ticket is {}", t.status.as_str()));
        }
        let legs: Vec<CashLeg> = t
            .legs
            .iter()
            .map(|l| CashLeg {
                symbol: l.symbol.clone(),
                side: l.side,
                r_u: resolved_r_u(l.side, l.status, None),
            })
            .collect();
        let open: BTreeSet<String> = legs
            .iter()
            .filter(|l| l.r_u.is_none())
            .map(|l| l.symbol.clone())
            .collect();
        let books = self.q.collect(&open, now_ms(), VALUER_MAX_AGE_MS).books;
        match value(t.payout_u, &legs, &books, &self.q.params) {
            Ok(c) => CashOut {
                available: true,
                c_u: Some(c.c_u),
                reason: None,
            },
            Err(r) => CashOut::unavailable(r.reason),
        }
    }
}

/// `GET /cashout-quote/{ticket}`.
pub async fn get_cashout_quote(
    State(state): State<AppState>,
    Path(ticket): Path<String>,
) -> Result<Json<CashoutQuoteResponse>, ApiError> {
    let r = cashout_quote(&state, &ticket).await;
    let result = match &r {
        Ok(_) => "ok",
        Err(e) if e.status_code().as_u16() == 404 => "not_found",
        Err(_) => "unavailable",
    };
    metrics::counter!("desk_cashout_quotes_total", "result" => result).increment(1);
    r.map(Json)
}

async fn cashout_quote(state: &AppState, ticket: &str) -> Result<CashoutQuoteResponse, ApiError> {
    let unknown = || ApiError::not_found(format!("unknown ticket {ticket}"));
    let pda = Pubkey::from_str(ticket).map_err(|_| unknown())?;
    let row: Option<(String, i64)> =
        sqlx::query_as("SELECT state, payout_u FROM tickets WHERE ticket = ?")
            .bind(ticket)
            .fetch_optional(&state.db)
            .await?;
    let (ticket_state, payout_u) = row.ok_or_else(unknown)?;
    if ticket_state != TicketStatus::Open.as_str() {
        return Err(ApiError::unavailable(format!(
            "cash-out unavailable: the ticket is {ticket_state}"
        )));
    }
    let payout_u = u64::try_from(payout_u).map_err(|_| unreadable(ticket))?;
    let rows: Vec<(String, String, String, Option<i64>)> = sqlx::query_as(
        "SELECT symbol, side, status, resolution_u FROM ticket_legs WHERE ticket = ? ORDER BY idx",
    )
    .bind(ticket)
    .fetch_all(&state.db)
    .await?;
    if rows.is_empty() {
        return Err(unreadable(ticket));
    }
    let legs = rows
        .into_iter()
        .map(|(symbol, side, status, resolution_u)| {
            let side: Side = parse_enum(&side).ok_or_else(|| unreadable(ticket))?;
            let status: LegStatus = parse_enum(&status).ok_or_else(|| unreadable(ticket))?;
            let resolution_u = resolution_u
                .map(u64::try_from)
                .transpose()
                .map_err(|_| unreadable(ticket))?;
            Ok(CashLeg {
                symbol,
                side,
                r_u: resolved_r_u(side, status, resolution_u),
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;

    let signer = state.quoter.signer(&state.config).map_err(|e| {
        tracing::error!(error = %e, "quote signer unavailable");
        ApiError::unavailable("the desk cannot sign cash-outs right now")
    })?;
    let open: BTreeSet<String> = legs
        .iter()
        .filter(|l| l.r_u.is_none())
        .map(|l| l.symbol.clone())
        .collect();
    let books = if open.is_empty() {
        Default::default()
    } else {
        books_for(state, &open)
            .await
            .map_err(|e| ApiError::unavailable(e.0))?
            .books
    };
    let c = value(payout_u, &legs, &books, &state.quoter.params)
        .map_err(|r| ApiError::unavailable(r.reason))?;
    if c.c_u == 0 {
        return Err(ApiError::unavailable(
            "cash-out unavailable: the ticket is worth nothing now",
        ));
    }
    let signed = sign_cash_out(&pda, c.c_u, now_s(), &state.config.program_id, &signer);
    sqlx::query(
        "INSERT INTO cashout_quotes (quote_hash, ticket, c_u, ts, msg, sig, quote_signer,
            created_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (quote_hash) DO NOTHING",
    )
    .bind(codec::hex(&codec::sha256(&signed.1)))
    .bind(ticket)
    .bind(to_i64(c.c_u))
    .bind(signed.0.ts)
    .bind(&signed.1)
    .bind(&signed.2[..])
    .bind(&signed.0.quote_signer)
    .bind(to_i64(now_ms()))
    .execute(&state.db)
    .await?;
    tracing::info!(ticket, c_u = c.c_u, "cash-out quote signed");
    Ok(signed.0)
}

fn unreadable(ticket: &str) -> ApiError {
    tracing::error!(ticket, "ticket record is unreadable");
    ApiError::unavailable("cash-out unavailable: the ticket record is incomplete")
}

/// A snake_case DB string as one of the C4 enums.
fn parse_enum<T: serde::de::DeserializeOwned>(s: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(s.to_owned())).ok()
}

/// Signs the cash-out of `ticket` for `c_u` at `ts`: the response, the message and the
/// signature.
pub fn sign_cash_out(
    ticket: &Pubkey,
    c_u: u64,
    ts: i64,
    program_id: &Pubkey,
    signer: &Keypair,
) -> (CashoutQuoteResponse, Vec<u8>, [u8; 64]) {
    let msg = CashOutMsg {
        program_id: program_id.to_bytes(),
        ticket: ticket.to_bytes(),
        c_u,
        ts,
    }
    .encode();
    let sig: [u8; 64] = signer.sign(&msg).into();
    let b64 = base64::engine::general_purpose::STANDARD;
    (
        CashoutQuoteResponse {
            ticket: ticket.to_string(),
            c_u,
            ts,
            quote_signer: signer.pubkey().to_string(),
            quote_msg: b64.encode(&msg),
            quote_sig: b64.encode(sig),
        },
        msg,
        sig,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_values() {
        assert_eq!(resolved_r_u(Side::Long, LegStatus::Open, None), None);
        assert_eq!(
            resolved_r_u(Side::Long, LegStatus::Won, None),
            Some(1_000_000)
        );
        assert_eq!(resolved_r_u(Side::Short, LegStatus::Won, None), Some(0));
        assert_eq!(
            resolved_r_u(Side::Short, LegStatus::Lost, None),
            Some(1_000_000)
        );
        assert_eq!(
            resolved_r_u(Side::Short, LegStatus::Void, None),
            Some(500_000)
        );
        // A recorded r wins over the status.
        assert_eq!(resolved_r_u(Side::Short, LegStatus::Won, Some(0)), Some(0));
        assert_eq!(
            resolved_r_u(Side::Long, LegStatus::Open, Some(2_000_000)),
            Some(1_000_000)
        );
        assert_eq!(parse_enum::<LegStatus>("half"), Some(LegStatus::Half));
        assert_eq!(parse_enum::<Side>("sideways"), None::<Side>);
    }
}
