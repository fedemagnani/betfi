//! Plan step 3.D′ (mode M, D-007 amendment): no desk code path can send an order to Pascal by
//! default, and the desk's real signed batch, produced on demand for `docs/live.md`.
//!
//! 1. The `live` profile without `PASCAL_TRADE_URL` fails every write before any I/O (D-047),
//!    both at the Pascal handle and through the hedge desk, which still signs and stores its
//!    orders (D-074).
//! 2. A source scan: no file under `crates/` (nor the app's code, `scripts/`, `justfile`,
//!    `.env.example`) names a `*.pascal.trade` host other than the data and docs
//!    hosts, except the reviewed lines in [`ALLOWED`]; Rust string literals near each other are
//!    joined before the check, so splitting the host across literals does not hide it; and the
//!    only constructions of a Pascal write client in library code are the desk's handle
//!    (`PASCAL_TRADE_URL`) and `pascal-cli place-order --i-am-authorized`. This complements
//!    `crates/pascal-rs/tests/no_prod_trade_url.rs` (D-025). What it cannot see: a host built
//!    at run time from non-literal pieces (bytes, chars, reversal, encoding), read from a file
//!    or the environment (`PASCAL_TRADE_URL` itself is the deliberate path), or literals more
//!    than 2 lines / 6 literals apart.
//! 3. With `PASCAL_TRADE_URL` on a localhost mock, the hedge desk sends its batch there and
//!    nowhere else: the read mock sees no write, the trade mock sees exactly the signed batch.
//!
//! `sign_demo` (ignored) is run by `scripts/sign_demo.sh`: the desk's inception hedge of a 3-leg
//! NBA moneyline schedina against an in-process, signature-verifying `pascal-mock` on
//! 127.0.0.1, with throwaway keys from the environment; it writes the exact `POST /api/v1/orders`
//! body the desk sent and the desk's `schedina::signed_order` log. No test here opens a
//! connection to anything but 127.0.0.1.

mod common;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::Method;
use axum::middleware::Next;
use axum::response::Response;
use desk::Config;
use desk::api::types::{Side, Venue};
use desk::config::{PASCAL_DATA_URL_LIVE, Profile};
use desk::hedge::trader::venue_of;
use desk::hedge::{BUILDER_FEE_RATE_U, HedgeConfig, Hedger, OrderSpec, Trader};
use desk::state::{AppState, DeskEvent, PendingLeg, PendingOpened};
use pascal_mock::{MockConfig, ScriptSource, TradingKeys};
use pascal_rs::signing::{self, PlaceOrderRequest};
use pascal_rs::{Error, Market, TickRule, TimeInForce, TradingSigner};
use pricing::{LegInput, Params, Quote};
use sqlx::Row;
use tokio::sync::broadcast;

use common::{TempDb, repo_root};

const PHI: &str = "NBA_26OCT28_PHIIND_ML.PHI";
const ORL: &str = "NBA_26OCT28_ORLTOR_ML.ORL";
const HOU: &str = "NBA_26OCT28_HOUMIL_ML.HOU";
/// The schedina: three NBA moneylines of 28 Oct 2026, overlapping, so all three are hedged at
/// inception in one batch.
const LEGS: [(&str, Side); 3] = [(PHI, Side::Long), (ORL, Side::Short), (HOU, Side::Long)];
const USD: i64 = 1_000_000;
const STAKE_U: u64 = 10_000_000;
const WRITE_PATHS: [&str; 3] = ["/api/v1/orders", "/api/v1/cancels", "/api/v1/mass-cancels"];

// --- keys -------------------------------------------------------------------------------------

/// The hedger identity a test runs with: a trading key file, the owner and the builder.
struct Keys {
    dir: PathBuf,
    trading_key_path: PathBuf,
    trading_pubkey: String,
    owner: String,
    builder: String,
    remove: bool,
}

