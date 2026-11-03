//! `init_config`, `set_params`, `pause`, `fund_vault`, `withdraw_house`.

mod common;

use anchor_spl::token::{Mint, TokenAccount};
use common::*;
use schedina::error::SchedinaError;
use schedina::events::{ConfigUpdated, HouseWithdrawn, VaultFunded};
use solana_signer::Signer;

#[test]
fn init_config_creates_mint_vault_and_escrow() {
    let h = Harness::initialized();
    assert_eq!(h.now(), T0);
    let c = h.config();
    assert_eq!(c.authority, h.authority.pubkey());
    assert_eq!(c.crank, h.crank.pubkey());
    assert_eq!(c.quote_signer, h.quote_signer.pubkey());
    assert_eq!(c.usdc_mint, mint_pda());
    assert_eq!((c.max_legs, c.min_stake, c.reserve), (5, USDC, 100 * USDC));
    assert!(!c.paused);
    let m: Mint = h.read(&mint_pda());
    assert_eq!(m.decimals, 6);
    assert_eq!(Option::<_>::from(m.mint_authority), Some(config_pda()));
    let v = h.vault();
    assert_eq!((v.usdc_ata, v.escrow_ata), (vault_usdc(), escrow_usdc()));
    assert_eq!((v.locked, v.free, v.lp_shares), (0, 0, 0));
    for acc in [vault_usdc(), escrow_usdc()] {
        let t: TokenAccount = h.read(&acc);
        assert_eq!((t.mint, t.owner, t.amount), (mint_pda(), config_pda(), 0));
    }
}

#[test]
fn init_config_needs_the_upgrade_authority_and_runs_once() {
    let mut h = Harness::new();
    let stranger = h.wallet();
    let params = default_params(h.crank.pubkey(), h.quote_signer.pubkey());
    let ix = h.ix_init_config(&stranger.pubkey(), stranger.pubkey(), params.clone());
    let res = h.send(ix, &[&stranger]);
    assert_err(&res, SchedinaError::NotUpgradeAuthority);

    let mut bad = params.clone();
    bad.max_legs = 6;
    let ix = h.ix_init_config(&h.deployer.pubkey(), h.authority.pubkey(), bad);
    assert_err(&h.send_deployer(ix), SchedinaError::InvalidParams);

    let ix = h.ix_init_config(&h.deployer.pubkey(), h.authority.pubkey(), params.clone());
    ok(h.send_deployer(ix));
    h.svm.expire_blockhash();
    let ix = h.ix_init_config(&h.deployer.pubkey(), h.authority.pubkey(), params);
    assert!(h.send_deployer(ix).is_err(), "second init_config");
}

#[test]
fn set_params_is_authority_only_and_validated() {
    let mut h = Harness::initialized();
    let mut p = default_params(h.crank.pubkey(), h.quote_signer.pubkey());
    p.max_legs = 3;
    p.min_stake = 5 * USDC;

    let stranger = h.wallet();
    let ix = h.ix_set_params(&stranger.pubkey(), p.clone());
    assert_err(&h.send(ix, &[&stranger]), SchedinaError::NotAuthority);
    let ix = h.ix_set_params(&h.crank.pubkey(), p.clone());
    assert_err(&h.send_crank(ix), SchedinaError::NotAuthority);

    let ix = h.ix_set_params(&h.authority.pubkey(), p.clone());
    let meta = ok(h.send_authority(ix));
    let c = h.config();
    assert_eq!((c.max_legs, c.min_stake), (3, 5 * USDC));
    let ev = cpi_events::<ConfigUpdated>(&meta);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].max_legs, 3);

    for f in [
        |p: &mut schedina::Params| p.max_legs = 0,
        |p: &mut schedina::Params| p.max_legs = 6,
        |p: &mut schedina::Params| p.min_stake = 0,
        |p: &mut schedina::Params| p.copy_fee_bps = 10_001,
        |p: &mut schedina::Params| p.quote_signer = Default::default(),
    ] {
        let mut bad = p.clone();
        f(&mut bad);
        let ix = h.ix_set_params(&h.authority.pubkey(), bad);
        assert_err(&h.send_authority(ix), SchedinaError::InvalidParams);
    }
}

