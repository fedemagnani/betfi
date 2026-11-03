//! Step 2.D review gate: permits verified with `pascal-rs`, the `/ws` channels, `/fills` and
//! `/builder-stats`, `--account-snapshot`, wall-clock script times (D-090), the demo script.
//! Everything runs on 127.0.0.1; no network.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use pascal_mock::{AccountSnapshot, Mock, MockConfig, RunningMock, ScriptSource, TradingKeys};
use pascal_rs::ratelimit::{Backoff, BucketConfig, RateLimits, RetryPolicy};
use pascal_rs::signing::{
    self, AuthParams, BuilderInfo, CancelOrder, MassCancel, PlaceOrder, PlaceOrderRequest,
    TradingSigner,
};
use pascal_rs::trade::BuilderPolicy;
use pascal_rs::{
    AccountQuery, AccountState, Channel, Client, FrameKind, MarketUpdateType, PageParams, Side,
    TradeClient, WsConfig, WsEvent, WsFrame, WsMessage, decode_response,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

const PHI: &str = "NBA_26OCT28_PHIIND_ML.PHI";
const HOU: &str = "NBA_26OCT28_HOUMIL_ML.HOU";
/// The sample tape's first books snapshot (PHI best ask 0.699 × 35,200).
const SNAP0: u64 = 1_793_212_987_458;
const USD: i64 = 1_000_000;
const OWNER: [u8; 32] = [21; 32];
const BUILDER: [u8; 32] = [99; 32];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn b58(k: &[u8; 32]) -> String {
    signing::base58(k)
}

/// The sample tape, frozen, no script, permits verified.
fn cfg() -> MockConfig {
    let mut cfg = MockConfig::new(root().join("tapes/nba-2026-10-28/sample"));
    cfg.speed = 0.0;
    cfg.script = ScriptSource::None;
    cfg.verify_permits = true;
    cfg
}

fn limits() -> RateLimits {
    RateLimits {
        per_host: BucketConfig {
            per_second: 1_000.0,
            burst: 1_000.0,
        },
        per_route: HashMap::new(),
    }
}

fn no_retry() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 1,
        backoff: Backoff {
            base: Duration::from_millis(1),
            max: Duration::from_millis(1),
        },
    }
}

async fn start(cfg: MockConfig) -> (RunningMock, Client, TradeClient) {
    let running = Mock::new(cfg).unwrap().spawn("127.0.0.1:0").await.unwrap();
    let client = Client::builder()
        .base_url(running.base_url())
        .rate_limits(limits())
        .retry(no_retry())
        .build()
        .unwrap();
    let trade = TradeClient::builder(running.base_url())
        .rate_limits(limits())
        .retry(no_retry())
        .builder_policy(BuilderPolicy::Any)
        .build()
        .unwrap();
    (running, client, trade)
}

fn signer() -> TradingSigner {
    TradingSigner::from_seed(&[8; 32])
}

/// A signed IOC with the builder code, as the desk sends its inception hedge.
fn signed_ioc(
    key: &TradingSigner,
    cid: u64,
    symbol: &str,
    side: Side,
    price_u: i64,
    size: u64,
) -> PlaceOrderRequest {
    let order = PlaceOrder::ioc(symbol, side, price_u, size, cid).with_builder(BuilderInfo {
        pubkey: BUILDER,
        fee_rate_u: 1_000,
    });
    key.sign_place_order(&AuthParams::now(OWNER), &order)
        .unwrap()
        .request
}

fn flip_signature_byte(req: &mut PlaceOrderRequest) {
    let mut sig = bs58::decode(&req.auth.signature).into_vec().unwrap();
    sig[17] ^= 0x01;
    req.auth.signature = bs58::encode(sig).into_string();
}

/// `size × p × (1 − p) × 0.001`, rounded toward zero (the mock's builder fee).
fn builder_fee(size: i64, p: i64) -> i64 {
    let n = i128::from(size) * i128::from(p) * i128::from(USD - p) * 1_000;
    (n / 1_000_000_000_000) as i64
}

// ---------- permits ----------

