//! `GET /debug/hedge` (plan 3.A): the lifecycle script's read-only view of the hedge desk. It
//! reports what the running hedge desk trades to, and exists only off the live profile.

mod common;

use std::time::Duration;

use common::{TempDb, pascal_stub};
use desk::{Config, Profile};
use serde_json::Value;

async fn get(url: &str) -> (u16, Value) {
    let r = reqwest::get(url).await.unwrap();
    let s = r.status().as_u16();
    (s, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn debug_hedge_reports_the_running_hedge_desk() {
    let pascal = pascal_stub().await;
    let db = TempDb::new("e2e-debug");
    let mut cfg = Config::local(&pascal, db.path());
    cfg.shutdown_grace = Duration::from_secs(5);
    let running = desk::start(cfg).await.unwrap();
    let url = format!("http://{}/debug/hedge", running.addr);

    // The hedge desk registers itself when its service starts.
    let mut last = (0, Value::Null);
    for _ in 0..50 {
        last = get(&url).await;
        if last.0 == 200 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(last.0, 200, "{last:?}");
    for k in ["held", "target", "resting"] {
        assert!(last.1[k].is_object(), "{k}: {last:?}");
    }

    // `held` is the shared map the hedge desk trades on; with no ticket the target is 0.
    // The hedge desk's start-up recovery rewrites `held` once, possibly after this point under
    // a loaded test run, so keep writing until the route reflects it.
    let (mut s, mut v) = (0, Value::Null);
    for _ in 0..50 {
        running
            .state
            .held
            .write()
            .insert("NBA_26OCT28_PHIIND_ML.PHI".into(), -7);
        (s, v) = get(&url).await;
        if v["held"]["NBA_26OCT28_PHIIND_ML.PHI"] == -7 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(s, 200);
    assert_eq!(v["held"]["NBA_26OCT28_PHIIND_ML.PHI"], -7);
    assert_eq!(v["target"]["NBA_26OCT28_PHIIND_ML.PHI"], 0);
    assert_eq!(v["resting"]["NBA_26OCT28_PHIIND_ML.PHI"], 0);
    running.join().await;
}

#[tokio::test]
async fn debug_hedge_is_absent_on_live() {
    let pascal = pascal_stub().await;
    let db = TempDb::new("e2e-debug-live");
    let mut cfg = Config::local(&pascal, db.path());
    cfg.profile = Profile::Live;
    cfg.shutdown_grace = Duration::from_secs(5);
    let running = desk::start(cfg).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (s, _) = get(&format!("http://{}/debug/hedge", running.addr)).await;
    assert_eq!(s, 404);
    running.join().await;
}
