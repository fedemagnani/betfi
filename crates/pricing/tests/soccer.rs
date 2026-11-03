//! Soccer 1-X-2 legs (7.0b, D-435) on the real 28 Oct listings: three EPL matches, one
//! Bundesliga and one LaLiga match (`fixtures/soccer/markets_2026-10-28_soccer.TRIMMED.json`)
//! and the NBA's PHI @ IND (`fixtures/lines/…`), with books built from each market's listed best
//! bid and ask. A soccer leg is a long on its outcome's own symbol, never short; one leg per
//! game; the overlap tiers and the in-play window apply across sports by start time.

use std::collections::BTreeMap;
use std::path::PathBuf;

use pascal_rs::{Level, Market, OrderBook};
use pricing::{
    LegInput, LegMarket, Params, Pick, RejectCode, SOCCER_GAME_LENGTH_MS, Side, check_correlation,
    quote,
};
use serde_json::Value;

const H: u64 = 3_600_000;
const PHI_ML: &str = "NBA_26OCT28_PHIIND_ML.PHI";
const PHI_START: u64 = 1_793_228_400_000;
const AVL: &str = "EPL_26OCT31_AVLFUL_ML.AVL";
const DRAW: &str = "EPL_26OCT31_AVLFUL_ML.DRAW";
const FUL: &str = "EPL_26OCT31_AVLFUL_ML.FUL";
const MCI_DRAW: &str = "EPL_26OCT31_MCIBHA_ML.DRAW";
/// Aston Villa – Fulham kicks off at 20:00 UTC on 31 Oct.
const AVL_START: u64 = 1_793_476_800_000;

fn load(rel: &str) -> Vec<Market> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel);
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    serde_json::from_value(v["data"].clone()).unwrap()
}