#[tokio::test]
async fn signed_orders_fill_and_bad_or_unregistered_signatures_are_unauthorized() {
    let (m, client, trade) = start(cfg()).await;
    let k = signer();

    // A correctly signed IOC at the best ask fills.
    let r = trade
        .place_orders(&[signed_ioc(&k, 1, PHI, Side::Bid, 699_000, 10)])
        .await
        .unwrap();
    let placed = r[0].as_ref().unwrap();
    assert_eq!(placed.data.order.size_filled, 10);
    assert_eq!(placed.data.fills[0].fill_price, 699_000);
    assert_eq!(
        placed.data.fills[0].builder_fee_paid_usd,
        Some(builder_fee(10, 699_000))
    );

    // The same order with one flipped signature byte: UNAUTHORIZED, HTTP 401, nothing booked.
    let mut bad = signed_ioc(&k, 2, PHI, Side::Bid, 699_000, 10);
    flip_signature_byte(&mut bad);
    let r = trade.place_orders(&[bad]).await.unwrap();
    let err = r[0].as_ref().unwrap_err();
    assert_eq!(err.api_code(), Some("UNAUTHORIZED"));
    assert_eq!(err.http_status(), Some(401));
    let acct = client
        .account(&b58(&OWNER), &AccountQuery::default())
        .await
        .unwrap();
    assert_eq!(acct.data.positions[0].size, 10);

    // A field changed after signing is caught the same way.
    let mut moved = signed_ioc(&k, 3, PHI, Side::Bid, 699_000, 10);
    moved.size = 11;
    let r = trade.place_orders(&[moved]).await.unwrap();
    assert_eq!(r[0].as_ref().unwrap_err().api_code(), Some("UNAUTHORIZED"));

    // Cancels and mass cancels are verified too: a resting bid, then a forged and a real cancel.
    let rest = PlaceOrder::limit(PHI, Side::Bid, 600_000, 1, 50);
    let rest = k
        .sign_place_order(&AuthParams::now(OWNER), &rest)
        .unwrap()
        .request;
    assert!(trade.place_orders(&[rest]).await.unwrap()[0].is_ok());
    let mut cancel = k
        .sign_cancel(
            &AuthParams::now(OWNER),
            &CancelOrder::by_client_order_id(50),
        )
        .unwrap()
        .request;
    let good_sig = cancel.auth.signature.clone();
    cancel.auth.signature = TradingSigner::from_seed(&[1; 32])
        .sign_cancel(
            &AuthParams::now(OWNER),
            &CancelOrder::by_client_order_id(50),
        )
        .unwrap()
        .request
        .auth
        .signature;
    let r = trade.cancel(&[cancel.clone()]).await.unwrap();
    assert_eq!(r[0].as_ref().unwrap_err().api_code(), Some("UNAUTHORIZED"));
    cancel.auth.signature = good_sig;
    assert!(trade.cancel(&[cancel]).await.unwrap()[0].is_ok());
    let mut mc = k
        .sign_mass_cancel(&AuthParams::now(OWNER), &MassCancel::new(PHI))
        .unwrap()
        .request;
    let ok = trade.mass_cancel(&[mc.clone()]).await.unwrap();
    assert!(ok[0].is_ok());
    mc.symbol = HOU.into();
    let r = trade.mass_cancel(&[mc]).await.unwrap();
    assert_eq!(r[0].as_ref().unwrap_err().api_code(), Some("UNAUTHORIZED"));
    drop(m);

    // With a registry, an unregistered (but validly signed) key is refused.
    let mut keys = TradingKeys::new();
    keys.add(
        &b58(&OWNER),
        &TradingSigner::from_seed(&[2; 32]).pubkey_base58(),
    )
    .unwrap();
    let mut c = cfg();
    c.verify_permits = false; // implied by the registry
    c.trading_keys = keys.clone();
    let (_m, _client, trade) = start(c).await;
    let r = trade
        .place_orders(&[signed_ioc(&k, 1, PHI, Side::Bid, 699_000, 1)])
        .await
        .unwrap();
    let err = r[0].as_ref().unwrap_err();
    assert_eq!(err.api_code(), Some("UNAUTHORIZED"));
    assert!(err.to_string().contains("not registered"), "{err}");
    // The registered key passes.
    let k2 = TradingSigner::from_seed(&[2; 32]);
    let r = trade
        .place_orders(&[signed_ioc(&k2, 1, PHI, Side::Bid, 699_000, 1)])
        .await
        .unwrap();
    assert!(r[0].is_ok());
}

