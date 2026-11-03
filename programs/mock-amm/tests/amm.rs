//! Mock AMM on LiteSVM: x·y = k against the hand-computed vectors of
//! `fixtures/swap/vectors.json`, slippage refusal, fee accounting, no way to drain a pool,
//! admin-only set-up, and the faucet's limits.

mod common;

use anchor_lang::prelude::Pubkey;
use common::*;
use mock_amm::error::AmmError;
use solana_keypair::Keypair;
use solana_signer::Signer;

fn u(v: &serde_json::Value, k: &str) -> u64 {
    v[k].as_u64().unwrap_or_else(|| panic!("{k}"))
}

#[test]
fn swaps_match_the_hand_computed_vectors() {
    let mut h = Amm::new();
    let v = vectors();
    let fee_bps = u16::try_from(u(&v, "fee_bps")).unwrap();
    assert_eq!(fee_bps, mock_amm::FEE_BPS);
    assert_eq!(h.pool(&h.bonk).fee_bps, fee_bps);
    for c in v["swaps"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let sell = c["direction"] == "sell";
        let (r_in, r_out, a, out) = (
            u(c, "r_in"),
            u(c, "r_out"),
            u(c, "amount_in"),
            u(c, "amount_out"),
        );
        // Host math first, then the program on reserves set to the vector's.
        let m = mock_amm::math::swap_out(a, r_in, r_out, fee_bps);
        if out == 0 {
            assert_eq!(m, None, "{name}");
        } else {
            let m = m.unwrap();
            assert_eq!(
                (m.fee_in, m.amount_out, m.gross_out),
                (u(c, "fee_in"), out, u(c, "gross_out")),
                "{name}"
            );
            assert_eq!(m.gross_out - m.amount_out, u(c, "fee_out"), "{name}");
            assert_eq!(
                mock_amm::math::min_out(out, 50),
                Some(u(c, "min_out")),
                "{name}"
            );
        }
        let bonk = h.bonk;
        let (rm, ru) = if sell { (r_in, r_out) } else { (r_out, r_in) };
        h.set_reserves(&bonk, rm, ru);
        let user = h.wallet();
        let (in_mint, out_mint) = if sell {
            (bonk, usdc_mint())
        } else {
            (usdc_mint(), bonk)
        };
        let src = h.token_account(&in_mint, &user.pubkey(), a);
        let dst = h.token_account(&out_mint, &user.pubkey(), 0);
        let before = h.pool(&bonk);
        let res = h.send(
            &[h.ix_swap(&user.pubkey(), &bonk, &src, &dst, sell, a, 0)],
            &[&user],
        );
        if out == 0 {
            assert_eq!(
                custom_error(&res),
                Some((0, amm_err(AmmError::ZeroOut))),
                "{name}"
            );
            assert_eq!(h.balance(&src), a, "{name}: nothing taken");
            continue;
        }
        res.unwrap_or_else(|e| panic!("{name}: {:?}", e.err));
        assert_eq!(h.balance(&dst), out, "{name}: paid out");
        assert_eq!(h.balance(&src), 0, "{name}: all of amount_in taken");
        let (nm, nu) = h.reserves(&bonk);
        let (n_in, n_out) = if sell { (nm, nu) } else { (nu, nm) };
        assert_eq!((n_in, n_out), (r_in + a, r_out - out), "{name}: reserves");
        let after = h.pool(&bonk);
        let fee = u(c, "fee_in");
        if sell {
            assert_eq!(after.fees_mint, before.fees_mint + fee, "{name}");
            assert_eq!(after.fees_usdc, before.fees_usdc, "{name}");
        } else {
            assert_eq!(after.fees_usdc, before.fees_usdc + fee, "{name}");
            assert_eq!(after.fees_mint, before.fees_mint, "{name}");
        }
        assert_eq!(after.swaps, before.swaps + 1);
    }
}

