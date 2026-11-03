//! Plan 7.0c review gate, profiles (l7-contracts L7-1): links and follows through the signed
//! `PUT /profile` (v2 message, pinned byte for byte), with the v1 message still accepted; a bad
//! signature, a replay, a URL as a link, a self-follow and a v1 signature over v2 fields are
//! refused; follows toggle idempotently and the counts follow; `GET /profile/{wallet}` serves
//! `links`, `followers`, `following` and `verified` (= on the tipster board, D-304).

mod common;

use std::sync::atomic::{AtomicI64, Ordering};

use base64::Engine;
use common::{TempDb, pascal_stub, start_desk};
use desk::Running;
use desk::api::types::{FollowRequest, ProfileLinks, ProfileResponse, TipsterResponse};
use desk::profiles;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use solana_pubkey::Pubkey;

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

/// A `ts` near now no other call here returns (ed25519 is deterministic).
fn unique_ts() -> i64 {
    static BASE: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    static STEP: AtomicI64 = AtomicI64::new(0);
    let base = *BASE.get_or_init(now_s);
    base - STEP.fetch_add(1, Ordering::Relaxed) % 250
}

fn sign(k: &SigningKey, msg: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(k.sign(msg.as_bytes()).to_bytes())
}

/// A v2 body by `k` for its own wallet.
fn v2(
    k: &SigningKey,
    handle: &str,
    links: Option<&ProfileLinks>,
    follow: Option<&FollowRequest>,
) -> Value {
    let wallet = wallet_of(k);
    let ts = unique_ts();
    let msg = profiles::message_v2(&wallet, handle, None, None, links, follow, ts);
    let mut b = json!({"wallet": wallet, "ts": ts, "sig": sign(k, &msg)});
    if !handle.is_empty() {
        b["handle"] = json!(handle);
    }
    if let Some(l) = links {
        b["links"] = serde_json::to_value(l).unwrap();
    }
    if let Some(f) = follow {
        b["follow"] = serde_json::to_value(f).unwrap();
    }
    b
}