impl Keys {
    /// Fixed test seeds (owner 7, trading key 8, builder 9: the public docs test keys), the
    /// trading key written to a fresh temp dir.
    fn test() -> Keys {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "schedina-live-guard-{}-{}-{}",
            std::process::id(),
            desk::now_ms(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("trading.json");
        std::fs::write(&path, serde_json::to_string(&[8u8; 32].to_vec()).unwrap()).unwrap();
        let b58 = |s: [u8; 32]| TradingSigner::from_seed(&s).pubkey_base58();
        Keys {
            trading_pubkey: b58([8; 32]),
            owner: b58([7; 32]),
            builder: b58([9; 32]),
            trading_key_path: path,
            dir,
            remove: true,
        }
    }

    fn signer(&self) -> TradingSigner {
        TradingSigner::from_file(&self.trading_key_path).unwrap()
    }
}

impl Drop for Keys {
    fn drop(&mut self) {
        if self.remove {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

// --- localhost servers ------------------------------------------------------------------------

/// One request a recorded server received.
#[derive(Debug, Clone)]
struct Hit {
    method: Method,
    path: String,
    body: Vec<u8>,
}

#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<Hit>>>);

impl Seen {
    fn hits(&self) -> Vec<Hit> {
        self.0.lock().unwrap().clone()
    }

    fn writes(&self) -> Vec<Hit> {
        self.hits()
            .into_iter()
            .filter(|h| h.method == Method::POST && WRITE_PATHS.contains(&h.path.as_str()))
            .collect()
    }
}

/// Records method, path and (for POSTs) the exact body, then passes the request on.
async fn record(State(seen): State<Seen>, req: Request, next: Next) -> Response {
    let (parts, body) = req.into_parts();
    let (body, bytes) = if parts.method == Method::POST {
        let bytes = to_bytes(body, 1 << 20).await.unwrap_or_default();
        (Body::from(bytes.clone()), bytes.to_vec())
    } else {
        (body, Vec::new())
    };
    seen.0.lock().unwrap().push(Hit {
        method: parts.method.clone(),
        path: parts.uri.path().to_owned(),
        body: bytes,
    });
    next.run(Request::from_parts(parts, body)).await
}

/// A `pascal-mock` on 127.0.0.1 behind a request recorder.
struct Served {
    mock: pascal_mock::Mock,
    addr: SocketAddr,
    seen: Seen,
    task: tokio::task::JoinHandle<()>,
}

impl Served {
    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn ws_url(&self) -> String {
        format!("ws://{}/ws", self.addr)
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The sample tape of 28 Oct, frozen, with ample collateral; with `trading_keys`, every permit
/// is verified and must be signed by a registered key of its owner.
async fn serve_mock(trading_keys: Option<(&str, &str)>) -> Served {
    let mut mc = MockConfig::new(repo_root().join("tapes/nba-2026-10-28/sample"));
    mc.speed = 0.0;
    mc.script = ScriptSource::None;
    mc.initial_collateral_u = 1_000_000 * USD;
    if let Some((owner, key)) = trading_keys {
        let mut keys = TradingKeys::new();
        keys.add(owner, key).unwrap();
        mc.trading_keys = keys;
        mc.verify_permits = true;
    }
    let mock = pascal_mock::Mock::new(mc).expect("tape loads");
    let seen = Seen::default();
    let app = mock
        .router()
        .layer(axum::middleware::from_fn_with_state(seen.clone(), record));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Served {
        mock,
        addr,
        seen,
        task,
    }
}

/// A 127.0.0.1 listener that only counts the connections it accepts.
async fn counting_listener() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let n = Arc::new(AtomicUsize::new(0));
    let count = n.clone();
    let task = tokio::spawn(async move {
        while let Ok((_sock, _)) = listener.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
        }
    });
    (format!("http://{addr}"), n, task)
}

// --- the desk -----------------------------------------------------------------------------------

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_test_writer()
        .try_init();
}

/// The desk's configuration as `Config::from_lookup` reads it from these variables.
fn config(vars: &[(&str, String)], keys: &Keys, db: &TempDb) -> Config {
    let mut env: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    env.insert("DESK_DB".into(), db.path());
    env.insert("DESK_CHAIN".into(), "0".into());
    env.insert("SOLANA_RPC_URL".into(), "http://127.0.0.1:9".into());
    env.insert("PASCAL_HEDGER_OWNER".into(), keys.owner.clone());
    env.insert(
        "PASCAL_HEDGER_TRADING_KEY_PATH".into(),
        keys.trading_key_path.to_string_lossy().into_owned(),
    );
    env.insert("PASCAL_BUILDER_PUBKEY".into(), keys.builder.clone());
    Config::from_lookup(|k| env.get(k).cloned()).expect("config")
}

/// The hedge desk running on `cfg`, talking to Pascal through the desk's own handle.
struct Desk {
    state: AppState,
    task: tokio::task::JoinHandle<()>,
    rx: broadcast::Receiver<DeskEvent>,
}

impl Desk {
    async fn start(cfg: Config) -> Desk {
        let pascal = desk::pascal::pascal_client(&cfg).expect("pascal handle");
        let pool = desk::db::open(&cfg.db_path).await.expect("db opens");
        let state = AppState::new(cfg, pascal, pool);
        let rx = state.subscribe();
        let hcfg = HedgeConfig {
            roll_timeout: Duration::from_millis(300),
            reconcile_every: Duration::from_secs(3600),
            ..HedgeConfig::default()
        };
        let hedger = Hedger::new(state.clone(), hcfg);
        assert!(hedger.trader.is_some(), "hedger keys load");
        let task = hedger.spawn();
        Desk { state, task, rx }
    }

    async fn stop(self) {
        self.state.shutdown();
        let _ = self.task.await;
        self.state.db.close().await;
    }

    /// What the quoter and the indexer do for a new pending: quote on `quotes`' books, write the
    /// rows, emit `PendingOpened`.
    async fn open_pending(&self, quotes: &pascal_mock::Mock, nonce: u64) -> (PendingOpened, Quote) {
        let q = quote(quotes, &LEGS, STAKE_U);
        let p = PendingOpened {
            pending: format!("pending-{nonce}"),
            ticket: format!("ticket-{nonce}"),
            user: "user-1".into(),
            nonce,
            stake_u: STAKE_U,
            copy_fee_u: 0,
            odds_offered_u: q.odds_offered_u,
            min_payout_per_unit_u: q.min_payout_per_unit_u,
            expires_at: 0,
            legs: q
                .legs
                .iter()
                .zip(LEGS)
                .map(|(l, (_, side))| PendingLeg {
                    symbol: l.symbol.clone(),
                    game_id: l.game_id.clone(),
                    side,
                    start_ms: l.start_ms,
                    price_u: l.price_u,
                    m1_ppm: l.m1_ppm,
                })
                .collect(),
            tx: "tx-open".into(),
            slot: 1,
        };
        insert_rows(&self.state, &p, &q).await;
        self.state
            .emit(DeskEvent::PendingOpened(Arc::new(p.clone())));
        (p, q)
    }

    /// The hedge desk's answer to the pending: `HedgeFilled` or `RefundNeeded`.
    async fn outcome(&mut self, pending: &str) -> DeskEvent {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            match tokio::time::timeout_at(deadline, self.rx.recv()).await {
                Ok(Ok(e)) => match &e {
                    DeskEvent::HedgeFilled(h) if h.pending == pending => return e,
                    DeskEvent::RefundNeeded { pending: p, .. } if p == pending => return e,
                    _ => {}
                },
                Ok(Err(e)) => panic!("bus: {e}"),
                Err(_) => panic!("timed out waiting for the hedge of {pending}"),
            }
        }
    }

    /// The inception orders the desk wrote ahead of sending: (request JSON, status, error), by
    /// client order id.
    async fn inception_orders(&self, pending: &str) -> Vec<(String, String, Option<String>)> {
        sqlx::query(
            "SELECT request_json, status, error FROM hedge_orders
             WHERE kind = 'inception' AND pending = ? ORDER BY client_order_id",
        )
        .bind(pending)
        .fetch_all(&self.state.db)
        .await
        .unwrap()
        .iter()
        .map(|r| (r.get("request_json"), r.get("status"), r.get("error")))
        .collect()
    }
}

fn markets(mock: &pascal_mock::Mock, symbols: &[String]) -> Vec<Market> {
    mock.with_engine(|e| e.search(symbols, &[], &[]))
        .into_iter()
        .map(|m| serde_json::from_value(m).expect("market decodes"))
        .collect()
}

/// The desk's quote (pricing crate) on the mock's current books.
fn quote(mock: &pascal_mock::Mock, legs: &[(&str, Side)], stake_u: u64) -> Quote {
    let symbols: Vec<String> = legs.iter().map(|(s, _)| (*s).to_owned()).collect();
    let markets = markets(mock, &symbols);
    let (books, now) = mock.with_engine(|e| (e.books(&symbols), e.now_ms()));
    let inputs: Vec<LegInput> = legs
        .iter()
        .map(|(s, side)| {
            let m = markets.iter().find(|m| m.symbol == *s).unwrap();
            let side = match side {
                Side::Long => pricing::Side::Long,
                Side::Short => pricing::Side::Short,
            };
            LegInput::from_market(m, side).unwrap()
        })
        .collect();
    pricing::quote(&inputs, &books.books, stake_u, now, &Params::default()).expect("quote")
}

async fn insert_rows(state: &AppState, p: &PendingOpened, q: &Quote) {
    sqlx::query(
        "INSERT INTO tickets (ticket, pending, user, nonce, state, n, stake_u, payout_u,
            odds_offered_u, min_payout_per_unit_u, updated_ms)
         VALUES (?, ?, ?, ?, 'pending', ?, ?, ?, ?, ?, 0)",
    )
    .bind(&p.ticket)
    .bind(&p.pending)
    .bind(&p.user)
    .bind(p.nonce as i64)
    .bind(p.legs.len() as i64)
    .bind(p.stake_u as i64)
    .bind(q.payout_u as i64)
    .bind(p.odds_offered_u as i64)
    .bind(p.min_payout_per_unit_u as i64)
    .execute(&state.db)
    .await
    .unwrap();
    for (i, (l, ql)) in p.legs.iter().zip(&q.legs).enumerate() {
        sqlx::query(
            "INSERT INTO ticket_legs (ticket, idx, symbol, game_id, side, pick, start_ms,
                p_quote_u, m1_ppm, overlaps)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&p.ticket)
        .bind(i as i64)
        .bind(&l.symbol)
        .bind(&l.game_id)
        .bind(if l.side == Side::Long {
            "long"
        } else {
            "short"
        })
        .bind(match ql.pick {
            pricing::Pick::Away => "away",
            pricing::Pick::Home => "home",
        })
        .bind(l.start_ms as i64)
        .bind(l.price_u as i64)
        .bind(i64::from(l.m1_ppm))
        .bind(ql.overlaps)
        .execute(&state.db)
        .await
        .unwrap();
    }
}

