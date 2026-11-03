//! Plan 6.0c review gate, profiles (l6-contracts L6-2): `PUT /profile` refuses a signature by
//! another wallet, a stale `ts`, a reused signature, a taken handle whatever its case, a second
//! change within 24 h, a held (recently released) handle, a title on someone else's tip, a second
//! title, and a title on a closed tip (a started one takes it, D-660); `GET /profile/{wallet}` never 404s;
//! handles reach `/tipsters`, titles reach `/ticket/{id}`, `/tipsters/{wallet}` and the stream;
//! writes are rate-limited per wallet. The signed message is pinned byte for byte (the worked
//! example 6.1e signs against).

mod common;

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use base64::Engine;
use common::{TempDb, pascal_stub, start_desk};
use desk::api::types::{ProfileResponse, Ticket, TipsterResponse, TipstersResponse};
use desk::profiles::{self, IP_LIMIT, WALLET_LIMIT};
use desk::{DeskEvent, Running};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use solana_pubkey::Pubkey;
use sqlx::SqlitePool;

const U: u64 = 1_000_000;

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn wallet_of(k: &SigningKey) -> String {
    Pubkey::new_from_array(k.verifying_key().to_bytes()).to_string()
}

fn now_s() -> i64 {
    (desk::now_ms() / 1000) as i64
}

/// A `PUT /profile` body for `wallet`, signed by `signer`.
fn body(
    signer: &SigningKey,
    wallet: &str,
    handle: &str,
    ticket: Option<&str>,
    title: Option<&str>,
    ts: i64,
) -> Value {
    let msg = profiles::message(wallet, handle, ticket, title, ts);
    let sig =
        base64::engine::general_purpose::STANDARD.encode(signer.sign(msg.as_bytes()).to_bytes());
    json!({"wallet": wallet, "handle": handle, "title": title, "ticket": ticket, "ts": ts, "sig": sig})
}

/// A `ts` near now that no other call in this binary returns. Ed25519 is deterministic, so the
/// same request signed twice with one `ts` is the same signature (refused as a replay): each call
/// steps one second further back from a fixed base, well inside the ±300 s window.
fn unique_ts() -> i64 {
    static BASE: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    static STEP: AtomicI64 = AtomicI64::new(0);
    let base = *BASE.get_or_init(now_s);
    base - STEP.fetch_add(1, Ordering::Relaxed) % 250
}

/// Signed by the wallet's own key, about now.
fn own(k: &SigningKey, handle: &str, ticket: Option<&str>, title: Option<&str>) -> Value {
    body(k, &wallet_of(k), handle, ticket, title, unique_ts())
}

async fn put(desk: &Running, b: &Value) -> (u16, Value) {
    let r = reqwest::Client::new()
        .put(format!("http://{}/profile", desk.addr))
        .json(b)
        .send()
        .await
        .unwrap();
    (r.status().as_u16(), r.json().await.unwrap())
}

async fn get<T: serde::de::DeserializeOwned>(desk: &Running, path: &str) -> (u16, T) {
    let r = reqwest::get(format!("http://{}{path}", desk.addr))
        .await
        .unwrap();
    (r.status().as_u16(), r.json().await.unwrap())
}

fn reason(v: &Value) -> String {
    assert_eq!(v["status"], "rejected", "{v}");
    v["reason"].as_str().unwrap().to_owned()
}

async fn desk(tag: &str) -> (TempDb, Running) {
    let pascal = pascal_stub().await;
    let db = TempDb::new(tag);
    let d = start_desk(&pascal, &db).await;
    (db, d)
}

