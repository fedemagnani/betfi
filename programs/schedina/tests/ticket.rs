//! `resolve_leg`, `settle`, `cash_out`.

mod common;

use anchor_lang::error::ErrorCode as AnchorError;
use common::*;
use schedina::error::SchedinaError;
use schedina::events::{LegResolved, TicketCashedOut, TicketSettled};
use schedina::{LegStatus, TicketState};
use solana_keypair::Keypair;
use solana_signer::Signer;

/// Ready harness, a user, and a 3-leg ticket at 0.50 (overlap tier) on 10 USDC:
/// payout 68.59 USDC.
fn three_legs() -> (Harness, User, anchor_lang::prelude::Pubkey) {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    let t = h.mint(&user, 1, legs_at_half(3), 10 * USDC);
    assert_eq!(h.ticket(&t).payout_u, 68_590_000);
    (h, user, t)
}

// --- resolve_leg / settle -------------------------------------------------------

#[test]
fn settle_with_a_half_leg_pays_half() {
    let (mut h, user, t) = three_legs();
    ok(h.resolve(&t, 0, 1_000_000));
    let meta = ok(h.resolve(&t, 1, 500_000));
    let ev = cpi_events::<LegResolved>(&meta);
    assert_eq!(
        (ev[0].leg_index, ev[0].r_ppm, ev[0].rtilde_ppm),
        (1, 500_000, 500_000)
    );
    ok(h.resolve(&t, 2, 1_000_000));
    let legs = h.ticket(&t).legs;
    assert_eq!(
        [legs[0].status, legs[1].status, legs[2].status],
        [LegStatus::Won, LegStatus::Partial, LegStatus::Won]
    );
    let free0 = h.vault().free;
    let caller = h.wallet(); // settle is permissionless
    let meta = ok(h.settle(&caller, &t, &user.usdc));
    assert_eq!(h.balance(&user.usdc), 90 * USDC + 34_295_000);
    let tk = h.ticket(&t);
    assert_eq!(
        (tk.state, tk.paid_u, tk.resolved_at),
        (TicketState::Settled, 34_295_000, h.now())
    );
    let v = h.vault();
    assert_eq!((v.locked, v.free), (0, free0 + 34_295_000));
    let ev = cpi_events::<TicketSettled>(&meta);
    assert_eq!((ev[0].payout_u, ev[0].paid_u), (68_590_000, 34_295_000));
    // Once only.
    h.svm.expire_blockhash();
    assert_err(
        &h.settle(&caller, &t, &user.usdc),
        SchedinaError::TicketNotOpen,
    );
}

#[test]
fn settle_vectors_from_the_fixture() {
    // payout_vectors.json "settle": two half-voids, one lost, all won.
    for (rs, paid) in [
        ([500_000, 500_000, 1_000_000], 17_147_500u64),
        ([1_000_000, 0, 1_000_000], 0),
        ([1_000_000, 1_000_000, 1_000_000], 68_590_000),
    ] {
        let (mut h, user, t) = three_legs();
        for (i, r) in rs.into_iter().enumerate() {
            ok(h.resolve(&t, i as u8, r));
        }
        let caller = h.wallet();
        ok(h.settle(&caller, &t, &user.usdc));
        assert_eq!(h.ticket(&t).paid_u, paid);
        assert_eq!(h.balance(&user.usdc), 90 * USDC + paid);
    }
}

#[test]
fn short_legs_win_on_zero() {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    let legs = vec![leg(0, 1, 500_000, 30_000), leg(1, 0, 500_000, 30_000)];
    let t = h.mint(&user, 1, legs, 10 * USDC);
    let p = h.ticket(&t).payout_u;
    ok(h.resolve(&t, 0, 0)); // short wins
    ok(h.resolve(&t, 1, 1_000_000)); // long wins
    let legs = h.ticket(&t).legs;
    assert_eq!(
        (legs[0].status, legs[1].status),
        (LegStatus::Won, LegStatus::Won)
    );
    let caller = h.wallet();
    ok(h.settle(&caller, &t, &user.usdc));
    assert_eq!(h.ticket(&t).paid_u, p);
}

#[test]
fn a_leg_resolves_once() {
    let (mut h, _user, t) = three_legs();
    ok(h.resolve(&t, 1, 1_000_000));
    assert_err(&h.resolve(&t, 1, 0), SchedinaError::LegAlreadyResolved);
    h.svm.expire_blockhash();
    assert_err(
        &h.resolve(&t, 1, 1_000_000),
        SchedinaError::LegAlreadyResolved,
    );
    assert_eq!(h.ticket(&t).legs[1].resolution_ppm, 1_000_000);
}

