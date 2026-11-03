//! Step 2.F: own tips and `TipsterProfile`, copies and the copy-fee split. Extra
//! accounts travel in `remaining_accounts` exactly as contract L2-4 lists them.
//! `assert_invariants` (every send) also checks each profile against the own tips of
//! its wallet, so a double count or a copy touching a profile fails any test.

// The error type of litesvm results.
#![allow(clippy::result_large_err)]

mod common;

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::AccountMeta;
use common::*;
use schedina::error::SchedinaError;
use schedina::events::{PendingOpened, PendingRefunded, TicketMinted, TipsterPaid};
use schedina::{QuoteLeg, TicketState, TipsterProfile};
use solana_account::Account;
use solana_signer::Signer;

/// 2 long legs at 0.50 (overlap tier) on 10 USDC: payout 36.1 USDC.
const STAKE: u64 = 10 * USDC;
const PAYOUT: u64 = 36_100_000;
/// 2% of 10 USDC, and its 80/20 split.
const FEE: u64 = 200_000;
const TIPSTER_CUT: u64 = 160_000;
const HOUSE_CUT: u64 = 40_000;

fn legs() -> Vec<QuoteLeg> {
    legs_at_half(2)
}

/// Ready harness, a tipster with a minted own tip (nonce 1), and a copier.
fn setup() -> (Harness, User, Pubkey, User) {
    let mut h = Harness::ready();
    let tipster = h.user(100 * USDC);
    let orig = h.mint_tip(&tipster, 1, legs(), STAKE);
    let copier = h.user(100 * USDC);
    (h, tipster, orig, copier)
}

fn profile_tuple(p: &TipsterProfile) -> (u32, u64, u32, u32, u64, u64, u64) {
    (
        p.open_count,
        p.open_staked,
        p.settled_count,
        p.won,
        p.settled_staked,
        p.returned,
        p.fees_earned,
    )
}

/// Opens a copy of `orig` on `legs` (fresh quote, floor = offered odds).
fn open_copy(h: &mut Harness, copier: &User, nonce: u64, legs: Vec<QuoteLeg>, orig: &Pubkey) {
    let q = quote(legs, STAKE, h.now());
    ok(h.open_flagged(copier, nonce, &q, false, Some(*orig)));
}

// --- own tips -----------------------------------------------------------------

#[test]
fn own_tip_profile_opens_at_finalize_and_settles_at_settle() {
    let mut h = Harness::ready();
    let tipster = h.user(100 * USDC);
    let other = h.user(100 * USDC);
    let q = quote(legs(), STAKE, h.now());
    ok(h.open_flagged(&tipster, 1, &q, true, None));
    assert!(
        h.profile(&tipster.key()).is_none(),
        "open_pending touches no profile"
    );

    // finalize needs the profile account (the crank may not mint a tip off the record).
    let res = h.finalize_with(&tipster, 1, vec![0, 0], 1_000_000, &[]);
    assert_err(&res, SchedinaError::BadTipsterProfile);
    let res = h.finalize_with(
        &tipster,
        1,
        vec![0, 0],
        1_000_000,
        &tip_accounts(&other.key()),
    );
    assert_err(&res, SchedinaError::BadTipsterProfile);
    let ro = [AccountMeta::new_readonly(
        tipster_pda(&tipster.key()),
        false,
    )];
    let res = h.finalize_with(&tipster, 1, vec![0, 0], 1_000_000, &ro);
    assert_err(&res, SchedinaError::BadTipsterProfile);

    let crank_sol = h.svm.get_balance(&h.crank.pubkey()).unwrap();
    let meta = ok(h.finalize_with(
        &tipster,
        1,
        vec![0, 0],
        1_000_000,
        &tip_accounts(&tipster.key()),
    ));
    assert!(
        h.svm.get_balance(&h.crank.pubkey()).unwrap() < crank_sol,
        "crank pays the rent"
    );
    let t = ticket_pda(&tipster.key(), 1);
    let ev = cpi_events::<TicketMinted>(&meta);
    assert_eq!(ev[0].tipster, Some(tipster.key()));
    assert!(cpi_events::<TipsterPaid>(&meta).is_empty());
    let p = h
        .profile(&tipster.key())
        .expect("profile created by finalize");
    assert_eq!(p.wallet, tipster.key());
    assert_eq!(profile_tuple(&p), (1, STAKE, 0, 0, 0, 0, 0));

    // resolve_leg never touches the profile.
    ok(h.resolve(&t, 0, 1_000_000));
    ok(h.resolve(&t, 1, 1_000_000));
    assert_eq!(
        profile_tuple(&h.profile(&tipster.key()).unwrap()),
        (1, STAKE, 0, 0, 0, 0, 0)
    );

    // settle of an own tip needs its profile: missing, another wallet's, read-only.
    let caller = h.wallet();
    assert_err(
        &h.settle_with(&caller, &t, &tipster.usdc, &[]),
        SchedinaError::BadTipsterProfile,
    );
    h.mint_tip(&other, 1, legs(), STAKE);
    assert_err(
        &h.settle_with(&caller, &t, &tipster.usdc, &tip_accounts(&other.key())),
        SchedinaError::BadTipsterProfile,
    );
    assert_err(
        &h.settle_with(&caller, &t, &tipster.usdc, &ro),
        SchedinaError::BadTipsterProfile,
    );
    // The 1.C builder (no remaining accounts) is refused the same way.
    assert_err(
        &h.settle(&caller, &t, &tipster.usdc),
        SchedinaError::BadTipsterProfile,
    );
    ok(h.settle_with(&caller, &t, &tipster.usdc, &tip_accounts(&tipster.key())));
    assert_eq!(h.ticket(&t).state, TicketState::Settled);
    assert_eq!(
        profile_tuple(&h.profile(&tipster.key()).unwrap()),
        (0, 0, 1, 1, STAKE, PAYOUT, 0)
    );
    // The other wallet's profile did not move.
    assert_eq!(
        profile_tuple(&h.profile(&other.key()).unwrap()),
        (1, STAKE, 0, 0, 0, 0, 0)
    );
}

