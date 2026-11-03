//! Spread and total legs, and in-play winner legs (6.0b, U-2), on the real 28 Oct listing of
//! PHI @ IND and ORL @ TOR (`fixtures/lines/markets_2026-10-28_phi-ind_orl-tor.TRIMMED.json`),
//! with books built from each market's listed best bid and ask.

use std::collections::BTreeMap;
use std::path::PathBuf;

use pascal_rs::{Level, Market, OrderBook};
use pricing::{LegInput, LegMarket, Params, Pick, RejectCode, Side, check_correlation, quote};
use serde_json::Value;

const H: u64 = 3_600_000;
const PHI_ML: &str = "NBA_26OCT28_PHIIND_ML.PHI";
const PHI_SP3: &str = "NBA_26OCT28_PHIIND_SP.PHI_3";
const IND_SP1: &str = "NBA_26OCT28_PHIIND_SP.IND_1";
const PHI_TT233: &str = "NBA_26OCT28_PHIIND_TT.233";
const ORL_ML: &str = "NBA_26OCT28_ORLTOR_ML.ORL";
const TOR_SP4: &str = "NBA_26OCT28_ORLTOR_SP.TOR_4";
const ORL_TT221: &str = "NBA_26OCT28_ORLTOR_TT.221";
/// `state_time_ms` of the listing (18:43 UTC, before every game).
const LISTED: u64 = 1_793_212_985_011;

fn markets() -> BTreeMap<String, Market> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/lines/markets_2026-10-28_phi-ind_orl-tor.TRIMMED.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let list: Vec<Market> = serde_json::from_value(v["data"].clone()).unwrap();
    list.into_iter().map(|m| (m.symbol.clone(), m)).collect()
}

/// Two levels a cent apart on each side of the listed best bid and ask, 5,000 contracts each.
fn books(m: &BTreeMap<String, Market>) -> BTreeMap<String, OrderBook> {
    m.values()
        .map(|mk| {
            let (bid, ask) = (mk.best_bid.unwrap(), mk.best_ask.unwrap());
            let lv = |price, size| Level { price, size };
            (
                mk.symbol.clone(),
                OrderBook {
                    asks: vec![lv(ask, 5_000), lv(ask + 10_000, 5_000)],
                    bids: vec![lv(bid, 5_000), lv(bid - 10_000, 5_000)],
                    spec: None,
                },
            )
        })
        .collect()
}

fn leg(m: &BTreeMap<String, Market>, symbol: &str, side: Side) -> LegInput {
    LegInput::from_market(&m[symbol], side).unwrap()
}

#[test]
fn market_kinds_and_long_teams() {
    let m = markets();
    let l = leg(&m, PHI_ML, Side::Short);
    assert_eq!(
        (l.market, l.long_pick, l.pick()),
        (LegMarket::Winner, Pick::Away, Pick::Home)
    );
    assert_eq!(
        (l.teams.away_abbr.as_str(), l.teams.home_abbr.as_str()),
        ("PHI", "IND")
    );
    // `SP.IND_1` is "Pacers -1.5": long backs the home team; its short side is "76ers +1.5".
    let l = leg(&m, IND_SP1, Side::Short);
    assert_eq!(
        (l.market, l.long_pick, l.pick()),
        (LegMarket::Spread, Pick::Home, Pick::Away)
    );
    let l = leg(&m, TOR_SP4, Side::Long);
    assert_eq!((l.market, l.pick()), (LegMarket::Spread, Pick::Home));
    let l = leg(&m, PHI_TT233, Side::Long);
    assert_eq!(l.market, LegMarket::Total);
    // Spreads and totals carry the game's subject ids (team rule) but no outcome labels.
    assert_eq!(l.teams.home_subject_id, "indiana-pacers");
    assert_eq!(l.game_id, "nba-phi-ind-2026-10-28");
    assert_eq!(l.start_ms, 1_793_228_400_000);
}

