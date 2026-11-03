//! Review gate (7.0b): soccer 1-X-2 in the catalogue and on `/card`, on the real 28 Oct listing
//! of three EPL matches, one Bundesliga and one LaLiga match
//! (`fixtures/soccer/markets_2026-10-28_soccer.TRIMMED.json`): parse, grouping by game (three
//! outcome symbols, the draw recorded), home and away from the symbol and the display
//! attributes, start times, the pick mapping 1 / X / 2, and `?sport=` filtering.
//! `fixtures/desk/card_soccer.json` is the soccer card of 31 Oct (set `BLESS=1` to rewrite it
//! after a reviewed change).

mod common;

use common::repo_root;
use desk::api::types::{
    CardResponse, League, MarketKind, Outcome, PickTeam, Side, Sport, TeamSide,
};
use desk::catalogue::{
    CardLines, Catalogue, CatalogueMarket, card_at, card_sport, catalogue_market, nba_market,
    soccer_market,
};
use pascal_rs::Market;
use serde_json::Value;

/// The listing's `state_time_ms` (28 Oct, 18:43 UTC).
const LISTED: u64 = 1_793_212_985_011;
const AVL_GAME: &str = "epl-ast-ful-2026-10-31";
/// Aston Villa – Fulham, Saturday 31 Oct 2026 20:00 UTC (20:00 in Birmingham: GMT since 25 Oct).
/// A European match's game-id date is its local date, which is also its UTC date.
const AVL_START: u64 = 1_793_476_800_000;

fn listing() -> Vec<Market> {
    let text = std::fs::read_to_string(
        repo_root().join("fixtures/soccer/markets_2026-10-28_soccer.TRIMMED.json"),
    )
    .unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    serde_json::from_value(v["data"].clone()).unwrap()
}

fn catalogue_of(list: &[Market]) -> Catalogue {
    Catalogue::build(
        LISTED,
        LISTED,
        None,
        list.iter().filter_map(catalogue_market).collect(),
    )
}

/// Each outcome of every match: symbol, game id, league, team picked (draw = None), whether it
/// is the draw, home abbr, away abbr, start, ask (the long price). Read off the listing by hand.
#[allow(clippy::type_complexity)]
const TABLE: [(&str, &str, League, Option<TeamSide>, &str, &str, u64, u64); 15] = [
    (
        "EPL_26OCT31_AVLFUL_ML.AVL",
        AVL_GAME,
        League::Epl,
        Some(TeamSide::Home),
        "AVL",
        "FUL",
        AVL_START,
        490_000,
    ),
    (
        "EPL_26OCT31_AVLFUL_ML.DRAW",
        AVL_GAME,
        League::Epl,
        None,
        "AVL",
        "FUL",
        AVL_START,
        274_000,
    ),
    (
        "EPL_26OCT31_AVLFUL_ML.FUL",
        AVL_GAME,
        League::Epl,
        Some(TeamSide::Away),
        "AVL",
        "FUL",
        AVL_START,
        264_000,
    ),
    (
        "EPL_26OCT31_MCIBHA_ML.MCI",
        "epl-mac-bri-2026-10-31",
        League::Epl,
        Some(TeamSide::Home),
        "MCI",
        "BHA",
        1_793_458_800_000,
        590_000,
    ),
    (
        "EPL_26OCT31_MCIBHA_ML.DRAW",
        "epl-mac-bri-2026-10-31",
        League::Epl,
        None,
        "MCI",
        "BHA",
        1_793_458_800_000,
        220_000,
    ),
    (
        "EPL_26OCT31_MCIBHA_ML.BHA",
        "epl-mac-bri-2026-10-31",
        League::Epl,
        Some(TeamSide::Away),
        "MCI",
        "BHA",
        1_793_458_800_000,
        210_000,
    ),
    (
        "EPL_26OCT31_HULIPS_ML.HUL",
        "epl-hul-ips-2026-10-31",
        League::Epl,
        Some(TeamSide::Home),
        "HUL",
        "IPS",
        1_793_458_800_000,
        365_000,
    ),
    (
        "EPL_26OCT31_HULIPS_ML.DRAW",
        "epl-hul-ips-2026-10-31",
        League::Epl,
        None,
        "HUL",
        "IPS",
        1_793_458_800_000,
        290_000,
    ),
    (
        "EPL_26OCT31_HULIPS_ML.IPS",
        "epl-hul-ips-2026-10-31",
        League::Epl,
        Some(TeamSide::Away),
        "HUL",
        "IPS",
        1_793_458_800_000,
        360_000,
    ),
    (
        "BUND_26OCT31_FCBBVB_ML.FCB",
        "bundesliga-fcb-bvb-2026-10-31",
        League::Bundesliga,
        Some(TeamSide::Home),
        "FCB",
        "BVB",
        1_793_467_800_000,
        734_000,
    ),
    (
        "BUND_26OCT31_FCBBVB_ML.DRAW",
        "bundesliga-fcb-bvb-2026-10-31",
        League::Bundesliga,
        None,
        "FCB",
        "BVB",
        1_793_467_800_000,
        173_000,
    ),
    (
        "BUND_26OCT31_FCBBVB_ML.BVB",
        "bundesliga-fcb-bvb-2026-10-31",
        League::Bundesliga,
        Some(TeamSide::Away),
        "FCB",
        "BVB",
        1_793_467_800_000,
        133_000,
    ),
    (
        "LALIGA_26OCT31_BARALA_ML.BAR",
        "lal-bar-ala-2026-10-31",
        League::LaLiga,
        Some(TeamSide::Home),
        "BAR",
        "ALA",
        AVL_START,
        892_000,
    ),
    (
        "LALIGA_26OCT31_BARALA_ML.DRAW",
        "lal-bar-ala-2026-10-31",
        League::LaLiga,
        None,
        "BAR",
        "ALA",
        AVL_START,
        82_000,
    ),
    (
        "LALIGA_26OCT31_BARALA_ML.ALA",
        "lal-bar-ala-2026-10-31",
        League::LaLiga,
        Some(TeamSide::Away),
        "BAR",
        "ALA",
        AVL_START,
        43_000,
    ),
];