#[test]
fn own_tip_cash_out_needs_the_profile_and_records_c() {
    let (mut h, tipster, t, _) = setup();
    assert_err(
        &h.cash_out_remaining(&tipster, &t, 5 * USDC, &[]),
        SchedinaError::BadTipsterProfile,
    );
    // The 1.C builder (no remaining accounts) is refused too.
    assert_err(
        &h.cash_out(&tipster, &t, 5 * USDC),
        SchedinaError::BadTipsterProfile,
    );
    ok(h.cash_out_remaining(&tipster, &t, 5 * USDC, &tip_accounts(&tipster.key())));
    assert_eq!(h.ticket(&t).state, TicketState::CashedOut);
    // C = 5 < stake 10: settled, not won.
    assert_eq!(
        profile_tuple(&h.profile(&tipster.key()).unwrap()),
        (0, 0, 1, 0, STAKE, 5 * USDC, 0)
    );
    // A second tip cashed out above its stake counts as won.
    let t2 = h.mint_tip(&tipster, 2, legs(), STAKE);
    ok(h.cash_out_remaining(&tipster, &t2, 12 * USDC, &tip_accounts(&tipster.key())));
    assert_eq!(
        profile_tuple(&h.profile(&tipster.key()).unwrap()),
        (0, 0, 2, 1, 2 * STAKE, 17 * USDC, 0)
    );
}

#[test]
fn non_tips_ignore_extra_accounts_and_never_create_a_profile() {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    let q = quote(legs(), STAKE, h.now());
    ok(h.open_flagged(&user, 1, &q, false, None));
    ok(h.finalize_with(&user, 1, vec![0, 0], 1_000_000, &tip_accounts(&user.key())));
    let t = ticket_pda(&user.key(), 1);
    ok(h.resolve(&t, 0, 0));
    ok(h.resolve(&t, 1, 0));
    let caller = h.wallet();
    ok(h.settle_with(&caller, &t, &user.usdc, &tip_accounts(&user.key())));
    assert!(h.profile(&user.key()).is_none());
    let t2 = h.mint(&user, 2, legs(), STAKE);
    ok(h.cash_out_remaining(&user, &t2, USDC, &tip_accounts(&user.key())));
    assert!(h.profile(&user.key()).is_none());
}

#[test]
fn a_tip_cannot_be_a_copy() {
    let (mut h, _, orig, copier) = setup();
    let q = quote(legs(), STAKE, h.now());
    assert_err(
        &h.open_flagged(&copier, 1, &q, true, Some(orig)),
        SchedinaError::TipNotAllowed,
    );
}