#[test]
fn a_spread_and_a_total_are_priced_like_any_binary_leg() {
    let m = markets();
    let b = books(&m);
    let p = Params::default();
    // 76ers -3.5 (long SP.PHI_3 at its 0.629 ask) and Under 221.5 (short TT.221: 1 − 0.551).
    let legs = [
        leg(&m, PHI_SP3, Side::Long),
        leg(&m, ORL_TT221, Side::Short),
    ];
    let q = quote(&legs, &b, 10_000_000, LISTED, &p).unwrap();
    assert_eq!(q.n, 2);
    assert_eq!(q.legs[0].price_u, 629_000);
    assert_eq!(q.legs[1].price_u, 449_000);
    assert_eq!(q.legs[0].mid_u, 625_000);
    assert_eq!(q.legs[1].mid_u, 445_000);
    // 00:00 and 00:30 CET: overlapping, both at the 5% tier and hedged at inception.
    assert!(
        q.legs
            .iter()
            .all(|l| l.overlaps && l.m1_ppm == 50_000 && l.hedged && !l.in_play)
    );
    // C2: 10 × 0.95 / 0.629 × 0.95 / 0.449, each step floored.
    let mut acc = 10_000_000_u64;
    for l in &q.legs {
        acc = acc * 950_000 / l.price_u;
    }
    assert_eq!(q.payout_u, acc);
    assert!(q.odds_offered_u < q.odds_fair_u);
    // One leg alone, either side.
    for (s, side) in [
        (PHI_TT233, Side::Long),
        (TOR_SP4, Side::Short),
        (IND_SP1, Side::Long),
    ] {
        let q = quote(&[leg(&m, s, side)], &b, 5_000_000, LISTED, &p).unwrap();
        assert_eq!(q.legs[0].m1_ppm, 30_000, "{s}");
    }
}

#[test]
fn one_leg_per_game_whatever_the_market() {
    let m = markets();
    let p = Params::default();
    let check = |a: &str, sa: Side, b: &str, sb: Side| {
        check_correlation(
            &[leg(&m, a, sa), leg(&m, b, sb)],
            10_000_000,
            LISTED,
            &p.caps,
        )
        .unwrap_err()
    };
    for (a, b) in [
        (PHI_ML, PHI_SP3),
        (PHI_SP3, PHI_TT233),
        (PHI_ML, PHI_TT233),
        (PHI_SP3, IND_SP1),
        (ORL_ML, TOR_SP4),
    ] {
        let r = check(a, Side::Long, b, Side::Short);
        assert_eq!(r.code, RejectCode::SameGame, "{a} + {b}");
        assert!(
            r.reason.starts_with("two legs on game nba-"),
            "{}",
            r.reason
        );
    }
    // Different games: fine.
    assert!(
        check_correlation(
            &[leg(&m, PHI_SP3, Side::Long), leg(&m, ORL_ML, Side::Short)],
            10_000_000,
            LISTED,
            &p.caps
        )
        .is_ok()
    );
    // A spread on PHI @ IND and a winner on another Pacers game the same day.
    let mut ind = leg(&m, ORL_ML, Side::Long);
    ind.game_id = "nba-ind-tor-2026-10-28".into();
    ind.teams.away_subject_id = "indiana-pacers".into();
    let r = check_correlation(
        &[leg(&m, PHI_SP3, Side::Long), ind],
        10_000_000,
        LISTED,
        &p.caps,
    )
    .unwrap_err();
    assert_eq!(r.code, RejectCode::SharedTeam);
}

