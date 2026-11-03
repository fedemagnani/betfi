//! The EUR/USD rate the app shows euros with (U-4, D-303; l6-contracts L6-2, step 6.0c).
//!
//! `GET /fx` → `{"eur_per_usd_ppm", "as_of", "source": "ecb"|"fallback", "fallback"}`. USDC is
//! taken at 1 USD; the app shows € = USDC × rate, truncated to the cent, with the USDC beneath.
//!
//! [`run`] reads the ECB reference rate from frankfurter (`DESK_FX_URL`, default
//! [`crate::config::FX_URL_DEFAULT`] on `demo` and `live`, nothing on `dev`) once every
//! `DESK_FX_S` (a day), retries a failed read every
//! [`RETRY_EVERY`], and stores each rate in `fx_rates` (`migrations/0009_profiles.sql`), so a
//! restart does not ask again within the day. The route serves the newest stored rate while its
//! ECB date is at most [`MAX_AGE_DAYS`] old (the ECB publishes on TARGET business days only), and
//! the configured fixed rate (`DESK_FX_FALLBACK_PPM`, flagged `fallback: true`) otherwise, so it
//! answers with the upstream down and never fails.
//!
//! frankfurter is a read-only host with no key; it is not a Pascal host, so the `live_guard` scan
//! is unaffected. Tests point `fx_url` at a local fake (an empty `fx_url` never fetches).
//!
//! 7.0c (l7-contracts L7-1) adds the SOL price ([`sol`]): `eur_per_sol_ppm`, `sol_as_of_ms`,
//! `sol_source` (`pyth` | `fallback`) and `sol_fallback`, read from memory, so `/fx` still never
//! waits on the network.

pub mod sol;

use std::time::Duration;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::Value;
use sqlx::SqlitePool;

use crate::api::types::FxResponse;
use crate::now_ms;
use crate::state::AppState;

/// The pair stored in `fx_rates.pair`.
pub const PAIR: &str = "USD/EUR";
/// A stored rate older than this (by its ECB date) gives way to the fallback.
pub const MAX_AGE_DAYS: i64 = 7;
/// Retry period after a failed read (capped by `DESK_FX_S`).
pub const RETRY_EVERY: Duration = Duration::from_secs(15 * 60);
/// Per-request timeout of a read.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest answer read, bytes.
pub const MAX_BODY: usize = 64 * 1024;
/// A rate outside `[MIN_PPM, MAX_PPM]` EUR per USD is refused as garbage.
pub const MIN_PPM: u64 = 500_000;
/// See [`MIN_PPM`].
pub const MAX_PPM: u64 = 2_000_000;

/// A rate read from upstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rate {
    /// ECB date, `YYYY-MM-DD`.
    pub as_of: String,
    /// EUR per USD × 1e6, rounded to the nearest.
    pub rate_ppm: u64,
}

/// Mounts `GET /fx`.
pub fn mount(router: Router<AppState>, _state: &AppState) -> Router<AppState> {
    router.route("/fx", get(fx))
}

/// `GET /fx`.
async fn fx(State(state): State<AppState>) -> Json<FxResponse> {
    Json(current(&state).await)
}

/// Days since 1970-01-01 of a `YYYY-MM-DD` date, or `None` if it is not one.
pub fn days_from_date(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' || !digits(0..4) {
        return None;
    }
    if !digits(5..7) || !digits(8..10) {
        return None;
    }
    let (y, m, d): (i64, i64, i64) = (
        s[0..4].parse().ok()?,
        s[5..7].parse().ok()?,
        s[8..10].parse().ok()?,
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Howard Hinnant's days_from_civil.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// The `YYYY-MM-DD` date `days` after 1970-01-01 (the inverse of [`days_from_date`]).
pub fn date_from_days(days: i64) -> String {
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn today_days() -> i64 {
    i64::try_from(now_ms() / 86_400_000).unwrap_or(i64::MAX)
}

/// The rate in an upstream answer: frankfurter v2 (`[{"date","base","quote","rate"}]`, the
/// `USD`→`EUR` item) or v1 (`{"base":"USD","date","rates":{"EUR":…}}`).
pub fn parse(body: &str) -> Result<Rate, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("not JSON: {e}"))?;
    let (date, rate) = match &v {
        Value::Array(items) => {
            let item = items
                .iter()
                .find(|i| i["base"] == "USD" && i["quote"] == "EUR")
                .ok_or("no USD→EUR item")?;
            (&item["date"], &item["rate"])
        }
        Value::Object(o) => {
            if o.get("base").is_some_and(|b| b != "USD") {
                return Err(format!("base is {}, not USD", o["base"]));
            }
            (&v["date"], &v["rates"]["EUR"])
        }
        _ => return Err("unexpected JSON".into()),
    };
    let as_of = date.as_str().ok_or("no date")?.to_owned();
    if days_from_date(&as_of).is_none() {
        return Err(format!("bad date {as_of:?}"));
    }
    let rate = rate.as_f64().ok_or("no EUR rate")?;
    let ppm = (rate * 1e6).round();
    if !ppm.is_finite() || ppm < MIN_PPM as f64 || ppm > MAX_PPM as f64 {
        return Err(format!("rate {rate} outside the sane range"));
    }
    // In range, so the cast is exact.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let rate_ppm = ppm as u64;
    Ok(Rate { as_of, rate_ppm })
}

