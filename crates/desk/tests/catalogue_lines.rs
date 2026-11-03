//! Review gate (6.0b): spreads and totals in the catalogue and on `/card`, on the real 28 Oct
//! listing of PHI @ IND (13 markets) and ORL @ TOR (10)
//! (`fixtures/lines/markets_2026-10-28_phi-ind_orl-tor.TRIMMED.json`): grouping by game, lines,
//! labels, both sides of every binary market, the card's line subset (proposed D-320) and the in-play card (U-2). `fixtures/desk/card_lines.json` is
//! the card at PHI @ IND's tip-off + 20 min (set `BLESS=1` to rewrite it after a reviewed
//! change).

mod common;

use common::repo_root;
use desk::api::types::{
    CardResponse, League, MarketKind, Outcome, PickTeam, Side, Sport, TeamSide,
};
use desk::catalogue::{
    CARD_LINES, CardLines, Catalogue, CatalogueMarket, card_at, nba_market, nba_moneyline,
};
use pascal_rs::Market;
use serde_json::Value;

const PHI_GAME: &str = "nba-phi-ind-2026-10-28";
const ORL_GAME: &str = "nba-orl-tor-2026-10-28";
const PHI_START: u64 = 1_793_228_400_000;
const ORL_START: u64 = 1_793_230_200_000;
/// The listing's `state_time_ms`.
const LISTED: u64 = 1_793_212_985_011;

fn listing() -> Vec<Market> {
    let text = std::fs::read_to_string(
        repo_root().join("fixtures/lines/markets_2026-10-28_phi-ind_orl-tor.TRIMMED.json"),
    )
    .unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    serde_json::from_value(v["data"].clone()).unwrap()
}

fn catalogue() -> Catalogue {
    let list = listing().iter().filter_map(nba_market).collect();
    Catalogue::build(LISTED, LISTED, None, list)
}

/// Every PHI @ IND market: symbol, kind, line, long team (`None` on a total), long label,
/// short label, long price (ask), short price (1 − bid). Read off the listing by hand.
#[allow(clippy::type_complexity)]
const PHI_TABLE: [(
    &str,
    MarketKind,
    Option<&str>,
    Option<TeamSide>,
    &str,
    &str,
    u64,
    u64,
); 13] = [
    (
        "NBA_26OCT28_PHIIND_ML.PHI",
        MarketKind::Winner,
        None,
        Some(TeamSide::Away),
        "76ers",
        "Pacers",
        699_000,
        309_000,
    ),
    (
        "NBA_26OCT28_PHIIND_SP.IND_1",
        MarketKind::Spread,
        Some("-1.5"),
        Some(TeamSide::Home),
        "Pacers -1.5",
        "76ers +1.5",
        279_000,
        740_000,
    ),
    (
        "NBA_26OCT28_PHIIND_SP.PHI_1",
        MarketKind::Spread,
        Some("-1.5"),
        Some(TeamSide::Away),
        "76ers -1.5",
        "Pacers +1.5",
        679_000,
        335_000,
    ),
    (
        "NBA_26OCT28_PHIIND_SP.PHI_2",
        MarketKind::Spread,
        Some("-2.5"),
        Some(TeamSide::Away),
        "76ers -2.5",
        "Pacers +2.5",
        659_000,
        349_000,
    ),
    (
        "NBA_26OCT28_PHIIND_SP.PHI_3",
        MarketKind::Spread,
        Some("-3.5"),
        Some(TeamSide::Away),
        "76ers -3.5",
        "Pacers +3.5",
        629_000,
        379_000,
    ),
    (
        "NBA_26OCT28_PHIIND_SP.PHI_4",
        MarketKind::Spread,
        Some("-4.5"),
        Some(TeamSide::Away),
        "76ers -4.5",
        "Pacers +4.5",
        599_000,
        409_000,
    ),
    (
        "NBA_26OCT28_PHIIND_SP.PHI_5",
        MarketKind::Spread,
        Some("-5.5"),
        Some(TeamSide::Away),
        "76ers -5.5",
        "Pacers +5.5",
        559_000,
        449_000,
    ),
    (
        "NBA_26OCT28_PHIIND_SP.PHI_6",
        MarketKind::Spread,
        Some("-6.5"),
        Some(TeamSide::Away),
        "76ers -6.5",
        "Pacers +6.5",
        539_000,
        469_000,
    ),
    (
        "NBA_26OCT28_PHIIND_TT.233",
        MarketKind::Total,
        Some("233.5"),
        None,
        "Over 233.5",
        "Under 233.5",
        509_000,
        499_000,
    ),
    (
        "NBA_26OCT28_PHIIND_TT.235",
        MarketKind::Total,
        Some("235.5"),
        None,
        "Over 235.5",
        "Under 235.5",
        459_000,
        549_000,
    ),
    (
        "NBA_26OCT28_PHIIND_TT.237",
        MarketKind::Total,
        Some("237.5"),
        None,
        "Over 237.5",
        "Under 237.5",
        419_000,
        589_000,
    ),
    (
        "NBA_26OCT28_PHIIND_TT.239",
        MarketKind::Total,
        Some("239.5"),
        None,
        "Over 239.5",
        "Under 239.5",
        379_000,
        639_000,
    ),
    (
        "NBA_26OCT28_PHIIND_TT.241",
        MarketKind::Total,
        Some("241.5"),
        None,
        "Over 241.5",
        "Under 241.5",
        369_000,
        649_000,
    ),
];