/// Verifies one signed request with pascal-rs: signature by `keys`' trading key, owner, the
/// builder code at 0.1%, IOC, price on the market's tick.
fn check_signed(req: &PlaceOrderRequest, keys: &Keys, markets: &[Market]) {
    let v = signing::verify_place_order(req, signing::DEPLOYMENT_PROD)
        .expect("signature verifies")
        .require_signer(&signing::decode_pubkey(&keys.trading_pubkey, "signer").unwrap())
        .expect("signed by the hedger's trading key");
    assert_eq!(signing::base58(&v.header.owner), keys.owner);
    let b = v.body.builder.expect("builder code");
    assert_eq!(signing::base58(&b.pubkey), keys.builder);
    assert_eq!(b.fee_rate_u, BUILDER_FEE_RATE_U);
    assert_eq!(
        &v.permit[120..152],
        b.pubkey.as_slice(),
        "builder bytes 120..152"
    );
    assert_eq!(
        v.permit[184..188],
        1_000i32.to_le_bytes(),
        "fee rate bytes 184..188"
    );
    assert_eq!(v.body.tif, TimeInForce::Ioc);
    let m = markets.iter().find(|m| m.symbol == v.body.symbol).unwrap();
    TickRule::from_market(m)
        .unwrap()
        .validate(v.body.price)
        .expect("price on tick");
}

fn spec(symbol: &str) -> OrderSpec {
    OrderSpec {
        symbol: symbol.into(),
        side: pascal_rs::Side::Bid,
        price_u: 500_000,
        size: 1,
        tif: TimeInForce::Ioc,
        post_only: false,
        reduce_only: false,
        kind: "test",
        reason: "live guard".into(),
        pending: None,
        serves: vec![],
    }
}

// --- 1. live without PASCAL_TRADE_URL ---------------------------------------------------------

