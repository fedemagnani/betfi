//! `finalize(fills, r_ppm)`: crank only. Mints the `Ticket` from a `Pending` at the
//! achieved prices (C2): `rS` moves from escrow to the vault, `S - rS` back to the user,
//! and the payout `P` is locked. `fills[i]` is the average fill price of leg `i`, or 0
//! for a leg left unhedged, which then uses its signed quote price. The `Pending` is
//! closed (rent to its user); the crank pays the `Ticket` rent.
//!
//! Step 2.F, through `remaining_accounts` only (L2-4; the declared list is 1.C's):
//! - own tip (`pending.is_tip`): `[tipster_profile (w)]`, PDA `["tipster", user]`,
//!   created here if absent (the crank pays); the ticket joins its `open_*` fields.
//! - copy (`pending.copied_from = Some(orig)`): `[orig_ticket (r), tipster_usdc (w)]`.
//!   The original must be an open own tip with every leg still unresolved (in play is
//!   fine, D-660) and the same legs (symbol, game id, side); the ticket's tipster is the
//!   original's user. Of the escrowed copy fee, `used = floor(copy_fee × r)` is charged: the tipster's share
//!   (`tipster_share_bps`, rounded down) goes to `tipster_usdc`, the rest to the vault
//!   (`Vault.free`); `copy_fee - used` is refunded with `S - rS`. Emits `TipsterPaid`.

use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token, TokenAccount};

use crate::constants::*;
use crate::error::SchedinaError;
use crate::events::{TicketMinted, TipsterPaid};
use crate::math;
use crate::state::{Config, LegStatus, Pending, Ticket, TicketLeg, TicketState, Vault};
use crate::tipster;
use crate::vault::{pay_from_program, pay_from_program_to};

#[event_cpi]
#[derive(Accounts)]
pub struct Finalize<'info> {
    #[account(mut)]
    pub crank: Signer<'info>,

    #[account(
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = crank @ SchedinaError::NotCrank,
        has_one = usdc_mint,
    )]
    pub config: Box<Account<'info, Config>>,

    #[account(mut, seeds = [VAULT_SEED], bump = vault.bump)]
    pub vault: Box<Account<'info, Vault>>,

    #[account(
        mut,
        close = rent_payer,
        seeds = [PENDING_SEED, pending.user.as_ref(), &pending.nonce.to_le_bytes()],
        bump = pending.bump,
    )]
    pub pending: Box<Account<'info, Pending>>,

    #[account(
        init,
        payer = crank,
        space = 8 + Ticket::INIT_SPACE,
        seeds = [TICKET_SEED, pending.user.as_ref(), &pending.nonce.to_le_bytes()],
        bump,
    )]
    pub ticket: Box<Account<'info, Ticket>>,

    /// CHECK: receives the `Pending` rent: whoever paid it (L8), address-checked against
    /// `pending.rent_receiver()`; any owner (a smart-wallet user must still get the refund).
    #[account(mut, address = pending.rent_receiver())]
    pub rent_payer: UncheckedAccount<'info>,

    /// Receives the refund `S - rS`; any USDC account owned by the pending's user.
    #[account(mut, token::mint = usdc_mint, token::authority = pending.user)]
    pub user_usdc: Box<Account<'info, TokenAccount>>,

    pub usdc_mint: Box<Account<'info, Mint>>,

    #[account(mut, address = vault.escrow_ata)]
    pub escrow_usdc: Box<Account<'info, TokenAccount>>,

    #[account(mut, address = vault.usdc_ata)]
    pub vault_usdc: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

