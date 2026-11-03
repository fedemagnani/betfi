//! Live smoke test against production. Ignored by default (network, and `/markets` is
//! rate-limited): run with `cargo test -p pascal-rs --test live_smoke -- --ignored`.

use std::time::Duration;

use pascal_rs::{Channel, Client, FrameKind, WsClient, WsConfig, WsEvent, WsMessage};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "hits the live Pascal API"]
async fn live_time_markets_and_one_ws_frame() {
    let client = Client::new().unwrap();

    let t = client.time().await.expect("/api/v1/time");
    let seq = t.seq.expect("seq on /time");
    assert!(t.data.server_time_ms > 1_790_000_000_000);
    println!("time: server_time_ms={} seq={seq}", t.data.server_time_ms);

    // Exactly one call to the ~5 MB list.
    let markets = client.markets_lenient().await.expect("/api/v1/markets");
    println!(
        "markets: {} decoded, {} failed, seq={:?}, raw items={}",
        markets.data.items.len(),
        markets.data.failures.len(),
        markets.seq,
        markets.raw_items().len()
    );
    for f in &markets.data.failures {
        println!("  failure at {}: {}", f.index, f.error);
    }
    assert!(markets.data.items.len() > 1000);
    assert!(
        markets.data.failures.is_empty(),
        "every live market decodes"
    );
    // The list is served from a cache: its seq can be older than the /time call made before it.
    println!(
        "markets seq lags /time seq by {}",
        seq as i64 - markets.seq.unwrap() as i64
    );

    let (ws, mut events) = WsClient::spawn(WsConfig::default(), [Channel::AllTrades]);
    let frame = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match events.recv().await.expect("ws task alive") {
                WsEvent::Frame(f) => return f,
                WsEvent::Connected { channels } => println!("ws: connected, {channels} channel(s)"),
                other => panic!("unexpected ws event: {other:?}"),
            }
        }
    })
    .await
    .expect("a ws frame within 30 s");
    match &frame.message {
        WsMessage::AllTrades { kind, trades } => {
            println!(
                "ws: all_trades {:?} with {} trades, seq={:?}",
                kind,
                trades.len(),
                frame.seq
            );
            assert_eq!(*kind, FrameKind::Snapshot);
            assert!(!trades.is_empty());
        }
        other => panic!("expected all_trades snapshot, got {other:?}"),
    }
    ws.shutdown().await;
}
