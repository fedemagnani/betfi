//! `WS /stream`: bus events as C4 frames, one JSON object per text frame.
//!
//! On connect the server sends a heartbeat at once, then forwards the bus. A client may send
//! `{"subscribe":{"user":"<base58>"}}`; from then on `progress` and `ticket` frames are limited to
//! that wallet (heartbeats and the feed go to everyone). Without a subscription every frame is
//! forwarded. A receiver that lags loses frames (counted in `desk_bus_lagged_total{consumer=
//! "stream"}`); the app refetches `/tickets` when it reconnects. A client that does not take a
//! frame within [`SEND_TIMEOUT`] is dropped.

use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::response::Response;

use super::types::{ClientMessage, ProgressStage, StreamFrame};
use crate::now_ms;
use crate::state::{AppState, DeskEvent, Recv, recv};

/// How long one frame may take to send before the client is dropped.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// Upgrades to the stream session.
pub async fn stream(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| session(socket, state))
}

/// The C4 frame for `event` as seen by a client subscribed to `user` (`None` = no filter).
pub fn frame_for(event: &DeskEvent, user: Option<&str>) -> Option<StreamFrame> {
    if let (Some(want), Some(owner)) = (user, event.user())
        && want != owner
    {
        return None;
    }
    Some(match event {
        DeskEvent::Heartbeat { ts_ms } => StreamFrame::Heartbeat { ts_ms: *ts_ms },
        DeskEvent::PendingOpened(p) => StreamFrame::Progress {
            pending: p.pending.clone(),
            stage: ProgressStage::Escrowed,
            fills: Vec::new(),
            ticket: None,
        },
        DeskEvent::HedgeFilled(h) => StreamFrame::Progress {
            pending: h.pending.clone(),
            stage: ProgressStage::Hedging,
            fills: h.fills.clone(),
            ticket: None,
        },
        DeskEvent::TicketMinted {
            pending, ticket, ..
        } => StreamFrame::Progress {
            pending: pending.clone(),
            stage: ProgressStage::Minted,
            fills: Vec::new(),
            ticket: Some(ticket.clone()),
        },
        DeskEvent::PendingRefunded { pending, .. } => StreamFrame::Progress {
            pending: pending.clone(),
            stage: ProgressStage::Refunded,
            fills: Vec::new(),
            ticket: None,
        },
        DeskEvent::TicketChanged(t) => StreamFrame::Ticket {
            ticket: Box::new((**t).clone()),
        },
        DeskEvent::Feed(f) => StreamFrame::Feed {
            kind: f.kind,
            text: f.text.clone(),
            ts_ms: f.ts_ms,
            who: f.who.clone(),
            amount_u: f.amount_u,
            team: f.team.clone(),
            odds_u: f.odds_u,
        },
        DeskEvent::CatalogueRefreshed { .. }
        | DeskEvent::HedgeUnwound(_)
        | DeskEvent::FinalizeRejected { .. }
        | DeskEvent::RefundNeeded { .. }
        | DeskEvent::MarketResolved { .. }
        | DeskEvent::LegResolved { .. }
        | DeskEvent::TicketSettled { .. }
        | DeskEvent::TicketCashedOut { .. } => return None,
    })
}

async fn send(socket: &mut WebSocket, frame: &StreamFrame) -> Result<(), ()> {
    let text = serde_json::to_string(frame).map_err(|_| ())?;
    match tokio::time::timeout(
        SEND_TIMEOUT,
        socket.send(Message::Text(Utf8Bytes::from(text))),
    )
    .await
    {
        Ok(Ok(())) => Ok(()),
        _ => Err(()),
    }
}

async fn session(mut socket: WebSocket, state: AppState) {
    // Subscribe before the first frame, so nothing emitted after the handshake is missed.
    let mut rx = state.subscribe();
    let clients = metrics::gauge!("desk_stream_clients");
    clients.increment(1.0);
    let mut user: Option<String> = None;
    let hello = StreamFrame::Heartbeat { ts_ms: now_ms() };
    if send(&mut socket, &hello).await.is_ok() {
        loop {
            tokio::select! {
                _ = state.shutdown_requested() => {
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
                msg = socket.recv() => match msg {
                    Some(Ok(Message::Text(t))) => match serde_json::from_str::<ClientMessage>(&t) {
                        Ok(m) => user = Some(m.subscribe.user),
                        Err(e) => tracing::debug!(error = %e, "ignored /stream client message"),
                    },
                    // Pings are answered by the socket itself; binary frames are ignored.
                    Some(Ok(Message::Binary(_) | Message::Ping(_) | Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                },
                ev = recv(&mut rx, "stream") => match ev {
                    Recv::Event(e) => {
                        if let Some(frame) = frame_for(&e, user.as_deref())
                            && send(&mut socket, &frame).await.is_err()
                        {
                            break;
                        }
                    }
                    Recv::Lagged(_) => {}
                    Recv::Closed => break,
                },
            }
        }
    }
    clients.decrement(1.0);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::api::types::FeedKind;
    use crate::state::{FeedItem, PendingOpened};

    fn pending(user: &str) -> DeskEvent {
        DeskEvent::PendingOpened(Arc::new(PendingOpened {
            pending: "P".into(),
            ticket: "T".into(),
            user: user.into(),
            nonce: 1,
            stake_u: 5_000_000,
            copy_fee_u: 0,
            odds_offered_u: 2_000_000,
            min_payout_per_unit_u: 1_900_000,
            expires_at: 0,
            legs: vec![],
            tx: "S".into(),
            slot: 1,
        }))
    }

    #[test]
    fn maps_and_filters() {
        let f = frame_for(&pending("alice"), None).unwrap();
        assert_eq!(
            serde_json::to_value(&f).unwrap(),
            serde_json::json!({"type":"progress","pending":"P","stage":"escrowed","fills":[],"ticket":null})
        );
        assert!(frame_for(&pending("alice"), Some("bob")).is_none());
        assert!(frame_for(&pending("alice"), Some("alice")).is_some());
        let feed = DeskEvent::Feed(Arc::new(FeedItem {
            kind: FeedKind::Mint,
            text: "minted".into(),
            ticket: None,
            ts_ms: 7,
            who: Some("lorenzo_hoops".into()),
            amount_u: Some(25_000_000),
            team: Some("76ers".into()),
            odds_u: Some(1_880_000),
        }));
        assert_eq!(
            serde_json::to_value(frame_for(&feed, Some("bob")).unwrap()).unwrap(),
            serde_json::json!({"type":"feed","kind":"mint","text":"minted","ts_ms":7,
                "who":"lorenzo_hoops","amount_u":25_000_000,"team":"76ers","odds_u":1_880_000})
        );
        let hb = DeskEvent::Heartbeat { ts_ms: 9 };
        assert_eq!(
            serde_json::to_value(frame_for(&hb, Some("bob")).unwrap()).unwrap(),
            serde_json::json!({"type":"heartbeat","ts_ms":9})
        );
        assert!(
            frame_for(
                &DeskEvent::CatalogueRefreshed {
                    as_of_ms: 1,
                    markets: 1
                },
                None
            )
            .is_none()
        );
    }
}