#[test]
fn pause_blocks_new_risk_only() {
    let mut h = Harness::ready();
    let user = h.user(100 * USDC);
    // A ticket minted before the pause.
    let t = h.mint(&user, 1, legs_at_half(1), 10 * USDC);
    h.open_default(&user, 2, legs_at_half(1), 10 * USDC);

    let stranger = h.wallet();
    let ix = h.ix_pause(&stranger.pubkey(), true);
    assert_err(&h.send(ix, &[&stranger]), SchedinaError::NotAuthority);
    let ix = h.ix_pause(&h.authority.pubkey(), true);
    let meta = ok(h.send_authority(ix));
    assert!(cpi_events::<ConfigUpdated>(&meta)[0].paused);
    assert!(h.config().paused);

    let q = quote(legs_at_half(1), 10 * USDC, h.now());
    let res = h.open(&user, 3, &q, q.odds_offered_u, h.now() + 45);
    assert_err(&res, SchedinaError::Paused);
    let res = h.finalize(&user, 2, vec![0], 1_000_000);
    assert_err(&res, SchedinaError::Paused);

    // Exits stay open: resolve, settle, cash-out, refund.
    ok(h.cash_out(&user, &t, 5 * USDC));
    h.warp(60);
    let ix = h.ix_refund_pending(&stranger.pubkey(), &user.key(), 2, &user.usdc);
    ok(h.send(ix, &[&stranger]));

    let ix = h.ix_pause(&h.authority.pubkey(), false);
    ok(h.send_authority(ix));
    let q = quote(legs_at_half(1), 10 * USDC, h.now());
    ok(h.open(&user, 3, &q, q.odds_offered_u, h.now() + 45));
}

#[test]
fn fund_vault_adds_to_free() {
    let mut h = Harness::initialized();
    let funder = h.user(1_000 * USDC);
    let meta = ok(h.fund_vault(&funder, 250 * USDC));
    assert_eq!(h.vault().free, 250 * USDC);
    assert_eq!(h.balance(&vault_usdc()), 250 * USDC);
    assert_eq!(h.balance(&funder.usdc), 750 * USDC);
    let ev = cpi_events::<VaultFunded>(&meta);
    assert_eq!((ev[0].amount_u, ev[0].vault_free), (250 * USDC, 250 * USDC));
    assert_err(&h.fund_vault(&funder, 0), SchedinaError::ZeroAmount);
}

#[test]
fn withdraw_house_cannot_cross_reserve_or_touch_locked() {
    let mut h = Harness::ready();
    let dest = h.token_account_for(&h.authority.pubkey());
    let reserve = h.config().reserve;

    // Lock a payout: locked money is not withdrawable either.
    let user = h.user(100 * USDC);
    h.mint(&user, 1, legs_at_half(3), 10 * USDC);
    let v = h.vault();
    assert_eq!(v.locked, 68_590_000);
    assert_eq!(v.free, HOUSE + 10 * USDC - 68_590_000);

    let max = v.free - reserve;
    let ix = h.ix_withdraw_house(&h.authority.pubkey(), &dest, max + 1);
    assert_err(&h.send_authority(ix), SchedinaError::BelowReserve);

    let stranger = h.user(0);
    let ix = h.ix_withdraw_house(&stranger.key(), &stranger.usdc, 1);
    assert_err(&h.send(ix, &[&stranger.kp]), SchedinaError::NotAuthority);
    // The authority cannot send house money to someone else's account.
    let ix = h.ix_withdraw_house(&h.authority.pubkey(), &stranger.usdc, 1);
    assert_anchor_err(
        &h.send_authority(ix),
        anchor_lang::error::ErrorCode::ConstraintTokenOwner,
    );

    let ix = h.ix_withdraw_house(&h.authority.pubkey(), &dest, max);
    let meta = ok(h.send_authority(ix));
    assert_eq!(h.vault().free, reserve);
    assert_eq!(h.balance(&dest), max);
    assert_eq!(cpi_events::<HouseWithdrawn>(&meta)[0].vault_free, reserve);

    let ix = h.ix_withdraw_house(&h.authority.pubkey(), &dest, 1);
    assert_err(&h.send_authority(ix), SchedinaError::BelowReserve);
}

#[test]
fn withdraw_house_with_free_below_reserve_fails() {
    let mut h = Harness::initialized();
    let funder = h.user(50 * USDC);
    ok(h.fund_vault(&funder, 50 * USDC)); // below the 100 USDC reserve
    let dest = h.token_account_for(&h.authority.pubkey());
    let ix = h.ix_withdraw_house(&h.authority.pubkey(), &dest, 1);
    assert_err(&h.send_authority(ix), SchedinaError::BelowReserve);
}
