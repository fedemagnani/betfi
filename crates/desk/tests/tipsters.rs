//! Plan 4.A review gate, offline part: every tipster metric of spec section 6 reproduced by hand
//! on a seeded book of ten settled own tips (wins, losses, a half-void, two cash-outs, one with
//! no resolved leg) plus copies that must not count; the leaderboard filter and sorts; an open
//! ticket changes no score; the `/tipsters`, `/tipsters/{wallet}`, `/house` routes; the
//! indexer's copy-fee and `TipsterPaid` rows; `Ticket.tx.resolve_leg`; the pause mirror
//! (`/quote` 503); and the numbers of `fixtures/desk/tipster.json`.

mod common;

use std::sync::atomic::{AtomicI64, Ordering};

use common::{TempDb, pascal_stub, repo_root, start_desk};
use desk::api::types::{
    ErrorBody, ErrorStatus, HouseResponse, Side, TicketStatus, TipsterResponse, TipsterRow,
    TipstersResponse, Venue,
};
use desk::program::{
    ConfigAccount, LegStatus as ChainLeg, Pdas, TicketAccount, TicketLeg, TicketState,
};
use desk::tipsters::metrics::{self, OwnTip, Sort, TipLeg, TipsterBook};
use desk::tipsters::store;
use desk::{DeskEvent, Running};
use solana_pubkey::Pubkey;
use sqlx::SqlitePool;

const NOW: i64 = 1_800_000_000;
const DAY: i64 = 86_400;
const U: u64 = 1_000_000;

/// One ticket row to seed.
struct Seed<'a> {
    id: &'a str,
    user: &'a str,
    state: &'a str,
    stake_u: u64,
    paid_u: Option<u64>,
    odds_u: u64,
    minted_at: i64,
    settled_at: Option<i64>,
    tipster: Option<&'a str>,
    copied_from: Option<&'a str>,
    copy_fee_u: u64,
    /// (side, p_fill_u, Some(r) when resolved).
    legs: Vec<(&'a str, u64, Option<u64>)>,
}

impl<'a> Seed<'a> {
    /// An own tip of `user`.
    fn tip(id: &'a str, user: &'a str, state: &'a str, stake_u: u64, paid_u: Option<u64>) -> Self {
        Seed {
            id,
            user,
            state,
            stake_u,
            paid_u,
            odds_u: 2 * U,
            minted_at: NOW - DAY,
            settled_at: None,
            tipster: Some(user),
            copied_from: None,
            copy_fee_u: 0,
            legs: vec![("long", 500_000, None)],
        }
    }

    fn odds(mut self, odds_u: u64) -> Self {
        self.odds_u = odds_u;
        self
    }

    fn at(mut self, minted_days_ago: i64, settled_days_ago: Option<i64>) -> Self {
        self.minted_at = NOW - minted_days_ago * DAY;
        self.settled_at = settled_days_ago.map(|d| NOW - d * DAY);
        self
    }

    fn legs(mut self, legs: Vec<(&'a str, u64, Option<u64>)>) -> Self {
        self.legs = legs;
        self
    }

    /// A copy by `user` of `orig` (whose user is `tipster`).
    fn copy(mut self, orig: &'a str, tipster: &'a str, fee_u: u64) -> Self {
        self.tipster = Some(tipster);
        self.copied_from = Some(orig);
        self.copy_fee_u = fee_u;
        self
    }
}

async fn seed(db: &SqlitePool, s: &Seed<'_>) {
    static NONCE: AtomicI64 = AtomicI64::new(1);
    sqlx::query(
        "INSERT INTO tickets (ticket, pending, user, nonce, state, n, stake_u, rs_u, payout_u,
            odds_offered_u, odds_u, min_payout_per_unit_u, copy_fee_u, tipster, copied_from,
            minted_at, settled_at, paid_u, updated_ms, is_tip, chain_fee_u)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, ?, 0, ?, ?)",
    )
    .bind(s.id)
    .bind(format!("pending-{}", s.id))
    .bind(s.user)
    .bind(NONCE.fetch_add(1, Ordering::Relaxed))
    .bind(s.state)
    .bind(s.legs.len() as i64)
    .bind(s.stake_u as i64)
    .bind((s.state != "pending").then_some(s.stake_u as i64))
    .bind((s.stake_u * s.odds_u / U) as i64)
    .bind(s.odds_u as i64)
    .bind(s.odds_u as i64)
    .bind(s.copy_fee_u as i64)
    .bind(s.tipster)
    .bind(s.copied_from)
    .bind(s.minted_at)
    .bind(s.settled_at)
    .bind(s.paid_u.map(|p| p as i64))
    .bind(i64::from(
        s.tipster == Some(s.user) && s.copied_from.is_none(),
    ))
    .bind((s.stake_u / 20) as i64)
    .execute(db)
    .await
    .unwrap();
    for (idx, (side, p, r)) in s.legs.iter().enumerate() {
        let status = match (side, r) {
            (_, None) => "open",
            (_, Some(500_000)) => "half",
            (&"long", Some(1_000_000)) | (&"short", Some(0)) => "won",
            _ => "lost",
        };
        sqlx::query(
            "INSERT INTO ticket_legs (ticket, idx, symbol, game_id, side, pick, start_ms,
                p_quote_u, p_fill_u, m1_ppm, status, resolution_u, resolved_ms, tx_resolve)
             VALUES (?, ?, ?, ?, ?, 'away', 0, ?, ?, 30000, ?, ?, ?, ?)",
        )
        .bind(s.id)
        .bind(idx as i64)
        .bind(format!("SYM_{idx}.X"))
        .bind(format!("game-{}-{idx}", s.id))
        .bind(side)
        .bind(*p as i64)
        .bind(*p as i64)
        .bind(status)
        .bind(r.map(|r| r as i64))
        .bind(r.map(|_| (NOW - 10 + idx as i64) * 1000))
        .bind(r.map(|_| format!("resolve-{}-{idx}", s.id)))
        .execute(db)
        .await
        .unwrap();
    }
}