#[test]
fn refuses_below_min_out_and_takes_nothing() {
    let mut h = Amm::seeded();
    let bonk = h.bonk;
    let user = h.wallet();
    let src = h.token_account(&bonk, &user.pubkey(), 10_000 * BONK);
    let dst = h.token_account(&usdc_mint(), &user.pubkey(), 0);
    let r0 = h.reserves(&bonk);
    // The first vector: 10,000 mockBONK into 5M:5,000 pays 9,930,238.
    let res = h.send(
        &[h.ix_swap(
            &user.pubkey(),
            &bonk,
            &src,
            &dst,
            true,
            10_000 * BONK,
            9_930_239,
        )],
        &[&user],
    );
    assert_eq!(
        custom_error(&res),
        Some((0, amm_err(AmmError::SlippageExceeded)))
    );
    assert_eq!(h.balance(&src), 10_000 * BONK);
    assert_eq!(h.balance(&dst), 0);
    assert_eq!(h.reserves(&bonk), r0);
    assert_eq!(h.pool(&bonk).swaps, 0);
    // Exactly min_out passes.
    h.send(
        &[h.ix_swap(
            &user.pubkey(),
            &bonk,
            &src,
            &dst,
            true,
            10_000 * BONK,
            9_930_238,
        )],
        &[&user],
    )
    .unwrap();
    assert_eq!(h.balance(&dst), 9_930_238);
}

#[test]
fn fees_accumulate_in_the_pool_and_k_grows() {
    let mut h = Amm::seeded();
    let bonk = h.bonk;
    let user = h.wallet();
    let bonk_acc = h.token_account(&bonk, &user.pubkey(), 1_000_000 * BONK);
    let usdc_acc = h.token_account(&usdc_mint(), &user.pubkey(), 2_000 * USDC);
    let (mut fees_mint, mut fees_usdc) = (0u64, 0u64);
    let mut k = {
        let (a, b) = h.reserves(&bonk);
        u128::from(a) * u128::from(b)
    };
    for (i, (sell, amount)) in [
        (true, 12_345 * BONK),
        (false, 17 * USDC),
        (true, 100_000 * BONK + 3),
        (false, 999_999),
        (true, 70_000),
        (false, 250 * USDC),
    ]
    .into_iter()
    .enumerate()
    {
        let (rm, ru) = h.reserves(&bonk);
        let (r_in, r_out) = if sell { (rm, ru) } else { (ru, rm) };
        let want = mock_amm::math::swap_out(amount, r_in, r_out, 50).unwrap();
        let (src, dst) = if sell {
            (bonk_acc, usdc_acc)
        } else {
            (usdc_acc, bonk_acc)
        };
        let (s0, d0) = (h.balance(&src), h.balance(&dst));
        h.send(
            &[h.ix_swap(
                &user.pubkey(),
                &bonk,
                &src,
                &dst,
                sell,
                amount,
                want.amount_out,
            )],
            &[&user],
        )
        .unwrap_or_else(|e| panic!("swap {i}: {:?}", e.err));
        assert_eq!(h.balance(&src), s0 - amount);
        assert_eq!(h.balance(&dst), d0 + want.amount_out);
        if sell {
            fees_mint += want.fee_in;
        } else {
            fees_usdc += want.fee_in;
        }
        let (a, b) = h.reserves(&bonk);
        let k2 = u128::from(a) * u128::from(b);
        assert!(k2 > k, "swap {i}: k must grow ({k} -> {k2})");
        k = k2;
    }
    let p = h.pool(&bonk);
    assert_eq!(
        (p.fees_mint, p.fees_usdc, p.swaps),
        (fees_mint, fees_usdc, 6)
    );
    // Value conservation: pool + user hold exactly what they started with, per mint.
    let (rm, ru) = h.reserves(&bonk);
    assert_eq!(
        rm + h.balance(&bonk_acc),
        5_000_000 * BONK + 1_000_000 * BONK
    );
    assert_eq!(ru + h.balance(&usdc_acc), 5_000 * USDC + 2_000 * USDC);
}

#[test]
fn a_round_trip_loses_money_so_nothing_drains_the_pool() {
    let mut h = Amm::seeded();
    let bonk = h.bonk;
    let user = h.wallet();
    let usdc_acc = h.token_account(&usdc_mint(), &user.pubkey(), 1_000 * USDC);
    let bonk_acc = h.token_account(&bonk, &user.pubkey(), 0);
    for _ in 0..5 {
        let all = h.balance(&usdc_acc);
        h.send(
            &[h.ix_swap(&user.pubkey(), &bonk, &usdc_acc, &bonk_acc, false, all, 1)],
            &[&user],
        )
        .unwrap();
        let all = h.balance(&bonk_acc);
        h.send(
            &[h.ix_swap(&user.pubkey(), &bonk, &bonk_acc, &usdc_acc, true, all, 1)],
            &[&user],
        )
        .unwrap();
    }
    assert!(
        h.balance(&usdc_acc) < 1_000 * USDC * 96 / 100,
        "≥ 0.5% per leg lost"
    );
    assert_eq!(h.balance(&bonk_acc), 0);
    let (rm, ru) = h.reserves(&bonk);
    assert_eq!(rm, 5_000_000 * BONK);
    assert!(ru > 5_000 * USDC);

    // Dumping a huge amount still leaves the USDC side non-empty.
    let whale = h.wallet();
    let big = h.token_account(&bonk, &whale.pubkey(), 1_000_000_000_000 * BONK);
    let out = h.token_account(&usdc_mint(), &whale.pubkey(), 0);
    h.send(
        &[h.ix_swap(
            &whale.pubkey(),
            &bonk,
            &big,
            &out,
            true,
            1_000_000_000_000 * BONK,
            1,
        )],
        &[&whale],
    )
    .unwrap();
    let (_, ru2) = h.reserves(&bonk);
    assert!(ru2 > 0 && h.balance(&out) < ru);
}

