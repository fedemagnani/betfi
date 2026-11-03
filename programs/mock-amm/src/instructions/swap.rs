//! `swap(amount_in, min_out)`: sells `amount_in` from `user_in` into the pool and pays
//! `floor((amount_in − fee) × r_out / (r_in + amount_in − fee))` to `user_out`, refusing
//! anything below `min_out` (the whole transaction then fails, so a following
//! `open_pending` never runs). Either direction: `vault_in`/`vault_out` are the pool's two
//! vaults in the order of the trade. Reserves are the vault balances before the trade.

use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

use crate::constants::*;
use crate::error::AmmError;
use crate::events::Swapped;
use crate::math;
use crate::state::Pool;

#[derive(Accounts)]
pub struct Swap<'info> {
    pub user: Signer<'info>,

    #[account(mut, seeds = [POOL_SEED, pool.mint.as_ref()], bump = pool.bump)]
    pub pool: Box<Account<'info, Pool>>,

    /// Pays `amount_in`; SPL Token checks that `user` is its owner or delegate.
    #[account(mut, constraint = user_in.mint == vault_in.mint @ AmmError::MintMismatch)]
    pub user_in: Box<Account<'info, TokenAccount>>,

    /// Receives the output; any account of the output mint.
    #[account(mut, constraint = user_out.mint == vault_out.mint @ AmmError::MintMismatch)]
    pub user_out: Box<Account<'info, TokenAccount>>,

    #[account(mut)]
    pub vault_in: Box<Account<'info, TokenAccount>>,

    #[account(mut)]
    pub vault_out: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(mut ctx: Context<Swap>, amount_in: u64, min_out: u64) -> Result<()> {
    require!(amount_in > 0, AmmError::ZeroAmount);
    let a = &mut ctx.accounts;
    let (vin, vout) = (a.vault_in.key(), a.vault_out.key());
    let p = &a.pool;
    let selling_mint = vin == p.vault && vout == p.vault_usdc;
    let buying_mint = vin == p.vault_usdc && vout == p.vault;
    require!(selling_mint || buying_mint, AmmError::WrongVault);

    let (r_in, r_out) = (a.vault_in.amount, a.vault_out.amount);
    require!(r_in > 0 && r_out > 0, AmmError::EmptyPool);
    let s = math::swap_out(amount_in, r_in, r_out, p.fee_bps).ok_or(AmmError::ZeroOut)?;
    require!(s.amount_out >= min_out, AmmError::SlippageExceeded);

    let tok = a.token_program.key();
    token::transfer(
        CpiContext::new(
            tok,
            Transfer {
                from: a.user_in.to_account_info(),
                to: a.vault_in.to_account_info(),
                authority: a.user.to_account_info(),
            },
        ),
        amount_in,
    )?;
    let mint = p.mint;
    let seeds: &[&[u8]] = &[POOL_SEED, mint.as_ref(), &[p.bump]];
    token::transfer(
        CpiContext::new_with_signer(
            tok,
            Transfer {
                from: a.vault_out.to_account_info(),
                to: a.user_out.to_account_info(),
                authority: a.pool.to_account_info(),
            },
            &[seeds],
        ),
        s.amount_out,
    )?;

    let pool = &mut a.pool;
    if selling_mint {
        pool.fees_mint = pool
            .fees_mint
            .checked_add(s.fee_in)
            .ok_or(AmmError::MathOverflow)?;
    } else {
        pool.fees_usdc = pool
            .fees_usdc
            .checked_add(s.fee_in)
            .ok_or(AmmError::MathOverflow)?;
    }
    pool.swaps = pool.swaps.saturating_add(1);
    let event = Swapped {
        pool: pool.key(),
        user: a.user.key(),
        mint_in: a.vault_in.mint,
        amount_in,
        fee_in: s.fee_in,
        amount_out: s.amount_out,
        reserve_in: r_in.checked_add(amount_in).ok_or(AmmError::MathOverflow)?,
        reserve_out: r_out - s.amount_out,
    };
    emit!(event);
    Ok(())
}
