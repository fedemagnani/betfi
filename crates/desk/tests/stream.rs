//! Review gate: `WS /stream` delivers heartbeat frames (one on connect, then from the bus at the
//! configured period), forwards bus events as C4 frames, honours `subscribe`, and closes on
//! shutdown.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{TempDb, pascal_stub, start_desk};
use desk::DeskEvent;
use desk::api::types::{FeedKind, StreamFrame};
use desk::state::{FeedItem, PendingOpened};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next_frame(ws: &mut Ws) -> Option<StreamFrame> {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("a frame within 5 s")?
            .ok()?;
        match msg {
            Message::Text(t) => return Some(serde_json::from_str(&t).expect("a C4 frame")),
            Message::Close(_) => return None,
            _ => {}
        }
    }
}

/// The next frame that is not a heartbeat.
async fn next_event(ws: &mut Ws) -> Option<StreamFrame> {
    loop {
        match next_frame(ws).await? {
            StreamFrame::Heartbeat { .. } => {}
            f => return Some(f),
        }
    }
}

fn pending(user: &str, pda: &str) -> DeskEvent {
    DeskEvent::PendingOpened(Arc::new(PendingOpened {
        pending: pda.into(),
        ticket: format!("T{pda}"),
        user: user.into(),
        nonce: 1,
        stake_u: 5_000_000,
        copy_fee_u: 0,
        odds_offered_u: 2_959_085,
        min_payout_per_unit_u: 2_870_312,
        expires_at: 0,
        legs: vec![],
        tx: "sig".into(),
        slot: 1,
    }))
}

#[tokio::test(flavor = "multi_thread")]
async fn heartbeats_events_and_shutdown() {
    let pascal = pascal_stub().await;
    let db = TempDb::new("stream");
    let desk = start_desk(&pascal, &db).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/stream", desk.addr))
        .await
        .expect("ws connects");

    // A heartbeat at once, then the bus heartbeat (200 ms in this test).
    let mut beats = 0;
    while beats < 3 {
        match next_frame(&mut ws).await.expect("open stream") {
            StreamFrame::Heartbeat { ts_ms } => {
                assert!(ts_ms > 1_700_000_000_000);
                beats += 1;
            }
            other => panic!("unexpected frame {other:?}"),
        }
    }

    // Unsubscribed: every user's progress frames.
    desk.state.emit(pending("alice", "P1"));
    let f = next_event(&mut ws).await.unwrap();
    assert_eq!(
        serde_json::to_value(&f).unwrap(),
        serde_json::json!({"type":"progress","pending":"P1","stage":"escrowed","fills":[],"ticket":null})
    );

    // Subscribed to bob: alice's progress is filtered out, the feed is not.
    ws.send(Message::text(r#"{"subscribe":{"user":"bob"}}"#))
        .await
        .unwrap();
    // Let the server read the subscription before emitting.
    tokio::time::sleep(Duration::from_millis(300)).await;
    desk.state.emit(pending("alice", "P2"));
    desk.state.emit(DeskEvent::Feed(Arc::new(FeedItem {
        kind: FeedKind::Mint,
        text: "ticket #7 minted".into(),
        ticket: None,
        ts_ms: 1,
        who: None,
        amount_u: None,
        team: None,
        odds_u: None,
    })));
    desk.state.emit(pending("bob", "P3"));
    match next_event(&mut ws).await.unwrap() {
        StreamFrame::Feed { kind, text, .. } => {
            assert_eq!(kind, FeedKind::Mint);
            assert_eq!(text, "ticket #7 minted");
        }
        other => panic!("expected the feed frame, got {other:?}"),
    }
    match next_event(&mut ws).await.unwrap() {
        StreamFrame::Progress { pending, .. } => assert_eq!(pending, "P3"),
        other => panic!("expected bob's progress, got {other:?}"),
    }

    // Shutdown closes the socket and the desk stops within the grace period.
    desk.shutdown();
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        while next_frame(&mut ws).await.is_some() {}
    })
    .await;
    assert!(closed.is_ok(), "socket not closed on shutdown");
    tokio::time::timeout(Duration::from_secs(10), desk.join())
        .await
        .expect("graceful shutdown");
}