async fn paid(db: &SqlitePool, copy: &str, tipster: &str, amount_u: u64) {
    sqlx::query(
        "INSERT INTO tipster_paid (ticket, tipster, amount_u, source, tx, updated_ms)
         VALUES (?, ?, ?, 'event', 'sig', 0)",
    )
    .bind(copy)
    .bind(tipster)
    .bind(amount_u as i64)
    .execute(db)
    .await
    .unwrap();
}

const T: &str = "tipster-T";
const X: &str = "tipster-X";

/// The hand-checked book: T's ten settled own tips (oldest first), and around them what must not
/// move T's scores.
fn book_t() -> Vec<Seed<'static>> {
    vec![
        // 1. won: long 0.50 → r̃ 1. Minted 40 days ago: outside the skin window.
        Seed::tip("t01", T, "settled", 10 * U, Some(19 * U))
            .odds(1_900_000)
            .at(40, Some(39))
            .legs(vec![("long", 500_000, Some(U))]),
        // 2. lost: long 0.60 → r̃ 0. Outside the window too.
        Seed::tip("t02", T, "settled", 10 * U, Some(0))
            .odds(1_583_333)
            .at(35, Some(34))
            .legs(vec![("long", 600_000, Some(0))]),
        // 3. won, two legs: long 0.50 → 1, short 0.40 with r = 0 → r̃ 1.
        Seed::tip("t03", T, "settled", 5 * U, Some(23_750_000))
            .odds(4_750_000)
            .at(20, Some(19))
            .legs(vec![
                ("long", 500_000, Some(U)),
                ("short", 400_000, Some(0)),
            ]),
        // 4. half-void: r = 0.5 pays P / 2 = 9.5 < 10 staked, so not won.
        Seed::tip("t04", T, "settled", 10 * U, Some(9_500_000))
            .odds(1_900_000)
            .at(15, Some(14))
            .legs(vec![("long", 500_000, Some(500_000))]),
        // 5. cashed out at 4 before its only leg resolved: lost, and no leg for Brier/edge.
        Seed::tip("t10", T, "cashed_out", 5 * U, Some(4 * U))
            .odds(1_583_333)
            .at(14, Some(13))
            .legs(vec![("long", 600_000, None)]),
        // 6. cashed out at 12 after leg 0 won (leg 1 unresolved): won.
        Seed::tip("t05", T, "cashed_out", 10 * U, Some(12 * U))
            .odds(2_714_285)
            .at(12, Some(11))
            .legs(vec![("long", 700_000, Some(U)), ("long", 500_000, None)]),
        // 7. lost: short 0.30 with r = 1 → r̃ 0.
        Seed::tip("t06", T, "settled", 4 * U, Some(0))
            .odds(3_166_666)
            .at(10, Some(9))
            .legs(vec![("short", 300_000, Some(U))]),
        // 8-10. three wins in a row (the streak).
        Seed::tip("t07", T, "settled", 2 * U, Some(7_600_000))
            .odds(3_800_000)
            .at(8, Some(7))
            .legs(vec![("long", 250_000, Some(U))]),
        Seed::tip("t08", T, "settled", 10 * U, Some(11_875_000))
            .odds(1_187_500)
            .at(6, Some(5))
            .legs(vec![("long", 800_000, Some(U))]),
        Seed::tip("t09", T, "settled", 5 * U, Some(10_500_000))
            .odds(2_100_000)
            .at(4, Some(3))
            .legs(vec![("long", 450_000, Some(U))]),
        // Not scored: a pending and a refunded own tip.
        Seed::tip("t12", T, "pending", 3 * U, None),
        Seed::tip("t13", T, "refunded", 3 * U, None),
        // X's own tip, and T's copy of it: T's copy counts for X's followers, never for T.
        Seed::tip("x01", X, "settled", 10 * U, Some(30 * U))
            .at(2, Some(1))
            .legs(vec![("long", 330_000, Some(U))]),
        Seed::tip("c01", T, "settled", 20 * U, Some(60 * U))
            .at(2, Some(1))
            .legs(vec![("long", 330_000, Some(U))])
            .copy("x01", X, 400_000),
        // Copies of T's tips by others: two settled, one open, one refunded.
        Seed::tip("c02", "user-U", "settled", 10 * U, Some(19 * U))
            .at(40, Some(39))
            .legs(vec![("long", 500_000, Some(U))])
            .copy("t01", T, 200_000),
        Seed::tip("c03", "user-V", "settled", 5 * U, Some(0))
            .at(20, Some(19))
            .legs(vec![("long", 500_000, Some(0))])
            .copy("t03", T, 100_000),
        Seed::tip("c04", "user-V", "open", 2 * U, None)
            .copy("t11", T, 40_000)
            .at(1, None),
        Seed::tip("c05", "user-U", "refunded", 2 * U, None).copy("t11", T, 40_000),
    ]
}

