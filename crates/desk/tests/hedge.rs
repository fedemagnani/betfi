//! Review gate of plan step 2.B: the hedge desk against the in-process `pascal-mock` (1.D
//! matcher, frozen clock, sample tape or a synthetic tape built here), behind an in-test fake
//! `PascalApi` (l2-contracts L2-5): orders, cancels and mass cancels go over HTTP to the mock's
//! matcher; reads come straight from the mock's engine, and `/accounts/{owner}/fills`, which the
//! 1.D mock does not serve, is answered from the engine's fill ledger.

mod common;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use desk::Config;
use desk::api::types::{LegStatus, Side, TicketStatus};
use desk::hedge::{BUILDER_FEE_RATE_U, HedgeConfig, Hedger, OrderSpec, Trader};
use desk::state::{
    AppState, DeskEvent, HedgeFilled, HedgeUnwound, Pascal, PendingLeg, PendingOpened,
};
use pascal_mock::{At, MockConfig, RunningMock, ScriptSource};
use pascal_rs::{
    AccountQuery, AccountState, Books, BuilderStats, CancelOrderRequest, Channel, Fill, Market,
    MarketSearch, MassCancelRequest, MassCancelled, Order, Page, PageParams, PascalApi,
    PlaceOrderRequest, PlacedOrder, ResolvedQuery, Response, TimeInForce, TradingSigner,
    WriteResult, WsStream,
};
use pricing::{LegInput, Params, Quote};
use serde_json::Value;
use sqlx::Row;
use tokio::sync::broadcast;

use common::{TempDb, repo_root};

const PHI: &str = "NBA_26OCT28_PHIIND_ML.PHI";
const ORL: &str = "NBA_26OCT28_ORLTOR_ML.ORL";
const HOU: &str = "NBA_26OCT28_HOUMIL_ML.HOU";
const SAS: &str = "NBA_26OCT28_SASUTA_ML.SAS";
const OWNER_SEED: [u8; 32] = [7; 32];
const TRADING_SEED: [u8; 32] = [8; 32];
const BUILDER_SEED: [u8; 32] = [9; 32];
const OTHER_SEED: [u8; 32] = [11; 32];
const USD: i64 = 1_000_000;
const STAKE: u64 = 10_000_000;

fn pubkey(seed: [u8; 32]) -> String {
    TradingSigner::from_seed(&seed).pubkey_base58()
}

fn owner() -> String {
    pubkey(OWNER_SEED)
}

fn pside(s: Side) -> pricing::Side {
    match s {
        Side::Long => pricing::Side::Long,
        Side::Short => pricing::Side::Short,
    }
}

fn sigma(s: Side) -> i64 {
    match s {
        Side::Long => 1,
        Side::Short => -1,
    }
}