fn markets() -> BTreeMap<String, Market> {
    load("soccer/markets_2026-10-28_soccer.TRIMMED.json")
        .into_iter()
        .chain(load(
            "lines/markets_2026-10-28_phi-ind_orl-tor.TRIMMED.json",
        ))
        .map(|m| (m.symbol.clone(), m))
        .collect()
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

/// `leg` with its start moved to `start_ms` (to mix it with the NBA night of 28 Oct).
fn leg_at(m: &BTreeMap<String, Market>, symbol: &str, start_ms: u64) -> LegInput {
    LegInput {
        start_ms,
        ..leg(m, symbol, Side::Long)
    }
}

#[test]
fn soccer_outcomes_are_three_way_legs() {
    let m = markets();
    for (s, long_pick) in [(AVL, Pick::Home), (DRAW, Pick::Home), (FUL, Pick::Away)] {
        let l = leg(&m, s, Side::Long);
        assert_eq!(l.market, LegMarket::ThreeWay, "{s}");
        assert!(l.market.is_winner());
        assert_eq!(l.long_pick, long_pick, "{s}");
        assert_eq!(l.pick(), long_pick, "{s}");
        assert_eq!(l.game_id, "epl-ast-ful-2026-10-31");
        assert_eq!(l.start_ms, AVL_START);
        assert_eq!(
            (
                l.teams.home_subject_id.as_str(),
                l.teams.away_subject_id.as_str()
            ),
            ("aston-villa", "fulham")
        );
    }
    assert_eq!(LegMarket::ThreeWay.as_str(), "three_way");
    // The NBA's two-way moneyline is unchanged.
    assert_eq!(leg(&m, PHI_ML, Side::Short).market, LegMarket::Winner);
    // Soccer is winner-only: its spreads, totals and both-teams-to-score are refused.
    let now = AVL_START - H;
    for s in [
        "EPL_26OCT31_AVLFUL_SP.AVL_1",
        "EPL_26OCT31_AVLFUL_TT.2",
        "EPL_26OCT31_AVLFUL_PP.BTTS",
    ] {
        let l = leg(&m, s, Side::Long);
        assert_eq!(l.market, LegMarket::Other, "{s}");
        let r = check_correlation(&[l], 10_000_000, now, &Params::default().caps).unwrap_err();
        assert_eq!(r.code, RejectCode::UnsupportedMarket, "{s}");
    }
}

#[test]
fn soccer_legs_are_long_only_and_one_per_game() {
    let m = markets();
    let caps = Params::default().caps;
    let now = AVL_START - H;
    for s in [AVL, DRAW, FUL] {
        check_correlation(&[leg(&m, s, Side::Long)], 10_000_000, now, &caps).unwrap();
        let r = check_correlation(&[leg(&m, s, Side::Short)], 10_000_000, now, &caps).unwrap_err();
        assert_eq!(r.code, RejectCode::LongOnly, "{s}");
        assert!(r.reason.contains("never short"), "{}", r.reason);
    }
    for (a, b) in [(DRAW, AVL), (AVL, FUL), (FUL, DRAW)] {
        let r = check_correlation(
            &[leg(&m, a, Side::Long), leg(&m, b, Side::Long)],
            10_000_000,
            now,
            &caps,
        )
        .unwrap_err();
        assert_eq!(r.code, RejectCode::SameGame, "{a} + {b}");
        assert_eq!(r.reason, "two legs on game epl-ast-ful-2026-10-31");
    }
    // Two draws on two matches the same day are fine.
    check_correlation(
        &[leg(&m, DRAW, Side::Long), leg(&m, MCI_DRAW, Side::Long)],
        10_000_000,
        now - 5 * H,
        &caps,
    )
    .unwrap();
}

#[test]
fn a_mixed_nba_and_epl_quote() {
    let m = markets();
    let b = books(&m);
    let p = Params::default();
    let now = PHI_START - H;
    // The draw kicks off an hour after PHI @ IND tips off: the games overlap, both legs take
    // the overlap tier and are hedged at inception.
    let legs = [leg(&m, PHI_ML, Side::Long), leg_at(&m, DRAW, PHI_START + H)];
    let q = quote(&legs, &b, 10_000_000, now, &p).unwrap();
    assert_eq!(q.n, 2);
    assert!(
        q.legs
            .iter()
            .all(|l| l.overlaps && l.m1_ppm == 50_000 && l.hedged)
    );
    // A long on the draw's own symbol: its price is the ask, its mid the book's mid.
    assert_eq!(q.legs[1].side, Side::Long);
    assert_eq!(q.legs[1].price_u, 274_000);
    assert_eq!(q.legs[1].mid_u, 265_000);
    assert_eq!(q.legs[1].pick, Pick::Home);
    assert_eq!(q.legs[0].price_u, 699_000);

    // Three hours apart: past the 2.5 h window, sequential, the lower tier for both.
    let legs = [
        leg(&m, PHI_ML, Side::Long),
        leg_at(&m, DRAW, PHI_START + 3 * H),
    ];
    let q = quote(&legs, &b, 10_000_000, now, &p).unwrap();
    assert!(q.legs.iter().all(|l| !l.overlaps && l.m1_ppm == 30_000));
    assert!(q.legs[0].hedged && !q.legs[1].hedged);

    // One window for every pair, the longest sport's (proposed D-460): two matches 2 h 15 min
    // apart count as overlapping although a match lasts 2 h.
    assert!(p.game_length_ms > SOCCER_GAME_LENGTH_MS);
    let legs = [
        leg_at(&m, DRAW, PHI_START),
        leg_at(&m, MCI_DRAW, PHI_START + 2 * H + H / 4),
    ];
    let q = quote(&legs, &b, 10_000_000, now, &p).unwrap();
    assert!(q.legs.iter().all(|l| l.overlaps));
}

#[test]
fn soccer_legs_in_play() {
    let m = markets();
    let b = books(&m);
    let p = Params::default();
    // Half time: a 1-X-2 leg is a winner leg, open in play (D-322).
    let q = quote(
        &[leg(&m, DRAW, Side::Long)],
        &b,
        10_000_000,
        AVL_START + H,
        &p,
    )
    .unwrap();
    assert!(q.legs[0].in_play && q.legs[0].hedged);
    // Closed from start + 2 h (the match's length).
    assert!(p.caps.in_play_max_ms <= SOCCER_GAME_LENGTH_MS);
    let r = quote(
        &[leg(&m, DRAW, Side::Long)],
        &b,
        10_000_000,
        AVL_START + p.caps.in_play_max_ms,
        &p,
    )
    .unwrap_err();
    assert_eq!(r.code, RejectCode::AlreadyStarted);
}