#[test]
fn init_tipster_profile_is_idempotent_and_finalize_survives_a_prefunded_pda() {
    let mut h = Harness::ready();
    let payer = h.wallet();
    let wallet = h.wallet().pubkey();
    let ix = h.ix_init_tipster_profile(&payer.pubkey(), &wallet);
    ok(h.send(ix.clone(), &[&payer]));
    let p = h.profile(&wallet).unwrap();
    assert_eq!(
        (p.wallet, profile_tuple(&p)),
        (wallet, (0, 0, 0, 0, 0, 0, 0))
    );
    ok(h.send(ix, &[&payer]));

    // Someone sends lamports to a future profile address: finalize still creates it.
    let tipster = h.user(100 * USDC);
    h.svm
        .airdrop(&tipster_pda(&tipster.key()), 1_000_000)
        .unwrap(); // below the profile rent
    h.mint_tip(&tipster, 1, legs(), STAKE);
    let acc = h.svm.get_account(&tipster_pda(&tipster.key())).unwrap();
    assert_eq!(acc.owner, schedina::ID);
    assert!(acc.lamports >= h.svm.minimum_balance_for_rent_exemption(acc.data.len()));
    // init on an existing, non-empty profile changes nothing.
    let ix = h.ix_init_tipster_profile(&payer.pubkey(), &tipster.key());
    ok(h.send(ix, &[&payer]));
    assert_eq!(
        profile_tuple(&h.profile(&tipster.key()).unwrap()),
        (1, STAKE, 0, 0, 0, 0, 0)
    );
    // A pre-created profile is reused by finalize.
    let t2 = h.mint_tip(&tipster, 2, legs(), STAKE);
    assert_eq!(h.ticket(&t2).tipster, Some(tipster.key()));
    assert_eq!(
        profile_tuple(&h.profile(&tipster.key()).unwrap()),
        (2, 2 * STAKE, 0, 0, 0, 0, 0)
    );
}

// --- copies -------------------------------------------------------------------

#[test]
fn self_copy_from_a_second_wallet_pays_the_fee_minus_20_percent() {
    let (mut h, tipster, orig, copier) = setup();
    let before = h.balance(&tipster.usdc) + h.balance(&copier.usdc);
    let q = quote(legs(), STAKE, h.now());
    let meta = ok(h.open_flagged(&copier, 1, &q, false, Some(orig)));
    let ev = cpi_events::<PendingOpened>(&meta);
    assert_eq!((ev[0].stake_u, ev[0].copy_fee_u), (STAKE, FEE));
    assert_eq!(h.balance(&copier.usdc), 100 * USDC - STAKE - FEE);
    let p = h.pending(&copier.key(), 1);
    assert_eq!(
        (p.copy_fee_u, p.tipster, p.copied_from),
        (FEE, None, Some(orig))
    );
    let profile0 = profile_tuple(&h.profile(&tipster.key()).unwrap());

    let free0 = h.vault().free;
    let tipster_bal0 = h.balance(&tipster.usdc);
    let meta = ok(h.finalize_with(
        &copier,
        1,
        vec![0, 0],
        1_000_000,
        &copy_accounts(&orig, &tipster.usdc),
    ));
    let copy = ticket_pda(&copier.key(), 1);
    assert_eq!(h.balance(&tipster.usdc), tipster_bal0 + TIPSTER_CUT);
    assert_eq!(h.vault().free, free0 + STAKE - PAYOUT + HOUSE_CUT);
    assert_eq!(h.balance(&copier.usdc), 100 * USDC - STAKE - FEE);
    let paid = cpi_events::<TipsterPaid>(&meta);
    assert_eq!(paid.len(), 1);
    assert_eq!(
        (paid[0].tipster, paid[0].ticket, paid[0].amount_u),
        (tipster.key(), copy, TIPSTER_CUT)
    );
    let minted = cpi_events::<TicketMinted>(&meta);
    assert_eq!(
        (minted[0].tipster, minted[0].copied_from, minted[0].refund_u),
        (Some(tipster.key()), Some(orig), 0)
    );
    let t = h.ticket(&copy);
    assert_eq!(
        (t.tipster, t.copied_from, t.copy_fee_u, t.payout_u),
        (Some(tipster.key()), Some(orig), FEE, PAYOUT)
    );
    // The same person behind both wallets is out exactly the house's 20%.
    assert_eq!(
        h.balance(&tipster.usdc) + h.balance(&copier.usdc),
        before - STAKE - HOUSE_CUT
    );
    // Copies never touch a profile: not at finalize, not at settle.
    assert_eq!(profile_tuple(&h.profile(&tipster.key()).unwrap()), profile0);
    assert!(h.profile(&copier.key()).is_none());
    ok(h.resolve(&copy, 0, 1_000_000));
    ok(h.resolve(&copy, 1, 1_000_000));
    let caller = h.wallet();
    ok(h.settle(&caller, &copy, &copier.usdc));
    assert_eq!(profile_tuple(&h.profile(&tipster.key()).unwrap()), profile0);
    assert!(h.profile(&copier.key()).is_none());
}

