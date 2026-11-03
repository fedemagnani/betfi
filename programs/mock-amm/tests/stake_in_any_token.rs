//! L4-3 on LiteSVM, against the real schedina program: `[swap, Ed25519 verify,
//! open_pending]` in one v0 transaction escrows exactly the quoted USDC; a swap below
//! `min_out` fails the whole transaction and leaves no `Pending`; the largest case
//! (5 legs, two compute-budget instructions) fits the 1,232-byte packet.

mod common;

use anchor_lang::solana_program::instruction::Instruction;
use common::*;
use mock_amm::error::AmmError;
use solana_signer::Signer;

fn compute_budget() -> Vec<Instruction> {
    let id = anchor_lang::prelude::pubkey!("ComputeBudget111111111111111111111111111111");
    let mut lim = vec![2u8];
    lim.extend_from_slice(&400_000u32.to_le_bytes());
    let mut price = vec![3u8];
    price.extend_from_slice(&1_000u64.to_le_bytes());
    vec![
        Instruction {
            program_id: id,
            accounts: vec![],
            data: lim,
        },
        Instruction {
            program_id: id,
            accounts: vec![],
            data: price,
        },
    ]
}

#[test]
fn a_mockbonk_stake_escrows_the_quoted_usdc_in_one_transaction() {
    let mut h = Amm::seeded();
    let bonk = h.bonk;
    let a = h.admin.pubkey();
    let lut = h.lookup_table(&h.lut_addresses(&bonk));
    h.send_admin(h.ix_set_lookup_table(&a, &lut.key)).unwrap();

    let user = h.wallet();
    let k = user.pubkey();
    let user_bonk = h.token_account(&bonk, &k, 0);
    h.send(
        &[h.ix_faucet(&k, &bonk, &user_bonk, &k, 10_000 * BONK)],
        &[&user],
    )
    .unwrap();
    let user_usdc = h.token_account(&usdc_mint(), &k, 0);

    // The desk's /swap-quote: 10,000 mockBONK → 9.930238 USDC, min_out 9.880586 (50 bps).
    let (rm, ru) = h.reserves(&bonk);
    let s = mock_amm::math::swap_out(10_000 * BONK, rm, ru, 50).unwrap();
    let min_out = mock_amm::math::min_out(s.amount_out, 50).unwrap();
    assert_eq!((s.amount_out, min_out), (9_930_238, 9_880_586));

    // A failed swap first (min_out above what the pool pays): nothing pending, nothing moved.
    let (_, verify) = h.signed_quote(3, s.amount_out + 1);
    let ixs = [
        h.ix_swap(
            &k,
            &bonk,
            &user_bonk,
            &user_usdc,
            true,
            10_000 * BONK,
            s.amount_out + 1,
        ),
        verify,
        h.ix_open_pending(&k, &user_usdc, 7, 1, h.now() + 45),
    ];
    let tx = h.v0_tx(&ixs, &[&user], &lut);
    let res = h.send_v0(tx);
    assert_eq!(
        custom_error(&res),
        Some((0, amm_err(AmmError::SlippageExceeded)))
    );
    assert!(!h.exists(&pending_pda(&k, 7)));
    assert_eq!(h.balance(&user_bonk), 10_000 * BONK);
    assert_eq!(h.balance(&user_usdc), 0);
    assert_eq!(h.balance(&escrow_usdc()), 0);

    // The real one: stake = min_out.
    let (q, verify) = h.signed_quote(3, min_out);
    let floor = q.odds_offered_u;
    let ixs = [
        h.ix_swap(
            &k,
            &bonk,
            &user_bonk,
            &user_usdc,
            true,
            10_000 * BONK,
            min_out,
        ),
        verify,
        h.ix_open_pending(&k, &user_usdc, 8, floor, h.now() + 45),
    ];
    let tx = h.v0_tx(&ixs, &[&user], &lut);
    // The verify instruction sits immediately before open_pending.
    let compiled = tx.message.instructions();
    let keys = tx.message.static_account_keys();
    let prog = |i: usize| keys[usize::from(compiled[i].program_id_index)];
    assert_eq!(
        [prog(0), prog(1), prog(2)],
        [mock_amm::ID, schedina::ED25519_PROGRAM_ID, schedina::ID]
    );
    h.send_v0(tx)
        .unwrap_or_else(|e| panic!("{:?}\n{:#?}", e.err, e.meta.logs));
    let p: schedina::Pending = h.read(&pending_pda(&k, 8));
    assert_eq!((p.user, p.stake_u, p.n), (k, min_out, 3));
    assert_eq!(h.balance(&escrow_usdc()), min_out);
    assert_eq!(h.balance(&user_usdc), s.amount_out - min_out);
    assert_eq!(h.balance(&user_bonk), 0);
    assert_eq!(h.reserves(&bonk), (rm + 10_000 * BONK, ru - s.amount_out));
}

#[test]
fn five_legs_plus_the_swap_fit_one_packet_with_the_lookup_table() {
    let mut h = Amm::seeded();
    let bonk = h.bonk;
    let lut = h.lookup_table(&h.lut_addresses(&bonk));
    let user = h.wallet();
    let k = user.pubkey();
    let user_bonk = h.token_account(&bonk, &k, 50_000 * BONK);
    let user_usdc = h.token_account(&usdc_mint(), &k, 0);
    let (rm, ru) = h.reserves(&bonk);
    let s = mock_amm::math::swap_out(50_000 * BONK, rm, ru, 50).unwrap();
    let min_out = mock_amm::math::min_out(s.amount_out, 50).unwrap();
    let (q, verify) = h.signed_quote(5, min_out);
    let mut ixs = compute_budget();
    ixs.push(h.ix_swap(
        &k,
        &bonk,
        &user_bonk,
        &user_usdc,
        true,
        50_000 * BONK,
        min_out,
    ));
    ixs.push(verify);
    ixs.push(h.ix_open_pending(&k, &user_usdc, 9, q.odds_offered_u, h.now() + 45));
    let tx = h.v0_tx(&ixs, &[&user], &lut);
    let size = wire_size(&tx);
    println!("5-leg swap + open_pending, 2 compute-budget ixs, v0 + lookup table: {size} bytes");
    assert!(size <= PACKET_DATA_SIZE, "{size} bytes");
    h.send_v0(tx)
        .unwrap_or_else(|e| panic!("{:?}\n{:#?}", e.err, e.meta.logs));
    assert_eq!(h.balance(&escrow_usdc()), min_out);
    let p: schedina::Pending = h.read(&pending_pda(&k, 9));
    assert_eq!(p.n, 5);

    // Without the lookup table the same transaction does not fit (why it is v0).
    let legacy = {
        let msg = solana_message::Message::new(&ixs, Some(&k));
        1 + 64 + msg.serialize().len()
    };
    println!("same as a legacy transaction: {legacy} bytes");
    assert!(legacy > PACKET_DATA_SIZE);
}