/// An own tip (or, with `copied_from`, a copy) of `user`, one leg starting at `start_ms`.
async fn seed_tip(
    db: &SqlitePool,
    id: &str,
    user: &str,
    state: &str,
    start_ms: i64,
    tipster: Option<&str>,
    copied_from: Option<&str>,
) {
    static NONCE: AtomicI64 = AtomicI64::new(1);
    sqlx::query(
        "INSERT INTO tickets (ticket, pending, user, nonce, state, n, stake_u, rs_u, payout_u,
            odds_offered_u, odds_u, min_payout_per_unit_u, copy_fee_u, tipster, copied_from,
            minted_at, settled_at, paid_u, updated_ms, is_tip, chain_fee_u)
         VALUES (?, ?, ?, ?, ?, 1, ?, ?, ?, 6980000, 6980000, 1, 0, ?, ?, ?, ?, ?, 0, ?, 0)",
    )
    .bind(id)
    .bind(format!("pending-{id}"))
    .bind(user)
    .bind(NONCE.fetch_add(1, Ordering::Relaxed))
    .bind(state)
    .bind((5 * U) as i64)
    .bind((state != "pending").then_some((5 * U) as i64))
    .bind((5 * U * 698 / 100) as i64)
    .bind(tipster)
    .bind(copied_from)
    .bind((state != "pending").then_some(now_s() - 3600))
    .bind((state == "settled").then_some(now_s() - 60))
    .bind((state == "settled").then_some((10 * U) as i64))
    .bind(i64::from(tipster == Some(user) && copied_from.is_none()))
    .execute(db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO ticket_legs (ticket, idx, symbol, game_id, side, pick, start_ms,
            p_quote_u, p_fill_u, m1_ppm, status, resolution_u)
         VALUES (?, 0, 'NBA_26OCT28_PHIIND_ML.PHI', 'nba-phi-ind-2026-10-28', 'long', 'away', ?,
            500000, 500000, 30000, ?, ?)",
    )
    .bind(id)
    .bind(start_ms)
    .bind(if state == "settled" { "won" } else { "open" })
    .bind((state == "settled").then_some(U as i64))
    .execute(db)
    .await
    .unwrap();
}

/// The exact message bytes and one worked signature (deterministic ed25519): the vector 6.1e's
/// signing code reproduces.
#[test]
fn the_signed_message_is_pinned() {
    let k = key(1);
    let wallet = wallet_of(&k);
    assert_eq!(wallet, "AKnL4NNf3DGWZJS6cPknBuEGnVsV4A4m5tgebLHaRSZ9");
    let ticket = Pubkey::new_from_array([2; 32]).to_string();
    let msg = profiles::message(
        &wallet,
        "lorenzo_hoops",
        Some(&ticket),
        Some("Thursday treble"),
        1_793_300_000,
    );
    assert_eq!(
        msg,
        "betfi/profile/v1\nAKnL4NNf3DGWZJS6cPknBuEGnVsV4A4m5tgebLHaRSZ9\nlorenzo_hoops\n\
         8qbHbw2BbbTHBW1sbeqakYXVKRQM8Ne7pLK7m6CVfeR\nThursday treble\n1793300000"
    );
    let sig = base64::engine::general_purpose::STANDARD.encode(k.sign(msg.as_bytes()).to_bytes());
    assert_eq!(
        sig,
        "TMMf9L3Wbku8HKvr+AjkHk7DtikyFAQarijRFZzZDFpHTQkwpM170ACs+JQqYesuyFx9EGtys9IN8uJI7WQwCg=="
    );
    profiles::verify(&wallet, msg.as_bytes(), &sig).unwrap();
    // Without a title: both optional fields are "-".
    assert_eq!(
        profiles::message(&wallet, "lorenzo_hoops", None, None, 1_793_300_000),
        format!("betfi/profile/v1\n{wallet}\nlorenzo_hoops\n-\n-\n1793300000")
    );
}