#[test]
fn partial_fill_scales_the_split_and_refunds_the_rest() {
    let (mut h, tipster, orig, copier) = setup();
    open_copy(&mut h, &copier, 1, legs(), &orig);
    let free0 = h.vault().free;
    let tipster_bal0 = h.balance(&tipster.usdc);
    // r = 0.6: rS = 6 USDC, used fee 120_000 (96_000 tipster, 24_000 house),
    // refund 4 USDC + 80_000.
    let meta = ok(h.finalize_with(
        &copier,
        1,
        vec![0, 0],
        600_000,
        &copy_accounts(&orig, &tipster.usdc),
    ));
    let payout = 21_660_000; // 6 × 0.95/0.5 × 0.95/0.5
    assert_eq!(h.balance(&tipster.usdc), tipster_bal0 + 96_000);
    assert_eq!(h.vault().free, free0 + 6 * USDC - payout + 24_000);
    assert_eq!(h.balance(&copier.usdc), 100 * USDC - 6 * USDC - 120_000);
    let minted = cpi_events::<TicketMinted>(&meta);
    assert_eq!(
        (minted[0].stake_u, minted[0].refund_u, minted[0].payout_u),
        (6 * USDC, 4 * USDC + 80_000, payout)
    );
    assert_eq!(cpi_events::<TipsterPaid>(&meta)[0].amount_u, 96_000);
    let t = h.ticket(&ticket_pda(&copier.key(), 1));
    assert_eq!((t.copy_fee_u, t.refund_u), (120_000, 4 * USDC + 80_000));
}

#[test]
fn refund_pending_returns_stake_and_copy_fee() {
    let (mut h, _, orig, copier) = setup();
    open_copy(&mut h, &copier, 1, legs(), &orig);
    h.warp(45 + 15);
    let caller = h.wallet();
    let ix = h.ix_refund_pending(&caller.pubkey(), &copier.key(), 1, &copier.usdc);
    let meta = ok(h.send(ix, &[&caller]));
    assert_eq!(
        cpi_events::<PendingRefunded>(&meta)[0].amount_u,
        STAKE + FEE
    );
    assert_eq!(h.balance(&copier.usdc), 100 * USDC);
}

#[test]
fn a_tip_in_play_is_minted_and_copied() {
    let mut h = Harness::ready();
    let tipster = h.user(100 * USDC);
    let copier = h.user(100 * USDC);
    let mut l = legs();
    l[1].start_ts = h.now() + 40; // leg 0 starts at START, two hours later
    h.warp(41); // leg 1 is in play before the tip is even opened (D-660)
    let orig = h.mint_tip(&tipster, 1, l.clone(), STAKE);
    assert_eq!(h.profile(&tipster.key()).unwrap().open_count, 1);
    open_copy(&mut h, &copier, 1, l, &orig);
    ok(h.finalize_with(
        &copier,
        1,
        vec![0, 0],
        1_000_000,
        &copy_accounts(&orig, &tipster.usdc),
    ));
}

#[test]
fn copy_after_a_leg_of_the_original_resolved_fails() {
    let (mut h, tipster, orig, copier) = setup();
    open_copy(&mut h, &copier, 1, legs(), &orig);
    ok(h.resolve(&orig, 0, 1_000_000));
    assert_err(
        &h.finalize_with(
            &copier,
            1,
            vec![0, 0],
            1_000_000,
            &copy_accounts(&orig, &tipster.usdc),
        ),
        SchedinaError::CopyTooLate,
    );
}

