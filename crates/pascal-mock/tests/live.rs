//! Live mode (plan 4.L, L4-1) against a local fake upstream: an axum server on 127.0.0.1 that
//! speaks Pascal's read API (`GET /api/v1/markets`, `POST /api/v1/markets/search`,
//! `GET /api/v1/trades` with opaque cursors) and its
//! market-data socket (`book` snapshots and level updates, `market_update`), with frames shaped
//! like `fixtures/pascal/ws_*.json` and the `book` examples of `pascal-rs`'s `ws.rs` tests. The
//! fake logs every request, so each test also checks that nothing but those reads reached it.
//!
//! One `#[ignore]`d smoke test reads the real `data.pascal.trade` for 30 s:
//! `cargo test -p pascal-mock --test live live_smoke_real_upstream -- --ignored --nocapture`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use pascal_mock::live::LiveConfig;
use pascal_mock::{Mock, MockConfig, RunningMock, ScriptSource, Tape};
use pascal_rs::ratelimit::{Backoff, BucketConfig, RateLimits};
use serde_json::{Value, json};
use tokio::sync::broadcast;

const PHI: &str = "NBA_26OCT28_PHIIND_ML.PHI";
const HOU: &str = "NBA_26OCT28_HOUMIL_ML.HOU";
const OKC: &str = "NBA_26OCT28_OKCDAL_ML.OKC";
const USD: i64 = 1_000_000;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

// ---------- the fake upstream ----------

#[derive(Default)]
struct Fake {
    /// "METHOD /path" of every HTTP request (the socket upgrade included).
    log: Mutex<Vec<String>>,
    /// The catalogue `GET /markets` serves.
    markets: Mutex<Vec<Value>>,
    /// What `POST /markets/search` returns per symbol (when not in the catalogue).
    search: Mutex<HashMap<String, Value>>,
    /// Current upstream book per symbol: `{"asks":[[p,s]…],"bids":[…]}`.
    books: Mutex<HashMap<String, Value>>,
    /// While set, book subscriptions are not answered until [`Fake::release`].
    hold: AtomicBool,
    /// Every `book` subscription seen, in order.
    subscribed: Mutex<Vec<String>>,
    connections: AtomicUsize,
    seq: AtomicUsize,
    /// The tape `GET /api/v1/trades` pages, newest first.
    trades: Mutex<Vec<Value>>,
    /// Query string of every trades request.
    trades_queries: Mutex<Vec<String>>,
    /// Each trades answer waits this long (ms).
    trades_delay_ms: AtomicU64,
    /// What `GET /markets/resolved` serves.
    resolved: Mutex<Vec<Value>>,
    frames: Mutex<Option<broadcast::Sender<Ctl>>>,
}

#[derive(Clone, Debug)]
enum Ctl {
    Frame(String),
    Kill,
    Release,
}

impl Fake {
    fn ctl(&self) -> broadcast::Sender<Ctl> {
        self.frames
            .lock()
            .unwrap()
            .get_or_insert_with(|| broadcast::channel(256).0)
            .clone()
    }

    fn seq(&self) -> String {
        (12_300_000_000 + self.seq.fetch_add(1, Ordering::Relaxed) as u64).to_string()
    }

    fn envelope(&self, data: Value) -> Value {
        let t = now_ms().to_string();
        json!({"status":"success","data":data,"seq":self.seq(),"state_time_ms":t,"server_time_ms":t})
    }

    fn book_frame(&self, symbol: &str, kind: &str, data: Value) -> String {
        let t = now_ms().to_string();
        json!({"channel":"book","symbol":symbol,"type":kind,"data":data,
               "seq":self.seq(),"state_time_ms":t,"server_time_ms":t})
        .to_string()
    }

    fn set_book(&self, symbol: &str, asks: &[(&str, &str)], bids: &[(&str, &str)]) {
        self.books
            .lock()
            .unwrap()
            .insert(symbol.into(), json!({"asks": asks, "bids": bids}));
    }

    /// Pushes a level update to every socket (and applies it to the fake's own book).
    fn update(&self, symbol: &str, asks: &[(&str, &str)], bids: &[(&str, &str)]) {
        {
            let mut books = self.books.lock().unwrap();
            let b = books
                .entry(symbol.into())
                .or_insert(json!({"asks":[],"bids":[]}));
            for (side, levels) in [("asks", asks), ("bids", bids)] {
                let mut cur: Vec<(String, String)> =
                    serde_json::from_value(b[side].clone()).unwrap_or_default();
                for (p, s) in levels {
                    cur.retain(|(cp, _)| cp != p);
                    if *s != "0" {
                        cur.push((p.to_string(), s.to_string()));
                    }
                }
                b[side] = json!(cur);
            }
        }
        let f = self.book_frame(symbol, "update", json!({"asks": asks, "bids": bids}));
        let _ = self.ctl().send(Ctl::Frame(f));
    }

    fn push_resolution(&self, symbol: &str, price: &str) {
        let mut m = self.market(symbol);
        let o = m.as_object_mut().unwrap();
        for k in ["stats", "best_bid", "best_ask"] {
            o.remove(k);
        }
        o.insert(
            "resolution".into(),
            json!({"price": price, "seq": self.seq(), "market_resolution_ts_ms": now_ms().to_string()}),
        );
        let t = now_ms().to_string();
        let f = json!({"channel":"market_update","type":"update",
            "data":[{"update_type":"resolved","market":m}],
            "seq":self.seq(),"state_time_ms":t,"server_time_ms":t});
        let _ = self.ctl().send(Ctl::Frame(f.to_string()));
    }