/// T's open own tip (added later: it must change no score).
fn open_t11() -> Seed<'static> {
    Seed::tip("t11", T, "open", 7 * U, None).at(1, None)
}

async fn seeded(tag: &str) -> (TempDb, SqlitePool) {
    let db = TempDb::new(tag);
    let pool = desk::db::open(&db.path()).await.unwrap();
    for s in book_t() {
        seed(&pool, &s).await;
    }
    for (copy, amount) in [("c02", 160_000), ("c03", 80_000), ("c04", 32_000)] {
        paid(&pool, copy, T, amount).await;
    }
    paid(&pool, "c01", X, 320_000).await;
    (db, pool)
}

async fn score_of(pool: &SqlitePool, wallet: &str) -> metrics::Scored {
    let mut books = store::books(pool, Some(wallet)).await.unwrap();
    assert_eq!(books.len(), 1, "{wallet}");
    metrics::score(&books.pop().unwrap(), NOW)
}

#[tokio::test]
async fn every_metric_by_hand() {
    let (_db, pool) = seeded("tipsters-hand").await;
    seed(&pool, &open_t11()).await;
    let s = score_of(&pool, T).await;
    let r = &s.row;

    // Stakes 10+10+5+10+5+10+4+2+10+5 = 71; returned 19+0+23.75+9.5+4+12+0+7.6+11.875+10.5
    // = 98.225. ROI = 27.225 / 71 = 0.3834507… → 383_450 ppm (truncated).
    assert_eq!(r.settled, 10);
    assert_eq!(r.roi_ppm, 383_450);
    // Won (paid > stake): t01, t03, t05, t07, t08, t09. Not t04 (half-void 9.5 < 10), not t10.
    assert_eq!(r.won, 6);
    assert_eq!(r.hit_rate_ppm, 600_000);
    // Resolved legs, d = r̃ − p (×1e-6): t01 +0.5, t02 −0.6, t03 +0.5 and +0.6, t04 0,
    // t05 +0.3 (leg 1 unresolved), t06 −0.3, t07 +0.75, t08 +0.2, t09 +0.55; t10 has none.
    // 10 legs. Σd = 2.5 → edge 0.25. Σd² = 0.25+0.36+0.25+0.36+0+0.09+0.09+0.5625+0.04+0.3025
    // = 2.305 → Brier 0.2305.
    assert_eq!(s.legs, 10);
    assert_eq!(r.edge_ppm, 250_000);
    assert_eq!(r.brier_ppm, 230_500);
    // Σ odds = 24_685_117 over 10.
    assert_eq!(r.avg_odds_u, 2_468_511);
    // Skin: scored own tips minted in the last 30 days: all but t01 and t02 → 51 USDC.
    assert_eq!(r.skin_u, 51 * U);
    // Streak: t07, t08, t09 won; t06 before them lost.
    assert_eq!(r.streak, 3);
    // Open: t11 only (t12 pending, t13 refunded do not count).
    assert_eq!((r.open_count, r.open_staked_u), (1, 7 * U));
    // Copies minted of T's tips: c02, c03, c04 (c05 refunded; c01 is T copying X).
    assert_eq!(r.copies, 3);
    // Followers: c02 + c03 staked (10 + 0.2) + (5 + 0.1) = 15.3, returned 19 → 0.2418300…
    assert_eq!(r.followers_roi_ppm, Some(241_830));
    // Σ TipsterPaid to T (c01's 0.32 went to X).
    assert_eq!(r.fees_earned_u, 272_000);

    // History, oldest first: after t01 ROI 0.9, Brier 0.25; after t02 (20 staked, 19 back)
    // −0.05, (0.25+0.36)/2; after t04 17.25/35, Brier 1.22/5; t10's cash-out (no resolved leg)
    // moves ROI to 16.25/40 but leaves Brier at 0.244; the last point is the row.
    let h = &s.history;
    assert_eq!(h.len(), 10);
    let ids: Vec<&str> = h.iter().map(|p| p.ticket.as_str()).collect();
    assert_eq!(
        ids,
        [
            "t01", "t02", "t03", "t04", "t10", "t05", "t06", "t07", "t08", "t09"
        ]
    );
    assert_eq!((h[0].roi_cum_ppm, h[0].brier_cum_ppm), (900_000, 250_000));
    assert_eq!((h[1].roi_cum_ppm, h[1].brier_cum_ppm), (-50_000, 305_000));
    assert_eq!((h[3].roi_cum_ppm, h[3].brier_cum_ppm), (492_857, 244_000));
    assert_eq!((h[4].roi_cum_ppm, h[4].brier_cum_ppm), (406_250, 244_000));
    assert_eq!(h[9].roi_cum_ppm, r.roi_ppm);
    assert_eq!(h[9].brier_cum_ppm, r.brier_ppm);
    assert_eq!(h[0].settled_at, NOW - 39 * DAY);

    // X: one own tip, 10 staked, 30 back; T's copy is X's follower: (60 − 20.4) / 20.4.
    let x = score_of(&pool, X).await.row;
    assert_eq!((x.settled, x.won, x.roi_ppm), (1, 1, 2_000_000));
    assert_eq!(x.copies, 1);
    assert_eq!(x.followers_roi_ppm, Some(1_941_176));
    assert_eq!(x.fees_earned_u, 320_000);
    // user-U and user-V only copied: no tipster row.
    assert!(
        store::books(&pool, Some("user-U"))
            .await
            .unwrap()
            .is_empty()
    );
}