#[test]
fn every_phi_ind_market_grouped_parsed_and_labelled() {
    let cat = catalogue();
    assert_eq!(cat.markets.len(), 23);
    assert_eq!(
        cat.games.keys().map(String::as_str).collect::<Vec<_>>(),
        [ORL_GAME, PHI_GAME]
    );
    assert_eq!(cat.games[PHI_GAME], "NBA_26OCT28_PHIIND_ML.PHI");
    assert_eq!(cat.lines[PHI_GAME].len(), 12);
    assert_eq!(cat.lines[ORL_GAME].len(), 9);
    // The moneyline filter still sees only the moneyline.
    assert_eq!(listing().iter().filter_map(nba_moneyline).count(), 2);

    for (symbol, kind, line, long_team, long_label, short_label, long_p, short_p) in PHI_TABLE {
        let m: &CatalogueMarket = cat.market(symbol).unwrap();
        assert_eq!(m.game_id, PHI_GAME, "{symbol}");
        assert_eq!(m.start_ms, PHI_START, "{symbol}");
        assert_eq!(m.kind, kind, "{symbol}");
        assert_eq!(m.line().as_deref(), line, "{symbol}");
        assert_eq!(m.team_for(Side::Long), long_team, "{symbol}");
        assert_eq!(
            m.team_for(Side::Short),
            long_team.map(TeamSide::other),
            "{symbol}"
        );
        assert_eq!(m.label(Side::Long), long_label, "{symbol}");
        assert_eq!(m.label(Side::Short), short_label, "{symbol}");
        assert_eq!(m.indicative_price_u(Side::Long), Some(long_p), "{symbol}");
        assert_eq!(m.indicative_price_u(Side::Short), Some(short_p), "{symbol}");
        // Spreads and totals take the moneyline's abbreviations; every market has the short
        // names and subject ids.
        assert_eq!((m.away.abbr.as_str(), m.home.abbr.as_str()), ("PHI", "IND"));
        assert_eq!(
            (m.away.short.as_str(), m.home.short.as_str()),
            ("76ers", "Pacers")
        );
        assert_eq!(m.home.subject_id, "indiana-pacers");
        assert_eq!(m.taker_fee_rate_ppm, 20_000);
        assert_eq!(m.tick_size_min_u, 1_000);
    }
    // ORL @ TOR: the home team gives the points.
    let tor = cat.market("NBA_26OCT28_ORLTOR_SP.TOR_4").unwrap();
    assert_eq!(tor.long_team, TeamSide::Home);
    assert_eq!(tor.line_x10, Some(-45));
    assert_eq!(tor.label(Side::Long), "Raptors -4.5");
    assert_eq!(tor.label(Side::Short), "Magic +4.5");
    assert_eq!(tor.ladder_key(), 45);
    let t = cat.market("NBA_26OCT28_ORLTOR_TT.224").unwrap();
    assert_eq!(
        (t.line_x10, t.line().unwrap().as_str()),
        (Some(2245), "224.5")
    );
    assert_eq!(t.label(Side::Short), "Under 224.5");
}