    fn market(&self, symbol: &str) -> Value {
        self.markets
            .lock()
            .unwrap()
            .iter()
            .find(|m| m["symbol"] == symbol)
            .cloned()
            .or_else(|| self.search.lock().unwrap().get(symbol).cloned())
            .unwrap()
    }

    fn kill_sockets(&self) {
        let _ = self.ctl().send(Ctl::Kill);
    }

    fn release(&self) {
        self.hold.store(false, Ordering::SeqCst);
        let _ = self.ctl().send(Ctl::Release);
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    fn subscriptions(&self, symbol: &str) -> usize {
        self.subscribed
            .lock()
            .unwrap()
            .iter()
            .filter(|s| *s == symbol)
            .count()
    }

    /// Only Pascal's public reads reached the fake, never a write path.
    fn assert_reads_only(&self) {
        for l in self.log() {
            assert!(
                matches!(
                    l.as_str(),
                    "GET /api/v1/markets"
                        | "POST /api/v1/markets/search"
                        | "GET /api/v1/trades"
                        | "GET /api/v1/markets/resolved"
                        | "GET /ws"
                ),
                "unexpected upstream request {l}"
            );
        }
    }
}

type F = State<Arc<Fake>>;

async fn log_requests(State(fake): F, req: Request, next: axum::middleware::Next) -> Response {
    fake.log
        .lock()
        .unwrap()
        .push(format!("{} {}", req.method(), req.uri().path()));
    next.run(req).await
}

async fn f_markets(State(fake): F) -> Response {
    let data = Value::Array(fake.markets.lock().unwrap().clone());
    axum::Json(fake.envelope(data)).into_response()
}

async fn f_search(State(fake): F, body: Bytes) -> Response {
    let b: Value = serde_json::from_slice(&body).unwrap_or_default();
    let mut out = Vec::new();
    for s in b["symbols"].as_array().cloned().unwrap_or_default() {
        let s = s.as_str().unwrap_or_default().to_owned();
        if let Some(m) = fake.search.lock().unwrap().get(&s) {
            out.push(m.clone());
        } else if let Some(m) = fake
            .markets
            .lock()
            .unwrap()
            .iter()
            .find(|m| m["symbol"] == s)
        {
            out.push(m.clone());
        }
    }
    axum::Json(fake.envelope(Value::Array(out))).into_response()
}

async fn f_resolved(State(fake): F) -> Response {
    let data = Value::Array(fake.resolved.lock().unwrap().clone());
    axum::Json(fake.envelope(data)).into_response()
}

/// Pascal-like paging with an opaque cursor (`c<index>`), filtered by `symbols`.
async fn f_trades(State(fake): F, req: Request) -> Response {
    let query = req.uri().query().unwrap_or_default().to_owned();
    fake.trades_queries.lock().unwrap().push(query.clone());
    let delay = fake.trades_delay_ms.load(Ordering::SeqCst);
    if delay > 0 {
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }
    let q: HashMap<String, String> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    let symbols: Vec<&str> = q.get("symbols").map_or(vec![], |s| s.split(',').collect());
    let limit: usize = q.get("limit").map_or(100, |l| l.parse().unwrap());
    let start: usize = q
        .get("before_cursor")
        .map_or(0, |c| c.trim_start_matches('c').parse().unwrap());
    let all: Vec<Value> = fake
        .trades
        .lock()
        .unwrap()
        .iter()
        .filter(|t| symbols.contains(&t["symbol"].as_str().unwrap_or_default()))
        .cloned()
        .collect();
    let items: Vec<Value> = all.iter().skip(start).take(limit).cloned().collect();
    let mut data = json!({ "items": items });
    if start + limit < all.len() {
        data["next_cursor"] = format!("c{}", start + limit).into();
    }
    axum::Json(fake.envelope(data)).into_response()
}

async fn f_ws(State(fake): F, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| f_session(fake, socket))
}