#[tokio::test]
async fn signatures_clock_and_replays() {
    let (_db, desk) = desk("profiles-sig").await;
    let (a, b) = (key(10), key(11));
    let wa = wallet_of(&a);

    // Signed by another wallet's key.
    let (s, e) = put(&desk, &body(&b, &wa, "alice", None, None, now_s())).await;
    assert_eq!(s, 422);
    assert!(reason(&e).contains("does not match"), "{e}");
    // A signature over a different handle than the one sent.
    let mut forged = own(&a, "alice", None, None);
    forged["handle"] = json!("mallory");
    let (s, _) = put(&desk, &forged).await;
    assert_eq!(s, 422);
    // Stale or future ts.
    for ts in [now_s() - 301, now_s() + 301] {
        let (s, e) = put(&desk, &body(&a, &wa, "alice", None, None, ts)).await;
        assert_eq!(s, 422);
        assert!(reason(&e).contains("300 s"), "{e}");
    }
    // Malformed: not a wallet, not base64, bad handle, a ticket without a title.
    let mut bad = own(&a, "alice", None, None);
    bad["wallet"] = json!("nope");
    assert_eq!(put(&desk, &bad).await.0, 422);
    let mut bad = own(&a, "alice", None, None);
    bad["sig"] = json!("!!");
    assert_eq!(put(&desk, &bad).await.0, 422);
    let t = Pubkey::new_unique().to_string();
    assert_eq!(put(&desk, &own(&a, "alice", Some(&t), None)).await.0, 422);
    assert_eq!(put(&desk, &own(&a, "Alice", None, None)).await.0, 422);
    assert_eq!(put(&desk, &own(&a, "betfi_team", None, None)).await.0, 422);
    let r = reqwest::Client::new()
        .put(format!("http://{}/profile", desk.addr))
        .header("content-type", "application/json")
        .body("{\"wallet\":1}")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 422);

    // 299 s off is inside the window.
    let ok = body(&a, &wa, "alice", None, None, now_s() - 299);
    let (s, p) = put(&desk, &ok).await;
    assert_eq!(s, 200, "{p}");
    let p: ProfileResponse = serde_json::from_value(p).unwrap();
    assert_eq!(p.handle.as_deref(), Some("alice"));
    assert!((p.since - now_s()).abs() < 5);
    // The same signed request again: refused.
    let (s, e) = put(&desk, &ok).await;
    assert_eq!(s, 409);
    assert!(reason(&e).contains("already used"), "{e}");

    // GET: known, unknown (handle null, never 404), not a wallet (422).
    let (s, p): (u16, ProfileResponse) = get(&desk, &format!("/profile/{wa}")).await;
    assert_eq!((s, p.handle.as_deref()), (200, Some("alice")));
    let stranger = Pubkey::new_unique().to_string();
    let (s, p): (u16, ProfileResponse) = get(&desk, &format!("/profile/{stranger}")).await;
    assert_eq!(
        (s, p),
        (
            200,
            ProfileResponse {
                wallet: stranger,
                handle: None,
                since: 0,
                verified: false,
                links: Default::default(),
                followers: 0,
                following: 0,
            }
        )
    );
    let (s, _): (u16, Value) = get(&desk, "/profile/not-a-wallet").await;
    assert_eq!(s, 422);
    desk.join().await;
}

#[tokio::test]
async fn handles_are_unique_and_change_once_a_day() {
    let (_db, desk) = desk("profiles-handles").await;
    let pool = desk.state.db.clone();
    let (a, b, c) = (key(20), key(21), key(22));
    let (wa, wb) = (wallet_of(&a), wallet_of(&b));

    assert_eq!(
        put(&desk, &own(&a, "lorenzo_hoops", None, None)).await.0,
        200
    );
    // Taken, whatever the case: the same handle is a conflict, an upper-case spelling is not
    // even a valid handle, and the index refuses a mixed-case row written behind the desk's back.
    let (s, e) = put(&desk, &own(&b, "lorenzo_hoops", None, None)).await;
    assert_eq!(s, 409);
    assert!(reason(&e).contains("taken"), "{e}");
    assert_eq!(
        put(&desk, &own(&b, "LORENZO_HOOPS", None, None)).await.0,
        422
    );
    let direct = sqlx::query(
        "INSERT INTO profiles (wallet, handle, handle_set_at, created_at, updated_ms)
         VALUES ('w', 'Lorenzo_Hoops', 0, 0, 0)",
    )
    .execute(&pool)
    .await;
    assert!(direct.is_err(), "NOCASE unique index");

    // Re-sending the current handle is no change (that is how a title is set alone).
    assert_eq!(
        put(&desk, &own(&a, "lorenzo_hoops", None, None)).await.0,
        200
    );
    // A second change within 24 h.
    let (s, e) = put(&desk, &own(&a, "lorenzo_2", None, None)).await;
    assert_eq!(s, 409);
    assert!(reason(&e).contains("24 h"), "{e}");
    // A day later it goes through, and the old handle is held for A for 30 days.
    sqlx::query("UPDATE profiles SET handle_set_at = handle_set_at - 86400 WHERE wallet = ?")
        .bind(&wa)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(put(&desk, &own(&a, "lorenzo_2", None, None)).await.0, 200);
    let (s, e) = put(&desk, &own(&b, "lorenzo_hoops", None, None)).await;
    assert_eq!(s, 409);
    assert!(reason(&e).contains("held"), "{e}");
    // B's first handle starts B's clock too.
    assert_eq!(put(&desk, &own(&b, "bob_b", None, None)).await.0, 200);
    assert_eq!(put(&desk, &own(&b, "bob_c", None, None)).await.0, 409);
    // After the hold, anyone may take it.
    sqlx::query("UPDATE profile_released_handles SET released_at = released_at - 2592000")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        put(&desk, &own(&c, "lorenzo_hoops", None, None)).await.0,
        200
    );
    let (_, p): (u16, ProfileResponse) = get(&desk, &format!("/profile/{wb}")).await;
    assert_eq!(p.handle.as_deref(), Some("bob_b"));
    desk.join().await;
}

