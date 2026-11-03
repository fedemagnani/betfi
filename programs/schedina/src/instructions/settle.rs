//! `settle()`: anyone, once every leg is resolved. Pays `P × Π r̃` (C2, floored per
//! leg) from the vault to a USDC account of the ticket's user, returns the rest of
//! `P` to `Vault.free`, and marks the ticket `Settled`. An own tip (2.F) needs
//! `remaining_accounts = [tipster_profile (w)]` and moves to its `settled_*` fields.

use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token, TokenAccount};

use crate::constants::*;
use crate::error::SchedinaError;
use crate::events::TicketSettled;
use crate::math;
use crate::state::{Config, LegStatus, Ticket, TicketState, Vault};
use crate::tipster;
use crate::vault::pay_from_program;

#[event_cpi]
#[derive(Accounts)]
pub struct Settle<'info> {
    /// Anyone; pays the transaction fee only.
    pub caller: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump, has_one = usdc_mint)]
    pub config: Box<Account<'info, Config>>,

    #[account(mut, seeds = [VAULT_SEED], bump = vault.bump)]
    pub vault: Box<Account<'info, Vault>>,

    #[account(
        mut,
        seeds = [TICKET_SEED, ticket.user.as_ref(), &ticket.nonce.to_le_bytes()],
        bump = ticket.bump,
    )]
    pub ticket: Box<Account<'info, Ticket>>,

    #[account(mut, token::mint = usdc_mint, token::authority = ticket.user)]
    pub user_usdc: Box<Account<'info, TokenAccount>>,

    pub usdc_mint: Box<Account<'info, Mint>>,

    #[account(mut, address = vault.usdc_ata)]
    pub vault_usdc: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(mut ctx: Context<Settle>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let a = &mut ctx.accounts;
    let t = &a.ticket;
    require!(t.state == TicketState::Open, SchedinaError::TicketNotOpen);
    let legs = &t.legs[..usize::from(t.n)];
    require!(
        legs.iter().all(|l| l.status != LegStatus::Open),
        SchedinaError::LegsUnresolved
    );
    // An own tip cannot leave the record (L2-4): its profile is required.
    let profile = if tipster::is_own_tip(t.tipster, &t.user, t.copied_from) {
        Some(tipster::require_profile(ctx.remaining_accounts, &t.user)?)
    } else {
        None
    };
    let paid_u = math::settle(
        t.payout_u,
        legs.iter().map(|l| math::rtilde(l.side, l.resolution_ppm)),
    )
    .ok_or(SchedinaError::MathOverflow)?;
    // paid_u <= payout_u (every factor <= 1e6), and payout_u <= locked (invariant).
    let payout_u = t.payout_u;
    a.vault.locked = a
        .vault
        .locked
        .checked_sub(payout_u)
        .ok_or(SchedinaError::MathOverflow)?;
    a.vault.free = a
        .vault
        .free
        .checked_add(payout_u - paid_u)
        .ok_or(SchedinaError::MathOverflow)?;
    pay_from_program(
        &a.token_program,
        &a.usdc_mint,
        &a.config,
        &a.vault_usdc,
        &a.user_usdc,
        paid_u,
    )?;
    if let Some((ai, p)) = profile {
        tipster::record_settled(ai, p, a.ticket.stake_u, paid_u)?;
    }
    let t = &mut a.ticket;
    t.state = TicketState::Settled;
    t.paid_u = paid_u;
    t.resolved_at = now;
    let event = TicketSettled {
        ticket: t.key(),
        user: t.user,
        payout_u,
        paid_u,
        vault_locked: a.vault.locked,
        vault_free: a.vault.free,
    };
    emit_cpi!(event);
    Ok(())
}
