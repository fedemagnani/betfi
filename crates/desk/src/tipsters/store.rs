//! Tipster rows in and out of the database: the indexer's hooks (`TipsterPaid` events, the copy
//! fee charged at finalize, the resync fallback) and the loaders [`super::metrics`] scores.

use std::collections::HashMap;

use solana_pubkey::Pubkey;
use sqlx::{Row, SqlitePool};

use super::metrics::{CopyTicket, OwnTip, TipLeg, TipsterBook};
use crate::api::types::Side;
use crate::crank::tickets::status_of;
use crate::now_ms;
use crate::program::TicketAccount;

fn i(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn u(v: Option<i64>) -> Option<u64> {
    v.and_then(|v| u64::try_from(v).ok())
}

/// A `TipsterPaid` event (authenticated, from the finalize transaction `tx`): replaces any
/// derived row of the same copy.
pub async fn record_paid(
    db: &SqlitePool,
    ticket: &Pubkey,
    tipster: &Pubkey,
    amount_u: u64,
    tx: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO tipster_paid (ticket, tipster, amount_u, source, tx, updated_ms)
         VALUES (?, ?, ?, 'event', ?, ?)
         ON CONFLICT (ticket) DO UPDATE SET tipster = excluded.tipster,
            amount_u = excluded.amount_u, source = 'event',
            tx = COALESCE(excluded.tx, tipster_paid.tx), updated_ms = excluded.updated_ms",
    )
    .bind(ticket.to_string())
    .bind(tipster.to_string())
    .bind(i(amount_u))
    .bind(tx)
    .bind(i(now_ms()))
    .execute(db)
    .await?;
    Ok(())
}

/// After the indexer applied a `Ticket` account: a copy's row takes the copy fee actually charged
/// (`Ticket.copy_fee_u`, the r-scaled part of the escrowed fee, D-111), and a copy whose
/// `TipsterPaid` the listener never saw (resync) gets a derived row,
/// `floor(copy_fee × tipster_share_bps / 1e4)`, once the indexer knows the config.
pub async fn on_ticket(
    db: &SqlitePool,
    key: &Pubkey,
    t: &TicketAccount,
) -> Result<(), sqlx::Error> {
    if t.copied_from.is_none() {
        return Ok(());
    }
    let ticket = key.to_string();
    sqlx::query("UPDATE tickets SET copy_fee_u = ? WHERE ticket = ? AND copy_fee_u != ?")
        .bind(i(t.copy_fee_u))
        .bind(&ticket)
        .bind(i(t.copy_fee_u))
        .execute(db)
        .await?;
    let Some(tipster) = t.tipster else {
        return Ok(());
    };
    let share: Option<i64> =
        sqlx::query_scalar("SELECT tipster_share_bps FROM chain_config WHERE id = 1")
            .fetch_optional(db)
            .await?;
    let Some(share) = share.and_then(|s| u64::try_from(s).ok()) else {
        return Ok(());
    };
    let amount = u128::from(t.copy_fee_u) * u128::from(share) / 10_000;
    sqlx::query(
        "INSERT OR IGNORE INTO tipster_paid (ticket, tipster, amount_u, source, tx, updated_ms)
         VALUES (?, ?, ?, 'derived', NULL, ?)",
    )
    .bind(&ticket)
    .bind(tipster.to_string())
    .bind(i64::try_from(amount).unwrap_or(i64::MAX))
    .bind(i(now_ms()))
    .execute(db)
    .await?;
    Ok(())
}

