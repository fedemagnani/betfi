//! Plan 7.0c review gate, tipster data (l7-contracts L7-1): every tip of `GET /tipsters/{wallet}`
//! carries `pool_u` (Σ rS of its minted copies), `copies` and `teams` (short codes in leg
//! order), and `GET /tipsters/{wallet}/copiers?ticket=` lists a tip's minted copies newest
//! first with handle, stake and `open_pending` signature. Seeded DB, no network.

mod common;

use std::sync::atomic::{AtomicI64, Ordering};

use common::{TempDb, pascal_stub, start_desk};
use desk::Running;
use desk::api::types::{CopiersResponse, TipsterResponse};
use serde_json::{Value, json};
use solana_pubkey::Pubkey;
use sqlx::SqlitePool;

const U: u64 = 1_000_000;

fn now_s() -> i64 {
    (desk::now_ms() / 1000) as i64
}

/// A ticket of `user` (an own tip when `copied_from` is `None` and `tipster == user`), with legs
/// `(symbol, game_id, side, pick)`.
#[allow(clippy::too_many_arguments)]
async fn seed(
    db: &SqlitePool,
    user: &str,
    state: &str,
    rs_u: u64,
    tipster: &str,
    copied_from: Option<&str>,
    minted_at: i64,
    legs: &[(&str, &str, &str, &str)],
) -> String {
    static NONCE: AtomicI64 = AtomicI64::new(1);
    let id = Pubkey::new_unique().to_string();
    let minted = state != "pending" && state != "refunded";
    sqlx::query(
        "INSERT INTO tickets (ticket, pending, user, nonce, state, n, stake_u, rs_u, payout_u,
            odds_offered_u, odds_u, min_payout_per_unit_u, copy_fee_u, tipster, copied_from,
            minted_at, opened_at, tx_open_pending, updated_ms, is_tip, chain_fee_u)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 3000000, 3000000, 1, 0, ?, ?, ?, ?, ?, 0, ?, 0)",
    )
    .bind(&id)
    .bind(format!("pending-{id}"))
    .bind(user)
    .bind(NONCE.fetch_add(1, Ordering::Relaxed))
    .bind(state)
    .bind(legs.len() as i64)
    .bind((rs_u + U) as i64)
    .bind(minted.then_some(rs_u as i64))
    .bind((rs_u * 3) as i64)
    .bind(tipster)
    .bind(copied_from)
    .bind(minted.then_some(minted_at))
    .bind(minted_at - 5)
    .bind(format!("SIG-{id}"))
    .bind(i64::from(copied_from.is_none() && tipster == user))
    .execute(db)
    .await
    .unwrap();
    for (idx, (symbol, game, side, pick)) in legs.iter().enumerate() {
        sqlx::query(
            "INSERT INTO ticket_legs (ticket, idx, symbol, game_id, side, pick, start_ms,
                p_quote_u, p_fill_u, m1_ppm, status)
             VALUES (?, ?, ?, ?, ?, ?, ?, 500000, 500000, 30000, 'open')",
        )
        .bind(&id)
        .bind(idx as i64)
        .bind(*symbol)
        .bind(*game)
        .bind(*side)
        .bind(*pick)
        .bind((now_s() + 7200) * 1000)
        .execute(db)
        .await
        .unwrap();
    }
    id
}

const PHI: (&str, &str) = ("NBA_26OCT28_PHIIND_ML.PHI", "nba-phi-ind-2026-10-28");
const ORL: (&str, &str) = ("NBA_26OCT28_ORLTOR_ML.ORL", "nba-orl-tor-2026-10-28");
const OKC: (&str, &str) = ("NBA_26OCT28_OKCDAL_ML.OKC", "nba-okc-dal-2026-10-28");

async fn get<T: serde::de::DeserializeOwned>(desk: &Running, path: &str) -> (u16, T) {
    let r = reqwest::get(format!("http://{}{path}", desk.addr))
        .await
        .unwrap();
    (r.status().as_u16(), r.json().await.unwrap())
}

