//! Plan 6.0c review gate, FX (U-4, l6-contracts L6-2): `GET /fx` serves the rate a local fake
//! frankfurter gave, stores it so a restart within the day does not ask again, keeps serving it
//! when the upstream goes down, and falls back to the configured rate (flagged) when there is no
//! fresh stored rate. No test here reaches anything but 127.0.0.1, except the `#[ignore]`d smoke
//! test: `cargo test -p desk --test fx real_frankfurter_smoke -- --ignored --nocapture`.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use common::{TempDb, pascal_stub};
use desk::api::types::FxResponse;
use desk::config::{FX_FALLBACK_AS_OF, FX_FALLBACK_PPM, FX_URL_DEFAULT};
use desk::{Config, Running};

#[derive(Clone, Default)]
struct Fake {
    calls: Arc<AtomicUsize>,
    down: Arc<AtomicBool>,
}

async fn rates(State(f): State<Fake>) -> axum::response::Response {
    f.calls.fetch_add(1, Ordering::SeqCst);
    if f.down.load(Ordering::SeqCst) {
        return (StatusCode::BAD_GATEWAY, "upstream down").into_response();
    }
    // Today's date, so the rate is fresh whenever the test runs.
    let today = desk::fx::date_from_days((desk::now_ms() / 86_400_000) as i64);
    let body = format!(r#"[{{"date":"{today}","base":"USD","quote":"EUR","rate":0.91234}}]"#);
    ([("content-type", "application/json")], body).into_response()
}

async fn fake() -> (String, Fake) {
    let f = Fake::default();
    let app = Router::new()
        .route("/v2/rates", get(rates))
        .with_state(f.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    (
        format!("http://{addr}/v2/rates?base=USD&quotes=EUR&providers=ECB"),
        f,
    )
}

async fn start(pascal: &str, db: &TempDb, fx_url: &str) -> Running {
    let mut cfg = Config::local(pascal, db.path());
    cfg.shutdown_grace = Duration::from_secs(5);
    cfg.fx_url = fx_url.to_owned();
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

/// Polls `/fx` until `ok` holds (the task fetches at start).
async fn until(desk: &Running, ok: impl Fn(&FxResponse) -> bool) -> FxResponse {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let r = fx(desk).await;
        if ok(&r) {
            return r;
        }
        assert!(tokio::time::Instant::now() < deadline, "/fx stuck at {r:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn the_fetched_rate_is_served_stored_and_kept_when_upstream_goes_down() {
    let pascal = pascal_stub().await;
    let (url, f) = fake().await;
    let db = TempDb::new("fx-fetch");
    let desk = start(&pascal, &db, &url).await;
    let r = until(&desk, |r| !r.fallback).await;
    assert_eq!(r.eur_per_usd_ppm, 912_340);
    assert_eq!(r.source, "ecb");
    assert!(desk::fx::days_from_date(&r.as_of).is_some());
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    desk.join().await;

    // A restart within the day asks nothing and serves the stored rate, even with the upstream
    // down.
    f.down.store(true, Ordering::SeqCst);
    let desk = start(&pascal, &db, &url).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(fx(&desk).await, r);
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    desk.join().await;
}

#[tokio::test]
async fn the_fallback_answers_when_the_fake_upstream_is_down() {
    let pascal = pascal_stub().await;
    let (url, f) = fake().await;
    f.down.store(true, Ordering::SeqCst);
    let db = TempDb::new("fx-down");
    let desk = start(&pascal, &db, &url).await;
    // The read happens (and fails) at start.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while f.calls.load(Ordering::SeqCst) == 0 {
        assert!(tokio::time::Instant::now() < deadline, "no read");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // 7.0c: no SOL URL in `Config::local`, so the SOL fallback (`sol::FALLBACK_USD_U`) at the fallback rate.
    let want = FxResponse {
        eur_per_usd_ppm: FX_FALLBACK_PPM,
        as_of: FX_FALLBACK_AS_OF.into(),
        source: "fallback".into(),
        fallback: true,
        eur_per_sol_ppm: desk::fx::sol::eur_per_sol_ppm(
            desk::fx::sol::FALLBACK_USD_U,
            FX_FALLBACK_PPM,
        ),
        sol_as_of_ms: 0,
        sol_source: "fallback".into(),
        sol_fallback: true,
    };
    assert_eq!(fx(&desk).await, want);
    // Nothing reachable at all (closed port) and nothing stored: the same fallback.
    let pool = desk.state.db.clone();
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fx_rates")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    desk.join().await;
    let db = TempDb::new("fx-closed");
    let desk = start(&pascal, &db, "http://127.0.0.1:1/v2/rates").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(fx(&desk).await, want);
    desk.join().await;
}

#[tokio::test]
async fn a_rate_older_than_a_week_gives_way_to_the_fallback() {
    let pascal = pascal_stub().await;
    let db = TempDb::new("fx-stale");
    // No fx_url: the desk never fetches (the Config::local default).
    let desk = start(&pascal, &db, "").await;
    let pool = desk.state.db.clone();
    let rate = desk::fx::Rate {
        as_of: "2020-01-02".into(),
        rate_ppm: 900_000,
    };
    desk::fx::store(&pool, &rate, "{}").await.unwrap();
    let r = fx(&desk).await;
    assert!(r.fallback && r.source == "fallback", "{r:?}");
    desk.join().await;
}

/// Reads the real frankfurter once (the default `DESK_FX_URL`).
#[tokio::test]
#[ignore = "reads the real api.frankfurter.dev"]
async fn real_frankfurter_smoke() {
    let client = desk::fx::client().unwrap();
    let (rate, raw) = desk::fx::fetch(&client, FX_URL_DEFAULT).await.unwrap();
    println!("{FX_URL_DEFAULT} → {raw}");
    println!("as_of {} eur_per_usd_ppm {}", rate.as_of, rate.rate_ppm);
    assert!((desk::fx::MIN_PPM..=desk::fx::MAX_PPM).contains(&rate.rate_ppm));
}
