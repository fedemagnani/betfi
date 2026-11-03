//! The SOL price `/fx` serves (V-3, D-436; l7-contracts L7-1, step 7.0c).
//!
//! `eur_per_sol_ppm` = SOL/USD × the ECB EUR/USD rate of [`super::current`]. The default source
//! is Coinbase's keyless public spot price ([`COINBASE_URL_DEFAULT`]); Pyth Hermes answers are
//! still parsed when `DESK_SOL_URL` points at Hermes with a `DESK_SOL_API_KEY`.
//! [`run`] reads `DESK_SOL_URL` (default [`COINBASE_URL_DEFAULT`] on `demo` and `live`,
//! nothing on `dev` or in [`crate::Config::local`]) every `DESK_SOL_S` (60 s) and keeps the
//! answer in memory ([`SolCache`], in [`crate::state::Shared::sol`]). The route only reads that
//! cache: it never waits on the network, and nothing a quote does reads it, so a slow or dead
//! upstream can neither block nor fail a quote. A price whose Pyth publish time is more than
//! [`MAX_AGE_MS`] old (or none at all) gives way to the configured fixed price
//! (`DESK_SOL_FALLBACK_USD_U`, micro-USD per SOL), flagged `sol_fallback: true`,
//! `sol_source: "fallback"`, `sol_as_of_ms: 0`.
//!
//! Hermes is a read-only host; `DESK_SOL_API_KEY`, when set, is sent as `Authorization: Bearer`
//! and never logged ([`Secret`]). On 30 Oct 2026 Hermes answered `401 unauthorized` to the
//! keyless `/v2/updates/price/latest` (its `/v2/price_feeds` metadata stays open and confirms the
//! feed id), which is why the default moved to Coinbase (D-470, superseded at merge).

use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;

use crate::now_ms;
use crate::state::AppState;

/// Pyth's SOL/USD price feed id (`Crypto.SOL/USD`, checked against Hermes `/v2/price_feeds` on
/// 30 Oct 2026).
pub const SOL_USD_FEED: &str = "ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d";
/// Default `DESK_SOL_URL`: Coinbase's public SOL/USD spot price, no key
/// (`{"data":{"amount":"93.12","base":"SOL","currency":"USD"}}`).
pub const COINBASE_URL_DEFAULT: &str = "https://api.coinbase.com/v2/prices/SOL-USD/spot";
/// Hermes' latest parsed update of [`SOL_USD_FEED`] (needs a key since Oct 2026).
pub const HERMES_URL_DEFAULT: &str = "https://hermes.pyth.network/v2/updates/price/latest?ids%5B%5D=0xef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d&parsed=true";
/// Default `DESK_SOL_FALLBACK_USD_U`: $151.00 per SOL (≈ €135.67 at the default ECB fallback).
pub const FALLBACK_USD_U: u64 = 95_000_000;
/// A price published longer ago than this gives way to the fallback, ms.
pub const MAX_AGE_MS: u64 = 10 * 60_000;
/// Per-request timeout.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
/// Largest answer read, bytes.
pub const MAX_BODY: usize = 64 * 1024;
/// A price outside `[MIN_USD_U, MAX_USD_U]` micro-USD per SOL is refused as garbage.
pub const MIN_USD_U: u64 = 1_000_000;
/// See [`MIN_USD_U`].
pub const MAX_USD_U: u64 = 100_000_000_000;

/// A string that never shows in `Debug` output (the Hermes API key in [`crate::Config`]).
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// One SOL/USD price as read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SolPrice {
    /// Micro-USD per SOL.
    pub usd_u: u64,
    /// Publish time (Pyth) or read time (Coinbase), Unix ms.
    pub publish_ms: u64,
    /// `coinbase` or `pyth`.
    pub source: &'static str,
}

/// The latest good price. Its lock is taken alone and never across an `.await` (D-043).
#[derive(Debug, Default)]
pub struct SolCache {
    price: Mutex<Option<SolPrice>>,
}

impl SolCache {
    /// The cached price.
    pub fn get(&self) -> Option<SolPrice> {
        *self
            .price
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Replaces the cached price.
    pub fn set(&self, p: SolPrice) {
        *self
            .price
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(p);
    }
}

/// What `/fx` says about SOL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolAnswer {
    /// Micro-USD per SOL.
    pub usd_u: u64,
    /// Pyth publish time, Unix ms; 0 for the fallback.
    pub as_of_ms: u64,
    /// `coinbase`, `pyth` or `fallback`.
    pub source: &'static str,
    /// Whether it is the fallback.
    pub fallback: bool,
}