async fn f_session(fake: Arc<Fake>, mut socket: WebSocket) {
    fake.connections.fetch_add(1, Ordering::SeqCst);
    let mut ctl = fake.ctl().subscribe();
    let mut held: Vec<String> = Vec::new();
    let mut books: Vec<String> = Vec::new();
    loop {
        let out: Vec<String> = tokio::select! {
            msg = socket.recv() => match msg {
                Some(Ok(Message::Text(t))) => {
                    let v: Value = serde_json::from_str(t.as_str()).unwrap_or_default();
                    let mut out = Vec::new();
                    match v["type"].as_str() {
                        Some("ping") => out.push(r#"{"type":"pong"}"#.to_owned()),
                        Some("subscribe") => {
                            for ch in v["channels"].as_array().cloned().unwrap_or_default() {
                                match ch["channel"].as_str() {
                                    Some("book") => {
                                        let s = ch["symbol"].as_str().unwrap().to_owned();
                                        fake.subscribed.lock().unwrap().push(s.clone());
                                        books.push(s.clone());
                                        if fake.hold.load(Ordering::SeqCst) {
                                            held.push(s);
                                        } else if let Some(b) = fake.books.lock().unwrap().get(&s) {
                                            out.push(fake.book_frame(&s, "snapshot", b.clone()));
                                        }
                                    }
                                    Some("market_update") => {
                                        let t = now_ms().to_string();
                                        out.push(json!({"channel":"market_update","type":"snapshot",
                                            "data":[],"seq":fake.seq(),"state_time_ms":t,
                                            "server_time_ms":t}).to_string());
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Some("unsubscribe") => {
                            for ch in v["channels"].as_array().cloned().unwrap_or_default() {
                                if let Some(s) = ch["symbol"].as_str() {
                                    books.retain(|b| b != s);
                                }
                            }
                        }
                        _ => {}
                    }
                    out
                }
                Some(Ok(_)) => continue,
                _ => break,
            },
            c = ctl.recv() => match c {
                Ok(Ctl::Frame(f)) => {
                    let v: Value = serde_json::from_str(&f).unwrap();
                    let mine = v["channel"] != "book"
                        || books.iter().any(|b| v["symbol"] == b.as_str());
                    // Forwarded even while snapshots are held: the mock must ignore updates
                    // to a book that has no fresh snapshot.
                    if mine { vec![f] } else { continue }
                }
                Ok(Ctl::Kill) => break,
                Ok(Ctl::Release) => {
                    let out = held
                        .iter()
                        .filter_map(|s| {
                            fake.books.lock().unwrap().get(s).map(|b| fake.book_frame(s, "snapshot", b.clone()))
                        })
                        .collect();
                    held.clear();
                    out
                }
                Err(_) => continue,
            },
        };
        for f in out {
            if socket.send(Message::Text(f.into())).await.is_err() {
                return;
            }
        }
    }
}

async fn f_other(State(_fake): F) -> Response {
    (
        axum::http::StatusCode::NOT_FOUND,
        "fake upstream: no such route",
    )
        .into_response()
}

/// The sample tape's catalogue, re-dated to start in 3 h.
fn catalogue() -> Vec<Value> {
    let text =
        std::fs::read_to_string(root().join("tapes/nba-2026-10-28/markets_selected.json")).unwrap();
    let body: Value = serde_json::from_str(&text).unwrap();
    let start = now_ms() + 3 * 3_600_000;
    body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            let mut m = m.clone();
            let d = &mut m["display_attributes"];
            d["expected_event_start_time_ms"] = start.to_string().into();
            d["expected_resolution_time_ms"] = (start + 4 * 3_600_000).to_string().into();
            m
        })
        .collect()
}

struct Upstream {
    fake: Arc<Fake>,
    addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn upstream() -> Upstream {
    let fake = Arc::new(Fake::default());
    *fake.markets.lock().unwrap() = catalogue();
    fake.set_book(
        PHI,
        &[("0.560000", "12"), ("0.570000", "20")],
        &[("0.540000", "15"), ("0.530000", "7")],
    );
    fake.set_book(HOU, &[("0.770000", "30")], &[("0.760000", "30")]);
    let app = Router::new()
        .route("/api/v1/markets", get(f_markets))
        .route("/api/v1/markets/search", post(f_search))
        .route("/api/v1/trades", get(f_trades))
        .route("/api/v1/markets/resolved", get(f_resolved))
        .route("/ws", get(f_ws))
        .fallback(f_other)
        .layer(axum::middleware::from_fn_with_state(
            fake.clone(),
            log_requests,
        ))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    Upstream { fake, addr, task }
}

fn live_cfg(up: &Upstream) -> LiveConfig {
    let mut c = LiveConfig::new(format!("http://{}", up.addr));
    c.markets_every = Duration::from_millis(300);
    c.search_every = Duration::from_millis(300);
    c.ready_wait = Duration::from_secs(3);
    c.record_books_every = Duration::from_millis(100);
    c.retry_refused_after = Duration::from_millis(100);
    c.rate_limits = RateLimits {
        per_host: BucketConfig {
            per_second: 1_000.0,
            burst: 1_000.0,
        },
        per_route: HashMap::new(),
    };
    c.ws_reconnect = Backoff {
        base: Duration::from_millis(50),
        max: Duration::from_millis(200),
    };
    c
}

async fn start_live(live: LiveConfig) -> RunningMock {
    let mut cfg = MockConfig::new("unused");
    cfg.script = ScriptSource::None;
    let mock = Mock::new_live(cfg, live).unwrap();
    assert!(
        mock.live()
            .unwrap()
            .wait_catalogue(Duration::from_secs(10))
            .await
    );
    mock.spawn("127.0.0.1:0").await.unwrap()
}

// ---------- helpers on the mock ----------

fn key(n: u8) -> String {
    bs58::encode([n; 32]).into_string()
}

fn order(cid: u64, symbol: &str, side: &str, price: &str, size: u64) -> Value {
    json!({
        "client_order_id": cid.to_string(), "symbol": symbol, "side": side, "price": price,
        "size": size.to_string(), "tif": "IOC",
        "auth": {"client_ts_ms": now_ms().to_string(), "recv_window_ms": "5000", "owner": key(1),
                 "signer": key(2), "signature": bs58::encode([7u8; 64]).into_string()},
    })
}

async fn place(m: &RunningMock, o: Value) -> Value {
    let body: Value = reqwest::Client::new()
        .post(format!("{}/api/v1/orders", m.base_url()))
        .json(&json!([o]))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    body[0].clone()
}

/// `[(price, size)]` of one side of the served book.
async fn levels(m: &RunningMock, symbol: &str, side: &str) -> Vec<(String, String)> {
    let b: Value = reqwest::get(format!("{}/api/v1/books?symbols={symbol}", m.base_url()))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    serde_json::from_value(b["data"]["books"][symbol][side].clone()).unwrap_or_default()
}

async fn info(m: &RunningMock) -> Value {
    reqwest::get(format!("{}/mock/info", m.base_url()))
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn search(m: &RunningMock, symbols: &[&str]) -> Vec<Value> {
    let b: Value = reqwest::Client::new()
        .post(format!("{}/api/v1/markets/search", m.base_url()))
        .json(&json!({ "symbols": symbols }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    b["data"].as_array().cloned().unwrap_or_default()
}

/// Polls `f` every 20 ms for up to 5 s.
async fn eventually<Fut: std::future::Future<Output = bool>>(
    what: &str,
    mut f: impl FnMut() -> Fut,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

/// A JSON string or number as text.
fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn lv(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(p, s)| (p.to_string(), s.to_string()))
        .collect()
}

// ---------- tests ----------

#[tokio::test]
async fn snapshot_then_updates_drive_books_and_an_ioc_fills_at_the_live_price() {
    let up = upstream().await;
    let m = start_live(live_cfg(&up)).await;

    // First /books subscribes lazily and waits for the snapshot.
    assert_eq!(up.fake.subscriptions(PHI), 0);
    assert_eq!(
        levels(&m, PHI, "asks").await,
        lv(&[("0.560000", "12"), ("0.570000", "20")])
    );
    assert_eq!(
        levels(&m, PHI, "bids").await[0],
        lv(&[("0.540000", "15")])[0]
    );
    assert_eq!(up.fake.subscriptions(PHI), 1);

    // Level updates: 0.560 removed, 0.550 added.
    up.fake
        .update(PHI, &[("0.560000", "0"), ("0.550000", "4")], &[]);
    eventually("the update on /books", || async {
        levels(&m, PHI, "asks").await == lv(&[("0.550000", "4"), ("0.570000", "20")])
    })
    .await;
    up.fake.update(PHI, &[("0.560000", "12")], &[]);
    eventually("the second update", || async {
        levels(&m, PHI, "asks").await.len() == 3
    })
    .await;

    // An IOC bid at 0.560 for 6 takes 4 at 0.550 and 2 at 0.560: the live prices.
    let r = place(&m, order(1, PHI, "BID", "0.560000", 6)).await;
    assert_eq!(r["status"], "success", "{r}");
    let fills = r["data"]["fills"].as_array().unwrap();
    let got: Vec<(String, String)> = fills
        .iter()
        .map(|f| (text(&f["fill_price"]), text(&f["fill_size"])))
        .collect();
    assert_eq!(got, lv(&[("0.550000", "4"), ("0.560000", "2")]));

    // Taken liquidity stays consumed until the upstream updates that level.
    assert_eq!(
        levels(&m, PHI, "asks").await,
        lv(&[("0.560000", "10"), ("0.570000", "20")])
    );
    up.fake.update(PHI, &[("0.560000", "12")], &[]);
    eventually("the level update to free the consumed size", || async {
        levels(&m, PHI, "asks").await == lv(&[("0.560000", "12"), ("0.570000", "20")])
    })
    .await;
    // 0.550 was not updated upstream: still consumed.
    assert!(
        !levels(&m, PHI, "asks")
            .await
            .iter()
            .any(|l| l.0 == "0.550000")
    );

    // The account is the mock's, not Pascal's.
    let a: Value = reqwest::get(format!("{}/api/v1/accounts/{}", m.base_url(), key(1)))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(text(&a["data"]["positions"][0]["size"]), "6");

    let i = info(&m).await;
    assert_eq!(i["mode"], "live");
    assert_eq!(i["upstream"], "127.0.0.1");
    assert_eq!(i["upstream_ok"], true);
    assert_eq!(i["books"], 1);
    assert_eq!(i["stale_books"], 0);
    up.fake.assert_reads_only();
}

#[tokio::test]
async fn a_socket_drop_makes_books_stale_and_fills_wait_for_the_new_snapshot() {
    let up = upstream().await;
    let m = start_live(live_cfg(&up)).await;
    assert_eq!(levels(&m, PHI, "asks").await[0].0, "0.560000");
    let r = place(&m, order(1, PHI, "BID", "0.560000", 1)).await;
    assert_eq!(r["status"], "success", "{r}");

    // The next session's snapshots are held back; the socket drops.
    up.fake.hold.store(true, Ordering::SeqCst);
    up.fake.kill_sockets();
    eventually("the reconnect and resubscription", || async {
        up.fake.connections.load(Ordering::SeqCst) >= 2 && up.fake.subscriptions(PHI) >= 2
    })
    .await;
    let i = info(&m).await;
    assert_eq!(i["stale_books"], 1, "{i}");

    // No fills on a stale book: Pascal's error envelope, NOT_READY.
    let r = place(&m, order(2, PHI, "BID", "0.570000", 1)).await;
    assert_eq!(r["status"], "error", "{r}");
    assert_eq!(r["data"]["code"], "NOT_READY");
    // /books keeps the last levels meanwhile (after waiting for the snapshot in vain).
    assert_eq!(levels(&m, PHI, "asks").await[0].0, "0.560000");

    // An update before the snapshot is not applied; the snapshot replaces the book.
    up.fake.update(PHI, &[("0.590000", "1")], &[]);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !levels(&m, PHI, "asks")
            .await
            .iter()
            .any(|l| l.0 == "0.590000")
    );
    assert_eq!(info(&m).await["stale_books"], 1);
    up.fake
        .set_book(PHI, &[("0.600000", "5")], &[("0.580000", "5")]);
    up.fake.release();
    eventually("the fresh snapshot", || async {
        info(&m).await["stale_books"] == 0
    })
    .await;
    assert_eq!(levels(&m, PHI, "asks").await, lv(&[("0.600000", "5")]));
    let r = place(&m, order(3, PHI, "BID", "0.600000", 2)).await;
    assert_eq!(r["status"], "success", "{r}");
    assert_eq!(r["data"]["fills"][0]["fill_price"], "0.600000");
    assert_eq!(info(&m).await["upstream_ok"], true);
    up.fake.assert_reads_only();
}

#[tokio::test]
async fn a_pushed_resolution_reaches_search_and_mock_resolve_wins() {
    let up = upstream().await;
    let m = start_live(live_cfg(&up)).await;
    // Follow both books, hold a PHI position.
    assert!(!levels(&m, PHI, "asks").await.is_empty());
    assert!(!levels(&m, HOU, "asks").await.is_empty());
    let r = place(&m, order(1, PHI, "BID", "0.560000", 3)).await;
    assert_eq!(r["status"], "success", "{r}");

    // The operator resolves HOU first.
    let r: Value = reqwest::Client::new()
        .post(format!("{}/mock/resolve", m.base_url()))
        .json(&json!({"symbol": HOU, "price": "0"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["data"]["resolution"]["price"], "0.000000", "{r}");

    // Pascal then pushes both results.
    up.fake.push_resolution(PHI, "1.000000");
    up.fake.push_resolution(HOU, "1.000000");
    eventually("PHI resolved in /markets/search", || async {
        search(&m, &[PHI])
            .await
            .first()
            .is_some_and(|x| x["resolution"]["price"] == "1.000000")
    })
    .await;
    let hou = search(&m, &[HOU]).await;
    assert_eq!(
        hou[0]["resolution"]["price"], "0.000000",
        "a market resolves once"
    );

    // Resolved markets leave /markets; the position settled at 1.
    let markets: Value = reqwest::get(format!("{}/api/v1/markets", m.base_url()))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let listed: Vec<&str> = markets["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["symbol"].as_str().unwrap())
        .collect();
    assert!(!listed.contains(&PHI) && !listed.contains(&HOU));
    assert!(listed.contains(&OKC));
    let a: Value = reqwest::get(format!("{}/api/v1/accounts/{}", m.base_url(), key(1)))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(a["data"]["positions"].as_array().unwrap().is_empty());
    let collateral =
        pascal_rs::num::parse_micros(a["data"]["collateral_usd"].as_str().unwrap()).unwrap();
    assert!(collateral > 10_000 * USD, "PHI paid out: {collateral}");
    // Orders on a resolved market are refused as on Pascal.
    let r = place(&m, order(2, PHI, "BID", "0.560000", 1)).await;
    assert_eq!(r["data"]["code"], "MARKET_RESOLVED");
    up.fake.assert_reads_only();
}

#[tokio::test]
async fn the_search_backstop_finds_a_result_that_was_never_pushed() {
    let up = upstream().await;
    up.fake
        .set_book(OKC, &[("0.400000", "10")], &[("0.380000", "10")]);
    let m = start_live(live_cfg(&up)).await;
    assert!(!levels(&m, OKC, "asks").await.is_empty());
    // OKC leaves /markets with its result available only through search.
    let mut resolved = up.fake.market(OKC);
    resolved["resolution"] = json!({"price":"0.000000","seq":"12300000999","market_resolution_ts_ms": now_ms().to_string()});
    up.fake.search.lock().unwrap().insert(OKC.into(), resolved);
    up.fake
        .markets
        .lock()
        .unwrap()
        .retain(|x| x["symbol"] != OKC);
    eventually("the backstop resolution", || async {
        search(&m, &[OKC])
            .await
            .first()
            .is_some_and(|x| x["resolution"]["price"] == "0.000000")
    })
    .await;
    assert!(
        up.fake
            .log()
            .iter()
            .any(|l| l == "POST /api/v1/markets/search")
    );
    up.fake.assert_reads_only();
}

#[tokio::test]
async fn upstream_refusing_a_book_is_not_retried_in_a_loop() {
    let up = upstream().await;
    let m = start_live(live_cfg(&up)).await;
    // A symbol the catalogue does not list is never subscribed.
    let b: Value = reqwest::get(format!("{}/api/v1/books?symbols=NOPE_X.Y", m.base_url()))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(b["data"]["books"], json!({}));
    assert_eq!(up.fake.subscriptions("NOPE_X.Y"), 0);
    // Many /books calls subscribe each book once.
    for _ in 0..20 {
        levels(&m, PHI, "asks").await;
    }
    assert_eq!(up.fake.subscriptions(PHI), 1);
    up.fake.assert_reads_only();
}

#[tokio::test]
async fn a_recorded_live_session_replays_in_replay_mode() {
    let up = upstream().await;
    let dir = std::env::temp_dir().join(format!("pascal-mock-record-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut cfg = live_cfg(&up);
    cfg.record = Some(dir.clone());
    let m = start_live(cfg).await;
    assert_eq!(levels(&m, PHI, "asks").await[0].0, "0.560000");
    assert!(!levels(&m, HOU, "asks").await.is_empty());
    // A fill of ours must not show in the recording (it holds the upstream's levels).
    let r = place(&m, order(1, PHI, "BID", "0.560000", 5)).await;
    assert_eq!(r["status"], "success", "{r}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    up.fake
        .update(PHI, &[("0.560000", "0"), ("0.555000", "9")], &[]);
    eventually("the update", || async {
        levels(&m, PHI, "asks").await[0].0 == "0.555000"
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    up.fake.push_resolution(HOU, "1.000000");
    eventually("HOU resolved", || async {
        search(&m, &[HOU])
            .await
            .first()
            .is_some_and(|x| x["resolution"].is_object())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    m.mock.stop_live("test").await;

    // The directory is a tape the replay mode loads.
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["recorder"]["stop_reason"], "test");
    assert_eq!(manifest["symbols"], json!([HOU, PHI]));
    assert!(manifest["games"][0]["start_ms"].as_u64().unwrap() > now_ms());
    assert_eq!(manifest["resolutions"][HOU], "1.000000");
    let tape = Tape::load(&dir).unwrap();
    assert!(tape.books.len() >= 2, "{} snapshots", tape.books.len());
    assert_eq!(tape.stats.bad_lines + tape.stats.error_lines, 0);
    let first = &tape.books[0].books[PHI];
    assert_eq!(first.asks[0].price, 560_000);
    assert_eq!(
        first.asks[0].size, 12,
        "upstream size, not the mock's view after its fill"
    );

    let mut rcfg = MockConfig::new(&dir);
    rcfg.speed = 0.0;
    rcfg.script = ScriptSource::None;
    let replay = Mock::new(rcfg).unwrap().spawn("127.0.0.1:0").await.unwrap();
    assert_eq!(info(&replay).await["mode"], "replay");
    assert_eq!(
        levels(&replay, PHI, "asks").await[0],
        lv(&[("0.560000", "12")])[0]
    );
    let r = place(&replay, order(1, PHI, "BID", "0.560000", 2)).await;
    assert_eq!(r["data"]["fills"][0]["fill_price"], "0.560000", "{r}");
    // At the end of the tape PHI shows the update and HOU its recorded result.
    let last = tape.last_ms().max(tape.markets[HOU].last().unwrap().t_ms);
    reqwest::Client::new()
        .post(format!("{}/mock/clock", replay.base_url()))
        .json(&json!({ "tape_ms": last + 1 }))
        .send()
        .await
        .unwrap();
    assert_eq!(levels(&replay, PHI, "asks").await[0].0, "0.555000");
    assert_eq!(
        search(&replay, &[HOU]).await[0]["resolution"]["price"],
        "1.000000"
    );
    std::fs::remove_dir_all(&dir).ok();
    up.fake.assert_reads_only();
}

/// The next `book` frame on the mock's `/ws` within `wait`: (`snapshot`|`update`, asks).
async fn next_book(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    wait: Duration,
) -> Option<(String, Vec<(String, String)>)> {
    use futures_util::StreamExt;
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let msg = tokio::time::timeout_at(deadline, ws.next())
            .await
            .ok()??
            .ok()?;
        let tokio_tungstenite::tungstenite::Message::Text(t) = msg else {
            continue;
        };
        let v: Value = serde_json::from_str(t.as_str()).unwrap();
        // Every frame decodes with pascal-rs, as the desk's subscriber reads it.
        pascal_rs::WsFrame::parse(t.as_str()).unwrap();
        if v["channel"] == "book" {
            let asks = serde_json::from_value(v["data"]["asks"].clone()).unwrap();
            return Some((v["type"].as_str().unwrap().to_owned(), asks));
        }
    }
}

#[tokio::test]
async fn the_mock_ws_book_channel_snapshots_only_fresh_books_and_resnapshots_after_a_reconnect() {
    use futures_util::SinkExt;
    let up = upstream().await;
    let m = start_live(live_cfg(&up)).await;
    up.fake.hold.store(true, Ordering::SeqCst);
    let (mut ws, _) = tokio_tungstenite::connect_async(m.ws_url()).await.unwrap();
    let sub = json!({"type":"subscribe","channels":[{"channel":"book","symbol":PHI}]});
    ws.send(tokio_tungstenite::tungstenite::Message::text(
        sub.to_string(),
    ))
    .await
    .unwrap();
    // Subscribing follows the book upstream; no snapshot while the upstream one is pending.
    eventually("the upstream subscription", || async {
        up.fake.subscriptions(PHI) == 1
    })
    .await;
    assert_eq!(next_book(&mut ws, Duration::from_millis(600)).await, None);
    up.fake.release();
    let (kind, asks) = next_book(&mut ws, Duration::from_secs(3)).await.unwrap();
    assert_eq!(kind, "snapshot");
    assert_eq!(asks, lv(&[("0.560000", "12"), ("0.570000", "20")]));
    // Upstream level updates become level diffs.
    up.fake.update(PHI, &[("0.560000", "7")], &[]);
    assert_eq!(
        next_book(&mut ws, Duration::from_secs(3)).await.unwrap(),
        ("update".to_owned(), lv(&[("0.560000", "7")]))
    );
    // The mock's upstream drops: nothing while stale, then a new snapshot once fresh again.
    up.fake.hold.store(true, Ordering::SeqCst);
    up.fake.kill_sockets();
    eventually("the upstream resubscription", || async {
        up.fake.subscriptions(PHI) >= 2
    })
    .await;
    up.fake.set_book(PHI, &[("0.610000", "3")], &[]);
    assert_eq!(next_book(&mut ws, Duration::from_millis(600)).await, None);
    up.fake.release();
    assert_eq!(
        next_book(&mut ws, Duration::from_secs(3)).await.unwrap(),
        ("snapshot".to_owned(), lv(&[("0.610000", "3")]))
    );
    up.fake.assert_reads_only();
}

#[tokio::test]
async fn live_mode_refuses_trade_hosts_and_a_clock_change() {
    let mut cfg = MockConfig::new("unused");
    cfg.script = ScriptSource::None;
    // Pascal's domain, from the read host (this file names no write-capable host, D-142).
    let domain = pascal_mock::live::READ_HOST.split_once('.').unwrap().1;
    for bad in [
        format!("https://{domain}"),
        format!("https://orders.{domain}"),
    ] {
        let e = Mock::new_live(cfg.clone(), LiveConfig::new(&bad))
            .err()
            .unwrap();
        assert!(e.to_string().contains("not Pascal's read host"), "{e}");
    }
    let up = upstream().await;
    let m = start_live(live_cfg(&up)).await;
    let r = reqwest::Client::new()
        .post(format!("{}/mock/clock", m.base_url()))
        .json(&json!({"advance_ms": 1000}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422);
    up.fake.assert_reads_only();
}

/// L5-4: `GET /api/v1/trades` in live mode is the upstream's page with the query passed on
/// (symbols, limit, its opaque cursor), read only, with at most two calls in flight; the
/// replay's own fills are not mixed in.
#[tokio::test]
async fn trades_pass_through_from_the_upstream_read_only_and_capped() {
    let up = upstream().await;
    let fx: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("fixtures/sentiment/nba-2026-10-28.json")).unwrap(),
    )
    .unwrap();
    *up.fake.trades.lock().unwrap() = fx["trades"]["data"]["items"].as_array().unwrap().clone();
    let m = start_live(live_cfg(&up)).await;
    let get = |q: String| {
        let url = format!("{}/api/v1/trades?{q}", m.base_url());
        async move {
            let r = reqwest::get(url).await.unwrap();
            let status = r.status().as_u16();
            (status, r.json::<Value>().await.unwrap())
        }
    };

    // PHI (7 trades) and OKC (3) in pages of 6: the upstream's cursor goes back unchanged.
    let (st, p1) = get(format!("symbols={PHI},{OKC}&limit=6")).await;
    assert_eq!(st, 200, "{p1}");
    let page: pascal_rs::Page<pascal_rs::PublicTrade> =
        serde_json::from_value(p1["data"].clone()).unwrap();
    assert_eq!(page.items.len(), 6);
    assert!(
        page.items
            .iter()
            .all(|t| t.symbol == PHI || t.symbol == OKC)
    );
    assert_eq!(page.next_cursor.as_deref(), Some("c6"));
    let (st, p2) = get(format!("symbols={PHI},{OKC}&limit=6&before_cursor=c6")).await;
    assert_eq!(st, 200);
    assert_eq!(p2["data"]["items"].as_array().unwrap().len(), 4);
    assert!(p2["data"].get("next_cursor").is_none());
    {
        let q = up.fake.trades_queries.lock().unwrap();
        assert_eq!(q.len(), 2);
        assert!(
            q[1].contains("before_cursor=c6") && q[1].contains("limit=6"),
            "{}",
            q[1]
        );
    }

    // Checked locally, never sent: no symbols, a bad limit, a cursor with odd characters.
    for bad in [
        "limit=5".to_owned(),
        format!("symbols={PHI}&limit=900"),
        format!("symbols={PHI}&before_cursor=a%20b"),
    ] {
        let (st, b) = get(bad.clone()).await;
        assert_eq!(st, 422, "{bad}: {b}");
    }
    assert_eq!(up.fake.trades_queries.lock().unwrap().len(), 2);

    // Four at once against a slow upstream: two go through, two are refused at once.
    up.fake.trades_delay_ms.store(600, Ordering::SeqCst);
    let calls = (0..4).map(|_| get(format!("symbols={PHI}&limit=2")));
    let res = futures_util::future::join_all(calls).await;
    let ok = res.iter().filter(|(s, _)| *s == 200).count();
    let limited: Vec<&Value> = res
        .iter()
        .filter(|(s, _)| *s == 429)
        .map(|(_, b)| b)
        .collect();
    assert_eq!((ok, limited.len()), (2, 2), "{res:?}");
    assert!(limited.iter().all(|b| b["data"]["code"] == "RATE_LIMITED"));
    up.fake.trades_delay_ms.store(0, Ordering::SeqCst);
    up.fake.assert_reads_only();
}

/// 31 Oct: `GET /api/v1/markets/resolved` in live mode adds the upstream's resolved markets the
/// mock does not track (a desk names legs it lost from them), never one it tracks, one upstream
/// read per query for a while, read only.
#[tokio::test]
async fn resolved_adds_the_upstreams_untracked_markets() {
    let up = upstream().await;
    let mut old = catalogue()[0].clone();
    old["symbol"] = json!("NBA_26OCT20_PHIIND_ML.PHI");
    old["resolution"] =
        json!({"price":"1.000000","seq":"1","market_resolution_ts_ms":now_ms().to_string()});
    let mut tracked = catalogue()[0].clone();
    tracked["resolution"] =
        json!({"price":"0.000000","seq":"2","market_resolution_ts_ms":now_ms().to_string()});
    *up.fake.resolved.lock().unwrap() = vec![old, tracked.clone()];
    let m = start_live(live_cfg(&up)).await;
    let get = || {
        let url = format!(
            "{}/api/v1/markets/resolved?lookback=3d&limit=200",
            m.base_url()
        );
        async move {
            reqwest::get(url)
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let body = get().await;
    let symbols: Vec<&str> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|x| x["symbol"].as_str())
        .collect();
    assert_eq!(symbols, ["NBA_26OCT20_PHIIND_ML.PHI"], "{body}");
    assert!(!symbols.contains(&tracked["symbol"].as_str().unwrap()));
    get().await;
    let reads = up
        .fake
        .log()
        .iter()
        .filter(|l| *l == "GET /api/v1/markets/resolved")
        .count();
    assert_eq!(reads, 1, "the second answer comes from the cache");
    up.fake.assert_reads_only();
}

/// Reads the real `data.pascal.trade` for 30 s (`--ignored`): catalogue, the books of the NBA
/// moneylines starting within 24 h over the live socket, and how many there are.
#[tokio::test]
#[ignore = "reads the real data.pascal.trade for 30 s"]
async fn live_smoke_real_upstream() {
    let mut cfg = MockConfig::new("unused");
    cfg.script = ScriptSource::None;
    let mock = Mock::new_live(cfg, LiveConfig::new(pascal_mock::live::DEFAULT_UPSTREAM)).unwrap();
    assert!(
        mock.live()
            .unwrap()
            .wait_catalogue(Duration::from_secs(60))
            .await,
        "no catalogue"
    );
    let m = mock.spawn("127.0.0.1:0").await.unwrap();
    let markets: Value = reqwest::get(format!("{}/api/v1/markets", m.base_url()))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let all = markets["data"].as_array().unwrap();
    let now = now_ms();
    let mut nba: Vec<(u64, String)> = all
        .iter()
        .filter(|x| {
            let d = &x["display_attributes"];
            d["market_type"]["sport"] == "basketball"
                && d["market_type"]["market"]["type"] == "moneyline"
                && d["tags"]
                    .as_array()
                    .is_some_and(|t| t.iter().any(|t| t == "NBA"))
        })
        .filter_map(|x| {
            let start: u64 = x["display_attributes"]["expected_event_start_time_ms"]
                .as_str()?
                .parse()
                .ok()?;
            (start > now && start <= now + 24 * 3_600_000)
                .then(|| (start, x["symbol"].as_str().unwrap().to_owned()))
        })
        .collect();
    nba.sort();
    let symbols: Vec<String> = nba.iter().take(10).map(|(_, s)| s.clone()).collect();
    let started = Instant::now();
    let mut with_asks = 0;
    if !symbols.is_empty() {
        let b: Value = reqwest::get(format!(
            "{}/api/v1/books?symbols={}",
            m.base_url(),
            symbols.join(",")
        ))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
        with_asks = symbols
            .iter()
            .filter(|s| {
                b["data"]["books"][s.as_str()]["asks"]
                    .as_array()
                    .is_some_and(|a| !a.is_empty())
            })
            .count();
    }
    while started.elapsed() < Duration::from_secs(30) {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let i = info(&m).await;
    let first = nba
        .first()
        .map(|(t, s)| format!("{s} in {:.1} h", (*t - now) as f64 / 3.6e6));
    println!(
        "LIVE SMOKE: markets {} · NBA moneylines starting within 24 h: {} (first {}) · books followed {} \
         stale {} · with asks {} · upstream_ok {} · socket {}",
        all.len(),
        nba.len(),
        first.unwrap_or_else(|| "-".into()),
        i["books"],
        i["stale_books"],
        with_asks,
        i["upstream_ok"],
        i["socket_connected"],
    );
    assert_eq!(i["upstream_ok"], true);
    assert_eq!(i["stale_books"], 0);
    m.mock.stop_live("smoke").await;
}
