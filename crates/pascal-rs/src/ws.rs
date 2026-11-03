//! Market-data WebSocket client for `wss://data.pascal.trade/ws`.
//!
//! [`WsClient::spawn`] runs one connection in a background task that
//! - sends `subscribe` / `unsubscribe` frames (chunked to stay under the 64 KiB frame limit),
//! - sends `{"type":"ping"}` every 30 s (the server closes idle sockets at 60 s) and treats a
//!   socket that has been silent for longer than the idle timeout as dead,
//! - reconnects with exponential backoff and jitter, then resubscribes every channel,
//! - decodes frames into [`WsFrame`]s and delivers them as [`WsEvent`]s.
//!
//! After every (re)connect each channel starts again with a snapshot, so consumers should
//! treat a snapshot as authoritative state and updates as deltas on top of it, ordering by
//! `seq` against REST data.
//!
//! **Slow consumers.** The task never blocks on the event channel: while an event waits for
//! room it stops reading the socket but keeps pinging and keeps obeying commands (including
//! shutdown). If an event cannot be delivered within [`WsConfig::lag_timeout`] the connection
//! is dropped, [`WsEvent::Lagged`] is emitted, and the reconnect resynchronises every channel
//! with fresh snapshots.

use std::collections::BTreeMap;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::envelope::Lenient;
use crate::error::{ApiError, Error, ErrorBody, Result};
use crate::num::opt_str_num;
use crate::ratelimit::Backoff;
use crate::types::{AccountState, MarketUpdate, OrderBook, PublicTrade};

/// Production market-data socket.
pub const DEFAULT_WS_URL: &str = "wss://data.pascal.trade/ws";

/// Channels per subscribe frame. An `account` descriptor is ~80 bytes, so 400 stay well under
/// Pascal's 64 KiB client frame limit.
const CHANNELS_PER_FRAME: usize = 400;

/// A channel descriptor, serialised exactly as Pascal expects in `channels[]`.
///
/// A subscription is identified by its [`ChannelKey`] (kind plus owner or symbol); options such
/// as `exclude_orders` are not part of the identity. Subscribing to a key that is already
/// subscribed with different options replaces the old subscription.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "channel", rename_all = "snake_case")]
pub enum Channel {
    /// Every public trade across all markets.
    AllTrades,
    /// Live state of one account: snapshot, then deltas (fills, positions, transfers, …).
    Account {
        /// Wallet (base58).
        owner: String,
        /// Omit resting orders (large market makers rest tens of thousands).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        exclude_orders: bool,
    },
    /// Listings, display changes and resolutions of all markets.
    MarketUpdate,
    /// L2 book of one market.
    Book {
        /// Market symbol.
        symbol: String,
    },
    /// Trades of one market.
    Trades {
        /// Market symbol.
        symbol: String,
    },
}

/// Identity of a subscription: the channel kind and its owner or symbol, without options.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ChannelKey {
    /// `all_trades`.
    AllTrades,
    /// `account` for an owner.
    Account(String),
    /// `market_update`.
    MarketUpdate,
    /// `book` for a symbol.
    Book(String),
    /// `trades` for a symbol.
    Trades(String),
}

impl Channel {
    /// `account` channel for `owner` with orders excluded (what the indexer wants).
    pub fn account(owner: impl Into<String>) -> Self {
        Channel::Account {
            owner: owner.into(),
            exclude_orders: true,
        }
    }

    /// The subscription identity of this channel.
    pub fn key(&self) -> ChannelKey {
        match self {
            Channel::AllTrades => ChannelKey::AllTrades,
            Channel::Account { owner, .. } => ChannelKey::Account(owner.clone()),
            Channel::MarketUpdate => ChannelKey::MarketUpdate,
            Channel::Book { symbol } => ChannelKey::Book(symbol.clone()),
            Channel::Trades { symbol } => ChannelKey::Trades(symbol.clone()),
        }
    }
}

#[derive(Serialize)]
struct ClientFrame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    channels: &'a [Channel],
}

/// The JSON text of a subscribe frame for `channels`.
pub fn subscribe_frame(channels: &[Channel]) -> String {
    client_frame("subscribe", channels)
}

/// The JSON text of an unsubscribe frame for `channels`.
pub fn unsubscribe_frame(channels: &[Channel]) -> String {
    client_frame("unsubscribe", channels)
}

