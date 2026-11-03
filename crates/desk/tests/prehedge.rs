//! Review gate of plan step 4.C: the drift pre-hedge and drift rebalance on the `book` stream,
//! against the in-process `pascal-mock` (frozen clock) over HTTP and its `/ws` socket, with a
//! **drifting synthetic tape** built in a temp dir: the sample tape with PHI, ORL and HOU moved
//! 5 h apart (a sequential ticket) plus later book snapshots where one symbol's ladder moves.
//! The desk talks to the mock exactly as in production (`desk::pascal::pascal_client`); the
//! socket-drop test puts a cuttable TCP proxy between the desk and the mock's `/ws`.

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use desk::Config;
use desk::api::types::{Side, TicketStatus};
use desk::hedge::{BUILDER_FEE_RATE_U, DriftConfig, HedgeConfig, Hedger, OrderSpec, Trader};
use desk::state::{AppState, DeskEvent, HedgeFilled, PendingLeg, PendingOpened};
use pascal_mock::{MockConfig, RunningMock, ScriptSource};
use pascal_rs::{MarketSearch, TimeInForce, TradingSigner};
use pricing::{LegInput, Params, Quote};
use serde_json::Value;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

use common::{TempDb, repo_root};

const PHI: &str = "NBA_26OCT28_PHIIND_ML.PHI";
const ORL: &str = "NBA_26OCT28_ORLTOR_ML.ORL";
const HOU: &str = "NBA_26OCT28_HOUMIL_ML.HOU";
const OWNER_SEED: [u8; 32] = [7; 32];
const TRADING_SEED: [u8; 32] = [8; 32];
const BUILDER_SEED: [u8; 32] = [9; 32];
const STAKE: u64 = 10_000_000;
const MIN: u64 = 60_000;

fn pubkey(seed: [u8; 32]) -> String {
    TradingSigner::from_seed(&seed).pubkey_base58()
}

fn owner() -> String {
    pubkey(OWNER_SEED)
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_test_writer()
        .try_init();
}

