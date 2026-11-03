//! Review gate (2.A): the desk over the in-process `pascal-mock`, frozen on the sample tape of
//! 28 Oct (localhost only, no Pascal):
//!
//! - `POST /quote` reproduces `pricing::quote`'s numbers on the same books (computed here from
//!   the tape's markets and the mock's `/books`, independently of the desk's catalogue path),
//!   its signed message decodes to those numbers, the signature verifies with the signer's key,
//!   and the `quotes` / `quote_legs` rows (L2-1) exist when the response arrives;
//! - two legs on one game are refused with the reason; an unknown symbol, a paused desk, a copy
//!   while copies are off are refused;
//! - with `/books` rate limited (429) a quote prices from the last snapshot while it is ≤ 10 s
//!   old, and answers 503 for books it has no snapshot of;
//! - `GET /cashout-quote` reads the ticket from the DB, matches `pricing::cash_out`, signs the
//!   99-byte message, refuses lost and closed tickets and 404s unknown ones; the registered
//!   `CashOutValuer` agrees; `book_snapshots` gets rows;
//! - copy-quotes work when enabled, for a minted, open own tip whose legs are unresolved and
//!   start over 60 s from now; anything else is refused with its reason before anything is
//!   signed (`/ticket/{id}/copy-quote` and `/quote` with `copied_from` alike).

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::Ordering;
use std::time::Duration;

use base64::Engine;
use common::{TempDb, repo_root};
use desk::api::types::{
    CashoutQuoteResponse, ErrorBody, ErrorStatus, LegStatus, QuoteLeg, QuoteResponse, Side,
    TicketStatus,
};
use desk::quoter::QUOTE_TTL_S;
use desk::quoter::codec::{self, CashOutMsg, QuoteMsg};
use desk::state::{LegState, TicketState};
use desk::{Config, Running};
use ed25519_dalek::{Signature, SigningKey, Verifier, VerifyingKey};
use pascal_mock::{Mock, MockConfig, RunningMock, ScriptSource};
use pascal_rs::{Market, OrderBook};
use serde_json::{Value, json};

const PHI: &str = "NBA_26OCT28_PHIIND_ML.PHI";
const HOU: &str = "NBA_26OCT28_HOUMIL_ML.HOU";
const ORL: &str = "NBA_26OCT28_ORLTOR_ML.ORL";
const OKC: &str = "NBA_26OCT28_OKCDAL_ML.OKC";
/// A Ticket PDA for the DB rows (any valid pubkey).
const TICKET: &str = "HGMZNFqTMf4bcdUpSoZNG981rPiAYk6utvoPuSJ19DZy";
const TICKET_LOST: &str = "H6j446c1VsVEYyK7JW6zM6Zfjv4Gg2DrvtfTFEURNY9g";
const TICKET_SETTLED: &str = "6P3JVey5HWAxhLTaRCAEeJ3yoXWNT7YsxbHhHbESntos";

async fn mock() -> RunningMock {
    let mut cfg = MockConfig::new(repo_root().join("tapes/nba-2026-10-28/sample"));
    cfg.speed = 0.0;
    cfg.script = ScriptSource::None;
    Mock::new(cfg).unwrap().spawn("127.0.0.1:0").await.unwrap()
}

/// A quote-signer keypair file in a fresh temp dir (test key, never committed).
struct TempKey {
    dir: PathBuf,
    pubkey: String,
}

impl TempKey {
    fn new(seed: u8) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "schedina-quoter-{}-{}-{seed}",
            std::process::id(),
            desk::now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let sk = SigningKey::from_bytes(&[seed; 32]);
        let mut bytes = vec![seed; 32];
        bytes.extend_from_slice(sk.verifying_key().as_bytes());
        std::fs::write(
            dir.join("quote-signer.json"),
            serde_json::to_string(&bytes).unwrap(),
        )
        .unwrap();
        TempKey {
            pubkey: bs58::encode(sk.verifying_key().as_bytes()).into_string(),
            dir,
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.join("quote-signer.json")
    }
}

