//! Review gate (6.0b): the desk over the in-process `pascal-mock` frozen on a synthetic tape (the
//! 28 Oct sample plus the real spread and total listings of PHI @ IND and ORL @ TOR, books built
//! by `desk::catalogue::synthetic`):
//!
//! - `/card` lists the lines; `POST /quote` prices a spread leg and a total leg exactly as
//!   `pricing::quote` does on the mock's books, signs the unchanged C3 layout and names the
//!   legs (`kind`, `line`, `label`);
//! - two legs on one game are refused, a winner and a spread included;
//! - with the mock's clock 10 min after PHI @ IND's tip-off, the card shows the game in play
//!   with its winner only, a winner leg on it is quoted with `"in_play": true`, and a spread leg
//!   on it is refused.
//!
//! The replay tape itself still has moneylines only: `tests/catalogue.rs` keeps its card.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use base64::Engine;
use common::{TempDb, repo_root};
use desk::api::types::{CardResponse, ErrorBody, MarketKind, QuoteResponse, Side};
use desk::quoter::codec::{self, QuoteMsg};
use desk::{Config, Running};
use pascal_mock::{Mock, MockConfig, RunningMock, ScriptSource};
use pascal_rs::{Market, OrderBook};
use serde_json::{Value, json};

const PHI_ML: &str = "NBA_26OCT28_PHIIND_ML.PHI";
const PHI_SP3: &str = "NBA_26OCT28_PHIIND_SP.PHI_3";
const PHI_TT233: &str = "NBA_26OCT28_PHIIND_TT.233";
const ORL_TT221: &str = "NBA_26OCT28_ORLTOR_TT.221";
const PHI_START: u64 = 1_793_228_400_000;

/// A synthetic tape in a fresh temp dir (removed on drop).
struct Tape(PathBuf);

impl Tape {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "schedina-lines-{tag}-{}-{}",
            std::process::id(),
            desk::now_ms()
        ));
        let added = desk::catalogue::synthetic::write_tape(
            &repo_root().join("tapes/nba-2026-10-28/sample"),
            &repo_root().join("fixtures/lines/markets_2026-10-28_phi-ind_orl-tor.TRIMMED.json"),
            &dir,
        )
        .unwrap();
        assert_eq!(added.len(), 21);
        Tape(dir)
    }
}

