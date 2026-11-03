//! L8 sponsored transactions: the desk's sponsor key pays the fee and the Pending rent, the
//! user signs with no SOL, and the rent goes back to whoever paid it.
#![allow(clippy::result_large_err)]

mod common;

use anchor_lang::error::ErrorCode as AnchorError;
use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::instruction::Instruction;
use common::*;
use solana_keypair::Keypair;

use solana_message::{v0, AddressLookupTableAccount, Hash, VersionedMessage};
use solana_signer::Signer;

/// A wallet with mock USDC and no lamports at all.
fn broke_user(h: &mut Harness, usdc: u64) -> User {
    let kp = Keypair::new();
    let usdc_acc = Keypair::new().pubkey();
    h.write_token_account(&usdc_acc, &kp.pubkey(), usdc);
    User { kp, usdc: usdc_acc }
}

fn lamports(h: &Harness, k: &Pubkey) -> u64 {
    h.svm.get_account(k).map_or(0, |a| a.lamports)
}

/// `[verify, open_pending(payer = sponsor)]`, fee payer the sponsor, signed by both.
fn open_sponsored(h: &mut Harness, user: &User, sponsor: &Keypair, nonce: u64, n: u8) -> TxResult {
    let q = quote(legs_at_half(n), 10 * USDC, h.now());
    let msg = borsh(&q);
    let sig = sign(&h.quote_signer, &msg);
    let verify = ed25519_ix(&msg, &sig, &h.quote_signer.pubkey());
    let open = h.ix_open_pending_paid_by(
        user,
        &sponsor.pubkey(),
        nonce,
        q.odds_offered_u,
        h.now() + 45,
        false,
        None,
    );
    let user_kp = user.kp.insecure_clone();
    h.send_ixs(&[verify, open], &[sponsor, &user_kp])
}

#[test]
fn a_user_with_no_sol_opens_and_is_finalized_rent_back_to_the_sponsor() {
    let mut h = Harness::ready();
    let user = broke_user(&mut h, 100 * USDC);
    let sponsor = h.wallet();
    let before = lamports(&h, &sponsor.pubkey());

    ok(open_sponsored(&mut h, &user, &sponsor, 1, 2));
    assert_eq!(lamports(&h, &user.key()), 0, "the user paid nothing");
    assert_eq!(h.balance(&user.usdc), 90 * USDC);
    let p = h.pending(&user.key(), 1);
    assert_eq!((p.user, p.rent_payer), (user.key(), sponsor.pubkey()));
    assert_eq!(p.rent_receiver(), sponsor.pubkey());
    let rent = lamports(&h, &pending_pda(&user.key(), 1));
    assert!(before - lamports(&h, &sponsor.pubkey()) >= rent);

    // The rent cannot be steered to the user.
    let crank = h.crank.pubkey();
    let to_user = h.ix_finalize_rent_to(
        &crank,
        &user.key(),
        1,
        &user.usdc,
        vec![0, 0],
        1_000_000,
        &user.key(),
    );
    assert_anchor_err(&h.send_crank(to_user), AnchorError::ConstraintAddress);

    let mid = lamports(&h, &sponsor.pubkey());
    let fin = h.ix_finalize_rent_to(
        &crank,
        &user.key(),
        1,
        &user.usdc,
        vec![0, 0],
        1_000_000,
        &sponsor.pubkey(),
    );
    ok(h.send_crank(fin));
    assert!(!h.exists(&pending_pda(&user.key(), 1)));
    assert_eq!(lamports(&h, &sponsor.pubkey()), mid + rent);
    assert_eq!(lamports(&h, &user.key()), 0);
}

#[test]
fn a_sponsored_pending_refunds_its_rent_to_the_sponsor() {
    let mut h = Harness::ready();
    let user = broke_user(&mut h, 100 * USDC);
    let sponsor = h.wallet();
    ok(open_sponsored(&mut h, &user, &sponsor, 4, 1));
    let rent = lamports(&h, &pending_pda(&user.key(), 4));
    let exp = h.pending(&user.key(), 4).expires_at;
    h.set_time(exp + 15);

    let stranger = h.wallet();
    let to_user = h.ix_refund_pending(&stranger.pubkey(), &user.key(), 4, &user.usdc);
    assert_anchor_err(
        &h.send(to_user, &[&stranger]),
        AnchorError::ConstraintAddress,
    );
    let mid = lamports(&h, &sponsor.pubkey());
    let ix = h.ix_refund_pending_rent_to(
        &stranger.pubkey(),
        &user.key(),
        4,
        &user.usdc,
        &sponsor.pubkey(),
    );
    ok(h.send(ix, &[&stranger]));
    assert_eq!(h.balance(&user.usdc), 100 * USDC);
    assert_eq!(lamports(&h, &sponsor.pubkey()), mid + rent);
    assert_eq!(lamports(&h, &user.key()), 0);
}