/// The newest stored rate: `(as_of, rate_ppm)`.
pub async fn latest(db: &SqlitePool) -> Result<Option<(String, u64)>, sqlx::Error> {
    let row: Option<(String, i64)> = sqlx::query_as(
        "SELECT as_of, rate_ppm FROM fx_rates WHERE pair = ? ORDER BY as_of DESC LIMIT 1",
    )
    .bind(PAIR)
    .fetch_optional(db)
    .await?;
    Ok(row.and_then(|(d, r)| Some((d, u64::try_from(r).ok()?))))
}

/// What `GET /fx` answers now: the newest stored rate if at most [`MAX_AGE_DAYS`] old, else the
/// fallback (also when the database cannot be read), with the cached SOL price ([`sol`]).
pub async fn current(state: &AppState) -> FxResponse {
    let stored = latest(&state.db).await.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "fx: cannot read the stored rate; serving the fallback");
        None
    });
    let ecb = answer(
        stored,
        today_days(),
        state.config.fx_fallback_ppm,
        &state.config.fx_fallback_as_of,
    );
    with_sol(ecb, &sol::current(state))
}

/// `fx` with the SOL fields of `s` (EUR per SOL = USD per SOL × `fx.eur_per_usd_ppm`).
pub fn with_sol(fx: FxResponse, s: &sol::SolAnswer) -> FxResponse {
    FxResponse {
        eur_per_sol_ppm: sol::eur_per_sol_ppm(s.usd_u, fx.eur_per_usd_ppm),
        sol_as_of_ms: s.as_of_ms,
        sol_source: s.source.to_owned(),
        sol_fallback: s.fallback,
        ..fx
    }
}

/// [`current`]'s choice, pure: `stored` if fresh on `today` (days since the epoch).
pub fn answer(
    stored: Option<(String, u64)>,
    today: i64,
    fallback_ppm: u64,
    fallback_as_of: &str,
) -> FxResponse {
    match stored {
        Some((as_of, ppm)) if days_from_date(&as_of).is_some_and(|d| today - d <= MAX_AGE_DAYS) => {
            FxResponse {
                eur_per_usd_ppm: ppm,
                as_of,
                source: "ecb".into(),
                fallback: false,
                ..FxResponse::default()
            }
        }
        _ => FxResponse {
            eur_per_usd_ppm: fallback_ppm,
            as_of: fallback_as_of.to_owned(),
            source: "fallback".into(),
            fallback: true,
            ..FxResponse::default()
        },
    }
}

/// The client [`run`] reads with.
pub fn client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .user_agent(concat!("schedina-desk/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// One read of `url`: the rate and the raw answer.
pub async fn fetch(client: &reqwest::Client, url: &str) -> Result<(Rate, String), String> {
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("request failed: {}", e.without_url()))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("read failed: {}", e.without_url()))?
    {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BODY {
            return Err(format!("answer over {MAX_BODY} bytes"));
        }
    }
    let text = String::from_utf8(body).map_err(|_| "answer is not UTF-8".to_owned())?;
    let rate = parse(&text)?;
    Ok((rate, text))
}

/// Stores `rate` (one row per ECB date; a re-read of the same date refreshes it).
pub async fn store(db: &SqlitePool, rate: &Rate, raw: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO fx_rates (pair, as_of, rate_ppm, source, fetched_ms, raw)
         VALUES (?, ?, ?, 'ecb', ?, ?)
         ON CONFLICT (pair, as_of) DO UPDATE SET rate_ppm = excluded.rate_ppm,
            fetched_ms = excluded.fetched_ms, raw = excluded.raw",
    )
    .bind(PAIR)
    .bind(&rate.as_of)
    .bind(i64::try_from(rate.rate_ppm).unwrap_or(i64::MAX))
    .bind(i64::try_from(now_ms()).unwrap_or(i64::MAX))
    .bind(raw)
    .execute(db)
    .await?;
    Ok(())
}

/// Reads `state.config.fx_url` once and stores the rate.
pub async fn refresh(state: &AppState, client: &reqwest::Client) -> Result<Rate, String> {
    let (rate, raw) = fetch(client, &state.config.fx_url).await?;
    store(&state.db, &rate, &raw)
        .await
        .map_err(|e| format!("cannot store the rate: {e}"))?;
    Ok(rate)
}

async fn last_fetched_ms(db: &SqlitePool) -> u64 {
    let v: Option<i64> = sqlx::query_scalar("SELECT MAX(fetched_ms) FROM fx_rates WHERE pair = ?")
        .bind(PAIR)
        .fetch_one(db)
        .await
        .unwrap_or(None);
    v.and_then(|v| u64::try_from(v).ok()).unwrap_or(0)
}