pub fn handler<'info>(
    mut ctx: Context<'info, Finalize<'info>>,
    fills: Vec<u64>,
    r_ppm: u32,
) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let a = &mut ctx.accounts;
    require!(!a.config.paused, SchedinaError::Paused);
    let p = &a.pending;
    require!(now < p.expires_at, SchedinaError::PendingExpired);
    let n = usize::from(p.n);
    require!(fills.len() == n, SchedinaError::FillCount);
    require!(
        r_ppm > 0 && u64::from(r_ppm) <= ONE,
        SchedinaError::BadFillRatio
    );

    let mut legs = [TicketLeg::default(); MAX_LEGS];
    for (i, (q, &fill)) in p.legs[..n].iter().zip(fills.iter()).enumerate() {
        let f = if fill == 0 { q.price_u } else { fill };
        require!(f > 0 && f <= ONE, SchedinaError::BadFillPrice);
        legs[i] = TicketLeg {
            symbol_hash: q.symbol_hash,
            game_id_hash: q.game_id_hash,
            start_ts: q.start_ts,
            side: q.side,
            price_u: q.price_u,
            fill_u: f,
            m1_ppm: q.m1_ppm,
            hedged: fill != 0,
            status: LegStatus::Open,
            resolution_ppm: 0,
        };
    }

    let rs_u = math::rs(p.stake_u, r_ppm).ok_or(SchedinaError::MathOverflow)?;
    require!(rs_u > 0, SchedinaError::BadFillRatio);
    let payout_u = math::payout(rs_u, legs[..n].iter().map(|l| (l.m1_ppm, l.fill_u)))
        .ok_or(SchedinaError::MathOverflow)?;
    require!(
        math::floor_ok(payout_u, rs_u, p.min_payout_per_unit_u),
        SchedinaError::BelowFloor
    );
    require!(
        payout_u <= a.config.max_payout,
        SchedinaError::PayoutTooLarge
    );
    let odds_u = math::odds(payout_u, rs_u).ok_or(SchedinaError::MathOverflow)?;
    let fee_u =
        math::fee(rs_u, legs[..n].iter().map(|l| l.m1_ppm)).ok_or(SchedinaError::MathOverflow)?;
    // Copy: check the original, split the used part of the fee (0 for other tickets).
    let copy = match p.copied_from {
        Some(orig) => Some(tipster::check_copy(
            ctx.remaining_accounts,
            p,
            &orig,
            &a.config.usdc_mint,
        )?),
        None => None,
    };
    let (used_fee_u, tipster_fee_u, house_fee_u) =
        math::copy_split(p.copy_fee_u, r_ppm, a.config.tipster_share_bps)
            .ok_or(SchedinaError::MathOverflow)?;
    // rs_u <= stake_u and used_fee_u <= copy_fee_u because r_ppm <= 1e6 (C2: refund_u
    // includes the unspent copy fee).
    let refund_u = (p.stake_u - rs_u)
        .checked_add(p.copy_fee_u - used_fee_u)
        .ok_or(SchedinaError::MathOverflow)?;

    // The stake joins the house money, then the full payout is locked out of it.
    let free = a
        .vault
        .free
        .checked_add(rs_u)
        .ok_or(SchedinaError::MathOverflow)?;
    require!(free >= payout_u, SchedinaError::VaultShort);
    a.vault.free = free - payout_u;
    a.vault.locked = a
        .vault
        .locked
        .checked_add(payout_u)
        .ok_or(SchedinaError::MathOverflow)?;
    // The house's share of the copy fee is house money.
    a.vault.free = a
        .vault
        .free
        .checked_add(house_fee_u)
        .ok_or(SchedinaError::MathOverflow)?;

    pay_from_program(
        &a.token_program,
        &a.usdc_mint,
        &a.config,
        &a.escrow_usdc,
        &a.vault_usdc,
        rs_u.checked_add(house_fee_u)
            .ok_or(SchedinaError::MathOverflow)?,
    )?;
    let ticket_tipster = match &copy {
        Some((tipster_key, tipster_usdc)) => {
            pay_from_program_to(
                &a.token_program,
                &a.usdc_mint,
                &a.config,
                &a.escrow_usdc,
                tipster_usdc.clone(),
                tipster_fee_u,
            )?;
            Some(*tipster_key)
        }
        None => a.pending.tipster,
    };
    pay_from_program(
        &a.token_program,
        &a.usdc_mint,
        &a.config,
        &a.escrow_usdc,
        &a.user_usdc,
        refund_u,
    )?;

    if a.pending.is_tip {
        // Own tip: the ticket joins the tipster's open fields.
        let (ai, profile) = tipster::profile_for_finalize(
            ctx.remaining_accounts,
            &a.pending.user,
            &a.crank.to_account_info(),
            &a.system_program.to_account_info(),
        )?;
        tipster::record_open(&ai, profile, rs_u)?;
    }

    let p = &a.pending;
    let t = &mut a.ticket;
    t.user = p.user;
    t.nonce = p.nonce;
    t.n = p.n;
    t.legs = legs;
    t.stake_u = rs_u;
    t.refund_u = refund_u;
    t.payout_u = payout_u;
    t.odds_u = odds_u;
    t.odds_offered_u = p.odds_offered_u;
    t.min_payout_per_unit_u = p.min_payout_per_unit_u;
    t.r_ppm = r_ppm;
    t.fee_u = fee_u;
    t.copy_fee_u = used_fee_u;
    t.state = TicketState::Open;
    t.tipster = ticket_tipster;
    t.copied_from = p.copied_from;
    t.quote_hash = p.quote_hash;
    t.minted_at = now;
    t.resolved_at = 0;
    t.paid_u = 0;
    t.bump = ctx.bumps.ticket;

    let event = TicketMinted {
        ticket: t.key(),
        pending: p.key(),
        user: p.user,
        nonce: p.nonce,
        fills,
        r_ppm,
        stake_u: rs_u,
        refund_u,
        payout_u,
        odds_u,
        fee_u,
        tipster: ticket_tipster,
        copied_from: p.copied_from,
        vault_locked: a.vault.locked,
        vault_free: a.vault.free,
    };
    let paid = copy.map(|(tipster, _)| TipsterPaid {
        tipster,
        ticket: event.ticket,
        amount_u: tipster_fee_u,
    });
    emit_cpi!(event);
    if let Some(paid) = paid {
        emit_cpi!(paid);
    }
    Ok(())
}