impl Drop for Tape {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A quote-signer key file next to the tape.
fn key(tape: &Tape) -> PathBuf {
    let sk = ed25519_dalek::SigningKey::from_bytes(&[21; 32]);
    let mut bytes = vec![21_u8; 32];
    bytes.extend_from_slice(sk.verifying_key().as_bytes());
    let p = tape.0.join("quote-signer.json");
    std::fs::write(&p, serde_json::to_string(&bytes).unwrap()).unwrap();
    p
}

async fn mock(tape: &Tape, start_ms: Option<u64>) -> RunningMock {
    let mut cfg = MockConfig::new(&tape.0);
    cfg.speed = 0.0;
    cfg.start_ms = start_ms;
    cfg.script = ScriptSource::None;
    Mock::new(cfg).unwrap().spawn("127.0.0.1:0").await.unwrap()
}

async fn desk(mock: &RunningMock, db: &TempDb, key: PathBuf) -> Running {
    let mut cfg = Config::local(mock.base_url(), db.path());
    cfg.quote_signer_keypair_path = key;
    cfg.shutdown_grace = Duration::from_secs(5);
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

async fn post_quote(base: &str, legs: &[(&str, &str)]) -> (u16, Value) {
    let body = json!({
        "legs": legs.iter().map(|(s, side)| json!({"symbol": s, "side": side})).collect::<Vec<_>>(),
        "stake_u": 10_000_000,
        "token_in": null,
        "copied_from": null,
    });
    let r = reqwest::Client::new()
        .post(format!("{base}/quote"))
        .json(&body)
        .send()
        .await
        .unwrap();
    (r.status().as_u16(), r.json().await.unwrap())
}

async fn card(base: &str) -> CardResponse {
    let r = reqwest::get(format!("{base}/card?sport=nba&date=2026-10-28"))
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    r.json().await.unwrap()
}

fn markets(tape: &Tape) -> BTreeMap<String, Market> {
    let text = std::fs::read_to_string(tape.0.join("markets_selected.json")).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    let list: Vec<Market> = serde_json::from_value(v["data"].clone()).unwrap();
    list.into_iter().map(|m| (m.symbol.clone(), m)).collect()
}

async fn mock_books(mock: &RunningMock, symbols: &[&str]) -> (u64, BTreeMap<String, OrderBook>) {
    let client = pascal_rs::Client::builder()
        .base_url(mock.base_url())
        .build()
        .unwrap();
    let r = client.books(symbols).await.unwrap();
    (r.server_time_ms.unwrap(), r.data.books)
}

#[tokio::test(flavor = "multi_thread")]
async fn spread_and_total_legs_over_the_mock() {
    let tape = Tape::new("pre");
    let mock = mock(&tape, None).await;
    let db = TempDb::new("quoter-lines");
    let d = desk(&mock, &db, key(&tape)).await;
    let base = format!("http://{}", d.addr);

    let c = card(&base).await;
    assert_eq!(c.games.len(), 5);
    let phi = c.games.iter().find(|g| g.symbol == PHI_ML).unwrap();
    assert_eq!(
        (phi.markets_count, phi.markets.len(), phi.in_play),
        (13, 7, false)
    );
    // The other three games have no lines on the tape.
    assert_eq!(c.games.iter().filter(|g| g.markets.len() == 1).count(), 3);

    // 76ers -3.5 and Under 221.5.
    let (status, body) = post_quote(&base, &[(PHI_SP3, "long"), (ORL_TT221, "short")]).await;
    assert_eq!(status, 200, "{body}");
    assert!(body["legs"][0].get("in_play").is_none(), "{body}");
    let resp: QuoteResponse = serde_json::from_value(body).unwrap();
    let m = markets(&tape);
    let (venue_now, books) = mock_books(&mock, &[PHI_SP3, ORL_TT221]).await;
    let legs = [
        pricing::LegInput::from_market(&m[PHI_SP3], pricing::Side::Long).unwrap(),
        pricing::LegInput::from_market(&m[ORL_TT221], pricing::Side::Short).unwrap(),
    ];
    let q = pricing::quote(
        &legs,
        &books,
        10_000_000,
        venue_now,
        &pricing::Params::default(),
    )
    .unwrap();
    assert_eq!(
        (
            resp.payout_u,
            resp.odds_offered_u,
            resp.odds_fair_u,
            resp.min_payout_per_unit_u
        ),
        (
            q.payout_u,
            q.odds_offered_u,
            q.odds_fair_u,
            q.min_payout_per_unit_u
        )
    );
    assert_eq!(
        resp.legs
            .iter()
            .map(|l| (l.price_u, l.p_max_u, l.m1_ppm))
            .collect::<Vec<_>>(),
        q.legs
            .iter()
            .map(|l| (l.price_u, l.p_max_u, l.m1_ppm))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        (resp.legs[0].price_u, resp.legs[1].price_u),
        (629_000, 449_000)
    );
    let named: Vec<_> = resp
        .legs
        .iter()
        .map(|l| (l.kind, l.line.as_deref(), l.label.as_deref(), l.in_play))
        .collect();
    assert_eq!(
        named,
        [
            (
                Some(MarketKind::Spread),
                Some("-3.5"),
                Some("76ers -3.5"),
                false
            ),
            (
                Some(MarketKind::Total),
                Some("221.5"),
                Some("Under 221.5"),
                false
            ),
        ]
    );
    // The signed message is the C3 layout, legs as for any symbol.
    let b64 = base64::engine::general_purpose::STANDARD;
    let msg = b64.decode(&resp.quote_msg).unwrap();
    assert_eq!(msg.len(), 77 + 85 * 2);
    let qm = QuoteMsg::decode(&msg).unwrap();
    assert_eq!(qm.legs[0].symbol_hash, codec::sha256(PHI_SP3.as_bytes()));
    assert_eq!(qm.legs[1].symbol_hash, codec::sha256(ORL_TT221.as_bytes()));
    assert_eq!(qm.legs[1].side, codec::SIDE_SHORT);
    assert_eq!(qm.legs[0].start_ts, (PHI_START / 1000) as i64);
    // The quote store keeps the names for the indexer.
    let stored: String = sqlx::query_scalar("SELECT legs_json FROM quotes")
        .fetch_one(&d.state.db)
        .await
        .unwrap();
    assert!(stored.contains("\"label\":\"Under 221.5\""), "{stored}");

    // One leg per game, whatever the market.
    for legs in [
        [(PHI_ML, "long"), (PHI_SP3, "short")],
        [(PHI_SP3, "long"), (PHI_TT233, "long")],
    ] {
        let (status, body) = post_quote(&base, &legs).await;
        assert_eq!(status, 422);
        let e: ErrorBody = serde_json::from_value(body).unwrap();
        assert_eq!(e.reason, "two legs on game nba-phi-ind-2026-10-28");
    }
    let (status, body) = post_quote(&base, &[("NBA_26OCT28_PHIIND_SP.PHI_9", "long")]).await;
    assert_eq!(status, 422);
    assert!(
        body["reason"]
            .as_str()
            .unwrap()
            .contains("winner, spread and total")
    );

    tokio::time::timeout(Duration::from_secs(10), d.join())
        .await
        .expect("graceful shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn in_play_winner_legs_over_the_mock() {
    let tape = Tape::new("live");
    let mock = mock(&tape, Some(PHI_START + 600_000)).await;
    let db = TempDb::new("quoter-inplay");
    let d = desk(&mock, &db, key(&tape)).await;
    let base = format!("http://{}", d.addr);

    let c = card(&base).await;
    let phi = c.games.iter().find(|g| g.symbol == PHI_ML).unwrap();
    assert!(phi.in_play);
    assert_eq!(phi.started_ms, Some(PHI_START));
    assert_eq!((phi.markets_count, phi.markets.len()), (1, 1));
    let orl = c
        .games
        .iter()
        .find(|g| g.game_id == "nba-orl-tor-2026-10-28")
        .unwrap();
    assert!(!orl.in_play && orl.markets.len() == 7);

    // The Pacers in play, with Under 221.5 on the next game (pre-game, overlapping).
    let (status, body) = post_quote(&base, &[(PHI_ML, "short"), (ORL_TT221, "short")]).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["legs"][0]["in_play"], true, "{body}");
    assert!(body["legs"][1].get("in_play").is_none());
    let resp: QuoteResponse = serde_json::from_value(body).unwrap();
    assert!(resp.legs[0].in_play && resp.legs[0].side == Side::Short);
    assert_eq!(resp.legs[0].label.as_deref(), Some("Pacers"));
    assert!(resp.legs.iter().all(|l| l.overlaps && l.m1_ppm == 50_000));
    // Its signed start is in the past: the program only checks starts for tips and copies.
    let msg = base64::engine::general_purpose::STANDARD
        .decode(&resp.quote_msg)
        .unwrap();
    assert_eq!(
        QuoteMsg::decode(&msg).unwrap().legs[0].start_ts,
        (PHI_START / 1000) as i64
    );

    // Spread and total legs close at tip-off.
    for (s, kind) in [(PHI_SP3, "spread"), (PHI_TT233, "total")] {
        let (status, body) = post_quote(&base, &[(s, "long")]).await;
        assert_eq!(status, 422, "{body}");
        assert_eq!(
            body["reason"],
            format!(
                "game nba-phi-ind-2026-10-28 has already started: {kind} bets close at tip-off"
            )
        );
    }

    tokio::time::timeout(Duration::from_secs(10), d.join())
        .await
        .expect("graceful shutdown");
}