/// `cached` if published within [`MAX_AGE_MS`] of `now_ms`, else the fallback.
pub fn answer(cached: Option<SolPrice>, now_ms: u64, fallback_usd_u: u64) -> SolAnswer {
    match cached {
        Some(p) if now_ms.saturating_sub(p.publish_ms) <= MAX_AGE_MS => SolAnswer {
            usd_u: p.usd_u,
            as_of_ms: p.publish_ms,
            source: p.source,
            fallback: false,
        },
        _ => SolAnswer {
            usd_u: fallback_usd_u,
            as_of_ms: 0,
            source: "fallback",
            fallback: true,
        },
    }
}

/// [`answer`] for the running desk (no I/O).
pub fn current(state: &AppState) -> SolAnswer {
    answer(state.sol.get(), now_ms(), state.config.sol_fallback_usd_u)
}

/// EUR per SOL × 1e6 from micro-USD per SOL and EUR per USD × 1e6, truncated.
pub fn eur_per_sol_ppm(usd_u: u64, eur_per_usd_ppm: u64) -> u64 {
    u64::try_from(u128::from(usd_u) * u128::from(eur_per_usd_ppm) / 1_000_000).unwrap_or(u64::MAX)
}

/// The SOL/USD price in a Coinbase spot answer (read time `now_ms`) or a Hermes
/// `/v2/updates/price/latest?parsed=true` answer.
pub fn parse(body: &str) -> Result<SolPrice, String> {
    parse_at(body, now_ms())
}

/// [`parse`] with an explicit read time for Coinbase answers (which carry no timestamp).
pub fn parse_at(body: &str, read_ms: u64) -> Result<SolPrice, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("not JSON: {e}"))?;
    if v.get("data").is_some() {
        return parse_coinbase(&v, read_ms);
    }
    parse_hermes(&v)
}

/// `{"data":{"amount":"93.12","base":"SOL","currency":"USD"}}`.
fn parse_coinbase(v: &Value, read_ms: u64) -> Result<SolPrice, String> {
    let d = &v["data"];
    if d["base"].as_str() != Some("SOL") || d["currency"].as_str() != Some("USD") {
        return Err("not a SOL/USD spot price".into());
    }
    let amount = d["amount"].as_str().ok_or("no amount")?;
    let usd_u = pascal_rs::parse_micros(amount)
        .ok()
        .and_then(|m| u64::try_from(m).ok())
        .ok_or_else(|| format!("bad amount {amount:?}"))?;
    if !(MIN_USD_U..=MAX_USD_U).contains(&usd_u) {
        return Err(format!(
            "SOL at {usd_u} micro-USD is outside the sane range"
        ));
    }
    Ok(SolPrice {
        usd_u,
        publish_ms: read_ms,
        source: "coinbase",
    })
}

/// `{"parsed":[{"id","price":{"price":"<int>","expo":<i32>,"publish_time":<s>}}]}`.
fn parse_hermes(v: &Value) -> Result<SolPrice, String> {
    let item = v["parsed"]
        .as_array()
        .ok_or("no parsed prices")?
        .iter()
        .find(|i| {
            i["id"].as_str().is_some_and(|id| {
                id.trim_start_matches("0x")
                    .eq_ignore_ascii_case(SOL_USD_FEED)
            })
        })
        .ok_or("no SOL/USD item")?;
    let p = &item["price"];
    let raw: i128 = match &p["price"] {
        Value::String(s) => s.parse().map_err(|_| format!("bad price {s:?}"))?,
        Value::Number(n) => n.as_i64().map(i128::from).ok_or("bad price")?,
        _ => return Err("no price".into()),
    };
    let expo = p["expo"].as_i64().ok_or("no expo")?;
    let publish_s = p["publish_time"].as_u64().ok_or("no publish_time")?;
    if raw <= 0 || !(-18..=18).contains(&expo) {
        return Err(format!("price {raw}e{expo} is not sane"));
    }
    // micro-USD = raw × 10^(expo + 6), truncated.
    let shift = expo + 6;
    let scale = 10i128.pow(u32::try_from(shift.unsigned_abs()).unwrap_or(0));
    let micro = if shift >= 0 {
        raw.checked_mul(scale).ok_or("price overflows")?
    } else {
        raw / scale
    };
    let usd_u = u64::try_from(micro).map_err(|_| "price overflows".to_owned())?;
    if !(MIN_USD_U..=MAX_USD_U).contains(&usd_u) {
        return Err(format!(
            "SOL at {usd_u} micro-USD is outside the sane range"
        ));
    }
    Ok(SolPrice {
        usd_u,
        publish_ms: publish_s.saturating_mul(1000),
        source: "pyth",
    })
}