#[tokio::test]
async fn malformed_auth_never_panics_and_is_refused() {
    let (m, _client, _trade) = start(cfg()).await;
    let http = reqwest::Client::new();
    let mut req =
        serde_json::to_value(signed_ioc(&signer(), 1, PHI, Side::Bid, 699_000, 1)).unwrap();
    for (field, value) in [
        ("signature", json!("not-base58-0OIl")),
        ("signature", json!(bs58::encode([0u8; 64]).into_string())),
        ("signer", json!(bs58::encode([0u8; 32]).into_string())),
        ("signer", json!(b58(&[7; 32]))),
    ] {
        let mut v = req.clone();
        v["auth"][field] = value;
        let resp = http
            .post(format!("{}/api/v1/orders", m.base_url()))
            .json(&json!([v]))
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.unwrap();
        assert!(status == 401 || status == 422, "{field}: {status} {body}");
        assert_eq!(body[0]["status"], "error", "{field}");
    }
    // A JSON number for metadata (D-037) still verifies.
    let mut order = PlaceOrder::ioc(PHI, Side::Bid, 699_000, 1, 9);
    order.metadata = 2;
    let s = signer()
        .sign_place_order(&AuthParams::now(OWNER), &order)
        .unwrap();
    req = serde_json::to_value(&s.request).unwrap();
    req["metadata"] = json!(2);
    let resp = http
        .post(format!("{}/api/v1/orders", m.base_url()))
        .json(&json!([req]))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
}

// ---------- WebSocket ----------

async fn next_frame(
    rx: &mut mpsc::Receiver<WsEvent>,
    what: &str,
    pred: impl Fn(&WsFrame) -> bool,
) -> WsFrame {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let ev = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
        match ev {
            Some(WsEvent::Frame(f)) if pred(&f) => return *f,
            Some(WsEvent::DecodeError { error, text }) => panic!("{error}: {text}"),
            Some(WsEvent::Frame(f)) => {
                if let WsMessage::Error { error, .. } = &f.message {
                    panic!("error frame while waiting for {what}: {error:?}");
                }
            }
            Some(_) => {}
            None => panic!("ws closed while waiting for {what}"),
        }
    }
}

fn account_frame(owner: &str, kind: FrameKind) -> impl Fn(&WsFrame) -> bool {
    let owner = owner.to_owned();
    move |f| matches!(&f.message, WsMessage::Account { owner: o, kind: k, .. } if *o == owner && *k == kind)
}

fn account_state(f: &WsFrame) -> &AccountState {
    match &f.message {
        WsMessage::Account { state, .. } => state,
        other => panic!("not an account frame: {other:?}"),
    }
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().unwrap().keys().cloned().collect()
}

fn ws_config(m: &RunningMock) -> WsConfig {
    WsConfig {
        url: m.ws_url(),
        ..WsConfig::default()
    }
}

