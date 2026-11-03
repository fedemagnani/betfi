//! The write API and the `PascalApi` trait against local servers only: no request leaves the
//! machine. The HTTP server verifies every signed request the way `pascal-mock` will.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use pascal_rs::ratelimit::{Backoff, RetryPolicy};
use pascal_rs::signing::{self, verify_cancel, verify_mass_cancel, verify_place_order_body};
use pascal_rs::{
    ApiErrorCode, AuthParams, BuilderInfo, BuilderPolicy, CancelOrder, Channel, Client, Error,
    LiveClient, MassCancel, OrderUpdateReason, PascalApi, PlaceOrder, PlaceOrderRequest, Side,
    TradeClient, TradingSigner, WsConfig, WsEvent, WsMessage,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// (method, target, body) of every request received.
type Log = Arc<Mutex<Vec<(String, String, String)>>>;

/// One request per connection; `handler(method, target, body)` returns (status, body).
async fn serve(
    handler: impl Fn(&str, &str, &str) -> (u16, String) + Send + Sync + 'static,
) -> (String, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log: Log = Arc::default();
    let log2 = log.clone();
    let handler = Arc::new(handler);
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let (log, handler) = (log2.clone(), handler.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                let head_end = loop {
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                    let n = sock.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
                let len: usize = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse().unwrap())
                    })
                    .unwrap_or(0);
                while buf.len() < head_end + len {
                    let n = sock.read(&mut chunk).await.unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                }
                let body = String::from_utf8(buf[head_end..head_end + len].to_vec()).unwrap();
                let mut parts = head.split(' ');
                let method = parts.next().unwrap_or("").to_owned();
                let target = parts.next().unwrap_or("").to_owned();
                if method == "POST" {
                    assert!(
                        head.to_ascii_lowercase()
                            .contains("content-type: application/json"),
                        "{head}"
                    );
                }
                let (status, out) = handler(&method, &target, &body);
                log.lock().unwrap().push((method, target, body));
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
                    out.len()
                );
                sock.write_all(resp.as_bytes()).await.unwrap();
                sock.shutdown().await.ok();
            });
        }
    });
    (format!("http://{addr}"), log)
}

fn fast_retry(max_attempts: u32) -> RetryPolicy {
    RetryPolicy {
        max_attempts,
        backoff: Backoff {
            base: Duration::from_millis(1),
            max: Duration::from_millis(5),
        },
    }
}

fn trading() -> TradingSigner {
    TradingSigner::from_seed(&[8; 32])
}

fn owner() -> [u8; 32] {
    TradingSigner::from_seed(&[7; 32]).pubkey()
}

fn builder() -> BuilderInfo {
    BuilderInfo {
        pubkey: TradingSigner::from_seed(&[9; 32]).pubkey(),
        fee_rate_u: 1_000,
    }
}

fn signed_order(coid: u64, side: Side, price: i64) -> PlaceOrderRequest {
    let order =
        PlaceOrder::ioc("NBA_26OCT28_PHIIND_ML.PHI", side, price, 5, coid).with_builder(builder());
    trading()
        .sign_place_order(&AuthParams::now(owner()), &order)
        .unwrap()
        .request
}

fn order_json(coid: u64, filled: u64) -> Value {
    json!({
        "id": (1000 + coid).to_string(), "client_order_id": coid.to_string(),
        "symbol": "NBA_26OCT28_PHIIND_ML.PHI", "side": "BID", "price": "0.715000",
        "size_original": "5", "size_remaining": "0", "size_filled": filled.to_string(),
        "notional_filled": format!("{}.{:06}", filled * 715_000 / 1_000_000, filled * 715_000 % 1_000_000),
        "tif": "IOC", "type": "LIMIT", "post_only": false, "reduce_only": false,
        "builder_pubkey": signing::base58(&builder().pubkey), "builder_fee_rate": "0.001000",
        "seq": "10", "update_seq": "11", "place_ts_ms": "1", "update_ts_ms": "1"
    })
}

fn fill_json(coid: u64) -> Value {
    json!({
        "trade_id": "555001", "symbol": "NBA_26OCT28_PHIIND_ML.PHI", "side": "BID",
        "reduce_only": false, "fill_price": "0.710000", "fill_size": "3", "liquidity": "TAKER",
        "counterparty": "5Z6Ay5NEcbg3xhopc522sBCRXQujkTiuDRnHGfQdcnSf",
        "order_id": (1000 + coid).to_string(), "client_order_id": coid.to_string(),
        "fee_usd": "0.000742", "collateral_change_usd": "-2.130742", "position_size_prev": "0",
        "realized_pnl_usd": "0.000000", "builder_pubkey": signing::base58(&builder().pubkey),
        "builder_fee_paid_usd": "0.000617", "seq": "11", "trade_ts_ms": "1"
    })
}