#[test]
fn resolve_checks() {
    let (mut h, user, t) = three_legs();
    assert_err(&h.resolve(&t, 3, 1_000_000), SchedinaError::BadLegIndex);
    assert_err(&h.resolve(&t, 0, 1_000_001), SchedinaError::BadResolution);
    let ix = h.ix_resolve_leg(&user.key(), &t, 0, 1_000_000);
    assert_err(&h.send(ix, &[&user.kp]), SchedinaError::NotResolver);
    // The authority may resolve (spec 10: market removed, 0.5).
    let ix = h.ix_resolve_leg(&h.authority.pubkey(), &t, 0, 500_000);
    ok(h.send_authority(ix));
    // Not settleable until every leg is resolved.
    let caller = h.wallet();
    assert_err(
        &h.settle(&caller, &t, &user.usdc),
        SchedinaError::LegsUnresolved,
    );
    ok(h.resolve(&t, 1, 1_000_000));
    ok(h.resolve(&t, 2, 1_000_000));
    // Pays only the ticket owner.
    let other = h.user(0);
    let res = h.settle(&caller, &t, &other.usdc);
    assert_anchor_err(&res, AnchorError::ConstraintTokenOwner);
    ok(h.settle(&caller, &t, &user.usdc));
    // A closed ticket takes no more resolutions.
    assert_err(&h.resolve(&t, 0, 1_000_000), SchedinaError::TicketNotOpen);
}

// --- cash_out -------------------------------------------------------------------

#[test]
fn cash_out_pays_c_and_frees_the_rest() {
    {
        let (mut h, user, t) = three_legs();
        ok(h.resolve(&t, 0, 1_000_000));
        let free0 = h.vault().free;
        let msg = cashout_msg(&t, 20 * USDC, h.now() - 30);
        let s = h.quote_signer.insecure_clone();
        let meta = ok(h.cash_out_with(&user, &msg, &s));
        assert_eq!(h.balance(&user.usdc), 110 * USDC);
        let tk = h.ticket(&t);
        assert_eq!((tk.state, tk.paid_u), (TicketState::CashedOut, 20 * USDC));
        let v = h.vault();
        assert_eq!((v.locked, v.free), (0, free0 + 68_590_000 - 20 * USDC));
        let ev = cpi_events::<TicketCashedOut>(&meta);
        assert_eq!((ev[0].c_u, ev[0].ts), (20 * USDC, msg.ts));
        // The same signed quote cannot be replayed.
        h.svm.expire_blockhash();
        let res = h.cash_out_with(&user, &msg, &s);
        assert_err(&res, SchedinaError::TicketNotOpen);
        // Nor resolved or settled afterwards.
        assert_err(&h.resolve(&t, 1, 1_000_000), SchedinaError::TicketNotOpen);
        let c = h.wallet();
        assert_err(&h.settle(&c, &t, &user.usdc), SchedinaError::TicketNotOpen);
    }
}

#[test]
fn cash_out_with_a_forged_signature_fails() {
    let (mut h, user, t) = three_legs();
    let msg = cashout_msg(&t, 60 * USDC, h.now());
    let res = h.cash_out_with(&user, &msg, &Keypair::new());
    assert_err(&res, SchedinaError::WrongQuoteSigner);

    // Signed by the desk for 1 USDC, submitted for 60.
    let bytes = borsh(&cashout_msg(&t, USDC, h.now()));
    let sig = sign(&h.quote_signer, &bytes);
    let ixs = [
        ed25519_ix(&bytes, &sig, &h.quote_signer.pubkey()),
        h.ix_cash_out(&user.key(), &t, &user.usdc, msg.clone()),
    ];
    assert_err(&h.send_ixs(&ixs, &[&user.kp]), SchedinaError::QuoteMismatch);

    // Corrupted desk signature: the precompile rejects the transaction.
    let bytes = borsh(&msg);
    let mut sig = sign(&h.quote_signer, &bytes);
    sig[63] ^= 0x10;
    let ixs = [
        ed25519_ix(&bytes, &sig, &h.quote_signer.pubkey()),
        h.ix_cash_out(&user.key(), &t, &user.usdc, msg.clone()),
    ];
    assert_precompile_err(&h.send_ixs(&ixs, &[&user.kp]));

    // No verify instruction at all.
    let ix = h.ix_cash_out(&user.key(), &t, &user.usdc, msg);
    assert_err(&h.send(ix, &[&user.kp]), SchedinaError::SignatureMissing);
    assert_eq!(h.ticket(&t).state, TicketState::Open);
}

