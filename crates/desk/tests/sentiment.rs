//! Outcome sentiment (5.0c, L5-4) on the real trades of the 28 Oct NBA tape
//! (`fixtures/sentiment/nba-2026-10-28.json`: 16 distinct trades on five games, 8 accounts, one
//! of them the maker in 15).
//!
//! Hand count of every account's position after its latest trade (`prev` + the trade; a taker
//! `BID` buys the symbol, i.e. backs the away team, D-021):
//!
//! | Game | Away (long) | Home (short) | Market maker `7ATd…` |
//! | --- | --- | --- | --- |
//! | PHI @ IND (7 trades) | `CMJH…` +152, `AuYn…` +11, `EKnc…` +5 | `5BoT…` −30, `21iP…` −36 | −102 |
//! | OKC @ DAL (3) | `HydD…` +20 (its one resting order) | `21iP…` −30 | +10 |
//! | SAS @ UTA (3) | `21iP…` +39 | `CMJH…` −32, `8jq8…` −21 | +14 |
//! | HOU @ MIL (2) | `EKnc…` +5 | `CMJH…` −32 | +27 |
//! | ORL @ TOR (1) | — | `EKnc…` −5 | +5 |
//!
//! One `#[ignore]`d smoke test reads today's real trades for the live NBA card:
//! `cargo test -p desk --test sentiment live_sentiment_smoke -- --ignored --nocapture`.

mod common;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::routing::get;
use common::{TempDb, repo_root};
use desk::Config;
use desk::api::types::TeamSide;
use desk::catalogue::{CatalogueMarket, nba_moneyline};
use desk::sentiment::follow::{Follower, TradeSource};
use desk::sentiment::tape::{MakerRule, TradeTape};
use desk::sentiment::{
    self, AwayHome, SchedinaStake, SentimentResponse, Traded, flow_u, game_sentiment, long_mid_u,
};
use desk::state::{AppState, Locked};
use pascal_rs::{Market, MarketStats, Page, PublicTrade, Response, Side};
use serde_json::{Value, json};

const PHI: &str = "NBA_26OCT28_PHIIND_ML.PHI";
const OKC: &str = "NBA_26OCT28_OKCDAL_ML.OKC";
const SAS: &str = "NBA_26OCT28_SASUTA_ML.SAS";
const HOU: &str = "NBA_26OCT28_HOUMIL_ML.HOU";
const ORL: &str = "NBA_26OCT28_ORLTOR_ML.ORL";
const MAKER: &str = "7ATdPM8uGMu3qiRSCpc6hUrSZ5VxFSUyYLfRipABG5bC";
const U: u64 = 1_000_000;

