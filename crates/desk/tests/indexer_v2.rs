//! Plan 7.0c review gate, indexer (l7-contracts L7-1): `Ticket.tx.slot` and `Ticket.tx.confirm_ms`
//! from indexer replays (written once, from the transaction that made the transition; a resync
//! never invents them; `null` without a block time), and the ticker's `feed` frames for mints and
//! copies (`who`, `amount_u`, `team`, `odds_u`, kind `copy`). Synthetic chain state, no network.

mod common;

use std::time::Duration;

use common::{TempDb, pascal_stub, start_desk};
use desk::api::types::{FeedKind, QuoteLeg as ApiQuoteLeg, Side, StreamFrame, TeamSide, Ticket};
use desk::indexer::store::{self, PendingData, TxFacts};
use desk::program::{
    LegStatus as ChainLeg, Pdas, QuoteLeg, TicketAccount, TicketLeg, TicketState as ChainState,
    hex, sha256,
};
use desk::state::FeedItem;
use desk::{AppState, DeskEvent, Running};
use solana_pubkey::Pubkey;
use tokio::sync::broadcast;

const PHI: (&str, &str, i64) = (
    "NBA_26OCT28_PHIIND_ML.PHI",
    "nba-phi-ind-2026-10-28",
    1_793_228_400,
);
const ORL: (&str, &str, i64) = (
    "NBA_26OCT28_ORLTOR_ML.ORL",
    "nba-orl-tor-2026-10-28",
    1_793_230_200,
);
/// Block time of every `open_pending` here, Unix s.
const OPENED_AT: i64 = 1_793_213_000;

fn leg((symbol, game, start): (&str, &str, i64), side: u8, price_u: u64) -> QuoteLeg {
    QuoteLeg {
        symbol_hash: sha256(symbol.as_bytes()),
        game_id_hash: sha256(game.as_bytes()),
        start_ts: start,
        side,
        price_u,
        m1_ppm: 50_000,
    }
}

fn pending(pdas: &Pdas, user: Pubkey, nonce: u64, legs: Vec<QuoteLeg>) -> PendingData {
    PendingData {
        pending: pdas.pending(&user, nonce),
        user,
        nonce,
        legs,
        stake_u: 5_000_000,
        copy_fee_u: 0,
        odds_offered_u: 2_900_000,
        min_payout_per_unit_u: 2_800_000,
        quote_expires_at: OPENED_AT,
        expires_at: OPENED_AT + 40,
        quote_hash: [nonce as u8; 32],
        is_tip: false,
        tipster: None,
        copied_from: None,
        opened_at: Some(OPENED_AT),
    }
}

