//! WebSocket client behaviour against a local server: slow consumers, connect timeouts,
//! backoff growth, rejected channels and subscription identity.

use std::future::Future;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use pascal_rs::ratelimit::Backoff;
use pascal_rs::{Channel, WsClient, WsConfig, WsEvent, WsMessage};
use serde_json::Value;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

type Ws = WebSocketStream<TcpStream>;

/// Accepts WebSocket connections forever; `handler(index, socket)` runs per connection.
async fn serve<F, Fut>(handler: F) -> String
where
    F: Fn(usize, Ws) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut index = 0;
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            let Ok(ws) = tokio_tungstenite::accept_async(tcp).await else {
                continue;
            };
            tokio::spawn(handler(index, ws));
            index += 1;
        }
    });
    format!("ws://{addr}")
}

/// Forwards every text frame the client sends, tagged with the connection index.
async fn record(index: usize, ws: &mut Ws, tx: &mpsc::UnboundedSender<(usize, Value)>) {
    while let Some(Ok(msg)) = ws.next().await {
        if let Message::Text(t) = msg {
            let _ = tx.send((index, serde_json::from_str(t.as_str()).unwrap()));
        }
    }
}

fn config(url: String) -> WsConfig {
    WsConfig {
        url,
        ping_interval: Duration::from_millis(100),
        idle_timeout: Duration::from_secs(30),
        connect_timeout: Duration::from_secs(5),
        reconnect: Backoff {
            base: Duration::from_millis(20),
            max: Duration::from_secs(5),
        },
        healthy_after: Duration::from_secs(30),
        lag_timeout: Duration::from_secs(30),
        max_message_size: 1 << 20,
        event_buffer: 16,
    }
}

const TRADE: &str = r#"{"trade_id":"1","symbol":"A.B","taker_side":"BID","price":"0.500000","size":"1","maker":"M","taker":"T","seq":"1","trade_ts_ms":"1"}"#;

