//! Desk-signature checks of `open_pending` and `cash_out` through the instructions
//! sysvar (C3), plus the precompile itself in LiteSVM (D-018). Rule: the transaction
//! is `[..., Ed25519Program verify, schedina ix]`, the verify carries exactly one
//! signature, and all its offsets point inside itself.

mod common;

use common::*;
use schedina::error::SchedinaError;
use schedina::{QuoteMsg, CASHOUT_TAG};
use solana_keypair::Keypair;
use solana_signer::Signer;

fn setup() -> (Harness, User, QuoteMsg) {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    let q = quote(legs_at_half(3), 10 * USDC, h.now());
    (h, user, q)
}

#[test]
fn signed_quote_opens_a_pending() {
    let (mut h, user, q) = setup();
    ok(h.open(&user, 1, &q, q.odds_offered_u, h.now() + 45));
    let p = h.pending(&user.key(), 1);
    let msg = borsh(&q);
    assert_eq!(p.quote_sig, sign(&h.quote_signer, &msg));
    assert_eq!(p.quote_hash, solana_sha256_hasher::hash(&msg).to_bytes());
}

#[test]
fn wallet_prepended_instructions_are_fine() {
    let (mut h, user, q) = setup();
    let s = h.quote_signer.insecure_clone();
    let mut ixs = compute_budget_ixs(400_000, 1_000);
    ixs.extend(h.open_ixs(
        &user,
        1,
        &borsh(&q),
        &s,
        q.odds_offered_u,
        h.now() + 45,
        false,
    ));
    ok(h.send_ixs(&ixs, &[&user.kp]));

    let t = h.mint(&user, 2, legs_at_half(1), 10 * USDC);
    let mut ixs = compute_budget_ixs(400_000, 1_000);
    ixs.extend(h.cash_out_ixs(&user, &cashout_msg(&t, USDC, h.now()), &s));
    ok(h.send_ixs(&ixs, &[&user.kp]));
}

#[test]
fn verify_not_immediately_before_fails() {
    let (mut h, user, q) = setup();
    let s = h.quote_signer.insecure_clone();
    // [verify, compute budget, open_pending]
    let pair = h.open_ixs(
        &user,
        1,
        &borsh(&q),
        &s,
        q.odds_offered_u,
        h.now() + 45,
        false,
    );
    let ixs = [
        vec![pair[0].clone()],
        compute_budget_ixs(400_000, 1),
        vec![pair[1].clone()],
    ]
    .concat();
    assert_err(
        &h.send_ixs(&ixs, &[&user.kp]),
        SchedinaError::SignatureMissing,
    );
    // [open_pending, verify]
    let ixs = [pair[1].clone(), pair[0].clone()];
    assert_err(
        &h.send_ixs(&ixs, &[&user.kp]),
        SchedinaError::SignatureMissing,
    );
    // No verify at all.
    assert_err(
        &h.send(pair[1].clone(), &[&user.kp]),
        SchedinaError::SignatureMissing,
    );

    let t = h.mint(&user, 2, legs_at_half(1), 10 * USDC);
    let pair = h.cash_out_ixs(&user, &cashout_msg(&t, USDC, h.now()), &s);
    let ixs = [
        vec![pair[0].clone()],
        compute_budget_ixs(400_000, 1),
        vec![pair[1].clone()],
    ]
    .concat();
    assert_err(
        &h.send_ixs(&ixs, &[&user.kp]),
        SchedinaError::SignatureMissing,
    );
}

#[test]
fn verify_must_be_single_and_self_contained() {
    let (mut h, user, _) = setup();
    let t = h.mint(&user, 1, legs_at_half(1), 10 * USDC);
    let co = cashout_msg(&t, USDC, h.now());
    let bytes = borsh(&co);
    let sig = sign(&h.quote_signer, &bytes);
    let pk = h.quote_signer.pubkey();
    let program_ix = h.ix_cash_out(&user.key(), &t, &user.usdc, co.clone());

    // Message read from the cash_out instruction's own data (valid for the precompile,
    // refused by the program: absolute indices are not position-independent).
    let ixs = [
        ed25519_ix_external_msg(bytes.len(), 1, &sig, &pk),
        program_ix.clone(),
    ];
    assert_err(
        &h.send_ixs(&ixs, &[&user.kp]),
        SchedinaError::SignatureMalformed,
    );

    // Two signatures in one verify instruction: the desk's twice.
    let mut d = vec![2u8, 0];
    let base = 2 + 14 * 2;
    let (pk_off, sig_off, msg_off) = (base, base + 32, base + 96);
    for _ in 0..2 {
        for v in [
            sig_off,
            0xFFFF,
            pk_off,
            0xFFFF,
            msg_off,
            bytes.len(),
            0xFFFF,
        ] {
            d.extend_from_slice(&(v as u16).to_le_bytes());
        }
    }
    d.extend_from_slice(pk.as_ref());
    d.extend_from_slice(&sig);
    d.extend_from_slice(&bytes);
    let mut two = ed25519_ix(&bytes, &sig, &pk);
    two.data = d;
    let ixs = [two, program_ix];
    assert_err(
        &h.send_ixs(&ixs, &[&user.kp]),
        SchedinaError::SignatureMalformed,
    );
}