fn ticket_of(p: &PendingData) -> TicketAccount {
    let legs: Vec<TicketLeg> = p
        .legs
        .iter()
        .map(|l| TicketLeg {
            symbol_hash: l.symbol_hash,
            game_id_hash: l.game_id_hash,
            start_ts: l.start_ts,
            side: l.side,
            price_u: l.price_u,
            fill_u: l.price_u,
            m1_ppm: l.m1_ppm,
            hedged: true,
            status: ChainLeg::Open,
            resolution_ppm: 0,
        })
        .collect();
    let payout = pricing::payout(
        p.stake_u,
        1_000_000,
        &legs
            .iter()
            .map(|l| pricing::PayoutLeg {
                m1_ppm: l.m1_ppm,
                f_u: l.fill_u,
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
    TicketAccount {
        user: p.user,
        nonce: p.nonce,
        legs,
        stake_u: payout.rs_u,
        refund_u: 0,
        payout_u: payout.payout_u,
        odds_u: payout.odds_u,
        odds_offered_u: p.odds_offered_u,
        min_payout_per_unit_u: p.min_payout_per_unit_u,
        r_ppm: 1_000_000,
        fee_u: 1,
        copy_fee_u: p.copy_fee_u,
        state: ChainState::Open,
        tipster: p.tipster,
        copied_from: p.copied_from,
        quote_hash: p.quote_hash,
        minted_at: OPENED_AT + 20,
        resolved_at: 0,
        paid_u: 0,
    }
}

/// The quote store rows 2.A writes for `p` (L2-1), with each leg's card label.
async fn store_quote(state: &AppState, p: &PendingData, names: &[(&str, &str, TeamSide, &str)]) {
    let legs: Vec<ApiQuoteLeg> = p
        .legs
        .iter()
        .zip(names)
        .map(|(l, (symbol, game, pick, label))| ApiQuoteLeg {
            symbol: (*symbol).into(),
            game_id: (*game).into(),
            side: if l.side == 1 { Side::Short } else { Side::Long },
            pick: *pick,
            start_ms: u64::try_from(l.start_ts).unwrap() * 1000,
            price_u: l.price_u,
            p_max_u: l.price_u + 10_000,
            m1_ppm: l.m1_ppm,
            overlaps: false,
            in_play: false,
            kind: None,
            line: None,
            label: Some((*label).into()),
        })
        .collect();
    sqlx::query(
        "INSERT INTO quotes (quote_hash, msg, sig, quote_signer, n, stake_u, odds_offered_u,
            odds_fair_u, payout_u, fee_u, min_payout_per_unit_u, quote_expires_at, legs_json,
            created_ms) VALUES (?, x'00', x'00', 'k', ?, ?, ?, 3500000, 14000000, 2000000, ?, ?, ?, 0)",
    )
    .bind(hex(&p.quote_hash))
    .bind(legs.len() as i64)
    .bind(p.stake_u as i64)
    .bind(p.odds_offered_u as i64)
    .bind(p.min_payout_per_unit_u as i64)
    .bind(p.quote_expires_at)
    .bind(serde_json::to_string(&legs).unwrap())
    .execute(&state.db)
    .await
    .unwrap();
}

fn feeds(rx: &mut broadcast::Receiver<DeskEvent>) -> Vec<FeedItem> {
    let mut out = vec![];
    while let Ok(e) = rx.try_recv() {
        if let DeskEvent::Feed(f) = e {
            out.push((*f).clone());
        }
    }
    out
}

async fn desk() -> (Running, TempDb) {
    let pascal = pascal_stub().await;
    let db = TempDb::new("indexer-v2");
    let desk = start_desk(&pascal, &db).await;
    for _ in 0..100 {
        if desk.state.catalogue().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(desk.state.catalogue().is_some(), "catalogue loaded");
    (desk, db)
}

async fn ticket(desk: &Running, id: &str) -> Ticket {
    reqwest::get(format!("http://{}/ticket/{id}", desk.addr))
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

fn ms(s: i64) -> u64 {
    u64::try_from(s).unwrap() * 1000
}

#[test]
fn confirm_ms_is_observed_minus_block_time() {
    assert_eq!(store::confirm_ms(Some(10), Some(10_640)), Some(640));
    assert_eq!(store::confirm_ms(None, Some(10_640)), None);
    assert_eq!(store::confirm_ms(Some(10), None), None);
    // A clock behind the block's whole second: 0, never negative.
    assert_eq!(store::confirm_ms(Some(10), Some(9_900)), Some(0));
    assert_eq!(store::confirm_ms(Some(-1), Some(1)), None);
    assert_eq!(
        store::short_address("7xK9ab3EeFgHhJjKkLmNnPpQqRrSsTtUuVvWwXx94Lp"),
        "7xK9…94Lp"
    );
    assert_eq!(store::short_address("short"), "short");
}

#[tokio::test(flavor = "multi_thread")]
async fn slots_and_confirm_times_from_indexer_replays() {
    let (desk, _db) = desk().await;
    let state = desk.state.clone();
    let pdas = Pdas::new(state.config.program_id);

    // A: open_pending applied from its transaction, then replayed (later clock) and resynced.
    let user = Pubkey::new_unique();
    let p = pending(
        &pdas,
        user,
        7,
        vec![leg(PHI, 0, 699_000), leg(ORL, 1, 629_000)],
    );
    let a = pdas.ticket(&user, 7).to_string();
    let seen = Some(ms(OPENED_AT) + 640);
    store::apply_pending_seen(&state, &pdas, &p, 418_000_001, Some("SIG_OPEN"), seen)
        .await
        .unwrap();
    store::apply_pending_seen(
        &state,
        &pdas,
        &p,
        418_000_009,
        Some("SIG_OPEN"),
        Some(ms(OPENED_AT) + 9_000),
    )
    .await
    .unwrap();
    store::apply_pending(&state, &pdas, &p, 418_000_500, None)
        .await
        .unwrap();
    let t = ticket(&desk, &a).await;
    assert_eq!(t.tx.slot.open_pending, Some(418_000_001));
    assert_eq!(t.tx.confirm_ms.open_pending, Some(640));
    assert_eq!((t.tx.slot.cash_out, t.tx.slot.settle), (None, None));
    assert_eq!(t.tx.confirm_ms.cash_out, None);

    // Minted, then cashed out by its own transaction (block time known).
    let key = pdas.ticket(&user, 7);
    let mut acc = ticket_of(&p);
    let mint = TxFacts {
        sig: Some("SIG_FIN".into()),
        minted: true,
        block_time: Some(OPENED_AT + 20),
        slot: Some(418_000_050),
        observed_ms: Some(ms(OPENED_AT + 20) + 500),
        ..TxFacts::default()
    };
    store::apply_ticket(&state, &pdas, &key, &acc, 418_000_050, &mint)
        .await
        .unwrap();
    acc.state = ChainState::CashedOut;
    acc.paid_u = 4_000_000;
    acc.resolved_at = 1_793_230_000;
    let co = TxFacts {
        sig: Some("SIG_CO".into()),
        cashed_out: Some((4_000_000, 1_793_230_000)),
        block_time: Some(1_793_230_000),
        slot: Some(418_100_000),
        observed_ms: Some(ms(1_793_230_000) + 380),
        ..TxFacts::default()
    };
    store::apply_ticket(&state, &pdas, &key, &acc, 418_100_002, &co)
        .await
        .unwrap();
    // Replayed later: nothing moves.
    let late = TxFacts {
        observed_ms: Some(ms(1_793_230_000) + 60_000),
        ..co.clone()
    };
    store::apply_ticket(&state, &pdas, &key, &acc, 418_100_900, &late)
        .await
        .unwrap();
    let t = ticket(&desk, &a).await;
    assert_eq!(t.tx.cash_out.as_deref(), Some("SIG_CO"));
    assert_eq!(t.tx.slot.cash_out, Some(418_100_000));
    assert_eq!(t.tx.confirm_ms.cash_out, Some(380));
    assert_eq!(t.tx.slot.open_pending, Some(418_000_001));
    assert_eq!(t.tx.slot.settle, None);

    // B: found by a resync first (open, then settled): no slot is invented; the settle
    // transaction arriving later backfills its signature and slot (no confirm time for settle).
    let user_b = Pubkey::new_unique();
    let pb = pending(&pdas, user_b, 8, vec![leg(PHI, 0, 699_000)]);
    let b = pdas.ticket(&user_b, 8).to_string();
    store::apply_pending(&state, &pdas, &pb, 418_200_000, None)
        .await
        .unwrap();
    let mut acc_b = ticket_of(&pb);
    acc_b.legs[0].status = ChainLeg::Won;
    acc_b.legs[0].resolution_ppm = 1_000_000;
    acc_b.state = ChainState::Settled;
    acc_b.paid_u = acc_b.payout_u;
    acc_b.resolved_at = 1_793_250_000;
    let key_b = pdas.ticket(&user_b, 8);
    store::apply_ticket(
        &state,
        &pdas,
        &key_b,
        &acc_b,
        418_300_000,
        &TxFacts::default(),
    )
    .await
    .unwrap();
    let t = ticket(&desk, &b).await;
    assert_eq!(t.tx.slot, Default::default());
    assert_eq!(t.tx.confirm_ms, Default::default());
    let settle = TxFacts {
        sig: Some("SIG_SET".into()),
        settled: true,
        block_time: Some(1_793_250_000),
        slot: Some(418_299_990),
        observed_ms: Some(ms(1_793_250_000) + 200),
        ..TxFacts::default()
    };
    store::apply_ticket(&state, &pdas, &key_b, &acc_b, 418_300_001, &settle)
        .await
        .unwrap();
    // The open_pending event arrives last (block time unknown): slot yes, confirm null.
    let pb_late = PendingData {
        opened_at: None,
        ..pb.clone()
    };
    store::apply_pending_seen(
        &state,
        &pdas,
        &pb_late,
        418_199_000,
        Some("SIG_OPEN_B"),
        Some(ms(OPENED_AT) + 1_000),
    )
    .await
    .unwrap();
    let t = ticket(&desk, &b).await;
    assert_eq!(t.tx.settle.as_deref(), Some("SIG_SET"));
    assert_eq!(t.tx.slot.settle, Some(418_299_990));
    assert_eq!(t.tx.open_pending.as_deref(), Some("SIG_OPEN_B"));
    assert_eq!(t.tx.slot.open_pending, Some(418_199_000));
    assert_eq!(t.tx.confirm_ms.open_pending, None);

    // The exact L7-1 keys, served.
    let v: serde_json::Value = reqwest::get(format!("http://{}/ticket/{a}", desk.addr))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    println!("served Ticket.tx: {}", v["tx"]);
    assert_eq!(
        v["tx"]["slot"],
        serde_json::json!({"open_pending": 418_000_001, "cash_out": 418_100_000, "settle": null})
    );
    assert_eq!(
        v["tx"]["confirm_ms"],
        serde_json::json!({"open_pending": 640, "cash_out": 380})
    );
    desk.join().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mint_and_copy_feed_frames_carry_who_amount_team_and_odds() {
    let (desk, _db) = desk().await;
    let state = desk.state.clone();
    let pdas = Pdas::new(state.config.program_id);
    let mut rx = state.subscribe();

    // The tipster has a handle; the copier does not.
    let tipster = Pubkey::new_unique();
    sqlx::query(
        "INSERT INTO profiles (wallet, handle, handle_set_at, created_at, updated_ms)
         VALUES (?, 'lorenzo_hoops', 1, 1, 0)",
    )
    .bind(tipster.to_string())
    .execute(&state.db)
    .await
    .unwrap();
    let mut tip = pending(
        &pdas,
        tipster,
        1,
        vec![leg(PHI, 0, 699_000), leg(ORL, 1, 629_000)],
    );
    tip.is_tip = true;
    tip.tipster = Some(tipster);
    store_quote(
        &state,
        &tip,
        &[
            (PHI.0, PHI.1, TeamSide::Away, "76ers"),
            (ORL.0, ORL.1, TeamSide::Home, "Raptors"),
        ],
    )
    .await;
    store::apply_pending(&state, &pdas, &tip, 1, Some("S1"))
        .await
        .unwrap();
    let tip_acc = ticket_of(&tip);
    let mint = TxFacts {
        sig: Some("F1".into()),
        minted: true,
        ..TxFacts::default()
    };
    let tip_key = pdas.ticket(&tipster, 1);
    store::apply_ticket(&state, &pdas, &tip_key, &tip_acc, 2, &mint)
        .await
        .unwrap();
    // Replayed: no second feed item.
    store::apply_ticket(&state, &pdas, &tip_key, &tip_acc, 3, &TxFacts::default())
        .await
        .unwrap();
    let f = feeds(&mut rx);
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].kind, FeedKind::Mint);
    assert_eq!(f[0].who.as_deref(), Some("lorenzo_hoops"));
    assert_eq!(f[0].amount_u, Some(tip_acc.stake_u));
    assert_eq!(f[0].team.as_deref(), Some("76ers"));
    assert_eq!(f[0].odds_u, Some(tip_acc.odds_u));
    assert!(f[0].text.contains("minted"), "text kept for old clients");

    // A copy of it, by a wallet without a handle and with no quote store row: kind `copy`, a
    // short address, the picked team's name.
    let copier = Pubkey::new_unique();
    let mut copy = pending(&pdas, copier, 2, vec![leg(PHI, 0, 699_000)]);
    copy.tipster = Some(tipster);
    copy.copied_from = Some(tip_key);
    copy.copy_fee_u = 100_000;
    store::apply_pending(&state, &pdas, &copy, 4, Some("S2"))
        .await
        .unwrap();
    let copy_acc = ticket_of(&copy);
    store::apply_ticket(
        &state,
        &pdas,
        &pdas.ticket(&copier, 2),
        &copy_acc,
        5,
        &TxFacts {
            sig: Some("F2".into()),
            minted: true,
            ..TxFacts::default()
        },
    )
    .await
    .unwrap();
    let f = feeds(&mut rx);
    assert_eq!(f.len(), 1, "{f:?}");
    let c = &f[0];
    assert_eq!(c.kind, FeedKind::Copy);
    let w = copier.to_string();
    assert_eq!(
        c.who.as_deref(),
        Some(format!("{}…{}", &w[..4], &w[w.len() - 4..]).as_str())
    );
    assert_eq!(c.team.as_deref(), Some("Philadelphia 76ers"));
    assert_eq!(c.amount_u, Some(copy_acc.stake_u));
    assert_eq!(c.odds_u, Some(copy_acc.odds_u));
    let kind: String = sqlx::query_scalar("SELECT kind FROM feed_events WHERE ticket = ?")
        .bind(pdas.ticket(&copier, 2).to_string())
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(kind, "copy");

    // The stream frame, exactly as served.
    let frame =
        desk::api::stream::frame_for(&DeskEvent::Feed(std::sync::Arc::new(c.clone())), None)
            .unwrap();
    let v = serde_json::to_value(&frame).unwrap();
    println!("served feed frame: {v}");
    assert_eq!(v["type"], "feed");
    assert_eq!(v["kind"], "copy");
    assert_eq!(v["who"], c.who.clone().unwrap());
    assert_eq!(v["amount_u"], copy_acc.stake_u);
    assert_eq!(v["team"], "Philadelphia 76ers");
    assert_eq!(v["odds_u"], copy_acc.odds_u);
    assert!(v["text"].is_string());
    assert!(matches!(
        serde_json::from_value::<StreamFrame>(v).unwrap(),
        StreamFrame::Feed { .. }
    ));
    desk.join().await;
}