#[test]
fn a_self_paid_pending_records_the_user_as_rent_payer() {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    h.open_default(&user, 2, legs_at_half(1), 10 * USDC);
    let p = h.pending(&user.key(), 2);
    assert_eq!((p.rent_payer, p.rent_receiver()), (user.key(), user.key()));
}

#[test]
fn the_sponsor_must_sign_as_payer() {
    let mut h = Harness::ready();
    let user = broke_user(&mut h, 100 * USDC);
    let sponsor = h.wallet();
    let q = quote(legs_at_half(1), 10 * USDC, h.now());
    let msg = borsh(&q);
    let sig = sign(&h.quote_signer, &msg);
    let verify = ed25519_ix(&msg, &sig, &h.quote_signer.pubkey());
    let mut open = h.ix_open_pending_paid_by(
        &user,
        &sponsor.pubkey(),
        3,
        q.odds_offered_u,
        h.now() + 45,
        false,
        None,
    );
    // The payer meta without its signer flag (someone else's SOL): refused.
    for m in open.accounts.iter_mut() {
        if m.pubkey == sponsor.pubkey() {
            m.is_signer = false;
        }
    }
    let fee_payer = h.wallet();
    let user_kp = user.kp.insecure_clone();
    assert_anchor_err(
        &h.send_ixs(&[verify, open], &[&fee_payer, &user_kp]),
        AnchorError::AccountNotSigner,
    );
}

/// The static accounts of `open_pending` that the devnet lookup table holds
/// (`AmmConfig.lookup_table`, built by `crank amm-setup`: desk `swap::ix::lookup_table_addresses`
/// starts with these six).
fn lookup_table() -> AddressLookupTableAccount {
    AddressLookupTableAccount {
        key: Keypair::new().pubkey(),
        addresses: vec![
            config_pda(),
            escrow_usdc(),
            sysvar_instructions(),
            anchor_spl::token::ID,
            anchor_lang::system_program::ID,
            event_authority(),
        ],
    }
}

fn v0_size(ixs: &[Instruction], payer: &Pubkey, table: &AddressLookupTableAccount) -> usize {
    let msg =
        v0::Message::try_compile(payer, ixs, std::slice::from_ref(table), Hash::default()).unwrap();
    let n = usize::from(msg.header.num_required_signatures);
    1 + 64 * n + VersionedMessage::V0(msg).serialize().len()
}

/// Worst case for a sponsored bet: 5 legs, `copied_from`, two compute-budget instructions and
/// a second signer (the sponsor as fee payer and rent payer). A legacy transaction fits up to
/// 4 legs; with the lookup table (v0), all 5 fit. The app sends sponsored bets as v0 (L8).
#[test]
fn sponsored_transaction_sizes() {
    let mut h = Harness::ready();
    let user = broke_user(&mut h, 100 * USDC);
    let sponsor = Keypair::new().pubkey();
    let signer = &h.quote_signer;
    let table = lookup_table();
    let mut sizes = vec![];
    for n in 1..=5u8 {
        let q = quote(legs_at_half(n), 10 * USDC, h.now());
        let msg = borsh(&q);
        let sig = sign(signer, &msg);
        let program_ix = h.ix_open_pending_paid_by(
            &user,
            &sponsor,
            u64::MAX,
            q.odds_offered_u,
            h.now() + 45,
            true,
            Some(Keypair::new().pubkey()),
        );
        let mut ixs = compute_budget_ixs(200_000, 1);
        ixs.extend([ed25519_ix(&msg, &sig, &signer.pubkey()), program_ix]);
        sizes.push((
            n,
            tx_size(&ixs, &sponsor, 2),
            v0_size(&ixs, &sponsor, &table),
        ));
    }
    println!(
        "sponsored (n, legacy bytes, v0 bytes) with 2 compute-budget ixs, copied_from: {sizes:?}"
    );
    for &(n, legacy, v0) in &sizes {
        assert!(
            n == 5 || legacy <= PACKET_DATA_SIZE,
            "{n} legs legacy: {legacy} bytes"
        );
        assert!(v0 <= PACKET_DATA_SIZE, "{n} legs v0: {v0} bytes");
    }
}
