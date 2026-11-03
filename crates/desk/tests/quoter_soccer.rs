//! Review gate (7.0b): soccer 1-X-2 legs in the quoter, the hedge targets and the resolution
//! mapping.
//!
//! - Over the in-process `pascal-mock` frozen on a synthetic tape (the 28 Oct sample, the real
//!   spread and total listings of PHI @ IND and ORL @ TOR, and the real Aston Villa – Fulham
//!   1-X-2 moved onto the tape's night by `desk::catalogue::synthetic::soccer_on_night`):
//!   `/card?sport=` lists the match; `POST /quote` prices an NBA leg plus the draw exactly as
//!   `pricing::quote` does on the mock's books (a long on the draw's own symbol, from its ask),
//!   with the unchanged C3 layout, and a 3-leg NBA + EPL ticket too; a short soccer leg and two
//!   legs on one match are refused.
//! - Without I/O: Pascal's resolution of each outcome symbol (draw at 1, both teams at 0, or a
//!   home win) maps a long leg to won or lost exactly as the program's `resolve_leg` does, and
//!   the hedge desk's targets stay per symbol (each outcome's target comes from its own legs; a
//!   won NBA leg rolls the draw; a lost outcome zeroes its ticket).

mod common;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use base64::Engine;
use common::{TempDb, repo_root};
use desk::api::types::{
    CardResponse, ErrorBody, LegStatus, MarketKind, Outcome, PickTeam, QuoteResponse, Side, Sport,
    TeamSide, TicketStatus,
};
use desk::hedge::targets;
use desk::quoter::codec::{self, QuoteMsg};
use desk::state::{LegState, TicketState};
use desk::{Config, Running};
use pascal_mock::{Mock, MockConfig, RunningMock, ScriptSource};
use pascal_rs::{Market, OrderBook};
use pricing::{RejectCode, UNIT};
use serde_json::{Value, json};

const PHI_ML: &str = "NBA_26OCT28_PHIIND_ML.PHI";
const HOU_ML: &str = "NBA_26OCT28_HOUMIL_ML.HOU";
const ORL_TT221: &str = "NBA_26OCT28_ORLTOR_TT.221";
const HOU_START: u64 = 1_793_232_000_000;
/// The tape's latest start (OKC @ DAL), where the synthetic tape puts the EPL match.
const EPL_START: u64 = 1_793_237_400_000;
const AVL: &str = "EPL_26OCT28_AVLFUL_ML.AVL";
const DRAW: &str = "EPL_26OCT28_AVLFUL_ML.DRAW";
const FUL: &str = "EPL_26OCT28_AVLFUL_ML.FUL";
const EPL_GAME: &str = "epl-ast-ful-2026-10-28";

/// A synthetic tape in a fresh temp dir (removed on drop).
struct Tape(PathBuf);

impl Tape {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "schedina-soccer-{tag}-{}-{}",
            std::process::id(),
            desk::now_ms()
        ));
        let added = desk::catalogue::synthetic::write_tape_with(
            &repo_root().join("tapes/nba-2026-10-28/sample"),
            &repo_root().join("fixtures/lines/markets_2026-10-28_phi-ind_orl-tor.TRIMMED.json"),
            Some(&repo_root().join("fixtures/soccer/markets_2026-10-28_soccer.TRIMMED.json")),
            &dir,
        )
        .unwrap();
        assert_eq!(added.len(), 21 + 3);
        for s in [AVL, DRAW, FUL] {
            assert!(added.iter().any(|a| a == s), "{s}");
        }
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