#[test]
fn the_card_shows_three_lines_of_each_kind_around_even_odds() {
    let cat = catalogue();
    let card = card_at(&cat, Some("2026-10-28"), LISTED, CardLines::Main);
    assert_eq!(card.games.len(), 2);
    let phi = &card.games[0];
    assert_eq!(phi.game_id, PHI_GAME);
    assert!(!phi.in_play && phi.started_ms.is_none());
    assert_eq!(phi.markets_count, 13);
    let shown = |g: &desk::api::types::CardGame| {
        g.markets
            .iter()
            .map(|m| (m.kind, m.line.clone().unwrap_or_default()))
            .collect::<Vec<_>>()
    };
    let s = |k, l: &str| (k, l.to_owned());
    use MarketKind::{Spread, Total, Winner};
    // PHI spreads, by the 76ers' handicap: -6.5 (mark 0.535, nearest even) is the ladder's end,
    // so the window shifts inward; totals: 233.5 (0.505) likewise.
    assert_eq!(
        shown(phi),
        [
            s(Winner, ""),
            s(Spread, "-6.5"),
            s(Spread, "-5.5"),
            s(Spread, "-4.5"),
            s(Total, "233.5"),
            s(Total, "235.5"),
            s(Total, "237.5"),
        ]
    );
    // Winner picks: exactly the compatibility `picks`, with labels, 1-X-2 codes (away "2",
    // home "1") and the market title (L7-1).
    let w = &phi.markets[0];
    assert_eq!(w.picks.len(), 2);
    for (p, c) in w.picks.iter().zip(&phi.picks) {
        assert_eq!(
            (p.team, p.side, p.price_u, p.odds_u, p.outcome),
            (Some(c.pick.into()), c.side, c.price_u, c.odds_u, c.outcome)
        );
        assert_eq!(p.symbol, phi.symbol);
    }
    assert_eq!(
        w.picks.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(),
        ["76ers", "Pacers"]
    );
    assert_eq!(
        w.picks.iter().map(|p| p.outcome).collect::<Vec<_>>(),
        [Some(Outcome::Away), Some(Outcome::Home)]
    );
    assert_eq!(w.market_title.as_deref(), Some("Moneyline"));
    assert!(
        phi.markets[1..]
            .iter()
            .all(|m| m.market_title.is_none() && m.picks.iter().all(|p| p.outcome.is_none()))
    );
    assert_eq!((phi.sport, phi.league), (Sport::Basketball, League::Nba));
    // A spread: away pick first, both picks on the one symbol, opposite sides.
    let sp = &phi.markets[1];
    assert_eq!(sp.picks[0].label, "76ers -6.5");
    assert_eq!(sp.picks[1].label, "Pacers +6.5");
    assert_eq!(sp.picks[0].symbol, "NBA_26OCT28_PHIIND_SP.PHI_6");
    assert_eq!(sp.picks[1].symbol, sp.picks[0].symbol);
    assert_eq!(
        (sp.picks[0].side, sp.picks[1].side),
        (Side::Long, Side::Short)
    );
    assert_eq!(
        (sp.picks[0].team, sp.picks[1].team),
        (Some(PickTeam::Away), Some(PickTeam::Home))
    );
    assert_eq!(
        (sp.picks[0].price_u, sp.picks[1].price_u),
        (539_000, 469_000)
    );
    assert_eq!(sp.picks[0].odds_u, 1_000_000_000_000 / 539_000);
    // A total: Over then Under, no team.
    let tt = &phi.markets[4];
    assert_eq!(
        tt.picks
            .iter()
            .map(|p| (p.label.as_str(), p.team, p.side))
            .collect::<Vec<_>>(),
        [
            ("Over 233.5", None, Side::Long),
            ("Under 233.5", None, Side::Short)
        ]
    );

    // ORL @ TOR: Raptors -4.5 (0.510) and 223.5 (0.505) are the main lines, with a neighbour
    // on each side where the ladder allows.
    let orl = &card.games[1];
    assert_eq!(orl.markets_count, 10);
    assert_eq!(
        shown(orl),
        [
            s(Winner, ""),
            s(Spread, "-3.5"),
            s(Spread, "-4.5"),
            s(Spread, "-5.5"),
            s(Total, "221.5"),
            s(Total, "223.5"),
            s(Total, "224.5"),
        ]
    );
    // The home team gives the points: the away pick is the symbol's short side.
    let tor = &orl.markets[2];
    assert_eq!(
        tor.picks
            .iter()
            .map(|p| (p.label.as_str(), p.side, p.price_u))
            .collect::<Vec<_>>(),
        [
            ("Magic +4.5", Side::Short, 499_000),
            ("Raptors -4.5", Side::Long, 519_000)
        ]
    );

    // `lines=all`: every line, by the away team's handicap (the Pacers' -1.5 last: 76ers +1.5).
    let all = card_at(&cat, Some("2026-10-28"), LISTED, CardLines::All);
    let lines: Vec<String> = all.games[0]
        .markets
        .iter()
        .filter(|m| m.kind == Spread)
        .map(|m| m.picks[0].label.clone())
        .collect();
    assert_eq!(
        lines,
        [
            "76ers -6.5",
            "76ers -5.5",
            "76ers -4.5",
            "76ers -3.5",
            "76ers -2.5",
            "76ers -1.5",
            "76ers +1.5"
        ]
    );
    assert_eq!(all.games[0].markets.len(), 13);
    assert_eq!(all.games[1].markets.len(), 10);
    const { assert!(CARD_LINES == 3) };
}

