//! Quotes over the committed NBA tape of 28 Oct 2026 (`tapes/nba-2026-10-28/`): the catalogue
//! from `markets_selected.json`, the books from `sample/books.jsonl`.
//!
//! The expected numbers were computed independently (a short Python transcription of C2 and the
//! depth walk over the same books).

use std::collections::BTreeMap;
use std::path::PathBuf;

use pascal_rs::{Books, Level, Market, OrderBook};
use pricing::{
    LegInput, LegMarket, Params, PayoutLeg, Pick, RejectCode, RejectStatus, Side, TickSpec,
    check_correlation, quote,
};
use serde_json::Value;

const PHI: &str = "NBA_26OCT28_PHIIND_ML.PHI";
const ORL: &str = "NBA_26OCT28_ORLTOR_ML.ORL";
const OKC: &str = "NBA_26OCT28_OKCDAL_ML.OKC";
const HOU: &str = "NBA_26OCT28_HOUMIL_ML.HOU";

fn tape() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tapes/nba-2026-10-28")
}

fn markets() -> BTreeMap<String, Market> {
    let text = std::fs::read_to_string(tape().join("markets_selected.json")).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    let list: Vec<Market> = serde_json::from_value(v["data"].clone()).unwrap();
    list.into_iter().map(|m| (m.symbol.clone(), m)).collect()
}

/// `(state_time_ms, books)` of every line of `sample/books.jsonl`.
fn snapshots() -> Vec<(u64, BTreeMap<String, OrderBook>)> {
    let text = std::fs::read_to_string(tape().join("sample/books.jsonl")).unwrap();
    text.lines()
        .map(|line| {
            let v: Value = serde_json::from_str(line).unwrap();
            let body = &v["body"];
            let t: u64 = body["state_time_ms"].as_str().unwrap().parse().unwrap();
            let books: Books = serde_json::from_value(body["data"].clone()).unwrap();
            (t, books.books)
        })
        .collect()
}

fn leg(m: &BTreeMap<String, Market>, symbol: &str, side: Side) -> LegInput {
    LegInput::from_market(&m[symbol], side).unwrap()
}