#[test]
fn parses_the_real_soccer_listing() {
    let list = listing();
    assert_eq!(list.len(), 18);
    // Both teams to score, the spread and the total goals of Aston Villa – Fulham are out of
    // scope (D-435); nothing soccer is an NBA market.
    let parsed: Vec<CatalogueMarket> = list.iter().filter_map(soccer_market).collect();
    assert_eq!(parsed.len(), 15);
    assert!(list.iter().all(|m| nba_market(m).is_none()));
    for s in [
        "EPL_26OCT31_AVLFUL_PP.BTTS",
        "EPL_26OCT31_AVLFUL_SP.AVL_1",
        "EPL_26OCT31_AVLFUL_TT.2",
    ] {
        assert!(parsed.iter().all(|m| m.symbol != s), "{s}");
    }

    let cat = catalogue_of(&list);
    assert_eq!(cat.markets.len(), 15);
    assert!(cat.games.is_empty() && cat.lines.is_empty());
    assert_eq!(cat.soccer.len(), 5);
    let g = &cat.soccer[AVL_GAME];
    assert_eq!(
        g.symbols(),
        [
            "EPL_26OCT31_AVLFUL_ML.AVL",
            "EPL_26OCT31_AVLFUL_ML.DRAW",
            "EPL_26OCT31_AVLFUL_ML.FUL"
        ]
    );
    // The draw is listed first among Hull – Ipswich's symbols and the away win first among
    // Man City – Brighton's: the outcome, not the symbol order, decides the slot.
    assert_eq!(
        cat.soccer["epl-hul-ips-2026-10-31"].symbols(),
        [
            "EPL_26OCT31_HULIPS_ML.HUL",
            "EPL_26OCT31_HULIPS_ML.DRAW",
            "EPL_26OCT31_HULIPS_ML.IPS"
        ]
    );
    assert_eq!(
        cat.soccer["epl-mac-bri-2026-10-31"].away,
        "EPL_26OCT31_MCIBHA_ML.BHA"
    );

    for (symbol, game_id, league, team, home, away, start, ask) in TABLE {
        let m = cat.market(symbol).unwrap();
        assert_eq!(m.game_id, game_id, "{symbol}");
        assert_eq!(m.league, league, "{symbol}");
        assert_eq!(m.league.sport(), Sport::Soccer, "{symbol}");
        assert_eq!(m.kind, MarketKind::Winner, "{symbol}");
        assert!(m.is_three_way(), "{symbol}");
        assert_eq!(m.draw, team.is_none(), "{symbol}");
        // The draw's long team is the home placeholder (proposed D-461).
        assert_eq!(m.long_team, team.unwrap_or(TeamSide::Home), "{symbol}");
        assert_eq!((m.home.abbr.as_str(), m.away.abbr.as_str()), (home, away));
        assert_eq!(m.start_ms, start, "{symbol}");
        // Pascal lists every soccer winner as resolving 2 h after the start (7.0b).
        assert_eq!(
            m.expected_resolution_ms - m.start_ms,
            pricing::SOCCER_GAME_LENGTH_MS,
            "{symbol}"
        );
        assert_eq!(m.indicative_price_u(Side::Long), Some(ask), "{symbol}");
        assert_eq!(m.date(), Some("2026-10-31"));
        assert_eq!(m.taker_fee_rate_ppm, 20_000);
        assert_eq!(m.tick_size_min_u, 1_000);
    }

    let avl = cat.market("EPL_26OCT31_AVLFUL_ML.AVL").unwrap();
    assert_eq!(
        (avl.home.name.as_str(), avl.away.name.as_str()),
        ("Aston Villa FC", "Fulham FC")
    );
    assert_eq!(
        (avl.home.short.as_str(), avl.away.short.as_str()),
        ("Aston Villa", "Fulham")
    );
    assert_eq!(
        (avl.home.subject_id.as_str(), avl.away.subject_id.as_str()),
        ("aston-villa", "fulham")
    );
    let bvb = cat.market("BUND_26OCT31_FCBBVB_ML.BVB").unwrap();
    assert_eq!(
        (bvb.home.short.as_str(), bvb.away.short.as_str()),
        ("Bayern München", "BV Borussia 09 Dortmund")
    );
}