fn fixture() -> Value {
    let p = repo_root().join("fixtures/sentiment/nba-2026-10-28.json");
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// The tape's trades, newest first.
fn trades() -> Vec<PublicTrade> {
    serde_json::from_value(fixture()["trades"]["data"]["items"].clone()).unwrap()
}

fn raw_markets() -> Vec<Value> {
    fixture()["markets"]["data"].as_array().unwrap().clone()
}

fn markets() -> HashMap<String, (CatalogueMarket, MarketStats)> {
    raw_markets()
        .into_iter()
        .map(|v| {
            let m: Market = serde_json::from_value(v).unwrap();
            let cm = nba_moneyline(&m).expect("an NBA moneyline");
            (m.symbol.clone(), (cm, m.stats.expect("stats")))
        })
        .collect()
}

fn tape_of(trades: &[PublicTrade]) -> TradeTape {
    let mut t = TradeTape::default();
    for tr in trades {
        t.ingest(tr);
    }
    let symbols: Vec<String> = [PHI, OKC, SAS, HOU, ORL].map(String::from).to_vec();
    t.set_synced(&symbols, true);
    t
}

fn none() -> HashSet<String> {
    HashSet::new()
}

#[test]
fn the_market_maker_is_excluded_and_positions_match_the_hand_count() {
    let trades = trades();
    assert_eq!(trades.len(), 16);
    let owners: HashSet<&str> = trades
        .iter()
        .flat_map(|t| [t.maker.as_str(), t.taker.as_str()])
        .collect();
    assert_eq!(owners.len(), 8);
    assert_eq!(trades.iter().filter(|t| t.maker == MAKER).count(), 15);

    let tape = tape_of(&trades);
    let rule = MakerRule::default();
    let card = tape.card_makers(&rule);
    assert_eq!(card, HashSet::from([MAKER.to_owned()]));

    // (away accounts, away contracts, home accounts, home contracts, trades)
    for (symbol, want) in [
        (PHI, (3, 168, 2, 66, 7)),
        (OKC, (1, 20, 1, 30, 3)),
        (SAS, (1, 39, 2, 53, 3)),
        (HOU, (1, 5, 1, 32, 2)),
        (ORL, (0, 0, 1, 5, 1)),
    ] {
        let h = tape.holders(symbol, &rule, &card, &none());
        assert_eq!(
            (
                h.long.accounts,
                h.long.contracts,
                h.short.accounts,
                h.short.contracts,
                h.trades_seen
            ),
            want,
            "{symbol}"
        );
        assert_eq!(h.excluded_makers, 1, "{symbol}");
        // Positions net to zero over every account, the maker included.
        let m = &tape.markets[symbol];
        assert_eq!(m.accounts.values().map(|a| a.position).sum::<i64>(), 0);
    }
    let maker_pos: Vec<i64> = [PHI, OKC, SAS, HOU, ORL]
        .iter()
        .map(|s| tape.markets[*s].accounts[MAKER].position)
        .collect();
    assert_eq!(maker_pos, [-102, 10, 14, 27, 5]);

    // The desk's own hedger is never a holder (here: pretend `CMJH…` is it).
    let cmjh = trades
        .iter()
        .find(|t| t.taker.starts_with("CMJH"))
        .unwrap()
        .taker
        .clone();
    let h = tape.holders(PHI, &rule, &card, &HashSet::from([cmjh]));
    assert_eq!((h.long.accounts, h.long.contracts), (2, 16));
}

#[test]
fn the_rule_needs_a_sample_and_the_literal_rule_would_drop_a_one_order_person() {
    let tape = tape_of(&trades());
    // L5-4's shares without minimum samples or the card-wide rule: OKC's one resting order
    // (`HydD…`, maker in 1 of 1) counts as a market maker, and the maker itself is caught only
    // where it made most of the market.
    let literal = MakerRule {
        own_min: 1,
        market_min: 1,
        card_min: u64::MAX,
        ..MakerRule::default()
    };
    let card = tape.card_makers(&literal);
    assert!(card.is_empty());
    let okc = tape.holders(OKC, &literal, &card, &none());
    assert_eq!(okc.excluded_makers, 2);
    assert_eq!(okc.long.accounts, 0);
    // D-251's rule keeps that person and still drops the maker everywhere.
    let rule = MakerRule::default();
    let card = tape.card_makers(&rule);
    let okc = tape.holders(OKC, &rule, &card, &none());
    assert_eq!((okc.excluded_makers, okc.long.accounts), (1, 1));
}

#[test]
fn a_trade_without_prev_leaves_that_account_out() {
    // The mock's own fills against tape liquidity carry no maker `prev` (synthetic owner), and
    // archival trades none at all: such an account's position is unknown, never a holder.
    let mut trades = trades();
    let last_phi = trades.iter_mut().find(|t| t.symbol == PHI).unwrap();
    last_phi.taker_position_size_prev = None;
    let taker = last_phi.taker.clone();
    let tape = tape_of(&trades);
    assert!(tape.markets[PHI].accounts[&taker].unknown);
    let rule = MakerRule::default();
    let h = tape.holders(PHI, &rule, &tape.card_makers(&rule), &none());
    // `EKnc…` (+5, the latest PHI trade) drops out of the away holders.
    assert_eq!((h.long.accounts, h.long.contracts), (2, 163));
}

#[test]
fn order_and_duplicates_do_not_matter() {
    let trades = trades();
    let base = tape_of(&trades);
    let mut rev = trades.clone();
    rev.reverse();
    let mut twice = rev.clone();
    twice.extend(trades.iter().cloned());
    let mut shuffled = trades.clone();
    shuffled.rotate_left(7);
    shuffled.swap(0, 11);
    for order in [rev, twice, shuffled] {
        let mut t = TradeTape::default();
        let new = order.iter().filter(|tr| t.ingest(tr)).count();
        assert_eq!(new, 16);
        for s in [PHI, OKC, SAS, HOU, ORL] {
            assert_eq!(t.markets[s].accounts, base.markets[s].accounts, "{s}");
        }
    }
}

#[test]
fn flow_maps_to_teams_and_the_thin_flag_is_set() {
    let ms = markets();
    // PHI before tip-off: takers bought 163 PHI for $114.047 (toward the 76ers, the away team)
    // and sold 15 at $10.515 (toward the Pacers, who paid 15 − 10.515).
    let (phi, phi_stats) = &ms[PHI];
    assert_eq!(phi.long_team, TeamSide::Away);
    let s = &phi_stats.last_24h;
    assert_eq!((s.taker_buy_volume, s.taker_sell_volume), (163, 15));
    assert_eq!(flow_u(s), (114_047_000, 4_485_000));
    // HOU: only sells, all toward the Bucks.
    assert_eq!(flow_u(&ms[HOU].1.last_24h), (0, 32 * U - 24_320_000));

    let tape = tape_of(&trades());
    let rule = MakerRule::default();
    let card = tape.card_makers(&rule);
    let mid = long_mid_u(phi, None).unwrap();
    assert_eq!(mid, 695_000);
    let g = game_sentiment(
        phi,
        Some(mid),
        &tape.holders(PHI, &rule, &card, &none()),
        Some(s),
        SchedinaStake::default(),
    );
    assert_eq!(
        g.price,
        AwayHome {
            away_u: 695_000,
            home_u: 305_000
        }
    );
    assert_eq!(
        (
            g.pascal_holders.away.accounts,
            g.pascal_holders.away.stake_u
        ),
        (3, 168 * 695_000)
    );
    assert_eq!(
        (
            g.pascal_holders.home.accounts,
            g.pascal_holders.home.stake_u
        ),
        (2, 66 * 305_000)
    );
    assert_eq!(
        g.flow_24h,
        AwayHome {
            away_u: 114_047_000,
            home_u: 4_485_000
        }
    );
    assert!(!g.pascal_holders.thin);

    // Two or fewer people: too few traders yet. SAS has exactly three.
    for (symbol, thin) in [(OKC, true), (HOU, true), (ORL, true), (SAS, false)] {
        let (m, st) = &ms[symbol];
        let g = game_sentiment(
            m,
            long_mid_u(m, None),
            &tape.holders(symbol, &rule, &card, &none()),
            Some(&st.last_24h),
            SchedinaStake::default(),
        );
        assert_eq!(g.pascal_holders.thin, thin, "{symbol}");
    }
    // Not read completely yet: thin too, whatever the count.
    let mut unsynced = tape.clone();
    unsynced.set_synced(&[PHI.to_owned()], false);
    let g = game_sentiment(
        phi,
        Some(mid),
        &unsynced.holders(PHI, &rule, &card, &none()),
        None,
        SchedinaStake::default(),
    );
    assert!(g.pascal_holders.thin && !g.pascal_holders.synced);
}

/// 6.0c (L6-2): `traded_all_time` is `stats.all_time` mapped to teams exactly as `flow_24h`.
#[test]
fn traded_all_time_maps_to_teams_on_the_28_oct_fixture() {
    let ms = markets();
    // PHI over its life: takers bought 163 for $114.047 (toward the 76ers, away and long) and sold
    // 30 for $19.83 (toward the Pacers, who paid 30 − 19.83 = 10.17). Total 124.217.
    let (phi, st) = &ms[PHI];
    let a = &st.all_time;
    assert_eq!((a.taker_buy_volume, a.taker_sell_volume), (163, 30));
    assert_eq!(
        sentiment::traded_u(phi.long_team, a),
        Traded {
            away_u: 114_047_000,
            home_u: 10_170_000,
            total_u: 124_217_000
        }
    );
    // Sells only: all toward the home team. SAS 53 sold for $45.05; OKC 20 for $13; HOU 32 for
    // $24.32; ORL never traded.
    for (symbol, home_u) in [
        (SAS, 53 * U - 45_050_000),
        (OKC, 20 * U - 13_000_000),
        (HOU, 32 * U - 24_320_000),
        (ORL, 0),
    ] {
        let (m, st) = &ms[symbol];
        assert_eq!(
            sentiment::traded_u(m.long_team, &st.all_time),
            Traded {
                away_u: 0,
                home_u,
                total_u: home_u
            },
            "{symbol}"
        );
    }
    // A market long on the home team maps the other way.
    let t = sentiment::traded_u(TeamSide::Home, a);
    assert_eq!((t.away_u, t.home_u), (10_170_000, 114_047_000));
}

#[test]
fn a_market_long_on_the_home_team_maps_the_other_way() {
    let ms = markets();
    let mut m = ms[PHI].0.clone();
    m.long_team = TeamSide::Home;
    let tape = tape_of(&trades());
    let rule = MakerRule::default();
    let card = tape.card_makers(&rule);
    let g = game_sentiment(
        &m,
        Some(695_000),
        &tape.holders(PHI, &rule, &card, &none()),
        Some(&ms[PHI].1.last_24h),
        SchedinaStake::default(),
    );
    assert_eq!(g.price.home_u, 695_000);
    assert_eq!(g.pascal_holders.home.accounts, 3);
    assert_eq!(g.flow_24h.home_u, 114_047_000);
}

#[test]
fn a_taker_buy_on_the_away_symbol_is_flow_toward_the_away_team() {
    let mut s = markets()[PHI].1.last_24h.clone();
    s.taker_buy_volume = 10;
    s.taker_buy_volume_notional = 6_000_000;
    s.taker_sell_volume = 0;
    s.taker_sell_volume_notional = 0;
    assert_eq!(flow_u(&s), (6_000_000, 0));
    s.taker_buy_volume = 0;
    s.taker_buy_volume_notional = 0;
    s.taker_sell_volume = 10;
    s.taker_sell_volume_notional = 6_000_000;
    assert_eq!(flow_u(&s), (0, 4_000_000));
}

// ---------- the follower over a paged source ----------

/// Pages `trades` (newest first) like Pascal: `limit` items before the cursor `seq:trade_id`.
struct Paged {
    trades: Mutex<Vec<PublicTrade>>,
    calls: AtomicUsize,
    fail_next: Mutex<bool>,
}

impl Paged {
    fn new(trades: Vec<PublicTrade>) -> Self {
        Paged {
            trades: Mutex::new(trades),
            calls: AtomicUsize::new(0),
            fail_next: Mutex::new(false),
        }
    }

    fn page_of(
        all: &[PublicTrade],
        symbols: &[String],
        limit: u32,
        before: Option<&str>,
    ) -> (Vec<PublicTrade>, Option<String>) {
        let before = before.map(|c| {
            let (s, i) = c.split_once(':').unwrap();
            (s.parse::<u64>().unwrap(), i.parse::<u64>().unwrap())
        });
        let mut items: Vec<PublicTrade> = all
            .iter()
            .filter(|t| symbols.contains(&t.symbol))
            .filter(|t| before.is_none_or(|b| (t.seq, t.trade_id) < b))
            .cloned()
            .collect();
        items.sort_by_key(|t| std::cmp::Reverse((t.seq, t.trade_id)));
        let more = items.len() > limit as usize;
        items.truncate(limit as usize);
        let next = more
            .then(|| items.last().map(|t| format!("{}:{}", t.seq, t.trade_id)))
            .flatten();
        (items, next)
    }
}

#[async_trait]
impl TradeSource for Paged {
    async fn page(
        &self,
        symbols: &[String],
        limit: u32,
        before: Option<&str>,
    ) -> pascal_rs::Result<Response<Page<PublicTrade>>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if std::mem::take(&mut *self.fail_next.lock().unwrap()) {
            return Err(pascal_rs::Error::InvalidArgument("injected".into()));
        }
        let all = self.trades.lock().unwrap().clone();
        let (items, next) = Self::page_of(&all, symbols, limit, before);
        let seq = all.iter().map(|t| t.seq).max();
        let data: Page<PublicTrade> = serde_json::from_value(json!({
            "items": items,
            "next_cursor": next,
        }))
        .unwrap();
        Ok(Response {
            data,
            seq,
            state_time_ms: None,
            server_time_ms: None,
            raw: Value::Null,
        })
    }
}

