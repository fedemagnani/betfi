//! `GET /ws`: Pascal's market-data WebSocket, driven by the mock clock (spec section 9).
//!
//! Client frames, as `pascal_rs::ws` sends them: `{"type":"subscribe","channels":[…]}`,
//! `{"type":"unsubscribe","channels":[…]}`, `{"type":"ping"}` (answered `{"type":"pong"}`).
//! Channels served:
//!
//! - `account` (`owner`, optional `exclude_orders`): a snapshot (the `GET /accounts/{owner}`
//!   state), then one `update` per request that changed the account, carrying `fills` (with
//!   `builder_fee_paid_usd`), changed `positions` (size 0 once closed), `collateral_usd`,
//!   `lifetime_stats`, `resolutions`, `rewards` after builder fees, and changed `orders` /
//!   `order_updates` (left out with `exclude_orders`, which is echoed as Pascal does);
//! - `market_update`: an empty snapshot, then one `resolved` entry per resolution (scripted,
//!   control endpoint, seen on the tape, or pushed by the live upstream);
//! - `book` (`symbol`): the book at the clock as a snapshot, then the changed levels (size 0 =
//!   removed) checked every [`TICK`]. In live mode the mock's view of the upstream book (its
//!   own consumption and resting orders included, D-036); subscribing follows it upstream.
//!
//! `all_trades` and `trades` are answered with an error frame naming the channel. Frames carry
//! `seq`, `state_time_ms`, `server_time_ms` (mock time, D-033). A connection with no client
//! message for the idle timeout (Pascal: 60 s) is closed. While a socket is open its session
//! ticks the engine every [`TICK`], so scripted resolutions fire on time with no REST traffic.
//! A slow client: a send that takes longer than [`SEND_TIMEOUT`] closes the socket; a client
//! that falls behind the event bus gets fresh snapshots of its channels.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use pascal_rs::ws::ChannelKey;
use pascal_rs::{ApiErrorCode, Channel, Level, OrderBook};
use serde_json::{Map, Value, json};
use tokio::sync::broadcast::error::RecvError;
use tokio::time::{Instant, MissedTickBehavior};

use crate::api::Shared;
use crate::events::MockEvent;
use crate::requests::is_pubkey;
use crate::util::strip_nulls;

/// Engine tick and book-diff cadence while a socket is open.
pub const TICK: Duration = Duration::from_millis(250);
/// A send slower than this drops the connection.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest client frame (Pascal's limit).
pub const MAX_CLIENT_FRAME: usize = 64 << 10;
/// Most channels one socket may hold (each book is diffed every tick).
pub const MAX_CHANNELS: usize = 1_000;

pub(crate) async fn upgrade(State(shared): State<Arc<Shared>>, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(MAX_CLIENT_FRAME)
        .max_frame_size(MAX_CLIENT_FRAME)
        .on_upgrade(move |socket| run(shared, socket))
}

struct Session {
    shared: Arc<Shared>,
    subs: BTreeMap<ChannelKey, Channel>,
    /// Envelope seq of the last `account` snapshot per owner: older updates are skipped.
    account_seq: HashMap<String, u64>,
    /// Last book sent per symbol (a symbol is absent until its snapshot went out).
    books: HashMap<String, OrderBook>,
    /// Live: the upstream snapshot generation each sent book snapshot described.
    book_gen: HashMap<String, Option<u64>>,
}

async fn run(shared: Arc<Shared>, mut socket: WebSocket) {
    // Subscribe before any snapshot so no update falls between the two.
    let mut events = shared.engine().subscribe();
    let idle = shared.ws_idle_timeout();
    let mut s = Session {
        shared,
        subs: BTreeMap::new(),
        account_seq: HashMap::new(),
        books: HashMap::new(),
        book_gen: HashMap::new(),
    };
    let mut last_rx = Instant::now();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        let out: Vec<String> = tokio::select! {
            msg = socket.recv() => match msg {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                Some(Ok(Message::Text(t))) => {
                    last_rx = Instant::now();
                    s.client_frame(t.as_str())
                }
                Some(Ok(Message::Binary(_))) => {
                    last_rx = Instant::now();
                    vec![error_frame(None, ApiErrorCode::InvalidJson, "frames must be JSON text")]
                }
                // Protocol pings are answered by the socket itself.
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {
                    last_rx = Instant::now();
                    continue;
                }
            },
            ev = events.recv() => match ev {
                Ok(ev) => s.event(&ev),
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!(missed = n, "ws client lagged; resending snapshots");
                    s.resnapshot()
                }
                Err(RecvError::Closed) => break,
            },
            _ = tick.tick() => {
                if last_rx.elapsed() >= idle {
                    let close = Message::Close(Some(CloseFrame {
                        code: 1000,
                        reason: "idle timeout".into(),
                    }));
                    let _ = tokio::time::timeout(SEND_TIMEOUT, socket.send(close)).await;
                    break;
                }
                s.tick()
            }
        };
        for text in out {
            match tokio::time::timeout(SEND_TIMEOUT, socket.send(Message::Text(text.into()))).await
            {
                Ok(Ok(())) => {}
                Ok(Err(_)) => return,
                Err(_) => {
                    tracing::warn!("ws send timed out; closing");
                    return;
                }
            }
        }
    }
}