#[test]
fn copy_with_another_side_game_or_leg_count_fails() {
    let (mut h, tipster, orig, copier) = setup();
    let acc = copy_accounts(&orig, &tipster.usdc);
    let mut side = legs();
    side[1].side = 1;
    let mut game = legs();
    game[1] = leg(7, 0, 500_000, 50_000);
    let mut symbol = legs();
    symbol[1].symbol_hash = [99; 32];
    for (nonce, l) in [
        (1, side),
        (2, game),
        (3, symbol),
        (4, legs_at_half(1)),
        (5, legs_at_half(3)),
    ] {
        open_copy(&mut h, &copier, nonce, l.clone(), &orig);
        let fills = vec![0; l.len()];
        assert_err(
            &h.finalize_with(&copier, nonce, fills, 1_000_000, &acc),
            SchedinaError::CopyMismatch,
        );
    }
    // The same legs in another order are the same ticket.
    let mut rev = legs();
    rev.reverse();
    open_copy(&mut h, &copier, 6, rev, &orig);
    ok(h.finalize_with(&copier, 6, vec![0, 0], 1_000_000, &acc));
}

#[test]
fn copy_of_a_non_tip_a_closed_tip_or_a_copy_fails() {
    let (mut h, tipster, orig, copier) = setup();
    // Not a tip.
    let plain = h.mint(&tipster, 2, legs(), STAKE);
    open_copy(&mut h, &copier, 1, legs(), &plain);
    assert_err(
        &h.finalize_with(
            &copier,
            1,
            vec![0, 0],
            1_000_000,
            &copy_accounts(&plain, &tipster.usdc),
        ),
        SchedinaError::OriginalNotCopyable,
    );
    // A copy of a copy.
    open_copy(&mut h, &copier, 2, legs(), &orig);
    ok(h.finalize_with(
        &copier,
        2,
        vec![0, 0],
        1_000_000,
        &copy_accounts(&orig, &tipster.usdc),
    ));
    let copy = ticket_pda(&copier.key(), 2);
    let third = h.user(100 * USDC);
    open_copy(&mut h, &third, 1, legs(), &copy);
    assert_err(
        &h.finalize_with(
            &third,
            1,
            vec![0, 0],
            1_000_000,
            &copy_accounts(&copy, &tipster.usdc),
        ),
        SchedinaError::OriginalNotCopyable,
    );
    assert_err(
        &h.finalize_with(
            &third,
            1,
            vec![0, 0],
            1_000_000,
            &copy_accounts(&copy, &copier.usdc),
        ),
        SchedinaError::OriginalNotCopyable,
    );
    // A cashed-out tip.
    ok(h.cash_out_remaining(&tipster, &orig, USDC, &tip_accounts(&tipster.key())));
    open_copy(&mut h, &copier, 3, legs(), &orig);
    assert_err(
        &h.finalize_with(
            &copier,
            3,
            vec![0, 0],
            1_000_000,
            &copy_accounts(&orig, &tipster.usdc),
        ),
        SchedinaError::OriginalNotCopyable,
    );
}