/// A same-night, 3-leg ticket with a home pick: 76ers (away, long), Raptors (home, short on the
/// ORL symbol), Thunder (away, long); 10 USDC.
#[test]
fn quotes_a_real_three_leg_nba_ticket() {
    let m = markets();
    let (now, books) = snapshots().remove(0);
    let legs = [
        leg(&m, PHI, Side::Long),
        leg(&m, ORL, Side::Short),
        leg(&m, OKC, Side::Long),
    ];
    assert_eq!(legs[1].pick(), Pick::Home);
    assert_eq!(legs[1].teams.home_abbr, "TOR");
    assert_eq!(legs[1].tick, TickSpec::NBA);
    assert_eq!(legs[1].taker_fee_rate_ppm, 20_000);

    let q = quote(&legs, &books, 10_000_000, now, &Params::default()).unwrap();
    assert_eq!(q.n, 3);
    let picks: Vec<Pick> = q.legs.iter().map(|l| l.pick).collect();
    assert_eq!(picks, [Pick::Away, Pick::Home, Pick::Away]);
    // PHI 00:00, ORL 00:30, OKC 02:30 CET: PHI–ORL and ORL–OKC overlap (PHI–OKC is exactly
    // 2.5 h apart, which does not), so every leg is at the 5% tier and, through ORL, in the
    // inception hedged set.
    assert!(
        q.legs
            .iter()
            .all(|l| l.overlaps && l.m1_ppm == 50_000 && l.hedged)
    );
    // Mids: PHI (0.691 + 0.699)/2; ORL short 1 − (0.371 + 0.379)/2; OKC (0.651 + 0.659)/2.
    let mids: Vec<u64> = q.legs.iter().map(|l| l.mid_u).collect();
    assert_eq!(mids, [695_000, 625_000, 655_000]);
    let sizes: Vec<u64> = q.legs.iter().map(|l| l.size).collect();
    assert_eq!(sizes, [13, 14, 14]);
    // p: PHI ask 0.699; ORL short 1 − bid 0.371 = 0.629; OKC ask 0.659 (all top of book).
    let prices: Vec<u64> = q.legs.iter().map(|l| l.price_u).collect();
    assert_eq!(prices, [699_000, 629_000, 659_000]);
    // p_max = p · 1.03^(1/3), snapped down to 0.001.
    let pm: Vec<u64> = q.legs.iter().map(|l| l.p_max_u).collect();
    assert_eq!(pm, [705_000, 635_000, 665_000]);
    assert_eq!(q.payout_u, 29_590_858);
    assert_eq!(q.odds_offered_u, 2_959_085);
    assert_eq!(q.payout_fair_u, 35_147_453);
    assert_eq!(q.odds_fair_u, 3_514_745);
    assert_eq!(q.fee_u, 5_556_595);
    assert_eq!(q.min_payout_per_unit_u, 2_870_312);
    assert_eq!(q.slippage_bps, 300);
    assert_eq!(q.margin_ppm, 142_625);
    // 13·0.699·0.301·2% + 14·0.629·0.371·2% + 14·0.659·0.341·2%.
    assert_eq!(q.pascal_fees_est_u, 54_704 + 65_341 + 62_922);
    assert_eq!(q.copy_fee_u, 0);

    // Offered payout is C2 at the quote prices, as the program will recompute it.
    let c2 = pricing::payout(
        10_000_000,
        1_000_000,
        &q.legs
            .iter()
            .map(|l| PayoutLeg {
                m1_ppm: l.m1_ppm,
                f_u: l.price_u,
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert_eq!(c2.payout_u, q.payout_u);

    // C4 field names on the wire.
    let json = serde_json::to_value(&q).unwrap();
    for key in [
        "n",
        "stake_u",
        "legs",
        "odds_fair_u",
        "odds_offered_u",
        "payout_u",
        "fee_u",
        "pascal_fees_est_u",
        "copy_fee_u",
        "slippage_bps",
        "min_payout_per_unit_u",
    ] {
        assert!(json.get(key).is_some(), "{key}");
    }
    let leg1 = &json["legs"][1];
    assert_eq!(leg1["side"], "short");
    assert_eq!(leg1["pick"], "home");
    assert_eq!(leg1["game_id"], "nba-orl-tor-2026-10-28");
    for key in [
        "symbol", "start_ms", "price_u", "p_max_u", "m1_ppm", "overlaps",
    ] {
        assert!(leg1.get(key).is_some(), "{key}");
    }
}

#[test]
fn every_snapshot_quotes_every_single_leg() {
    let m = markets();
    for (now, books) in snapshots() {
        for symbol in m.keys() {
            for side in [Side::Long, Side::Short] {
                let q = quote(
                    &[leg(&m, symbol, side)],
                    &books,
                    5_000_000,
                    now,
                    &Params::default(),
                )
                .unwrap_or_else(|r| panic!("{symbol} {side:?}: {r}"));
                let l = &q.legs[0];
                assert!(!l.overlaps && l.m1_ppm == 30_000 && l.hedged);
                assert!(l.price_u >= l.mid_u, "{symbol} {side:?}: p below mid");
                assert!(q.odds_offered_u < q.odds_fair_u);
            }
        }
    }
}

#[test]
fn correlation_rejections() {
    let m = markets();
    let (now, books) = snapshots().remove(0);
    let p = Params::default();
    let phi = leg(&m, PHI, Side::Long);
    let code = |legs: &[LegInput], stake: u64, now: u64| {
        let r = quote(legs, &books, stake, now, &p).unwrap_err();
        (r.code, r.reason)
    };

    let (c, reason) = code(&[phi.clone(), leg(&m, PHI, Side::Short)], 10_000_000, now);
    assert_eq!(c, RejectCode::SameGame);
    assert_eq!(reason, "two legs on game nba-phi-ind-2026-10-28");

    assert_eq!(
        code(std::slice::from_ref(&phi), 4_999_999, now).0,
        RejectCode::StakeTooSmall
    );
    assert_eq!(code(&[], 10_000_000, now).0, RejectCode::NoLegs);
    let six: Vec<LegInput> = std::iter::repeat_n(phi.clone(), 6).collect();
    assert_eq!(code(&six, 10_000_000, now).0, RejectCode::TooManyLegs);

    // A winner leg may be taken in play (U-2) until 2 h after the start.
    let q = quote(
        std::slice::from_ref(&phi),
        &books,
        10_000_000,
        phi.start_ms,
        &p,
    )
    .unwrap();
    assert!(q.legs[0].in_play && q.legs[0].hedged);
    let (c, reason) = code(
        std::slice::from_ref(&phi),
        10_000_000,
        phi.start_ms + 2 * 3_600_000,
    );
    assert_eq!(c, RejectCode::AlreadyStarted);
    assert_eq!(
        reason,
        "game nba-phi-ind-2026-10-28 started more than 120 min ago: too late to bet on the winner"
    );
    assert_eq!(
        code(
            std::slice::from_ref(&phi),
            10_000_000,
            phi.start_ms - 24 * 3_600_000 - 1
        )
        .0,
        RejectCode::StartsTooLate
    );
    assert!(
        check_correlation(
            std::slice::from_ref(&phi),
            5_000_000,
            phi.start_ms - 24 * 3_600_000,
            &p.caps
        )
        .is_ok()
    );

    let mut other = leg(&m, HOU, Side::Long);
    other.market = LegMarket::Other;
    other.symbol = "NBA_26OCT28_HOUMIL_BTTS".into();
    let (c, reason) = code(&[phi.clone(), other], 10_000_000, now);
    assert_eq!(c, RejectCode::UnsupportedMarket);
    assert_eq!(
        reason,
        "NBA_26OCT28_HOUMIL_BTTS is not a winner, spread or total market"
    );

    // The Pacers again in another game the same day (a made-up back-to-back).
    let mut ind = leg(&m, HOU, Side::Long);
    ind.game_id = "nba-ind-mil-2026-10-28".into();
    ind.teams.away_subject_id = "indiana-pacers".into();
    let (c, reason) = code(&[phi.clone(), ind.clone()], 10_000_000, now);
    assert_eq!(c, RejectCode::SharedTeam);
    assert!(reason.contains("indiana-pacers"), "{reason}");
    // By abbreviation only.
    ind.teams.away_subject_id = "x".into();
    ind.teams.away_abbr = "ind".into();
    assert_eq!(
        code(&[phi.clone(), ind], 10_000_000, now).0,
        RejectCode::SharedTeam
    );

    // Books: none, or one-sided -> unavailable (503).
    let mut no_book = books.clone();
    no_book.remove(PHI);
    let r = quote(std::slice::from_ref(&phi), &no_book, 10_000_000, now, &p).unwrap_err();
    assert_eq!(
        (r.code, r.status()),
        (RejectCode::NoBook, RejectStatus::Unavailable)
    );
    let mut one_sided = books.clone();
    one_sided.get_mut(PHI).unwrap().bids.clear();
    let r = quote(std::slice::from_ref(&phi), &one_sided, 10_000_000, now, &p).unwrap_err();
    assert_eq!(r.code, RejectCode::OneSidedBook);
    assert_eq!(r.status(), RejectStatus::Unavailable);
}

/// Synthetic legs on distinct games, two hours apart, with the given book for every leg.
fn synthetic(n: usize, now: u64, book: &OrderBook) -> (Vec<LegInput>, BTreeMap<String, OrderBook>) {
    let m = markets();
    let base = leg(&m, PHI, Side::Long);
    let mut legs = Vec::new();
    let mut books = BTreeMap::new();
    for i in 0..n {
        let mut l = base.clone();
        l.symbol = format!("SYN_{i}");
        l.game_id = format!("syn-{i}");
        l.start_ms = now + 3_600_000 + i as u64 * 7_200_000;
        l.teams.away_subject_id = format!("away-{i}");
        l.teams.home_subject_id = format!("home-{i}");
        l.teams.away_abbr = format!("A{i}");
        l.teams.home_abbr = format!("H{i}");
        books.insert(l.symbol.clone(), book.clone());
        legs.push(l);
    }
    (legs, books)
}

fn lv(price: i64, size: u64) -> Level {
    Level { price, size }
}

#[test]
fn payout_cap_and_overflow_worst_cases() {
    let now = 1_793_212_987_458;
    let p = Params::default();
    // 5 legs at 0.501 ask / 0.499 bid: 20 USDC pays 490.29 (under the 500 cap), 21 pays 514.8.
    let book = OrderBook {
        asks: vec![lv(501_000, 10_000)],
        bids: vec![lv(499_000, 10_000)],
        spec: None,
    };
    let (legs, books) = synthetic(5, now, &book);
    let q = quote(&legs, &books, 20_000_000, now, &p).unwrap();
    assert!(
        q.payout_u <= 500_000_000 && q.payout_u > 490_000_000,
        "{}",
        q.payout_u
    );
    assert!(q.legs.iter().all(|l| l.overlaps && l.hedged));
    let r = quote(&legs, &books, 21_000_000, now, &p).unwrap_err();
    assert_eq!(r.code, RejectCode::PayoutTooLarge);

    // 5 legs at the minimum tick: the payout overflows u64 and is refused, never wrapped.
    let cheap = OrderBook {
        asks: vec![lv(1_000, u64::MAX / 2)],
        bids: vec![lv(1_000, u64::MAX / 2)],
        spec: None,
    };
    let (legs, books) = synthetic(5, now, &cheap);
    let r = quote(&legs, &books, 5_000_000, now, &p).unwrap_err();
    assert_eq!(r.code, RejectCode::PayoutTooLarge);
    // One leg at the minimum tick still fits (5 USDC → 4,850 USDC) but is above the cap.
    let r = quote(&legs[..1], &books, 5_000_000, now, &p).unwrap_err();
    assert_eq!(r.code, RejectCode::PayoutTooLarge);

    // Depth gate: 5 legs whose books show 61 contracts within 0.02 (cap 30) for a 31-contract
    // hedge.
    let thin = OrderBook {
        asks: vec![lv(501_000, 61), lv(600_000, 1_000_000)],
        bids: vec![lv(499_000, 10_000)],
        spec: None,
    };
    let (legs, books) = synthetic(5, now, &thin);
    let r = quote(&legs, &books, 20_000_000, now, &p).unwrap_err();
    assert_eq!(r.code, RejectCode::NotEnoughDepth);
    assert!(
        r.reason.contains("31 contracts needed, 61 within 0.02"),
        "{}",
        r.reason
    );
}
