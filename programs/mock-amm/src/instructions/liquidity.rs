//! `add_liquidity(mint_amount, max_usdc)`: deposits `mint_amount` of the mock mint and the
//! USDC that keeps the price, `ceil(mint_amount × r_usdc / r_mint)` (at most `max_usdc`).
//! The first deposit, which sets the price, is the admin's and takes exactly
//! `(mint_amount, max_usdc)`. There are no LP shares and no withdrawal: liquidity is a
//! donation to the pool, so nothing but `swap` ever moves tokens out of a vault.

use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

use crate::constants::*;
use crate::error::AmmError;
use crate::events::LiquidityAdded;
use crate::math;
use crate::state::{AmmConfig, Pool};

#[derive(Accounts)]
pub struct AddLiquidity<'info> {
    pub provider: Signer<'info>,

    #[account(seeds = [AMM_CONFIG_SEED], bump = amm_config.bump)]
    pub amm_config: Box<Account<'info, AmmConfig>>,

    #[account(
        seeds = [POOL_SEED, pool.mint.as_ref()],
        bump = pool.bump,
        has_one = vault @ AmmError::WrongVault,
        has_one = vault_usdc @ AmmError::WrongVault,
    )]
    pub pool: Box<Account<'info, Pool>>,

    #[account(mut, token::mint = pool.mint, token::authority = provider)]
    pub provider_mint: Box<Account<'info, TokenAccount>>,

    #[account(mut, token::mint = pool.usdc_mint, token::authority = provider)]
    pub provider_usdc: Box<Account<'info, TokenAccount>>,

    #[account(mut)]
    pub vault: Box<Account<'info, TokenAccount>>,

    #[account(mut)]
    pub vault_usdc: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(mut ctx: Context<AddLiquidity>, mint_amount: u64, max_usdc: u64) -> Result<()> {
    require!(mint_amount > 0 && max_usdc > 0, AmmError::ZeroAmount);
    let a = &mut ctx.accounts;
    let (r_mint, r_usdc) = (a.vault.amount, a.vault_usdc.amount);
    let usdc_amount = if r_mint == 0 || r_usdc == 0 {
        require_keys_eq!(a.provider.key(), a.amm_config.admin, AmmError::NotAdmin);
        max_usdc
    } else {
        let need =
            math::usdc_for_deposit(mint_amount, r_mint, r_usdc).ok_or(AmmError::MathOverflow)?;
        require!(need <= max_usdc, AmmError::DepositSlippage);
        need
    };
    let reserve_mint = r_mint
        .checked_add(mint_amount)
        .ok_or(AmmError::MathOverflow)?;
    let reserve_usdc = r_usdc
        .checked_add(usdc_amount)
        .ok_or(AmmError::MathOverflow)?;
    // k = r_mint × r_usdc must stay inside u128 for every later swap: it does (u64 × u64).
    let tok = a.token_program.key();
    let auth = a.provider.to_account_info();
    for (from, to, amount) in [
        (&a.provider_mint, &a.vault, mint_amount),
        (&a.provider_usdc, &a.vault_usdc, usdc_amount),
    ] {
        token::transfer(
            CpiContext::new(
                tok,
                Transfer {
                    from: from.to_account_info(),
                    to: to.to_account_info(),
                    authority: auth.clone(),
                },
            ),
            amount,
        )?;
    }
    let event = LiquidityAdded {
        pool: a.pool.key(),
        provider: a.provider.key(),
        mint_amount,
        usdc_amount,
        reserve_mint,
        reserve_usdc,
    };
    emit!(event);
    Ok(())
}