#[test]
fn foreign_or_mixed_vaults_are_refused() {
    let mut h = Amm::seeded();
    let (bonk, sol) = (h.bonk, h.sol);
    let user = h.wallet();
    let src = h.token_account(&bonk, &user.pubkey(), 1_000 * BONK);
    let dst = h.token_account(&usdc_mint(), &user.pubkey(), 0);
    let attacker_usdc = h.token_account(&usdc_mint(), &user.pubkey(), 1_000 * USDC);
    let (pb, ps) = (pool(&bonk), pool(&sol));
    let good = h.ix_swap(&user.pubkey(), &bonk, &src, &dst, true, 1_000 * BONK, 1);

    // vault_out of another pool (the mockSOL pool's USDC) under the mockBONK pool.
    let mut bad = good.clone();
    bad.accounts[5].pubkey = vault(&ps, &usdc_mint());
    let res = h.send(&[bad], &[&user]);
    assert_eq!(custom_error(&res), Some((0, amm_err(AmmError::WrongVault))));

    // An attacker-owned USDC account posing as the pool's USDC vault.
    let mut bad = good.clone();
    bad.accounts[5].pubkey = attacker_usdc;
    let res = h.send(&[bad], &[&user]);
    assert_eq!(custom_error(&res), Some((0, amm_err(AmmError::WrongVault))));

    // The mockSOL pool with the mockBONK vaults.
    let mut bad = good.clone();
    bad.accounts[1].pubkey = ps;
    let res = h.send(&[bad], &[&user]);
    assert_eq!(custom_error(&res), Some((0, amm_err(AmmError::WrongVault))));

    // A fake pool account (right layout, not owned by the program).
    let mut fake = h.svm.get_account(&pb).unwrap();
    fake.owner = user.pubkey();
    let fake_key = Pubkey::new_unique();
    h.svm.set_account(fake_key, fake).unwrap();
    let mut bad = good.clone();
    bad.accounts[1].pubkey = fake_key;
    assert!(h.send(&[bad], &[&user]).is_err());

    // Output account of the wrong mint (the USDC vault paying into a mockBONK account).
    let other_bonk = h.token_account(&bonk, &user.pubkey(), 0);
    let mut bad = good.clone();
    bad.accounts[3].pubkey = other_bonk;
    let res = h.send(&[bad], &[&user]);
    assert_eq!(
        custom_error(&res),
        Some((0, amm_err(AmmError::MintMismatch)))
    );

    // Someone else's token account as the source: SPL Token refuses the transfer.
    let victim = h.wallet();
    let victim_bonk = h.token_account(&bonk, &victim.pubkey(), 1_000 * BONK);
    let mut bad = good.clone();
    bad.accounts[2].pubkey = victim_bonk;
    assert!(h.send(&[bad], &[&user]).is_err());
    assert_eq!(h.balance(&victim_bonk), 1_000 * BONK);

    // Zero in.
    let res = h.send(
        &[h.ix_swap(&user.pubkey(), &bonk, &src, &dst, true, 0, 0)],
        &[&user],
    );
    assert_eq!(custom_error(&res), Some((0, amm_err(AmmError::ZeroAmount))));

    // Nothing moved; the honest swap still works.
    assert_eq!(h.balance(&src), 1_000 * BONK);
    h.send(&[good], &[&user]).unwrap();
}