fn err_item(code: &str) -> Value {
    json!({"status": "error", "data": {"code": code, "message": code.to_lowercase()}, "server_time_ms": "5"})
}

/// A mock-like order endpoint: verifies every permit, fills the first order, rejects the rest
/// with the codes in `codes`.
fn orders_handler(codes: Vec<&'static str>) -> impl Fn(&str, &str, &str) -> (u16, String) {
    move |method, target, body| {
        assert_eq!((method, target), ("POST", "/api/v1/orders"));
        let verified = verify_place_order_body(body.as_bytes(), 3).unwrap();
        let mut out = Vec::new();
        for (i, v) in verified.into_iter().enumerate() {
            let v = v.expect("every permit verifies");
            assert_eq!(v.header.signer, trading().pubkey());
            assert_eq!(v.body.builder, Some(builder()));
            let coid = v.body.client_order_id;
            if i == 0 {
                out.push(json!({
                    "status": "success",
                    "data": {"order": order_json(coid, 3), "fills": [fill_json(coid)],
                             "update_reason": "IOC_REMAINDER_CANCELED"},
                    "seq": "11", "state_time_ms": "1", "server_time_ms": "2"
                }));
            } else {
                out.push(err_item(codes[(i - 1) % codes.len()]));
            }
        }
        (200, Value::Array(out).to_string())
    }
}

