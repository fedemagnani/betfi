//! `open_pending`, `finalize`, `refund_pending`.

mod common;

use anchor_lang::error::ErrorCode as AnchorError;
use common::*;
use schedina::error::SchedinaError;
use schedina::events::{PendingOpened, PendingRefunded, TicketMinted};
use schedina::{math, LegStatus, TicketState};
use solana_signer::Signer;

fn signer(h: &Harness) -> solana_keypair::Keypair {
    h.quote_signer.insecure_clone()
}

// --- open_pending -----------------------------------------------------------

#[test]
fn open_pending_escrows_the_stake_and_stores_the_quote() {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    let q = quote(legs_at_half(3), 10 * USDC, h.now());
    let floor = q.odds_offered_u * 97 / 100;
    let exp = h.now() + 45;
    let meta = ok(h.open(&user, 7, &q, floor, exp));
    assert_eq!(h.balance(&user.usdc), 90 * USDC);
    assert_eq!(h.balance(&escrow_usdc()), 10 * USDC);
    let p = h.pending(&user.key(), 7);
    assert_eq!((p.user, p.nonce, p.n), (user.key(), 7, 3));
    assert_eq!((p.stake_u, p.copy_fee_u), (10 * USDC, 0));
    assert_eq!(&p.legs[..3], q.legs.as_slice());
    assert_eq!(p.legs[3], Default::default());
    assert_eq!(p.odds_offered_u, 6_859_000);
    assert_eq!(p.min_payout_per_unit_u, floor);
    assert_eq!(
        (p.expires_at, p.quote_expires_at),
        (exp, q.quote_expires_at)
    );
    assert_eq!((p.is_tip, p.tipster, p.copied_from), (false, None, None));
    let ev = cpi_events::<PendingOpened>(&meta);
    assert_eq!(ev.len(), 1);
    assert_eq!(
        (ev[0].pending, ev[0].nonce),
        (pending_pda(&user.key(), 7), 7)
    );
    assert_eq!(ev[0].legs, q.legs);
    assert_eq!(ev[0].quote_hash, p.quote_hash);
}

#[test]
fn quote_expiry_is_checked() {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    let q = quote(legs_at_half(1), 10 * USDC, h.now());
    h.warp(10); // now == quote_expires_at: still valid
    ok(h.open(&user, 1, &q, q.odds_offered_u, h.now() + 45));
    h.warp(1);
    let res = h.open(&user, 2, &q, q.odds_offered_u, h.now() + 45);
    assert_err(&res, SchedinaError::QuoteExpired);
}

#[test]
fn one_leg_per_game() {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    let mut legs = legs_at_half(3);
    legs[2].game_id_hash = legs[0].game_id_hash; // other symbol, same game
    let q = quote(legs, 10 * USDC, h.now());
    let res = h.open(&user, 1, &q, q.odds_offered_u, h.now() + 45);
    assert_err(&res, SchedinaError::DuplicateGame);
}

#[test]
fn leg_count_within_max_legs() {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    let mut p = default_params(h.crank.pubkey(), h.quote_signer.pubkey());
    p.max_legs = 3;
    let ix = h.ix_set_params(&h.authority.pubkey(), p);
    ok(h.send_authority(ix));

    let q = quote(legs_at_half(4), 10 * USDC, h.now());
    let res = h.open(&user, 1, &q, q.odds_offered_u, h.now() + 45);
    assert_err(&res, SchedinaError::TooManyLegs);
    let q = quote(vec![], 10 * USDC, h.now());
    let res = h.open(&user, 1, &q, 1, h.now() + 45);
    assert_err(&res, SchedinaError::TooManyLegs);
    let q = quote(legs_at_half(3), 10 * USDC, h.now());
    ok(h.open(&user, 1, &q, q.odds_offered_u, h.now() + 45));
}

#[test]
fn six_legs_never_fit() {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    let q = quote(legs_at_half(6), 10 * USDC, h.now());
    let res = h.open(&user, 1, &q, q.odds_offered_u, h.now() + 45);
    assert_err(&res, SchedinaError::TooManyLegs);
}

#[test]
fn stake_at_least_min_stake() {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    let q = quote(legs_at_half(1), USDC - 1, h.now());
    let res = h.open(&user, 1, &q, q.odds_offered_u, h.now() + 45);
    assert_err(&res, SchedinaError::StakeTooSmall);
    let q = quote(legs_at_half(1), USDC, h.now());
    ok(h.open(&user, 1, &q, q.odds_offered_u, h.now() + 45));
}

#[test]
fn floor_at_most_offered_odds() {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    let q = quote(legs_at_half(2), 10 * USDC, h.now());
    let res = h.open(&user, 1, &q, q.odds_offered_u + 1, h.now() + 45);
    assert_err(&res, SchedinaError::FloorAboveOffered);
    let res = h.open(&user, 1, &q, 0, h.now() + 45);
    assert_err(&res, SchedinaError::FloorAboveOffered);
    ok(h.open(&user, 1, &q, q.odds_offered_u, h.now() + 45));
}

