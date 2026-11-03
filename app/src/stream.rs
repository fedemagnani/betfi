//! The single `/stream` WebSocket (C4) feeding `Signal<AppState>`.
//!
//! One coroutine, started by the root component, owns the socket. It reconnects with
//! exponential backoff and jitter (0.5 s doubling to 30 s, reset after a session that
//! delivered a frame), treats 45 s without any frame as a dead link, and resends the current user's
//! `subscribe` on every (re)connect. The coroutine is cancelled when the root unmounts; dropping
//! the gloo `WebSocket` closes it and removes its JS listeners, so nothing leaks across
//! reconnects. Frames that do not decode are counted and ignored, never fatal.
//!
//! In demo mode (no desk URL) the same coroutine replays `fixtures/desk/stream.json` through the
//! same decoder instead of opening a socket.

use dioxus::logger::tracing::{debug, warn};
use dioxus::prelude::*;
use futures_util::future::{Either, poll_fn, select};
use futures_util::{SinkExt, StreamExt};
use gloo_net::websocket::{Message, State, futures::WebSocket};
use gloo_timers::future::TimeoutFuture;
use serde::Deserialize;

use crate::desk::Desk;
use crate::desk::fixtures;
use crate::desk::types::{Frame, Subscribe, SubscribeUser};
use crate::state::{AppState, Link};

const BACKOFF_BASE_MS: u32 = 500;
const BACKOFF_MAX_MS: u32 = 30_000;
/// No frame at all (heartbeats included) for this long: the link is dead, reconnect.
const SILENCE_MS: u32 = 45_000;
/// A socket still not open after this long is abandoned.
const OPEN_TIMEOUT_MS: u32 = 10_000;

/// Commands to the stream coroutine.
#[derive(Debug, Clone)]
pub enum StreamCmd {
    /// Follow this user's tickets (sent now and on every reconnect).
    Subscribe(String),
    /// Demo mode: play the example frames again from the start.
    ReplayDemo,
}

/// Start the stream coroutine. Call once, from the root component.
pub fn use_stream(state: Signal<AppState>, desk: Desk) -> Coroutine<StreamCmd> {
    use_coroutine(move |rx: UnboundedReceiver<StreamCmd>| {
        let desk = desk.clone();
        async move {
            match desk.stream_url() {
                Some(url) => run_socket(url, state, rx).await,
                None => run_demo(state, rx).await,
            }
        }
    })
}

/// Decode one text frame into the state. Heartbeats do not write the signal, so they do not
/// re-render every screen.
fn ingest(mut state: Signal<AppState>, text: &str) {
    match serde_json::from_str::<Frame>(text) {
        Ok(Frame::Heartbeat { .. }) => {}
        Ok(frame) => state.write().apply(frame),
        Err(e) => {
            warn!("ignored a /stream frame that does not decode: {e}");
            let mut s = state.write();
            s.bad_frames = s.bad_frames.saturating_add(1);
        }
    }
}

fn set_link(mut state: Signal<AppState>, link: Link) {
    if state.peek().link != link {
        state.write().link = link;
    }
}

/// Send `{"subscribe":{"user":…}}`; false when the socket refused it.
async fn send_subscribe(ws: &mut WebSocket, user: &str) -> bool {
    let Ok(frame) = serde_json::to_string(&Subscribe {
        subscribe: SubscribeUser {
            user: user.to_string(),
        },
    }) else {
        return true;
    };
    ws.send(Message::Text(frame)).await.is_ok()
}

/// `base * 2^(attempt-1)`, capped, with ±20 % jitter so many clients do not reconnect in step.
fn backoff_ms(attempt: u32) -> u32 {
    let exp = BACKOFF_BASE_MS.saturating_mul(1u32 << attempt.saturating_sub(1).min(16));
    let capped = exp.min(BACKOFF_MAX_MS) as f64;
    let jitter = 0.8 + 0.4 * js_sys::Math::random();
    (capped * jitter) as u32
}

enum SessionEnd {
    /// The socket closed, failed or went silent. `healthy`: it delivered at least one frame
    /// (only then is the backoff reset, so a server that accepts and drops at once is not
    /// hammered every 0.5 s).
    Dropped { healthy: bool },
    /// The command channel closed: the app is going away.
    Shutdown,
}