fn trades_update() -> String {
    format!(r#"{{"channel":"all_trades","type":"update","data":[{TRADE}],"seq":"1"}}"#)
}

async fn next_event(rx: &mut mpsc::Receiver<WsEvent>) -> WsEvent {
    timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("event within 5 s")
        .expect("task alive")
}

async fn next_frame(rx: &mut mpsc::UnboundedReceiver<(usize, Value)>) -> (usize, Value) {
    timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("client frame within 5 s")
        .unwrap()
}

/// Finding 2: a consumer that stops reading must not stop pings or shutdown.
#[tokio::test]
async fn slow_consumer_keeps_pinging_and_shuts_down() {
    let (tx, mut frames) = mpsc::unbounded_channel();
    let url = serve(move |i, mut ws| {
        let tx = tx.clone();
        async move {
            for _ in 0..50 {
                ws.send(Message::text(trades_update())).await.unwrap();
            }
            record(i, &mut ws, &tx).await;
        }
    })
    .await;
    let mut cfg = config(url);
    cfg.event_buffer = 1;
    let (ws, _events) = WsClient::spawn(cfg, [Channel::AllTrades]);
    // Nobody reads `_events`. Pings must keep flowing.
    let mut pings = 0;
    while pings < 3 {
        let (_, f) = next_frame(&mut frames).await;
        if f["type"] == "ping" {
            pings += 1;
        }
    }
    timeout(Duration::from_secs(2), ws.shutdown())
        .await
        .expect("shutdown is reachable while the event channel is full");
}

/// Finding 2: an event stuck longer than `lag_timeout` drops the connection, emits `Lagged`,
/// and the client reconnects (fresh snapshots).
#[tokio::test]
async fn lagging_consumer_is_resynced() {
    let url = serve(|_, mut ws| async move {
        for _ in 0..50 {
            if ws.send(Message::text(trades_update())).await.is_err() {
                return;
            }
        }
        while ws.next().await.is_some() {}
    })
    .await;
    let mut cfg = config(url);
    cfg.event_buffer = 1;
    cfg.lag_timeout = Duration::from_millis(300);
    let (ws, mut events) = WsClient::spawn(cfg, [Channel::AllTrades]);
    tokio::time::sleep(Duration::from_secs(1)).await;
    let mut lagged = None;
    loop {
        match next_event(&mut events).await {
            WsEvent::Lagged { waited } => lagged = Some(waited),
            WsEvent::Connected { .. } if lagged.is_some() => break,
            _ => {}
        }
    }
    assert!(lagged.unwrap() >= Duration::from_millis(300));
    ws.shutdown().await;
}

/// Finding 3: a server that accepts TCP but never upgrades hits the connect timeout, and
/// shutdown during a connect attempt returns promptly.
#[tokio::test]
async fn connect_timeout_and_shutdown_while_connecting() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let mut held = Vec::new();
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            held.push(tcp); // never answer the upgrade
        }
    });
    let mut cfg = config(url.clone());
    cfg.connect_timeout = Duration::from_millis(200);
    let (ws, mut events) = WsClient::spawn(cfg, [Channel::AllTrades]);
    let started = Instant::now();
    match next_event(&mut events).await {
        WsEvent::Disconnected { reason, .. } => assert!(reason.contains("timed out"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(2));
    ws.shutdown().await;

    let mut cfg = config(url);
    cfg.connect_timeout = Duration::from_secs(60);
    let (ws, _events) = WsClient::spawn(cfg, [Channel::AllTrades]);
    tokio::time::sleep(Duration::from_millis(100)).await;
    timeout(Duration::from_secs(1), ws.shutdown())
        .await
        .expect("shutdown during connect");
}

/// Collects the `retry_in` of the first `n` disconnects.
async fn retry_delays(cfg: WsConfig, n: usize) -> Vec<Duration> {
    let (ws, mut events) = WsClient::spawn(cfg, [Channel::AllTrades]);
    let mut out = Vec::new();
    while out.len() < n {
        if let WsEvent::Disconnected { retry_in, .. } = next_event(&mut events).await {
            out.push(retry_in);
        }
    }
    ws.shutdown().await;
    out
}

/// Finding 4: a server that sends one frame and closes must see growing backoff; only a
/// connection that stayed healthy resets it.
#[tokio::test]
async fn backoff_grows_until_connection_is_healthy() {
    let url = serve(|_, mut ws| async move {
        let _ = ws.send(Message::text(trades_update())).await;
        let _ = ws.close(None).await;
    })
    .await;
    let base = Duration::from_millis(20);
    let delays = retry_delays(config(url.clone()), 5).await;
    for (n, d) in delays.iter().enumerate() {
        let cap = base * 2u32.pow(n as u32);
        assert!(
            *d >= cap / 2 && *d <= cap,
            "attempt {n}: {d:?} (cap {cap:?})"
        );
    }

    let mut cfg = config(url);
    cfg.healthy_after = Duration::ZERO;
    let delays = retry_delays(cfg, 5).await;
    assert!(delays.iter().all(|d| *d <= base), "{delays:?}");
}

/// Finding 5: an error frame carries the full channel and the channel is not resubscribed.
#[tokio::test]
async fn rejected_channel_is_reported_and_dropped() {
    let (tx, mut frames) = mpsc::unbounded_channel();
    let url = serve(move |i, mut ws| {
        let tx = tx.clone();
        async move {
            if i == 0 {
                let Some(Ok(Message::Text(t))) = ws.next().await else {
                    return;
                };
                let _ = tx.send((i, serde_json::from_str(t.as_str()).unwrap()));
                let err = r#"{"channel":"book","symbol":"BAD.SYM","type":"error","data":{"code":"SYMBOL_NOT_FOUND","message":"symbol not found"}}"#;
                ws.send(Message::text(err)).await.unwrap();
                let _ = ws.close(None).await;
            } else {
                record(i, &mut ws, &tx).await;
            }
        }
    })
    .await;
    let bad = Channel::Book {
        symbol: "BAD.SYM".into(),
    };
    let (ws, mut events) = WsClient::spawn(config(url), [Channel::AllTrades, bad.clone()]);
    let (_, first) = next_frame(&mut frames).await;
    assert_eq!(first["channels"].as_array().unwrap().len(), 2);
    loop {
        if let WsEvent::Frame(f) = next_event(&mut events).await
            && let WsMessage::Error { channel, error, .. } = &f.message
        {
            assert_eq!(channel.as_ref(), Some(&bad));
            assert_eq!(error.code, "SYMBOL_NOT_FOUND");
            break;
        }
    }
    let (i, second) = loop {
        let (i, f) = next_frame(&mut frames).await;
        if f["type"] == "subscribe" {
            break (i, f);
        }
    };
    assert_eq!(i, 1);
    assert_eq!(
        second["channels"],
        serde_json::json!([{"channel": "all_trades"}])
    );
    ws.shutdown().await;
}

/// Finding 7: subscriptions are keyed by (kind, owner/symbol); changing `exclude_orders`
/// replaces the subscription, re-subscribing the same descriptor sends nothing, and the
/// option is surfaced on account messages.
#[tokio::test]
async fn account_options_replace_subscription() {
    const OWNER: &str = "7ATdPM8uGMu3qiRSCpc6hUrSZ5VxFSUyYLfRipABG5bC";
    let (tx, mut frames) = mpsc::unbounded_channel();
    let (push_tx, push_rx) = mpsc::unbounded_channel::<String>();
    let push_rx = std::sync::Arc::new(tokio::sync::Mutex::new(push_rx));
    let url = serve(move |i, ws| {
        let (tx, push_rx) = (tx.clone(), push_rx.clone());
        async move {
            let (mut sink, mut stream) = ws.split();
            tokio::spawn(async move {
                while let Some(text) = push_rx.lock().await.recv().await {
                    if sink.send(Message::text(text)).await.is_err() {
                        break;
                    }
                }
            });
            while let Some(Ok(msg)) = stream.next().await {
                if let Message::Text(t) = msg {
                    let _ = tx.send((i, serde_json::from_str::<Value>(t.as_str()).unwrap()));
                }
            }
        }
    })
    .await;
    let with_orders = Channel::Account {
        owner: OWNER.into(),
        exclude_orders: false,
    };
    let (ws, mut events) = WsClient::spawn(config(url), [with_orders]);
    let non_ping = |f: &Value| f["type"] != "ping";
    let mut next = async || loop {
        let (_, f) = next_frame(&mut frames).await;
        if non_ping(&f) {
            return f;
        }
    };
    assert_eq!(
        next().await,
        serde_json::json!({"type":"subscribe","channels":[{"channel":"account","owner":OWNER}]})
    );

    ws.subscribe([Channel::account(OWNER)]).unwrap();
    assert_eq!(
        next().await,
        serde_json::json!({"type":"unsubscribe","channels":[{"channel":"account","owner":OWNER}]})
    );
    assert_eq!(
        next().await,
        serde_json::json!({"type":"subscribe","channels":[{"channel":"account","owner":OWNER,"exclude_orders":true}]})
    );
    // Same descriptor again: nothing is sent; the next frame is the all_trades subscribe.
    ws.subscribe([Channel::account(OWNER)]).unwrap();
    ws.subscribe([Channel::AllTrades]).unwrap();
    assert_eq!(
        next().await,
        serde_json::json!({"type":"subscribe","channels":[{"channel":"all_trades"}]})
    );
    // Unsubscribe matches by key and uses the stored descriptor.
    ws.unsubscribe([Channel::Account {
        owner: OWNER.into(),
        exclude_orders: false,
    }])
    .unwrap();
    assert_eq!(
        next().await,
        serde_json::json!({"type":"unsubscribe","channels":[{"channel":"account","owner":OWNER,"exclude_orders":true}]})
    );

    push_tx
        .send(format!(
            r#"{{"channel":"account","owner":"{OWNER}","exclude_orders":true,"type":"snapshot","data":{{"orders":[],"positions":[],"pending_deposits":[],"pending_withdrawals":[]}},"seq":"5"}}"#
        ))
        .unwrap();
    loop {
        if let WsEvent::Frame(f) = next_event(&mut events).await
            && let WsMessage::Account {
                owner,
                exclude_orders,
                ..
            } = &f.message
        {
            assert_eq!(owner, OWNER);
            assert!(*exclude_orders);
            break;
        }
    }
    ws.shutdown().await;
}