#[tokio::test]
async fn tips_carry_pool_copies_teams_and_the_copiers_route_lists_them() {
    let pascal = pascal_stub().await;
    let db = TempDb::new("tipsters-v2");
    let desk = start_desk(&pascal, &db).await;
    for _ in 0..100 {
        if desk.state.catalogue().is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let pool = desk.state.db.clone();
    let tipster = Pubkey::new_unique().to_string();
    let (alice, bob, carol) = (
        Pubkey::new_unique().to_string(),
        Pubkey::new_unique().to_string(),
        Pubkey::new_unique().to_string(),
    );
    sqlx::query(
        "INSERT INTO profiles (wallet, handle, handle_set_at, created_at, updated_ms)
         VALUES (?, 'alice_a', 1, 1, 0)",
    )
    .bind(&alice)
    .execute(&pool)
    .await
    .unwrap();
    let t0 = now_s() - 3600;
    // The tip: 76ers (away), Raptors (home), Thunder (away) → PHI, TOR, OKC.
    let tip = seed(
        &pool,
        &tipster,
        "open",
        10 * U,
        &tipster,
        None,
        t0,
        &[
            (PHI.0, PHI.1, "long", "away"),
            (ORL.0, ORL.1, "short", "home"),
            (OKC.0, OKC.1, "long", "away"),
        ],
    )
    .await;
    let legs = [(PHI.0, PHI.1, "long", "away")];
    // Minted copies: alice (oldest), bob (cashed out), the tipster's own copy (newest); a
    // pending and a refunded copy are not in the pool.
    let a = seed(
        &pool,
        &alice,
        "open",
        5 * U,
        &tipster,
        Some(&tip),
        t0 + 10,
        &legs,
    )
    .await;
    let b = seed(
        &pool,
        &bob,
        "cashed_out",
        7 * U,
        &tipster,
        Some(&tip),
        t0 + 20,
        &legs,
    )
    .await;
    let s = seed(
        &pool,
        &tipster,
        "settled",
        2 * U,
        &tipster,
        Some(&tip),
        t0 + 30,
        &legs,
    )
    .await;
    seed(
        &pool,
        &carol,
        "pending",
        9 * U,
        &tipster,
        Some(&tip),
        t0 + 40,
        &legs,
    )
    .await;
    seed(
        &pool,
        &carol,
        "refunded",
        9 * U,
        &tipster,
        Some(&tip),
        t0 + 50,
        &legs,
    )
    .await;
    // A second tip nobody copied.
    let lonely = seed(
        &pool,
        &tipster,
        "open",
        3 * U,
        &tipster,
        None,
        t0 + 60,
        &[(ORL.0, ORL.1, "long", "away")],
    )
    .await;

    let (code, t): (u16, TipsterResponse) = get(&desk, &format!("/tipsters/{tipster}")).await;
    assert_eq!(code, 200);
    let open = |id: &str| t.open.iter().find(|x| x.ticket == id).unwrap().clone();
    let tip_t = open(&tip);
    assert_eq!(tip_t.pool_u, Some(14 * U));
    assert_eq!(tip_t.copies, Some(3));
    assert_eq!(
        tip_t.teams.as_deref(),
        Some(&["PHI".to_owned(), "TOR".to_owned(), "OKC".to_owned()][..])
    );
    let lonely_t = open(&lonely);
    assert_eq!((lonely_t.pool_u, lonely_t.copies), (Some(0), Some(0)));
    assert_eq!(lonely_t.teams.as_deref(), Some(&["ORL".to_owned()][..]));
    assert_eq!(t.tipster.copies, 3);

    // The copiers, newest first; pool as on the tip.
    let (code, c): (u16, CopiersResponse) =
        get(&desk, &format!("/tipsters/{tipster}/copiers?ticket={tip}")).await;
    assert_eq!(code, 200);
    assert_eq!(c.ticket, tip);
    assert_eq!(c.pool_u, 14 * U);
    let rows: Vec<(&str, Option<&str>, u64, Option<&str>)> = c
        .copiers
        .iter()
        .map(|r| {
            (
                r.wallet.as_str(),
                r.handle.as_deref(),
                r.stake_u,
                r.tx.as_deref(),
            )
        })
        .collect();
    let (sa, sb, ss) = (format!("SIG-{a}"), format!("SIG-{b}"), format!("SIG-{s}"));
    assert_eq!(
        rows,
        [
            (tipster.as_str(), None, 2 * U, Some(ss.as_str())),
            (bob.as_str(), None, 7 * U, Some(sb.as_str())),
            (alice.as_str(), Some("alice_a"), 5 * U, Some(sa.as_str())),
        ]
    );
    // The exact L7-1 keys.
    let (_, v): (u16, Value) =
        get(&desk, &format!("/tipsters/{tipster}/copiers?ticket={tip}")).await;
    println!("served GET /tipsters/{{wallet}}/copiers: {v}");
    assert_eq!(
        v["copiers"][2],
        json!({"wallet": alice, "handle": "alice_a", "stake_u": 5 * U, "tx": sa})
    );
    let (_, v): (u16, Value) = get(&desk, &format!("/tipsters/{tipster}")).await;
    let o = v["open"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["ticket"] == tip.as_str())
        .unwrap();
    println!(
        "served open tip (pool_u, copies, teams): {} {} {}",
        o["pool_u"], o["copies"], o["teams"]
    );
    assert_eq!(o["pool_u"], 14 * U);
    assert_eq!(o["copies"], 3);
    assert_eq!(o["teams"], json!(["PHI", "TOR", "OKC"]));

    // A copy, someone else's tip, a bad or missing ticket, a bad wallet.
    let other = Pubkey::new_unique().to_string();
    for (path, want) in [
        (format!("/tipsters/{tipster}/copiers?ticket={a}"), 404),
        (format!("/tipsters/{other}/copiers?ticket={tip}"), 404),
        (format!("/tipsters/{tipster}/copiers?ticket=nope"), 422),
        (format!("/tipsters/{tipster}/copiers"), 422),
        (format!("/tipsters/nope/copiers?ticket={tip}"), 422),
    ] {
        let (code, _): (u16, Value) = get(&desk, &path).await;
        assert_eq!(code, want, "{path}");
    }
    desk.join().await;
}