/// 6.0c (l6-contracts L6-2): `roi_30d_ppm`, `copiers` and `verified` by hand on the same book,
/// extended with an open self-copy and a second copy by a wallet that already copied.
#[tokio::test]
async fn thirty_day_roi_copiers_and_verified_by_hand() {
    let (_db, pool) = seeded("tipsters-6c").await;
    seed(&pool, &open_t11()).await;
    // T copies its own open tip (D-114): a copy, never a copier.
    seed(
        &pool,
        &Seed::tip("c07", T, "open", 2 * U, None)
            .copy("t11", T, 40_000)
            .at(0, None),
    )
    .await;
    // user-U copies again: still one copier.
    seed(
        &pool,
        &Seed::tip("c08", "user-U", "open", 2 * U, None)
            .copy("t11", T, 40_000)
            .at(0, None),
    )
    .await;
    let r = score_of(&pool, T).await.row;
    // Minted in the last 30 days (all but t01 at 40 d and t02 at 35 d): stakes 5+10+5+10+4+2+10+5
    // = 51 (the skin), returned 23.75+9.5+4+12+0+7.6+11.875+10.5 = 79.225. ROI 28.225 / 51 =
    // 0.5534313… → 553_431 ppm (truncated); the all-time ROI stays 383_450.
    assert_eq!(r.skin_u, 51 * U);
    assert_eq!(r.roi_30d_ppm, 553_431);
    assert_eq!(r.roi_ppm, 383_450);
    // Minted copies: c02 (user-U), c03, c04 (user-V), c07 (T itself), c08 (user-U); c05 was
    // refunded. Copiers: {user-U, user-V} = 2.
    assert_eq!(r.copies, 5);
    assert_eq!(r.copiers, 2);
    // On the board: 10 settled ≥ 5, skin 51 ≥ 20.
    assert!(r.verified);
    assert_eq!(r.handle, None, "filled by the routes, not the metrics");
    // X: one settled tip (fewer than 5), its only copier is T.
    let x = score_of(&pool, X).await.row;
    assert!(!x.verified);
    assert_eq!((x.copiers, x.roi_30d_ppm), (1, 2_000_000));
    // Nothing minted in the window: no skin, ROI 0 (not a division by zero).
    let old = metrics::score(
        &TipsterBook {
            wallet: "old".into(),
            own: vec![OwnTip {
                ticket: "o1".into(),
                state: TicketStatus::Settled,
                stake_u: 10 * U,
                paid_u: Some(20 * U),
                odds_u: 2 * U,
                minted_at: Some(NOW - 31 * DAY),
                settled_at: Some(NOW - 30 * DAY),
                legs: vec![],
            }],
            ..TipsterBook::default()
        },
        NOW,
    );
    assert_eq!((old.row.roi_30d_ppm, old.row.roi_ppm), (0, 1_000_000));
}

#[tokio::test]
async fn an_open_ticket_changes_no_score() {
    let (_db, pool) = seeded("tipsters-open").await;
    let before = score_of(&pool, T).await;
    seed(&pool, &open_t11()).await;
    // And a new open copy of T's open tip: a copy, not a score.
    seed(
        &pool,
        &Seed::tip("c06", "user-U", "open", 3 * U, None)
            .copy("t11", T, 60_000)
            .at(0, None),
    )
    .await;
    let after = score_of(&pool, T).await;
    let strip = |r: &TipsterRow| TipsterRow {
        open_count: 0,
        open_staked_u: 0,
        copies: 0,
        ..r.clone()
    };
    assert_eq!(strip(&before.row), strip(&after.row));
    assert_eq!(before.history, after.history);
    assert_eq!((before.row.open_count, after.row.open_count), (0, 1));
    assert_eq!(after.row.open_staked_u, 7 * U);
    assert_eq!(after.row.copies, before.row.copies + 1);
    assert_eq!(after.row.followers_roi_ppm, before.row.followers_roi_ppm);
}

