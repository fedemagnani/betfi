//! `refund_pending()`: anyone, from `expires_at + 15 s`. Returns the escrowed stake and
//! copy fee to a USDC account of the pending's user and closes the `Pending` (rent to
//! the user).

use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token, TokenAccount};

use crate::constants::*;
use crate::error::SchedinaError;
use crate::events::PendingRefunded;
use crate::state::{Config, Pending};
use crate::vault::pay_from_program;

#[event_cpi]
#[derive(Accounts)]
pub struct RefundPending<'info> {
    /// Anyone; pays the transaction fee only.
    pub caller: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump, has_one = usdc_mint)]
    pub config: Box<Account<'info, Config>>,

    #[account(
        mut,
        close = rent_payer,
        seeds = [PENDING_SEED, pending.user.as_ref(), &pending.nonce.to_le_bytes()],
        bump = pending.bump,
    )]
    pub pending: Box<Account<'info, Pending>>,

    /// CHECK: receives the `Pending` rent: whoever paid it (L8), address-checked against
    /// `pending.rent_receiver()`; any owner (a smart-wallet user must still get the refund).
    #[account(mut, address = pending.rent_receiver())]
    pub rent_payer: UncheckedAccount<'info>,

    #[account(mut, token::mint = usdc_mint, token::authority = pending.user)]
    pub user_usdc: Box<Account<'info, TokenAccount>>,

    pub usdc_mint: Box<Account<'info, Mint>>,

    #[account(mut, seeds = [ESCROW_USDC_SEED], bump)]
    pub escrow_usdc: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<RefundPending>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let a = &ctx.accounts;
    let p = &a.pending;
    let opens = p
        .expires_at
        .checked_add(REFUND_GRACE)
        .ok_or(SchedinaError::MathOverflow)?;
    require!(now >= opens, SchedinaError::RefundTooEarly);
    let amount_u = p
        .stake_u
        .checked_add(p.copy_fee_u)
        .ok_or(SchedinaError::MathOverflow)?;
    pay_from_program(
        &a.token_program,
        &a.usdc_mint,
        &a.config,
        &a.escrow_usdc,
        &a.user_usdc,
        amount_u,
    )?;
    let event = PendingRefunded {
        pending: p.key(),
        user: p.user,
        nonce: p.nonce,
        amount_u,
    };
    emit_cpi!(event);
    Ok(())
}