impl Drop for TempKey {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn desk(mock: &RunningMock, db: &TempDb, key: &TempKey, copy_quote: bool) -> Running {
    let mut cfg = Config::local(mock.base_url(), db.path());
    cfg.quote_signer_keypair_path = key.path();
    cfg.shutdown_grace = Duration::from_secs(5);
    cfg.copy_quote = copy_quote;
    let d = desk::start(cfg).await.expect("desk starts");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while d.state.catalogue().is_none() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "catalogue never loaded"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    d
}

async fn post(url: &str, body: &Value) -> (u16, Value) {
    let r = reqwest::Client::new()
        .post(url)
        .json(body)
        .send()
        .await
        .unwrap();
    (r.status().as_u16(), r.json().await.unwrap())
}

async fn get(url: &str) -> (u16, Value) {
    let r = reqwest::get(url).await.unwrap();
    (r.status().as_u16(), r.json().await.unwrap())
}

fn req(legs: &[(&str, &str)], stake_u: u64) -> Value {
    json!({
        "legs": legs.iter().map(|(s, side)| json!({"symbol": s, "side": side})).collect::<Vec<_>>(),
        "stake_u": stake_u,
        "token_in": null,
        "copied_from": null,
    })
}

fn error(status: u16, body: &Value) -> (u16, ErrorBody) {
    (status, serde_json::from_value(body.clone()).unwrap())
}

fn tape_markets() -> BTreeMap<String, Market> {
    let text =
        std::fs::read_to_string(repo_root().join("tapes/nba-2026-10-28/markets_selected.json"))
            .unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    let list: Vec<Market> = serde_json::from_value(v["data"].clone()).unwrap();
    list.into_iter().map(|m| (m.symbol.clone(), m)).collect()
}

/// The mock's books and clock, read with a plain pascal-rs client.
async fn mock_books(mock: &RunningMock, symbols: &[&str]) -> (u64, BTreeMap<String, OrderBook>) {
    let client = pascal_rs::Client::builder()
        .base_url(mock.base_url())
        .build()
        .unwrap();
    let r = client.books(symbols).await.unwrap();
    (r.server_time_ms.unwrap(), r.data.books)
}

fn verify(signer: &str, msg: &[u8], sig: &[u8]) {
    let pk: [u8; 32] = bs58::decode(signer).into_vec().unwrap().try_into().unwrap();
    VerifyingKey::from_bytes(&pk)
        .unwrap()
        .verify(msg, &Signature::from_slice(sig).unwrap())
        .expect("signature verifies with the quote signer's key");
}

fn b64(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(s).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn quote_over_the_mock() {
    let mock = mock().await;
    let db = TempDb::new("quoter");
    let key = TempKey::new(11);
    let d = desk(&mock, &db, &key, false).await;
    let base = format!("http://{}", d.addr);

    // --- a two-leg quote, equal to pricing::quote on the mock's books -------------------------
    let (status, body) = post(
        &format!("{base}/quote"),
        &req(&[(PHI, "long"), (HOU, "short")], 10_000_000),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let first_quote_at = tokio::time::Instant::now();
    let resp: QuoteResponse = serde_json::from_value(body).unwrap();

    let markets = tape_markets();
    let (venue_now, books) = mock_books(&mock, &[PHI, HOU]).await;
    let legs = [
        pricing::LegInput::from_market(&markets[PHI], pricing::Side::Long).unwrap(),
        pricing::LegInput::from_market(&markets[HOU], pricing::Side::Short).unwrap(),
    ];
    let q = pricing::quote(
        &legs,
        &books,
        10_000_000,
        venue_now,
        &pricing::Params::default(),
    )
    .unwrap();
    assert_eq!(resp.n, q.n);
    assert_eq!(resp.stake_u, q.stake_u);
    assert_eq!(resp.odds_fair_u, q.odds_fair_u);
    assert_eq!(resp.odds_offered_u, q.odds_offered_u);
    assert_eq!(resp.payout_u, q.payout_u);
    assert_eq!(resp.fee_u, q.fee_u);
    assert_eq!(resp.pascal_fees_est_u, q.pascal_fees_est_u);
    assert_eq!(resp.slippage_bps, q.slippage_bps);
    assert_eq!(resp.min_payout_per_unit_u, q.min_payout_per_unit_u);
    assert_eq!(resp.copy_fee_u, 0);
    assert_eq!(resp.legs.len(), 2);
    for (a, b) in resp.legs.iter().zip(&q.legs) {
        assert_eq!(
            (a.symbol.as_str(), a.game_id.as_str(), a.start_ms),
            (b.symbol.as_str(), b.game_id.as_str(), b.start_ms)
        );
        assert_eq!(
            (a.price_u, a.p_max_u, a.m1_ppm, a.overlaps),
            (b.price_u, b.p_max_u, b.m1_ppm, b.overlaps)
        );
    }
    assert_eq!(resp.legs[1].side, Side::Short);
    // The sample tape's first snapshot (pricing's own tape test has the same top of book).
    assert_eq!(
        (resp.legs[0].price_u, resp.legs[1].price_u),
        (699_000, 240_000)
    );

    // TTL on the desk's wall clock.
    let now_s = (desk::now_ms() / 1000) as i64;
    assert!(
        (now_s + QUOTE_TTL_S - 1..=now_s + QUOTE_TTL_S).contains(&resp.quote_expires_at),
        "{} vs {now_s}",
        resp.quote_expires_at
    );

    // The signed bytes say what the response says, and verify with the signer's key.
    assert_eq!(resp.quote_signer, key.pubkey);
    let msg = b64(&resp.quote_msg);
    let sig = b64(&resp.quote_sig);
    assert_eq!(msg.len(), 77 + 85 * 2);
    assert_eq!(sig.len(), 64);
    verify(&resp.quote_signer, &msg, &sig);
    let m = QuoteMsg::decode(&msg).unwrap();
    assert_eq!(m.program_id, d.state.config.program_id.to_bytes());
    assert_eq!(m.stake_u, resp.stake_u);
    assert_eq!(m.odds_offered_u, resp.odds_offered_u);
    assert_eq!(m.quote_expires_at, resp.quote_expires_at);
    for (ml, rl) in m.legs.iter().zip(&resp.legs) {
        assert_eq!(ml.symbol_hash, codec::sha256(rl.symbol.as_bytes()));
        assert_eq!(ml.game_id_hash, codec::sha256(rl.game_id.as_bytes()));
        assert_eq!(ml.start_ts, (rl.start_ms / 1000) as i64);
        assert_eq!(ml.price_u, rl.price_u);
        assert_eq!(ml.m1_ppm, rl.m1_ppm);
    }
    assert_eq!(m.legs[0].side, codec::SIDE_LONG);
    assert_eq!(m.legs[1].side, codec::SIDE_SHORT);

    // L2-1: the quote is on record, keyed by the hash of the signed bytes.
    let hash = codec::hex(&codec::sha256(&msg));
    let (stored_msg, legs_json, n): (Vec<u8>, String, i64) =
        sqlx::query_as("SELECT msg, legs_json, n FROM quotes WHERE quote_hash = ?")
            .bind(&hash)
            .fetch_one(&d.state.db)
            .await
            .unwrap();
    assert_eq!(stored_msg, msg);
    assert_eq!(n, 2);
    let stored_legs: Vec<QuoteLeg> = serde_json::from_str(&legs_json).unwrap();
    assert_eq!(stored_legs, resp.legs);
    let rows: Vec<(i64, String, String, String)> = sqlx::query_as(
        "SELECT idx, symbol, symbol_hash, game_id_hash FROM quote_legs WHERE quote_hash = ?
         ORDER BY idx",
    )
    .bind(&hash)
    .fetch_all(&d.state.db)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].1, PHI);
    assert_eq!(rows[0].2, codec::hex(&codec::sha256(PHI.as_bytes())));
    assert_eq!(
        rows[1].3,
        codec::hex(&codec::sha256(b"nba-hou-mil-2026-10-28"))
    );
    // The books were published for the other services.
    {
        let books = d.state.books.read();
        assert!(books.contains_key(PHI) && books.contains_key(HOU));
        assert_eq!(books[PHI].asks[0].price, 699_000);
    }

    // --- refusals ------------------------------------------------------------------------------
    let (status, body) = post(
        &format!("{base}/quote"),
        &req(&[(PHI, "long"), (PHI, "short")], 10_000_000),
    )
    .await;
    let (status, e) = error(status, &body);
    assert_eq!((status, e.status), (422, ErrorStatus::Rejected));
    assert_eq!(e.reason, "two legs on game nba-phi-ind-2026-10-28");

    let (status, body) = post(
        &format!("{base}/quote"),
        &req(&[("NBA_26OCT28_XXXYYY_ML.XXX", "long")], 10_000_000),
    )
    .await;
    assert_eq!(error(status, &body).0, 422, "{body}");
    let (status, body) = post(&format!("{base}/quote"), &req(&[(PHI, "long")], 1_000_000)).await;
    assert_eq!(error(status, &body).0, 422, "{body}");
    assert!(
        body["reason"].as_str().unwrap().contains("minimum"),
        "{body}"
    );
    let (status, body) = post(&format!("{base}/quote"), &json!({"legs": 3})).await;
    assert_eq!(
        error(status, &body).1.status,
        ErrorStatus::Rejected,
        "{status}"
    );
    let mut copy = req(&[(PHI, "long")], 10_000_000);
    copy["copied_from"] = json!(TICKET);
    let (status, body) = post(&format!("{base}/quote"), &copy).await;
    assert_eq!(error(status, &body).0, 422, "{body}");
    let (status, body) = post(
        &format!("{base}/ticket/{TICKET}/copy-quote"),
        &json!({"stake_u": 10_000_000}),
    )
    .await;
    assert_eq!(error(status, &body).0, 422, "{body}");

    d.state.paused.store(true, Ordering::Relaxed);
    let (status, body) = post(&format!("{base}/quote"), &req(&[(PHI, "long")], 10_000_000)).await;
    assert_eq!(error(status, &body), (503, error(503, &body).1));
    assert_eq!(body["status"], "unavailable");
    d.state.paused.store(false, Ordering::Relaxed);

    // --- Pascal rate limits /books (spec 12 runbook) -------------------------------------------
    let client = reqwest::Client::new();
    let r = client
        .post(format!("{}/mock/inject", mock.base_url()))
        .json(&json!({"code": "RATE_LIMITED", "times": 100000, "path": "/api/v1/books"}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    // No snapshot of ORL and OKC: unavailable.
    let (status, body) = post(
        &format!("{base}/quote"),
        &req(&[(ORL, "long"), (OKC, "long")], 10_000_000),
    )
    .await;
    let (status, e) = error(status, &body);
    assert_eq!(
        (status, e.status),
        (503, ErrorStatus::Unavailable),
        "{body}"
    );
    // PHI and HOU were fetched < 10 s ago but > 1 s ago: priced from that snapshot, same numbers.
    let wait = Duration::from_millis(1_100).saturating_sub(first_quote_at.elapsed());
    tokio::time::sleep(wait).await;
    let (status, body) = post(
        &format!("{base}/quote"),
        &req(&[(PHI, "long"), (HOU, "short")], 10_000_000),
    )
    .await;
    assert!(
        first_quote_at.elapsed() < Duration::from_secs(10),
        "test too slow to exercise the snapshot window"
    );
    assert_eq!(status, 200, "{body}");
    let again: QuoteResponse = serde_json::from_value(body).unwrap();
    assert_eq!(again.payout_u, resp.payout_u);
    assert_eq!(again.legs, resp.legs);

    tokio::time::timeout(Duration::from_secs(15), d.join())
        .await
        .expect("graceful shutdown");
}

type LegRow<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str, Option<i64>);

async fn insert_ticket(
    db: &sqlx::SqlitePool,
    ticket: &str,
    nonce: i64,
    state: &str,
    payout_u: i64,
    legs: &[LegRow<'_>],
) {
    sqlx::query(
        "INSERT INTO tickets (ticket, pending, user, nonce, state, n, stake_u, payout_u,
            odds_offered_u, min_payout_per_unit_u, updated_ms)
         VALUES (?, ?, 'U', ?, ?, ?, 10000000, ?, 1, 1, 0)",
    )
    .bind(ticket)
    .bind(format!("P-{ticket}"))
    .bind(nonce)
    .bind(state)
    .bind(legs.len() as i64)
    .bind(payout_u)
    .execute(db)
    .await
    .unwrap();
    for (idx, (symbol, game_id, side, pick, status, resolution_u)) in legs.iter().enumerate() {
        sqlx::query(
            "INSERT INTO ticket_legs (ticket, idx, symbol, game_id, side, pick, start_ms,
                p_quote_u, m1_ppm, status, resolution_u)
             VALUES (?, ?, ?, ?, ?, ?, 1793228400000, 500000, 50000, ?, ?)",
        )
        .bind(ticket)
        .bind(idx as i64)
        .bind(symbol)
        .bind(game_id)
        .bind(side)
        .bind(pick)
        .bind(status)
        .bind(resolution_u)
        .execute(db)
        .await
        .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cash_out_quote_over_the_mock() {
    let mock = mock().await;
    let db = TempDb::new("quoter-cashout");
    let key = TempKey::new(12);
    let d = desk(&mock, &db, &key, true).await;
    let base = format!("http://{}", d.addr);
    let pool = &d.state.db;
    let phi = (PHI, "nba-phi-ind-2026-10-28", "long", "away");
    let hou = (HOU, "nba-hou-mil-2026-10-28", "short", "home");
    insert_ticket(
        pool,
        TICKET,
        1,
        "open",
        30_000_000,
        &[
            (phi.0, phi.1, phi.2, phi.3, "won", Some(1_000_000)),
            (hou.0, hou.1, hou.2, hou.3, "open", None),
        ],
    )
    .await;
    insert_ticket(
        pool,
        TICKET_LOST,
        2,
        "open",
        30_000_000,
        &[
            (phi.0, phi.1, phi.2, phi.3, "lost", Some(0)),
            (hou.0, hou.1, hou.2, hou.3, "open", None),
        ],
    )
    .await;
    insert_ticket(
        pool,
        TICKET_SETTLED,
        3,
        "settled",
        30_000_000,
        &[(phi.0, phi.1, phi.2, phi.3, "won", Some(1_000_000))],
    )
    .await;

    // --- the open ticket: C = P · 1 · (1 − ask_HOU(Δ)) · 0.98 ------------------------------------
    let (status, body) = get(&format!("{base}/cashout-quote/{TICKET}")).await;
    assert_eq!(status, 200, "{body}");
    let resp: CashoutQuoteResponse = serde_json::from_value(body).unwrap();
    let (_, books) = mock_books(&mock, &[HOU]).await;
    let expected = pricing::cash_out(
        30_000_000,
        &[
            pricing::CashOutLeg {
                symbol: PHI,
                side: pricing::Side::Long,
                status: pricing::LegStatus::Resolved { r_u: 1_000_000 },
                book: None,
            },
            pricing::CashOutLeg {
                symbol: HOU,
                side: pricing::Side::Short,
                status: pricing::LegStatus::Open,
                book: books.get(HOU),
            },
        ],
        &pricing::Params::default(),
    )
    .unwrap();
    assert_eq!(resp.c_u, expected.c_u);
    assert!(resp.c_u > 0 && resp.c_u < 30_000_000);
    assert_eq!(resp.ticket, TICKET);
    assert_eq!(resp.quote_signer, key.pubkey);
    let now_s = (desk::now_ms() / 1000) as i64;
    assert!((now_s - 1..=now_s).contains(&resp.ts));
    let msg = b64(&resp.quote_msg);
    assert_eq!(msg.len(), 99);
    verify(&resp.quote_signer, &msg, &b64(&resp.quote_sig));
    let m = CashOutMsg::decode(&msg).unwrap();
    assert_eq!(
        m.ticket,
        solana_pubkey::Pubkey::from_str(TICKET).unwrap().to_bytes()
    );
    assert_eq!((m.c_u, m.ts), (resp.c_u, resp.ts));
    assert_eq!(m.program_id, d.state.config.program_id.to_bytes());
    let logged: i64 = sqlx::query_scalar("SELECT c_u FROM cashout_quotes WHERE ticket = ?")
        .bind(TICKET)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(logged as u64, resp.c_u);

    // The registered valuer agrees from the cached books (no I/O).
    let ts = TicketState {
        ticket: TICKET.to_owned(),
        pending: "P".to_owned(),
        user: "U".to_owned(),
        status: TicketStatus::Open,
        stake_u: 10_000_000,
        rs_u: Some(10_000_000),
        payout_u: 30_000_000,
        legs: vec![
            leg_state(0, PHI, Side::Long, LegStatus::Won),
            leg_state(1, HOU, Side::Short, LegStatus::Open),
        ],
    };
    let c = d.state.cash_out_for(&ts);
    assert!(c.available, "{c:?}");
    assert_eq!(c.c_u, Some(resp.c_u));
    let lost = TicketState {
        legs: vec![
            leg_state(0, PHI, Side::Long, LegStatus::Lost),
            leg_state(1, HOU, Side::Short, LegStatus::Open),
        ],
        ..ts.clone()
    };
    assert!(!d.state.cash_out_for(&lost).available);
    let unknown_book = TicketState {
        legs: vec![leg_state(0, ORL, Side::Long, LegStatus::Open)],
        ..ts.clone()
    };
    let c = d.state.cash_out_for(&unknown_book);
    assert!(!c.available && c.reason.unwrap().contains("no book"));

    // --- refusals --------------------------------------------------------------------------------
    let (status, body) = get(&format!("{base}/cashout-quote/{TICKET_LOST}")).await;
    let (status, e) = error(status, &body);
    assert_eq!((status, e.status), (503, ErrorStatus::Unavailable));
    assert!(e.reason.contains("lost"), "{}", e.reason);
    let (status, body) = get(&format!("{base}/cashout-quote/{TICKET_SETTLED}")).await;
    assert_eq!(error(status, &body).0, 503);
    assert!(body["reason"].as_str().unwrap().contains("settled"));
    let (status, body) = get(&format!(
        "{base}/cashout-quote/11111111111111111111111111111112"
    ))
    .await;
    assert_eq!(error(status, &body), (404, error(404, &body).1));
    assert_eq!(body["status"], "not_found");
    let (status, _) = get(&format!("{base}/cashout-quote/not-a-key")).await;
    assert_eq!(status, 404);

    // --- book_snapshots: open legs and quoted symbols ---------------------------------------------
    let n = desk::quoter::books::snapshot_once(&d.state).await.unwrap();
    assert!(n >= 1, "{n}");
    let (bid, ask, window, venue): (Option<i64>, Option<i64>, i64, Option<i64>) = sqlx::query_as(
        "SELECT best_bid_u, best_ask_u, depth_window_u, state_time_ms FROM book_snapshots
         WHERE symbol = ? ORDER BY ts_ms DESC LIMIT 1",
    )
    .bind(HOU)
    .fetch_one(pool)
    .await
    .unwrap();
    let hou_book = &books[HOU];
    assert_eq!(bid, Some(hou_book.bids[0].price));
    assert_eq!(ask, Some(hou_book.asks[0].price));
    assert_eq!(window, 20_000);
    assert!(venue.is_some());

    // --- copy-quote (enabled on this desk) refuses a ticket that is not a tip -------------------
    let (status, body) = post(
        &format!("{base}/ticket/{TICKET}/copy-quote"),
        &json!({"stake_u": 10_000_000}),
    )
    .await;
    assert_eq!(error(status, &body).0, 422, "{body}");
    assert!(
        body["reason"].as_str().unwrap().contains("not a tip"),
        "{body}"
    );

    tokio::time::timeout(Duration::from_secs(15), d.join())
        .await
        .expect("graceful shutdown");
}

/// A copy-source leg: symbol, game, side, pick, status, start (Unix ms).
type CopyLeg<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str, u64);

/// A ticket row owned by `TIPSTER`, for the copy pre-check. The quoter prices from the tape's
/// catalogue (its venue clock is 28 Oct), while the pre-check reads the start times on record, so
/// a test sets those relative to the wall clock.
async fn insert_copy_source(
    db: &sqlx::SqlitePool,
    ticket: &str,
    nonce: i64,
    state: &str,
    who: (Option<&str>, Option<&str>),
    legs: &[CopyLeg<'_>],
) {
    let (tipster, copied_from) = who;
    sqlx::query(
        "INSERT INTO tickets (ticket, pending, user, nonce, state, n, stake_u, payout_u,
            odds_offered_u, min_payout_per_unit_u, tipster, copied_from, is_tip, updated_ms)
         VALUES (?, ?, 'TIPSTER', ?, ?, ?, 10000000, 30000000, 1, 1, ?, ?, ?, 0)",
    )
    .bind(ticket)
    .bind(format!("P-{ticket}"))
    .bind(nonce)
    .bind(state)
    .bind(legs.len() as i64)
    .bind(tipster)
    .bind(copied_from)
    .bind(i64::from(
        tipster == Some("TIPSTER") && copied_from.is_none(),
    ))
    .execute(db)
    .await
    .unwrap();
    for (idx, (symbol, game_id, side, pick, status, start_ms)) in legs.iter().enumerate() {
        sqlx::query(
            "INSERT INTO ticket_legs (ticket, idx, symbol, game_id, side, pick, start_ms,
                p_quote_u, m1_ppm, status)
             VALUES (?, ?, ?, ?, ?, ?, ?, 500000, 50000, ?)",
        )
        .bind(ticket)
        .bind(idx as i64)
        .bind(symbol)
        .bind(game_id)
        .bind(side)
        .bind(pick)
        .bind(*start_ms as i64)
        .bind(status)
        .execute(db)
        .await
        .unwrap();
    }
}

/// A refused copy: id, state, (tipster, copied_from), legs, reason fragment.
type CopyCase<'a> = (
    &'a str,
    &'a str,
    (Option<&'a str>, Option<&'a str>),
    Vec<CopyLeg<'a>>,
    &'a str,
);