#[test]
fn cash_out_with_a_stale_ts_fails() {
    let (mut h, user, t) = three_legs();
    let s = h.quote_signer.insecure_clone();
    let stale = cashout_msg(&t, 10 * USDC, h.now() - 31);
    let res = h.cash_out_with(&user, &stale, &s);
    assert_err(&res, SchedinaError::CashOutStale);
    let future = cashout_msg(&t, 10 * USDC, h.now() + 31);
    let res = h.cash_out_with(&user, &future, &s);
    assert_err(&res, SchedinaError::CashOutStale);
    // The 30 s edge passes.
    let edge = cashout_msg(&t, 10 * USDC, h.now() - 30);
    ok(h.cash_out_with(&user, &edge, &s));
}

#[test]
fn cash_out_above_payout_fails() {
    let (mut h, user, t) = three_legs();
    assert_err(
        &h.cash_out(&user, &t, 68_590_001),
        SchedinaError::CashOutAbovePayout,
    );
    ok(h.cash_out(&user, &t, 68_590_000));
    assert_eq!(h.vault().locked, 0);
}

#[test]
fn cash_out_with_a_lost_leg_fails() {
    let (mut h, user, t) = three_legs();
    ok(h.resolve(&t, 0, 500_000)); // half-void: cash-out still allowed
    ok(h.resolve(&t, 2, 0)); // lost
    assert_err(&h.cash_out(&user, &t, USDC), SchedinaError::LegLost);
}

#[test]
fn cash_out_is_bound_to_ticket_and_owner() {
    let (mut h, user, t) = three_legs();
    let t2 = h.mint(&user, 2, legs_at_half(1), 10 * USDC);
    let s = h.quote_signer.insecure_clone();
    // A quote signed for ticket 1 used on ticket 2.
    let msg = cashout_msg(&t, USDC, h.now());
    let bytes = borsh(&msg);
    let sig = sign(&s, &bytes);
    let ixs = [
        ed25519_ix(&bytes, &sig, &s.pubkey()),
        h.ix_cash_out(&user.key(), &t2, &user.usdc, msg.clone()),
    ];
    assert_err(&h.send_ixs(&ixs, &[&user.kp]), SchedinaError::WrongTicket);

    // Someone else holding the desk quote cannot take the money.
    let thief = h.user(0);
    let ixs = [
        ed25519_ix(&bytes, &sig, &s.pubkey()),
        h.ix_cash_out(&thief.key(), &t, &thief.usdc, msg.clone()),
    ];
    assert_anchor_err(
        &h.send_ixs(&ixs, &[&thief.kp]),
        AnchorError::ConstraintAddress,
    );
    // Nor redirect it to their own token account.
    let ixs = [
        ed25519_ix(&bytes, &sig, &s.pubkey()),
        h.ix_cash_out(&user.key(), &t, &thief.usdc, msg),
    ];
    assert_anchor_err(
        &h.send_ixs(&ixs, &[&user.kp]),
        AnchorError::ConstraintTokenOwner,
    );
}

#[test]
fn many_tickets_keep_the_vault_whole() {
    let mut h = Harness::ready();
    let users: Vec<User> = (0..3).map(|_| h.user(100 * USDC)).collect();
    let mut tickets = vec![];
    for (i, u) in users.iter().enumerate() {
        for k in 0..3u64 {
            let n = (k as u8 % 3) + 1;
            tickets.push((i, h.mint(u, k, legs_at_half(n), 5 * USDC + k)));
        }
    }
    // Settle some, cash out some, leave some open; invariants run after each send.
    for (j, (i, t)) in tickets.iter().enumerate() {
        let n = h.ticket(t).n;
        match j % 3 {
            0 => {
                for leg in 0..n {
                    ok(h.resolve(t, leg, if leg == 0 { 500_000 } else { 1_000_000 }));
                }
                let c = h.wallet();
                ok(h.settle(&c, t, &users[*i].usdc));
            }
            1 => {
                ok(h.cash_out(&users[*i], t, 3 * USDC));
            }
            _ => {}
        }
    }
    let open: u64 = tickets
        .iter()
        .map(|(_, t)| h.ticket(t))
        .filter(|t| t.state == TicketState::Open)
        .map(|t| t.payout_u)
        .sum();
    assert_eq!(h.vault().locked, open);
}