#[tokio::test]
async fn titles_once_by_the_tipster_while_the_tip_is_open() {
    let (_db, desk) = desk("profiles-titles").await;
    let pool = desk.state.db.clone();
    let (a, b) = (key(30), key(31));
    let (wa, wb) = (wallet_of(&a), wallet_of(&b));
    let later = (desk::now_ms() + 3_600_000) as i64;
    let earlier = (desk::now_ms() - 60_000) as i64;
    let [mine, theirs, started, copy, settled] = [(); 5].map(|_| Pubkey::new_unique().to_string());
    seed_tip(&pool, &mine, &wa, "open", later, Some(&wa), None).await;
    seed_tip(&pool, &theirs, &wb, "open", later, Some(&wb), None).await;
    seed_tip(&pool, &started, &wa, "open", earlier, Some(&wa), None).await;
    seed_tip(&pool, &copy, &wa, "open", later, Some(&wb), Some(&theirs)).await;
    seed_tip(&pool, &settled, &wa, "settled", later, Some(&wa), None).await;

    let mut rx = desk.state.subscribe();
    // Someone else's tip.
    let (s, e) = put(&desk, &own(&a, "anna", Some(&theirs), Some("Mine now"))).await;
    assert_eq!(s, 422);
    assert!(reason(&e).contains("not yours"), "{e}");
    // A copy is not an own tip.
    let (s, e) = put(&desk, &own(&a, "anna", Some(&copy), Some("Copied"))).await;
    assert_eq!(s, 422);
    assert!(reason(&e).contains("not a tip of yours"), "{e}");
    // Settled, unknown.
    assert_eq!(
        put(&desk, &own(&a, "anna", Some(&settled), Some("Done")))
            .await
            .0,
        409
    );
    let nobody = Pubkey::new_unique().to_string();
    assert_eq!(
        put(&desk, &own(&a, "anna", Some(&nobody), Some("Ghost")))
            .await
            .0,
        422
    );
    // Control characters and bidi overrides.
    assert_eq!(
        put(&desk, &own(&a, "anna", Some(&mine), Some("a\u{202E}b")))
            .await
            .0,
        422
    );
    // A refused title took nothing with it: the handle of the failed requests was not set.
    let (_, p): (u16, ProfileResponse) = get(&desk, &format!("/profile/{wa}")).await;
    assert_eq!(p.handle, None);

    // The tipster titles their own open tip, trimmed; the handle is set in the same request.
    let (s, p) = put(
        &desk,
        &own(&a, "anna", Some(&mine), Some("  Thursday treble ")),
    )
    .await;
    assert_eq!(s, 200, "{p}");
    assert_eq!(p["handle"], "anna");
    let (_, t): (u16, Ticket) = get(&desk, &format!("/ticket/{mine}")).await;
    assert_eq!(t.title.as_deref(), Some("Thursday treble"));
    assert_eq!(t.posted_odds_u, 6_980_000);
    // Once only.
    let (s, e) = put(
        &desk,
        &own(&a, "anna", Some(&mine), Some("Second thoughts")),
    )
    .await;
    assert_eq!(s, 409);
    assert!(reason(&e).contains("already has a title"), "{e}");
    // A tip whose first game has started is titled too (D-660: tips may be live).
    let (s, p) = put(&desk, &own(&a, "anna", Some(&started), Some("In play"))).await;
    assert_eq!(s, 200, "{p}");
    // The stream saw the titled ticket after the write.
    let frame = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(DeskEvent::TicketChanged(t)) = rx.recv().await
                && t.ticket == mine
            {
                return t;
            }
        }
    })
    .await
    .expect("ticket frame");
    assert_eq!(frame.title.as_deref(), Some("Thursday treble"));
    // Others' tickets have no title.
    let (_, t): (u16, Ticket) = get(&desk, &format!("/ticket/{theirs}")).await;
    assert_eq!(t.title, None);
    desk.join().await;
}