fn card_symbols() -> Vec<String> {
    [PHI, OKC, SAS, HOU, ORL].map(String::from).to_vec()
}

#[tokio::test]
async fn the_follower_pages_backfills_and_then_reads_only_what_is_new() {
    let src = Paged::new(trades());
    let tape = Locked::new(TradeTape::default());
    let mut f = Follower::new();
    f.limit = 5;
    f.budget = 2;
    let symbols = card_symbols();

    // 16 trades in pages of 5, two pages per pass: a backfill over two passes.
    let s = f.pass(&src, &symbols, &tape).await.unwrap();
    assert_eq!((s.pages, s.new_trades, s.backfilling), (2, 10, 1));
    assert!(!tape.read().markets[PHI].synced);
    // A failed page keeps the cursor: nothing is read twice or skipped.
    *src.fail_next.lock().unwrap() = true;
    assert!(f.pass(&src, &symbols, &tape).await.is_err());
    assert!(f.backfilling());
    let s = f.pass(&src, &symbols, &tape).await.unwrap();
    assert_eq!((s.pages, s.new_trades, s.backfilling), (2, 6, 0));
    let top = trades()[0].seq;
    assert_eq!(f.synced(PHI), Some(top));
    assert!(tape.read().markets.values().all(|m| m.synced));
    assert_eq!(*tape.read(), tape_of(&trades()));

    // Steady state: one page, nothing new.
    let s = f.pass(&src, &symbols, &tape).await.unwrap();
    assert_eq!((s.pages, s.new_trades), (1, 0));

    // A new trade: one page, ingested; `EKnc…` backs the Pacers more.
    let mut t = trades()[0].clone();
    assert_eq!(t.symbol, SAS);
    t.symbol = PHI.to_owned();
    t.trade_id = 999_999;
    t.seq = top + 1_000;
    t.taker = trades()
        .iter()
        .find(|x| x.taker.starts_with("EKnc"))
        .unwrap()
        .taker
        .clone();
    t.taker_side = Side::Ask;
    t.size = 20;
    t.taker_position_size_prev = Some(5);
    t.maker = MAKER.to_owned();
    t.maker_position_size_prev = Some(-102);
    src.trades.lock().unwrap().insert(0, t.clone());
    let s = f.pass(&src, &symbols, &tape).await.unwrap();
    assert_eq!((s.pages, s.new_trades), (1, 1));
    assert_eq!(tape.read().markets[PHI].accounts[&t.taker].position, -15);
    assert_eq!(f.synced(PHI), Some(top + 1_000));

    // A new symbol on the card reads the chunk's history again (bounded by the budget).
    let calls = src.calls.load(Ordering::SeqCst);
    let mut more = symbols.clone();
    more.push("NBA_26OCT29_AAABBB_ML.AAA".to_owned());
    let s = f.pass(&src, &more, &tape).await.unwrap();
    assert_eq!(s.pages, 2);
    assert_eq!(src.calls.load(Ordering::SeqCst), calls + 2);
    assert_eq!(s.new_trades, 0);
}