fn tmp_path(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    std::env::temp_dir().join(format!(
        "schedina-hedge-{tag}-{}-{}-{}",
        std::process::id(),
        desk::now_ms(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

/// A temporary directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let p = tmp_path(tag);
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// --- the fake PascalApi ----------------------------------------------------------------------

struct FakePascal {
    http: Pascal,
    mock: pascal_mock::Mock,
    fail_account: AtomicBool,
    front_run: Mutex<Vec<PlaceOrderRequest>>,
}

impl FakePascal {
    fn resp<T>(&self, data: T) -> Response<T> {
        let t = self.mock.with_engine(|e| e.now_ms());
        Response {
            data,
            seq: None,
            state_time_ms: Some(t),
            server_time_ms: Some(t),
            raw: Value::Null,
        }
    }
}

#[async_trait]
impl PascalApi for FakePascal {
    async fn markets(&self) -> pascal_rs::Result<Response<Vec<Market>>> {
        self.http.markets().await
    }
    async fn search_markets(
        &self,
        body: &MarketSearch,
    ) -> pascal_rs::Result<Response<Vec<Market>>> {
        let raw = self
            .mock
            .with_engine(|e| e.search(&body.symbols, &body.events, &body.games));
        let markets = raw
            .into_iter()
            .map(|m| serde_json::from_value(m).expect("market decodes"))
            .collect();
        Ok(self.resp(markets))
    }
    async fn resolved_markets(&self, q: ResolvedQuery) -> pascal_rs::Result<Response<Vec<Market>>> {
        self.http.resolved_markets(q).await
    }
    async fn books(&self, symbols: &[String]) -> pascal_rs::Result<Response<Books>> {
        let b = self.mock.with_engine(|e| e.books(symbols));
        Ok(self.resp(b))
    }
    async fn account(
        &self,
        owner: &str,
        q: &AccountQuery,
    ) -> pascal_rs::Result<Response<AccountState>> {
        if self.fail_account.load(Ordering::SeqCst) {
            return Err(pascal_rs::Error::InvalidArgument(
                "fake: /accounts is down".into(),
            ));
        }
        let a = self
            .mock
            .with_engine(|e| e.account(owner, q.symbol.as_deref(), q.exclude_orders));
        Ok(self.resp(a))
    }
    async fn fills(
        &self,
        owner: &str,
        symbols: &[String],
        page: &PageParams,
    ) -> pascal_rs::Result<Response<Page<Fill>>> {
        let mut all: Vec<Fill> = self
            .mock
            .with_engine(|e| e.account_fills(owner).to_vec())
            .into_iter()
            .filter(|f| symbols.is_empty() || symbols.contains(&f.symbol))
            .collect();
        all.reverse(); // newest first, as Pascal pages them
        let start = page
            .before_cursor
            .as_deref()
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = page.limit.unwrap_or(100) as usize;
        let items: Vec<Fill> = all.iter().skip(start).take(limit).cloned().collect();
        let next_cursor = (start + limit < all.len()).then(|| (start + limit).to_string());
        Ok(self.resp(Page { items, next_cursor }))
    }
    async fn builder_stats(&self, owner: &str) -> pascal_rs::Result<Response<BuilderStats>> {
        self.http.builder_stats(owner).await
    }
    async fn place_orders(
        &self,
        orders: &[PlaceOrderRequest],
    ) -> pascal_rs::Result<Vec<WriteResult<PlacedOrder>>> {
        let front: Vec<PlaceOrderRequest> = std::mem::take(&mut *self.front_run.lock().unwrap());
        if !front.is_empty() {
            for r in self.http.place_orders(&front).await? {
                r.expect("front-run order placed");
            }
        }
        self.http.place_orders(orders).await
    }
    async fn cancel(
        &self,
        cancels: &[CancelOrderRequest],
    ) -> pascal_rs::Result<Vec<WriteResult<Order>>> {
        self.http.cancel(cancels).await
    }
    async fn mass_cancel(
        &self,
        requests: &[MassCancelRequest],
    ) -> pascal_rs::Result<Vec<WriteResult<MassCancelled>>> {
        self.http.mass_cancel(requests).await
    }
    async fn subscribe(&self, channels: Vec<Channel>) -> pascal_rs::Result<WsStream> {
        self.http.subscribe(channels).await
    }
}

// --- the environment ---------------------------------------------------------------------------

struct Env {
    keys: TempDir,
    db: TempDb,
    mock: RunningMock,
    fake: Arc<FakePascal>,
    state: AppState,
    hedger: Hedger,
    task: tokio::task::JoinHandle<()>,
    rx: broadcast::Receiver<DeskEvent>,
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_test_writer()
        .try_init();
}

fn sample_tape() -> PathBuf {
    repo_root().join("tapes/nba-2026-10-28/sample")
}

async fn start(tape: &Path, collateral_u: i64) -> Env {
    start_with(tape, collateral_u, false).await
}

/// `verify`: the 2.D mock checks every permit signature (L2-5).
async fn start_with(tape: &Path, collateral_u: i64, verify: bool) -> Env {
    init_tracing();
    let mut mc = MockConfig::new(tape);
    mc.verify_permits = verify;
    mc.speed = 0.0;
    mc.script = ScriptSource::None;
    mc.initial_collateral_u = collateral_u;
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
    with_parts(keys, TempDb::new("hedge"), mock, false, false).await
}

/// `real`: the desk talks to the mock over HTTP only (its own `/fills` and `/accounts`),
/// exactly as in production; otherwise through [`FakePascal`].
async fn with_parts(
    keys: TempDir,
    db: TempDb,
    mock: RunningMock,
    fail_account: bool,
    real: bool,
) -> Env {
    let mut cfg = Config::local(mock.base_url(), db.path());
    cfg.hedger_owner = Some(owner());
    cfg.hedger_trading_key_path = Some(keys.0.join("trading.json"));
    cfg.builder_pubkey = Some(pubkey(BUILDER_SEED));
    let fake = Arc::new(FakePascal {
        http: desk::pascal::pascal_client(&cfg).expect("client"),
        mock: mock.mock.clone(),
        fail_account: AtomicBool::new(fail_account),
        front_run: Mutex::new(Vec::new()),
    });
    let pool = desk::db::open(&db.path()).await.expect("db opens");
    let pascal: Pascal = if real {
        desk::pascal::pascal_client(&cfg).expect("client")
    } else {
        fake.clone()
    };
    let state = AppState::new(cfg, pascal, pool);
    let hcfg = HedgeConfig {
        roll_timeout: Duration::from_millis(300),
        reconcile_every: Duration::from_secs(3600),
        ..HedgeConfig::default()
    };
    let rx = state.subscribe();
    let hedger = Hedger::new(state.clone(), hcfg);
    assert!(hedger.trader.is_some(), "hedger keys load");
    let task = hedger.clone().spawn();
    Env {
        keys,
        db,
        mock,
        fake,
        state,
        hedger,
        task,
        rx,
    }
}

impl Env {
    /// Stops the hedge desk (as a crash would) and returns what a restart reuses.
    async fn stop(self) -> (TempDir, TempDb, RunningMock) {
        self.state.shutdown();
        let _ = self.task.await;
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

    async fn quote(&self, legs: &[(&str, Side)], stake_u: u64) -> Quote {
        let symbols: Vec<String> = legs.iter().map(|(s, _)| (*s).to_owned()).collect();
        let markets = self
            .fake
            .search_markets(&MarketSearch {
                symbols: symbols.clone(),
                ..MarketSearch::default()
            })
            .await
            .unwrap()
            .data;
        let books = self.fake.books(&symbols).await.unwrap();
        let inputs: Vec<LegInput> = legs
            .iter()
            .map(|(s, side)| {
                let m = markets.iter().find(|m| m.symbol == *s).unwrap();
                LegInput::from_market(m, pside(*side)).unwrap()
            })
            .collect();
        pricing::quote(
            &inputs,
            &books.data.books,
            stake_u,
            books.state_time_ms.unwrap(),
            &Params::default(),
        )
        .expect("quote")
    }

    /// What the quoter (2.A) and the indexer (2.C) would do: quote, write the pending's rows,
    /// emit `PendingOpened`.
    async fn open_pending(
        &mut self,
        legs: &[(&str, Side)],
        nonce: u64,
        tweak: impl FnOnce(&mut PendingOpened),
    ) -> (PendingOpened, Quote) {
        let q = self.quote(legs, STAKE).await;
        let mut p = PendingOpened {
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
        tweak(&mut p);
        insert_rows(&self.state, &p, &q).await;
        self.state
            .emit(DeskEvent::PendingOpened(Arc::new(p.clone())));
        (p, q)
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

    async fn hedge_filled(&mut self, pending: &str) -> HedgeFilled {
        let pending = pending.to_owned();
        match self
            .wait("HedgeFilled", |e| {
                matches!(e, DeskEvent::HedgeFilled(h) if h.pending == pending)
                    || matches!(e, DeskEvent::RefundNeeded { pending: p, .. } if *p == pending)
            })
            .await
        {
            DeskEvent::HedgeFilled(h) => (*h).clone(),
            e => panic!("expected HedgeFilled, got {e:?}"),
        }
    }

    /// What the indexer does on `finalize`: the ticket row goes open with the final payout,
    /// then `TicketMinted`.
    async fn mint(&mut self, p: &PendingOpened, hf: &HedgeFilled) -> u64 {
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
        assert!(pricing::floor_ok(
            pay.payout_u,
            pay.rs_u,
            p.min_payout_per_unit_u
        ));
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
        pay.payout_u
    }

    async fn count(&self, sql: &'static str) -> i64 {
        sqlx::query_scalar(sql)
            .fetch_one(&self.state.db)
            .await
            .unwrap()
    }
}

async fn eventually(what: &str, cond: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
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

/// The sample tape with PHI, ORL and HOU moved 5 h apart (1 h, 6 h, 11 h after the first
/// snapshot), so none overlaps another: a sequential ticket.
fn sequential_tape() -> TempDir {
    let src = sample_tape();
    let dir = TempDir::new("tape");
    for f in ["books.jsonl", "trades.jsonl"] {
        std::fs::copy(src.join(f), dir.0.join(f)).unwrap();
    }
    let first_books: Value = serde_json::from_str(
        std::fs::read_to_string(src.join("books.jsonl"))
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let t0: u64 = first_books["body"]["state_time_ms"]
        .as_str()
        .map(|s| s.parse().unwrap())
        .unwrap_or_else(|| first_books["recv_ms"].as_u64().unwrap());
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
    dir
}

/// Inception orders as written in `hedge_orders`: symbol → (side, price, size, tif, builder).
async fn inception_orders(
    env: &Env,
    pending: &str,
) -> BTreeMap<String, (String, i64, i64, String, String)> {
    sqlx::query(
        "SELECT symbol, side, price_u, size, tif, request_json FROM hedge_orders
         WHERE kind = 'inception' AND pending = ?",
    )
    .bind(pending)
    .fetch_all(&env.state.db)
    .await
    .unwrap()
    .iter()
    .map(|r| {
        (
            r.get::<String, _>("symbol"),
            (
                r.get("side"),
                r.get("price_u"),
                r.get("size"),
                r.get("tif"),
                r.get("request_json"),
            ),
        )
    })
    .collect()
}

/// Checks the inception batch: one IOC per hedged leg at the max price (long BID at p, short
/// ASK at 1 − p), Δᵢ contracts, the builder code on each.
async fn check_inception_orders(env: &Env, p: &PendingOpened, q: &Quote) {
    let orders = inception_orders(env, &p.pending).await;
    let hedged: Vec<_> = q
        .legs
        .iter()
        .zip(&p.legs)
        .filter(|(ql, _)| ql.hedged)
        .collect();
    assert_eq!(orders.len(), hedged.len(), "{orders:?}");
    let builder = pubkey(BUILDER_SEED);
    for (ql, pl) in hedged {
        let (side, price, size, tif, json) = &orders[&pl.symbol];
        let (want_side, want_price) = match pl.side {
            Side::Long => ("bid", ql.p_max_u as i64),
            Side::Short => ("ask", 1_000_000 - ql.p_max_u as i64),
        };
        assert_eq!(side, want_side, "{}", pl.symbol);
        assert_eq!(*price, want_price, "{}", pl.symbol);
        assert_eq!(*size, ql.size as i64, "{}", pl.symbol);
        assert_eq!(tif, "ioc");
        let req: Value = serde_json::from_str(json).unwrap();
        assert_eq!(req["builder_info"]["builder_pubkey"], builder.as_str());
        assert_eq!(req["builder_info"]["builder_fee_rate"], "0.001000");
        assert_eq!(req["auth"]["owner"], owner().as_str());
        assert!(
            req["auth"]["signature"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
        );
    }
    let n = env
        .count("SELECT COUNT(*) FROM pascal_fills WHERE venue = 'mock' AND builder_fee_u > 0 AND signed_payload IS NOT NULL")
        .await;
    assert!(
        n >= orders.len() as i64,
        "fills with builder fee and signed payload: {n}"
    );
}

/// `fᵢ` recomputed from the stored inception fills: average on the user's side, rounded up.
async fn f_from_fills(env: &Env, p: &PendingOpened) -> Vec<u64> {
    let mut out = Vec::new();
    for l in &p.legs {
        let rows = sqlx::query(
            "SELECT f.price_u, f.size FROM pascal_fills f JOIN hedge_orders o
               ON o.client_order_id = f.client_order_id
             WHERE o.kind = 'inception' AND o.pending = ? AND f.symbol = ?",
        )
        .bind(&p.pending)
        .bind(&l.symbol)
        .fetch_all(&env.state.db)
        .await
        .unwrap();
        let size: u64 = rows.iter().map(|r| r.get::<i64, _>("size") as u64).sum();
        let notional: u64 = rows
            .iter()
            .map(|r| r.get::<i64, _>("price_u") as u64 * r.get::<i64, _>("size") as u64)
            .sum();
        if size == 0 {
            out.push(0);
            continue;
        }
        let side_notional = pricing::side_notional_u(pside(l.side), size, notional);
        out.push(side_notional.div_ceil(size));
    }
    out
}

// --- the gate ---------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overlapping_three_legs_hedge_all_three_at_inception() {
    let mut env = start(&sample_tape(), 1_000_000 * USD).await;
    let legs = [(PHI, Side::Long), (ORL, Side::Short), (HOU, Side::Long)];
    let (p, q) = env.open_pending(&legs, 1, |_| {}).await;
    assert!(
        q.legs
            .iter()
            .all(|l| l.overlaps && l.hedged && l.m1_ppm == 50_000)
    );
    let hf = env.hedge_filled(&p.pending).await;
    assert_eq!(hf.r_ppm, 1_000_000);
    assert_eq!(hf.f_u.len(), 3);
    assert!(hf.f_u.iter().all(|&f| f > 0));
    assert_eq!(hf.f_u, f_from_fills(&env, &p).await);
    assert!(!hf.fills.is_empty());
    check_inception_orders(&env, &p, &q).await;
    for (ql, (s, side)) in q.legs.iter().zip(legs) {
        let want = sigma(side) * ql.size as i64;
        assert_eq!(env.hedger.held(s), want, "{s}");
        assert_eq!(env.position(s), want, "{s}");
        let alloc = env.hedger.allocations();
        let idx = legs.iter().position(|(x, _)| *x == s).unwrap() as u8;
        assert_eq!(alloc[&(p.ticket.clone(), idx)].1, want, "{s}");
    }
    // The short leg's user-side price is 1 − the ASK fill.
    assert_eq!(hf.f_u[1], 1_000_000 - 371_000);
    assert!(
        env.count("SELECT COUNT(*) FROM hedge_positions WHERE reason = 'inception'")
            .await
            >= 3
    );
    // A redelivered PendingOpened is ignored.
    env.state
        .emit(DeskEvent::PendingOpened(Arc::new(p.clone())));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(inception_orders(&env, &p.pending).await.len(), 3);
}

async fn sequential_hedged(env: &mut Env) -> (PendingOpened, Quote, HedgeFilled) {
    let legs = [(PHI, Side::Long), (ORL, Side::Short), (HOU, Side::Long)];
    let (p, q) = env.open_pending(&legs, 2, |_| {}).await;
    let hf = env.hedge_filled(&p.pending).await;
    (p, q, hf)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sequential_three_legs_hedge_only_the_first() {
    let tape = sequential_tape();
    let mut env = start(&tape.0, 1_000_000 * USD).await;
    let (p, q, hf) = sequential_hedged(&mut env).await;
    assert_eq!(
        q.legs
            .iter()
            .map(|l| (l.hedged, l.overlaps, l.m1_ppm))
            .collect::<Vec<_>>(),
        [
            (true, false, 30_000),
            (false, false, 30_000),
            (false, false, 30_000)
        ]
    );
    assert_eq!(hf.r_ppm, 1_000_000);
    assert!(hf.f_u[0] > 0);
    assert_eq!(
        &hf.f_u[1..],
        &[0, 0],
        "unhedged legs carry 0 (the program uses p_i)"
    );
    check_inception_orders(&env, &p, &q).await;
    assert_eq!(env.hedger.held(PHI), q.legs[0].size as i64);
    assert_eq!(env.position(ORL), 0);
    assert_eq!(env.position(HOU), 0);
    assert_eq!(env.count("SELECT COUNT(*) FROM hedge_orders").await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn won_first_leg_rolls_the_second_to_s_over_p1_p2() {
    let tape = sequential_tape();
    let mut env = start(&tape.0, 1_000_000 * USD).await;
    let (p, _q, hf) = sequential_hedged(&mut env).await;
    let payout = env.mint(&p, &hf).await;
    // Leg 0 is the only leg hedged after mint.
    let held_phi = env.hedger.held(PHI);
    assert!(held_phi > 0);
    assert_eq!(env.hedger.target(ORL), 0);

    // Pascal resolves PHI (long won): the position settles; the chain's resolve_leg follows.
    env.mock
        .mock
        .with_engine(|e| e.schedule_resolution(PHI, 1_000_000, At::Now))
        .unwrap();
    env.state.emit(DeskEvent::LegResolved {
        ticket: p.ticket.clone(),
        user: p.user.clone(),
        leg_idx: 0,
        symbol: PHI.into(),
        resolution_u: 1_000_000,
        status: LegStatus::Won,
        tx: "tx-resolve".into(),
    });

    // Δ₁ = P · q₀ · q₂ with q₀ = 1 (won) and q₂ = p₂ (unhedged, quote price): the roll target.
    let states = [
        pricing::LegState::Resolved {
            r_tilde_ppm: 1_000_000,
        },
        pricing::LegState::Unresolved {
            price_u: p.legs[1].price_u,
        },
        pricing::LegState::Unresolved {
            price_u: p.legs[2].price_u,
        },
    ];
    let micro = pricing::delta_i_micro(payout, &states, 1).unwrap();
    let want = pricing::net_target(&[pricing::TargetEntry {
        hedged: true,
        side: pricing::Side::Short,
        delta_micro: micro,
    }]);
    assert!(want < 0);
    // ≈ S(1 − m)³ / (f₁ p₂): the stake rolled over two legs (S/(p₁p₂) of the spec).
    let approx = STAKE as f64 / 1e6 * 0.97_f64.powi(3)
        / (hf.f_u[0] as f64 / 1e6 * p.legs[1].price_u as f64 / 1e6);
    assert!(
        (want.unsigned_abs() as f64 - approx).abs() <= 1.0,
        "{want} vs {approx}"
    );

    let h = env.hedger.clone();
    eventually("roll filled", || h.held(ORL) == want).await;
    assert_eq!(env.position(ORL), want);
    assert_eq!(env.position(PHI), 0, "settled by the resolution");
    assert_eq!(env.hedger.held(PHI), 0);
    assert_eq!(env.position(HOU), 0, "leg 3 still waits for its roll");
    // Post-only one tick inside the best bid (an ASK: 0.371 + 0.001), then the IOC.
    let post: (i64, i64, String) = sqlx::query_as(
        "SELECT post_only, price_u, tif FROM hedge_orders WHERE kind = 'roll_post' AND symbol = ?",
    )
    .bind(ORL)
    .fetch_one(&env.state.db)
    .await
    .unwrap();
    assert_eq!(post, (1, 372_000, "gtc".to_owned()));
    let ioc = env
        .count("SELECT COUNT(*) FROM hedge_orders WHERE kind = 'roll_ioc' AND tif = 'ioc'")
        .await;
    assert_eq!(ioc, 1);
    let alloc = env.hedger.allocations();
    assert_eq!(alloc[&(p.ticket.clone(), 1)].1, want);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_leg_unwinds_everything() {
    let mut env = start(&sample_tape(), 1_000_000 * USD).await;
    let legs = [(PHI, Side::Long), (ORL, Side::Short), (HOU, Side::Long)];
    let (p, _q) = env.open_pending(&legs, 3, |_| {}).await;
    let hf = env.hedge_filled(&p.pending).await;
    env.mint(&p, &hf).await;
    assert!(env.position(ORL) < 0 && env.position(HOU) > 0);

    env.mock
        .mock
        .with_engine(|e| e.schedule_resolution(PHI, 0, At::Now))
        .unwrap();
    env.state.emit(DeskEvent::LegResolved {
        ticket: p.ticket.clone(),
        user: p.user.clone(),
        leg_idx: 0,
        symbol: PHI.into(),
        resolution_u: 0,
        status: LegStatus::Lost,
        tx: "tx-resolve".into(),
    });
    let ticket = p.ticket.clone();
    let e = env
        .wait(
            "HedgeUnwound",
            |e| matches!(e, DeskEvent::HedgeUnwound(u) if u.ticket == ticket),
        )
        .await;
    let DeskEvent::HedgeUnwound(u) = e else {
        unreachable!()
    };
    let u: &HedgeUnwound = &u;
    assert_eq!(u.reason, "lost_leg");
    assert_eq!(u.pending, None);
    assert!(!u.fills.is_empty());
    for s in [PHI, ORL, HOU] {
        assert_eq!(env.position(s), 0, "{s}");
        assert_eq!(env.hedger.held(s), 0, "{s}");
        assert_eq!(env.hedger.target(s), 0, "{s}");
    }
    let unwinds = env
        .count("SELECT COUNT(*) FROM hedge_orders WHERE kind = 'unwind' AND reduce_only = 1 AND tif = 'ioc'")
        .await;
    assert_eq!(unwinds, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_fill_gives_r_below_one_and_a_consistent_refund() {
    let mut env = start(&sample_tape(), 1_000_000 * USD).await;
    let legs = [(PHI, Side::Long), (HOU, Side::Long)];
    // Someone else takes PHI's asks up to the max price, leaving half of Δ₀, between the
    // desk's re-quote and its IOC.
    let q = env.quote(&legs, STAKE).await;
    let (d0, pm0) = (q.legs[0].size, q.legs[0].p_max_u as i64);
    let book = env.fake.books(&[PHI.to_owned()]).await.unwrap().data.books[PHI].clone();
    let depth: u64 = book
        .asks
        .iter()
        .filter(|l| l.price <= pm0)
        .map(|l| l.size)
        .sum();
    let leave = d0 / 2;
    let other = Trader::new(
        TradingSigner::from_seed(&OTHER_SEED),
        &pubkey(OTHER_SEED),
        &pubkey(BUILDER_SEED),
        BUILDER_FEE_RATE_U,
        desk::api::types::Venue::Mock,
    )
    .unwrap();
    let front = other
        .sign_order(
            &OrderSpec {
                symbol: PHI.into(),
                side: pascal_rs::Side::Bid,
                price_u: pm0,
                size: depth - leave,
                tif: TimeInForce::Ioc,
                post_only: false,
                reduce_only: false,
                kind: "test",
                reason: "front-run".into(),
                pending: None,
                serves: vec![],
            },
            1,
        )
        .unwrap();
    env.fake
        .front_run
        .lock()
        .unwrap()
        .push(front.signed.request);

    let (p, q2) = env.open_pending(&legs, 4, |_| {}).await;
    assert_eq!(q2.legs[0].size, d0);
    let hf = env.hedge_filled(&p.pending).await;
    let want_r = (leave * 1_000_000 / d0) as u32;
    assert_eq!(hf.r_ppm, want_r);
    assert!(hf.r_ppm > 0 && hf.r_ppm < 1_000_000);
    assert_eq!(hf.f_u, f_from_fills(&env, &p).await);

    // The program's finalize on these numbers: floor holds, refund = S − rS.
    let pl: Vec<pricing::PayoutLeg> = p
        .legs
        .iter()
        .zip(&hf.f_u)
        .map(|(l, &f)| pricing::PayoutLeg {
            m1_ppm: l.m1_ppm,
            f_u: f,
        })
        .collect();
    let pay = pricing::payout(STAKE, hf.r_ppm, &pl).unwrap();
    assert!(pricing::floor_ok(
        pay.payout_u,
        pay.rs_u,
        p.min_payout_per_unit_u
    ));
    assert_eq!(
        pay.refund_u,
        STAKE - STAKE * u64::from(hf.r_ppm) / 1_000_000
    );
    assert!(pay.refund_u > 0);

    // Held = ⌈r·Δᵢ⌉ per leg: PHI kept its partial fill, HOU's excess was unwound reduce-only.
    let keep = |d: u64| (d * u64::from(hf.r_ppm)).div_ceil(1_000_000) as i64;
    assert_eq!(env.hedger.held(PHI), leave as i64);
    assert_eq!(env.position(PHI), leave as i64);
    assert_eq!(keep(d0), leave as i64);
    let d1 = q2.legs[1].size;
    assert_eq!(env.hedger.held(HOU), keep(d1));
    assert_eq!(env.position(HOU), keep(d1));
    let excess = env
        .count("SELECT COUNT(*) FROM hedge_orders WHERE kind = 'excess' AND reduce_only = 1 AND symbol = 'NBA_26OCT28_HOUMIL_ML.HOU'")
        .await;
    assert_eq!(excess, 1);
    let alloc = env.hedger.allocations();
    assert_eq!(alloc[&(p.ticket.clone(), 1)].1, keep(d1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_mid_ticket_recovers_held_and_targets_from_fills() {
    let mut env = start(&sample_tape(), 1_000_000 * USD).await;
    let legs = [(PHI, Side::Long), (ORL, Side::Short), (HOU, Side::Long)];
    let (p, _q) = env.open_pending(&legs, 5, |_| {}).await;
    let hf = env.hedge_filled(&p.pending).await;
    env.mint(&p, &hf).await;
    let symbols = [PHI, ORL, HOU];
    let held: Vec<i64> = symbols.iter().map(|s| env.hedger.held(s)).collect();
    let targets: Vec<i64> = symbols.iter().map(|s| env.hedger.target(s)).collect();
    let tickets = env.hedger.tickets();
    let alloc = env.hedger.allocations();
    let fills = env.count("SELECT COUNT(*) FROM pascal_fills").await;
    let orders = env.count("SELECT COUNT(*) FROM hedge_orders").await;
    assert!(held.iter().all(|&h| h != 0));

    // Crash, losing the fill rows (hedge_orders were written ahead of the send) and the cursor.
    let (keys, db, mock) = env.stop().await;
    {
        let pool = desk::db::open(&db.path()).await.unwrap();
        for t in ["fill_allocations", "pascal_fills", "cursors"] {
            sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM {t}")))
                .execute(&pool)
                .await
                .unwrap();
        }
        pool.close().await;
    }
    // Restart with /accounts down: held must come from /fills.
    let env = with_parts(keys, db, mock, true, false).await;
    let h = env.hedger.clone();
    eventually("tickets rebuilt", || !h.tickets().is_empty()).await;
    let want = held.clone();
    eventually("held recovered", || {
        symbols.iter().map(|s| h.held(s)).collect::<Vec<_>>() == want
    })
    .await;
    assert_eq!(
        symbols
            .iter()
            .map(|s| env.hedger.target(s))
            .collect::<Vec<_>>(),
        targets
    );
    let mut after = env.hedger.tickets();
    let mut before = tickets;
    after.sort_by(|a, b| a.ticket.cmp(&b.ticket));
    before.sort_by(|a, b| a.ticket.cmp(&b.ticket));
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].status, TicketStatus::Open);
    assert_eq!(
        after[0].legs.iter().map(|l| l.target).collect::<Vec<_>>(),
        before[0].legs.iter().map(|l| l.target).collect::<Vec<_>>()
    );
    assert_eq!(env.hedger.allocations(), alloc);
    assert_eq!(env.count("SELECT COUNT(*) FROM pascal_fills").await, fills);
    assert!(
        env.count("SELECT value FROM cursors WHERE name = 'hedge.fills_seq'")
            .await
            > 0
    );
    // Nothing to trade: held equals the targets.
    assert_eq!(env.count("SELECT COUNT(*) FROM hedge_orders").await, orders);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finalize_rejected_unwinds_then_asks_for_the_refund() {
    let mut env = start(&sample_tape(), 1_000_000 * USD).await;
    let (p, _q) = env.open_pending(&[(PHI, Side::Long)], 6, |_| {}).await;
    env.hedge_filled(&p.pending).await;
    assert!(env.position(PHI) > 0);
    env.state.emit(DeskEvent::FinalizeRejected {
        pending: p.pending.clone(),
        user: p.user.clone(),
        error: "FloorBreached".into(),
    });
    let pending = p.pending.clone();
    let e = env
        .wait("HedgeUnwound or RefundNeeded", |e| {
            matches!(e, DeskEvent::HedgeUnwound(u) if u.pending.as_deref() == Some(pending.as_str()))
                || matches!(e, DeskEvent::RefundNeeded { pending: x, .. } if *x == pending)
        })
        .await;
    assert!(
        matches!(e, DeskEvent::HedgeUnwound(_)),
        "unwound before the refund: {e:?}"
    );
    env.wait(
        "RefundNeeded",
        |e| matches!(e, DeskEvent::RefundNeeded { pending: x, .. } if *x == pending),
    )
    .await;
    assert_eq!(env.position(PHI), 0);
    assert_eq!(env.hedger.held(PHI), 0);
    assert!(env.hedger.tickets().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_hedge_when_the_floor_or_the_collateral_fails() {
    // The fresh quote misses the user's floor: nothing is sent, nothing is emitted.
    let mut env = start(&sample_tape(), 1_000_000 * USD).await;
    let (p, _) = env
        .open_pending(&[(PHI, Side::Long)], 7, |p| {
            p.min_payout_per_unit_u = p.odds_offered_u * 2;
        })
        .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(env.count("SELECT COUNT(*) FROM hedge_orders").await, 0);
    while let Ok(e) = env.rx.try_recv() {
        assert!(
            !matches!(
                e,
                DeskEvent::HedgeFilled(_) | DeskEvent::RefundNeeded { .. }
            ),
            "{e:?}"
        );
    }
    assert!(env.hedger.tickets().is_empty(), "{}", p.pending);

    // The hedger's free collateral (1 USD) does not cover the hedge.
    let mut env = start(&sample_tape(), USD).await;
    env.open_pending(&[(PHI, Side::Long)], 8, |_| {}).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(env.count("SELECT COUNT(*) FROM hedge_orders").await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flatten_cancels_and_closes_everything() {
    let mut env = start(&sample_tape(), 1_000_000 * USD).await;
    let legs = [(PHI, Side::Long), (ORL, Side::Short)];
    let (p, _q) = env.open_pending(&legs, 9, |_| {}).await;
    env.hedge_filled(&p.pending).await;
    let trader = env.hedger.trader.clone().unwrap();
    // A resting order for the mass cancel to find.
    let rest = trader
        .sign_order(
            &OrderSpec {
                symbol: SAS.into(),
                side: pascal_rs::Side::Bid,
                price_u: 500_000,
                size: 3,
                tif: TimeInForce::Gtc,
                post_only: true,
                reduce_only: false,
                kind: "test",
                reason: "resting".into(),
                pending: None,
                serves: vec![],
            },
            env.state.next_client_order_id(),
        )
        .unwrap();
    env.fake.place_orders(&[rest.signed.request]).await.unwrap()[0]
        .as_ref()
        .expect("resting order placed");
    let report = desk::hedge::flatten::flatten(&env.state.pascal, &trader, Some(&env.state.db))
        .await
        .unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.orders_cancelled, 1);
    assert_eq!(report.positions.len(), 2);
    assert!(report.left.is_empty(), "{:?}", report.left);
    for s in [PHI, ORL, SAS] {
        assert_eq!(env.position(s), 0, "{s}");
    }
    let o = owner();
    let open = env
        .mock
        .mock
        .with_engine(|e| e.account(&o, None, false).orders.len());
    assert_eq!(open, 0);
    assert_eq!(
        env.count("SELECT COUNT(*) FROM hedge_orders WHERE kind = 'flatten' AND reduce_only = 1")
            .await,
        2
    );
}

/// Joint check with the 2.D mock: permits verified, and the restart replays the mock's own
/// `GET /accounts/{hedger}/fills` and reads `/accounts` over HTTP (no fake in the desk's path).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_against_the_real_mock_with_verified_permits() {
    let mut env = start_with(&sample_tape(), 1_000_000 * USD, true).await;
    let legs = [(PHI, Side::Long), (ORL, Side::Short)];
    let (p, _q) = env.open_pending(&legs, 10, |_| {}).await;
    let hf = env.hedge_filled(&p.pending).await;
    assert_eq!(
        hf.r_ppm, 1_000_000,
        "the verifying mock accepted and filled the signed batch"
    );
    env.mint(&p, &hf).await;
    let held: Vec<i64> = [PHI, ORL].iter().map(|s| env.hedger.held(s)).collect();
    let alloc = env.hedger.allocations();
    let fills = env.count("SELECT COUNT(*) FROM pascal_fills").await;
    let (keys, db, mock) = env.stop().await;
    {
        let pool = desk::db::open(&db.path()).await.unwrap();
        for t in ["fill_allocations", "pascal_fills", "cursors"] {
            sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM {t}")))
                .execute(&pool)
                .await
                .unwrap();
        }
        pool.close().await;
    }
    let env = with_parts(keys, db, mock, false, true).await;
    let h = env.hedger.clone();
    let want = held.clone();
    eventually("held recovered over HTTP", || {
        !h.tickets().is_empty() && [PHI, ORL].iter().map(|s| h.held(s)).collect::<Vec<_>>() == want
    })
    .await;
    eventually("allocations replayed", || h.allocations() == alloc).await;
    assert_eq!(env.count("SELECT COUNT(*) FROM pascal_fills").await, fills);
    // A bad signature is refused by the verifying mock.
    let trader = env.hedger.trader.clone().unwrap();
    let mut bad = trader
        .sign_order(
            &OrderSpec {
                symbol: SAS.into(),
                side: pascal_rs::Side::Bid,
                price_u: 500_000,
                size: 1,
                tif: TimeInForce::Ioc,
                post_only: false,
                reduce_only: false,
                kind: "test",
                reason: "bad signature".into(),
                pending: None,
                serves: vec![],
            },
            env.state.next_client_order_id(),
        )
        .unwrap()
        .signed
        .request;
    bad.size += 1;
    let r = env.state.pascal.place_orders(&[bad]).await;
    let refused = match r {
        Ok(v) => v[0]
            .as_ref()
            .err()
            .and_then(|e| e.api_code().map(str::to_owned)),
        Err(e) => e.api_code().map(str::to_owned),
    };
    assert_eq!(refused.as_deref(), Some("UNAUTHORIZED"));
}