#[tokio::test]
async fn handles_and_titles_reach_the_tipster_routes() {
    let (_db, desk) = desk("profiles-tipsters").await;
    let pool = desk.state.db.clone();
    let a = key(40);
    let wa = wallet_of(&a);
    let later = (desk::now_ms() + 3_600_000) as i64;
    // Five settled tips of 5 USDC minted today (verified: settled ≥ 5, skin 25 ≥ 20), one open.
    for _ in 0..5 {
        let id = Pubkey::new_unique().to_string();
        seed_tip(&pool, &id, &wa, "settled", later, Some(&wa), None).await;
    }
    let open = Pubkey::new_unique().to_string();
    seed_tip(&pool, &open, &wa, "open", later, Some(&wa), None).await;
    assert_eq!(
        put(
            &desk,
            &own(&a, "anna_tips", Some(&open), Some("Wednesday double"))
        )
        .await
        .0,
        200
    );
    let (_, board): (u16, TipstersResponse) = get(&desk, "/tipsters").await;
    let row = board.tipsters.iter().find(|r| r.wallet == wa).unwrap();
    assert_eq!(row.handle.as_deref(), Some("anna_tips"));
    assert!(row.verified);
    // 5 × (10 − 5) / 25 = +100%.
    assert_eq!((row.roi_30d_ppm, row.roi_ppm), (1_000_000, 1_000_000));
    let (_, one): (u16, TipsterResponse) = get(&desk, &format!("/tipsters/{wa}")).await;
    assert_eq!(one.tipster, *row);
    assert_eq!(one.open[0].title.as_deref(), Some("Wednesday double"));
    assert!(one.settled.iter().all(|t| t.title.is_none()));
    assert!(one.settled.iter().all(|t| t.posted_odds_u == 6_980_000));
    desk.join().await;
}

#[tokio::test]
async fn writes_are_rate_limited_per_wallet() {
    let (_db, desk) = desk("profiles-limit").await;
    let (a, b) = (key(50), key(51));
    let wa = wallet_of(&a);
    // Junk for A signed by B is refused before it can spend A's budget.
    for _ in 0..15 {
        let forged = body(&b, &wa, "rate_me", None, None, unique_ts());
        assert_eq!(put(&desk, &forged).await.0, 422);
    }
    // Each request is a fresh signature (ts differs), all for the same handle.
    let mut codes = Vec::new();
    for _ in 0..=WALLET_LIMIT.0 {
        codes.push(put(&desk, &own(&a, "rate_me", None, None)).await.0);
    }
    assert!(
        codes[..WALLET_LIMIT.0].iter().all(|c| *c == 200),
        "{codes:?}"
    );
    assert_eq!(codes[WALLET_LIMIT.0], 429);
    // Another wallet has its own budget.
    assert_eq!(put(&desk, &own(&b, "other_one", None, None)).await.0, 200);
    // The address's budget counts every write, valid or not.
    let sent = 15 + WALLET_LIMIT.0 + 1 + 1;
    for _ in sent..IP_LIMIT.0 {
        assert_eq!(put(&desk, &json!({})).await.0, 422);
    }
    let (s, e) = put(&desk, &own(&key(52), "fresh_one", None, None)).await;
    assert_eq!(s, 429);
    assert!(reason(&e).contains("address"), "{e}");
    desk.join().await;
}