#[test]
fn expires_at_between_30_and_360_seconds() {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    let q = quote(legs_at_half(1), 10 * USDC, h.now());
    let now = h.now();
    for (nonce, exp, good) in [
        (1, now + 29, false),
        (2, now + 30, true),
        (3, now + 360, true),
        (4, now + 361, false),
    ] {
        let res = h.open(&user, nonce, &q, q.odds_offered_u, exp);
        if good {
            ok(res);
        } else {
            assert_err(&res, SchedinaError::BadExpiry);
        }
    }
}

#[test]
fn invalid_legs_and_payout_cap() {
    let mut h = Harness::ready();
    let user = h.user(10_000 * USDC);
    for bad in [
        leg(0, 2, 500_000, 30_000),
        leg(0, 0, 0, 30_000),
        leg(0, 0, 1_000_001, 30_000),
        leg(0, 0, 500_000, 1_000_000),
    ] {
        let mut q = quote(legs_at_half(1), 10 * USDC, h.now());
        q.legs[0] = bad;
        let res = h.open(&user, 1, &q, 1, h.now() + 45);
        assert_err(&res, SchedinaError::InvalidLeg);
    }
    // 1,000 USDC at 5 legs of 0.5: about 24,761 USDC, above max_payout (10,000).
    let q = quote(legs_at_half(5), 1_000 * USDC, h.now());
    let res = h.open(&user, 1, &q, q.odds_offered_u, h.now() + 45);
    assert_err(&res, SchedinaError::PayoutTooLarge);
}

#[test]
fn tips_and_copies() {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    let s = signer(&h);
    let q = quote(legs_at_half(2), 10 * USDC, h.now());
    let exp = h.now() + 45;
    ok(h.open_with(&user, 1, &q, q.odds_offered_u, exp, true, &s));
    let p = h.pending(&user.key(), 1);
    assert_eq!((p.is_tip, p.tipster), (true, Some(user.key())));

    // A tip with a leg in play opens like any schedina (D-660).
    let mut legs = legs_at_half(2);
    legs[1].start_ts = h.now() - 600;
    let q = quote(legs, 10 * USDC, h.now());
    ok(h.open_with(&user, 2, &q, q.odds_offered_u, exp, true, &s));
    assert!(h.pending(&user.key(), 2).is_tip);

    // copied_from was reserved for step 2.F (`CopiesNotEnabled`); since 2.F a copy opens
    // and escrows the 2% copy fee; `finalize` checks the original (tests/tipster.rs).
    let q = quote(legs_at_half(1), 10 * USDC, h.now());
    let msg = borsh(&q);
    let sig = sign(&s, &msg);
    let ixs = [
        ed25519_ix(&msg, &sig, &s.pubkey()),
        h.ix_open_pending(
            &user,
            3,
            q.odds_offered_u,
            exp,
            false,
            Some(ticket_pda(&user.key(), 1)),
        ),
    ];
    ok(h.send_ixs(&ixs, &[&user.kp]));
    assert_eq!(h.pending(&user.key(), 3).copy_fee_u, 200_000);
}

#[test]
fn same_nonce_twice_fails() {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    h.open_default(&user, 1, legs_at_half(1), 10 * USDC);
    let q = quote(legs_at_half(1), 10 * USDC, h.now());
    assert!(h
        .open(&user, 1, &q, q.odds_offered_u, h.now() + 45)
        .is_err());
}

// --- finalize ---------------------------------------------------------------

#[test]
fn mint_at_the_floor_passes_and_one_micro_below_fails() {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    let stake = 10 * USDC;
    let legs = legs_at_half(3);
    let fills = vec![505_000u64, 505_000, 505_000];
    let p = math::payout(stake, fills.iter().map(|&f| (50_000, f))).unwrap();
    let achieved = math::odds(p, stake).unwrap();
    assert!(achieved < 6_859_000);

    let q = quote(legs.clone(), stake, h.now());
    let exp = h.now() + 45;
    ok(h.open(&user, 1, &q, achieved, exp));
    ok(h.open(&user, 2, &q, achieved + 1, exp));

    ok(h.finalize(&user, 1, fills.clone(), 1_000_000));
    let t = h.ticket(&ticket_pda(&user.key(), 1));
    assert_eq!((t.payout_u, t.odds_u), (p, achieved));
    let res = h.finalize(&user, 2, fills, 1_000_000);
    assert_err(&res, SchedinaError::BelowFloor);
}