/// Runs the ECB reader ([`run_ecb`]) and the SOL price reader ([`sol::run`]) until shutdown.
pub async fn run(state: AppState) {
    tokio::join!(run_ecb(state.clone()), sol::run(state));
}

/// Reads the rate when the stored one is a period old, until shutdown. Idle when `fx_url` is
/// empty or `DESK_FX_S=0`.
pub async fn run_ecb(state: AppState) {
    if state.config.fx_url.is_empty() || state.config.fx_every.is_zero() {
        tracing::info!(
            "fx: not fetching (no DESK_FX_URL or DESK_FX_S=0); /fx serves what is stored or the fallback"
        );
        state.shutdown_requested().await;
        return;
    }
    let client = match client() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "fx: cannot build the client; idle");
            state.shutdown_requested().await;
            return;
        }
    };
    let every = state.config.fx_every;
    let every_ms = u64::try_from(every.as_millis()).unwrap_or(u64::MAX);
    loop {
        let due = last_fetched_ms(&state.db).await.saturating_add(every_ms);
        let wait = Duration::from_millis(due.saturating_sub(now_ms()));
        if !wait.is_zero() {
            tokio::select! {
                _ = state.shutdown_requested() => break,
                _ = tokio::time::sleep(wait) => continue,
            }
        }
        let outcome = tokio::select! {
            _ = state.shutdown_requested() => break,
            r = refresh(&state, &client) => r,
        };
        match outcome {
            Ok(r) => {
                metrics::counter!("desk_fx_fetch_total", "result" => "ok").increment(1);
                tracing::info!(as_of = %r.as_of, eur_per_usd_ppm = r.rate_ppm, "fx rate stored");
            }
            Err(e) => {
                metrics::counter!("desk_fx_fetch_total", "result" => "error").increment(1);
                tracing::warn!(error = %e, "fx: read failed; serving the stored rate or the fallback");
                tokio::select! {
                    _ = state.shutdown_requested() => break,
                    _ = tokio::time::sleep(RETRY_EVERY.min(every)) => {}
                }
            }
        }
    }
    tracing::info!("fx stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_count_days() {
        assert_eq!(days_from_date("1970-01-01"), Some(0));
        assert_eq!(days_from_date("2000-03-01"), Some(11_017));
        assert_eq!(days_from_date("2026-10-29"), Some(20_755));
        for bad in [
            "2026-1-29",
            "2026-13-01",
            "26-10-2029",
            "2026-10-29x",
            "+026-10-29",
        ] {
            assert_eq!(days_from_date(bad), None, "{bad}");
        }
        for d in [0, 59, 60, 11_016, 11_017, 20_755, 20_819, 47_482] {
            assert_eq!(days_from_date(&date_from_days(d)), Some(d), "{d}");
        }
        assert_eq!(date_from_days(20_755), "2026-10-29");
        assert_eq!(date_from_days(20_819), "2027-01-01");
    }

    #[test]
    fn both_upstream_shapes_parse() {
        let v2 = r#"[{"date":"2026-10-29","base":"USD","quote":"EUR","rate":0.89847}]"#;
        let v1 = r#"{"amount":1.0,"base":"USD","date":"2026-10-29","rates":{"EUR":0.89847}}"#;
        let want = Rate {
            as_of: "2026-10-29".into(),
            rate_ppm: 898_470,
        };
        assert_eq!(parse(v2).unwrap(), want);
        assert_eq!(parse(v1).unwrap(), want);
        assert!(
            parse(r#"[{"date":"2026-10-29","base":"EUR","quote":"USD","rate":1.11}]"#).is_err()
        );
        assert!(parse(r#"{"base":"EUR","date":"2026-10-29","rates":{"EUR":1}}"#).is_err());
        assert!(parse(r#"[{"date":"2026-10-29","base":"USD","quote":"EUR","rate":92}]"#).is_err());
        assert!(parse(r#"[{"date":"yesterday","base":"USD","quote":"EUR","rate":0.9}]"#).is_err());
        assert!(parse("<html>").is_err());
    }

    #[test]
    fn a_stale_rate_gives_way_to_the_fallback() {
        let today = days_from_date("2026-10-29").unwrap();
        let fresh = answer(Some(("2026-10-23".into(), 900_000)), today, 1, "x");
        assert_eq!((fresh.eur_per_usd_ppm, fresh.fallback), (900_000, false));
        assert_eq!(fresh.source, "ecb");
        let stale = answer(
            Some(("2026-10-21".into(), 900_000)),
            today,
            898_470,
            "2026-10-29",
        );
        assert_eq!(
            stale,
            FxResponse {
                eur_per_usd_ppm: 898_470,
                as_of: "2026-10-29".into(),
                source: "fallback".into(),
                fallback: true,
                ..FxResponse::default()
            }
        );
        assert!(answer(None, today, 1, "x").fallback);
    }
}
