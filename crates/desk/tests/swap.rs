//! `GET /tokens` and `POST /swap-quote` are mounted and answer with C4 error bodies when they
//! cannot quote: 503 with no reachable RPC, 422 for a malformed body. The happy path over a
//! real validator is `swap_chain.rs` (`just swap-chain-test`).

mod common;

use std::time::Duration;

use common::{TempDb, pascal_stub};
use desk::Config;

#[tokio::test]
async fn swap_routes_are_mounted_and_fail_closed() {
    let db = TempDb::new("swap");
    let mut cfg = Config::local(pascal_stub().await, db.path());
    // Nothing listens on port 9 of localhost: every pool read fails.
    cfg.solana_rpc_url = "http://127.0.0.1:9".into();
    cfg.shutdown_grace = Duration::from_secs(5);
    let running = desk::start(cfg).await.expect("desk starts");
    let base = format!("http://{}", running.addr);
    let http = reqwest::Client::new();

    let r = http.get(format!("{base}/tokens")).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 503);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["status"], "unavailable");

    let r = http
        .post(format!("{base}/swap-quote"))
        .header("content-type", "application/json")
        .body(r#"{"mint": 5}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 422);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["status"], "rejected");

    let r = http
        .post(format!("{base}/swap-quote"))
        .json(&serde_json::json!({"mint": "9n4tnNg5pREwfsMvkpaGNQ6hXrvq6onFvo6eCma34jjq", "amount_in": "1000"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 503);
    running.join().await;
}
