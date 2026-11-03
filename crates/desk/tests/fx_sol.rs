//! Plan 7.0c review gate, SOL price (V-3, l7-contracts L7-1): `GET /fx` serves
//! `eur_per_sol_ppm` = SOL/USD (a local fake Pyth Hermes) × the EUR/USD rate, with
//! `sol_as_of_ms`, `sol_source` and `sol_fallback`; the fixed fallback when the upstream is down,
//! refuses, answers garbage or a stale price; and `/fx` answers at once while the upstream hangs.
//! Nothing here reaches anything but 127.0.0.1, except the `#[ignore]`d smoke test:
//! `cargo test -p desk --test fx_sol real_hermes_smoke -- --ignored --nocapture`.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use common::{TempDb, pascal_stub};
use desk::api::types::FxResponse;
use desk::config::FX_FALLBACK_PPM;
use desk::fx::sol::{self, SOL_USD_FEED, Secret};
use desk::{Config, Running};

/// What the fake answers: 0 a fresh price, 1 HTTP 401, 2 garbage, 3 a 20-minute-old price,
/// 4 hang for a minute.
#[derive(Clone, Default)]
struct Fake {
    calls: Arc<AtomicUsize>,
    mode: Arc<AtomicU8>,
    /// Require `Authorization: Bearer k`.
    keyed: bool,
}

/// $151.23456789 at `publish_s`.
fn hermes_body(publish_s: u64) -> String {
    format!(
        r#"{{"binary":{{"encoding":"hex","data":["504e4155"]}},"parsed":[{{"id":"{SOL_USD_FEED}",
        "price":{{"price":"15123456789","conf":"7000000","expo":-8,"publish_time":{publish_s}}},
        "ema_price":{{"price":"15100000000","conf":"7000000","expo":-8,"publish_time":{publish_s}}},
        "metadata":{{"slot":1,"proof_available_time":{publish_s},"prev_publish_time":{publish_s}}}}}]}}"#
    )
}

async fn latest(State(f): State<Fake>, headers: HeaderMap) -> axum::response::Response {
    f.calls.fetch_add(1, Ordering::SeqCst);
    if f.keyed && headers.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer k") {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let now_s = desk::now_ms() / 1000;
    match f.mode.load(Ordering::SeqCst) {
        0 => ([("content-type", "application/json")], hermes_body(now_s)).into_response(),
        1 => (StatusCode::UNAUTHORIZED, "unauthorized").into_response(),
        2 => ([("content-type", "application/json")], "{\"parsed\":[]}").into_response(),
        3 => (
            [("content-type", "application/json")],
            hermes_body(now_s - 1200),
        )
            .into_response(),
        _ => {
            tokio::time::sleep(Duration::from_secs(60)).await;
            "late".into_response()
        }
    }
}