/// A temporary directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "schedina-prehedge-{tag}-{}-{}-{}",
            std::process::id(),
            desk::now_ms(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// --- the drifting synthetic tape ---------------------------------------------------------------

fn micros(v: &Value) -> i64 {
    pascal_rs::parse_micros(v.as_str().unwrap()).unwrap()
}

/// The sample tape, sequential (PHI, ORL, HOU start 1 h, 6 h, 11 h after the first snapshot),
/// plus one extra `/books` snapshot per `(minutes after the first, symbol, price shift)`: the
/// last snapshot with that symbol's whole ladder moved by the shift (micro-units).
fn drifting_tape(drifts: &[(u64, &str, i64)]) -> (TempDir, u64) {
    let src = repo_root().join("tapes/nba-2026-10-28/sample");
    let dir = TempDir::new("tape");
    std::fs::copy(src.join("trades.jsonl"), dir.0.join("trades.jsonl")).unwrap();
    let lines: Vec<Value> = std::fs::read_to_string(src.join("books.jsonl"))
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let t_of = |v: &Value| -> u64 {
        v["body"]["state_time_ms"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    };
    let t0 = t_of(&lines[0]);
    let mut out: Vec<Value> = lines.clone();
    let mut seq: u64 = lines.last().unwrap()["body"]["seq"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    for &(minutes, symbol, shift) in drifts {
        let mut v = out.last().unwrap().clone();
        let t = t0 + minutes * MIN;
        seq += 1;
        v["recv_ms"] = t.into();
        v["body"]["state_time_ms"] = t.to_string().into();
        v["body"]["server_time_ms"] = t.to_string().into();
        v["body"]["seq"] = seq.to_string().into();
        let book = &mut v["body"]["data"]["books"][symbol];
        for side in ["asks", "bids"] {
            for level in book[side].as_array_mut().unwrap() {
                let p = (micros(&level[0]) + shift).clamp(1_000, 999_000);
                level[0] = pascal_rs::format_micros(p).into();
            }
        }
        out.push(v);
    }
    let mut text = String::new();
    for v in &out {
        text.push_str(&serde_json::to_string(v).unwrap());
        text.push('\n');
    }
    std::fs::write(dir.0.join("books.jsonl"), text).unwrap();
    let shift: HashMap<&str, u64> = [(PHI, 1), (ORL, 6), (HOU, 11)]
        .into_iter()
        .map(|(s, h)| (s, t0 + h * 3_600_000))
        .collect();
    for f in ["markets.jsonl", "search.jsonl"] {
        let text = std::fs::read_to_string(src.join(f)).unwrap();
        let mut out = String::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let mut v: Value = serde_json::from_str(line).unwrap();
            for m in v["body"]["data"].as_array_mut().unwrap() {
                let sym = m["symbol"].as_str().unwrap().to_owned();
                if let Some(&start) = shift.get(sym.as_str()) {
                    let da = &mut m["display_attributes"];
                    da["expected_event_start_time_ms"] = Value::String(start.to_string());
                    da["expected_resolution_time_ms"] =
                        Value::String((start + 4 * 3_600_000).to_string());
                }
            }
            out.push_str(&serde_json::to_string(&v).unwrap());
            out.push('\n');
        }
        std::fs::write(dir.0.join(f), out).unwrap();
    }
    (dir, t0)
}

// --- a cuttable TCP proxy (the socket-drop test) -----------------------------------------------

/// Forwards 127.0.0.1:<port> to `upstream`. [`Proxy::cut`] closes every open connection and
/// refuses new ones until [`Proxy::restore`].
struct Proxy {
    port: u16,
    up: Arc<AtomicBool>,
    conns: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    accept: tokio::task::JoinHandle<()>,
}

async fn write_all(w: &tokio::net::tcp::OwnedWriteHalf, mut data: &[u8]) -> std::io::Result<()> {
    while !data.is_empty() {
        w.writable().await?;
        match w.try_write(data) {
            Ok(n) => data = &data[n..],
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

async fn pump(r: tokio::net::tcp::OwnedReadHalf, w: tokio::net::tcp::OwnedWriteHalf) {
    let mut buf = vec![0u8; 64 << 10];
    loop {
        if r.readable().await.is_err() {
            return;
        }
        match r.try_read(&mut buf) {
            Ok(0) => return,
            Ok(n) => {
                if write_all(&w, &buf[..n]).await.is_err() {
                    return;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => return,
        }
    }
}

impl Proxy {
    async fn start(upstream: String) -> Proxy {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let up = Arc::new(AtomicBool::new(true));
        let conns: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>> = Arc::default();
        let (up2, conns2) = (up.clone(), conns.clone());
        let accept = tokio::spawn(async move {
            loop {
                let Ok((client, _)) = listener.accept().await else {
                    return;
                };
                if !up2.load(Ordering::SeqCst) {
                    drop(client);
                    continue;
                }
                let Ok(server) = TcpStream::connect(&upstream).await else {
                    continue;
                };
                let (cr, cw) = client.into_split();
                let (sr, sw) = server.into_split();
                let mut c = conns2.lock().unwrap();
                c.push(tokio::spawn(pump(cr, sw)));
                c.push(tokio::spawn(pump(sr, cw)));
            }
        });
        Proxy {
            port,
            up,
            conns,
            accept,
        }
    }

    fn cut(&self) {
        self.up.store(false, Ordering::SeqCst);
        for t in self.conns.lock().unwrap().drain(..) {
            t.abort();
        }
    }

    fn restore(&self) {
        self.up.store(true, Ordering::SeqCst);
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.accept.abort();
        self.cut();
    }
}

// --- the environment ---------------------------------------------------------------------------

struct Env {
    keys: TempDir,
    db: TempDb,
    mock: RunningMock,
    state: AppState,
    hedger: Hedger,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    rx: broadcast::Receiver<DeskEvent>,
}

fn drift_cfg() -> HedgeConfig {
    HedgeConfig {
        roll_timeout: Duration::from_millis(300),
        reconcile_every: Duration::from_secs(3600),
        drift: DriftConfig {
            debounce: Duration::from_millis(300),
            cooldown: Duration::from_millis(500),
            eval_every: Duration::from_millis(50),
            rest_every: Duration::from_millis(100),
            ..DriftConfig::default()
        },
        ..HedgeConfig::default()
    }
}

async fn start(tape: &Path, ws_url: Option<String>) -> Env {
    init_tracing();
    let mut mc = MockConfig::new(tape);
    mc.speed = 0.0;
    mc.script = ScriptSource::None;
    mc.initial_collateral_u = 10_000 * 1_000_000;
    let mock = pascal_mock::Mock::new(mc)
        .expect("tape loads")
        .spawn("127.0.0.1:0")
        .await
        .expect("mock serves");
    let keys = TempDir::new("keys");
    std::fs::write(
        keys.0.join("trading.json"),
        serde_json::to_string(&TRADING_SEED.to_vec()).unwrap(),
    )
    .unwrap();
    with_parts(keys, TempDb::new("prehedge"), mock, ws_url).await
}

async fn with_parts(keys: TempDir, db: TempDb, mock: RunningMock, ws_url: Option<String>) -> Env {
    let mut cfg = Config::local(mock.base_url(), db.path());
    if let Some(u) = ws_url {
        cfg.pascal_ws_url = u;
    }
    cfg.hedger_owner = Some(owner());
    cfg.hedger_trading_key_path = Some(keys.0.join("trading.json"));
    cfg.builder_pubkey = Some(pubkey(BUILDER_SEED));
    let pascal = desk::pascal::pascal_client(&cfg).expect("client");
    let pool = desk::db::open(&db.path()).await.expect("db opens");
    let state = AppState::new(cfg, pascal, pool);
    let rx = state.subscribe();
    let hedger = Hedger::new(state.clone(), drift_cfg());
    assert!(hedger.trader.is_some(), "hedger keys load");
    let mut tasks = vec![hedger.clone().spawn()];
    tasks.extend(hedger.spawn_drift());
    Env {
        keys,
        db,
        mock,
        state,
        hedger,
        tasks,
        rx,
    }
}

impl Env {
    /// Stops the desk (as a crash would) and returns what a restart reuses.
    async fn stop(self) -> (TempDir, TempDb, RunningMock) {
        self.state.shutdown();
        for t in self.tasks {
            let _ = t.await;
        }
        self.state.db.close().await;
        (self.keys, self.db, self.mock)
    }

    fn position(&self, symbol: &str) -> i64 {
        let o = owner();
        self.mock.mock.with_engine(|e| {
            e.account(&o, Some(symbol), true)
                .positions
                .iter()
                .map(|p| p.size)
                .sum()
        })
    }

    /// Moves the mock's (frozen) clock to `t_ms`: the tape snapshot of that time is the book.
    fn set_clock(&self, t_ms: u64) {
        self.mock
            .mock
            .with_engine(|e| e.set_clock(Some(t_ms), None, None))
            .map_err(|r| format!("{r:?}"))
            .expect("clock moves");
    }

    async fn quote(&self, legs: &[(&str, Side)]) -> Quote {
        let symbols: Vec<String> = legs.iter().map(|(s, _)| (*s).to_owned()).collect();
        let markets = self
            .state
            .pascal
            .search_markets(&MarketSearch {
                symbols: symbols.clone(),
                ..MarketSearch::default()
            })
            .await
            .unwrap()
            .data;
        let books = self.state.pascal.books(&symbols).await.unwrap();
        let inputs: Vec<LegInput> = legs
            .iter()
            .map(|(s, side)| {
                let m = markets.iter().find(|m| m.symbol == *s).unwrap();
                let ps = match side {
                    Side::Long => pricing::Side::Long,
                    Side::Short => pricing::Side::Short,
                };
                LegInput::from_market(m, ps).unwrap()
            })
            .collect();
        pricing::quote(
            &inputs,
            &books.data.books,
            STAKE,
            books.state_time_ms.unwrap(),
            &Params::default(),
        )
        .expect("quote")
    }

    async fn wait(&mut self, what: &str, pred: impl Fn(&DeskEvent) -> bool) -> DeskEvent {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            match tokio::time::timeout_at(deadline, self.rx.recv()).await {
                Ok(Ok(e)) if pred(&e) => return e,
                Ok(Ok(_)) => {}
                Ok(Err(e)) => panic!("bus: {e}"),
                Err(_) => panic!("timed out waiting for {what}"),
            }
        }
    }

    /// What the quoter and the indexer do (quote, rows, `PendingOpened`), the inception, then
    /// what the indexer does on `finalize` (row open, `TicketMinted`).
    async fn open_ticket(&mut self, legs: &[(&str, Side)], nonce: u64) -> PendingOpened {
        let q = self.quote(legs).await;
        let p = PendingOpened {
            pending: format!("pending-{nonce}"),
            ticket: format!("ticket-{nonce}"),
            user: "user-1".into(),
            nonce,
            stake_u: STAKE,
            copy_fee_u: 0,
            odds_offered_u: q.odds_offered_u,
            min_payout_per_unit_u: q.min_payout_per_unit_u,
            expires_at: 0,
            legs: q
                .legs
                .iter()
                .zip(legs)
                .map(|(l, (_, side))| PendingLeg {
                    symbol: l.symbol.clone(),
                    game_id: l.game_id.clone(),
                    side: *side,
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
        let pending = p.pending.clone();
        let hf = match self
            .wait("HedgeFilled", |e| {
                matches!(e, DeskEvent::HedgeFilled(h) if h.pending == pending)
                    || matches!(e, DeskEvent::RefundNeeded { pending: x, .. } if *x == pending)
            })
            .await
        {
            DeskEvent::HedgeFilled(h) => (*h).clone(),
            e => panic!("expected HedgeFilled, got {e:?}"),
        };
        self.mint(&p, &hf).await;
        p
    }

    async fn mint(&mut self, p: &PendingOpened, hf: &HedgeFilled) {
        let legs: Vec<pricing::PayoutLeg> = p
            .legs
            .iter()
            .zip(&hf.f_u)
            .map(|(l, &f)| pricing::PayoutLeg {
                m1_ppm: l.m1_ppm,
                f_u: if f == 0 { l.price_u } else { f },
            })
            .collect();
        let pay = pricing::payout(p.stake_u, hf.r_ppm, &legs).unwrap();
        sqlx::query(
            "UPDATE tickets SET state = 'open', rs_u = ?, r_ppm = ?, payout_u = ? WHERE ticket = ?",
        )
        .bind(pay.rs_u as i64)
        .bind(i64::from(hf.r_ppm))
        .bind(pay.payout_u as i64)
        .bind(&p.ticket)
        .execute(&self.state.db)
        .await
        .unwrap();
        for (i, &f) in hf.f_u.iter().enumerate() {
            sqlx::query(
                "UPDATE ticket_legs SET p_fill_u = ?, hedged = ? WHERE ticket = ? AND idx = ?",
            )
            .bind(if f == 0 { p.legs[i].price_u } else { f } as i64)
            .bind(f != 0)
            .bind(&p.ticket)
            .bind(i as i64)
            .execute(&self.state.db)
            .await
            .unwrap();
        }
        self.state.emit(DeskEvent::TicketMinted {
            pending: p.pending.clone(),
            ticket: p.ticket.clone(),
            user: p.user.clone(),
            tx: "tx-finalize".into(),
        });
        let t = p.ticket.clone();
        let h = self.hedger.clone();
        eventually("ticket open", || {
            h.tickets()
                .iter()
                .any(|x| x.ticket == t && x.status == TicketStatus::Open)
        })
        .await;
    }

    async fn count(&self, sql: &'static str) -> i64 {
        sqlx::query_scalar(sql)
            .fetch_one(&self.state.db)
            .await
            .unwrap()
    }

    async fn orders(&self, kind: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM hedge_orders WHERE kind = ?")
            .bind(kind)
            .fetch_one(&self.state.db)
            .await
            .unwrap()
    }

    fn best_ask(&self, symbol: &str) -> Option<i64> {
        self.hedger
            .live_book(symbol)
            .and_then(|b| b.asks.first().map(|l| l.price))
    }
}

async fn eventually(what: &str, cond: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !cond() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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

const LEGS: [(&str, Side); 3] = [(PHI, Side::Long), (ORL, Side::Long), (HOU, Side::Long)];

/// A sequential 3-leg ticket: only PHI hedged, the book socket on all three symbols.
async fn sequential_ticket(env: &mut Env) -> PendingOpened {
    assert_eq!(
        env.hedger.stream_status().subscribed,
        0,
        "no ticket, no socket"
    );
    let p = env.open_ticket(&LEGS, 1).await;
    assert!(env.hedger.target(PHI) > 0);
    assert_eq!(env.hedger.target(ORL), 0);
    assert_eq!(env.hedger.target(HOU), 0);
    let h = env.hedger.clone();
    eventually("live books on the three legs", || {
        let s = h.stream_status();
        s.connected && s.subscribed == 3 && s.live == 3
    })
    .await;
    p
}

// --- the gate ----------------------------------------------------------------------------------

/// The third leg's side gets 6 points dearer: pre-hedged once, from the socket's book, bought to
/// its target; the subscription drops when the ticket closes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn third_leg_drifting_against_the_ticket_is_prehedged_once() {
    let (tape, t0) = drifting_tape(&[(10, HOU, 60_000)]);
    let mut env = start(&tape.0, None).await;
    let p = sequential_ticket(&mut env).await;
    let ask0 = env.best_ask(HOU).unwrap();
    assert_eq!(env.hedger.drift_counts().prehedges, 0);

    env.set_clock(t0 + 10 * MIN);
    let h = env.hedger.clone();
    eventually("HOU's moved book on the socket", || {
        h.live_book(HOU)
            .and_then(|b| b.asks.first().map(|l| l.price))
            == Some(ask0 + 60_000)
    })
    .await;
    eventually("pre-hedge", || h.drift_counts().prehedges == 1).await;
    let target = env.hedger.target(HOU);
    assert!(target > 0, "HOU joined the hedged set: target {target}");
    // PHI keeps its inception target; ORL stays unhedged.
    assert_eq!(env.hedger.target(ORL), 0);
    assert!(env.hedger.target(PHI) > 0);
    let e = &env;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while e.position(HOU) != target {
        assert!(
            tokio::time::Instant::now() < deadline,
            "HOU never reached {target}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(env.hedger.held(HOU), target);
    assert_eq!(env.hedger.prehedged_legs(), vec![(p.ticket.clone(), 2)]);
    // The row was written, from the socket's book, with the trigger price.
    let (source, quote_u, trig_u): (String, i64, i64) = sqlx::query_as(
        "SELECT source, quote_price_u, trigger_price_u FROM hedge_prehedges
         WHERE ticket = ? AND leg_idx = 2",
    )
    .bind(&p.ticket)
    .fetch_one(&env.state.db)
    .await
    .unwrap();
    assert_eq!(source, "ws");
    assert!(trig_u > quote_u + 50_000, "{trig_u} vs {quote_u}");

    // Exactly once: the book stays moved, nothing more fires.
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert_eq!(env.hedger.drift_counts().prehedges, 1);
    assert_eq!(env.orders("prehedge").await, 1);
    assert_eq!(env.orders("drift").await, 0);
    assert_eq!(env.count("SELECT COUNT(*) FROM hedge_prehedges").await, 1);
    assert_eq!(env.position(HOU), target);

    // The live prices reach the API's shared state and the hedge debug view.
    let shared = env.state.books.read().get(HOU).cloned().unwrap();
    assert_eq!(shared.asks[0].price, ask0 + 60_000);
    let dbg = env.hedger.debug_view();
    assert_eq!(dbg["stream"]["connected"], true);
    assert_eq!(dbg["books"][HOU]["ask_u"], ask0 + 60_000);
    assert_eq!(dbg["prehedged"][0]["leg"], 2);

    // Cash-out closes the ticket: its subscriptions are dropped and the socket closes.
    env.state.emit(DeskEvent::TicketCashedOut {
        ticket: p.ticket.clone(),
        user: p.user.clone(),
        c_u: 1,
        tx: "tx-cash".into(),
    });
    eventually("no subscriptions", || {
        let s = h.stream_status();
        s.subscribed == 0 && !s.connected
    })
    .await;
    assert!(env.hedger.prehedged_legs().is_empty());
    env.stop().await;
}

/// A 3-point move is under the 5-point trigger: no pre-hedge, no order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drift_under_the_threshold_triggers_nothing() {
    let (tape, t0) = drifting_tape(&[(10, HOU, 30_000), (20, ORL, -80_000)]);
    let mut env = start(&tape.0, None).await;
    sequential_ticket(&mut env).await;
    let ask0 = env.best_ask(HOU).unwrap();
    env.set_clock(t0 + 10 * MIN);
    let h = env.hedger.clone();
    eventually("HOU's moved book on the socket", || {
        h.live_book(HOU)
            .and_then(|b| b.asks.first().map(|l| l.price))
            == Some(ask0 + 30_000)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    // ORL gets 8 points cheaper for the user: drift in the desk's favour, no trigger either.
    env.set_clock(t0 + 20 * MIN);
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let c = env.hedger.drift_counts();
    assert_eq!((c.prehedges, c.prehedge_orders, c.drift_orders), (0, 0, 0));
    assert_eq!(env.count("SELECT COUNT(*) FROM hedge_prehedges").await, 0);
    assert_eq!(
        env.count("SELECT COUNT(*) FROM hedge_orders WHERE kind NOT IN ('inception', 'excess')")
            .await,
        0
    );
    assert_eq!(env.position(HOU), 0);
    assert_eq!(env.position(ORL), 0);
    env.stop().await;
}

/// A hedged leg pushed 5 contracts under target (an outside reduce the 30 s reconcile booked):
/// one drift IOC brings it back, and nothing churns afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hedged_leg_pushed_off_target_is_rebalanced_once() {
    let (tape, _) = drifting_tape(&[]);
    let mut env = start(&tape.0, None).await;
    sequential_ticket(&mut env).await;
    let target = env.hedger.target(PHI);
    let h = env.hedger.clone();
    // Right after the mint PHI may hold one contract over its net target (D-123).
    let held = env.hedger.held(PHI);
    assert!(
        (target..=target + 1).contains(&held),
        "PHI {held} vs {target}"
    );
    assert!(target >= 10, "PHI target {target}");
    assert_eq!(env.position(PHI), held);

    // The push: a reduce-only IOC sell straight to the mock leaves PHI 5 under target, then
    // `held` as /accounts shows it.
    let push = held - (target - 5);
    let trader = Trader::from_config(&env.state.config, BUILDER_FEE_RATE_U).unwrap();
    let spec = OrderSpec {
        symbol: PHI.into(),
        side: pascal_rs::Side::Ask,
        price_u: 10_000,
        size: push as u64,
        tif: TimeInForce::Ioc,
        post_only: false,
        reduce_only: true,
        kind: "outside",
        reason: "test".into(),
        pending: None,
        serves: Vec::new(),
    };
    let signed = trader
        .sign_order(&spec, env.state.next_client_order_id())
        .unwrap();
    let r = desk::hedge::trader::place(&env.state.pascal, &[signed])
        .await
        .unwrap();
    assert_eq!(r[0].as_ref().unwrap().data.order.size_filled, push as u64);
    assert_eq!(env.position(PHI), target - 5);
    env.state.held.write().insert(PHI.into(), target - 5);

    eventually("one drift order", || h.drift_counts().drift_orders == 1).await;
    eventually("PHI back on target", || h.held(PHI) == target).await;
    assert_eq!(env.position(PHI), target);
    // Several debounce and cooldown periods later: still one.
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert_eq!(env.hedger.drift_counts().drift_orders, 1);
    assert_eq!(env.orders("drift").await, 1);
    assert_eq!(env.orders("prehedge").await, 0);
    let (side, size, reduce): (String, i64, bool) =
        sqlx::query_as("SELECT side, size, reduce_only FROM hedge_orders WHERE kind = 'drift'")
            .fetch_one(&env.state.db)
            .await
            .unwrap();
    assert_eq!((side.as_str(), size, reduce), ("bid", 5, false));
    env.stop().await;
}

/// The socket drops: books go stale, the trigger reads REST `/books` and still pre-hedges; the
/// socket comes back and resubscribes with fresh snapshots.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn socket_drop_falls_back_to_rest_and_resubscribes() {
    let (tape, t0) = drifting_tape(&[(10, HOU, 60_000)]);
    // The mock first, so the proxy knows where to forward.
    init_tracing();
    let mut mc = MockConfig::new(&tape.0);
    mc.speed = 0.0;
    mc.script = ScriptSource::None;
    mc.initial_collateral_u = 10_000 * 1_000_000;
    let mock = pascal_mock::Mock::new(mc)
        .expect("tape loads")
        .spawn("127.0.0.1:0")
        .await
        .expect("mock serves");
    let upstream = mock.base_url().trim_start_matches("http://").to_owned();
    let proxy = Proxy::start(upstream).await;
    let keys = TempDir::new("keys");
    std::fs::write(
        keys.0.join("trading.json"),
        serde_json::to_string(&TRADING_SEED.to_vec()).unwrap(),
    )
    .unwrap();
    let ws = format!("ws://127.0.0.1:{}/ws", proxy.port);
    let mut env = with_parts(keys, TempDb::new("prehedge"), mock, Some(ws)).await;
    let p = sequential_ticket(&mut env).await;
    let connections = env.hedger.stream_status().epoch;
    let polls = env.hedger.drift_counts().rest_polls;

    proxy.cut();
    let h = env.hedger.clone();
    eventually("socket down", || !h.stream_status().connected).await;
    assert!(env.hedger.live_book(HOU).is_none(), "stale while down");
    env.set_clock(t0 + 10 * MIN);
    eventually("pre-hedge from REST", || h.drift_counts().prehedges == 1).await;
    assert!(env.hedger.drift_counts().rest_polls > polls);
    let source: String =
        sqlx::query_scalar("SELECT source FROM hedge_prehedges WHERE ticket = ? AND leg_idx = 2")
            .bind(&p.ticket)
            .fetch_one(&env.state.db)
            .await
            .unwrap();
    assert_eq!(source, "rest");
    let target = env.hedger.target(HOU);
    eventually("HOU on target", || h.held(HOU) == target).await;
    assert_eq!(env.position(HOU), target);

    proxy.restore();
    eventually("reconnected and resubscribed", || {
        let s = h.stream_status();
        s.connected && s.epoch > connections && s.live == 3
    })
    .await;
    // The fresh snapshot carries the moved book.
    let ask = env.best_ask(HOU).unwrap();
    let rest = env.state.pascal.books(&[HOU.to_owned()]).await.unwrap();
    assert_eq!(ask, rest.data.books[HOU].asks[0].price);
    assert_eq!(env.hedger.drift_counts().prehedges, 1);
    assert!(env.hedger.drift_counts().disconnects >= 1);
    env.stop().await;
    drop(proxy);
}

/// A restart reloads the pre-hedge: the leg stays in the hedged set (target kept, position
/// untouched) and the trigger does not fire again although the book is still moved.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_does_not_refire_a_prehedge() {
    let (tape, t0) = drifting_tape(&[(10, HOU, 60_000)]);
    let mut env = start(&tape.0, None).await;
    let p = sequential_ticket(&mut env).await;
    env.set_clock(t0 + 10 * MIN);
    let h = env.hedger.clone();
    eventually("pre-hedge", || h.drift_counts().prehedges == 1).await;
    let target = env.hedger.target(HOU);
    eventually("HOU on target", || h.held(HOU) == target).await;
    let phi = env.hedger.target(PHI);

    let (keys, db, mock) = env.stop().await;
    let env = with_parts(keys, db, mock, None).await;
    let h = env.hedger.clone();
    // Recovery (tickets, marks, held) can finish after the stream is back under a loaded run;
    // the desk trades only once it has, so wait for both.
    eventually("recovered with live books", || {
        let s = h.stream_status();
        h.is_recovered() && s.connected && s.live == 3
    })
    .await;
    assert_eq!(env.hedger.prehedged_legs(), vec![(p.ticket.clone(), 2)]);
    assert_eq!(env.hedger.target(HOU), target, "pre-hedge not forgotten");
    assert_eq!(env.hedger.target(PHI), phi);
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let c = env.hedger.drift_counts();
    assert_eq!((c.prehedges, c.prehedge_orders, c.drift_orders), (0, 0, 0));
    assert_eq!(env.orders("prehedge").await, 1);
    assert_eq!(env.count("SELECT COUNT(*) FROM hedge_prehedges").await, 1);
    assert_eq!(env.position(HOU), target);
    assert_eq!(env.hedger.held(HOU), target);
    env.stop().await;
}