#[test]
fn liquidity_keeps_the_price_and_set_up_is_admin_only() {
    let mut h = Amm::new();
    let (bonk, sol) = (h.bonk, h.sol);
    let mallory = h.wallet();
    let mk = mallory.pubkey();
    let m_bonk = h.token_account(&bonk, &mk, 1_000_000 * BONK);
    let m_usdc = h.token_account(&usdc_mint(), &mk, 10_000 * USDC);

    // The first deposit sets the price: admin only.
    let res = h.send(
        &[h.ix_add_liquidity(&mk, &bonk, &m_bonk, &m_usdc, 1_000 * BONK, USDC)],
        &[&mallory],
    );
    assert_eq!(custom_error(&res), Some((0, amm_err(AmmError::NotAdmin))));
    h.seed(&bonk, 5_000_000 * BONK, 5_000 * USDC);

    // Later deposits pay ceil(mint × r_usdc / r_mint) USDC.
    let res = h.send(
        &[h.ix_add_liquidity(&mk, &bonk, &m_bonk, &m_usdc, 333_333, 3_333)],
        &[&mallory],
    );
    assert_eq!(
        custom_error(&res),
        Some((0, amm_err(AmmError::DepositSlippage)))
    );
    h.send(
        &[h.ix_add_liquidity(&mk, &bonk, &m_bonk, &m_usdc, 333_333, 3_334)],
        &[&mallory],
    )
    .unwrap();
    assert_eq!(
        h.reserves(&bonk),
        (5_000_000 * BONK + 333_333, 5_000 * USDC + 3_334)
    );
    assert_eq!(h.balance(&m_usdc), 10_000 * USDC - 3_334);

    // Admin-only set-up.
    let res = h.send(&[h.ix_init_mock_mint(&mk, "WIF", 6, 1)], &[&mallory]);
    assert_eq!(custom_error(&res), Some((0, amm_err(AmmError::NotAdmin))));
    let lut = h.lookup_table(&[bonk]);
    let res = h.send(&[h.ix_set_lookup_table(&mk, &lut.key)], &[&mallory]);
    assert_eq!(custom_error(&res), Some((0, amm_err(AmmError::NotAdmin))));
    let a = h.admin.pubkey();
    // Not a lookup table.
    let res = h.send_admin(h.ix_set_lookup_table(&a, &bonk));
    assert_eq!(
        custom_error(&res),
        Some((0, amm_err(AmmError::BadLookupTable)))
    );
    h.send_admin(h.ix_set_lookup_table(&a, &lut.key)).unwrap();
    let cfg: mock_amm::AmmConfig = h.read(&amm_config());
    assert_eq!(
        (cfg.admin, cfg.usdc_mint, cfg.lookup_table),
        (a, usdc_mint(), lut.key)
    );
    // A second init_amm or init_pool fails (accounts exist); init_amm by a non-authority too.
    assert!(h.send_admin(h.ix_init_amm(&a, usdc_mint())).is_err());
    assert!(h.send_admin(h.ix_init_pool(&a, &sol)).is_err());
    let res = h.send(&[h.ix_init_amm(&mk, bonk)], &[&mallory]);
    assert!(res.is_err());
    // Bad symbols.
    for s in ["", "bonk", "TOOLONGXX", "B-K"] {
        assert!(
            h.send_admin(h.ix_init_mock_mint(&a, s, 5, 1)).is_err(),
            "{s:?}"
        );
    }
    let res = h.send_admin(h.ix_init_mock_mint(&a, "WIF", 10, 1));
    assert_eq!(
        custom_error(&res),
        Some((0, amm_err(AmmError::BadDecimals)))
    );
    let t = h.mock_token(&bonk);
    assert_eq!(
        (t.symbol.as_str(), t.decimals, t.faucet_max),
        ("BONK", 5, 100_000 * BONK)
    );
}

/// L8: the desk's sponsor key pays the fee and the `FaucetClaim` rent; the claim (and its
/// cooldown) is still the caller's, who holds no SOL.
#[test]
fn a_sponsored_faucet_claim_is_the_callers() {
    let mut h = Amm::new();
    let bonk = h.bonk;
    let user = Keypair::new();
    let k = user.pubkey();
    let acc = h.token_account(&bonk, &k, 0);
    let sponsor = h.wallet();
    let s = sponsor.pubkey();
    h.send(
        &[h.ix_faucet_paid_by(&k, &s, &bonk, &acc, BONK)],
        &[&sponsor, &user],
    )
    .unwrap();
    assert_eq!(h.balance(&acc), BONK);
    assert!(h.svm.get_account(&k).is_none_or(|a| a.lamports == 0));
    let claim = h.svm.get_account(&faucet_claim(&bonk, &k)).expect("claim");
    assert!(claim.lamports > 0);
    let res = h.send(
        &[h.ix_faucet_paid_by(&k, &s, &bonk, &acc, BONK)],
        &[&sponsor, &user],
    );
    assert_eq!(
        custom_error(&res),
        Some((0, amm_err(AmmError::FaucetCooldown)))
    );
}