#[tokio::test]
async fn live_profile_without_trade_url_fails_every_write_before_any_io() {
    let keys = Keys::test();
    let db = TempDb::new("live-guard-ro");
    let (url, connections, listener) = counting_listener().await;
    let ws = url.replacen("http", "ws", 1);
    // Every URL the desk could use points at the counting listener; PASCAL_TRADE_URL is blank
    // (blank counts as unset).
    let cfg = config(
        &[
            ("PROFILE", "live".into()),
            ("PASCAL_DATA_URL", url.clone()),
            ("PASCAL_WS_URL", ws),
            ("PASCAL_TRADE_URL", "   ".into()),
        ],
        &keys,
        &db,
    );
    // And the production default: PROFILE=live alone reads Pascal's data host, writes nowhere.
    let prod = config(&[("PROFILE", "live".into())], &keys, &db);
    assert_eq!(prod.pascal_data_url, PASCAL_DATA_URL_LIVE);

    let trader = Trader::new(
        keys.signer(),
        &keys.owner,
        &keys.builder,
        BUILDER_FEE_RATE_U,
        Venue::Mock,
    )
    .unwrap();
    let order = trader.sign_order(&spec(PHI), 1).unwrap().signed.request;
    let cancel = trader.sign_cancel(1).unwrap().request;
    let mass = trader.sign_mass_cancel(PHI).unwrap().request;
    for cfg in [&cfg, &prod] {
        assert_eq!(cfg.profile, Profile::Live);
        assert_eq!(cfg.pascal_trade_url, None);
        assert_eq!(desk::pascal::trade_url(cfg), None);
        assert_eq!(venue_of(cfg), Venue::Mock);
        let p = desk::pascal::pascal_client(cfg).unwrap();
        let errs = [
            p.place_orders(std::slice::from_ref(&order))
                .await
                .map(|_| ())
                .unwrap_err(),
            p.place_orders(&[]).await.map(|_| ()).unwrap_err(),
            p.cancel(std::slice::from_ref(&cancel))
                .await
                .map(|_| ())
                .unwrap_err(),
            p.mass_cancel(std::slice::from_ref(&mass))
                .await
                .map(|_| ())
                .unwrap_err(),
        ];
        for e in errs {
            assert!(
                matches!(&e, Error::InvalidArgument(m) if m.contains("PASCAL_TRADE_URL")),
                "{e:?}"
            );
        }
    }
    // desk-flatten builds the same handle (`desk::pascal::pascal_client`), so it is read-only too.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        connections.load(Ordering::SeqCst),
        0,
        "no connection at all"
    );
    listener.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_hedge_desk_without_trade_url_signs_and_stores_but_sends_nothing() {
    init_tracing();
    let keys = Keys::test();
    let db = TempDb::new("live-guard-hedge-ro");
    let data = serve_mock(None).await;
    let cfg = config(
        &[
            ("PROFILE", "live".into()),
            ("PASCAL_DATA_URL", data.url()),
            ("PASCAL_WS_URL", data.ws_url()),
        ],
        &keys,
        &db,
    );
    assert_eq!(desk::pascal::trade_url(&cfg), None);
    let mut desk = Desk::start(cfg).await;
    let (p, _q) = desk.open_pending(&data.mock, 1).await;
    match desk.outcome(&p.pending).await {
        DeskEvent::RefundNeeded { .. } => {}
        e => panic!("expected RefundNeeded, got {e:?}"),
    }
    // The batch was built, signed and written ahead (D-074), then refused locally.
    let markets = markets(&data.mock, &LEGS.map(|(s, _)| s.to_owned()));
    let orders = desk.inception_orders(&p.pending).await;
    assert_eq!(orders.len(), 3, "{orders:?}");
    for (json, status, error) in &orders {
        check_signed(&serde_json::from_str(json).unwrap(), &keys, &markets);
        assert_eq!(status, "error");
        assert!(
            error
                .as_deref()
                .is_some_and(|e| e.contains("PASCAL_TRADE_URL")),
            "{error:?}"
        );
    }
    // The only server the desk can reach saw reads and no write.
    let hits = data.seen.hits();
    assert!(!hits.is_empty(), "the desk read books from the data URL");
    assert!(data.seen.writes().is_empty(), "{hits:?}");
    assert!(
        hits.iter()
            .all(|h| h.method == Method::GET || h.path == "/api/v1/markets/search"),
        "{hits:?}"
    );
    let owner = keys.owner.clone();
    assert!(
        data.mock
            .with_engine(|e| e.account_fills(&owner).is_empty())
    );
    desk.stop().await;
}