#[test]
fn a_started_game_offers_its_winner_only() {
    let cat = catalogue();
    let now = PHI_START + 20 * 60_000;
    let card = card_at(&cat, Some("2026-10-28"), now, CardLines::Main);
    let phi = &card.games[0];
    assert!(phi.in_play);
    assert_eq!(phi.started_ms, Some(PHI_START));
    assert_eq!(phi.markets_count, 1);
    assert_eq!(phi.markets.len(), 1);
    assert_eq!(phi.markets[0].kind, MarketKind::Winner);
    assert_eq!(phi.picks.len(), 2);
    let orl = &card.games[1];
    assert!(!orl.in_play && orl.started_ms.is_none());
    assert_eq!(orl.markets.len(), 7);
    // At the start itself.
    let at = card_at(&cat, Some("2026-10-28"), ORL_START, CardLines::All);
    assert!(at.games.iter().all(|g| g.in_play && g.markets.len() == 1));

    let path = repo_root().join("fixtures/desk/card_lines.json");
    let got = serde_json::to_value(&card).unwrap();
    if std::env::var_os("BLESS").is_some() {
        std::fs::write(&path, serde_json::to_string_pretty(&got).unwrap() + "\n").unwrap();
    }
    let want: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(got, want, "card differs from fixtures/desk/card_lines.json");
    let typed: CardResponse = serde_json::from_value(want).unwrap();
    assert_eq!(typed, card);
}

#[test]
fn whole_lines_and_unpriced_markets_stay_off() {
    let mut list = listing();
    for m in &mut list {
        if m.symbol == "NBA_26OCT28_PHIIND_SP.PHI_6" {
            m.best_ask = None;
            m.best_bid = None;
            m.mark_price = None;
        }
        if m.symbol == "NBA_26OCT28_PHIIND_TT.233"
            && let Some(pascal_rs::MarketType::Sports(s)) = &mut m.display_attributes.market_type
        {
            s.market = pascal_rs::SportsMarket::TotalPoints { line: "233".into() };
        }
    }
    let cat = Catalogue::build(
        LISTED,
        LISTED,
        None,
        list.iter().filter_map(nba_market).collect(),
    );
    assert!(cat.market("NBA_26OCT28_PHIIND_TT.233").is_none());
    let card = card_at(&cat, Some("2026-10-28"), LISTED, CardLines::Main);
    let phi = &card.games[0];
    // 12 listed (TT.233 has no half-point line), one of them unpriced and not shown.
    assert_eq!(phi.markets_count, 12);
    let lines: Vec<&str> = phi
        .markets
        .iter()
        .filter_map(|m| m.line.as_deref())
        .collect();
    assert_eq!(lines, ["-5.5", "-4.5", "-3.5", "235.5", "237.5", "239.5"]);
}