#[test]
fn partial_fill_refunds_the_rest() {
    // Vector "mixed tiers, odd prices, r = 0.6, 5 USDC" from payout_vectors.json.
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    let legs = vec![
        leg(0, 0, 695_000, 50_000),
        leg(1, 1, 412_345, 50_000),
        leg(2, 0, 233_001, 30_000),
    ];
    let q = quote(legs, 5 * USDC, h.now());
    ok(h.open(&user, 1, &q, q.odds_offered_u * 97 / 100, h.now() + 45));
    let free0 = h.vault().free;
    let meta = ok(h.finalize(&user, 1, vec![695_000, 412_345, 233_001], 600_000));
    let t = h.ticket(&ticket_pda(&user.key(), 1));
    assert_eq!(
        (t.stake_u, t.refund_u, t.payout_u),
        (3_000_000, 2_000_000, 39_331_166)
    );
    assert_eq!(t.odds_u, 13_110_388);
    assert_eq!(
        h.balance(&user.usdc),
        95 * USDC + 2_000_000,
        "0.4 S refunded"
    );
    let v = h.vault();
    assert_eq!(v.locked, 39_331_166);
    assert_eq!(v.free, free0 + 3_000_000 - 39_331_166);
    assert_eq!(h.balance(&escrow_usdc()), 0);
    let ev = cpi_events::<TicketMinted>(&meta);
    assert_eq!(
        (ev[0].refund_u, ev[0].payout_u, ev[0].r_ppm),
        (2_000_000, 39_331_166, 600_000)
    );
}

#[test]
fn r_point_six_refunds_point_four_s() {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    let q = quote(legs_at_half(2), 10 * USDC, h.now());
    ok(h.open(&user, 1, &q, q.odds_offered_u * 97 / 100, h.now() + 45));
    ok(h.finalize(&user, 1, vec![500_000, 500_000], 600_000));
    assert_eq!(h.balance(&user.usdc), 90 * USDC + 4 * USDC);
    let t = h.ticket(&ticket_pda(&user.key(), 1));
    assert_eq!(
        (t.stake_u, t.refund_u, t.r_ppm),
        (6 * USDC, 4 * USDC, 600_000)
    );
}

#[test]
fn unhedged_legs_take_their_quote_price() {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    let legs = vec![
        leg(0, 0, 400_000, 30_000),
        leg(1, 1, 600_000, 30_000),
        leg(2, 0, 450_000, 50_000),
    ];
    let q = quote(legs, 10 * USDC, h.now());
    ok(h.open(&user, 1, &q, q.odds_offered_u * 97 / 100, h.now() + 45));
    let pending_rent = h
        .svm
        .get_account(&pending_pda(&user.key(), 1))
        .unwrap()
        .lamports;
    let sol0 = h.svm.get_balance(&user.key()).unwrap();
    ok(h.finalize(&user, 1, vec![0, 590_000, 0], 1_000_000));
    let t = h.ticket(&ticket_pda(&user.key(), 1));
    let used: Vec<_> = t.legs[..3].iter().map(|l| (l.fill_u, l.hedged)).collect();
    assert_eq!(
        used,
        vec![(400_000, false), (590_000, true), (450_000, false)]
    );
    let want = math::payout(
        10 * USDC,
        [(30_000, 400_000), (30_000, 590_000), (50_000, 450_000)],
    )
    .unwrap();
    assert_eq!(t.payout_u, want);
    assert_eq!(
        t.fee_u,
        10 * USDC - math::settle(10 * USDC, [970_000, 970_000, 950_000]).unwrap()
    );
    assert_eq!(t.state, TicketState::Open);
    assert!(t.legs.iter().all(|l| l.status == LegStatus::Open));
    assert_eq!(t.minted_at, h.now());
    // The Pending is closed and its rent returned to the user.
    assert!(!h.exists(&pending_pda(&user.key(), 1)));
    assert_eq!(h.svm.get_balance(&user.key()).unwrap(), sol0 + pending_rent);
}

#[test]
fn finalize_after_expiry_fails() {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    h.open_default(&user, 1, legs_at_half(1), 10 * USDC);
    let exp = h.pending(&user.key(), 1).expires_at;
    h.set_time(exp - 1);
    h.open_default(&user, 2, legs_at_half(1), 10 * USDC);
    ok(h.finalize(&user, 1, vec![0], 1_000_000)); // one second before expiry
    h.set_time(h.pending(&user.key(), 2).expires_at);
    let res = h.finalize(&user, 2, vec![0], 1_000_000);
    assert_err(&res, SchedinaError::PendingExpired);
}