async fn fake(mode: u8, keyed: bool) -> (String, Fake) {
    let f = Fake {
        keyed,
        ..Fake::default()
    };
    f.mode.store(mode, Ordering::SeqCst);
    let app = Router::new()
        .route("/v2/updates/price/latest", get(latest))
        .with_state(f.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    (
        format!("http://{addr}/v2/updates/price/latest?ids%5B%5D=0x{SOL_USD_FEED}&parsed=true"),
        f,
    )
}

async fn start(db: &TempDb, sol_url: &str, key: Option<&str>) -> Running {
    let pascal = pascal_stub().await;
    let mut cfg = Config::local(&pascal, db.path());
    cfg.shutdown_grace = Duration::from_secs(5);
    cfg.sol_url = sol_url.to_owned();
    cfg.sol_every = Duration::from_millis(200);
    cfg.sol_api_key = key.map(|k| Secret(k.to_owned()));
    desk::start(cfg).await.expect("desk starts")
}

async fn fx(desk: &Running) -> FxResponse {
    reqwest::get(format!("http://{}/fx", desk.addr))
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn until_calls(f: &Fake, n: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while f.calls.load(Ordering::SeqCst) < n {
        assert!(Instant::now() < deadline, "no read of the fake");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The SOL fallback at the EUR/USD fallback: `sol::FALLBACK_USD_U` × 0.89847.
const FALLBACK_EUR_PER_SOL: u64 = desk::fx::sol::FALLBACK_USD_U * 898_470 / 1_000_000;

#[tokio::test]
async fn a_fresh_pyth_price_is_served_in_euros() {
    let (url, f) = fake(0, true).await;
    let db = TempDb::new("fx-sol");
    let desk = start(&db, &url, Some("k")).await;
    until_calls(&f, 1).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    let r = loop {
        let r = fx(&desk).await;
        if !r.sol_fallback {
            break r;
        }
        assert!(Instant::now() < deadline, "/fx stuck at {r:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(r.sol_source, "pyth");
    // $151.234567 (micro-USD, truncated) × 0.89847 EUR per USD.
    assert_eq!(r.eur_per_usd_ppm, FX_FALLBACK_PPM);
    assert_eq!(r.eur_per_sol_ppm, 151_234_567 * 898_470 / 1_000_000);
    assert!(r.sol_as_of_ms.abs_diff(desk::now_ms()) < 10_000, "{r:?}");
    assert_eq!(r.sol_as_of_ms % 1000, 0);
    // The EUR/USD half is unchanged (fallback here: no DESK_FX_URL).
    assert!(r.fallback && r.source == "fallback");
    // The exact L7-1 keys.
    let v: serde_json::Value = reqwest::get(format!("http://{}/fx", desk.addr))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    println!("served GET /fx: {v}");
    for k in [
        "eur_per_usd_ppm",
        "as_of",
        "source",
        "fallback",
        "eur_per_sol_ppm",
        "sol_as_of_ms",
        "sol_source",
        "sol_fallback",
    ] {
        assert!(v.get(k).is_some(), "{k}");
    }

    // The upstream starts refusing: the last good price stays (until it is 10 minutes old).
    f.mode.store(1, Ordering::SeqCst);
    let n = f.calls.load(Ordering::SeqCst);
    until_calls(&f, n + 2).await;
    assert_eq!(fx(&desk).await.eur_per_sol_ppm, r.eur_per_sol_ppm);
    desk.join().await;
}

#[tokio::test]
async fn the_fallback_answers_when_the_upstream_fails() {
    // Refused without the key, garbage, a stale price: each the flagged fallback.
    for (mode, keyed, tag) in [
        (0, true, "nokey"),
        (2, false, "garbage"),
        (3, false, "stale"),
    ] {
        let (url, f) = fake(mode, keyed).await;
        let db = TempDb::new(&format!("fx-sol-{tag}"));
        let desk = start(&db, &url, None).await;
        until_calls(&f, 2).await;
        let r = fx(&desk).await;
        assert_eq!(
            (
                r.eur_per_sol_ppm,
                r.sol_as_of_ms,
                r.sol_source.as_str(),
                r.sol_fallback
            ),
            (FALLBACK_EUR_PER_SOL, 0, "fallback", true),
            "{tag}"
        );
        desk.join().await;
    }
    // No URL at all (the `Config::local` default): never fetched, the fallback.
    let db = TempDb::new("fx-sol-off");
    let desk = start(&db, "", None).await;
    let r = fx(&desk).await;
    assert!(r.sol_fallback && r.eur_per_sol_ppm == FALLBACK_EUR_PER_SOL);
    desk.join().await;
}

#[tokio::test]
async fn fx_never_waits_on_a_hanging_upstream() {
    let (url, f) = fake(4, false).await;
    let db = TempDb::new("fx-sol-hang");
    let desk = start(&db, &url, None).await;
    until_calls(&f, 1).await;
    let t0 = Instant::now();
    let r = fx(&desk).await;
    assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
    assert!(r.sol_fallback);
    // A quote-path read of the price is the same memory read.
    assert!(sol::current(&desk.state).fallback);
    desk.join().await;
}

/// Reads the real Pyth Hermes once (`DESK_SOL_API_KEY` from the environment when set) and prints
/// EUR per SOL at the real ECB rate (or the fallback rate if frankfurter is unreachable).
#[tokio::test]
#[ignore = "reads the real hermes.pyth.network (and api.frankfurter.dev)"]
async fn real_hermes_smoke() {
    let client = sol::client().unwrap();
    let key = std::env::var("DESK_SOL_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
        .map(Secret);
    let eur_per_usd =
        match desk::fx::fetch(&desk::fx::client().unwrap(), desk::config::FX_URL_DEFAULT).await {
            Ok((r, _)) => r.rate_ppm,
            Err(e) => {
                println!("frankfurter: {e}; using the fallback rate");
                FX_FALLBACK_PPM
            }
        };
    let r = sol::fetch(&client, sol::COINBASE_URL_DEFAULT, key.as_ref()).await;
    match &r {
        Ok(p) => println!(
            "SOL/USD {} (published {} ms) × EUR/USD {} → EUR per SOL {}",
            p.usd_u as f64 / 1e6,
            p.publish_ms,
            eur_per_usd as f64 / 1e6,
            sol::eur_per_sol_ppm(p.usd_u, eur_per_usd) as f64 / 1e6
        ),
        Err(e) => println!("hermes: {e} (key set: {})", key.is_some()),
    }
    r.unwrap();
}