#[tokio::test]
async fn place_orders_maps_results_and_error_codes() {
    let codes = vec![
        "INVALID_PRICE",
        "WOULD_CROSS",
        "INSUFFICIENT_COLLATERAL",
        "RATE_LIMITED",
        "UNAUTHORIZED",
    ];
    let (base, log) = serve(orders_handler(codes.clone())).await;
    let trade = TradeClient::builder(&base)
        .builder_policy(BuilderPolicy::Exactly(builder()))
        .build()
        .unwrap();
    let orders: Vec<_> = (1..=6)
        .map(|i| signed_order(i, Side::Bid, 715_000))
        .collect();
    let results = trade.place_orders(&orders).await.unwrap();
    assert_eq!(results.len(), 6);

    let ok = results[0].as_ref().unwrap();
    assert_eq!(ok.data.order.client_order_id, 1);
    assert_eq!(ok.data.order.size_filled, 3);
    assert_eq!(ok.data.fills.len(), 1);
    assert_eq!(ok.data.fills[0].fill_price, 710_000);
    assert_eq!(ok.data.fills[0].builder_fee_paid_usd, Some(617));
    assert_eq!(
        ok.data.update_reason,
        Some(OrderUpdateReason::IocRemainderCanceled)
    );
    assert_eq!(ok.seq, Some(11));

    let want = [
        ApiErrorCode::InvalidPrice,
        ApiErrorCode::WouldCross,
        ApiErrorCode::InsufficientCollateral,
        ApiErrorCode::RateLimited,
        ApiErrorCode::Unauthorized,
    ];
    for (r, code) in results[1..].iter().zip(want) {
        match r {
            Err(Error::Api(e)) => {
                assert_eq!(e.kind(), Some(code));
                assert_eq!(e.http_status, Some(200));
                assert_eq!(e.server_time_ms, Some(5));
            }
            other => panic!("expected {code:?}, got {other:?}"),
        }
    }
    // One request; a per-item RATE_LIMITED is not re-sent (the batch partly executed).
    assert_eq!(log.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn request_level_rate_limit_is_retried_but_unknown_outcomes_are_not() {
    let calls = Arc::new(Mutex::new(0));
    let c = calls.clone();
    let (base, log) = serve(move |m, t, b| {
        let mut n = c.lock().unwrap();
        *n += 1;
        match *n {
            1 => (
                429,
                r#"{"status":"error","data":{"code":"RATE_LIMITED","message":"slow down"}}"#.into(),
            ),
            2 => orders_handler(vec!["WOULD_CROSS"])(m, t, b),
            3 => (
                503,
                r#"{"status":"error","data":{"code":"WRITE_RESULT_UNAVAILABLE","message":"?"}}"#
                    .into(),
            ),
            4 => (502, "<html>bad gateway</html>".into()),
            _ => (
                401,
                r#"{"status":"error","data":{"code":"UNAUTHORIZED","message":"bad key"}}"#.into(),
            ),
        }
    })
    .await;
    let trade = TradeClient::builder(&base)
        .retry(fast_retry(3))
        .build()
        .unwrap();
    let orders = [signed_order(1, Side::Bid, 715_000)];
    let r = trade.place_orders(&orders).await.unwrap();
    assert!(r[0].is_ok());
    assert_eq!(log.lock().unwrap().len(), 2, "429 re-sent once");
    let first = log.lock().unwrap()[0].2.clone();
    assert_eq!(first, log.lock().unwrap()[1].2, "the same signed bytes");

    let e = trade.place_orders(&orders).await.unwrap_err();
    assert_eq!(e.api_code(), Some("WRITE_RESULT_UNAVAILABLE"));
    let e = trade.place_orders(&orders).await.unwrap_err();
    assert!(matches!(e, Error::HttpStatus { status: 502, .. }));
    let e = trade.place_orders(&orders).await.unwrap_err();
    assert_eq!(e.api_code(), Some("UNAUTHORIZED"));
    assert_eq!(e.http_status(), Some(401));
    assert_eq!(log.lock().unwrap().len(), 5, "none of those were re-sent");
}

#[tokio::test]
async fn batch_limits_and_builder_policy_are_checked_before_sending() {
    let (base, log) = serve(|_, _, _| (500, String::new())).await;
    let trade = TradeClient::new(&base).unwrap();
    let many: Vec<_> = (1..=51)
        .map(|i| signed_order(i, Side::Bid, 715_000))
        .collect();
    assert!(matches!(
        trade.place_orders(&many).await,
        Err(Error::InvalidArgument(_))
    ));
    assert!(matches!(
        trade.place_orders(&[]).await,
        Err(Error::InvalidArgument(_))
    ));
    let no_builder = trading()
        .sign_place_order(
            &AuthParams::now(owner()),
            &PlaceOrder::ioc("A.B", Side::Bid, 500_000, 1, 1),
        )
        .unwrap()
        .request;
    let e = trade
        .place_orders(std::slice::from_ref(&no_builder))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("missing builder_info"), "{e}");
    let pinned = TradeClient::builder(&base)
        .builder_policy(BuilderPolicy::Exactly(BuilderInfo {
            fee_rate_u: 2_000,
            ..builder()
        }))
        .build()
        .unwrap();
    let e = pinned
        .place_orders(&[signed_order(1, Side::Bid, 715_000)])
        .await
        .unwrap_err();
    assert!(e.to_string().contains("differs"), "{e}");
    let any = TradeClient::builder(&base)
        .builder_policy(BuilderPolicy::Any)
        .retry(fast_retry(1))
        .build()
        .unwrap();
    assert!(matches!(
        any.place_orders(&[no_builder]).await,
        Err(Error::HttpStatus { status: 500, .. })
    ));
    let mcs: Vec<_> = (0..11)
        .map(|i| {
            trading()
                .sign_mass_cancel(
                    &AuthParams::now(owner()),
                    &MassCancel::new(format!("M{i}.X")),
                )
                .unwrap()
                .request
        })
        .collect();
    assert!(matches!(
        trade.mass_cancel(&mcs).await,
        Err(Error::InvalidArgument(_))
    ));
    assert_eq!(
        log.lock().unwrap().len(),
        1,
        "only the BuilderPolicy::Any call was sent"
    );
}

#[tokio::test]
async fn cancel_and_mass_cancel_round_trip() {
    let (base, log) = serve(|method, target, body| {
        assert_eq!(method, "POST");
        let items: Vec<Value> = serde_json::from_str(body).unwrap();
        let out: Vec<Value> = match target {
            "/api/v1/cancels" => items
                .into_iter()
                .map(|v| {
                    let req = serde_json::from_value(v).unwrap();
                    let ok = verify_cancel(&req, 3).unwrap();
                    match ok.body.target {
                        signing::CancelTarget::ClientOrderId(id) => json!({
                            "status": "success", "data": order_json(id, 0), "seq": "12"
                        }),
                        signing::CancelTarget::OrderId(_) => err_item("ORDER_NOT_FOUND"),
                    }
                })
                .collect(),
            "/api/v1/mass-cancels" => items
                .into_iter()
                .map(|v| {
                    let req = serde_json::from_value(v).unwrap();
                    let ok = verify_mass_cancel(&req, 3).unwrap();
                    assert_eq!(ok.body.symbol, "NBA_26OCT28_PHIIND_ML.PHI");
                    json!({"status": "success", "data": {"orders": [order_json(4, 0), order_json(5, 1)]}})
                })
                .collect(),
            t => panic!("unexpected {t}"),
        };
        (200, Value::Array(out).to_string())
    })
    .await;
    let trade = TradeClient::new(&base).unwrap();
    let auth = AuthParams::now(owner());
    let cancels = [
        trading()
            .sign_cancel(&auth, &CancelOrder::by_client_order_id(42))
            .unwrap()
            .request,
        trading()
            .sign_cancel(&auth, &CancelOrder::by_order_id(7))
            .unwrap()
            .request,
    ];
    let r = trade.cancel(&cancels).await.unwrap();
    assert_eq!(r[0].as_ref().unwrap().data.client_order_id, 42);
    assert_eq!(
        r[1].as_ref().unwrap_err().api_code(),
        Some(ApiErrorCode::OrderNotFound.as_str())
    );
    let mc = trading()
        .sign_mass_cancel(&auth, &MassCancel::new("NBA_26OCT28_PHIIND_ML.PHI"))
        .unwrap()
        .request;
    let r = trade.mass_cancel(&[mc]).await.unwrap();
    let orders = &r[0].as_ref().unwrap().data.orders;
    assert_eq!(
        orders.iter().map(|o| o.client_order_id).collect::<Vec<_>>(),
        [4, 5]
    );
    let log = log.lock().unwrap();
    assert_eq!(
        log.iter().map(|(_, t, _)| t.as_str()).collect::<Vec<_>>(),
        ["/api/v1/cancels", "/api/v1/mass-cancels"]
    );
}

/// `Arc<dyn PascalApi>` compiles and routes reads, writes and the socket.
#[tokio::test]
async fn pascal_api_as_a_trait_object() {
    let books = r#"{"status":"success","data":{"books":{"A.B":{"asks":[["0.550000","10"]],"bids":[["0.540000","4"]]}}},"seq":"9"}"#;
    // The recorded search response serves as a market list, one market given an unknown sport.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/pascal/rest_markets_search.json");
    let mut markets: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    markets["data"][1]["display_attributes"]["market_type"] =
        json!({"kind": "sports", "sport": "curling"});
    let markets = markets.to_string();
    let (base, log) = serve(move |method, target, body| {
        if method == "GET" && target == "/api/v1/markets" {
            (200, markets.clone())
        } else if method == "GET" {
            assert_eq!(target, "/api/v1/books?symbols=A.B");
            (200, books.into())
        } else {
            orders_handler(vec!["WOULD_CROSS"])(method, target, body)
        }
    })
    .await;

    // A socket that answers the subscribe frame with one account update.
    let ws_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_addr = ws_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (tcp, _) = ws_listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::Message;
        while let Some(Ok(msg)) = ws.next().await {
            if let Message::Text(t) = msg {
                let v: Value = serde_json::from_str(t.as_str()).unwrap();
                if v["type"] == "subscribe" {
                    assert_eq!(v["channels"][0]["channel"], "account");
                    assert_eq!(v["channels"][0]["exclude_orders"], true);
                    let frame = json!({
                        "channel": "account", "type": "update", "owner": signing::base58(&owner()),
                        "exclude_orders": true,
                        "data": {"orders": [], "positions": [], "pending_deposits": [],
                                 "pending_withdrawals": [], "fills": [fill_json(1)]},
                        "seq": "11"
                    });
                    ws.send(Message::text(frame.to_string())).await.unwrap();
                }
            }
        }
    });

    let api: Arc<dyn PascalApi> = LiveClient::new(
        Client::builder().base_url(&base).build().unwrap(),
        TradeClient::new(&base).unwrap(),
        WsConfig {
            url: format!("ws://{ws_addr}"),
            ..WsConfig::default()
        },
    )
    .into_dyn();

    let b = api.books(&["A.B".to_owned()]).await.unwrap();
    assert_eq!(b.data.books["A.B"].asks[0].price, 550_000);
    // D-004: the lenient list keeps the good markets and reports the bad one.
    let m = api.markets_lenient().await.unwrap();
    assert_eq!((m.data.items.len(), m.data.failures.len()), (2, 1));
    assert_eq!(m.data.failures[0].index, 1);
    let r = api
        .place_orders(&[
            signed_order(1, Side::Bid, 715_000),
            signed_order(2, Side::Ask, 300_000),
        ])
        .await
        .unwrap();
    assert!(r[0].is_ok());
    assert_eq!(r[1].as_ref().unwrap_err().api_code(), Some("WOULD_CROSS"));
    assert_eq!(log.lock().unwrap().len(), 3);

    let mut stream = api
        .subscribe(vec![Channel::account(signing::base58(&owner()))])
        .await
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match stream.next().await.expect("stream open") {
                WsEvent::Frame(f) => break f,
                WsEvent::Connected { .. } => continue,
                other => panic!("{other:?}"),
            }
        }
    })
    .await
    .expect("frame within 5 s");
    match frame.message {
        WsMessage::Account { state, .. } => {
            assert_eq!(state.fills.as_ref().unwrap()[0].client_order_id, 1);
        }
        other => panic!("expected an account frame, got {other:?}"),
    }
    drop(stream);
}