// --- 3. a localhost trade URL ------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hedge_desk_with_a_localhost_trade_url_sends_there_and_nowhere_else() {
    init_tracing();
    let keys = Keys::test();
    let db = TempDb::new("live-guard-hedge-local");
    let data = serve_mock(None).await;
    let trade = serve_mock(Some((&keys.owner, &keys.trading_pubkey))).await;
    // The live profile, as mode R would run it, but every URL on 127.0.0.1.
    let cfg = config(
        &[
            ("PROFILE", "live".into()),
            ("PASCAL_DATA_URL", data.url()),
            ("PASCAL_WS_URL", data.ws_url()),
            ("PASCAL_TRADE_URL", trade.url()),
        ],
        &keys,
        &db,
    );
    let trade_url = trade.url();
    assert_eq!(desk::pascal::trade_url(&cfg), Some(trade_url.as_str()));
    assert_eq!(
        venue_of(&cfg),
        Venue::Mock,
        "a localhost trade host is mock"
    );
    let mut desk = Desk::start(cfg).await;
    let (p, _q) = desk.open_pending(&data.mock, 1).await;
    let hf = match desk.outcome(&p.pending).await {
        DeskEvent::HedgeFilled(h) => h,
        e => panic!("expected HedgeFilled, got {e:?}"),
    };
    assert_eq!(
        hf.r_ppm, 1_000_000,
        "the verifying trade mock filled the batch"
    );

    // The trade mock received exactly one write: the inception batch, byte for byte the
    // requests the desk stored before sending.
    let writes = trade.seen.writes();
    assert_eq!(writes.len(), 1, "{writes:?}");
    assert_eq!(writes[0].path, "/api/v1/orders");
    let sent: Vec<PlaceOrderRequest> = serde_json::from_slice(&writes[0].body).unwrap();
    let stored: Vec<PlaceOrderRequest> = desk
        .inception_orders(&p.pending)
        .await
        .iter()
        .map(|(json, status, _)| {
            assert_eq!(status, "placed");
            serde_json::from_str(json).unwrap()
        })
        .collect();
    assert_eq!(sent, stored);
    assert_eq!(sent.len(), 3);
    let markets = markets(&data.mock, &LEGS.map(|(s, _)| s.to_owned()));
    for r in &sent {
        check_signed(r, &keys, &markets);
    }
    // Reads went to the data URL only, and the data mock saw no write.
    assert!(trade.seen.hits().iter().all(|h| h.method == Method::POST));
    assert!(data.seen.writes().is_empty(), "{:?}", data.seen.hits());
    // Fills are labelled mock (D-076).
    let venues: Vec<String> = sqlx::query_scalar("SELECT DISTINCT venue FROM pascal_fills")
        .fetch_all(&desk.state.db)
        .await
        .unwrap();
    assert_eq!(venues, ["mock"]);
    desk.stop().await;
}

// --- 2. the source scan ------------------------------------------------------------------------

/// Pascal's domain, split so that this file does not spell it (the scan reads this file too).
const NEEDLE: [&str; 2] = ["pascal", "trade"];

/// Reviewed lines that may name a Pascal host other than the data and docs hosts:
/// (path from the repository root, exact trimmed line with `{D}` for the domain and `{p}` for
/// its first word, why).
const ALLOWED: &[(&str, &str, &str)] = &[
    (
        "crates/pascal-rs/src/bin/pascal-cli.rs",
        r#"const PROD_TRADE_URL: &str = "https://trade.{D}";"#,
        "the --i-am-authorized path (D-025; its single use is checked by pascal-rs's guard)",
    ),
    (
        "crates/pascal-rs/tests/no_prod_trade_url.rs",
        r#"let host = ["trade", "{p}", "trade"].join(".");"#,
        "the D-025 guard's own needle",
    ),
    (
        "crates/desk/src/hedge/trader.rs",
        r#".is_some_and(|h| h == "{D}" || h.ends_with(".{D}"));"#,
        "venue_of labels fills by host suffix; it is not a destination",
    ),
    (
        "crates/desk/src/hedge/trader.rs",
        r#"c.pascal_trade_url = Some("https://trade.{D}".into());"#,
        "unit test of venue_of; no client is built",
    ),
    (
        "crates/desk/src/hedge/trader.rs",
        r#"c.pascal_trade_url = Some("https://{D}.evil.example".into());"#,
        "unit test of venue_of; no client is built",
    ),
    (
        "crates/desk/tests/live_guard.rs",
        r#"const NEEDLE: [&str; 2] = ["{p}", "trade"];"#,
        "this scan's own needle",
    ),
    (
        "crates/desk/src/pascal.rs",
        r#"h == "{D}" || h.ends_with(".{D}")"#,
        "D-143: is_pascal_host refuses the data-URL write fallback on Pascal hosts; not a destination",
    ),
    (
        "crates/desk/src/pascal.rs",
        r#""https://{D}","#,
        "D-143 unit test input; the handle it builds is read-only",
    ),
    (
        "crates/desk/src/pascal.rs",
        r#""wss://anything.{D}/ws","#,
        "D-143 unit test input; the handle it builds is read-only",
    ),
    (
        "crates/desk/src/pascal.rs",
        r#""https://{D}.evil.example","#,
        "D-143 look-alike host test input; no client is built",
    ),
    (
        "crates/desk/src/pascal.rs",
        r#""https://not{D}","#,
        "D-143 look-alike host test input; no client is built",
    ),
    (
        "scripts/e2e/lib.sh",
        r#"*{D}*|*mainnet*) e2e_die "$v=${!v} is set in the environment; the e2e run is localnet + mock only" ;;"#,
        "3.A: the lifecycle script refuses to run with a Pascal host in its environment; not a destination",
    ),
];

/// An [`ALLOWED`] line with the domain put back.
fn unmask(line: &str) -> String {
    line.replace("{D}", &NEEDLE.join("."))
        .replace("{p}", NEEDLE[0])
}

/// Hosts under Pascal's domain that carry no write API.
const READ_ONLY_LABELS: [&str; 2] = ["data", "docs"];