/// The books of every tipster, or of `wallet` only (empty when it has no minted own tip).
pub async fn books(db: &SqlitePool, wallet: Option<&str>) -> Result<Vec<TipsterBook>, sqlx::Error> {
    let only = wallet.is_some();
    let own = sqlx::query(
        "SELECT ticket, user, state, COALESCE(rs_u, stake_u) AS stake, paid_u,
                COALESCE(odds_u, odds_offered_u) AS odds, minted_at, settled_at
         FROM tickets WHERE tipster = user AND copied_from IS NULL
         AND state IN ('open', 'settled', 'cashed_out') AND (? = 0 OR user = ?)",
    )
    .bind(i64::from(only))
    .bind(wallet)
    .fetch_all(db)
    .await?;
    let legs = sqlx::query(
        "SELECT l.ticket, l.side, COALESCE(l.p_fill_u, l.p_quote_u) AS p, l.status, l.resolution_u
         FROM ticket_legs l JOIN tickets t ON t.ticket = l.ticket
         WHERE t.tipster = t.user AND t.copied_from IS NULL
         AND t.state IN ('open', 'settled', 'cashed_out') AND (? = 0 OR t.user = ?)
         ORDER BY l.ticket, l.idx",
    )
    .bind(i64::from(only))
    .bind(wallet)
    .fetch_all(db)
    .await?;
    let copies = sqlx::query(
        "SELECT tipster, user, state, COALESCE(rs_u, stake_u) AS stake, copy_fee_u, paid_u
         FROM tickets WHERE copied_from IS NOT NULL AND tipster IS NOT NULL
         AND state IN ('open', 'settled', 'cashed_out') AND (? = 0 OR tipster = ?)",
    )
    .bind(i64::from(only))
    .bind(wallet)
    .fetch_all(db)
    .await?;
    let fees: Vec<(String, i64)> = sqlx::query_as(
        "SELECT tipster, SUM(amount_u) FROM tipster_paid WHERE (? = 0 OR tipster = ?)
         GROUP BY tipster",
    )
    .bind(i64::from(only))
    .bind(wallet)
    .fetch_all(db)
    .await?;

    let mut legs_of: HashMap<String, Vec<TipLeg>> = HashMap::new();
    for l in &legs {
        let resolved = l.try_get::<String, _>("status").is_ok_and(|s| s != "open");
        legs_of
            .entry(l.try_get("ticket")?)
            .or_default()
            .push(TipLeg {
                side: if l.try_get::<String, _>("side")? == "short" {
                    Side::Short
                } else {
                    Side::Long
                },
                p_fill_u: u(l.try_get("p")?).unwrap_or(0),
                resolution_u: if resolved {
                    u(l.try_get("resolution_u")?)
                } else {
                    None
                },
            });
    }
    let mut by_wallet: HashMap<String, TipsterBook> = HashMap::new();
    for r in &own {
        let wallet: String = r.try_get("user")?;
        let ticket: String = r.try_get("ticket")?;
        let book = by_wallet
            .entry(wallet.clone())
            .or_insert_with(|| TipsterBook {
                wallet,
                ..TipsterBook::default()
            });
        book.own.push(OwnTip {
            legs: legs_of.remove(&ticket).unwrap_or_default(),
            ticket,
            state: status_of(&r.try_get::<String, _>("state")?),
            stake_u: u(r.try_get("stake")?).unwrap_or(0),
            paid_u: u(r.try_get("paid_u")?),
            odds_u: u(r.try_get("odds")?).unwrap_or(0),
            minted_at: r.try_get("minted_at")?,
            settled_at: r.try_get("settled_at")?,
        });
    }
    // Copies and fees only for wallets that have own tips (a copy needs an own-tip original).
    for c in &copies {
        let tipster: String = c.try_get("tipster")?;
        if let Some(book) = by_wallet.get_mut(&tipster) {
            book.copies.push(CopyTicket {
                user: c.try_get("user")?,
                state: status_of(&c.try_get::<String, _>("state")?),
                stake_u: u(c.try_get("stake")?).unwrap_or(0),
                copy_fee_u: u(c.try_get("copy_fee_u")?).unwrap_or(0),
                paid_u: u(c.try_get("paid_u")?),
            });
        }
    }
    for (tipster, sum) in fees {
        if let Some(book) = by_wallet.get_mut(&tipster) {
            book.fees_earned_u = u64::try_from(sum).unwrap_or(0);
        }
    }
    let mut out: Vec<TipsterBook> = by_wallet.into_values().collect();
    out.sort_by(|a, b| a.wallet.cmp(&b.wallet));
    Ok(out)
}

/// `wallet`'s settled and cashed-out own tips (`settled`), newest settlement first, or its open
/// ones, newest mint first; at most `limit`.
pub async fn own_tip_ids(
    db: &SqlitePool,
    wallet: &str,
    settled: bool,
    limit: i64,
) -> Result<Vec<String>, sqlx::Error> {
    let sql = if settled {
        "SELECT ticket FROM tickets WHERE user = ? AND tipster = user AND copied_from IS NULL
         AND state IN ('settled', 'cashed_out')
         ORDER BY COALESCE(settled_at, 0) DESC, ticket DESC LIMIT ?"
    } else {
        "SELECT ticket FROM tickets WHERE user = ? AND tipster = user AND copied_from IS NULL
         AND state = 'open' ORDER BY COALESCE(minted_at, 0) DESC, ticket DESC LIMIT ?"
    };
    sqlx::query_scalar(sql)
        .bind(wallet)
        .bind(limit)
        .fetch_all(db)
        .await
}