#[tokio::test(flavor = "multi_thread")]
async fn copy_quote_pre_check() {
    let mock = mock().await;
    let db = TempDb::new("quoter-copy");
    let key = TempKey::new(14);
    let d = desk(&mock, &db, &key, true).await;
    let base = format!("http://{}", d.addr);
    let pool = &d.state.db;
    let now = desk::now_ms();
    let later = now + 2 * 3_600_000;
    let phi = |status, start| (PHI, "nba-phi-ind-2026-10-28", "long", "away", status, start);
    let hou = |status, start| {
        (
            HOU,
            "nba-hou-mil-2026-10-28",
            "short",
            "home",
            status,
            start,
        )
    };
    let tip = (Some("TIPSTER"), None);
    let good = vec![phi("open", later), hou("open", later + 1_800_000)];
    insert_copy_source(pool, "TIP_OPEN", 1, "open", tip, &good).await;

    // --- an open own tip with unstarted, unresolved legs: quoted with the copy fee -------------
    let (status, body) = post(
        &format!("{base}/ticket/TIP_OPEN/copy-quote"),
        &json!({"stake_u": 10_000_000}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let cq: QuoteResponse = serde_json::from_value(body).unwrap();
    assert_eq!(cq.copy_fee_u, 200_000);
    assert_eq!(cq.legs.len(), 2);
    assert_eq!(cq.legs[1].side, Side::Short);
    let hash = codec::hex(&codec::sha256(&b64(&cq.quote_msg)));
    let from: Option<String> =
        sqlx::query_scalar("SELECT copied_from FROM quotes WHERE quote_hash = ?")
            .bind(&hash)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(from.as_deref(), Some("TIP_OPEN"));
    // `/quote` with `copied_from`: the original's legs pass, other legs do not.
    let mut same = req(&[(PHI, "long"), (HOU, "short")], 10_000_000);
    same["copied_from"] = json!("TIP_OPEN");
    let (status, body) = post(&format!("{base}/quote"), &same).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["copy_fee_u"], 200_000);
    let mut wrong = req(&[(PHI, "long")], 10_000_000);
    wrong["copied_from"] = json!("TIP_OPEN");
    let (status, body) = post(&format!("{base}/quote"), &wrong).await;
    assert_eq!(error(status, &body).0, 422, "{body}");
    assert!(
        body["reason"].as_str().unwrap().contains("legs differ"),
        "{body}"
    );

    // --- refusals, each with its reason and nothing signed --------------------------------------
    let cases: Vec<CopyCase> = vec![
        (
            "TIP_PENDING",
            "pending",
            tip,
            good.clone(),
            "not minted yet",
        ),
        (
            "TIP_SETTLED",
            "settled",
            tip,
            good.clone(),
            "no longer open",
        ),
        ("TIP_CASHED", "cashed_out", tip, good.clone(), "cashed out"),
        ("NOT_TIP", "open", (None, None), good.clone(), "not a tip"),
        (
            "OTHER_TIPSTER",
            "open",
            (Some("SOMEONE"), None),
            good.clone(),
            "not a tip",
        ),
        (
            "A_COPY",
            "open",
            (Some("TIPSTER"), Some("TIP_OPEN")),
            good.clone(),
            "itself a copy",
        ),
        (
            "TIP_DECIDED",
            "open",
            tip,
            vec![phi("won", later), hou("open", later)],
            "already decided",
        ),
    ];
    for (i, (id, state, who, legs, _)) in cases.iter().enumerate() {
        insert_copy_source(pool, id, 10 + i as i64, state, *who, legs).await;
    }
    let quotes_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM quotes")
        .fetch_one(pool)
        .await
        .unwrap();
    for (id, _, _, legs, why) in &cases {
        let (status, body) = post(
            &format!("{base}/ticket/{id}/copy-quote"),
            &json!({"stake_u": 10_000_000}),
        )
        .await;
        let (status, e) = error(status, &body);
        assert_eq!(
            (status, e.status),
            (422, ErrorStatus::Rejected),
            "{id}: {body}"
        );
        assert!(e.reason.contains(why), "{id}: {}", e.reason);
        assert!(e.reason.contains(id), "{id}: {}", e.reason);
        // The same check guards `/quote` with `copied_from`.
        let sides: Vec<(&str, &str)> = legs.iter().map(|l| (l.0, l.2)).collect();
        let mut q = req(&sides, 10_000_000);
        q["copied_from"] = json!(id);
        let (status, body) = post(&format!("{base}/quote"), &q).await;
        assert_eq!(error(status, &body).0, 422, "{id}: {body}");
        assert!(
            body["reason"].as_str().unwrap().contains(why),
            "{id}: {body}"
        );
    }
    let quotes_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM quotes")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(quotes_after, quotes_before, "a refused copy signs nothing");

    // Unknown ticket: 404 on both routes.
    let unknown = "11111111111111111111111111111112";
    let (status, _) = post(
        &format!("{base}/ticket/{unknown}/copy-quote"),
        &json!({"stake_u": 10_000_000}),
    )
    .await;
    assert_eq!(status, 404);
    let mut q = req(&[(PHI, "long")], 10_000_000);
    q["copied_from"] = json!(unknown);
    let (status, _) = post(&format!("{base}/quote"), &q).await;
    assert_eq!(status, 404);

    tokio::time::timeout(Duration::from_secs(15), d.join())
        .await
        .expect("graceful shutdown");
}

fn leg_state(idx: u8, symbol: &str, side: Side, status: LegStatus) -> LegState {
    LegState {
        idx,
        symbol: symbol.to_owned(),
        game_id: String::new(),
        side,
        start_ms: 0,
        p_quote_u: 500_000,
        p_fill_u: Some(500_000),
        m1_ppm: 50_000,
        target: 0,
        status,
    }
}

/// Without a signer key the desk still starts, and `/quote` answers 503.
#[tokio::test(flavor = "multi_thread")]
async fn no_signer_is_unavailable() {
    let mock = mock().await;
    let db = TempDb::new("quoter-nokey");
    let key = TempKey::new(13);
    std::fs::remove_file(key.path()).unwrap();
    let d = desk(&mock, &db, &key, false).await;
    let (status, body) = post(
        &format!("http://{}/quote", d.addr),
        &req(&[(PHI, "long")], 10_000_000),
    )
    .await;
    assert_eq!(status, 503, "{body}");
    assert!(!d.failed());
    tokio::time::timeout(Duration::from_secs(15), d.join())
        .await
        .expect("graceful shutdown");
}

/// L9: `POST /quote` with `title` and `wallet` keeps the trimmed title for that wallet and the
/// quote's hash; a title without a wallet, a bad wallet, a bad title, or a title on a copy is
/// refused before anything is signed.
#[tokio::test(flavor = "multi_thread")]
async fn quote_keeps_a_tip_title() {
    let mock = mock().await;
    let db = TempDb::new("quoter-title");
    let key = TempKey::new(14);
    let d = desk(&mock, &db, &key, true).await;
    let base = format!("http://{}", d.addr);
    let wallet = "HGMZNFqTMf4bcdUpSoZNG981rPiAYk6utvoPuSJ19DZy";
    let with = |title: Value, wallet: Value, copied_from: Value| {
        let mut r = req(&[(PHI, "long")], 10_000_000);
        r["title"] = title;
        r["wallet"] = wallet;
        r["copied_from"] = copied_from;
        r
    };

    let (status, body) = post(
        &format!("{base}/quote"),
        &with(json!("  Thursday treble "), json!(wallet), Value::Null),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let resp: QuoteResponse = serde_json::from_value(body).unwrap();
    let hash = codec::hex(&codec::sha256(&b64(&resp.quote_msg)));
    let row: (String, String) =
        sqlx::query_as("SELECT wallet, title FROM quote_titles WHERE quote_hash = ?")
            .bind(&hash)
            .fetch_one(&d.state.db)
            .await
            .unwrap();
    assert_eq!(row, (wallet.to_owned(), "Thursday treble".to_owned()));

    for (body, why) in [
        (
            with(json!("Name"), Value::Null, Value::Null),
            "needs the wallet",
        ),
        (
            with(json!("Name"), json!("not-a-key"), Value::Null),
            "not a base58",
        ),
        (
            with(json!("a\u{202E}b"), json!(wallet), Value::Null),
            "title",
        ),
        (with(json!("   "), json!(wallet), Value::Null), "title"),
        (
            with(json!("Name"), json!(wallet), json!(TICKET)),
            "a copy takes no title",
        ),
    ] {
        let (status, e) = post(&format!("{base}/quote"), &body).await;
        let (status, e) = error(status, &e);
        assert_eq!((status, e.status), (422, ErrorStatus::Rejected), "{body}");
        assert!(e.reason.contains(why), "{}", e.reason);
    }
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM quote_titles")
        .fetch_one(&d.state.db)
        .await
        .unwrap();
    assert_eq!(n, 1);
    d.join().await;
}