#[test]
fn faucet_is_capped_and_hourly_and_the_admin_is_exempt() {
    let mut h = Amm::new();
    let bonk = h.bonk;
    let user = h.wallet();
    let k = user.pubkey();
    let acc = h.token_account(&bonk, &k, 0);
    let cap = 100_000 * BONK;
    let res = h.send(&[h.ix_faucet(&k, &bonk, &acc, &k, cap + 1)], &[&user]);
    assert_eq!(
        custom_error(&res),
        Some((0, amm_err(AmmError::FaucetAmount)))
    );
    let res = h.send(&[h.ix_faucet(&k, &bonk, &acc, &k, 0)], &[&user]);
    assert_eq!(custom_error(&res), Some((0, amm_err(AmmError::ZeroAmount))));
    h.send(&[h.ix_faucet(&k, &bonk, &acc, &k, cap)], &[&user])
        .unwrap();
    assert_eq!(h.balance(&acc), cap);
    // Again within the hour, into any account: refused (the claim is per mint and signer).
    let acc2 = h.token_account(&bonk, &k, 0);
    let res = h.send(&[h.ix_faucet(&k, &bonk, &acc2, &k, 1)], &[&user]);
    assert_eq!(
        custom_error(&res),
        Some((0, amm_err(AmmError::FaucetCooldown)))
    );
    // Someone else minting to this user spends their own claim, not the user's.
    let other = h.wallet();
    h.send(
        &[h.ix_faucet(&other.pubkey(), &bonk, &acc2, &k, 1)],
        &[&other],
    )
    .unwrap();
    let res = h.send(
        &[h.ix_faucet(&other.pubkey(), &bonk, &acc2, &k, 1)],
        &[&other],
    );
    assert_eq!(
        custom_error(&res),
        Some((0, amm_err(AmmError::FaucetCooldown)))
    );
    let fresh = h.wallet();
    let fresh_acc = h.token_account(&bonk, &fresh.pubkey(), 0);
    h.send(
        &[h.ix_faucet(&fresh.pubkey(), &bonk, &fresh_acc, &fresh.pubkey(), cap)],
        &[&fresh],
    )
    .unwrap();
    // Another mint is a separate claim.
    let sol = h.sol;
    let sol_acc = h.token_account(&sol, &k, 0);
    h.send(&[h.ix_faucet(&k, &sol, &sol_acc, &k, 10 * SOL)], &[&user])
        .unwrap();
    // A claim record for one owner cannot be passed for another (PDA seeds).
    let other_bonk = h.token_account(&bonk, &other.pubkey(), 0);
    let mut bad = h.ix_faucet(&other.pubkey(), &bonk, &other_bonk, &other.pubkey(), 1);
    bad.accounts[5].pubkey = faucet_claim(&bonk, &Pubkey::new_unique());
    assert!(h.send(&[bad], &[&other]).is_err());
    // The admin mints any amount, any time, and does not move the user's cooldown.
    let a = h.admin.pubkey();
    h.send_admin(h.ix_faucet(&a, &bonk, &acc, &k, 10 * cap))
        .unwrap();
    h.send_admin(h.ix_faucet(&a, &bonk, &acc, &k, 10 * cap))
        .unwrap();
    assert_eq!(h.balance(&acc), 21 * cap);
    h.warp(3_599);
    let res = h.send(&[h.ix_faucet(&k, &bonk, &acc, &k, 1)], &[&user]);
    assert_eq!(
        custom_error(&res),
        Some((0, amm_err(AmmError::FaucetCooldown)))
    );
    h.warp(1);
    h.send(&[h.ix_faucet(&k, &bonk, &acc, &k, 1)], &[&user])
        .unwrap();
    // Not a mock mint: the USDC mint has no MockToken.
    let usdc_acc = h.token_account(&usdc_mint(), &k, 0);
    h.warp(3_600);
    assert!(h
        .send(&[h.ix_faucet(&k, &usdc_mint(), &usdc_acc, &k, 1)], &[&user])
        .is_err());
}