/// Files the scan reads: Rust, TOML, SQL, shell, Python, JS, CI YAML, `justfile`, `.env.example`.
fn scanned_files(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if !(name.starts_with('.') || name == "target" || name == "node_modules") {
                    walk(&path, out);
                }
            } else if ["rs", "toml", "sql", "sh", "py", "js", "mjs", "yml", "yaml"]
                .iter()
                .any(|e| path.extension().is_some_and(|x| x == *e))
            {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    for d in [
        "crates",
        "app/src",
        "app/tx",
        "app/assets",
        "app/public",
        "app/scripts",
        "scripts",
    ] {
        walk(&root.join(d), &mut out);
    }
    for f in ["justfile", ".env.example", "Cargo.toml", "app/Cargo.toml"] {
        let p = root.join(f);
        if p.is_file() {
            out.push(p);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Byte offsets in `text` (lower-cased) where a Pascal host that is not read-only starts: the
/// domain after a label outside [`READ_ONLY_LABELS`], or the bare domain. A `*.` wildcard
/// (a pattern, not a host) passes.
fn bad_hosts(text: &str) -> Vec<usize> {
    let lower = text.to_ascii_lowercase();
    let domain = NEEDLE.join(".");
    let mut out = Vec::new();
    for (at, _) in lower.match_indices(&domain) {
        let before = &lower[..at];
        let label_start = before
            .char_indices()
            .rev()
            .find(|&(_, c)| !(c.is_ascii_alphanumeric() || c == '-' || c == '.'))
            .map_or(0, |(i, c)| i + c.len_utf8());
        let labels = &lower[label_start..at];
        // `labels` is e.g. "data." or "trade." or "" (bare domain).
        let first = labels
            .trim_end_matches('.')
            .rsplit('.')
            .next()
            .unwrap_or("");
        // `*.` + the domain describes a pattern (comments, venue labelling), not a host.
        let wildcard = labels == "." && before[..label_start].ends_with('*');
        if !(wildcard || labels.ends_with('.') && READ_ONLY_LABELS.contains(&first)) {
            out.push(label_start);
        }
    }
    out
}

/// Rust string literals of `src` with their 1-based line: plain, raw and byte strings; comments
/// and char literals are skipped.
fn rust_literals(src: &str) -> Vec<(usize, String)> {
    let c: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut line) = (0, 1);
    let peek = |i: usize| c.get(i).copied().unwrap_or('\0');
    while i < c.len() {
        match c[i] {
            '\n' => {
                line += 1;
                i += 1;
            }
            '/' if peek(i + 1) == '/' => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            }
            '/' if peek(i + 1) == '*' => {
                let mut depth = 0;
                while i < c.len() {
                    if c[i] == '/' && peek(i + 1) == '*' {
                        depth += 1;
                        i += 2;
                    } else if c[i] == '*' && peek(i + 1) == '/' {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        if c[i] == '\n' {
                            line += 1;
                        }
                        i += 1;
                    }
                }
            }
            '\'' => {
                // 'x', '\n', '\'' or a lifetime.
                if peek(i + 1) == '\\' {
                    i += 2;
                    while i < c.len() && c[i] != '\'' {
                        i += 1;
                    }
                    i += 1;
                } else if peek(i + 2) == '\'' {
                    i += 3;
                } else {
                    i += 1;
                }
            }
            'r' | 'b' if !(i > 0 && (c[i - 1].is_alphanumeric() || c[i - 1] == '_')) => {
                // r"..", r#".."#, b"..", br#".."#
                let mut j = i + 1;
                if c[i] == 'b' && peek(j) == 'r' {
                    j += 1;
                }
                let raw = c[i] == 'r' || peek(i + 1) == 'r';
                let mut hashes = 0;
                while raw && peek(j) == '#' {
                    hashes += 1;
                    j += 1;
                }
                if peek(j) != '"' {
                    i += 1;
                    continue;
                }
                if !raw {
                    i = j; // a byte string: lexed as a plain string next round
                    continue;
                }
                let (start_line, mut s) = (line, String::new());
                j += 1;
                while j < c.len() {
                    if c[j] == '"' && (0..hashes).all(|h| peek(j + 1 + h) == '#') {
                        j += 1 + hashes;
                        break;
                    }
                    if c[j] == '\n' {
                        line += 1;
                    }
                    s.push(c[j]);
                    j += 1;
                }
                out.push((start_line, s));
                i = j;
            }
            '"' => {
                let (start_line, mut s) = (line, String::new());
                i += 1;
                while i < c.len() && c[i] != '"' {
                    if c[i] == '\\' {
                        i += 1;
                    }
                    if c.get(i) == Some(&'\n') {
                        line += 1;
                    }
                    if let Some(&ch) = c.get(i) {
                        s.push(ch);
                    }
                    i += 1;
                }
                out.push((start_line, s));
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

/// Lines where string literals near each other (within 2 lines, up to 6 literals) spell a
/// non-read-only Pascal host once joined: with `""` (`concat!("https://tr", "ade.pas…")`) or as
/// words (`["trade", "pascal", "trade"].join(".")`, `format!("{}.pascal.{}", "trade", …)`).
fn split_hosts(src: &str) -> Vec<usize> {
    let lits = rust_literals(src);
    let mut out = Vec::new();
    for i in 0..lits.len() {
        let window: Vec<&(usize, String)> = lits[i..]
            .iter()
            .take(6)
            .take_while(|(l, _)| *l <= lits[i].0 + 2)
            .collect();
        // Joined with "": a host spanning two or more literals.
        let mut joined = String::new();
        let mut owner = Vec::new(); // literal index of each byte of `joined`
        for (k, (_, s)) in window.iter().enumerate() {
            joined.push_str(s);
            owner.extend(std::iter::repeat_n(k, s.len()));
        }
        let domain = NEEDLE.join(".");
        for at in bad_hosts(&joined) {
            // Only hosts that span literals: one inside a single literal is the raw check's.
            let Some(end) = joined[at..].find(&domain).map(|o| at + o + domain.len()) else {
                continue;
            };
            if owner[at] != owner[end - 1] {
                out.push(window[owner[end - 1]].0);
            }
        }
    }
    // As words over all literals in order: the domain's two words within 2 lines of each other,
    // unless right after a read-only label.
    let words: Vec<(usize, String)> = lits
        .iter()
        .flat_map(|(l, s)| {
            s.to_ascii_lowercase()
                .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
                .filter(|w| !w.is_empty())
                .map(|w| (*l, w.to_owned()))
                .collect::<Vec<_>>()
        })
        .collect();
    for k in 0..words.len().saturating_sub(1) {
        let (line, w) = &words[k];
        if *w == NEEDLE[0] && words[k + 1].1 == NEEDLE[1] && words[k + 1].0 <= line + 2 {
            let after_read_only = k > 0
                && words[k - 1].0 + 2 >= *line
                && READ_ONLY_LABELS.contains(&words[k - 1].1.as_str());
            if !after_read_only {
                out.push(*line);
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

#[test]
fn no_pascal_trade_host_outside_the_authorized_cli_path() {
    let root = repo_root().canonicalize().unwrap();
    let files = scanned_files(&root);
    assert!(files.len() > 50, "scanned {} files", files.len());
    let mut used = vec![0usize; ALLOWED.len()];
    let mut violations = Vec::new();
    for f in &files {
        let rel = f
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        // Raw text, comments included.
        let mut hits: Vec<usize> = bad_hosts(&text)
            .into_iter()
            .map(|at| text[..at].matches('\n').count() + 1)
            .collect();
        if rel.ends_with(".rs") {
            hits.extend(split_hosts(&text));
        }
        hits.sort_unstable();
        hits.dedup();
        for line in hits {
            let trimmed = lines.get(line - 1).map_or("", |l| l.trim());
            match ALLOWED
                .iter()
                .position(|(p, l, _)| *p == rel && unmask(l) == trimmed)
            {
                Some(k) => used[k] += 1,
                None => violations.push(format!("{rel}:{line}: {trimmed}")),
            }
        }
    }
    assert!(
        violations.is_empty(),
        "Pascal host outside the authorized path (D-007, D-025):\n{}",
        violations.join("\n")
    );
    for ((p, l, _), n) in ALLOWED.iter().zip(&used) {
        assert_eq!(*n, 1, "allow-list entry not matched exactly once: {p}: {l}");
    }
}

#[test]
fn only_two_places_build_a_pascal_write_client() {
    let root = repo_root().canonicalize().unwrap();
    let mut sites = Vec::new();
    for f in scanned_files(&root) {
        let rel = f
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        // Library code only: tests may build write clients against their own localhost servers.
        let in_src = rel.contains("/src/") && rel.ends_with(".rs");
        if !in_src || rel == "crates/pascal-rs/src/trade.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        for (i, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if code.contains("TradeClient::builder(")
                || code.contains("TradeClient::new(")
                || code.contains("TradeClientBuilder {")
            {
                sites.push((rel.clone(), i));
            }
            // Pascal's write paths are spelled only by the client and the mock's server.
            if WRITE_PATHS.iter().any(|p| code.contains(&format!("\"{p}"))) {
                assert!(
                    rel.starts_with("crates/pascal-mock/src/"),
                    "{rel}:{}: a Pascal write path outside pascal-rs's trade client",
                    i + 1
                );
            }
        }
        // The desk's handle: the URL comes only from `trade_url(config)` (PASCAL_TRADE_URL, or
        // the mock's data URL on dev/demo; none on live).
        if rel == "crates/desk/src/pascal.rs" {
            assert!(text.contains("Ok(match trade_url(config) {"));
            assert!(text.contains(
                "Some(url) => LiveClient::new(read, TradeClient::builder(url).build()?, ws).into_dyn(),"
            ));
            assert!(text.contains("None => Arc::new(ReadOnly { read, ws }),"));
        }
        // pascal-cli: the write client is built after the --i-am-authorized refusal.
        if rel == "crates/pascal-rs/src/bin/pascal-cli.rs" {
            let refuse = text.find("if !i_am_authorized {").expect("the refusal");
            let build = text
                .find("TradeClient::builder(trade_url)")
                .expect("the client");
            let place = text.find("async fn place_order(").expect("place_order");
            assert!(place < refuse && refuse < build);
        }
    }
    let files: Vec<&str> = sites.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(
        files,
        [
            "crates/desk/src/pascal.rs",
            "crates/pascal-rs/src/bin/pascal-cli.rs"
        ],
        "{sites:?}"
    );
}

#[test]
fn the_scan_sees_through_split_strings() {
    let needle = NEEDLE;
    let host = |label: &str| format!("{label}.{}.{}", needle[0], needle[1]);
    let q = |s: &str| format!("\"{s}\"");
    let caught = |src: &str| !(bad_hosts(src).is_empty() && split_hosts(src).is_empty());
    // Whole hosts, then hosts split across literals in the ways a refactor might.
    for src in [
        format!("let u = {};", q(&format!("https://{}", host("trade")))),
        format!("let u = {};", q(&format!("https://{}", host("api")))),
        format!(
            "let u = {};",
            q(&format!("https://{}.{}", needle[0], needle[1]))
        ),
        format!(
            "let h = [{}, {}, {}].join(\".\");",
            q("trade"),
            q(needle[0]),
            q(needle[1])
        ),
        format!(
            "let u = concat!({}, {});",
            q("https://trade.pas"),
            q(&format!("cal.{}", needle[1]))
        ),
        format!(
            "let u = format!({}, {});",
            q(&format!("https://{{}}.{}.{}", needle[0], needle[1])),
            q("trade")
        ),
        format!(
            "let a = {};\nlet b = {};\nlet u = a + b;",
            q("https://trade."),
            q(&format!("{}.{}", needle[0], needle[1]))
        ),
        format!("let u = r#{}#;", q(&host("trade"))),
        format!("// see {}", host("trade")),
    ] {
        assert!(caught(&src), "not caught: {src}");
    }
    // The read-only hosts, prose and identifiers pass.
    for src in [
        format!("let u = {};", q(&format!("https://{}", host("data")))),
        format!("let u = {};", q(&format!("wss://{}/ws", host("data")))),
        format!("// {}", host("docs")),
        format!(
            "let pascal_trade_url = cfg.pascal_trade_url; // {} ids",
            needle.join(" ")
        ),
        format!("let e = {};", q("PASCAL_TRADE_URL is not set")),
        "let c = '\"'; let s = \"x\";".to_owned(),
        format!("// → {}", host("data")),
    ] {
        assert!(!caught(&src), "false positive: {src}");
    }
}

// --- the demo batch (scripts/sign_demo.sh) -----------------------------------------------------

/// Run by `scripts/sign_demo.sh` with throwaway keys: `PASCAL_HEDGER_OWNER`,
/// `PASCAL_HEDGER_TRADING_KEY_PATH`, `PASCAL_BUILDER_PUBKEY` and `SIGN_DEMO_OUT` (a directory).
/// Writes `batch.json` (the exact `POST /api/v1/orders` body the desk sent to the mock on
/// 127.0.0.1), `signed_order.log` (the desk's `schedina::signed_order` lines) and `fills.txt`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "run by scripts/sign_demo.sh"]
async fn sign_demo() {
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} is not set"));
    let out = PathBuf::from(var("SIGN_DEMO_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let log = std::fs::File::create(out.join("signed_order.log")).unwrap();
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            "schedina::signed_order=info",
        ))
        .with_writer(Mutex::new(log))
        .try_init()
        .expect("sign_demo runs alone (it installs the log writer)");
    let trading_key_path = PathBuf::from(var("PASCAL_HEDGER_TRADING_KEY_PATH"));
    let keys = Keys {
        trading_pubkey: TradingSigner::from_file(&trading_key_path)
            .unwrap()
            .pubkey_base58(),
        trading_key_path,
        owner: var("PASCAL_HEDGER_OWNER"),
        builder: var("PASCAL_BUILDER_PUBKEY"),
        dir: out.clone(),
        remove: false,
    };
    let db = TempDb::new("sign-demo");
    // Mode M as the demo runs it: profile demo, the mock as data and trade URL, every permit
    // verified against the registered trading key.
    let mock = serve_mock(Some((&keys.owner, &keys.trading_pubkey))).await;
    let cfg = config(
        &[("PROFILE", "demo".into()), ("PASCAL_DATA_URL", mock.url())],
        &keys,
        &db,
    );
    assert_eq!(desk::pascal::trade_url(&cfg), Some(mock.url().as_str()));
    let mut desk = Desk::start(cfg).await;
    let (p, q) = desk.open_pending(&mock.mock, 1).await;
    let hf = match desk.outcome(&p.pending).await {
        DeskEvent::HedgeFilled(h) => h,
        e => panic!("expected HedgeFilled, got {e:?}"),
    };
    let writes = mock.seen.writes();
    assert_eq!(writes.len(), 1, "{writes:?}");
    let body = &writes[0].body;
    let sent: Vec<PlaceOrderRequest> = serde_json::from_slice(body).unwrap();
    let markets = markets(&mock.mock, &LEGS.map(|(s, _)| s.to_owned()));
    for r in &sent {
        check_signed(r, &keys, &markets);
    }
    std::fs::write(out.join("batch.json"), body).unwrap();

    let mut fills = format!(
        "schedina: stake {} USDC, odds offered {}, r = {}\n",
        STAKE_U as f64 / 1e6,
        q.odds_offered_u as f64 / 1e6,
        hf.r_ppm as f64 / 1e6
    );
    for ((l, ql), (_, side)) in p.legs.iter().zip(&q.legs).zip(LEGS) {
        fills.push_str(&format!(
            "leg {} {:?}: quoted p {}, p_max {}, {} contracts\n",
            l.symbol,
            side,
            pascal_rs::format_micros(l.price_u as i64),
            pascal_rs::format_micros(ql.p_max_u as i64),
            ql.size
        ));
    }
    let rows = sqlx::query(
        "SELECT symbol, side, price_u, size, fee_u, builder_fee_u, venue FROM pascal_fills
         ORDER BY symbol, trade_id",
    )
    .fetch_all(&desk.state.db)
    .await
    .unwrap();
    for r in rows {
        fills.push_str(&format!(
            "fill {} {} {} @ {}  fee {}  builder_fee {}  venue {}\n",
            r.get::<String, _>("symbol"),
            r.get::<String, _>("side"),
            r.get::<i64, _>("size"),
            pascal_rs::format_micros(r.get("price_u")),
            pascal_rs::format_micros(r.get("fee_u")),
            pascal_rs::format_micros(r.get("builder_fee_u")),
            r.get::<String, _>("venue"),
        ));
    }
    std::fs::write(out.join("fills.txt"), &fills).unwrap();
    print!("{fills}");
    desk.stop().await;
    // The desk's own log carries the same requests, in the same order.
    let logged: Vec<PlaceOrderRequest> = std::fs::read_to_string(out.join("signed_order.log"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .filter(|v| v["fields"]["pending"] == p.pending.as_str())
        .map(|v| serde_json::from_str(v["fields"]["request"].as_str().unwrap()).unwrap())
        .collect();
    assert_eq!(
        logged, sent,
        "schedina::signed_order log = the POSTed batch"
    );
}