/// The JSON text of a ping frame.
pub const PING_FRAME: &str = r#"{"type":"ping"}"#;

fn client_frame(kind: &'static str, channels: &[Channel]) -> String {
    serde_json::to_string(&ClientFrame { kind, channels }).expect("channel descriptors serialise")
}

/// Whether a frame is the channel's initial state or a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameKind {
    /// Initial state after (re)subscribing.
    Snapshot,
    /// Change since the previous frame.
    Update,
}

/// A decoded server frame.
#[derive(Debug, Clone)]
pub struct WsFrame {
    /// The typed message.
    pub message: WsMessage,
    /// Matching-engine sequence of the state that produced the frame.
    pub seq: Option<u64>,
    /// Matching-engine time, Unix ms.
    pub state_time_ms: Option<u64>,
    /// Server time, Unix ms.
    pub server_time_ms: Option<u64>,
    /// The frame as received, for raw storage.
    pub raw: Value,
}

/// Typed payload of a server frame.
#[derive(Debug, Clone)]
pub enum WsMessage {
    /// `all_trades`: trades oldest first (snapshot = up to 100 recent trades).
    AllTrades {
        /// Snapshot or update.
        kind: FrameKind,
        /// Trades.
        trades: Vec<PublicTrade>,
    },
    /// `trades` for one market.
    Trades {
        /// Market symbol.
        symbol: String,
        /// Snapshot or update.
        kind: FrameKind,
        /// Trades.
        trades: Vec<PublicTrade>,
    },
    /// `account` state (snapshot) or changes (update).
    Account {
        /// Wallet.
        owner: String,
        /// Whether the subscription excludes orders (echoed by the server).
        exclude_orders: bool,
        /// Snapshot or update.
        kind: FrameKind,
        /// State or delta.
        state: Box<AccountState>,
    },
    /// `market_update`: the snapshot is empty by design; updates carry listings, display
    /// changes and resolutions. Decoded item by item so one unknown shape does not drop the rest.
    MarketUpdate {
        /// Snapshot or update.
        kind: FrameKind,
        /// Updates, each with its index in `raw["data"]`.
        updates: Lenient<MarketUpdate>,
    },
    /// `book` for one market (updates: changed levels, size 0 = removed).
    Book {
        /// Market symbol.
        symbol: String,
        /// Snapshot or update.
        kind: FrameKind,
        /// Levels.
        book: OrderBook,
    },
    /// An error frame. When it names a channel, [`WsClient`] drops that channel from its
    /// subscription set (it is not resubscribed on reconnect); subscribe again to retry.
    Error {
        /// The channel the error is about, with its owner / symbol, when the frame names a
        /// channel this crate knows.
        channel: Option<Channel>,
        /// The raw channel name, also for channels this crate does not model.
        channel_name: Option<String>,
        /// The error.
        error: ApiError,
    },
    /// Reply to a ping.
    Pong,
}

#[derive(Deserialize)]
struct FrameHead {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    exclude_orders: Option<bool>,
    #[serde(default, with = "opt_str_num")]
    seq: Option<u64>,
    #[serde(default, with = "opt_str_num")]
    state_time_ms: Option<u64>,
    #[serde(default, with = "opt_str_num")]
    server_time_ms: Option<u64>,
}

impl FrameHead {
    /// The channel descriptor carried by the frame, if it names a known channel.
    fn channel(&self) -> Option<Channel> {
        Some(match self.channel.as_deref()? {
            "all_trades" => Channel::AllTrades,
            "market_update" => Channel::MarketUpdate,
            "account" => Channel::Account {
                owner: self.owner.clone()?,
                exclude_orders: self.exclude_orders.unwrap_or(false),
            },
            "book" => Channel::Book {
                symbol: self.symbol.clone()?,
            },
            "trades" => Channel::Trades {
                symbol: self.symbol.clone()?,
            },
            _ => return None,
        })
    }
}

impl WsFrame {
    /// Decodes one text frame. Unknown channels or frame types are an error, never skipped.
    pub fn parse(text: &str) -> Result<WsFrame> {
        let raw: Value = serde_json::from_str(text).map_err(|source| Error::Decode {
            context: "websocket frame".into(),
            source,
        })?;
        Self::from_value(raw)
    }