#[test]
fn picks_map_to_1_x_2() {
    let cat = catalogue_of(&listing());
    let [home, draw, away] = cat.soccer_game(AVL_GAME).unwrap();
    // Every pick is a long on its own symbol; the short side backs two outcomes, never offered.
    for (m, team, outcome, label) in [
        (home, PickTeam::Home, Outcome::Home, "Aston Villa"),
        (draw, PickTeam::Draw, Outcome::Draw, "Draw"),
        (away, PickTeam::Away, Outcome::Away, "Fulham"),
    ] {
        assert_eq!(m.pick_team(Side::Long), Some(team), "{}", m.symbol);
        assert_eq!(m.outcome(Side::Long), Some(outcome), "{}", m.symbol);
        assert_eq!(m.label(Side::Long), label);
        assert_eq!(m.pick_team(Side::Short), None);
        assert_eq!(m.outcome(Side::Short), None);
        assert_eq!(m.label(Side::Short), format!("Not {label}"));
    }
    assert_eq!(
        serde_json::to_value([Outcome::Home, Outcome::Draw, Outcome::Away]).unwrap(),
        serde_json::json!(["1", "X", "2"])
    );
    assert_eq!(
        serde_json::to_value([PickTeam::Home, PickTeam::Draw, PickTeam::Away]).unwrap(),
        serde_json::json!(["home", "draw", "away"])
    );
}

