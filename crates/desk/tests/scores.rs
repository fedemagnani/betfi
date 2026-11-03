//! Live scores on `/card` (31 Oct): on the real 28 Oct soccer listing, a started game is followed
//! by its Polymarket event slug, shows the feed's score while live, and leaves the card once the
//! feed says it ended (or, with no feed, once its book is decided or it is well past Pascal's
//! expected resolution time).

mod common;

use common::repo_root;
use desk::api::types::{CardResponse, GameScore};
use desk::catalogue::{CardLines, Catalogue, card_sport, catalogue_market, with_scores};
use desk::scores::{RESOLUTION_GRACE_MS, Reading, to_follow};
use pascal_rs::Market;
use serde_json::Value;

/// The listing's `state_time_ms` (28 Oct, 18:43 UTC).
const LISTED: u64 = 1_793_212_985_011;
const AVL_GAME: &str = "epl-ast-ful-2026-10-31";
/// Aston Villa – Fulham kicks off at 20:00 UTC on 31 Oct 2026.
const AVL_START: u64 = 1_793_476_800_000;
const DATE: &str = "2026-10-31";

fn catalogue() -> Catalogue {
    let text = std::fs::read_to_string(
        repo_root().join("fixtures/soccer/markets_2026-10-28_soccer.TRIMMED.json"),
    )
    .unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    let list: Vec<Market> = serde_json::from_value(v["data"].clone()).unwrap();
    Catalogue::build(
        LISTED,
        LISTED,
        None,
        list.iter().filter_map(catalogue_market).collect(),
    )
}

fn card(cat: &Catalogue, now: u64, feed: Option<Reading>) -> CardResponse {
    let mut c = card_sport(cat, Some(DATE), now, CardLines::Main, None);
    with_scores(
        &mut c,
        |id| feed.clone().filter(|_| id == AVL_GAME),
        now,
        now,
    );
    c
}

fn avl(c: &CardResponse) -> Option<&desk::api::types::CardGame> {
    c.games.iter().find(|g| g.game_id == AVL_GAME)
}

fn reading(score: Option<(u32, u32)>, live: bool, ended: bool, read_ms: u64) -> Reading {
    Reading {
        score: score.map(|(home, away)| GameScore {
            home,
            away,
            period: Some("2H".into()),
            elapsed: Some("67".into()),
        }),
        live,
        ended,
        read_ms,
    }
}

#[test]
fn started_games_are_followed_by_their_polymarket_event() {
    let cat = catalogue();
    let (follow, all) = to_follow(&cat, AVL_START - 1, |_| false);
    assert!(
        !follow.contains_key("epl-ast-ful-2026-10-31"),
        "not started: {follow:?}"
    );
    // Bayern – Dortmund (17:30) has: Pascal's game id is not Polymarket's slug.
    assert_eq!(
        follow.get("bun-bay-dor-2026-10-31").map(String::as_str),
        Some("bundesliga-fcb-bvb-2026-10-31")
    );
    assert!(all.contains(AVL_GAME));
    let (follow, _) = to_follow(&cat, AVL_START + 60_000, |_| false);
    assert_eq!(
        follow.get("epl-ast-ful-2026-10-31").map(String::as_str),
        Some(AVL_GAME)
    );
    let (follow, _) = to_follow(&cat, AVL_START + 60_000, |id| id == AVL_GAME);
    assert!(
        !follow.contains_key("epl-ast-ful-2026-10-31"),
        "skipped once over"
    );
}

#[test]
fn a_live_game_shows_its_score_and_leaves_the_card_once_over() {
    let cat = catalogue();
    let now = AVL_START + 67 * 60_000;

    // Before the start: no score, even with a reading.
    let before = card(
        &cat,
        AVL_START - 1,
        Some(reading(Some((0, 0)), false, false, AVL_START - 1)),
    );
    assert_eq!(avl(&before).unwrap().score, None);

    // Live: the score.
    let live = card(&cat, now, Some(reading(Some((2, 1)), true, false, now)));
    let g = avl(&live).expect("a live game stays on the card");
    assert!(g.in_play);
    let s = g.score.as_ref().unwrap();
    assert_eq!((s.home, s.away, s.elapsed.as_deref()), (2, 1, Some("67")));

    // A stale reading shows no score (the game stays: its book is open).
    let stale = card(
        &cat,
        now,
        Some(reading(Some((2, 1)), true, false, now - 10 * 60_000)),
    );
    assert_eq!(avl(&stale).unwrap().score, None);

    // No feed: still live on its open book.
    assert!(avl(&card(&cat, now, None)).is_some());

    // The feed says it ended: off the card, the others stay.
    let all = card(&cat, now, None).games.len();
    let ended = card(&cat, now, Some(reading(Some((2, 2)), false, true, now)));
    assert!(avl(&ended).is_none());
    assert_eq!(ended.games.len(), all - 1);

    // No feed, long past the expected resolution time: off the card.
    let exp = avl(&card(&cat, now, None)).unwrap().expected_resolution_ms;
    assert!(avl(&card(&cat, exp + RESOLUTION_GRACE_MS, None)).is_none());
}