    /// Decodes a frame that is already parsed as JSON.
    pub fn from_value(raw: Value) -> Result<WsFrame> {
        let head = FrameHead::deserialize(&raw).map_err(|source| Error::Decode {
            context: "websocket frame header".into(),
            source,
        })?;
        let message = decode_message(&head, &raw)?;
        Ok(WsFrame {
            message,
            seq: head.seq,
            state_time_ms: head.state_time_ms,
            server_time_ms: head.server_time_ms,
            raw,
        })
    }
}

fn decode_message(head: &FrameHead, raw: &Value) -> Result<WsMessage> {
    let channel = head.channel.as_deref().unwrap_or("");
    let context = format!("websocket {channel} {}", head.kind);
    let data = raw.get("data").unwrap_or(&Value::Null);
    let decode = |source| Error::Decode {
        context: context.clone(),
        source,
    };
    let need = |v: &Option<String>, field: &'static str| {
        v.clone()
            .ok_or_else(|| decode(serde::de::Error::missing_field(field)))
    };
    let kind = match head.kind.as_str() {
        "snapshot" => FrameKind::Snapshot,
        "update" => FrameKind::Update,
        "pong" => return Ok(WsMessage::Pong),
        "error" => {
            let body = ErrorBody::deserialize(data).map_err(decode)?;
            return Ok(WsMessage::Error {
                channel: head.channel(),
                channel_name: head.channel.clone(),
                error: ApiError {
                    http_status: None,
                    code: body.code,
                    message: body.message,
                    seq: head.seq,
                    server_time_ms: head.server_time_ms,
                },
            });
        }
        other => {
            return Err(decode(serde::de::Error::unknown_variant(
                other,
                &["snapshot", "update", "error", "pong"],
            )));
        }
    };
    Ok(match channel {
        "all_trades" => WsMessage::AllTrades {
            kind,
            trades: Vec::deserialize(data).map_err(decode)?,
        },
        "trades" => WsMessage::Trades {
            symbol: need(&head.symbol, "symbol")?,
            kind,
            trades: Vec::deserialize(data).map_err(decode)?,
        },
        "account" => WsMessage::Account {
            owner: need(&head.owner, "owner")?,
            exclude_orders: head.exclude_orders.unwrap_or(false),
            kind,
            state: Box::new(AccountState::deserialize(data).map_err(decode)?),
        },
        "market_update" => WsMessage::MarketUpdate {
            kind,
            updates: Lenient::from_value(data).map_err(decode)?,
        },
        "book" => WsMessage::Book {
            symbol: need(&head.symbol, "symbol")?,
            kind,
            book: OrderBook::deserialize(data).map_err(decode)?,
        },
        other => {
            return Err(decode(serde::de::Error::unknown_variant(
                other,
                &["all_trades", "trades", "account", "market_update", "book"],
            )));
        }
    })
}

/// Settings for [`WsClient`].
#[derive(Debug, Clone)]
pub struct WsConfig {
    /// Socket URL (default [`DEFAULT_WS_URL`]).
    pub url: String,
    /// Ping cadence (default 30 s; the server drops sockets idle for 60 s).
    pub ping_interval: Duration,
    /// Reconnect when nothing (not even a pong) arrived for this long (default 75 s).
    pub idle_timeout: Duration,
    /// Give up a connection attempt (TCP + TLS + upgrade) after this long (default 15 s).
    pub connect_timeout: Duration,
    /// Reconnect backoff (default 1 s doubling to 60 s, with jitter).
    pub reconnect: Backoff,
    /// A connection that stayed up this long resets the backoff (default 30 s), so a server
    /// that accepts, sends a frame and closes still gets exponentially slower retries.
    pub healthy_after: Duration,
    /// Drop the connection (and resync on reconnect) when an event waits longer than this for
    /// room in the event channel (default 30 s).
    pub lag_timeout: Duration,
    /// Largest inbound message accepted (default 64 MiB: a big maker's account snapshot with
    /// orders is ~22 MB).
    pub max_message_size: usize,
    /// Capacity of the event channel.
    pub event_buffer: usize,
}