#[test]
fn the_soccer_card() {
    let cat = catalogue_of(&listing());
    let card = card_sport(
        &cat,
        Some("2026-10-31"),
        LISTED,
        CardLines::Main,
        Some(Sport::Soccer),
    );
    assert_eq!(card.sport, "soccer");
    assert_eq!(card.games.len(), 5);
    // By start time, then game id.
    assert_eq!(
        card.games
            .iter()
            .map(|g| g.game_id.as_str())
            .collect::<Vec<_>>(),
        [
            "epl-hul-ips-2026-10-31",
            "epl-mac-bri-2026-10-31",
            "bundesliga-fcb-bvb-2026-10-31",
            AVL_GAME,
            "lal-bar-ala-2026-10-31",
        ]
    );
    let avl = card.games.iter().find(|g| g.game_id == AVL_GAME).unwrap();
    assert_eq!((avl.sport, avl.league), (Sport::Soccer, League::Epl));
    assert_eq!(avl.symbol, "EPL_26OCT31_AVLFUL_ML.AVL");
    assert_eq!(avl.long_team, TeamSide::Home);
    assert_eq!(
        (avl.start_ms, avl.in_play, avl.started_ms),
        (AVL_START, false, None)
    );
    assert_eq!(
        (avl.home.name.as_str(), avl.home.abbr.as_str()),
        ("Aston Villa FC", "AVL")
    );
    assert_eq!(
        (avl.away.name.as_str(), avl.away.abbr.as_str()),
        ("Fulham FC", "FUL")
    );
    assert!(avl.picks.is_empty());
    assert_eq!((avl.markets_count, avl.markets.len()), (1, 1));
    let w = &avl.markets[0];
    assert_eq!(w.kind, MarketKind::Winner);
    assert_eq!(w.line, None);
    assert_eq!(w.market_title.as_deref(), Some("Full Time Result (1X2)"));
    let shown: Vec<_> = w
        .picks
        .iter()
        .map(|p| {
            (
                p.label.as_str(),
                p.team,
                p.outcome,
                p.symbol.as_str(),
                p.side,
                p.price_u,
                p.odds_u,
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            (
                "Aston Villa",
                Some(PickTeam::Home),
                Some(Outcome::Home),
                "EPL_26OCT31_AVLFUL_ML.AVL",
                Side::Long,
                490_000,
                2_040_816
            ),
            (
                "Draw",
                Some(PickTeam::Draw),
                Some(Outcome::Draw),
                "EPL_26OCT31_AVLFUL_ML.DRAW",
                Side::Long,
                274_000,
                3_649_635
            ),
            (
                "Fulham",
                Some(PickTeam::Away),
                Some(Outcome::Away),
                "EPL_26OCT31_AVLFUL_ML.FUL",
                Side::Long,
                264_000,
                3_787_878
            ),
        ]
    );

    // Kick-off: the game is in play (its winner is the only market anyway).
    let live = card_sport(&cat, Some("2026-10-31"), AVL_START, CardLines::Main, None);
    assert_eq!(live.sport, "all");
    let g = live.games.iter().find(|g| g.game_id == AVL_GAME).unwrap();
    assert!(g.in_play && g.started_ms == Some(AVL_START) && g.markets.len() == 1);

    // The NBA card has no soccer game; another date has none.
    assert!(
        card_at(&cat, Some("2026-10-31"), LISTED, CardLines::Main)
            .games
            .is_empty()
    );
    let basketball = card_sport(
        &cat,
        Some("2026-10-31"),
        LISTED,
        CardLines::Main,
        Some(Sport::Basketball),
    );
    assert!(basketball.games.is_empty());
    assert!(
        card_sport(&cat, Some("2026-10-30"), LISTED, CardLines::Main, None)
            .games
            .is_empty()
    );

    let path = repo_root().join("fixtures/desk/card_soccer.json");
    let got = serde_json::to_value(&card).unwrap();
    if std::env::var_os("BLESS").is_some() {
        let mut text = serde_json::to_string_pretty(&got).unwrap();
        text.push('\n');
        std::fs::write(&path, text).unwrap();
    }
    let want: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        got, want,
        "card differs from fixtures/desk/card_soccer.json"
    );
    let typed: CardResponse = serde_json::from_value(want).unwrap();
    assert_eq!(typed, card);
}

#[test]
fn a_game_without_all_three_outcomes_stays_off() {
    let mut list = listing();
    list.retain(|m| m.symbol != "EPL_26OCT31_AVLFUL_ML.DRAW");
    // An unpriced outcome keeps its game in the catalogue but off the card.
    for m in &mut list {
        if m.symbol == "EPL_26OCT31_HULIPS_ML.IPS" {
            m.best_ask = None;
            m.mark_price = None;
        }
    }
    let cat = catalogue_of(&list);
    assert!(!cat.soccer.contains_key(AVL_GAME));
    assert!(cat.market("EPL_26OCT31_AVLFUL_ML.AVL").is_none());
    assert!(cat.soccer.contains_key("epl-hul-ips-2026-10-31"));
    let card = card_sport(&cat, Some("2026-10-31"), LISTED, CardLines::Main, None);
    assert_eq!(
        card.games
            .iter()
            .map(|g| g.game_id.as_str())
            .collect::<Vec<_>>(),
        [
            "epl-mac-bri-2026-10-31",
            "bundesliga-fcb-bvb-2026-10-31",
            "lal-bar-ala-2026-10-31",
        ]
    );
}

