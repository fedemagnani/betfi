//! The mock over HTTP, read through `pascal-rs`'s unmodified `Client` (review gate of plan 1.D).
//! Everything runs on 127.0.0.1; no network.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use pascal_mock::{Mock, MockConfig, RunningMock, ScriptSource};
use pascal_rs::ratelimit::{Backoff, BucketConfig, RateLimits, RetryPolicy};
use pascal_rs::{
    AccountQuery, Client, Fill, MarketSearch, Order, OrderUpdateReason, PageParams, ResolvedQuery,
    Side, decode_response,
};
use serde::Deserialize;
use serde_json::{Value, json};

const PHI: &str = "NBA_26OCT28_PHIIND_ML.PHI";
const HOU: &str = "NBA_26OCT28_HOUMIL_ML.HOU";
const SYMBOLS: [&str; 5] = [
    "NBA_26OCT28_PHIIND_ML.PHI",
    "NBA_26OCT28_ORLTOR_ML.ORL",
    "NBA_26OCT28_HOUMIL_ML.HOU",
    "NBA_26OCT28_SASUTA_ML.SAS",
    "NBA_26OCT28_OKCDAL_ML.OKC",
];
/// The sample tape's two books snapshots.
const SNAP0: u64 = 1_793_212_987_458;
const SNAP1: u64 = 1_793_213_017_476;
const USD: i64 = 1_000_000;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sample_cfg() -> MockConfig {
    let mut cfg = MockConfig::new(root().join("tapes/nba-2026-10-28/sample"));
    cfg.speed = 0.0;
    cfg.script = ScriptSource::None;
    cfg
}

async fn start(cfg: MockConfig) -> (RunningMock, Client, reqwest::Client) {
    let running = Mock::new(cfg).unwrap().spawn("127.0.0.1:0").await.unwrap();
    let client = Client::builder()
        .base_url(running.base_url())
        .rate_limits(RateLimits {
            per_host: BucketConfig {
                per_second: 1_000.0,
                burst: 1_000.0,
            },
            per_route: HashMap::new(),
        })
        .retry(RetryPolicy {
            max_attempts: 1,
            backoff: Backoff {
                base: Duration::from_millis(1),
                max: Duration::from_millis(1),
            },
        })
        .build()
        .unwrap();
    (running, client, reqwest::Client::new())
}

fn key(n: u8) -> String {
    bs58::encode([n; 32]).into_string()
}

/// A place-order element as `pascal-sign.js` builds it (signature not checked in 1.D).
fn order(owner: &str, cid: u64, symbol: &str, side: &str, price: &str, size: u64) -> Value {
    json!({
        "client_order_id": cid.to_string(), "symbol": symbol, "side": side, "price": price,
        "size": size.to_string(), "tif": "IOC",
        "builder_info": {"builder_pubkey": key(99), "builder_fee_rate": "0.001000"},
        "auth": {"client_ts_ms": "1793213000000", "recv_window_ms": "5000", "owner": owner,
                 "signer": key(200), "signature": bs58::encode([7u8; 64]).into_string()},
    })
}

#[derive(Debug, Deserialize)]
struct PlaceData {
    order: Order,
    fills: Vec<Fill>,
    #[serde(default)]
    update_reason: Option<OrderUpdateReason>,
}

/// POSTs a batch and decodes each element with `pascal-rs`'s envelope decoder.
async fn post_orders(
    http: &reqwest::Client,
    base: &str,
    path: &str,
    batch: Value,
) -> (u16, Vec<Value>) {
    let resp = http
        .post(format!("{base}{path}"))
        .json(&batch)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap();
    (
        status,
        body.as_array().cloned().unwrap_or_else(|| vec![body]),
    )
}

fn place_data(item: &Value) -> pascal_rs::Result<PlaceData> {
    let bytes = serde_json::to_vec(item).unwrap();
    decode_response::<PlaceData>(200, &bytes, "orders item").map(|r| r.data)
}