async fn run_socket(url: String, state: Signal<AppState>, mut rx: UnboundedReceiver<StreamCmd>) {
    let mut attempt: u32 = 0;
    loop {
        set_link(state, Link::Connecting);
        let end = match WebSocket::open(&url) {
            Ok(ws) => session(ws, state, &mut rx).await,
            Err(e) => {
                warn!("/stream: cannot open {url}: {e}");
                SessionEnd::Dropped { healthy: false }
            }
        };
        match end {
            SessionEnd::Shutdown => return,
            SessionEnd::Dropped { healthy } => {
                if healthy {
                    attempt = 0;
                }
            }
        }
        attempt = attempt.saturating_add(1);
        let wait = backoff_ms(attempt);
        set_link(
            state,
            Link::Retrying {
                attempt,
                in_ms: wait,
            },
        );
        // Wait out the backoff, still draining commands (the subscribe is resent on reconnect
        // from the state, so queued ones need not be kept) and noticing shutdown.
        let mut timer = TimeoutFuture::new(wait);
        loop {
            match select(rx.next(), &mut timer).await {
                Either::Left((Some(_), _)) => continue,
                Either::Left((None, _)) => return,
                Either::Right(_) => break,
            }
        }
    }
}

enum Event {
    Ws(Option<Result<Message, gloo_net::websocket::WebSocketError>>),
    Cmd(Option<StreamCmd>),
    Silence,
}

async fn session(
    mut ws: WebSocket,
    state: Signal<AppState>,
    rx: &mut UnboundedReceiver<StreamCmd>,
) -> SessionEnd {
    // Phase 1: wait for open (the sink is ready once the socket leaves CONNECTING; an error
    // event also wakes it), bounded by a timeout.
    let ready = select(
        poll_fn(|cx| ws.poll_ready_unpin(cx)),
        TimeoutFuture::new(OPEN_TIMEOUT_MS),
    )
    .await;
    if matches!(ready, Either::Right(_)) || !matches!(ws.state(), State::Open) {
        return SessionEnd::Dropped { healthy: false };
    }
    set_link(state, Link::Live);
    debug!("/stream open");
    let mut healthy = false;

    let user = state.peek().user.clone();
    if let Some(user) = user
        && !send_subscribe(&mut ws, &user).await
    {
        return SessionEnd::Dropped { healthy };
    }

    // Phase 2: frames and commands until the socket ends, goes silent, or the app shuts down.
    loop {
        // The futures borrowing `ws` and `rx` are dropped at the end of this statement.
        let event = match select(select(ws.next(), rx.next()), TimeoutFuture::new(SILENCE_MS)).await
        {
            Either::Left((Either::Left((msg, _)), _)) => Event::Ws(msg),
            Either::Left((Either::Right((cmd, _)), _)) => Event::Cmd(cmd),
            Either::Right(_) => Event::Silence,
        };
        match event {
            Event::Ws(Some(Ok(Message::Text(text)))) => {
                healthy = true;
                ingest(state, &text);
            }
            Event::Ws(Some(Ok(Message::Bytes(_)))) => {
                warn!("/stream: ignored a binary frame");
            }
            Event::Ws(Some(Err(e))) => {
                warn!("/stream dropped: {e}");
                return SessionEnd::Dropped { healthy };
            }
            Event::Ws(None) => return SessionEnd::Dropped { healthy },
            Event::Silence => {
                warn!("/stream silent for {SILENCE_MS} ms, reconnecting");
                return SessionEnd::Dropped { healthy };
            }
            Event::Cmd(Some(StreamCmd::Subscribe(user))) => {
                if !send_subscribe(&mut ws, &user).await {
                    return SessionEnd::Dropped { healthy };
                }
            }
            Event::Cmd(Some(StreamCmd::ReplayDemo)) => {}
            Event::Cmd(None) => return SessionEnd::Shutdown,
        }
    }
}

#[derive(Deserialize)]
struct ScriptedFrame {
    delay_ms: u32,
    frame: String,
}

async fn run_demo(mut state: Signal<AppState>, mut rx: UnboundedReceiver<StreamCmd>) {
    set_link(state, Link::Demo);
    let script: Vec<ScriptedFrame> = match serde_json::from_str(fixtures::STREAM) {
        Ok(s) => s,
        Err(e) => {
            warn!("demo stream script does not decode: {e}");
            Vec::new()
        }
    };
    // A replay asked mid-script restarts it at once, so the screen that asked sees the whole
    // sequence. The subscribe is meaningless without a desk.
    'replay: loop {
        for step in &script {
            let mut timer = TimeoutFuture::new(step.delay_ms);
            loop {
                match select(rx.next(), &mut timer).await {
                    Either::Left((Some(StreamCmd::ReplayDemo), _)) => {
                        state.write().progress.clear();
                        continue 'replay;
                    }
                    Either::Left((Some(StreamCmd::Subscribe(_)), _)) => {}
                    Either::Left((None, _)) => return,
                    Either::Right(_) => break,
                }
            }
            ingest(state, &step.frame);
        }
        loop {
            match rx.next().await {
                Some(StreamCmd::ReplayDemo) => {
                    state.write().progress.clear();
                    continue 'replay;
                }
                Some(StreamCmd::Subscribe(_)) => {}
                None => return,
            }
        }
    }
}