#[test]
fn copy_accounts_are_checked() {
    let (mut h, tipster, orig, copier) = setup();
    let other_tip = h.mint_tip(&copier, 9, legs(), STAKE);
    open_copy(&mut h, &copier, 1, legs(), &orig);
    let fin = |h: &mut Harness, acc: &[AccountMeta]| {
        h.finalize_with(&copier, 1, vec![0, 0], 1_000_000, acc)
    };
    // Missing, or only the ticket.
    assert_err(&fin(&mut h, &[]), SchedinaError::BadCopyAccounts);
    assert_err(
        &fin(&mut h, &[AccountMeta::new_readonly(orig, false)]),
        SchedinaError::BadCopyAccounts,
    );
    // A valid tip, but not the one named by copied_from.
    assert_err(
        &fin(&mut h, &copy_accounts(&other_tip, &copier.usdc)),
        SchedinaError::BadCopyAccounts,
    );
    // Not a ticket at all.
    assert_err(
        &fin(&mut h, &copy_accounts(&config_pda(), &tipster.usdc)),
        SchedinaError::BadCopyAccounts,
    );
    // The fee account must be the tipster's, writable, on the config mint.
    assert_err(
        &fin(&mut h, &copy_accounts(&orig, &copier.usdc)),
        SchedinaError::BadCopyAccounts,
    );
    assert_err(
        &fin(&mut h, &copy_accounts(&orig, &vault_usdc())),
        SchedinaError::BadCopyAccounts,
    );
    let ro = [
        AccountMeta::new_readonly(orig, false),
        AccountMeta::new_readonly(tipster.usdc, false),
    ];
    assert_err(&fin(&mut h, &ro), SchedinaError::BadCopyAccounts);
    let foreign = Pubkey::new_unique();
    let mut data = vec![0u8; anchor_spl::token::TokenAccount::LEN];
    data[0..32].copy_from_slice(Pubkey::new_unique().as_ref()); // another mint
    data[32..64].copy_from_slice(tipster.key().as_ref());
    data[108] = 1;
    let lamports = h.svm.minimum_balance_for_rent_exemption(data.len());
    h.svm
        .set_account(
            foreign,
            Account {
                lamports,
                data,
                owner: anchor_spl::token::ID,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
    assert_err(
        &fin(&mut h, &copy_accounts(&orig, &foreign)),
        SchedinaError::BadCopyAccounts,
    );
    // Any USDC account of the tipster works, not only the ATA.
    let second = h.token_account_for(&tipster.key());
    let meta = ok(fin(&mut h, &copy_accounts(&orig, &second)));
    assert_eq!(h.balance(&second), TIPSTER_CUT);
    println!("CU finalize(copy, 2 legs): {}", meta.compute_units_consumed);
    assert!(meta.compute_units_consumed < 200_000);
}

#[test]
fn copy_fee_is_two_percent_rounded_up() {
    let (mut h, _, orig, copier) = setup();
    let q = quote(legs(), 1_000_001, h.now());
    ok(h.open_flagged(&copier, 1, &q, false, Some(orig)));
    assert_eq!(h.pending(&copier.key(), 1).copy_fee_u, 20_001);
    assert_eq!(h.balance(&copier.usdc), 100 * USDC - 1_000_001 - 20_001);
}

/// 5 legs: CU of the 2.F paths, and the transactions with their remaining accounts
/// (plus a wallet's two compute-budget instructions) still fit one packet.
#[test]
fn five_leg_tips_and_copies_fit_and_stay_cheap() {
    let mut h = Harness::ready();
    let tipster = h.user(100 * USDC);
    let copier = h.user(100 * USDC);
    let q = quote(legs_at_half(5), STAKE, h.now());
    ok(h.open_flagged(&tipster, 1, &q, true, None));
    let tip_acc = tip_accounts(&tipster.key());
    let tip_fin = ok(h.finalize_with(&tipster, 1, vec![0; 5], 1_000_000, &tip_acc));
    let orig = ticket_pda(&tipster.key(), 1);
    open_copy(&mut h, &copier, 1, legs_at_half(5), &orig);
    let copy_acc = copy_accounts(&orig, &tipster.usdc);
    let copy_fin = ok(h.finalize_with(&copier, 1, vec![0; 5], 1_000_000, &copy_acc));
    let co = ok(h.cash_out_remaining(&tipster, &orig, USDC, &tip_acc));
    let cu = [
        tip_fin.compute_units_consumed,
        copy_fin.compute_units_consumed,
        co.compute_units_consumed,
    ];
    println!("CU finalize(tip, 5, profile created) / finalize(copy, 5) / cash_out(tip): {cu:?}");
    assert!(cu.iter().all(|&c| c < 200_000), "{cu:?}");

    let crank = h.crank.pubkey();
    let mut ixs = compute_budget_ixs(200_000, 1);
    ixs.push(with_remaining(
        h.ix_finalize(&copier.key(), 2, &copier.usdc, vec![1; 5], 1_000_000),
        &copy_acc,
    ));
    let fin_size = tx_size(&ixs, &crank, 1);
    let msg = cashout_msg(&orig, USDC, h.now());
    let signer = h.quote_signer.insecure_clone();
    let mut co_ixs = h.cash_out_ixs(&tipster, &msg, &signer);
    let last = co_ixs.pop().unwrap();
    co_ixs.push(with_remaining(last, &tip_acc));
    let mut ixs = compute_budget_ixs(200_000, 1);
    ixs.extend(co_ixs);
    let co_size = tx_size(&ixs, &tipster.key(), 1);
    println!("bytes finalize(copy, 5) / cash_out(tip): {fin_size} / {co_size}");
    assert!(fin_size <= PACKET_DATA_SIZE && co_size <= PACKET_DATA_SIZE);
}