#[tokio::test]
async fn account_channel_first_update_carries_the_fill_then_resolution_frames() {
    let (m, _client, trade) = start(cfg()).await;
    let owner = b58(&OWNER);
    let builder = b58(&BUILDER);
    let (_ws, mut rx) = pascal_rs::WsClient::spawn(
        ws_config(&m),
        [
            Channel::account(owner.clone()),
            Channel::Account {
                owner: builder.clone(),
                exclude_orders: false,
            },
            Channel::MarketUpdate,
        ],
    );

    // Three snapshots, in the order the client subscribed them.
    let (mut snap, mut market_snap, mut builder_snap) = (None, false, false);
    while snap.is_none() || !market_snap || !builder_snap {
        let f = next_frame(&mut rx, "snapshots", |f| f.raw["type"] == "snapshot").await;
        match &f.message {
            WsMessage::Account { owner: o, .. } if *o == owner => snap = Some(f),
            WsMessage::Account { owner: o, .. } if *o == builder => builder_snap = true,
            WsMessage::MarketUpdate { updates, .. } => {
                assert!(updates.items.is_empty());
                assert_eq!(f.raw["data"], json!([]));
                market_snap = true;
            }
            other => panic!("{other:?}"),
        }
    }
    let snap = snap.unwrap();
    assert_eq!(account_state(&snap).collateral_usd, Some(10_000 * USD));
    assert_eq!(snap.raw["exclude_orders"], true);
    assert_eq!(snap.state_time_ms, Some(SNAP0));

    // A resting order changes nothing an `exclude_orders` subscriber sees: no frame for it.
    let rest = PlaceOrder::limit(PHI, Side::Bid, 600_000, 1, 77);
    let rest = signer()
        .sign_place_order(&AuthParams::now(OWNER), &rest)
        .unwrap()
        .request;
    assert!(trade.place_orders(&[rest]).await.unwrap()[0].is_ok());

    let placed = trade
        .place_orders(&[signed_ioc(&signer(), 1, PHI, Side::Bid, 699_000, 10)])
        .await
        .unwrap();
    let rest_fill = placed[0].as_ref().unwrap().data.fills[0].clone();

    // The first account update after the order carries the fill with builder_fee_paid_usd.
    let up = next_frame(
        &mut rx,
        "account update",
        account_frame(&owner, FrameKind::Update),
    )
    .await;
    let st = account_state(&up);
    let fills = st.fills.as_ref().expect("fills in the first update");
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0], rest_fill);
    assert_eq!(
        fills[0].builder_fee_paid_usd,
        Some(builder_fee(10, 699_000))
    );
    assert_eq!(fills[0].builder_pubkey.as_deref(), Some(builder.as_str()));
    assert_eq!(st.positions.len(), 1);
    assert_eq!(
        (st.positions[0].symbol.as_str(), st.positions[0].size),
        (PHI, 10)
    );
    assert!(st.collateral_usd.unwrap() < 10_000 * USD);
    assert!(st.lifetime_stats.is_some());
    assert!(st.orders.is_empty(), "exclude_orders");
    assert!(st.order_updates.is_none(), "exclude_orders");
    assert!(up.seq.unwrap() > snap.seq.unwrap());
    // Same envelope and field names as the recorded Pascal frame.
    let fixture: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("fixtures/pascal/ws_account_update.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(keys(&up.raw), keys(&fixture));
    let extra: BTreeSet<String> = ["order_updates", "resolutions", "rewards"]
        .into_iter()
        .map(String::from)
        .collect();
    assert!(
        keys(&up.raw["data"]).is_subset(&keys(&fixture["data"]).union(&extra).cloned().collect()),
        "{:?}",
        keys(&up.raw["data"])
    );
    assert!(keys(&fixture["data"]["fills"][0]).is_subset(&keys(&up.raw["data"]["fills"][0])));

    // The builder's account sees the fee.
    let bup = next_frame(
        &mut rx,
        "builder update",
        account_frame(&builder, FrameKind::Update),
    )
    .await;
    assert_eq!(
        account_state(&bup)
            .rewards
            .unwrap()
            .builder_fees_received_usd,
        builder_fee(10, 699_000)
    );

    // A resolution: market_update frame, then the position settles on the account channel.
    let http = reqwest::Client::new();
    http.post(format!("{}/mock/resolve", m.base_url()))
        .json(&json!({"symbol": PHI, "price": "1"}))
        .send()
        .await
        .unwrap();
    let mu = next_frame(&mut rx, "market_update resolved", |f| {
        matches!(
            &f.message,
            WsMessage::MarketUpdate {
                kind: FrameKind::Update,
                ..
            }
        )
    })
    .await;
    let WsMessage::MarketUpdate { updates, .. } = &mu.message else {
        unreachable!()
    };
    assert!(updates.failures.is_empty(), "{:?}", updates.failures);
    let (_, u) = &updates.items[0];
    assert_eq!(u.update_type, MarketUpdateType::Resolved);
    assert_eq!(u.market.symbol, PHI);
    assert_eq!(u.market.resolution.as_ref().unwrap().price, USD);
    assert!(mu.raw["data"][0]["market"].get("best_bid").is_none());
    let res = next_frame(
        &mut rx,
        "account resolution",
        account_frame(&owner, FrameKind::Update),
    )
    .await;
    let st = account_state(&res);
    let r = &st.resolutions.as_ref().unwrap()[0];
    assert_eq!((r.symbol.as_str(), r.size, r.resolution), (PHI, 10, USD));
    assert_eq!(st.positions[0].size, 0);
}