#[test]
fn corrupted_signature_fails_in_the_precompile() {
    let (mut h, user, q) = setup();
    let msg = borsh(&q);
    let mut sig = sign(&h.quote_signer, &msg);
    sig[0] ^= 1;
    let ixs = [
        ed25519_ix(&msg, &sig, &h.quote_signer.pubkey()),
        h.ix_open_pending(&user, 1, q.odds_offered_u, h.now() + 45, false, None),
    ];
    assert_precompile_err(&h.send_ixs(&ixs, &[&user.kp]));
}

#[test]
fn quote_signed_by_the_wrong_key_fails() {
    let (mut h, user, q) = setup();
    let exp = h.now() + 45;
    let res = h.open_with(&user, 1, &q, q.odds_offered_u, exp, false, &Keypair::new());
    assert_err(&res, SchedinaError::WrongQuoteSigner);
}

#[test]
fn desk_signed_bytes_that_are_not_a_quote_fail() {
    let (mut h, user, q) = setup();
    let s = h.quote_signer.insecure_clone();
    let exp = h.now() + 45;
    // Trailing byte after a valid quote.
    let mut msg = borsh(&q);
    msg.push(0);
    let ixs = h.open_ixs(&user, 1, &msg, &s, q.odds_offered_u, exp, false);
    assert_err(
        &h.send_ixs(&ixs, &[&user.kp]),
        SchedinaError::QuoteMalformed,
    );
    // Truncated.
    let msg = borsh(&q);
    let ixs = h.open_ixs(
        &user,
        1,
        &msg[..msg.len() - 1],
        &s,
        q.odds_offered_u,
        exp,
        false,
    );
    assert_err(
        &h.send_ixs(&ixs, &[&user.kp]),
        SchedinaError::QuoteMalformed,
    );
    // A desk-signed cash-out presented as a quote: the tag differs.
    let t = h.mint(&user, 2, legs_at_half(1), 10 * USDC);
    let co = borsh(&cashout_msg(&t, USDC, h.now()));
    let ixs = h.open_ixs(&user, 3, &co, &s, q.odds_offered_u, exp, false);
    let res = h.send_ixs(&ixs, &[&user.kp]);
    assert!(
        custom_code(&res) == u32::from(SchedinaError::QuoteMalformed)
            || custom_code(&res) == u32::from(SchedinaError::BadTag)
    );
}

#[test]
fn cash_out_signature_over_other_bytes_fails() {
    let (mut h, user, q) = setup();
    let t = h.mint(&user, 1, legs_at_half(2), 10 * USDC);
    // The desk signed C = 1 USDC; the user submits C = 30 USDC.
    let signed = borsh(&cashout_msg(&t, USDC, h.now()));
    let sig = sign(&h.quote_signer, &signed);
    let ixs = [
        ed25519_ix(&signed, &sig, &h.quote_signer.pubkey()),
        h.ix_cash_out(
            &user.key(),
            &t,
            &user.usdc,
            cashout_msg(&t, 30 * USDC, h.now()),
        ),
    ];
    assert_err(&h.send_ixs(&ixs, &[&user.kp]), SchedinaError::QuoteMismatch);
    // A quote signature presented to cash_out.
    let qb = borsh(&q);
    let sig = sign(&h.quote_signer, &qb);
    let ixs = [
        ed25519_ix(&qb, &sig, &h.quote_signer.pubkey()),
        h.ix_cash_out(&user.key(), &t, &user.usdc, cashout_msg(&t, USDC, h.now())),
    ];
    assert_err(&h.send_ixs(&ixs, &[&user.kp]), SchedinaError::QuoteMismatch);
}