/// The client [`run`] reads with.
pub fn client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .user_agent(concat!("schedina-desk/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// One read of `url` (with `key` as a bearer token when set).
pub async fn fetch(
    client: &reqwest::Client,
    url: &str,
    key: Option<&Secret>,
) -> Result<SolPrice, String> {
    let mut req = client.get(url);
    if let Some(k) = key {
        req = req.bearer_auth(&k.0);
    }
    let mut resp = req
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
    parse(&text)
}

/// Reads the price every `sol_every` until shutdown, keeping the last good one. Idle when
/// `sol_url` is empty or `DESK_SOL_S=0` (`/fx` then serves the fallback).
pub async fn run(state: AppState) {
    if state.config.sol_url.is_empty() || state.config.sol_every.is_zero() {
        tracing::info!("fx: SOL price not fetched (no DESK_SOL_URL or DESK_SOL_S=0); fallback");
        state.shutdown_requested().await;
        return;
    }
    let client = match client() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "fx: cannot build the SOL client; fallback");
            state.shutdown_requested().await;
            return;
        }
    };
    let mut tick = tokio::time::interval(state.config.sol_every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = state.shutdown_requested() => break,
            _ = tick.tick() => {}
        }
        let r = tokio::select! {
            _ = state.shutdown_requested() => break,
            r = fetch(&client, &state.config.sol_url, state.config.sol_api_key.as_ref()) => r,
        };
        match r {
            Ok(p) => {
                metrics::counter!("desk_sol_fetch_total", "result" => "ok").increment(1);
                tracing::debug!(usd_u = p.usd_u, publish_ms = p.publish_ms, "SOL price read");
                state.sol.set(p);
            }
            Err(e) => {
                metrics::counter!("desk_sol_fetch_total", "result" => "error").increment(1);
                tracing::warn!(error = %e, "fx: SOL price read failed; keeping the last one or the fallback");
            }
        }
    }
    tracing::info!("fx SOL price stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hermes(price: &str, expo: i64, publish: u64) -> String {
        format!(
            r#"{{"binary":{{"encoding":"hex","data":["00"]}},"parsed":[{{"id":"{SOL_USD_FEED}",
            "price":{{"price":"{price}","conf":"1","expo":{expo},"publish_time":{publish}}},
            "ema_price":{{"price":"1","conf":"1","expo":-8,"publish_time":{publish}}},
            "metadata":{{"slot":1}}}}]}}"#
        )
    }

    #[test]
    fn hermes_answers_parse() {
        let p = parse(&hermes("15123456789", -8, 1_793_300_000)).unwrap();
        assert_eq!(
            p,
            SolPrice {
                usd_u: 151_234_567,
                publish_ms: 1_793_300_000_000,
                source: "pyth"
            }
        );
        assert_eq!(parse(&hermes("151", 0, 1)).unwrap().usd_u, 151_000_000);
        assert!(parse(&hermes("-1", -8, 1)).is_err());
        assert!(parse(&hermes("1", -8, 1)).is_err(), "a millionth of a cent");
        assert!(parse(&hermes("15123456789", -40, 1)).is_err());
        assert!(parse(r#"{"parsed":[]}"#).is_err());
        assert!(parse(&hermes("15123456789", -8, 1).replace(SOL_USD_FEED, "ab")).is_err());
        assert!(parse("unauthorized").is_err());
    }

    #[test]
    fn coinbase_answers_parse() {
        let body = r#"{"data":{"amount":"93.125","base":"SOL","currency":"USD"}}"#;
        let p = parse_at(body, 1_793_300_000_000).unwrap();
        assert_eq!(
            p,
            SolPrice {
                usd_u: 93_125_000,
                publish_ms: 1_793_300_000_000,
                source: "coinbase"
            }
        );
        assert!(
            parse_at(
                r#"{"data":{"amount":"93","base":"SOL","currency":"EUR"}}"#,
                1
            )
            .is_err()
        );
        assert!(
            parse_at(
                r#"{"data":{"amount":"x","base":"SOL","currency":"USD"}}"#,
                1
            )
            .is_err()
        );
        assert!(
            parse_at(
                r#"{"data":{"amount":"0","base":"SOL","currency":"USD"}}"#,
                1
            )
            .is_err()
        );
    }

    #[test]
    fn a_stale_price_gives_way_to_the_fallback() {
        let p = SolPrice {
            usd_u: 150_000_000,
            publish_ms: 1_000_000,
            source: "pyth",
        };
        let fresh = answer(Some(p), 1_000_000 + MAX_AGE_MS, 9);
        assert_eq!(
            (fresh.usd_u, fresh.source, fresh.fallback),
            (150_000_000, "pyth", false)
        );
        assert_eq!(fresh.as_of_ms, 1_000_000);
        let stale = answer(Some(p), 1_000_001 + MAX_AGE_MS, 9);
        assert_eq!(
            stale,
            SolAnswer {
                usd_u: 9,
                as_of_ms: 0,
                source: "fallback",
                fallback: true
            }
        );
        assert!(answer(None, 0, 9).fallback);
        // $151.20 at 0.89847 EUR per USD: €135.848664.
        assert_eq!(eur_per_sol_ppm(151_200_000, 898_470), 135_848_664);
        assert_eq!(format!("{:?}", Secret("k".into())), "Secret(<redacted>)");
    }
}