#[test]
fn leaderboard_filter_and_sorts() {
    let tip = |id: &str, stake_u: u64, paid_u: u64, p: u64, minted_days_ago: i64| OwnTip {
        ticket: id.to_owned(),
        state: TicketStatus::Settled,
        stake_u,
        paid_u: Some(paid_u),
        odds_u: 2 * U,
        minted_at: Some(NOW - minted_days_ago * DAY),
        settled_at: Some(NOW - minted_days_ago * DAY + 3600),
        legs: vec![TipLeg {
            side: Side::Long,
            p_fill_u: p,
            resolution_u: Some(if paid_u > 0 { U } else { 0 }),
        }],
    };
    let book = |wallet: &str, tips: Vec<OwnTip>| TipsterBook {
        wallet: wallet.to_owned(),
        own: tips,
        ..TipsterBook::default()
    };
    // A: 5 settled, skin 25 (on the board), ROI +0.2.
    let a = book(
        "A",
        (0..5)
            .map(|k| tip(&format!("a{k}"), 5 * U, 6 * U, 500_000, 1))
            .collect(),
    );
    // B: 6 settled, skin 30 (on the board), ROI −1, perfectly wrong prices → worst Brier.
    let b = book(
        "B",
        (0..6)
            .map(|k| tip(&format!("b{k}"), 5 * U, 0, 900_000, 2))
            .collect(),
    );
    // C: 4 settled (off the board) with the best ROI.
    let c = book(
        "C",
        (0..4)
            .map(|k| tip(&format!("c{k}"), 10 * U, 40 * U, 250_000, 1))
            .collect(),
    );
    // D: 5 settled, but 4 × 5 of the stake was 40 days ago: skin 5 < 20 (off the board).
    let d = book(
        "D",
        (0..5)
            .map(|k| {
                tip(
                    &format!("d{k}"),
                    5 * U,
                    10 * U,
                    500_000,
                    if k == 0 { 1 } else { 40 },
                )
            })
            .collect(),
    );
    // E: only an open tip: nothing scored.
    let mut e = book("E", vec![tip("e0", 50 * U, 0, 500_000, 1)]);
    e.own[0].state = TicketStatus::Open;
    e.own[0].paid_u = None;
    let all: Vec<metrics::Scored> = [&a, &b, &c, &d, &e]
        .iter()
        .map(|bk| metrics::score(bk, NOW))
        .collect();
    let skins: Vec<u64> = all.iter().map(|s| s.row.skin_u).collect();
    assert_eq!(skins, [25 * U, 30 * U, 40 * U, 5 * U, 0]);
    assert_eq!(all[0].row.roi_ppm, 200_000);
    assert_eq!(all[1].row.roi_ppm, -1_000_000);

    let wallets = |rows: &[metrics::Scored]| -> Vec<String> {
        rows.iter().map(|s| s.row.wallet.clone()).collect()
    };
    let mut board: Vec<metrics::Scored> = all
        .iter()
        .filter(|s| metrics::on_board(&s.row))
        .cloned()
        .collect();
    metrics::sort_board(&mut board, Sort::Roi);
    assert_eq!(wallets(&board), ["A", "B"]);

    let mut everyone = all.clone();
    metrics::sort_board(&mut everyone, Sort::Roi);
    // C (ROI 3) > D (1) > A (0.2) > B (−1); E has nothing settled: last.
    assert_eq!(wallets(&everyone), ["C", "D", "A", "B", "E"]);
    metrics::sort_board(&mut everyone, Sort::Brier);
    // (r̃ − p)²: C 0.5625, D 0.25, A 0.25, B 0.81; ties go to more settled, then wallet.
    let briers: Vec<u64> = everyone.iter().map(|s| s.row.brier_ppm).collect();
    assert_eq!(wallets(&everyone), ["A", "D", "C", "B", "E"]);
    assert_eq!(briers, [250_000, 250_000, 562_500, 810_000, 0]);
    metrics::sort_board(&mut everyone, Sort::Skin);
    assert_eq!(wallets(&everyone), ["C", "B", "A", "D", "E"]);
    metrics::sort_board(&mut everyone, Sort::FollowersRoi);
    // Nobody has a settled copy: settled count, then wallet.
    assert_eq!(wallets(&everyone), ["B", "A", "D", "C", "E"]);
    assert!(everyone.iter().all(|s| s.row.followers_roi_ppm.is_none()));

    assert_eq!("".parse::<Sort>(), Ok(Sort::Roi));
    assert_eq!("followers_roi".parse::<Sort>(), Ok(Sort::FollowersRoi));
    assert!("hit_rate".parse::<Sort>().is_err());
}