async fn mock(tape: &Tape) -> RunningMock {
    let mut cfg = MockConfig::new(&tape.0);
    cfg.speed = 0.0;
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

async fn card(base: &str, sport: &str) -> CardResponse {
    let r = reqwest::get(format!("{base}/card?{sport}date=2026-10-28"))
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
async fn soccer_legs_over_the_mock() {
    let tape = Tape::new("quote");
    let mock = mock(&tape).await;
    let db = TempDb::new("quoter-soccer");
    let d = desk(&mock, &db, key(&tape)).await;
    let base = format!("http://{}", d.addr);

    // The card: one soccer game on the night, listed beside the NBA's without `sport`.
    let soccer = card(&base, "sport=soccer&").await;
    assert_eq!(soccer.sport, "soccer");
    assert_eq!(soccer.games.len(), 1);
    let g = &soccer.games[0];
    assert_eq!((g.game_id.as_str(), g.start_ms), (EPL_GAME, EPL_START));
    assert_eq!(g.sport, Sport::Soccer);
    let picks: Vec<_> = g.markets[0]
        .picks
        .iter()
        .map(|p| (p.symbol.as_str(), p.team, p.outcome, p.side))
        .collect();
    assert_eq!(
        picks,
        [
            (AVL, Some(PickTeam::Home), Some(Outcome::Home), Side::Long),
            (DRAW, Some(PickTeam::Draw), Some(Outcome::Draw), Side::Long),
            (FUL, Some(PickTeam::Away), Some(Outcome::Away), Side::Long),
        ]
    );
    let all = card(&base, "").await;
    assert_eq!(all.games.len(), 6);
    assert_eq!(card(&base, "sport=nba&").await.games.len(), 5);

    // Houston (NBA winner) and the draw: two overlapping longs, priced as pricing does.
    let (status, body) = post_quote(&base, &[(HOU_ML, "long"), (DRAW, "long")]).await;
    assert_eq!(status, 200, "{body}");
    let resp: QuoteResponse = serde_json::from_value(body).unwrap();
    let m = markets(&tape);
    let (venue_now, books) = mock_books(&mock, &[HOU_ML, DRAW]).await;
    let legs = [
        pricing::LegInput::from_market(&m[HOU_ML], pricing::Side::Long).unwrap(),
        pricing::LegInput::from_market(&m[DRAW], pricing::Side::Long).unwrap(),
    ];
    assert_eq!(legs[1].market, pricing::LegMarket::ThreeWay);
    let q = pricing::quote(
        &legs,
        &books,
        10_000_000,
        venue_now,
        &pricing::Params::default(),
    )
    .unwrap();
    assert_eq!(
        (resp.payout_u, resp.odds_offered_u, resp.odds_fair_u),
        (q.payout_u, q.odds_offered_u, q.odds_fair_u)
    );
    assert_eq!(
        resp.legs
            .iter()
            .map(|l| (l.price_u, l.p_max_u, l.m1_ppm, l.overlaps))
            .collect::<Vec<_>>(),
        q.legs
            .iter()
            .map(|l| (l.price_u, l.p_max_u, l.m1_ppm, l.overlaps))
            .collect::<Vec<_>>()
    );
    // The draw's price is its own book's ask (the synthetic book's best ask is the listing's).
    assert_eq!(
        resp.legs[1].price_u,
        pricing::ask(&books[DRAW], q.legs[1].size).unwrap()
    );
    assert_eq!(resp.legs[1].price_u, 274_000);
    assert!(resp.legs.iter().all(|l| l.m1_ppm == 50_000 && l.overlaps));
    let named: Vec<_> = resp
        .legs
        .iter()
        .map(|l| {
            (
                l.symbol.as_str(),
                l.game_id.as_str(),
                l.side,
                l.pick,
                l.kind,
                l.line.as_deref(),
                l.label.as_deref(),
                l.start_ms,
            )
        })
        .collect();
    assert_eq!(
        named,
        [
            (
                HOU_ML,
                "nba-hou-mil-2026-10-28",
                Side::Long,
                TeamSide::Away,
                Some(MarketKind::Winner),
                None,
                Some("Rockets"),
                HOU_START
            ),
            // The draw's `pick` is the home placeholder (proposed D-461): read `label`.
            (
                DRAW,
                EPL_GAME,
                Side::Long,
                TeamSide::Home,
                Some(MarketKind::Winner),
                None,
                Some("Draw"),
                EPL_START
            ),
        ]
    );
    // The signed message is the C3 layout: the draw is a long leg like any other.
    let msg = base64::engine::general_purpose::STANDARD
        .decode(&resp.quote_msg)
        .unwrap();
    assert_eq!(msg.len(), 77 + 85 * 2);
    let qm = QuoteMsg::decode(&msg).unwrap();
    assert_eq!(qm.legs[1].symbol_hash, codec::sha256(DRAW.as_bytes()));
    assert_eq!(qm.legs[1].game_id_hash, codec::sha256(EPL_GAME.as_bytes()));
    assert_eq!(qm.legs[1].side, codec::SIDE_LONG);
    assert_eq!(qm.legs[1].start_ts, (EPL_START / 1000) as i64);
    assert_eq!(qm.legs[1].price_u, 274_000);
    let stored: String = sqlx::query_scalar("SELECT legs_json FROM quotes")
        .fetch_one(&d.state.db)
        .await
        .unwrap();
    assert!(stored.contains("\"label\":\"Draw\""), "{stored}");

    // Three legs, NBA and EPL: 76ers, Under 221.5 and Fulham.
    let (status, body) = post_quote(
        &base,
        &[(PHI_ML, "long"), (ORL_TT221, "short"), (FUL, "long")],
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["legs"][2]["label"], "Fulham");
    assert_eq!(body["legs"][2]["side"], "long");

    // Never short a soccer outcome.
    for s in [DRAW, AVL, FUL] {
        let (status, body) = post_quote(&base, &[(HOU_ML, "long"), (s, "short")]).await;
        assert_eq!(status, 422, "{body}");
        let e: ErrorBody = serde_json::from_value(body).unwrap();
        assert_eq!(
            e.reason,
            format!(
                "{s} cannot be laid: a soccer result is backed on its own outcome (home, draw or away), never short"
            )
        );
    }
    // One leg per match: the draw and a team, or both teams, are two legs on one game.
    for legs in [
        [(DRAW, "long"), (AVL, "long")],
        [(AVL, "long"), (FUL, "long")],
    ] {
        let (status, body) = post_quote(&base, &legs).await;
        assert_eq!(status, 422, "{body}");
        assert_eq!(body["reason"], format!("two legs on game {EPL_GAME}"));
    }

    tokio::time::timeout(Duration::from_secs(10), d.join())
        .await
        .expect("graceful shutdown");
}

/// The rules on catalogue legs, without books: a short soccer leg and two legs on one soccer
/// game are refused; NBA + EPL passes; the abbreviations are league-qualified.
#[test]
fn soccer_rules_on_catalogue_legs() {
    let text = std::fs::read_to_string(
        repo_root().join("fixtures/soccer/markets_2026-10-28_soccer.TRIMMED.json"),
    )
    .unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    let list: Vec<Market> = serde_json::from_value(v["data"].clone()).unwrap();
    let cat = desk::catalogue::Catalogue::build(
        0,
        0,
        None,
        list.iter()
            .filter_map(desk::catalogue::catalogue_market)
            .collect(),
    );
    let leg = |s: &str, side: Side| desk::quoter::quote::leg_input(&cat, s, side).unwrap();
    let now = 1_793_440_000_000; // 31 Oct 2026 07:46 UTC, every match later that day
    let caps = pricing::Caps::default();
    let check =
        |legs: &[pricing::LegInput]| pricing::check_correlation(legs, 10_000_000, now, &caps);

    let draw = leg("EPL_26OCT31_AVLFUL_ML.DRAW", Side::Long);
    assert_eq!(draw.market, pricing::LegMarket::ThreeWay);
    assert_eq!(draw.pick(), pricing::Pick::Home);
    assert_eq!(
        (draw.teams.home_abbr.as_str(), draw.teams.away_abbr.as_str()),
        ("EPL:AVL", "EPL:FUL")
    );
    assert_eq!(
        leg("EPL_26OCT31_AVLFUL_ML.FUL", Side::Long).pick(),
        pricing::Pick::Away
    );
    check(std::slice::from_ref(&draw)).unwrap();
    check(&[draw.clone(), leg("BUND_26OCT31_FCBBVB_ML.BVB", Side::Long)]).unwrap();

    let short = check(&[leg("EPL_26OCT31_AVLFUL_ML.AVL", Side::Short)]).unwrap_err();
    assert_eq!(short.code, RejectCode::LongOnly);
    assert_eq!(short.status(), pricing::RejectStatus::Rejected);
    let same = check(&[draw.clone(), leg("EPL_26OCT31_AVLFUL_ML.AVL", Side::Long)]).unwrap_err();
    assert_eq!(
        (same.code, same.reason.as_str()),
        (
            RejectCode::SameGame,
            "two legs on game epl-ast-ful-2026-10-31"
        )
    );
    // In play (D-322): a 1-X-2 leg is a winner, open for 2 h after kick-off.
    let ko = draw.start_ms;
    let at = |t: u64| pricing::check_correlation(std::slice::from_ref(&draw), 10_000_000, t, &caps);
    at(ko + 60 * 60_000).unwrap();
    assert_eq!(
        at(ko + caps.in_play_max_ms).unwrap_err().code,
        RejectCode::AlreadyStarted
    );
    assert!(caps.in_play_max_ms <= pricing::SOCCER_GAME_LENGTH_MS);
    assert!(pricing::Params::default().game_length_ms >= pricing::SOCCER_GAME_LENGTH_MS);
}

fn leg_state(idx: u8, symbol: &str, game_id: &str, start_ms: u64, p: u64) -> LegState {
    LegState {
        idx,
        symbol: symbol.into(),
        game_id: game_id.into(),
        side: Side::Long,
        start_ms,
        p_quote_u: p,
        p_fill_u: Some(p),
        m1_ppm: 50_000,
        target: 0,
        status: LegStatus::Open,
    }
}

fn ticket(id: &str, payout_u: u64, legs: Vec<LegState>) -> TicketState {
    TicketState {
        ticket: id.into(),
        pending: format!("pending-{id}"),
        user: "user".into(),
        status: TicketStatus::Open,
        stake_u: 10_000_000,
        rs_u: Some(10_000_000),
        payout_u,
        legs,
    }
}

/// The status a long leg gets from Pascal's resolution `r` of its symbol, as the program's
/// `resolve_leg` computes it (`r̃ = r` for a long leg; 0 lost, 1 won).
fn long_leg_status(r_u: u64) -> LegStatus {
    targets::rtilde_status(pricing::r_tilde(pricing::Side::Long, r_u))
}

#[test]
fn each_outcome_symbol_resolves_its_own_legs() {
    // The match ends level: Pascal resolves the draw at 1 and both team symbols at 0.
    let level = [(AVL, 0), (DRAW, UNIT), (FUL, 0)];
    let got: Vec<_> = level
        .iter()
        .map(|&(s, r)| (s, long_leg_status(r)))
        .collect();
    assert_eq!(
        got,
        [
            (AVL, LegStatus::Lost),
            (DRAW, LegStatus::Won),
            (FUL, LegStatus::Lost)
        ]
    );
    // A home win: only the home pick wins.
    let home_win = [(AVL, UNIT), (DRAW, 0), (FUL, 0)];
    assert_eq!(
        home_win
            .iter()
            .map(|&(_, r)| long_leg_status(r))
            .collect::<Vec<_>>(),
        [LegStatus::Won, LegStatus::Lost, LegStatus::Lost]
    );
    // Settlement pays P × Π r̃: the NBA leg and the draw both won → P; the draw lost → 0.
    assert_eq!(
        pricing::settle(40_000_000, &[UNIT, UNIT]).unwrap(),
        40_000_000
    );
    assert_eq!(pricing::settle(40_000_000, &[UNIT, 0]).unwrap(), 0);

    // The hedge desk: three tickets, one per outcome of the match (the first with an NBA leg
    // before it). Each outcome symbol's net target comes from its own legs only.
    let mut tickets: HashMap<String, TicketState> = HashMap::new();
    let p = 40_000_000;
    tickets.insert(
        "d".into(),
        ticket(
            "d",
            p,
            vec![
                leg_state(0, HOU_ML, "nba-hou-mil-2026-10-28", HOU_START, 300_000),
                leg_state(1, DRAW, EPL_GAME, EPL_START, 274_000),
            ],
        ),
    );
    tickets.insert(
        "h".into(),
        ticket(
            "h",
            20_000_000,
            vec![leg_state(0, AVL, EPL_GAME, EPL_START, 490_000)],
        ),
    );
    tickets.insert(
        "a".into(),
        ticket(
            "a",
            30_000_000,
            vec![leg_state(0, FUL, EPL_GAME, EPL_START, 264_000)],
        ),
    );
    let gl = pricing::Params::default().game_length_ms;
    let rt = targets::RTilde::new();
    let target = |t: &HashMap<String, TicketState>, s: &str, rt: &targets::RTilde| {
        targets::symbol_target(t, s, rt, gl).0
    };
    // Both legs of "d" overlap (1.5 h apart): both hedged at inception, Δ = P · Π other q:
    // 40 × 0.30 = 12 on the draw, 40 × 0.274 = 10.96 → 11 on Houston.
    assert_eq!(target(&tickets, DRAW, &rt), 12);
    assert_eq!(target(&tickets, HOU_ML, &rt), 11);
    assert_eq!(target(&tickets, AVL, &rt), 20);
    assert_eq!(target(&tickets, FUL, &rt), 30);

    // The NBA leg wins: the draw rolls to P (Π of the resolved r̃ = 1).
    let mut rt = targets::RTilde::new();
    rt.insert(("d".into(), 0), UNIT);
    tickets.get_mut("d").unwrap().legs[0].status = long_leg_status(UNIT);
    assert_eq!(target(&tickets, DRAW, &rt), 40);
    assert_eq!(target(&tickets, HOU_ML, &rt), 0);
    assert_eq!(target(&tickets, AVL, &rt), 20);

    // The match ends level: the draw leg wins, both team legs lose; every target goes to 0.
    for (id, idx, symbol) in [("d", 1_u8, DRAW), ("h", 0, AVL), ("a", 0, FUL)] {
        let r = level.iter().find(|(s, _)| *s == symbol).unwrap().1;
        let rtilde = pricing::r_tilde(pricing::Side::Long, r);
        rt.insert((id.into(), idx), rtilde);
        tickets.get_mut(id).unwrap().legs[usize::from(idx)].status = targets::rtilde_status(rtilde);
    }
    for s in [HOU_ML, AVL, DRAW, FUL] {
        assert_eq!(target(&tickets, s, &rt), 0, "{s}");
    }
    assert_eq!(tickets["d"].legs[1].status, LegStatus::Won);
    assert_eq!(tickets["h"].legs[0].status, LegStatus::Lost);
    assert_eq!(tickets["a"].legs[0].status, LegStatus::Lost);
}