impl Default for WsConfig {
    fn default() -> Self {
        WsConfig {
            url: DEFAULT_WS_URL.to_owned(),
            ping_interval: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(75),
            connect_timeout: Duration::from_secs(15),
            reconnect: Backoff {
                base: Duration::from_secs(1),
                max: Duration::from_secs(60),
            },
            healthy_after: Duration::from_secs(30),
            lag_timeout: Duration::from_secs(30),
            max_message_size: 64 << 20,
            event_buffer: 4096,
        }
    }
}

/// What the background task reports.
#[derive(Debug)]
pub enum WsEvent {
    /// Connected and subscribe frames sent; snapshots follow.
    Connected {
        /// Number of channels (re)subscribed.
        channels: usize,
    },
    /// A decoded frame.
    Frame(Box<WsFrame>),
    /// A frame that could not be decoded (shape change or unknown channel). Not fatal.
    DecodeError {
        /// The decode error.
        error: Error,
        /// The frame text (truncated to 4 KiB).
        text: String,
    },
    /// The consumer did not take an event within [`WsConfig::lag_timeout`]: the connection was
    /// dropped (the undelivered frame with it) and will be re-established with fresh snapshots.
    Lagged {
        /// How long the event waited.
        waited: Duration,
    },
    /// The connection dropped; a reconnect follows after `retry_in`.
    Disconnected {
        /// Why.
        reason: String,
        /// Delay before the next attempt.
        retry_in: Duration,
    },
}

enum Command {
    Subscribe(Vec<Channel>),
    Unsubscribe(Vec<Channel>),
    Shutdown,
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Command::Subscribe(c) => write!(f, "Subscribe({})", c.len()),
            Command::Unsubscribe(c) => write!(f, "Unsubscribe({})", c.len()),
            Command::Shutdown => write!(f, "Shutdown"),
        }
    }
}

/// Handle to the background WebSocket task. Dropping it shuts the task down.
#[derive(Debug)]
pub struct WsClient {
    commands: mpsc::UnboundedSender<Command>,
    task: Option<JoinHandle<()>>,
}

impl WsClient {
    /// Starts the connection task with an initial set of channels. Must be called inside a
    /// Tokio runtime.
    pub fn spawn(
        config: WsConfig,
        channels: impl IntoIterator<Item = Channel>,
    ) -> (WsClient, mpsc::Receiver<WsEvent>) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = mpsc::channel(config.event_buffer.max(1));
        let mut subs = Subs::new();
        for ch in channels {
            subs.insert(ch.key(), ch);
        }
        let task = tokio::spawn(run(config, subs, cmd_rx, ev_tx));
        (
            WsClient {
                commands: cmd_tx,
                task: Some(task),
            },
            ev_rx,
        )
    }

    /// Adds channels (or replaces the options of an already subscribed key); they are sent
    /// now if connected and on every reconnect.
    pub fn subscribe(&self, channels: impl IntoIterator<Item = Channel>) -> Result<()> {
        self.send(Command::Subscribe(channels.into_iter().collect()))
    }

    /// Removes channels, matched by [`Channel::key`].
    pub fn unsubscribe(&self, channels: impl IntoIterator<Item = Channel>) -> Result<()> {
        self.send(Command::Unsubscribe(channels.into_iter().collect()))
    }

    /// Closes the socket and waits for the task to end. Returns promptly even if the consumer
    /// has stopped reading events or a connection attempt is in flight.
    pub async fn shutdown(mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(t) = self.task.take() {
            let _ = t.await;
        }
    }

    fn send(&self, c: Command) -> Result<()> {
        self.commands
            .send(c)
            .map_err(|_| Error::InvalidArgument("websocket task has stopped".into()))
    }
}

impl Drop for WsClient {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
    }
}

type Subs = BTreeMap<ChannelKey, Channel>;

/// Applies a subscription change while disconnected. Returns false on shutdown.
fn apply_offline(subs: &mut Subs, cmd: Option<Command>) -> bool {
    match cmd {
        None | Some(Command::Shutdown) => false,
        Some(Command::Subscribe(c)) => {
            for ch in c {
                subs.insert(ch.key(), ch);
            }
            true
        }
        Some(Command::Unsubscribe(c)) => {
            for ch in c {
                subs.remove(&ch.key());
            }
            true
        }
    }
}