// ---------- the route, end to end over HTTP ----------

type Stub = Arc<StubData>;

struct StubData {
    markets: Value,
    trades: Vec<PublicTrade>,
    log: Mutex<Vec<String>>,
}

async fn stub_markets(State(s): State<Stub>) -> axum::response::Response {
    s.log.lock().unwrap().push("GET /api/v1/markets".into());
    axum::Json(s.markets.clone()).into_response()
}

async fn stub_trades(
    State(s): State<Stub>,
    Query(q): Query<HashMap<String, String>>,
) -> axum::response::Response {
    s.log.lock().unwrap().push("GET /api/v1/trades".into());
    let symbols: Vec<String> = q["symbols"].split(',').map(str::to_owned).collect();
    let limit: u32 = q.get("limit").map_or(100, |l| l.parse().unwrap());
    let (items, next) = Paged::page_of(
        &s.trades,
        &symbols,
        limit,
        q.get("before_cursor").map(String::as_str),
    );
    let mut data = json!({ "items": items });
    if let Some(n) = next {
        data["next_cursor"] = n.into();
    }
    axum::Json(json!({"status":"success","data":data,"seq":"12276030078",
        "state_time_ms":"1793249437398","server_time_ms":"1793249437398"}))
    .into_response()
}

async fn stub() -> (String, Stub) {
    let data = Arc::new(StubData {
        markets: json!({"status":"success","data":raw_markets(),"seq":"12237329576"}),
        trades: trades(),
        log: Mutex::new(Vec::new()),
    });
    let app = Router::new()
        .route("/api/v1/markets", get(stub_markets))
        .route("/api/v1/trades", get(stub_trades))
        .with_state(data.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    (format!("http://{addr}"), data)
}

async fn seed_ticket(
    db: &sqlx::SqlitePool,
    id: &str,
    state: &str,
    stake_u: u64,
    rs_u: Option<u64>,
    legs: &[(&str, &str, &str)],
) {
    static NONCE: AtomicUsize = AtomicUsize::new(1);
    sqlx::query(
        "INSERT INTO tickets (ticket, pending, user, nonce, state, n, stake_u, rs_u, payout_u,
            odds_offered_u, min_payout_per_unit_u, updated_ms)
         VALUES (?, ?, 'user', ?, ?, ?, ?, ?, 0, 0, 0, 0)",
    )
    .bind(id)
    .bind(format!("pending-{id}"))
    .bind(NONCE.fetch_add(1, Ordering::Relaxed) as i64)
    .bind(state)
    .bind(legs.len() as i64)
    .bind(stake_u as i64)
    .bind(rs_u.map(|r| r as i64))
    .execute(db)
    .await
    .unwrap();
    for (idx, (symbol, pick, status)) in legs.iter().enumerate() {
        let side = if *pick == "away" { "long" } else { "short" };
        sqlx::query(
            "INSERT INTO ticket_legs (ticket, idx, symbol, game_id, side, pick, start_ms,
                p_quote_u, m1_ppm, status)
             VALUES (?, ?, ?, ?, ?, ?, 0, 500000, 30000, ?)",
        )
        .bind(id)
        .bind(idx as i64)
        .bind(*symbol)
        .bind(format!("game-{symbol}"))
        .bind(side)
        .bind(*pick)
        .bind(*status)
        .execute(db)
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn get_sentiment_answers_the_l5_4_shape_from_trades_markets_and_tickets() {
    let (url, stub) = stub().await;
    let db = TempDb::new("sentiment");
    let config = Config::local(&url, db.path());
    let pool = desk::db::open(&db.path()).await.unwrap();
    let pascal = desk::state::pascal_client(&config).unwrap();
    let state = AppState::new(config.clone(), pascal, pool.clone());

    // Before the catalogue: 503.
    let app = desk::api::router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    let base = format!("http://{addr}");
    let r = reqwest::get(format!("{base}/sentiment")).await.unwrap();
    assert_eq!(r.status(), 503);

    desk::catalogue::refresh(&state).await.unwrap();
    seed_ticket(
        &pool,
        "t1",
        "open",
        10 * U,
        Some(9 * U),
        &[(PHI, "away", "open"), (OKC, "home", "open")],
    )
    .await;
    seed_ticket(
        &pool,
        "t2",
        "pending",
        5 * U,
        None,
        &[(PHI, "home", "open")],
    )
    .await;
    seed_ticket(
        &pool,
        "t3",
        "settled",
        50 * U,
        Some(50 * U),
        &[(PHI, "away", "won")],
    )
    .await;
    seed_ticket(
        &pool,
        "t4",
        "open",
        7 * U,
        Some(7 * U),
        &[(PHI, "away", "won"), (SAS, "away", "open")],
    )
    .await;
    // 7.0c: a refunded ticket counts nowhere.
    seed_ticket(
        &pool,
        "t5",
        "refunded",
        3 * U,
        None,
        &[(PHI, "away", "open")],
    )
    .await;
    // 7.0c (L7-1): owners and opening times, interleaved with PHI's Pascal trades.
    for (id, user, opened_at) in [
        ("t1", "alice", 1_793_212_000_i64),
        ("t2", "bob", 1_793_220_000),
        ("t3", "alice", 1_793_100_000),
        ("t4", "carol", 1_793_236_000),
        ("t5", "dave", 1_793_237_000),
    ] {
        sqlx::query("UPDATE tickets SET user = ?, opened_at = ? WHERE ticket = ?")
            .bind(user)
            .bind(opened_at)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }

    let src = Arc::new(sentiment::trades_client(&config).unwrap());
    let task = tokio::spawn(sentiment::run_with(state.clone(), src));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while state.sentiment.last_pass_ms.load(Ordering::Relaxed) == 0 {
        assert!(tokio::time::Instant::now() < deadline, "no sentiment pass");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let body: Value = reqwest::get(format!("{base}/sentiment?symbols={PHI},{OKC},NOPE"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let r: SentimentResponse = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(r.games.len(), 2);
    assert_eq!(r.min_holders, 3);
    let phi = r.games.iter().find(|g| g.symbol == PHI).unwrap();
    assert_eq!(phi.game_id, "nba-phi-ind-2026-10-28");
    assert_eq!(phi.price.away_u, 695_000);
    assert_eq!(phi.pascal_holders.away.accounts, 3);
    assert_eq!(phi.pascal_holders.home.stake_u, 66 * 305_000);
    assert_eq!(phi.pascal_holders.excluded_makers, 1);
    assert_eq!(phi.pascal_holders.trades_seen, 7);
    assert!(phi.pascal_holders.synced && !phi.pascal_holders.thin);
    assert_eq!(phi.flow_24h.away_u, 114_047_000);
    // 6.0c: all-time takers from the same stored row.
    assert_eq!(
        phi.traded_all_time,
        Traded {
            away_u: 114_047_000,
            home_u: 10_170_000,
            total_u: 124_217_000
        }
    );
    // rS of the open ticket, S of the pending; settled tickets and resolved legs left out.
    assert_eq!(
        phi.schedina,
        SchedinaStake {
            away_u: 9 * U,
            home_u: 5 * U,
            tickets: 2
        }
    );
    let okc = r.games.iter().find(|g| g.symbol == OKC).unwrap();
    assert!(okc.pascal_holders.thin);
    assert_eq!(okc.schedina.home_u, 9 * U);

    // 7.0c (L7-1): traders = 5 non-maker holders + alice, bob and carol (dave was refunded).
    assert_eq!(phi.traders, 8);
    // The 10 newest stakes on PHI: its 7 Pascal taker trades (all against the maker, none by
    // it; a buy backs the 76ers at size × price, a sell the Pacers at size × (1 − price)) and 4
    // Schedina tickets (rS once minted, S while pending), the oldest (t3) cut by the cap.
    let want = [
        (4_900_000, "away", 1_793_238_438_325, "pascal"),
        (7 * U, "away", 1_793_236_000_000, "schedina"),
        (2_345_000, "home", 1_793_235_463_208, "pascal"),
        (11_439_000, "home", 1_793_235_407_299, "pascal"),
        (7_799_000, "away", 1_793_221_168_722, "pascal"),
        (5 * U, "home", 1_793_220_000_000, "schedina"),
        (106_248_000, "away", 1_793_215_138_376, "pascal"),
        (9 * U, "away", 1_793_212_000_000, "schedina"),
        (4_485_000, "home", 1_793_211_625_238, "pascal"),
        (5_685_000, "home", 1_793_135_700_359, "pascal"),
    ];
    let got: Vec<(u64, &str, u64, &str)> = phi
        .recent
        .iter()
        .map(|s| (s.amount_u, s.team.as_str(), s.ts_ms, s.source.as_str()))
        .collect();
    assert_eq!(got, want);
    // OKC: the maker's own taker trade is left out; 2 holders + alice.
    assert_eq!(okc.traders, 3);
    let got: Vec<(u64, &str, &str)> = okc
        .recent
        .iter()
        .map(|s| (s.amount_u, s.team.as_str(), s.source.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            (9_285_000, "home", "pascal"),
            (7_635_000, "home", "pascal"),
            (9 * U, "home", "schedina"),
        ]
    );
    println!(
        "served /sentiment PHI traders {} recent {}",
        body["games"][0]["traders"], body["games"][0]["recent"]
    );
    for k in ["traders", "recent"] {
        assert!(body["games"][0].get(k).is_some(), "{k}");
    }
    for k in ["amount_u", "team", "ts_ms", "source"] {
        assert!(body["games"][0]["recent"][0].get(k).is_some(), "{k}");
    }
    // The exact L5-4 keys.
    let g = &body["games"][0];
    for k in [
        "game_id",
        "symbol",
        "price",
        "pascal_holders",
        "flow_24h",
        "schedina",
        "traded_all_time",
    ] {
        assert!(g.get(k).is_some(), "{k}");
    }
    for k in ["away_u", "home_u", "total_u"] {
        assert!(g["traded_all_time"][k].is_u64(), "{k}");
    }
    for k in ["away", "home", "excluded_makers", "trades_seen"] {
        assert!(g["pascal_holders"].get(k).is_some(), "{k}");
    }
    assert!(g["pascal_holders"]["away"]["stake_u"].is_u64());
    assert!(g["schedina"]["tickets"].is_u64());

    // Every game by start time without `symbols`; the stub saw reads only.
    let all: SentimentResponse = reqwest::get(format!("{base}/sentiment"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(all.games.len(), 5);
    assert!(all.games.windows(2).all(|w| w[0].game_id != w[1].game_id));
    // `fixtures/desk/sentiment.json` (the app's demo answer) is this answer, dated like the
    // example card. `SENTIMENT_WRITE_FIXTURE=1` rewrites it.
    let fixture_path = repo_root().join("fixtures/desk/sentiment.json");
    let mut dated = all.clone();
    dated.as_of_ms = 1_793_212_985_011;
    if std::env::var("SENTIMENT_WRITE_FIXTURE").is_ok_and(|v| v == "1") {
        let text = serde_json::to_string_pretty(&dated).unwrap() + "\n";
        std::fs::write(&fixture_path, text).unwrap();
    }
    let on_disk: SentimentResponse =
        serde_json::from_str(&std::fs::read_to_string(&fixture_path).unwrap()).unwrap();
    assert_eq!(on_disk, dated, "fixtures/desk/sentiment.json is stale");
    for l in stub.log.lock().unwrap().iter() {
        assert!(
            l == "GET /api/v1/markets" || l == "GET /api/v1/trades",
            "{l}"
        );
    }
    state.shutdown();
    task.await.unwrap();
}

// ---------- the live smoke test ----------

/// Reads today's real trades for the live NBA card from `data.pascal.trade` (read only; one
/// `/markets` and a few `/trades` pages) and prints the per-game numbers.
#[tokio::test]
#[ignore = "reads the real data.pascal.trade"]
async fn live_sentiment_smoke() {
    let db = TempDb::new("sentiment-live");
    let mut config = Config::local(desk::config::PASCAL_DATA_URL_LIVE, db.path());
    config.profile = desk::Profile::Live;
    let pool = desk::db::open(&db.path()).await.unwrap();
    let pascal = desk::state::pascal_client(&config).unwrap();
    let state = AppState::new(config.clone(), pascal, pool);
    desk::catalogue::refresh(&state).await.unwrap();
    let cat = state.catalogue().unwrap();
    let now = desk::now_ms();
    // Games not started yet or started within the last 4 h: tonight's card.
    let symbols: Vec<String> = cat
        .games
        .values()
        .filter(|s| {
            let m = &cat.markets[*s];
            m.start_ms + 4 * 3_600_000 > now && m.start_ms < now + 36 * 3_600_000
        })
        .cloned()
        .collect();
    assert!(!symbols.is_empty(), "no NBA moneyline on the card");
    let src = sentiment::trades_client(&config).unwrap();
    let mut f = Follower::new();
    f.budget = 10;
    let stats = f.pass(&src, &symbols, &state.sentiment.tape).await.unwrap();
    println!(
        "pages {} trades {} backfilling {}",
        stats.pages, stats.new_trades, stats.backfilling
    );
    let card = {
        let t = state.sentiment.tape.read();
        let accounts: HashSet<&String> =
            t.markets.values().flat_map(|m| m.accounts.keys()).collect();
        println!("accounts on the card: {}", accounts.len());
        for (s, m) in &t.markets {
            for (o, a) in &m.accounts {
                println!(
                    "  {s} {}… trades {} as maker {} position {}",
                    &o[..4],
                    a.trades,
                    a.maker,
                    a.position
                );
            }
        }
        t.card_makers(&MakerRule::default())
    };
    println!("card-wide market makers: {card:?}");
    let q = HashMap::from([("symbols".to_owned(), symbols.join(","))]);
    let axum::Json(r) = sentiment::sentiment(State(state.clone()), Query(q))
        .await
        .unwrap();
    for g in &r.games {
        let m = &cat.markets[&g.symbol];
        let h = &g.pascal_holders;
        println!(
            "{} {} @ {} | mid {:.3}/{:.3} | trades {} makers out {} | holders {} {} (${:.2}) {} {} (${:.2}) | 24 h ${:.2}/${:.2} | thin {}",
            m.game_id,
            m.away.abbr,
            m.home.abbr,
            g.price.away_u as f64 / 1e6,
            g.price.home_u as f64 / 1e6,
            h.trades_seen,
            h.excluded_makers,
            m.away.abbr,
            h.away.accounts,
            h.away.stake_u as f64 / 1e6,
            m.home.abbr,
            h.home.accounts,
            h.home.stake_u as f64 / 1e6,
            g.flow_24h.away_u as f64 / 1e6,
            g.flow_24h.home_u as f64 / 1e6,
            h.thin,
        );
    }
}