impl Session {
    fn client_frame(&mut self, text: &str) -> Vec<String> {
        let Ok(v) = serde_json::from_str::<Value>(text) else {
            return vec![error_frame(
                None,
                ApiErrorCode::InvalidJson,
                "frame is not JSON",
            )];
        };
        match v.get("type").and_then(Value::as_str) {
            Some("ping") => vec![r#"{"type":"pong"}"#.to_owned()],
            Some(kind @ ("subscribe" | "unsubscribe")) => {
                let Some(items) = v.get("channels").and_then(Value::as_array) else {
                    return vec![error_frame(
                        None,
                        ApiErrorCode::InvalidRequest,
                        "channels must be an array",
                    )];
                };
                let mut out = Vec::new();
                for item in items {
                    let ch = match serde_json::from_value::<Channel>(item.clone()) {
                        Ok(ch) => ch,
                        Err(e) => {
                            out.push(error_frame(
                                Some(item),
                                ApiErrorCode::InvalidRequest,
                                &format!("invalid channel: {e}"),
                            ));
                            continue;
                        }
                    };
                    if kind == "subscribe" {
                        out.extend(self.subscribe(ch, item));
                    } else {
                        self.unsubscribe(&ch);
                    }
                }
                out
            }
            _ => vec![error_frame(
                None,
                ApiErrorCode::InvalidRequest,
                "type must be subscribe, unsubscribe or ping",
            )],
        }
    }

    fn subscribe(&mut self, ch: Channel, raw: &Value) -> Vec<String> {
        match &ch {
            Channel::Account { owner, .. } if !is_pubkey(owner) => {
                return vec![error_frame(
                    Some(raw),
                    ApiErrorCode::InvalidRequest,
                    &format!("{owner:?} is not a base58 public key"),
                )];
            }
            Channel::Book { symbol } => {
                // Live mode: following a book subscribes it upstream (the snapshot sent now
                // may be empty; updates follow once the upstream snapshot lands).
                if let Some(link) = self.shared.live() {
                    link.track(&self.shared, std::slice::from_ref(symbol));
                }
                let known = self.shared.engine().knows(symbol);
                if !known {
                    return vec![error_frame(
                        Some(raw),
                        ApiErrorCode::SymbolNotFound,
                        &format!("unknown symbol {symbol}"),
                    )];
                }
            }
            Channel::AllTrades | Channel::Trades { .. } => {
                return vec![error_frame(
                    Some(raw),
                    ApiErrorCode::InvalidRequest,
                    "pascal-mock serves account, market_update and book only",
                )];
            }
            _ => {}
        }
        if self.subs.len() >= MAX_CHANNELS && !self.subs.contains_key(&ch.key()) {
            return vec![error_frame(
                Some(raw),
                ApiErrorCode::InvalidRequest,
                &format!("at most {MAX_CHANNELS} channels per connection"),
            )];
        }
        self.subs.insert(ch.key(), ch.clone());
        // Live: a book whose upstream snapshot has not landed gets its snapshot from `tick`.
        self.snapshot(&ch).into_iter().collect()
    }

    fn unsubscribe(&mut self, ch: &Channel) {
        let key = ch.key();
        self.subs.remove(&key);
        match key {
            ChannelKey::Account(owner) => {
                self.account_seq.remove(&owner);
            }
            ChannelKey::Book(symbol) => {
                self.books.remove(&symbol);
                self.book_gen.remove(&symbol);
            }
            _ => {}
        }
    }

    /// The channel's snapshot frame, recording what it covers. `None` for a live book that is
    /// not fresh yet (no snapshot from a stale or unsnapshotted upstream book).
    fn snapshot(&mut self, ch: &Channel) -> Option<String> {
        let mut e = self.shared.engine();
        let env = e.envelope();
        let data = match ch {
            Channel::Account {
                owner,
                exclude_orders,
            } => {
                let state = e.account(owner, None, *exclude_orders);
                self.account_seq.insert(owner.clone(), env.seq);
                to_json(&state)
            }
            Channel::Book { symbol } => {
                if !e.book_servable(symbol) {
                    self.books.remove(symbol);
                    return None;
                }
                let book = e.book(symbol);
                let data = book_json(&book.asks, &book.bids);
                self.books.insert(symbol.clone(), book);
                self.book_gen
                    .insert(symbol.clone(), e.live_book_generation(symbol));
                data
            }
            _ => json!([]),
        };
        Some(frame(ch, "snapshot", data, env.seq, env.time_ms))
    }

    fn resnapshot(&mut self) -> Vec<String> {
        let subs: Vec<Channel> = self.subs.values().cloned().collect();
        subs.iter().filter_map(|ch| self.snapshot(ch)).collect()
    }

    fn event(&mut self, ev: &MockEvent) -> Vec<String> {
        match ev {
            MockEvent::AccountUpdate {
                owner,
                update,
                seq,
                time_ms,
            } => {
                let Some(ch) = self.subs.get(&ChannelKey::Account(owner.clone())) else {
                    return Vec::new();
                };
                if self.account_seq.get(owner).is_some_and(|s| seq <= s) {
                    return Vec::new();
                }
                let exclude = matches!(
                    ch,
                    Channel::Account {
                        exclude_orders: true,
                        ..
                    }
                );
                // Without orders, an order-only change (a resting order placed or cancelled:
                // no fill, no collateral change, D-035) has nothing to say.
                if exclude
                    && update.fills.is_none()
                    && update.resolutions.is_none()
                    && update.rewards.is_none()
                    && update.positions.is_empty()
                {
                    return Vec::new();
                }
                let mut data = to_json(update);
                if exclude && let Some(obj) = data.as_object_mut() {
                    obj.insert("orders".into(), json!([]));
                    obj.remove("order_updates");
                }
                vec![frame(ch, "update", data, *seq, *time_ms)]
            }
            MockEvent::MarketUpdate {
                update_type,
                market,
                seq,
                time_ms,
            } => match self.subs.get(&ChannelKey::MarketUpdate) {
                Some(ch) => {
                    let data = json!([{"update_type": update_type, "market": market}]);
                    vec![frame(ch, "update", data, *seq, *time_ms)]
                }
                None => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// Advances the engine (scripted resolutions fire, the flush publishes their account
    /// deltas) and sends the book levels that changed. Live: a book gets its snapshot once its
    /// upstream book is fresh, and a new one after each upstream resnapshot (the mock's own
    /// reconnect); nothing is sent for a book while it is stale.
    fn tick(&mut self) -> Vec<String> {
        let e = self.shared.engine();
        let symbols: Vec<String> = self
            .subs
            .values()
            .filter_map(|ch| match ch {
                Channel::Book { symbol } => Some(symbol.clone()),
                _ => None,
            })
            .collect();
        if symbols.is_empty() {
            return Vec::new();
        }
        let env = e.envelope();
        let live = e.is_live();
        let mut out = Vec::new();
        for symbol in symbols {
            let ch = Channel::Book {
                symbol: symbol.clone(),
            };
            if live {
                if !e.book_servable(&symbol) {
                    continue;
                }
                let generation = e.live_book_generation(&symbol);
                if !self.books.contains_key(&symbol)
                    || self.book_gen.get(&symbol) != Some(&generation)
                {
                    let book = e.book(&symbol);
                    out.push(frame(
                        &ch,
                        "snapshot",
                        book_json(&book.asks, &book.bids),
                        env.seq,
                        env.time_ms,
                    ));
                    self.books.insert(symbol.clone(), book);
                    self.book_gen.insert(symbol, generation);
                    continue;
                }
            }
            let now = e.book(&symbol);
            let Some(last) = self.books.get_mut(&symbol) else {
                continue;
            };
            let asks = changed(&last.asks, &now.asks, false);
            let bids = changed(&last.bids, &now.bids, true);
            if asks.is_empty() && bids.is_empty() {
                continue;
            }
            *last = now;
            out.push(frame(
                &ch,
                "update",
                book_json(&asks, &bids),
                env.seq,
                env.time_ms,
            ));
        }
        out
    }
}

/// Levels whose size differs between `old` and `new` (size 0 = removed), best first.
fn changed(old: &[Level], new: &[Level], bids: bool) -> Vec<Level> {
    let o: BTreeMap<i64, u64> = old.iter().map(|l| (l.price, l.size)).collect();
    let n: BTreeMap<i64, u64> = new.iter().map(|l| (l.price, l.size)).collect();
    let mut prices: Vec<i64> = o.keys().chain(n.keys()).copied().collect();
    prices.sort_unstable();
    prices.dedup();
    if bids {
        prices.reverse();
    }
    prices
        .into_iter()
        .filter(|p| o.get(p) != n.get(p))
        .map(|price| Level {
            price,
            size: n.get(&price).copied().unwrap_or(0),
        })
        .collect()
}

fn book_json(asks: &[Level], bids: &[Level]) -> Value {
    json!({"asks": to_json(&asks), "bids": to_json(&bids)})
}

fn to_json<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v)
        .map(strip_nulls)
        .unwrap_or(Value::Null)
}

/// A server frame for `ch`, as Pascal shapes them (`fixtures/pascal/ws_*.json`).
fn frame(ch: &Channel, kind: &str, data: Value, seq: u64, time_ms: u64) -> String {
    let mut m = channel_fields(ch);
    m.insert("type".into(), kind.into());
    m.insert("data".into(), data);
    m.insert("seq".into(), seq.to_string().into());
    m.insert("state_time_ms".into(), time_ms.to_string().into());
    m.insert("server_time_ms".into(), time_ms.to_string().into());
    Value::Object(m).to_string()
}

fn channel_fields(ch: &Channel) -> Map<String, Value> {
    let mut m = Map::new();
    match ch {
        Channel::Account {
            owner,
            exclude_orders,
        } => {
            m.insert("channel".into(), "account".into());
            m.insert("owner".into(), owner.clone().into());
            if *exclude_orders {
                m.insert("exclude_orders".into(), true.into());
            }
        }
        Channel::MarketUpdate => {
            m.insert("channel".into(), "market_update".into());
        }
        Channel::Book { symbol } => {
            m.insert("channel".into(), "book".into());
            m.insert("symbol".into(), symbol.clone().into());
        }
        Channel::Trades { symbol } => {
            m.insert("channel".into(), "trades".into());
            m.insert("symbol".into(), symbol.clone().into());
        }
        Channel::AllTrades => {
            m.insert("channel".into(), "all_trades".into());
        }
    }
    m
}

/// An error frame; `raw` is the channel descriptor it is about (its `channel`, `owner` and
/// `symbol` are echoed so the client can drop that subscription).
fn error_frame(raw: Option<&Value>, code: ApiErrorCode, message: &str) -> String {
    let mut m = Map::new();
    if let Some(obj) = raw.and_then(Value::as_object) {
        for key in ["channel", "owner", "symbol"] {
            if let Some(v) = obj.get(key).filter(|v| v.is_string()) {
                m.insert(key.into(), v.clone());
            }
        }
    }
    m.insert("type".into(), "error".into());
    m.insert(
        "data".into(),
        json!({"code": code.as_str(), "message": message}),
    );
    Value::Object(m).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pascal_rs::{WsFrame, WsMessage};

    #[test]
    fn book_diff_reports_changed_and_removed_levels() {
        let lv = |v: &[(i64, u64)]| {
            v.iter()
                .map(|&(price, size)| Level { price, size })
                .collect::<Vec<_>>()
        };
        let old = lv(&[(700_000, 10), (710_000, 5)]);
        let new = lv(&[(700_000, 4), (720_000, 1)]);
        let d = changed(&old, &new, false);
        assert_eq!(
            d.iter().map(|l| (l.price, l.size)).collect::<Vec<_>>(),
            vec![(700_000, 4), (710_000, 0), (720_000, 1)]
        );
        let d = changed(&new, &new, true);
        assert!(d.is_empty());
    }

    #[test]
    fn frames_parse_with_pascal_rs() {
        let ch = Channel::Book {
            symbol: "A.B".into(),
        };
        let f = frame(
            &ch,
            "update",
            book_json(&[Level { price: 1, size: 0 }], &[]),
            7,
            9,
        );
        let parsed = WsFrame::parse(&f).unwrap();
        assert_eq!(parsed.seq, Some(7));
        assert!(matches!(parsed.message, WsMessage::Book { .. }));
        let e = error_frame(
            Some(&json!({"channel": "trades", "symbol": "A.B"})),
            ApiErrorCode::InvalidRequest,
            "no",
        );
        match WsFrame::parse(&e).unwrap().message {
            WsMessage::Error { channel, error, .. } => {
                assert_eq!(error.code, "INVALID_REQUEST");
                assert_eq!(
                    channel,
                    Some(Channel::Trades {
                        symbol: "A.B".into()
                    })
                );
            }
            other => panic!("{other:?}"),
        }
    }
}