/// Delivers an event while still obeying commands. Returns false on shutdown or when the
/// receiver is gone.
async fn deliver(
    events: &mpsc::Sender<WsEvent>,
    event: WsEvent,
    subs: &mut Subs,
    commands: &mut mpsc::UnboundedReceiver<Command>,
) -> bool {
    let send = events.send(event);
    tokio::pin!(send);
    loop {
        tokio::select! {
            r = &mut send => return r.is_ok(),
            cmd = commands.recv() => if !apply_offline(subs, cmd) { return false },
        }
    }
}

enum SessionEnd {
    Shutdown,
    Lost(String),
    Lagged(Duration),
}

async fn run(
    config: WsConfig,
    mut subs: Subs,
    mut commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::Sender<WsEvent>,
) {
    let mut failures: u32 = 0;
    loop {
        let ws_config = WebSocketConfig::default()
            .max_message_size(Some(config.max_message_size))
            .max_frame_size(Some(config.max_message_size));
        let connect = tokio::time::timeout(
            config.connect_timeout,
            tokio_tungstenite::connect_async_with_config(
                config.url.as_str(),
                Some(ws_config),
                true,
            ),
        );
        tokio::pin!(connect);
        let connected = loop {
            tokio::select! {
                r = &mut connect => break r,
                cmd = commands.recv() => if !apply_offline(&mut subs, cmd) { return },
            }
        };
        let end = match connected {
            Ok(Ok((socket, _))) => {
                session(
                    &config,
                    socket,
                    &mut subs,
                    &mut commands,
                    &events,
                    &mut failures,
                )
                .await
            }
            Ok(Err(e)) => SessionEnd::Lost(format!("connect failed: {e}")),
            Err(_) => SessionEnd::Lost(format!(
                "connect timed out after {:?}",
                config.connect_timeout
            )),
        };
        let reason = match end {
            SessionEnd::Shutdown => return,
            SessionEnd::Lost(r) => r,
            SessionEnd::Lagged(waited) => {
                if !deliver(
                    &events,
                    WsEvent::Lagged { waited },
                    &mut subs,
                    &mut commands,
                )
                .await
                {
                    return;
                }
                format!("consumer lagged {waited:?}")
            }
        };
        let retry_in = config.reconnect.delay(failures);
        failures = failures.saturating_add(1);
        tracing::warn!(%reason, ?retry_in, "Pascal websocket disconnected");
        let event = WsEvent::Disconnected { reason, retry_in };
        let deadline = Instant::now() + retry_in;
        if !deliver(&events, event, &mut subs, &mut commands).await {
            return;
        }
        // Keep applying subscription changes while waiting to reconnect.
        let sleep = tokio::time::sleep_until(deadline);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                () = &mut sleep => break,
                cmd = commands.recv() => if !apply_offline(&mut subs, cmd) { return },
            }
        }
    }
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type Sink = futures_util::stream::SplitSink<Socket, Message>;

async fn send_frames(
    sink: &mut Sink,
    kind: &'static str,
    channels: &[Channel],
) -> Result<(), String> {
    for chunk in channels.chunks(CHANNELS_PER_FRAME) {
        sink.send(Message::text(client_frame(kind, chunk)))
            .await
            .map_err(|e| format!("{kind} failed: {e}"))?;
    }
    Ok(())
}

/// Decodes a text frame into an event, copying the text only when decoding fails.
fn frame_event(text: &str) -> WsEvent {
    match WsFrame::parse(text) {
        Ok(frame) => WsEvent::Frame(Box::new(frame)),
        Err(error) => {
            let mut cut = text.len().min(4096);
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            WsEvent::DecodeError {
                error,
                text: text[..cut].to_owned(),
            }
        }
    }
}