#[tokio::test]
async fn book_channel_snapshot_and_updates() {
    let (m, _client, trade) = start(cfg()).await;
    let (_ws, mut rx) =
        pascal_rs::WsClient::spawn(ws_config(&m), [Channel::Book { symbol: PHI.into() }]);
    let snap = next_frame(&mut rx, "book snapshot", |f| {
        matches!(
            &f.message,
            WsMessage::Book {
                kind: FrameKind::Snapshot,
                ..
            }
        )
    })
    .await;
    let WsMessage::Book { book, .. } = &snap.message else {
        unreachable!()
    };
    assert_eq!((book.asks[0].price, book.asks[0].size), (699_000, 35_200));
    // Take 200 of the best ask: the next update shows the level's new size.
    trade
        .place_orders(&[signed_ioc(&signer(), 1, PHI, Side::Bid, 699_000, 200)])
        .await
        .unwrap();
    let up = next_frame(&mut rx, "book update", |f| {
        matches!(
            &f.message,
            WsMessage::Book {
                kind: FrameKind::Update,
                ..
            }
        )
    })
    .await;
    let WsMessage::Book { book, symbol, .. } = &up.message else {
        unreachable!()
    };
    assert_eq!(symbol, PHI);
    assert_eq!(book.asks.len(), 1);
    assert_eq!((book.asks[0].price, book.asks[0].size), (699_000, 35_000));
    assert!(book.bids.is_empty());
}

type Raw =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn raw_next(ws: &mut Raw) -> Option<Value> {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
            Err(_) => panic!("no frame within 5 s"),
            Ok(None) | Ok(Some(Err(_))) | Ok(Some(Ok(Message::Close(_)))) => return None,
            Ok(Some(Ok(Message::Text(t)))) => return Some(serde_json::from_str(&t).unwrap()),
            Ok(Some(Ok(_))) => {}
        }
    }
}