#[test]
fn other_soccer_markets_and_periods_are_refused() {
    let base = listing()
        .into_iter()
        .find(|m| m.symbol == "EPL_26OCT31_AVLFUL_ML.DRAW")
        .unwrap();
    assert!(soccer_market(&base).is_some());
    let mut half = base.clone();
    if let Some(pascal_rs::MarketType::Sports(s)) = &mut half.display_attributes.market_type {
        s.period = Some(pascal_rs::SportPeriod::FirstHalf);
    }
    assert!(soccer_market(&half).is_none());
    let mut resolved = base.clone();
    resolved.resolution = Some(pascal_rs::MarketResolution {
        price: 1_000_000,
        seq: 1,
        market_resolution_ts_ms: 1,
    });
    assert!(soccer_market(&resolved).is_none());
    let mut other_league = base.clone();
    other_league.symbol = "LAL_26OCT10_RAYATH_ML.DRAW".into();
    assert!(soccer_market(&other_league).is_none());
    let mut no_start = base.clone();
    no_start.display_attributes.expected_event_start_time_ms = None;
    assert!(soccer_market(&no_start).is_none());
    let mut no_game = base;
    no_game.display_attributes.game_page = None;
    assert!(soccer_market(&no_game).is_none());
}

/// The real 28 Oct listing of two NBA games with their lines
/// (`fixtures/lines/markets_2026-10-28_phi-ind_orl-tor.TRIMMED.json`), on another day than the
/// soccer listing's matches.
fn nba_listing() -> Vec<Market> {
    let text = std::fs::read_to_string(
        repo_root().join("fixtures/lines/markets_2026-10-28_phi-ind_orl-tor.TRIMMED.json"),
    )
    .unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    serde_json::from_value(v["data"].clone()).unwrap()
}

/// 2 Nov: without a date the card holds every game not over, whatever its day (the NBA games of
/// 28 Oct and the soccer matches of 31 Oct here), by start time; with one, that day's only.
#[test]
fn without_a_date_the_card_holds_every_day() {
    let mut list = listing();
    list.extend(nba_listing());
    let cat = catalogue_of(&list);
    let day = |d| card_sport(&cat, Some(d), LISTED, CardLines::Main, None);
    let (nba, soccer) = (day("2026-10-28"), day("2026-10-31"));
    assert!(!nba.games.is_empty() && !soccer.games.is_empty());
    assert!(nba.games.iter().all(|g| g.sport == Sport::Basketball));
    assert!(soccer.games.iter().all(|g| g.sport == Sport::Soccer));

    let all = card_sport(&cat, None, LISTED, CardLines::Main, None);
    assert_eq!(all.date, "");
    assert_eq!(all.games.len(), nba.games.len() + soccer.games.len());
    assert!(all.games.windows(2).all(|w| w[0].start_ms <= w[1].start_ms));
}

/// 3 Nov: the Champions League (`UCL_`) on the soccer card, from the real listing of that
/// morning: both matches with their three outcomes (both-teams-to-score, spreads and totals
/// left out), named "Champions League" on the wire, team codes from the symbols even when the
/// home code is four letters (`AEK1`).
#[test]
fn the_champions_league_is_on_the_soccer_card() {
    let text = std::fs::read_to_string(
        repo_root().join("fixtures/soccer/markets_2026-11-03_ucl.TRIMMED.json"),
    )
    .unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    let list: Vec<Market> = serde_json::from_value(v["data"].clone()).unwrap();
    let listed: u64 = v["state_time_ms"].as_str().unwrap().parse().unwrap();
    let parsed: Vec<CatalogueMarket> = list.iter().filter_map(catalogue_market).collect();
    assert_eq!(parsed.len(), 6, "three outcomes per match");
    assert!(parsed.iter().all(|m| m.league == League::Ucl));
    let cat = Catalogue::build(listed, listed, None, parsed);
    let card = card_sport(&cat, None, listed, CardLines::Main, Some(Sport::Soccer));
    let ids: Vec<&str> = card.games.iter().map(|g| g.game_id.as_str()).collect();
    assert_eq!(ids, ["ucl-atm-bay-2026-11-03", "ucl-aek1-rma-2026-11-04"]);
    let atm = &card.games[0];
    assert_eq!((atm.sport, atm.league), (Sport::Soccer, League::Ucl));
    assert_eq!(
        (
            atm.home.name.as_str(),
            atm.home.abbr.as_str(),
            atm.away.name.as_str(),
            atm.away.abbr.as_str()
        ),
        ("Atlético Madrid", "ATM", "Bayern Munich", "BAY")
    );
    let aek = &card.games[1];
    assert_eq!(
        (aek.home.abbr.as_str(), aek.away.abbr.as_str()),
        ("AEK1", "RMA")
    );
    let wire = serde_json::to_value(atm).unwrap();
    assert_eq!(wire["league"], "Champions League");
}