#[tokio::test]
async fn sample_tape_reads_decode_with_pascal_rs() {
    let (m, client, http) = start(sample_cfg()).await;

    let t = client.time().await.unwrap();
    assert_eq!(t.data.server_time_ms, SNAP0);

    let markets = client.markets().await.unwrap();
    let mut got: Vec<&str> = markets.data.iter().map(|m| m.symbol.as_str()).collect();
    got.sort_unstable();
    let mut want = SYMBOLS.to_vec();
    want.sort_unstable();
    assert_eq!(got, want);
    let phi = markets.data.iter().find(|m| m.symbol == PHI).unwrap();
    assert_eq!(phi.tick_sig_figs, 3);
    assert_eq!(phi.best_ask, Some(699_000));
    assert_eq!(phi.best_bid, Some(691_000));

    let books = client.books(&SYMBOLS).await.unwrap();
    assert_eq!(books.data.books.len(), 5);
    let b = &books.data.books[PHI];
    assert_eq!((b.asks[0].price, b.asks[0].size), (699_000, 35_200));
    assert_eq!((b.bids[0].price, b.bids[0].size), (691_000, 50_000));
    assert!(b.asks.windows(2).all(|w| w[0].price < w[1].price));
    assert!(b.bids.windows(2).all(|w| w[0].price > w[1].price));
    // D-009: no `spec`, like Pascal.
    assert!(books.raw["data"]["books"][PHI].get("spec").is_none());
    assert!(books.seq.unwrap() >= 12_216_606_546);
    assert_eq!(books.state_time_ms, Some(SNAP0));

    let found = client
        .search_markets(&MarketSearch {
            symbols: vec![PHI.into()],
            ..MarketSearch::default()
        })
        .await
        .unwrap();
    assert_eq!(found.data.len(), 1);
    assert!(found.data[0].resolution.is_none());
    let by_game = client
        .search_markets(&MarketSearch {
            games: vec!["nba-hou-mil-2026-10-28".into()],
            ..MarketSearch::default()
        })
        .await
        .unwrap();
    assert_eq!(by_game.data[0].symbol, HOU);
    assert!(
        client
            .resolved_markets(ResolvedQuery::default())
            .await
            .unwrap()
            .data
            .is_empty()
    );

    // The tape's /trades response was recorded after the first books snapshot: not visible yet.
    let page = client
        .trades(&SYMBOLS, &PageParams::limit(2))
        .await
        .unwrap();
    assert!(page.data.items.is_empty());

    let acct = client
        .account(
            &key(1),
            &AccountQuery {
                symbol: None,
                exclude_orders: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(acct.data.collateral_usd, Some(10_000 * USD));
    assert!(acct.data.positions.is_empty());

    let text: Value = http
        .post(format!("{}/api/v1/markets/text-search", m.base_url()))
        .json(&json!({"query": "76ers pacers"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(text["data"][0]["symbol"], PHI);
    assert_eq!(text["data"].as_array().unwrap().len(), 1);

    // Clock control: the second snapshot's book.
    let clock: Value = http
        .post(format!("{}/mock/clock", m.base_url()))
        .json(&json!({"tape_ms": SNAP1}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(clock["data"]["book_snapshot_ms"], SNAP1);
    let b = client.books(&[PHI]).await.unwrap();
    assert_eq!(b.data.books[PHI].asks[0].size, 20_741);

    let page = client
        .trades(&SYMBOLS, &PageParams::limit(2))
        .await
        .unwrap();
    assert_eq!(page.data.items.len(), 2);
    assert!(page.data.items[0].seq > page.data.items[1].seq);
    let next = PageParams {
        limit: Some(2),
        before_cursor: page.data.next_cursor.clone(),
        at_or_before_seq: None,
    };
    let page2 = client.trades(&SYMBOLS, &next).await.unwrap();
    assert!(page2.data.items[0].seq < page.data.items[1].seq);
    assert!(page2.data.next_cursor.is_none());
}

#[tokio::test]
async fn ioc_bid_at_best_ask_fills_and_the_account_shows_it() {
    let (m, client, http) = start(sample_cfg()).await;
    let a = key(1);
    let (status, items) = post_orders(
        &http,
        &m.base_url(),
        "/api/v1/orders",
        json!([order(&a, 1, PHI, "BID", "0.699000", 10)]),
    )
    .await;
    assert_eq!(status, 200);
    assert!(items[0].get("server_receive_time_ms").is_some());
    let d = place_data(&items[0]).unwrap();
    assert_eq!(d.fills.len(), 1);
    let f = &d.fills[0];
    assert_eq!(
        (f.side, f.fill_price, f.fill_size),
        (Side::Bid, 699_000, 10)
    );
    // 10 × 0.699 × 0.301 × 0.02 = 0.0420798; builder at 0.1%: 0.00210399.
    assert_eq!(f.fee_usd, 42_079);
    assert_eq!(f.builder_fee_paid_usd, Some(2_103));
    assert_eq!(f.collateral_change_usd, -6_990_000 - 42_079 - 2_103);
    assert_eq!(d.order.size_filled, 10);
    assert_eq!(d.update_reason, None);
    assert!(
        items[0].get("data").unwrap()["order"]
            .get("replace_client_order_id")
            .is_none()
    );

    let acct = client.account(&a, &AccountQuery::default()).await.unwrap();
    assert_eq!(acct.data.positions.len(), 1);
    assert_eq!(acct.data.positions[0].symbol, PHI);
    assert_eq!(acct.data.positions[0].size, 10);
    assert_eq!(
        acct.data.collateral_usd,
        Some(10_000 * USD - 6_990_000 - 42_079 - 2_103)
    );
    let builder = client
        .account(&key(99), &AccountQuery::default())
        .await
        .unwrap();
    assert_eq!(
        builder.data.rewards.unwrap().builder_fees_received_usd,
        2_103
    );
    let b = client.books(&[PHI]).await.unwrap();
    assert_eq!(b.data.books[PHI].asks[0].size, 35_190);
    // The fill shows on the public tape.
    let trades = client.trades(&[PHI], &PageParams::limit(1)).await.unwrap();
    assert_eq!(trades.data.items[0].trade_id, f.trade_id);
    assert_eq!(trades.data.items[0].taker, a);
}

#[tokio::test]
async fn partial_fill_when_depth_is_short() {
    let (m, client, http) = start(sample_cfg()).await;
    let a = key(2);
    // HOU's best bid is 293 @ 0.760; the next is 0.748.
    let (status, items) = post_orders(
        &http,
        &m.base_url(),
        "/api/v1/orders",
        json!([order(&a, 1, HOU, "ASK", "0.760000", 500)]),
    )
    .await;
    assert_eq!(status, 200);
    let d = place_data(&items[0]).unwrap();
    assert_eq!(d.order.size_filled, 293);
    assert_eq!(d.order.size_remaining, 0);
    assert_eq!(
        d.update_reason,
        Some(OrderUpdateReason::IocRemainderCanceled)
    );
    let acct = client.account(&a, &AccountQuery::default()).await.unwrap();
    assert_eq!(acct.data.positions[0].size, -293);
    // Short lock 293 × 0.24, fee 293 × 0.76 × 0.24 × 0.02, builder 293 × 0.76 × 0.24 × 0.001.
    assert_eq!(
        acct.data.collateral_usd,
        Some(10_000 * USD - 70_320_000 - 1_068_864 - 53_443)
    );
}

#[tokio::test]
async fn scripted_resolution_appears_in_search_and_resolved() {
    let dir = std::env::temp_dir().join(format!("pascal-mock-script-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("script.toml");
    std::fs::write(
        &script,
        format!("[[resolve]]\nsymbol = \"{PHI}\"\nprice = \"1\"\nat = \"start+1h\"\n"),
    )
    .unwrap();
    let mut cfg = sample_cfg();
    cfg.script = ScriptSource::File(script);
    let (m, client, http) = start(cfg).await;

    // A long position that the resolution will pay.
    let a = key(3);
    post_orders(
        &http,
        &m.base_url(),
        "/api/v1/orders",
        json!([order(&a, 1, PHI, "BID", "0.699000", 10)]),
    )
    .await;
    let before = client.account(&a, &AccountQuery::default()).await.unwrap();
    let search = MarketSearch {
        symbols: vec![PHI.into()],
        ..MarketSearch::default()
    };
    assert!(
        client.search_markets(&search).await.unwrap().data[0]
            .resolution
            .is_none()
    );

    http.post(format!("{}/mock/clock", m.base_url()))
        .json(&json!({"advance_ms": 3_600_000}))
        .send()
        .await
        .unwrap();
    let found = client.search_markets(&search).await.unwrap();
    let r = found.data[0].resolution.as_ref().unwrap();
    assert_eq!(r.price, 1_000_000);
    assert_eq!(r.market_resolution_ts_ms, SNAP0 + 3_600_000);
    let resolved = client
        .resolved_markets(ResolvedQuery::default())
        .await
        .unwrap();
    assert_eq!(resolved.data.len(), 1);
    assert_eq!(resolved.data[0].symbol, PHI);
    assert_eq!(client.markets().await.unwrap().data.len(), 4);
    let b = client.books(&[PHI]).await.unwrap();
    assert!(b.data.books[PHI].asks.is_empty() && b.data.books[PHI].bids.is_empty());
    let after = client.account(&a, &AccountQuery::default()).await.unwrap();
    assert!(after.data.positions.is_empty());
    assert_eq!(
        after.data.collateral_usd.unwrap() - before.data.collateral_usd.unwrap(),
        10 * USD
    );

    // Control endpoint, immediate.
    let v: Value = http
        .post(format!("{}/mock/resolve", m.base_url()))
        .json(&json!({"symbol": HOU, "price": 0}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["data"]["resolution"]["price"], "0.000000");
    let resolved = client
        .resolved_markets(ResolvedQuery::default())
        .await
        .unwrap();
    let mut syms: Vec<&str> = resolved.data.iter().map(|m| m.symbol.as_str()).collect();
    syms.sort_unstable();
    assert_eq!(syms, vec![HOU, PHI]);
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn errors_real_and_injected() {
    let (m, client, http) = start(sample_cfg()).await;
    let base = m.base_url();
    let a = key(4);

    // Off-tick price: every item failed, so the batch is HTTP 422 with per-item envelopes.
    let (status, items) = post_orders(
        &http,
        &base,
        "/api/v1/orders",
        json!([order(&a, 1, PHI, "BID", "0.699500", 1)]),
    )
    .await;
    assert_eq!(status, 422);
    assert_eq!(items[0]["data"]["code"], "INVALID_PRICE");
    let err = place_data(&items[0]).unwrap_err();
    assert_eq!(err.api_code(), Some("INVALID_PRICE"));

    // Mixed batch: HTTP 200, one success and one failure, in request order.
    let (status, items) = post_orders(
        &http,
        &base,
        "/api/v1/orders",
        json!([
            order(&a, 2, PHI, "BID", "0.600000", 1),
            order(&a, 3, "NOPE.X", "BID", "0.600000", 1)
        ]),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(items[0]["status"], "success");
    assert_eq!(items[1]["data"]["code"], "SYMBOL_NOT_FOUND");

    for code in [
        "INVALID_PRICE",
        "WOULD_CROSS",
        "INSUFFICIENT_COLLATERAL",
        "UNAUTHORIZED",
    ] {
        let (status, items) = post_orders(
            &http,
            &base,
            &format!("/api/v1/orders?inject={code}"),
            json!([
                order(&a, 4, PHI, "BID", "0.600000", 1),
                order(&a, 5, PHI, "BID", "0.600000", 1)
            ]),
        )
        .await;
        assert_eq!(items.len(), 2, "{code}");
        assert!(items.iter().all(|i| i["data"]["code"] == code));
        assert_eq!(status, if code == "UNAUTHORIZED" { 401 } else { 422 });
    }
    let (status, items) = post_orders(
        &http,
        &base,
        "/api/v1/orders?inject=RATE_LIMITED",
        json!([order(&a, 4, PHI, "BID", "0.600000", 1)]),
    )
    .await;
    assert_eq!(status, 429);
    assert_eq!(items[0]["status"], "error");
    assert_eq!(items[0]["data"]["code"], "RATE_LIMITED");
    // Nothing was placed by the injected requests.
    let acct = client.account(&a, &AccountQuery::default()).await.unwrap();
    assert_eq!(acct.data.orders.len(), 0);

    // Queued injection seen by the unmodified client, once.
    http.post(format!("{base}/mock/inject"))
        .json(&json!({"code": "RATE_LIMITED", "path": "/api/v1/books"}))
        .send()
        .await
        .unwrap();
    let err = client.books(&[PHI]).await.unwrap_err();
    assert_eq!(err.api_code(), Some("RATE_LIMITED"));
    assert_eq!(err.http_status(), Some(429));
    assert!(client.books(&[PHI]).await.is_ok());

    // Unknown cancel.
    let (status, items) = post_orders(
        &http,
        &base,
        "/api/v1/cancels",
        json!([{"client_order_id": "999", "auth": order(&a, 0, PHI, "BID", "0.5", 1)["auth"]}]),
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(items[0]["data"]["code"], "ORDER_NOT_FOUND");

    // Not JSON.
    let resp = http
        .post(format!("{base}/api/v1/orders"))
        .body("[{")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);
}

#[tokio::test]
async fn resting_order_cancel_and_mass_cancel_over_http() {
    let (m, client, http) = start(sample_cfg()).await;
    let base = m.base_url();
    let a = key(5);
    let mut gtc = order(&a, 10, PHI, "BID", "0.650000", 4);
    gtc["tif"] = json!("GTC");
    let mut gtc2 = order(&a, 11, PHI, "BID", "0.640000", 4);
    gtc2["tif"] = json!("GTC");
    let (_, items) = post_orders(&http, &base, "/api/v1/orders", json!([gtc, gtc2])).await;
    assert_eq!(place_data(&items[0]).unwrap().order.size_remaining, 4);
    let acct = client.account(&a, &AccountQuery::default()).await.unwrap();
    assert_eq!(acct.data.orders.len(), 2);
    let excl = client
        .account(
            &a,
            &AccountQuery {
                symbol: None,
                exclude_orders: true,
            },
        )
        .await
        .unwrap();
    assert!(excl.data.orders.is_empty());
    let auth = order(&a, 0, PHI, "BID", "0.5", 1)["auth"].clone();
    let (status, items) = post_orders(
        &http,
        &base,
        "/api/v1/cancels",
        json!([{"client_order_id": "10", "auth": auth}]),
    )
    .await;
    assert_eq!(status, 200);
    let o: Order = serde_json::from_value(items[0]["data"].clone()).unwrap();
    assert_eq!((o.client_order_id, o.size_remaining), (10, 0));
    let (status, items) = post_orders(
        &http,
        &base,
        "/api/v1/mass-cancels",
        json!([{"symbol": PHI, "auth": auth}]),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(items[0]["data"]["orders"].as_array().unwrap().len(), 1);
    let acct = client.account(&a, &AccountQuery::default()).await.unwrap();
    assert!(acct.data.orders.is_empty());
}

/// The sibling's verbatim fixtures, wrapped as a tape, served and decoded by `pascal-rs`.
#[tokio::test]
async fn sibling_fixtures_served_through_the_mock_decode() {
    let fx = root().join("fixtures/pascal");
    let dir = std::env::temp_dir().join(format!("pascal-mock-fixtures-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut last_ms = 0;
    for (file, fixture) in [
        ("books.jsonl", "rest_books.json"),
        ("markets.jsonl", "rest_markets.json"),
        ("search.jsonl", "rest_markets_search.json"),
        ("trades.jsonl", "rest_trades_page1.json"),
    ] {
        let body: Value =
            serde_json::from_str(&std::fs::read_to_string(fx.join(fixture)).unwrap()).unwrap();
        let t: u64 = body["state_time_ms"].as_str().unwrap().parse().unwrap();
        last_ms = last_ms.max(t);
        let line = json!({"recv_ms": t, "method": "GET", "path": "/", "http": 200, "body": body});
        std::fs::write(dir.join(file), format!("{line}\n")).unwrap();
    }
    let mut cfg = MockConfig::new(&dir);
    cfg.start_ms = Some(last_ms);
    cfg.speed = 0.0;
    cfg.script = ScriptSource::None;
    let (_m, client, _http) = start(cfg).await;

    let markets = client.markets().await.unwrap();
    assert!(markets.data.len() >= 45, "{}", markets.data.len());
    let raw_books: Value =
        serde_json::from_str(&std::fs::read_to_string(fx.join("rest_books.json")).unwrap())
            .unwrap();
    let symbols: Vec<String> = raw_books["data"]["books"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let books = client.books(&symbols).await.unwrap();
    assert_eq!(books.data.books.len(), symbols.len());
    let want: pascal_rs::Books = serde_json::from_value(raw_books["data"].clone()).unwrap();
    assert_eq!(books.data, want, "books served verbatim");

    let search = client
        .search_markets(&MarketSearch {
            symbols: vec![
                "TX_GOV_2026.REP".into(),
                "US_IRAN_CFR_THRU.26OCT24".into(),
                "NBA_26OCT26_GSWDEN_ML.GSW".into(),
            ],
            ..MarketSearch::default()
        })
        .await
        .unwrap();
    assert_eq!(search.data.len(), 3);
    let gsw = search
        .data
        .iter()
        .find(|m| m.symbol == "NBA_26OCT26_GSWDEN_ML.GSW")
        .unwrap();
    assert_eq!(gsw.resolution.as_ref().unwrap().price, 1_000_000);
    let raw_trades: Value =
        serde_json::from_str(&std::fs::read_to_string(fx.join("rest_trades_page1.json")).unwrap())
            .unwrap();
    let mut syms: Vec<String> = raw_trades["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["symbol"].as_str().unwrap().to_owned())
        .collect();
    syms.sort_unstable();
    syms.dedup();
    let trades = client.trades(&syms, &PageParams::limit(500)).await.unwrap();
    // The page served verbatim, newest first.
    assert_eq!(trades.raw["data"]["items"], raw_trades["data"]["items"]);
    std::fs::remove_dir_all(&dir).ok();
}
