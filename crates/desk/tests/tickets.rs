//! Plan 2.C, non-chain part: the indexer's DB writes and bus events from synthetic chain state
//! (idempotent on replay, DB before bus, L2-1 naming), and `GET /tickets/:user`, `GET /ticket/:id`
//! served from a seeded DB with the quoter's live cash-out value (C4, D-039, D-040).

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{TempDb, pascal_stub, start_desk};
use desk::api::types::{
    CashOut, ErrorBody, ErrorStatus, LegStatus, MarketKind, QuoteLeg as ApiQuoteLeg, Side,
    TeamSide, Ticket, TicketStatus, TicketsResponse,
};
use desk::indexer::store::{self, PendingData, TxFacts};
use desk::program::{
    LegStatus as ChainLeg, Pdas, QuoteLeg, TicketAccount, TicketLeg, TicketState as ChainState,
    hex, sha256,
};
use desk::state::{CashOutValuer, TicketState};
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
        quote_expires_at: 1_793_213_000,
        expires_at: 1_793_213_040,
        quote_hash: [nonce as u8; 32],
        is_tip: false,
        tipster: None,
        copied_from: None,
        opened_at: Some(1_793_213_000),
    }
}

fn ticket_of(p: &PendingData, fills: &[u64]) -> TicketAccount {
    let legs: Vec<TicketLeg> = p
        .legs
        .iter()
        .zip(fills)
        .map(|(l, &f)| TicketLeg {
            symbol_hash: l.symbol_hash,
            game_id_hash: l.game_id_hash,
            start_ts: l.start_ts,
            side: l.side,
            price_u: l.price_u,
            fill_u: if f == 0 { l.price_u } else { f },
            m1_ppm: l.m1_ppm,
            hedged: f != 0,
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
        copy_fee_u: 0,
        state: ChainState::Open,
        tipster: None,
        copied_from: None,
        quote_hash: p.quote_hash,
        minted_at: 1_793_213_020,
        resolved_at: 0,
        paid_u: 0,
    }
}

/// Inserts the quote store rows 2.A would write for `p` (L2-1).
async fn store_quote(state: &AppState, p: &PendingData, names: &[(&str, &str, TeamSide)]) {
    let legs: Vec<ApiQuoteLeg> = p
        .legs
        .iter()
        .zip(names)
        .map(|(l, (symbol, game, pick))| ApiQuoteLeg {
            symbol: (*symbol).into(),
            game_id: (*game).into(),
            side: if l.side == 1 { Side::Short } else { Side::Long },
            pick: *pick,
            start_ms: u64::try_from(l.start_ts).unwrap() * 1000,
            price_u: l.price_u,
            p_max_u: l.price_u + 10_000,
            m1_ppm: l.m1_ppm,
            overlaps: true,
            in_play: false,
            kind: None,
            line: None,
            label: None,
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

/// Every non-heartbeat event already on the bus.
fn drain(rx: &mut broadcast::Receiver<DeskEvent>) -> Vec<DeskEvent> {
    let mut out = vec![];
    while let Ok(e) = rx.try_recv() {
        if !matches!(
            e,
            DeskEvent::Heartbeat { .. } | DeskEvent::CatalogueRefreshed { .. }
        ) {
            out.push(e);
        }
    }
    out
}

fn kinds(events: &[DeskEvent]) -> Vec<&'static str> {
    events.iter().map(DeskEvent::kind).collect()
}

async fn desk() -> (Running, TempDb) {
    let pascal = pascal_stub().await;
    let db = TempDb::new("tickets");
    let desk = start_desk(&pascal, &db).await;
    // The catalogue's first pass fills `markets` (names, teams).
    for _ in 0..100 {
        if desk.state.catalogue().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(desk.state.catalogue().is_some(), "catalogue loaded");
    (desk, db)
}

async fn get<T: serde::de::DeserializeOwned>(desk: &Running, path: &str) -> (u16, T) {
    let r = reqwest::get(format!("http://{}{path}", desk.addr))
        .await
        .unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap())
}

struct Fixed(u64);

impl CashOutValuer for Fixed {
    fn cash_out(&self, t: &TicketState) -> CashOut {
        assert_eq!(t.legs.len(), 2);
        assert!(t.legs.iter().all(|l| l.p_fill_u.is_some()));
        CashOut {
            available: true,
            c_u: Some(self.0 + t.payout_u % 7),
            reason: None,
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn indexes_a_ticket_life_once_and_serves_it() {
    let (desk, _db) = desk().await;
    let state = desk.state.clone();
    let pdas = Pdas::new(state.config.program_id);
    let mut rx = state.subscribe();
    let user = Pubkey::new_unique();

    // open_pending, applied twice (event, then resync): one row, one PendingOpened.
    let p = pending(
        &pdas,
        user,
        7,
        vec![leg(PHI, 0, 699_000), leg(ORL, 1, 629_000)],
    );
    store_quote(
        &state,
        &p,
        &[
            (PHI.0, PHI.1, TeamSide::Away),
            (ORL.0, ORL.1, TeamSide::Home),
        ],
    )
    .await;
    store::apply_pending(&state, &pdas, &p, 10, Some("SIG_OPEN"))
        .await
        .unwrap();
    store::apply_pending(&state, &pdas, &p, 11, None)
        .await
        .unwrap();
    let ev = drain(&mut rx);
    assert_eq!(kinds(&ev), ["pending_opened", "ticket_changed"]);
    let DeskEvent::PendingOpened(po) = &ev[0] else {
        unreachable!()
    };
    assert_eq!(po.ticket, pdas.ticket(&user, 7).to_string());
    assert_eq!(po.legs[0].symbol, PHI.0);
    assert_eq!(po.legs[1].game_id, ORL.1);
    assert_eq!(po.legs[1].side, Side::Short);
    assert_eq!(po.legs[0].start_ms, 1_793_228_400_000);
    assert_eq!(po.tx, "SIG_OPEN");

    let id = pdas.ticket(&user, 7).to_string();
    let (code, t): (u16, Ticket) = get(&desk, &format!("/ticket/{id}")).await;
    assert_eq!(code, 200);
    assert_eq!(t.state, TicketStatus::Pending);
    assert_eq!(t.payout_u, 14_000_000, "the quote store's payout");
    assert_eq!(t.odds_fair_u, 3_500_000);
    assert!(!t.cash_out.available);

    // finalize: the ticket account, applied as its tx and again by a resync.
    let key = pdas.ticket(&user, 7);
    let mut acc = ticket_of(&p, &[700_000, 0]);
    let mint = TxFacts {
        sig: Some("SIG_FIN".into()),
        minted: true,
        ..TxFacts::default()
    };
    store::apply_ticket(&state, &pdas, &key, &acc, 20, &mint)
        .await
        .unwrap();
    store::apply_ticket(&state, &pdas, &key, &acc, 21, &TxFacts::default())
        .await
        .unwrap();
    let ev = drain(&mut rx);
    assert_eq!(kinds(&ev), ["ticket_minted", "feed", "ticket_changed"]);

    // Live cash-out from the registered valuer: ours, unless the quoter (2.A) registered first.
    let ours = state.cash_out.set(Arc::new(Fixed(4_000_000))).is_ok();
    let (_, t): (u16, Ticket) = get(&desk, &format!("/ticket/{id}")).await;
    assert_eq!(t.state, TicketStatus::Open);
    assert_eq!(t.stake_u, acc.stake_u);
    assert_eq!(t.payout_u, acc.payout_u);
    assert_eq!(t.odds_offered_u, acc.odds_u);
    assert_eq!(t.minted_at, Some(1_793_213_020));
    assert_eq!(t.tx.open_pending.as_deref(), Some("SIG_OPEN"));
    assert_eq!(t.tx.finalize.as_deref(), Some("SIG_FIN"));
    assert_eq!(t.legs[0].away, "Philadelphia 76ers");
    assert_eq!(t.legs[1].home, "Toronto Raptors");
    assert_eq!(t.legs[1].pick, TeamSide::Home);
    assert_eq!(t.legs[0].p_fill_u, Some(700_000));
    assert!(t.legs[0].hedged);
    assert_eq!(t.legs[1].p_fill_u, Some(629_000));
    assert!(!t.legs[1].hedged);
    if ours {
        assert!(t.cash_out.available);
        assert_eq!(t.cash_out.c_u, Some(4_000_000 + acc.payout_u % 7));
    } else {
        assert_ne!(t.cash_out.reason.as_deref(), Some("quoter not running"));
        assert_eq!(t.cash_out.available, t.cash_out.c_u.is_some());
    }

    // A leg resolves (twice), then the ticket settles (twice).
    acc.legs[0].status = ChainLeg::Won;
    acc.legs[0].resolution_ppm = 1_000_000;
    let res = TxFacts {
        sig: Some("SIG_RES0".into()),
        resolved: vec![0],
        ..TxFacts::default()
    };
    store::apply_ticket(&state, &pdas, &key, &acc, 30, &res)
        .await
        .unwrap();
    store::apply_ticket(&state, &pdas, &key, &acc, 31, &res)
        .await
        .unwrap();
    let ev = drain(&mut rx);
    assert_eq!(kinds(&ev), ["leg_resolved", "feed", "ticket_changed"]);
    match &ev[0] {
        DeskEvent::LegResolved {
            leg_idx,
            symbol,
            resolution_u,
            status,
            tx,
            ..
        } => {
            assert_eq!((*leg_idx, symbol.as_str()), (0, PHI.0));
            assert_eq!(*resolution_u, 1_000_000);
            assert_eq!(*status, LegStatus::Won);
            assert_eq!(tx, "SIG_RES0");
        }
        e => panic!("{e:?}"),
    }
    acc.legs[1].status = ChainLeg::Partial;
    acc.legs[1].resolution_ppm = 500_000;
    acc.state = ChainState::Settled;
    acc.paid_u = acc.payout_u / 2;
    acc.resolved_at = 1_793_250_000;
    // The resync sees resolve and settle at once (no tx facts): both transitions, no signatures.
    store::apply_ticket(&state, &pdas, &key, &acc, 40, &TxFacts::default())
        .await
        .unwrap();
    // Then the settle tx arrives: it only backfills the signature.
    let settle = TxFacts {
        sig: Some("SIG_SET".into()),
        settled: true,
        ..TxFacts::default()
    };
    store::apply_ticket(&state, &pdas, &key, &acc, 41, &settle)
        .await
        .unwrap();
    let ev = drain(&mut rx);
    assert_eq!(
        kinds(&ev),
        [
            "leg_resolved",
            "ticket_settled",
            "feed",
            "feed",
            "ticket_changed"
        ]
    );
    let (_, t): (u16, Ticket) = get(&desk, &format!("/ticket/{id}")).await;
    assert_eq!(t.state, TicketStatus::Settled);
    assert_eq!(t.paid_u, Some(acc.payout_u / 2));
    assert_eq!(t.settled_at, Some(1_793_250_000));
    assert_eq!(t.tx.settle.as_deref(), Some("SIG_SET"));
    assert_eq!(t.legs[1].status, LegStatus::Half);
    assert_eq!(t.legs[1].resolution_u, Some(500_000));
    assert_eq!(t.cash_out.reason.as_deref(), Some("ticket settled"));
    let feeds: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM feed_events WHERE ticket = ?")
        .bind(&id)
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(feeds, 4, "mint, two resolves, settle");

    // A second pending of the same user is refunded (event, then resync): once.
    let p2 = pending(&pdas, user, 8, vec![leg(PHI, 0, 699_000)]);
    store_quote(&state, &p2, &[(PHI.0, PHI.1, TeamSide::Away)]).await;
    store::apply_pending(&state, &pdas, &p2, 50, Some("SIG_OPEN2"))
        .await
        .unwrap();
    store::apply_refund(&state, &p2.pending, 60, Some("SIG_REF"))
        .await
        .unwrap();
    store::apply_refund(&state, &p2.pending, 61, None)
        .await
        .unwrap();
    let ev = drain(&mut rx);
    assert_eq!(
        kinds(&ev),
        [
            "pending_opened",
            "ticket_changed",
            "pending_refunded",
            "ticket_changed"
        ]
    );

    // /tickets: newest first, both; an empty wallet is an empty list; junk is 422; 404.
    let (code, list): (u16, TicketsResponse) = get(&desk, &format!("/tickets/{user}")).await;
    assert_eq!(code, 200);
    assert_eq!(list.user, user.to_string());
    assert_eq!(list.tickets.len(), 2);
    assert_eq!(list.tickets[0].nonce, 8);
    assert_eq!(list.tickets[0].state, TicketStatus::Refunded);
    assert_eq!(
        list.tickets[0].tx.refund_pending.as_deref(),
        Some("SIG_REF")
    );
    assert_eq!(list.tickets[1].ticket, id);
    let (_, empty): (u16, TicketsResponse) =
        get(&desk, &format!("/tickets/{}", Pubkey::new_unique())).await;
    assert!(empty.tickets.is_empty());
    let (code, e): (u16, ErrorBody) = get(&desk, "/tickets/not-a-wallet").await;
    assert_eq!((code, e.status), (422, ErrorStatus::Rejected));
    let (code, e): (u16, ErrorBody) =
        get(&desk, &format!("/ticket/{}", Pubkey::new_unique())).await;
    assert_eq!((code, e.status), (404, ErrorStatus::NotFound));

    desk.join().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn names_legs_from_the_catalogue_or_not_at_all() {
    let (desk, _db) = desk().await;
    let state = desk.state.clone();
    let pdas = Pdas::new(state.config.program_id);
    let mut rx = state.subscribe();
    let user = Pubkey::new_unique();

    // No quote row: the catalogue names the legs (L2-1 fallback); a short leg picks the home
    // team of an away-long symbol.
    let p = pending(&pdas, user, 1, vec![leg(ORL, 1, 629_000)]);
    store::apply_pending(&state, &pdas, &p, 1, Some("S1"))
        .await
        .unwrap();
    let ev = drain(&mut rx);
    assert_eq!(kinds(&ev), ["pending_opened", "ticket_changed"]);
    let DeskEvent::PendingOpened(po) = &ev[0] else {
        unreachable!()
    };
    assert_eq!(po.legs[0].symbol, ORL.0);
    let t = desk::crank::tickets::load_ticket(&state, &po.ticket)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.legs[0].pick, TeamSide::Home);
    assert_eq!(t.odds_fair_u, 0, "unknown without the quote store");

    // A leg nobody can name: the row exists (and will be refunded), no PendingOpened.
    let mut unknown = leg(PHI, 0, 699_000);
    unknown.symbol_hash = sha256(b"NBA_26OCT99_XXXYYY_ML.XXX");
    let p = pending(&pdas, user, 2, vec![leg(PHI, 0, 699_000), unknown]);
    store::apply_pending(&state, &pdas, &p, 2, Some("S2"))
        .await
        .unwrap();
    assert_eq!(kinds(&drain(&mut rx)), ["ticket_changed"]);
    let (named, sym): (i64, String) = sqlx::query_as(
        "SELECT t.names_resolved, l.symbol FROM tickets t JOIN ticket_legs l
         ON l.ticket = t.ticket WHERE t.pending = ? AND l.idx = 1",
    )
    .bind(p.pending.to_string())
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(named, 0);
    assert!(sym.starts_with("sha256:"));

    // A ticket the DB never saw as pending (fresh DB): created from the account, announced as
    // minted (the hedge desk must register it), not as opened.
    let p = pending(&pdas, user, 3, vec![leg(PHI, 0, 699_000)]);
    let acc = ticket_of(&p, &[699_000]);
    let key = pdas.ticket(&user, 3);
    store::apply_ticket(&state, &pdas, &key, &acc, 5, &TxFacts::default())
        .await
        .unwrap();
    assert_eq!(
        kinds(&drain(&mut rx)),
        ["ticket_minted", "feed", "ticket_changed"]
    );
    let t = desk::crank::tickets::load_ticket(&state, &key.to_string())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.state, TicketStatus::Open);
    assert_eq!(t.legs[0].symbol, PHI.0);
    assert_eq!(t.stake_u, acc.stake_u);

    desk.join().await;
}

/// 31 Oct: a ticket met on chain with legs nobody could name (a fresh database) is named later
/// from Pascal's resolved list: its symbols, games and picks, its teams (the markets are
/// remembered, not listed), announced again; a ticket with a leg still unknown is left alone.
#[tokio::test]
async fn names_hashed_legs_from_resolved_markets() {
    let (desk, _db) = desk().await;
    let state = desk.state.clone();
    let pdas = Pdas::new(state.config.program_id);
    let mut rx = state.subscribe();
    let user = Pubkey::new_unique();
    // Markets the catalogue does not list, as Pascal's resolved list served them (30 Oct;
    // resolved, so the catalogue's own parser skips them).
    let body = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/resolved/nba-2026-10-30.json"),
    )
    .unwrap();
    let raw: serde_json::Value = serde_json::from_str(&body).unwrap();
    let resolved: Vec<(desk::catalogue::CatalogueMarket, serde_json::Value)> = raw["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            let m: pascal_rs::Market = serde_json::from_value(v.clone()).unwrap();
            assert!(m.resolution.is_some());
            assert!(desk::catalogue::catalogue_market(&m).is_none());
            (desk::catalogue::resolved_market(&m).unwrap(), v.clone())
        })
        .collect();
    assert_eq!(resolved.len(), 3);
    let old = resolved[1].0.clone();
    let start = i64::try_from(old.start_ms / 1000).unwrap();
    let old_leg = (old.symbol.as_str(), old.game_id.as_str(), start);

    // Short on an away-long symbol: the home team.
    let named = pending(&pdas, user, 7, vec![leg(old_leg, 1, 401_000)]);
    let mut unknown = leg(old_leg, 0, 599_000);
    unknown.symbol_hash = sha256(b"NBA_26OCT20_XXXYYY_ML.XXX");
    let left = pending(&pdas, user, 8, vec![leg(old_leg, 0, 599_000), unknown]);
    for (p, slot) in [(&named, 1), (&left, 2)] {
        store::apply_pending(&state, &pdas, p, slot, None)
            .await
            .unwrap();
    }
    drain(&mut rx);
    assert!(store::has_unnamed(&state.db).await.unwrap());

    let out = store::name_unnamed(&state, &resolved).await.unwrap();
    let key = pdas.ticket(&user, 7).to_string();
    assert_eq!(out, std::slice::from_ref(&key));
    assert_eq!(kinds(&drain(&mut rx)), ["ticket_changed"]);
    let t = desk::crank::tickets::load_ticket(&state, &key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.legs[0].symbol, old.symbol);
    assert_eq!(t.legs[0].game_id, old.game_id);
    assert_eq!(t.legs[0].pick, old.long_team.other());
    assert_eq!(
        (t.legs[0].away.as_str(), t.legs[0].home.as_str()),
        (old.away.name.as_str(), old.home.name.as_str())
    );
    let live: i64 = sqlx::query_scalar("SELECT live FROM markets WHERE symbol = ?")
        .bind(&old.symbol)
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(live, 0, "remembered, not listed");
    // The other keeps its hashes (one leg is still unknown) and is asked about again.
    let (n, sym): (i64, String) = sqlx::query_as(
        "SELECT t.names_resolved, l.symbol FROM tickets t JOIN ticket_legs l
         ON l.ticket = t.ticket WHERE t.ticket = ? AND l.idx = 0",
    )
    .bind(pdas.ticket(&user, 8).to_string())
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(n, 0);
    assert!(sym.starts_with("sha256:"));
    assert!(store::has_unnamed(&state.db).await.unwrap());

    desk.join().await;
}

/// 3.C regression: the app draws random 64-bit nonces. Every one must come back from
/// `GET /tickets/:user` exactly, so the client's Ticket PDA `["ticket", user, nonce]` is the
/// ticket's (a saturating i64 conversion served `i64::MAX` for half of them, and the app refused
/// the cash-out: "ticket address does not match its owner and number").
#[tokio::test(flavor = "multi_thread")]
async fn serves_nonces_above_i64_max_exactly() {
    let (desk, _db) = desk().await;
    let state = desk.state.clone();
    let pdas = Pdas::new(state.config.program_id);
    let user = Pubkey::new_unique();
    let nonces = [u64::MAX, 1 << 63, 12_345_678_901_234_567_890, 42];
    for (i, nonce) in nonces.into_iter().enumerate() {
        let mut p = pending(&pdas, user, nonce, vec![leg(PHI, 0, 699_000)]);
        p.quote_hash = [i as u8 + 1; 32];
        store::apply_pending(&state, &pdas, &p, 1, Some("SIG"))
            .await
            .unwrap();
    }
    let (code, resp): (u16, TicketsResponse) = get(&desk, &format!("/tickets/{user}")).await;
    assert_eq!(code, 200);
    assert_eq!(resp.tickets.len(), nonces.len(), "one row per nonce");
    for nonce in nonces {
        let t = resp
            .tickets
            .iter()
            .find(|t| t.nonce == nonce)
            .unwrap_or_else(|| panic!("nonce {nonce} not served exactly"));
        assert_eq!(t.ticket, pdas.ticket(&user, nonce).to_string());
        let (code, one): (u16, Ticket) = get(&desk, &format!("/ticket/{}", t.ticket)).await;
        assert_eq!((code, one.nonce), (200, nonce));
    }

    desk.join().await;
}

/// D-424: a ticket's legs carry the kind, line and label of its signed quote, so a total or a
/// spread is not shown as "to win" once minted; a ticket without a quote row leaves them out.
#[tokio::test(flavor = "multi_thread")]
async fn ticket_legs_carry_kind_line_and_label_from_the_quote() {
    let (desk, _db) = desk().await;
    let state = desk.state.clone();
    let pdas = Pdas::new(state.config.program_id);
    let user = Pubkey::new_unique();
    let p = pending(
        &pdas,
        user,
        77,
        vec![leg(PHI, 0, 699_000), leg(ORL, 0, 520_000)],
    );
    store_quote(
        &state,
        &p,
        &[
            (PHI.0, PHI.1, TeamSide::Away),
            (ORL.0, ORL.1, TeamSide::Away),
        ],
    )
    .await;
    // Give the quote's second leg a total's kind, line and label, as 6.0b's quoter stores it.
    let legs_json: String = sqlx::query_scalar("SELECT legs_json FROM quotes WHERE quote_hash = ?")
        .bind(hex(&p.quote_hash))
        .fetch_one(&state.db)
        .await
        .unwrap();
    let mut legs: Vec<serde_json::Value> = serde_json::from_str(&legs_json).unwrap();
    legs[0]["kind"] = "winner".into();
    legs[0]["label"] = "76ers".into();
    legs[1]["kind"] = "total".into();
    legs[1]["line"] = "237.5".into();
    legs[1]["label"] = "Over 237.5".into();
    sqlx::query("UPDATE quotes SET legs_json = ? WHERE quote_hash = ?")
        .bind(serde_json::to_string(&legs).unwrap())
        .bind(hex(&p.quote_hash))
        .execute(&state.db)
        .await
        .unwrap();
    store::apply_pending(&state, &pdas, &p, 1, Some("SIG"))
        .await
        .unwrap();

    let id = pdas.ticket(&user, 77).to_string();
    let (code, t): (u16, Ticket) = get(&desk, &format!("/ticket/{id}")).await;
    assert_eq!(code, 200);
    assert_eq!(t.legs[0].kind, Some(MarketKind::Winner));
    assert_eq!(t.legs[0].label.as_deref(), Some("76ers"));
    assert_eq!(t.legs[1].kind, Some(MarketKind::Total));
    assert_eq!(t.legs[1].line.as_deref(), Some("237.5"));
    assert_eq!(t.legs[1].label.as_deref(), Some("Over 237.5"));

    // No quote row: the fields are left out rather than guessed.
    let mut bare = pending(&pdas, user, 78, vec![leg(PHI, 0, 699_000)]);
    bare.quote_hash = [99; 32];
    store::apply_pending(&state, &pdas, &bare, 1, Some("SIG2"))
        .await
        .unwrap();
    let (_, b): (u16, Ticket) = get(&desk, &format!("/ticket/{}", pdas.ticket(&user, 78))).await;
    assert_eq!((b.legs[0].kind, b.legs[0].label.as_deref()), (None, None));

    desk.join().await;
}

/// L9: a tip named in the drawer gets the title its wallet sent with the quote, when that
/// wallet opens it as an own tip; another wallet's title for the same quote, a plain bet and a
/// copy get none.
#[tokio::test(flavor = "multi_thread")]
async fn names_an_own_tip_from_its_quote_title() {
    let (desk, _db) = desk().await;
    let state = desk.state.clone();
    let pdas = Pdas::new(state.config.program_id);
    let (user, other) = (Pubkey::new_unique(), Pubkey::new_unique());
    let title = |p: &PendingData, wallet: Pubkey, title: &str| {
        let db = state.db.clone();
        let hash = hex(&p.quote_hash);
        let title = title.to_owned();
        async move {
            sqlx::query(
                "INSERT INTO quote_titles (quote_hash, wallet, title, created_ms) VALUES (?, ?, ?, 0)",
            )
            .bind(hash)
            .bind(wallet.to_string())
            .bind(title)
            .execute(&db)
            .await
            .unwrap();
        }
    };
    let served = |p: &PendingData| {
        let id = pdas.ticket(&p.user, p.nonce).to_string();
        let desk = &desk;
        async move {
            let (code, t): (u16, Ticket) = get(desk, &format!("/ticket/{id}")).await;
            assert_eq!(code, 200);
            t.title
        }
    };

    let mut tip = pending(&pdas, user, 21, vec![leg(PHI, 0, 699_000)]);
    tip.is_tip = true;
    tip.tipster = Some(user);
    title(&tip, other, "Not theirs").await;
    title(&tip, user, "Thursday treble").await;
    store::apply_pending(&state, &pdas, &tip, 1, Some("SIG_TIP"))
        .await
        .unwrap();
    assert_eq!(served(&tip).await.as_deref(), Some("Thursday treble"));

    let plain = pending(&pdas, user, 22, vec![leg(PHI, 0, 699_000)]);
    title(&plain, user, "Private").await;
    store::apply_pending(&state, &pdas, &plain, 1, Some("SIG_PLAIN"))
        .await
        .unwrap();
    assert_eq!(served(&plain).await, None);

    let mut copy = pending(&pdas, user, 23, vec![leg(PHI, 0, 699_000)]);
    copy.tipster = Some(other);
    copy.copied_from = Some(pdas.ticket(&other, 1));
    title(&copy, user, "Copied").await;
    store::apply_pending(&state, &pdas, &copy, 1, Some("SIG_COPY"))
        .await
        .unwrap();
    assert_eq!(served(&copy).await, None);

    desk.join().await;
}