#[test]
fn finalize_checks_crank_and_arguments() {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    h.open_default(&user, 1, legs_at_half(2), 10 * USDC);

    let ix = h.ix_finalize_by(
        &user.key(),
        &user.key(),
        1,
        &user.usdc,
        vec![0, 0],
        1_000_000,
    );
    assert_err(&h.send(ix, &[&user.kp]), SchedinaError::NotCrank);
    for (fills, r, e) in [
        (vec![0], 1_000_000, SchedinaError::FillCount),
        (vec![0, 0, 0], 1_000_000, SchedinaError::FillCount),
        (vec![0, 0], 0, SchedinaError::BadFillRatio),
        (vec![0, 0], 1_000_001, SchedinaError::BadFillRatio),
        (vec![0, 1_000_001], 1_000_000, SchedinaError::BadFillPrice),
    ] {
        assert_err(&h.finalize(&user, 1, fills, r), e);
    }
    // Refund to an account the user does not own.
    let other = h.user(0);
    let ix = h.ix_finalize(&user.key(), 1, &other.usdc, vec![0, 0], 1_000_000);
    assert_anchor_err(&h.send_crank(ix), AnchorError::ConstraintTokenOwner);
    // Pending rent to someone else.
    let mut ix = h.ix_finalize(&user.key(), 1, &user.usdc, vec![0, 0], 1_000_000);
    ix.accounts[5].pubkey = other.key();
    assert_anchor_err(&h.send_crank(ix), AnchorError::ConstraintAddress);
    ok(h.finalize(&user, 1, vec![0, 0], 1_000_000));
    // The Pending is gone: no second ticket.
    assert!(h.finalize(&user, 1, vec![0, 0], 1_000_000).is_err());
}

#[test]
fn finalize_needs_vault_cover() {
    let mut h = Harness::initialized();
    let funder = h.user(10 * USDC);
    ok(h.fund_vault(&funder, 10 * USDC));
    let user = h.user(100 * USDC);
    // 3 legs at 0.5 on 10 USDC pays 68.59: free (10) + stake (10) cannot cover it.
    h.open_default(&user, 1, legs_at_half(3), 10 * USDC);
    let res = h.finalize(&user, 1, vec![0, 0, 0], 1_000_000);
    assert_err(&res, SchedinaError::VaultShort);
    // 1 leg pays 19 <= 20: covered.
    h.open_default(&user, 2, legs_at_half(1), 10 * USDC);
    ok(h.finalize(&user, 2, vec![0], 1_000_000));
    assert_eq!(h.vault().free, 20 * USDC - 19_000_000);
}

#[test]
fn finalize_respects_max_payout_after_price_improvement() {
    let mut h = Harness::ready();
    let user = h.user(1_000 * USDC);
    let q = quote(legs_at_half(3), 1_000 * USDC, h.now()); // quoted 6,859 USDC
    ok(h.open(&user, 1, &q, q.odds_offered_u, h.now() + 45));
    let res = h.finalize(&user, 1, vec![300_000, 300_000, 300_000], 1_000_000);
    assert_err(&res, SchedinaError::PayoutTooLarge);
}

// --- refund_pending -----------------------------------------------------------

#[test]
fn refund_opens_15_seconds_after_expiry() {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    h.open_default(&user, 1, legs_at_half(2), 10 * USDC);
    let exp = h.pending(&user.key(), 1).expires_at;
    let stranger = h.wallet();
    let ix = h.ix_refund_pending(&stranger.pubkey(), &user.key(), 1, &user.usdc);

    for t in [h.now(), exp, exp + 14] {
        h.set_time(t);
        assert_err(
            &h.send(ix.clone(), &[&stranger]),
            SchedinaError::RefundTooEarly,
        );
    }
    // Finalize lost its window too.
    assert_err(
        &h.finalize(&user, 1, vec![0, 0], 1_000_000),
        SchedinaError::PendingExpired,
    );

    h.set_time(exp + 15);
    let other = h.user(0);
    let bad = h.ix_refund_pending(&stranger.pubkey(), &user.key(), 1, &other.usdc);
    assert_anchor_err(
        &h.send(bad, &[&stranger]),
        AnchorError::ConstraintTokenOwner,
    );
    let meta = ok(h.send(ix.clone(), &[&stranger]));
    assert_eq!(h.balance(&user.usdc), 100 * USDC);
    assert_eq!(h.balance(&escrow_usdc()), 0);
    assert!(!h.exists(&pending_pda(&user.key(), 1)));
    let ev = cpi_events::<PendingRefunded>(&meta);
    assert_eq!((ev[0].user, ev[0].amount_u), (user.key(), 10 * USDC));
    h.svm.expire_blockhash();
    assert!(h.send(ix, &[&stranger]).is_err(), "second refund");
}

#[test]
fn finalized_pending_cannot_be_refunded() {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    h.mint(&user, 1, legs_at_half(1), 10 * USDC);
    h.warp(120);
    let ix = h.ix_refund_pending(&user.key(), &user.key(), 1, &user.usdc);
    assert_anchor_err(&h.send(ix, &[&user.kp]), AnchorError::AccountNotInitialized);
}