async fn get<T: serde::de::DeserializeOwned>(desk: &Running, path: &str) -> (u16, T) {
    let r = reqwest::get(format!("http://{}{path}", desk.addr))
        .await
        .unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap())
}

#[tokio::test]
async fn routes_serve_the_board_the_tipster_and_the_house() {
    let pascal = pascal_stub().await;
    let db = TempDb::new("tipsters-routes");
    let desk = start_desk(&pascal, &db).await;
    let pool = desk.state.db.clone();
    // Real wallets for the routes (they check the address).
    let t = Pubkey::new_unique().to_string();
    let z = Pubkey::new_unique().to_string();
    let u = Pubkey::new_unique().to_string();
    let ids: Vec<String> = (0..9).map(|_| Pubkey::new_unique().to_string()).collect();
    // T: five settled wins, 5 USDC each, minted today: on the board. Z: five losses.
    for (k, id) in ids.iter().take(5).enumerate() {
        let mut s = Seed::tip(id, &t, "settled", 5 * U, Some(10 * U)).legs(vec![(
            "long",
            500_000,
            Some(U),
        )]);
        s.minted_at = desk::now_ms() as i64 / 1000 - 3600;
        s.settled_at = Some(s.minted_at + 600 + k as i64);
        seed(&pool, &s).await;
        let zid = format!("z{k}");
        let mut s =
            Seed::tip(&zid, &z, "settled", 5 * U, Some(0)).legs(vec![("long", 600_000, Some(0))]);
        s.minted_at = desk::now_ms() as i64 / 1000 - 3600;
        s.settled_at = Some(s.minted_at + 600);
        seed(&pool, &s).await;
    }
    seed(&pool, &Seed::tip(&ids[5], &t, "open", 7 * U, None)).await;
    seed(
        &pool,
        &Seed::tip(&ids[6], &u, "settled", 5 * U, Some(10 * U))
            .legs(vec![("long", 500_000, Some(U))])
            .copy(&ids[0], &t, 100_000),
    )
    .await;
    paid(&pool, &ids[6], &t, 80_000).await;

    let (s, board): (u16, TipstersResponse) = get(&desk, "/tipsters").await;
    assert_eq!(s, 200);
    let wallets: Vec<&str> = board.tipsters.iter().map(|r| r.wallet.as_str()).collect();
    assert_eq!(wallets, [t.as_str(), z.as_str()]);
    let row = &board.tipsters[0];
    assert_eq!(
        (row.settled, row.won, row.roi_ppm, row.streak),
        (5, 5, 1_000_000, 5)
    );
    assert_eq!((row.open_count, row.open_staked_u), (1, 7 * U));
    assert_eq!((row.copies, row.fees_earned_u), (1, 80_000));
    // (10 − 5.1) / 5.1
    assert_eq!(row.followers_roi_ppm, Some(960_784));
    let (_, brier): (u16, TipstersResponse) = get(&desk, "/tipsters?sort=brier&all=1").await;
    assert_eq!(brier.tipsters[0].wallet, t);
    let (s, e): (u16, ErrorBody) = get(&desk, "/tipsters?sort=luck").await;
    assert_eq!((s, e.status), (422, ErrorStatus::Rejected));
    let (s, _): (u16, ErrorBody) = get(&desk, "/tipsters?all=yes").await;
    assert_eq!(s, 422);

    let (s, one): (u16, TipsterResponse) = get(&desk, &format!("/tipsters/{t}")).await;
    assert_eq!(s, 200);
    assert_eq!(one.tipster, *row);
    assert_eq!(one.history.len(), 5);
    assert_eq!(one.settled.len(), 5);
    assert_eq!(one.settled[0].ticket, ids[4], "newest first");
    assert!(
        one.settled
            .iter()
            .all(|x| x.tipster.as_deref() == Some(t.as_str()))
    );
    assert_eq!(one.open.len(), 1);
    assert_eq!(one.open[0].state, TicketStatus::Open);
    // Seeded legs carry their resolve_leg signature.
    assert_eq!(
        one.settled[0].tx.resolve_leg,
        [format!("resolve-{}-0", ids[4])]
    );
    // Every Ticket the desk serves carries tx.resolve_leg: /tickets/{user} as well (the stream's
    // `ticket` frame is built by the same load_ticket).
    let (s, list): (u16, desk::api::types::TicketsResponse) =
        get(&desk, &format!("/tickets/{t}")).await;
    assert_eq!(s, 200);
    let mine = list.tickets.iter().find(|x| x.ticket == ids[4]).unwrap();
    assert_eq!(mine.tx.resolve_leg, [format!("resolve-{}-0", ids[4])]);
    let open_one = list.tickets.iter().find(|x| x.ticket == ids[5]).unwrap();
    assert!(open_one.tx.resolve_leg.is_empty());
    let (s, e): (u16, ErrorBody) = get(&desk, &format!("/tipsters/{u}")).await;
    assert_eq!((s, e.status), (404, ErrorStatus::NotFound));
    let (s, _): (u16, ErrorBody) = get(&desk, "/tipsters/not-a-wallet").await;
    assert_eq!(s, 422);

    // /house with the chain off: counts and fees from the DB.
    sqlx::query(
        "INSERT INTO pascal_fills (venue, trade_id, symbol, side, price_u, size, liquidity, fee_u,
            builder_fee_u, collateral_change_u, client_order_id, seq, ts_ms, raw)
         VALUES ('mock', 1, 'S', 'bid', 500000, 10, 'taker', 70000, 5000, -5000000, 1, 1, 0, '{}'),
                ('mock', 2, 'S', 'ask', 500000, 2, 'maker', -3000, 1000, 1000000, 2, 2, 0, '{}')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (s, h): (u16, HouseResponse) = get(&desk, "/house").await;
    assert_eq!(s, 200);
    assert_eq!(h.profile, "dev");
    assert_eq!(h.vault, Default::default());
    assert_eq!(
        (h.tickets.open, h.tickets.settled, h.tickets.pending),
        (1, 11, 0)
    );
    assert_eq!(h.hedger.venue, Venue::Mock);
    assert!(h.hedger.positions.is_empty());
    // chain_fee_u = stake / 20 in the seed: 11 settled × 0.25 + open 0.35.
    assert_eq!(h.fees.margin_u, 11 * 250_000 + 350_000);
    assert_eq!(
        (h.fees.pascal_fees_u, h.fees.builder_fees_u),
        (67_000, 6_000)
    );
    assert_eq!(h.fees.copy_fees_u, 20_000);
    desk.join().await;
}