#[test]
fn spreads_and_totals_close_at_tip_off_winners_stay_open_two_hours() {
    let m = markets();
    let b = books(&m);
    let p = Params::default();
    let start = m[PHI_ML]
        .display_attributes
        .expected_event_start_time_ms
        .unwrap();
    for (s, kind) in [(PHI_SP3, "spread"), (PHI_TT233, "total")] {
        for now in [start, start + 1, start + H] {
            let r = quote(&[leg(&m, s, Side::Long)], &b, 10_000_000, now, &p).unwrap_err();
            assert_eq!(r.code, RejectCode::AlreadyStarted);
            assert_eq!(
                r.reason,
                format!(
                    "game nba-phi-ind-2026-10-28 has already started: {kind} bets close at tip-off"
                )
            );
        }
        // One millisecond before the start it is still a pre-game leg.
        assert!(quote(&[leg(&m, s, Side::Long)], &b, 10_000_000, start - 1, &p).is_ok());
    }
    // The winner: in play from the start until 2 h after it.
    for now in [start, start + H, start + 2 * H - 1] {
        let q = quote(&[leg(&m, PHI_ML, Side::Short)], &b, 10_000_000, now, &p).unwrap();
        let l = &q.legs[0];
        assert!(l.in_play && l.hedged && !l.overlaps && l.m1_ppm == 30_000);
        assert_eq!(l.price_u, 309_000);
    }
    let r = quote(
        &[leg(&m, PHI_ML, Side::Long)],
        &b,
        10_000_000,
        start + 2 * H,
        &p,
    )
    .unwrap_err();
    assert_eq!(r.code, RejectCode::AlreadyStarted);
    let q = quote(
        &[leg(&m, PHI_ML, Side::Long)],
        &b,
        10_000_000,
        start - 1,
        &p,
    )
    .unwrap();
    assert!(!q.legs[0].in_play);
}

#[test]
fn in_play_winners_keep_the_overlap_tiers_and_are_always_hedged() {
    let m = markets();
    let mut b = books(&m);
    let p = Params::default();
    let phi_start = m[PHI_ML]
        .display_attributes
        .expected_event_start_time_ms
        .unwrap();
    // 10 min into PHI @ IND: the 76ers in play with Under 221.5 on ORL @ TOR (starts 20 min
    // later): overlapping, both at 5%, both hedged.
    let now = phi_start + 600_000;
    let legs = [leg(&m, PHI_ML, Side::Long), leg(&m, ORL_TT221, Side::Short)];
    let q = quote(&legs, &b, 10_000_000, now, &p).unwrap();
    assert_eq!(
        q.legs
            .iter()
            .map(|l| (l.in_play, l.overlaps, l.m1_ppm, l.hedged))
            .collect::<Vec<_>>(),
        [(true, true, 50_000, true), (false, true, 50_000, true)]
    );
    // With a later game (made up, 3 h on): the in-play leg is the roll's first leg, at 3%.
    let mut late = leg(&m, TOR_SP4, Side::Long);
    late.game_id = "nba-bos-nyk-2026-10-28".into();
    late.symbol = "NBA_26OCT28_BOSNYK_SP.NYK_4".into();
    late.teams.away_subject_id = "boston-celtics".into();
    late.teams.home_subject_id = "new-york-knicks".into();
    late.start_ms = now + 3 * H;
    b.insert(late.symbol.clone(), b[TOR_SP4].clone());
    let q = quote(
        &[leg(&m, PHI_ML, Side::Long), late],
        &b,
        10_000_000,
        now,
        &p,
    )
    .unwrap();
    assert_eq!(
        q.legs
            .iter()
            .map(|l| (l.in_play, l.overlaps, l.m1_ppm, l.hedged))
            .collect::<Vec<_>>(),
        [(true, false, 30_000, true), (false, false, 30_000, false)]
    );
    // Two winners in play, started 30 min apart: overlapping, both hedged.
    let now = phi_start + 3_000_000;
    let q = quote(
        &[leg(&m, PHI_ML, Side::Long), leg(&m, ORL_ML, Side::Short)],
        &b,
        10_000_000,
        now,
        &p,
    )
    .unwrap();
    assert!(q.legs.iter().all(|l| l.in_play && l.overlaps && l.hedged));
    // A configuration that lets a leg be in play longer than a game lasts could leave one
    // unhedged: refused rather than priced.
    let mut wide = p;
    wide.caps.in_play_max_ms = 4 * H;
    let mut old = leg(&m, ORL_ML, Side::Long);
    old.start_ms = phi_start - 3 * H;
    let r = quote(
        &[old, leg(&m, PHI_ML, Side::Long)],
        &b,
        10_000_000,
        phi_start + 1,
        &wide,
    )
    .unwrap_err();
    assert_eq!(r.code, RejectCode::Unpriceable);
    // The default keeps every in-play leg within one game length of every other.
    assert!(p.caps.in_play_max_ms <= p.game_length_ms);
}