#[test]
fn tag_and_program_id_are_bound() {
    let (mut h, user, q) = setup();
    let signer = h.quote_signer.insecure_clone();
    let exp = h.now() + 45;

    let mut bad = q.clone();
    bad.tag = *b"schedina/quote/v2";
    let res = h.open_with(&user, 1, &bad, q.odds_offered_u, exp, false, &signer);
    assert_err(&res, SchedinaError::BadTag);

    let mut bad = q.clone();
    bad.program_id = Keypair::new().pubkey();
    let res = h.open_with(&user, 1, &bad, q.odds_offered_u, exp, false, &signer);
    assert_err(&res, SchedinaError::WrongProgramId);

    let t = h.mint(&user, 2, legs_at_half(1), 10 * USDC);
    let mut co = cashout_msg(&t, USDC, h.now());
    co.tag = *b"schedina/cashout/v0";
    assert_ne!(co.tag, CASHOUT_TAG);
    assert_err(&h.cash_out_with(&user, &co, &signer), SchedinaError::BadTag);
    let mut co = cashout_msg(&t, USDC, h.now());
    co.program_id = Keypair::new().pubkey();
    assert_err(
        &h.cash_out_with(&user, &co, &signer),
        SchedinaError::WrongProgramId,
    );
}

/// Five legs, with a wallet's compute-budget instructions: every step fits the
/// default 200k CU per instruction.
#[test]
fn compute_units() {
    let (mut h, user, _) = setup();
    let q = quote(legs_at_half(5), 2 * USDC, h.now());
    let s = h.quote_signer.insecure_clone();
    let mut ixs = compute_budget_ixs(200_000, 1);
    ixs.extend(h.open_ixs(
        &user,
        1,
        &borsh(&q),
        &s,
        q.odds_offered_u,
        h.now() + 45,
        false,
    ));
    let open = ok(h.send_ixs(&ixs, &[&user.kp]));
    let fills = vec![500_000, 0, 500_000, 0, 500_000];
    let fin = ok(h.finalize(&user, 1, fills, 900_000));
    let t = ticket_pda(&user.key(), 1);
    let co = ok(h.cash_out(&user, &t, USDC));
    let cu = [
        open.compute_units_consumed,
        fin.compute_units_consumed,
        co.compute_units_consumed,
    ];
    println!("CU open_pending(5) / finalize(5) / cash_out: {cu:?}");
    assert!(cu.iter().all(|&c| c < 200_000), "{cu:?}");
}

/// Worst case: 5 legs, `copied_from` set (2.F), and a wallet's two compute-budget
/// instructions still fit the 1,232-byte packet.
#[test]
fn transaction_sizes() {
    let (h, user, _) = setup();
    let signer = &h.quote_signer;
    let mut sizes = vec![];
    for n in 1..=5u8 {
        let q = quote(legs_at_half(n), 10 * USDC, h.now());
        let msg = borsh(&q);
        assert_eq!(msg.len(), 77 + 85 * usize::from(n));
        let sig = sign(signer, &msg);
        let program_ix = h.ix_open_pending(
            &user,
            u64::MAX,
            q.odds_offered_u,
            h.now() + 45,
            true,
            Some(Keypair::new().pubkey()),
        );
        let mut ixs = compute_budget_ixs(200_000, 1);
        ixs.extend([ed25519_ix(&msg, &sig, &signer.pubkey()), program_ix]);
        sizes.push((n, tx_size(&ixs, &user.key(), 1)));
    }
    println!("(n, bytes) with 2 compute-budget ixs and copied_from: {sizes:?}");
    for &(n, size) in &sizes {
        assert!(size <= PACKET_DATA_SIZE, "{n} legs: {size} bytes");
    }
    let co = cashout_msg(&Keypair::new().pubkey(), 1, 0);
    let bytes = borsh(&co);
    assert_eq!(bytes.len(), 99);
    let sig = sign(signer, &bytes);
    let mut ixs = compute_budget_ixs(200_000, 1);
    ixs.extend([
        ed25519_ix(&bytes, &sig, &signer.pubkey()),
        h.ix_cash_out(&user.key(), &co.ticket, &user.usdc, co.clone()),
    ]);
    assert!(tx_size(&ixs, &user.key(), 1) <= PACKET_DATA_SIZE);
}