async fn session(
    config: &WsConfig,
    socket: Socket,
    subs: &mut Subs,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    events: &mpsc::Sender<WsEvent>,
    failures: &mut u32,
) -> SessionEnd {
    let connected_at = Instant::now();
    let (mut sink, mut stream) = socket.split();
    let all: Vec<Channel> = subs.values().cloned().collect();
    if let Err(e) = send_frames(&mut sink, "subscribe", &all).await {
        return SessionEnd::Lost(e);
    }
    // One event at a time waits for room; the socket is not read while it waits.
    let mut pending = Some(WsEvent::Connected {
        channels: all.len(),
    });
    let mut pending_since = Instant::now();
    let mut ping =
        tokio::time::interval_at(Instant::now() + config.ping_interval, config.ping_interval);
    let mut last_activity = Instant::now();
    loop {
        let waiting = pending.is_some();
        tokio::select! {
            biased;
            cmd = commands.recv() => {
                let (unsub, sub) = match cmd {
                    None | Some(Command::Shutdown) => {
                        let _ = sink.send(Message::Close(None)).await;
                        return SessionEnd::Shutdown;
                    }
                    Some(Command::Subscribe(c)) => {
                        let (mut unsub, mut sub) = (Vec::new(), Vec::new());
                        for ch in c {
                            match subs.insert(ch.key(), ch.clone()) {
                                None => sub.push(ch),
                                Some(old) if old == ch => {}
                                Some(old) => {
                                    unsub.push(old);
                                    sub.push(ch);
                                }
                            }
                        }
                        (unsub, sub)
                    }
                    Some(Command::Unsubscribe(c)) => {
                        let gone = c.iter().filter_map(|ch| subs.remove(&ch.key())).collect();
                        (gone, Vec::new())
                    }
                };
                if let Err(e) = send_frames(&mut sink, "unsubscribe", &unsub).await {
                    return SessionEnd::Lost(e);
                }
                if let Err(e) = send_frames(&mut sink, "subscribe", &sub).await {
                    return SessionEnd::Lost(e);
                }
            }
            _ = ping.tick() => {
                if !waiting && last_activity.elapsed() > config.idle_timeout {
                    return SessionEnd::Lost(format!("no frame for {:?}", last_activity.elapsed()));
                }
                if let Err(e) = sink.send(Message::text(PING_FRAME)).await {
                    return SessionEnd::Lost(format!("ping failed: {e}"));
                }
            }
            permit = events.reserve(), if waiting => {
                let Ok(permit) = permit else {
                    let _ = sink.send(Message::Close(None)).await;
                    return SessionEnd::Shutdown;
                };
                if let Some(ev) = pending.take() {
                    permit.send(ev);
                }
                // Reading resumes now; do not count the wait as socket silence.
                last_activity = Instant::now();
            }
            () = tokio::time::sleep_until(pending_since + config.lag_timeout), if waiting => {
                let _ = sink.send(Message::Close(None)).await;
                return SessionEnd::Lagged(pending_since.elapsed());
            }
            msg = stream.next(), if !waiting => {
                let event = match msg {
                    None => return SessionEnd::Lost("stream ended".into()),
                    Some(Err(e)) => return SessionEnd::Lost(e.to_string()),
                    Some(Ok(Message::Close(frame))) => {
                        return SessionEnd::Lost(match frame {
                            Some(f) => format!("closed by server: {} {}", f.code, f.reason),
                            None => "closed by server".into(),
                        });
                    }
                    Some(Ok(Message::Text(t))) => frame_event(t.as_str()),
                    Some(Ok(Message::Binary(b))) => frame_event(&String::from_utf8_lossy(&b)),
                    // Protocol pings are answered by tungstenite itself.
                    Some(Ok(_)) => {
                        last_activity = Instant::now();
                        continue;
                    }
                };
                last_activity = Instant::now();
                if connected_at.elapsed() >= config.healthy_after {
                    *failures = 0;
                }
                if let WsEvent::Frame(f) = &event
                    && let WsMessage::Error { channel: Some(ch), .. } = &f.message
                {
                    // A rejected or no-longer-served channel is not resubscribed.
                    subs.remove(&ch.key());
                }
                pending = Some(event);
                pending_since = Instant::now();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_serialise_like_the_docs() {
        let chans = vec![
            Channel::AllTrades,
            Channel::Account {
                owner: "O".into(),
                exclude_orders: false,
            },
            Channel::account("P"),
            Channel::MarketUpdate,
            Channel::Book {
                symbol: "A.B".into(),
            },
        ];
        assert_eq!(
            subscribe_frame(&chans),
            r#"{"type":"subscribe","channels":[{"channel":"all_trades"},{"channel":"account","owner":"O"},{"channel":"account","owner":"P","exclude_orders":true},{"channel":"market_update"},{"channel":"book","symbol":"A.B"}]}"#
        );
        assert_eq!(
            unsubscribe_frame(&[Channel::Trades {
                symbol: "A.B".into()
            }]),
            r#"{"type":"unsubscribe","channels":[{"channel":"trades","symbol":"A.B"}]}"#
        );
    }

    #[test]
    fn key_ignores_options() {
        let a = Channel::account("O");
        let b = Channel::Account {
            owner: "O".into(),
            exclude_orders: false,
        };
        assert_ne!(a, b);
        assert_eq!(a.key(), b.key());
        assert_eq!(a.key(), ChannelKey::Account("O".into()));
    }

    #[test]
    fn pong_and_errors_parse() {
        assert!(matches!(
            WsFrame::parse(r#"{"type":"pong"}"#).unwrap().message,
            WsMessage::Pong
        ));
        let f = WsFrame::parse(
            r#"{"channel":"book","symbol":"SIM_EVENT_1.MARKET_1","type":"error","data":{"code":"SYMBOL_NOT_FOUND","message":"symbol not found"},"seq":"67891","state_time_ms":"1731536000050","server_time_ms":"1731536000123"}"#,
        )
        .unwrap();
        match f.message {
            WsMessage::Error {
                channel,
                channel_name,
                error,
            } => {
                assert_eq!(
                    channel,
                    Some(Channel::Book {
                        symbol: "SIM_EVENT_1.MARKET_1".into()
                    })
                );
                assert_eq!(channel_name.as_deref(), Some("book"));
                assert_eq!(error.kind(), Some(crate::ApiErrorCode::SymbolNotFound));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(f.seq, Some(67891));
        // An error about a channel this crate does not model keeps its name.
        let f = WsFrame::parse(
            r#"{"channel":"candles","symbol":"A.B","interval":"1m","type":"error","data":{"code":"INVALID_REQUEST","message":"x"}}"#,
        )
        .unwrap();
        match f.message {
            WsMessage::Error {
                channel,
                channel_name,
                ..
            } => {
                assert_eq!(channel, None);
                assert_eq!(channel_name.as_deref(), Some("candles"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unknown_channel_is_an_error() {
        let e = WsFrame::parse(r#"{"channel":"candles","type":"update","data":[]}"#);
        assert!(matches!(e, Err(Error::Decode { .. })));
        let e = WsFrame::parse(r#"{"channel":"all_trades","type":"hello","data":[]}"#);
        assert!(matches!(e, Err(Error::Decode { .. })));
    }

    #[test]
    fn decode_error_keeps_truncated_text() {
        let text = format!(
            r#"{{"channel":"nope","type":"update","data":"{}"}}"#,
            "é".repeat(5000)
        );
        match frame_event(&text) {
            WsEvent::DecodeError { text: t, .. } => {
                assert!(t.len() <= 4096 && text.starts_with(&t));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn docs_book_frames_parse() {
        let snap = r#"{"channel":"book","symbol":"SIM_EVENT_1.MARKET_1","type":"snapshot","data":{"asks":[["0.560000","12"],["0.570000","20"]],"bids":[["0.540000","15"],["0.530000","7"]],"spec":{"symbol":"SIM_EVENT_1.MARKET_1","taker_fee_rate":"0.001000","maker_rebate_share":"0.500000","tick_size_min":"0.010000","tick_sig_figs":2}},"seq":"67891","state_time_ms":"1731536000050","server_time_ms":"1731536000123"}"#;
        match WsFrame::parse(snap).unwrap().message {
            WsMessage::Book { symbol, kind, book } => {
                assert_eq!(symbol, "SIM_EVENT_1.MARKET_1");
                assert_eq!(kind, FrameKind::Snapshot);
                assert_eq!(book.asks[0].price, 560_000);
                assert_eq!(book.bids[1].size, 7);
                assert_eq!(book.spec.unwrap().tick_size_min, 10_000);
            }
            other => panic!("{other:?}"),
        }
        let upd = r#"{"channel":"book","symbol":"SIM_EVENT_1.MARKET_1","type":"update","data":{"asks":[["0.570000","0"]],"bids":[["0.540000","15"]]},"seq":"67891","state_time_ms":"1731536000050","server_time_ms":"1731536000123"}"#;
        match WsFrame::parse(upd).unwrap().message {
            WsMessage::Book { kind, book, .. } => {
                assert_eq!(kind, FrameKind::Update);
                assert_eq!(book.asks[0].size, 0);
            }
            other => panic!("{other:?}"),
        }
    }
}