fn follow(w: &str, on: bool) -> FollowRequest {
    FollowRequest {
        wallet: w.to_owned(),
        on,
    }
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

async fn profile(desk: &Running, w: &str) -> ProfileResponse {
    reqwest::get(format!("http://{}/profile/{w}", desk.addr))
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
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

/// The v2 message bytes and one worked signature: the vector L7-1's signing code reproduces.
#[test]
fn the_v2_message_is_pinned() {
    let k = key(1);
    let wallet = wallet_of(&k);
    assert_eq!(wallet, "AKnL4NNf3DGWZJS6cPknBuEGnVsV4A4m5tgebLHaRSZ9");
    let followee = Pubkey::new_from_array([2; 32]).to_string();
    let links = ProfileLinks {
        x: Some("lorenzo_hoops".into()),
        youtube: None,
        telegram: Some("@lorenzo_tg".into()),
        instagram: None,
    };
    let msg = profiles::message_v2(
        &wallet,
        "lorenzo_hoops",
        None,
        None,
        Some(&links),
        Some(&follow(&followee, true)),
        1_793_300_000,
    );
    assert_eq!(
        msg,
        "betfi/profile/v2\nAKnL4NNf3DGWZJS6cPknBuEGnVsV4A4m5tgebLHaRSZ9\nlorenzo_hoops\n-\n-\n\
         x=lorenzo_hoops;youtube=-;telegram=@lorenzo_tg;instagram=-\n\
         8qbHbw2BbbTHBW1sbeqakYXVKRQM8Ne7pLK7m6CVfeR:on\n1793300000"
    );
    let sig = sign(&k, &msg);
    assert_eq!(sig, PINNED_V2_SIG);
    profiles::verify(&wallet, msg.as_bytes(), &sig).unwrap();
    // Follow-only, without a handle.
    assert_eq!(
        profiles::message_v2(
            &wallet,
            "",
            None,
            None,
            None,
            Some(&follow(&followee, false)),
            7
        ),
        format!("betfi/profile/v2\n{wallet}\n-\n-\n-\n-\n{followee}:off\n7")
    );
}

/// `sign(key(1), <the message above>)`.
const PINNED_V2_SIG: &str =
    "02h+W3/CbAWb4efkkCTU8F0Lr9sTBb3wuJIqxbhKL6cMzkoPBXziljjs4/APxvjoMJJ9CzDhJOIXymrkbAWHCQ==";

#[tokio::test]
async fn links_are_signed_validated_and_served() {
    let (_db, desk) = desk("profiles-v2-links").await;
    let (a, b) = (key(40), key(41));
    let wa = wallet_of(&a);
    let links = ProfileLinks {
        x: Some("@Lorenzo_Hoops".into()),
        youtube: Some("lorenzo.hoops".into()),
        telegram: None,
        instagram: Some("lorenzo.hoops".into()),
    };

    // The v1 message still sets a handle.
    let ts = unique_ts();
    let v1 = profiles::message(&wa, "lorenzo_hoops", None, None, ts);
    let (s, p) = put(
        &desk,
        &json!({"wallet": wa, "handle": "lorenzo_hoops", "ts": ts, "sig": sign(&a, &v1)}),
    )
    .await;
    assert_eq!(s, 200, "{p}");

    // Links signed over v2: stored without the `@`, served by GET and in the PUT answer.
    let ok = v2(&a, "lorenzo_hoops", Some(&links), None);
    let (s, p) = put(&desk, &ok).await;
    assert_eq!(s, 200, "{p}");
    let p: ProfileResponse = serde_json::from_value(p).unwrap();
    let stored = ProfileLinks {
        x: Some("Lorenzo_Hoops".into()),
        ..links.clone()
    };
    assert_eq!(p.links, stored);
    assert_eq!(profile(&desk, &wa).await.links, stored);
    // Replayed: refused, and nothing changes.
    let (s, e) = put(&desk, &ok).await;
    assert_eq!(s, 409);
    assert!(reason(&e).contains("already used"), "{e}");

    // The same fields signed over v1 (or by another key, or edited after signing): refused.
    let ts = unique_ts();
    let mut forged = json!({"wallet": wa, "handle": "lorenzo_hoops", "ts": ts,
        "sig": sign(&a, &profiles::message(&wa, "lorenzo_hoops", None, None, ts)),
        "links": serde_json::to_value(&links).unwrap()});
    assert_eq!(put(&desk, &forged).await.0, 422);
    let mut by_b = v2(&b, "lorenzo_hoops", Some(&links), None);
    by_b["wallet"] = json!(wa);
    let (s, e) = put(&desk, &by_b).await;
    assert_eq!(s, 422);
    assert!(reason(&e).contains("does not match"), "{e}");
    forged = v2(&a, "lorenzo_hoops", Some(&links), None);
    forged["links"]["x"] = json!("mallory");
    assert_eq!(put(&desk, &forged).await.0, 422);

    // URLs and odd characters are refused before anything is written.
    for (net, v) in [
        ("x", "https://x.com/lorenzo"),
        ("youtube", "youtube.com/@lorenzo"),
        ("telegram", "t.me/lorenzo_tg"),
        ("instagram", "lorenzo;x=1"),
        ("x", "-"),
    ] {
        let mut l = ProfileLinks::default();
        match net {
            "x" => l.x = Some(v.into()),
            "youtube" => l.youtube = Some(v.into()),
            "telegram" => l.telegram = Some(v.into()),
            _ => l.instagram = Some(v.into()),
        }
        let (s, e) = put(&desk, &v2(&a, "lorenzo_hoops", Some(&l), None)).await;
        assert_eq!(s, 422, "{net} {v}");
        assert!(reason(&e).contains("not a URL"), "{e}");
    }
    assert_eq!(profile(&desk, &wa).await.links, stored);

    // Links need a handle; `links` replaces all four (null clears).
    let (s, _) = put(&desk, &v2(&b, "", Some(&links), None)).await;
    assert_eq!(s, 422);
    let only_x = ProfileLinks {
        x: Some("lorenzo_hoops".into()),
        ..ProfileLinks::default()
    };
    let (s, _) = put(&desk, &v2(&a, "lorenzo_hoops", Some(&only_x), None)).await;
    assert_eq!(s, 200);
    assert_eq!(profile(&desk, &wa).await.links, only_x);
    // A v2 message with no new fields is accepted too (the handle alone).
    let (s, _) = put(&desk, &v2(&a, "lorenzo_hoops", None, None)).await;
    assert_eq!(s, 200);
    assert_eq!(profile(&desk, &wa).await.links, only_x);

    // The exact L7-1 keys.
    let v: Value = reqwest::get(format!("http://{}/profile/{wa}", desk.addr))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        v["links"],
        json!({"x": "lorenzo_hoops", "youtube": null, "telegram": null, "instagram": null})
    );
    println!("served GET /profile/{{wallet}}: {v}");
    for k in [
        "wallet",
        "handle",
        "since",
        "verified",
        "followers",
        "following",
    ] {
        assert!(v.get(k).is_some(), "{k}");
    }
    desk.join().await;
}

#[tokio::test]
async fn follows_toggle_and_count() {
    let (_db, desk) = desk("profiles-v2-follow").await;
    let (a, b, c) = (key(50), key(51), key(52));
    let (wa, wb, wc) = (wallet_of(&a), wallet_of(&b), wallet_of(&c));

    // b (no handle) and c (with one) follow a; following twice counts once.
    let (s, p) = put(&desk, &v2(&b, "", None, Some(&follow(&wa, true)))).await;
    assert_eq!(s, 200, "{p}");
    let p: ProfileResponse = serde_json::from_value(p).unwrap();
    assert_eq!((p.handle, p.following, p.followers), (None, 1, 0));
    put(&desk, &v2(&b, "", None, Some(&follow(&wa, true)))).await;
    let (s, _) = put(&desk, &v2(&c, "carla", None, Some(&follow(&wa, true)))).await;
    assert_eq!(s, 200);
    let pa = profile(&desk, &wa).await;
    assert_eq!((pa.followers, pa.following, pa.handle), (2, 0, None));
    assert_eq!(profile(&desk, &wb).await.following, 1);
    assert_eq!(profile(&desk, &wc).await.handle.as_deref(), Some("carla"));

    // Unfollow (twice: idempotent).
    for _ in 0..2 {
        let (s, _) = put(&desk, &v2(&b, "", None, Some(&follow(&wa, false)))).await;
        assert_eq!(s, 200);
    }
    assert_eq!(profile(&desk, &wa).await.followers, 1);
    assert_eq!(profile(&desk, &wb).await.following, 0);

    // Refused: oneself, not a wallet, a follow signed over v1, an empty handle without follow.
    let (s, e) = put(&desk, &v2(&a, "", None, Some(&follow(&wa, true)))).await;
    assert_eq!(s, 422);
    assert!(reason(&e).contains("itself"), "{e}");
    assert_eq!(
        put(&desk, &v2(&a, "", None, Some(&follow("nope", true))))
            .await
            .0,
        422
    );
    let ts = unique_ts();
    let v1 = profiles::message(&wb, "bob_b", None, None, ts);
    let (s, _) = put(
        &desk,
        &json!({"wallet": wb, "handle": "bob_b", "ts": ts, "sig": sign(&b, &v1),
            "follow": {"wallet": wa, "on": true}}),
    )
    .await;
    assert_eq!(s, 422);
    assert_eq!(put(&desk, &v2(&b, "", None, None)).await.0, 422);
    assert_eq!(profile(&desk, &wa).await.followers, 1);
    desk.join().await;
}

/// `verified` is the board rule: five settled own tips and ≥ 20 USDC of skin (D-304).
#[tokio::test]
async fn verified_is_on_the_board() {
    let (_db, desk) = desk("profiles-v2-verified").await;
    let pool = desk.state.db.clone();
    let tipster = Pubkey::new_unique().to_string();
    assert!(!profile(&desk, &tipster).await.verified);
    for i in 0..5 {
        let id = Pubkey::new_unique().to_string();
        sqlx::query(
            "INSERT INTO tickets (ticket, pending, user, nonce, state, n, stake_u, rs_u, payout_u,
                odds_offered_u, odds_u, min_payout_per_unit_u, copy_fee_u, tipster, minted_at,
                settled_at, paid_u, updated_ms, is_tip, chain_fee_u)
             VALUES (?, ?, ?, ?, 'settled', 1, ?, ?, ?, 2000000, 2000000, 1, 0, ?, ?, ?, ?, 0, 1, 0)",
        )
        .bind(&id)
        .bind(format!("pending-{id}"))
        .bind(&tipster)
        .bind(i64::from(i))
        .bind((5 * U) as i64)
        .bind((5 * U) as i64)
        .bind((10 * U) as i64)
        .bind(&tipster)
        .bind(now_s() - 7200)
        .bind(now_s() - 60)
        .bind((10 * U) as i64)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO ticket_legs (ticket, idx, symbol, game_id, side, pick, start_ms,
                p_quote_u, p_fill_u, m1_ppm, status, resolution_u)
             VALUES (?, 0, 'NBA_26OCT28_PHIIND_ML.PHI', 'nba-phi-ind-2026-10-28', 'long', 'away',
                0, 500000, 500000, 30000, 'won', 1000000)",
        )
        .bind(&id)
        .execute(&pool)
        .await
        .unwrap();
    }
    let t: TipsterResponse = reqwest::get(format!("http://{}/tipsters/{tipster}", desk.addr))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(t.tipster.verified, "{:?}", t.tipster);
    assert!(profile(&desk, &tipster).await.verified);
    desk.join().await;
}
