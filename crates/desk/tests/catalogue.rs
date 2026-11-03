//! Review gate: the desk, pointed at an in-process Pascal stub serving the 28 Oct tape, answers
//! `/card?sport=nba&date=2026-10-28` with the tape's five games and both picks, byte-for-byte as
//! `fixtures/desk/card.json` (computed independently from the tape). No network.

mod common;

use std::time::Duration;

use common::{TempDb, pascal_stub, repo_root, start_desk};
use desk::api::types::{
    CardResponse, ErrorBody, ErrorStatus, League, Outcome, Side, Sport, TeamSide,
};
use serde_json::Value;

async fn get(url: &str) -> (u16, Value) {
    let r = reqwest::get(url).await.expect("desk answers");
    let status = r.status().as_u16();
    (status, r.json().await.expect("json body"))
}

#[tokio::test(flavor = "multi_thread")]
async fn card_from_the_tape() {
    let pascal = pascal_stub().await;
    let db = TempDb::new("catalogue");
    let desk = start_desk(&pascal, &db).await;
    let base = format!("http://{}", desk.addr);
    let card_url = format!("{base}/card?sport=nba&date=2026-10-28");

    // The first catalogue pass runs at start; wait for it.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let card = loop {
        let (status, body) = get(&card_url).await;
        if status == 200 {
            break body;
        }
        assert_eq!(status, 503, "{body}");
        assert!(
            tokio::time::Instant::now() < deadline,
            "catalogue never loaded"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    let typed: CardResponse = serde_json::from_value(card.clone()).unwrap();
    assert_eq!(typed.games.len(), 5);
    for g in &typed.games {
        assert_eq!(g.picks.len(), 2, "{}", g.game_id);
        assert_eq!(g.picks[0].pick, TeamSide::Away);
        assert_eq!(g.picks[1].pick, TeamSide::Home);
        // L7-1: 1-X-2 codes even on the NBA (2 = away, 1 = home), the sport and the league.
        assert_eq!(g.picks[0].outcome, Some(Outcome::Away));
        assert_eq!(g.picks[1].outcome, Some(Outcome::Home));
        assert_eq!((g.sport, g.league), (Sport::Basketball, League::Nba));
        assert_eq!(g.markets[0].market_title.as_deref(), Some("Moneyline"));
        // D-021: one symbol, the two picks are opposite sides of it.
        assert_ne!(g.picks[0].side, g.picks[1].side);
        for p in &g.picks {
            assert!(p.price_u > 0 && p.price_u < 1_000_000);
            assert_eq!(p.odds_u, 1_000_000_000_000 / p.price_u);
        }
    }
    let phi = &typed.games[0];
    assert_eq!(phi.game_id, "nba-phi-ind-2026-10-28");
    assert_eq!(phi.picks[0].side, Side::Long);
    assert_eq!(
        (phi.picks[0].price_u, phi.picks[0].odds_u),
        (699_000, 1_430_615)
    );
    assert_eq!(
        (phi.picks[1].price_u, phi.picks[1].odds_u),
        (309_000, 3_236_245)
    );
    assert_eq!(typed.as_of_ms, 1_793_212_985_011);

    let fixture: Value = serde_json::from_str(
        &std::fs::read_to_string(repo_root().join("fixtures/desk/card.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(card, fixture, "card differs from fixtures/desk/card.json");

    // `?sport=` (L7-1): basketball is the NBA card, soccer has no game on this tape, and
    // without the parameter every sport is listed.
    for (q, sport, n) in [
        ("sport=basketball&", "basketball", 5),
        ("sport=soccer&", "soccer", 0),
        ("", "all", 5),
    ] {
        let (status, body) = get(&format!("{base}/card?{q}date=2026-10-28")).await;
        assert_eq!(status, 200, "{q}");
        assert_eq!(body["sport"], sport);
        assert_eq!(body["games"].as_array().unwrap().len(), n, "{q}");
        if n > 0 {
            assert_eq!(body["games"], card["games"], "{q}");
        }
    }

    // Another date: no games. No date, or an empty one (2 Nov): every game not over, whatever its
    // day (the tape's five). Bad parameters: C4 422 bodies.
    let (status, body) = get(&format!("{base}/card?sport=nba&date=2026-10-29")).await;
    assert_eq!(status, 200);
    assert_eq!(body["games"], Value::Array(vec![]));
    for q in ["sport=nba", "sport=nba&date="] {
        let (status, body) = get(&format!("{base}/card?{q}")).await;
        assert_eq!(status, 200, "{q}");
        assert_eq!(body["date"], "", "{q}");
        assert_eq!(body["games"], card["games"], "{q}");
    }
    for q in [
        "sport=nfl&date=2026-10-28",
        "sport=&date=2026-10-28",
        "sport=nba&date=28-10-2026",
    ] {
        let (status, body) = get(&format!("{base}/card?{q}")).await;
        assert_eq!(status, 422, "{q}");
        let e: ErrorBody = serde_json::from_value(body).unwrap();
        assert_eq!(e.status, ErrorStatus::Rejected);
    }

    // The pass was written to `markets`.
    let rows: Vec<(String, i64, i64)> =
        sqlx::query_as("SELECT symbol, live, taker_fee_rate_ppm FROM markets ORDER BY symbol")
            .fetch_all(&desk.state.db)
            .await
            .unwrap();
    assert_eq!(rows.len(), 5);
    assert!(
        rows.iter()
            .all(|(_, live, fee)| *live == 1 && *fee == 20_000)
    );

    // Health and metrics answer.
    let (status, health) = get(&format!("{base}/health")).await;
    assert_eq!(status, 200);
    assert_eq!(health["catalogue"]["markets"], 5);
    let metrics = reqwest::get(format!("{base}/metrics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(metrics.contains("desk_catalogue_markets"), "{metrics}");

    tokio::time::timeout(Duration::from_secs(10), desk.join())
        .await
        .expect("graceful shutdown");
}