#[tokio::test]
async fn raw_socket_ping_errors_and_idle_close() {
    let mut c = cfg();
    c.ws_idle_timeout = Duration::from_millis(800);
    let (m, _client, _trade) = start(c).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(m.ws_url()).await.unwrap();

    ws.send(Message::text(r#"{"type":"ping"}"#)).await.unwrap();
    assert_eq!(raw_next(&mut ws).await.unwrap(), json!({"type": "pong"}));
    ws.send(Message::text("not json")).await.unwrap();
    assert_eq!(
        raw_next(&mut ws).await.unwrap()["data"]["code"],
        "INVALID_JSON"
    );
    ws.send(Message::text(
        r#"{"type":"subscribe","channels":[{"channel":"book","symbol":"NOPE.X"},{"channel":"account","owner":"abc"},{"channel":"all_trades"},{"channel":"bogus"}]}"#,
    ))
    .await
    .unwrap();
    let mut codes = Vec::new();
    for _ in 0..4 {
        let f = raw_next(&mut ws).await.unwrap();
        assert_eq!(f["type"], "error");
        WsFrame::from_value(f.clone()).unwrap();
        codes.push(f["data"]["code"].as_str().unwrap().to_owned());
    }
    assert_eq!(
        codes,
        [
            "SYMBOL_NOT_FOUND",
            "INVALID_REQUEST",
            "INVALID_REQUEST",
            "INVALID_REQUEST"
        ]
    );
    // Pings keep it open past the idle timeout; silence closes it.
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_millis(400)).await;
        ws.send(Message::text(r#"{"type":"ping"}"#)).await.unwrap();
        assert_eq!(raw_next(&mut ws).await.unwrap()["type"], "pong");
    }
    let t = Instant::now();
    assert!(raw_next(&mut ws).await.is_none());
    assert!(
        t.elapsed() >= Duration::from_millis(700),
        "{:?}",
        t.elapsed()
    );
}

// ---------- /fills, /builder-stats ----------

#[tokio::test]
async fn fills_paginate_through_the_pager_and_builder_stats_decode() {
    let (m, client, trade) = start(cfg()).await;
    let owner = b58(&OWNER);
    let k = signer();
    // Seven fills over three markets.
    let mut sizes = 0;
    for (i, (sym, size)) in [
        (PHI, 1),
        (HOU, 2),
        (PHI, 3),
        (PHI, 1),
        (HOU, 1),
        (PHI, 2),
        (PHI, 4),
    ]
    .into_iter()
    .enumerate()
    {
        let book = client.books(&[sym]).await.unwrap();
        let ask = book.data.books[sym].asks[0].price;
        let r = trade
            .place_orders(&[signed_ioc(&k, i as u64, sym, Side::Bid, ask, size)])
            .await
            .unwrap();
        assert_eq!(r[0].as_ref().unwrap().data.fills.len(), 1);
        sizes += size;
    }

    let all = client
        .fills(&owner, &[] as &[&str], &PageParams::limit(500))
        .await
        .unwrap();
    assert_eq!(all.data.items.len(), 7);
    assert!(all.data.next_cursor.is_none());
    assert!(
        all.data.items.windows(2).all(|w| w[0].seq > w[1].seq),
        "newest first"
    );

    let mut pager = client.fills_pager(&owner, &[] as &[&str], Some(2)).unwrap();
    let mut paged = Vec::new();
    while let Some(page) = pager.next_page().await.unwrap() {
        assert!(page.data.items.len() <= 2);
        if let Some(c) = &page.data.next_cursor {
            assert_eq!(c.len(), 31, "{c}");
        }
        paged.extend(page.data.items);
    }
    assert_eq!(pager.pages_fetched(), 4);
    assert_eq!(paged, all.data.items);

    let phi = client
        .fills(&owner, &[PHI], &PageParams::limit(500))
        .await
        .unwrap();
    assert_eq!(phi.data.items.len(), 5);
    let seq = all.data.items[3].seq;
    let older = client
        .fills(
            &owner,
            &[] as &[&str],
            &PageParams {
                limit: Some(10),
                at_or_before_seq: Some(seq),
                ..PageParams::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(older.data.items, all.data.items[3..]);

    // Pascal's fixture decodes the same way: the mock shape matches.
    let http = reqwest::Client::new();
    for bad in [
        "limit=0",
        "limit=501",
        "before_cursor=x",
        "before_cursor=1:2&at_or_before_seq=3",
    ] {
        let r = http
            .get(format!(
                "{}/api/v1/accounts/{owner}/fills?{bad}",
                m.base_url()
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 422, "{bad}");
    }

    let stats = client.builder_stats(&b58(&BUILDER)).await.unwrap();
    let fees: i64 = all
        .data
        .items
        .iter()
        .map(|f| f.builder_fee_paid_usd.unwrap())
        .sum();
    assert_eq!(stats.data.builder_volume_contracts, sizes as u128);
    assert_eq!(stats.data.builder_fees_received_usd, fees);
    let notional: i64 = all
        .data
        .items
        .iter()
        .map(|f| f.fill_price * f.fill_size as i64)
        .sum();
    assert_eq!(stats.data.builder_volume_notional_usd, notional);
    assert!(stats.data.builder_fee_base_usd > 0);
    let none = client.builder_stats(&owner).await.unwrap();
    assert_eq!(none.data.builder_volume_contracts, 0);
    assert_eq!(none.data.builder_fees_received_usd, 0);
    let fixture =
        std::fs::read(root().join("fixtures/pascal/rest_accounts_owner_builder_stats.json"))
            .unwrap();
    decode_response::<pascal_rs::BuilderStats>(200, &fixture, "fixture").unwrap();
}

// ---------- --account-snapshot ----------

#[tokio::test]
async fn account_snapshot_boots_positions_and_collateral() {
    let hedger = b58(&[33; 32]);
    let saved = json!({
        "status": "success",
        "data": {
            "has_deposited": true,
            "collateral_usd": "250.000000",
            "rewards": {"referral_rewards_received_usd": "0.000000", "builder_fees_received_usd": "0.000000", "user_rewards_received_usd": "0.000000"},
            "lifetime_stats": {
                "maker": {"volume_contracts": "0", "weighted_volume": "0.000000", "volume_notional_usd": "0.000000", "fees_paid_usd": "0.000000"},
                "taker": {"volume_contracts": "15", "weighted_volume": "3.100000", "volume_notional_usd": "10.050000", "fees_paid_usd": "0.060000"}
            },
            "orders": [],
            "positions": [
                {"symbol": PHI, "size": "10", "cumulative_increasing_contracts": "10",
                 "cumulative_increasing_notional_usd": "6.950000", "cumulative_reducing_contracts": "0",
                 "cumulative_reducing_notional_usd": "0.000000", "cumulative_fees_usd": "0.042000",
                 "realized_pnl_usd": "0.000000", "unrealized_pnl_usd": "0.000000", "mark_price": "0.695000",
                 "average_entry_price": "0.695000", "remaining_entry_notional_usd": "6.950000",
                 "seq": "12216604100", "update_seq": "12216604100",
                 "open_ts_ms": "1793212990000", "update_ts_ms": "1793212990000"},
                {"symbol": HOU, "size": "-5", "cumulative_increasing_contracts": "5",
                 "cumulative_increasing_notional_usd": "3.100000", "cumulative_reducing_contracts": "0",
                 "cumulative_reducing_notional_usd": "0.000000", "cumulative_fees_usd": "0.018000",
                 "realized_pnl_usd": "0.000000", "unrealized_pnl_usd": "0.000000", "mark_price": "0.620000",
                 "average_entry_price": "0.620000",
                 "seq": "12216604101", "update_seq": "12216604101",
                 "open_ts_ms": "1793212990000", "update_ts_ms": "1793212990000"}
            ],
            "pending_deposits": [],
            "pending_withdrawals": []
        },
        "seq": "12216604102", "state_time_ms": "1793212990000", "server_time_ms": "1793212990001"
    });
    let dir = std::env::temp_dir().join(format!("pascal-mock-snap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("hedger.json");
    std::fs::write(&file, saved.to_string()).unwrap();

    let mut c = cfg();
    c.account_snapshots
        .push(AccountSnapshot::load_spec(&format!("{hedger}={}", file.display())).unwrap());
    let (m, client, _trade) = start(c).await;
    let a = client
        .account(&hedger, &AccountQuery::default())
        .await
        .unwrap()
        .data;
    assert_eq!(a.collateral_usd, Some(250 * USD));
    let sizes: Vec<(&str, i64)> = a
        .positions
        .iter()
        .map(|p| (p.symbol.as_str(), p.size))
        .collect();
    assert_eq!(sizes, vec![(HOU, -5), (PHI, 10)]);
    let phi = a.positions.iter().find(|p| p.symbol == PHI).unwrap();
    assert_eq!(phi.average_entry_price, Some(695_000));
    assert_eq!(phi.seq, 12_216_604_100);
    assert_eq!(a.lifetime_stats.unwrap().taker.volume_contracts, 15);

    // The booted positions settle like the mock's own: PHI wins (+10), HOU long side loses,
    // so the short pays 5 × (1 − 0) = 5.
    let http = reqwest::Client::new();
    for (sym, p) in [(PHI, "1"), (HOU, "0")] {
        http.post(format!("{}/mock/resolve", m.base_url()))
            .json(&json!({"symbol": sym, "price": p}))
            .send()
            .await
            .unwrap();
    }
    let a = client
        .account(&hedger, &AccountQuery::default())
        .await
        .unwrap()
        .data;
    assert!(a.positions.is_empty());
    assert_eq!(a.collateral_usd, Some(265 * USD));
    std::fs::remove_dir_all(&dir).ok();
}

// ---------- D-048 / D-090: wall-clock script times ----------

fn script_file(tag: &str, body: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("pascal-mock-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("script.toml");
    std::fs::write(&file, body).unwrap();
    (dir, file)
}

#[tokio::test]
async fn wall_times_do_not_fire_at_boot_on_a_frozen_clock() {
    let (dir, file) = script_file(
        "wall0",
        &format!("[[resolve]]\nsymbol = \"{PHI}\"\nprice = \"1\"\nat = \"wall+600ms\"\n"),
    );
    let mut c = cfg();
    c.script = ScriptSource::File(file);
    let mock = Mock::new(c).unwrap();
    assert!(
        mock.with_engine(|e| e.resolved_markets(u64::MAX, 50))
            .is_empty()
    );
    tokio::time::sleep(Duration::from_millis(700)).await;
    let res = mock.with_engine(|e| e.resolved_markets(u64::MAX, 50));
    assert_eq!(res.len(), 1);
    // Fired at the frozen tape time.
    assert_eq!(
        res[0]["resolution"]["market_resolution_ts_ms"],
        SNAP0.to_string()
    );
    std::fs::remove_dir_all(&dir).ok();
}

// ---------- the demo script ----------

/// Quote (books), hedge (signed IOC with builder code), fill on the `account` socket, then the
/// scripted resolution of leg 1 pushed on `market_update` with no REST traffic, at `--speed 60`.
async fn demo(cfg: MockConfig, wall_s: u64) -> Duration {
    let t0 = Instant::now();
    let (m, client, trade) = start(cfg).await;
    let owner = b58(&OWNER);
    let (_ws, mut rx) = pascal_rs::WsClient::spawn(
        ws_config(&m),
        [Channel::account(owner.clone()), Channel::MarketUpdate],
    );
    next_frame(
        &mut rx,
        "snapshot",
        account_frame(&owner, FrameKind::Snapshot),
    )
    .await;

    let books = client.books(&[PHI]).await.unwrap();
    let ask = books.data.books[PHI].asks[0].price;
    let r = trade
        .place_orders(&[signed_ioc(&signer(), 1, PHI, Side::Bid, ask, 5)])
        .await
        .unwrap();
    assert_eq!(r[0].as_ref().unwrap().data.order.size_filled, 5);
    let up = next_frame(&mut rx, "fill", account_frame(&owner, FrameKind::Update)).await;
    assert!(
        account_state(&up).fills.as_ref().unwrap()[0]
            .builder_fee_paid_usd
            .unwrap()
            > 0
    );

    let wait = tokio::time::timeout(Duration::from_secs(wall_s + 30), async {
        loop {
            match rx.recv().await {
                Some(WsEvent::Frame(f)) => {
                    if let WsMessage::MarketUpdate {
                        kind: FrameKind::Update,
                        updates,
                    } = &f.message
                    {
                        return updates.items[0].1.clone();
                    }
                }
                Some(_) => {}
                None => panic!("socket closed"),
            }
        }
    })
    .await
    .expect("leg 1 resolved");
    assert_eq!(wait.market.symbol, PHI);
    let r = wait.market.resolution.unwrap();
    assert_eq!(r.price, USD);
    // `wall+Ns` at speed 60 lands about N minutes into the tape.
    let tape_s = (r.market_resolution_ts_ms - SNAP0) / 1_000;
    assert!(
        tape_s >= wall_s * 60 && tape_s <= (wall_s + 10) * 60,
        "{tape_s} s of tape"
    );
    // The settled position shows on the socket too.
    let res = next_frame(
        &mut rx,
        "settlement",
        account_frame(&owner, FrameKind::Update),
    )
    .await;
    assert_eq!(account_state(&res).resolutions.as_ref().unwrap()[0].size, 5);
    t0.elapsed()
}

#[tokio::test]
async fn demo_script_quote_to_resolution_scaled() {
    let (dir, file) = script_file(
        "demo",
        &format!("[[resolve]]\nsymbol = \"{PHI}\"\nprice = \"1\"\nat = \"wall+2s\"\n"),
    );
    let mut c = cfg();
    c.speed = 60.0;
    c.script = ScriptSource::File(file);
    let took = demo(c, 2).await;
    assert!(
        took >= Duration::from_secs(2) && took < Duration::from_secs(10),
        "{took:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The real demo: `tapes/nba-2026-10-28/script.toml` (`wall+40s`) at `--speed 60`, under 90 s.
#[tokio::test]
#[ignore = "real time: about 40 s"]
async fn demo_script_real_time() {
    let mut c = MockConfig::new(root().join("tapes/nba-2026-10-28"));
    c.speed = 60.0;
    c.verify_permits = true;
    let took = demo(c, 40).await;
    assert!(
        took >= Duration::from_secs(40) && took < Duration::from_secs(90),
        "{took:?}"
    );
}

// ---------- error injection (spec section 9) ----------

#[tokio::test]
async fn spec_error_list_injects_on_orders() {
    let (m, _client, trade) = start(cfg()).await;
    let http = reqwest::Client::new();
    for (code, status) in [
        ("INVALID_PRICE", 422),
        ("WOULD_CROSS", 422),
        ("INSUFFICIENT_COLLATERAL", 422),
        ("RATE_LIMITED", 429),
    ] {
        let r = http
            .post(format!("{}/mock/inject", m.base_url()))
            .json(&json!({"code": code, "path": "/api/v1/orders"}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 200);
        let out = trade
            .place_orders(&[signed_ioc(&signer(), 1, PHI, Side::Bid, 699_000, 1)])
            .await;
        let err = match out {
            Ok(items) => items.into_iter().next().unwrap().unwrap_err(),
            Err(e) => e,
        };
        assert_eq!(err.api_code(), Some(code));
        assert_eq!(err.http_status(), Some(status));
    }
    let r = http
        .post(format!("{}/mock/inject", m.base_url()))
        .json(&json!({"code": "MADE_UP"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 422);
}