#[tokio::test]
async fn indexer_records_copy_fees_and_mirrors_pause() {
    let pascal = pascal_stub().await;
    let db = TempDb::new("tipsters-indexer");
    let desk = start_desk(&pascal, &db).await;
    let state = desk.state.clone();
    let pdas = Pdas::new(state.config.program_id);
    let (tipster, copier) = (Pubkey::new_unique(), Pubkey::new_unique());
    let orig = pdas.ticket(&tipster, 1);
    let leg = TicketLeg {
        symbol_hash: [1; 32],
        game_id_hash: [2; 32],
        start_ts: NOW,
        side: 0,
        price_u: 500_000,
        fill_u: 500_000,
        m1_ppm: 30_000,
        hedged: true,
        status: ChainLeg::Open,
        resolution_ppm: 0,
    };
    let copy = TicketAccount {
        user: copier,
        nonce: 9,
        legs: vec![leg],
        stake_u: 10 * U,
        refund_u: 0,
        payout_u: 19_400_000,
        odds_u: 1_940_000,
        odds_offered_u: 1_940_000,
        min_payout_per_unit_u: 1_880_000,
        r_ppm: 1_000_000,
        fee_u: 300_000,
        copy_fee_u: 200_000,
        state: TicketState::Open,
        tipster: Some(tipster),
        copied_from: Some(orig),
        quote_hash: [3; 32],
        minted_at: NOW,
        resolved_at: 0,
        paid_u: 0,
    };
    let key = pdas.ticket(&copier, 9);
    let cfg = ConfigAccount {
        authority: tipster,
        crank: tipster,
        quote_signer: tipster,
        usdc_mint: pdas.usdc_mint(),
        copy_fee_bps: 200,
        tipster_share_bps: 8_000,
        max_legs: 5,
        max_payout: 10_000 * U,
        min_stake: U,
        reserve: 100 * U,
        paused: false,
    };
    desk::indexer::config::record(&state, &cfg, None, 10)
        .await
        .unwrap();
    // A copy found by a resync (no TipsterPaid seen): the charged fee and a derived row.
    desk::indexer::store::apply_ticket(
        &state,
        &pdas,
        &key,
        &copy,
        11,
        &desk::indexer::store::TxFacts::default(),
    )
    .await
    .unwrap();
    let row: (i64, String, i64, String) = sqlx::query_as(
        "SELECT t.copy_fee_u, p.tipster, p.amount_u, p.source FROM tickets t
         JOIN tipster_paid p ON p.ticket = t.ticket WHERE t.ticket = ?",
    )
    .bind(key.to_string())
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(
        row,
        (200_000, tipster.to_string(), 160_000, "derived".into())
    );
    // The event replaces the derived row; a replay of the account changes nothing.
    store::record_paid(&state.db, &key, &tipster, 160_000, Some("sig1"))
        .await
        .unwrap();
    desk::indexer::store::apply_ticket(
        &state,
        &pdas,
        &key,
        &copy,
        12,
        &desk::indexer::store::TxFacts::default(),
    )
    .await
    .unwrap();
    let row: (String, Option<String>) =
        sqlx::query_as("SELECT source, tx FROM tipster_paid WHERE ticket = ?")
            .bind(key.to_string())
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(row, ("event".into(), Some("sig1".into())));

    // Pause mirror: Config.paused → AppState::paused → /quote 503; an older read cannot undo
    // a newer one.
    let mut rx = state.subscribe();
    let paused = ConfigAccount {
        paused: true,
        ..cfg.clone()
    };
    desk::indexer::config::record(&state, &paused, None, 20)
        .await
        .unwrap();
    assert!(state.paused.load(Ordering::SeqCst));
    desk::indexer::config::record(&state, &cfg, None, 15)
        .await
        .unwrap();
    assert!(state.is_paused(), "a stale read does not unpause");
    let r = reqwest::Client::new()
        .post(format!("http://{}/quote", desk.addr))
        .json(&serde_json::json!({
            "legs": [{"symbol": "NBA_26OCT28_PHIIND_ML.PHI", "side": "long"}],
            "stake_u": 5_000_000
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 503);
    let e: ErrorBody = r.json().await.unwrap();
    assert!(e.reason.contains("paused"), "{}", e.reason);
    let (_, health): (u16, serde_json::Value) = get(&desk, "/health").await;
    assert_eq!(health["paused"], true);
    // A reset ledger restarts its slots: once the stored read is a minute old, a lower slot wins.
    sqlx::query("UPDATE chain_config SET updated_ms = updated_ms - 61000")
        .execute(&state.db)
        .await
        .unwrap();
    desk::indexer::config::record(&state, &cfg, None, 1)
        .await
        .unwrap();
    assert!(!state.is_paused());
    // The mirror is not a bus event.
    while let Ok(e) = rx.try_recv() {
        assert!(
            matches!(
                e,
                DeskEvent::Heartbeat { .. } | DeskEvent::CatalogueRefreshed { .. }
            ),
            "{e:?}"
        );
    }
    desk.join().await;
}

#[tokio::test]
async fn resolve_leg_signatures_follow_resolution_order() {
    let pascal = pascal_stub().await;
    let db = TempDb::new("tipsters-resolve-leg");
    let desk = start_desk(&pascal, &db).await;
    let pool = desk.state.db.clone();
    seed(
        &pool,
        &Seed::tip("r1", "user-R", "open", 5 * U, None).legs(vec![
            ("long", 500_000, Some(U)),
            ("long", 500_000, Some(U)),
            ("long", 500_000, None),
        ]),
    )
    .await;
    // Leg 1 resolved before leg 0; leg 0's signature was not seen (resync).
    sqlx::query("UPDATE ticket_legs SET resolved_ms = 1 WHERE ticket = 'r1' AND idx = 1")
        .execute(&pool)
        .await
        .unwrap();
    let t = desk::crank::tickets::load_ticket(&desk.state, "r1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.tx.resolve_leg, ["resolve-r1-1", "resolve-r1-0"]);
    sqlx::query("UPDATE ticket_legs SET tx_resolve = NULL WHERE ticket = 'r1' AND idx = 0")
        .execute(&pool)
        .await
        .unwrap();
    let t = desk::crank::tickets::load_ticket(&desk.state, "r1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.tx.resolve_leg, ["resolve-r1-1"]);
    seed(&pool, &Seed::tip("r2", "user-R", "open", 5 * U, None)).await;
    let t = desk::crank::tickets::load_ticket(&desk.state, "r2")
        .await
        .unwrap()
        .unwrap();
    assert!(t.tx.resolve_leg.is_empty());
    desk.join().await;
}

/// `fixtures/desk/tipster.json`: its row is what the metrics give for its own tickets.
#[test]
fn tipster_fixture_numbers_hold() {
    let text = std::fs::read_to_string(repo_root().join("fixtures/desk/tipster.json")).unwrap();
    let f: TipsterResponse = serde_json::from_str(&text).unwrap();
    let to_tip = |t: &desk::api::types::Ticket| OwnTip {
        ticket: t.ticket.clone(),
        state: t.state,
        stake_u: t.stake_u,
        paid_u: t.paid_u,
        odds_u: t.odds_offered_u,
        minted_at: t.minted_at,
        settled_at: t.settled_at,
        legs: t
            .legs
            .iter()
            .map(|l| TipLeg {
                side: l.side,
                p_fill_u: l.p_fill_u.unwrap_or(l.p_quote_u),
                resolution_u: l.resolution_u,
            })
            .collect(),
    };
    let book = TipsterBook {
        wallet: f.tipster.wallet.clone(),
        own: f.settled.iter().chain(&f.open).map(to_tip).collect(),
        ..TipsterBook::default()
    };
    let now = f.settled.iter().filter_map(|t| t.settled_at).max().unwrap();
    let s = metrics::score(&book, now);
    // Copies, fees and the handle are not in the fixture's own tickets.
    let own_only = |r: &TipsterRow| TipsterRow {
        copies: 0,
        followers_roi_ppm: None,
        fees_earned_u: 0,
        copiers: 0,
        handle: None,
        ..r.clone()
    };
    assert_eq!(own_only(&s.row), own_only(&f.tipster));
    assert_eq!(s.history, f.history);
    assert!(
        f.settled
            .iter()
            .chain(&f.open)
            .all(|t| { t.tipster.as_deref() == Some(t.user.as_str()) && t.copied_from.is_none() })
    );
}
