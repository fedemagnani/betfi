//! `withdraw_house(amount)`: authority only. Takes house money out of `Vault.free`
//! into an authority-owned USDC account; never below `Config.reserve`, never from
//! `Vault.locked`.

use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token, TokenAccount};

use crate::constants::*;
use crate::error::SchedinaError;
use crate::events::HouseWithdrawn;
use crate::state::{Config, Vault};
use crate::vault::pay_from_program;

#[event_cpi]
#[derive(Accounts)]
pub struct WithdrawHouse<'info> {
    pub authority: Signer<'info>,

    #[account(
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = authority @ SchedinaError::NotAuthority,
        has_one = usdc_mint,
    )]
    pub config: Box<Account<'info, Config>>,

    #[account(mut, seeds = [VAULT_SEED], bump = vault.bump)]
    pub vault: Box<Account<'info, Vault>>,

    pub usdc_mint: Box<Account<'info, Mint>>,

    #[account(mut, address = vault.usdc_ata)]
    pub vault_usdc: Box<Account<'info, TokenAccount>>,

    #[account(mut, token::mint = usdc_mint, token::authority = authority)]
    pub destination: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(mut ctx: Context<WithdrawHouse>, amount: u64) -> Result<()> {
    require!(amount > 0, SchedinaError::ZeroAmount);
    let a = &mut ctx.accounts;
    let available = a
        .vault
        .free
        .checked_sub(a.config.reserve)
        .ok_or(SchedinaError::BelowReserve)?;
    require!(amount <= available, SchedinaError::BelowReserve);
    pay_from_program(
        &a.token_program,
        &a.usdc_mint,
        &a.config,
        &a.vault_usdc,
        &a.destination,
        amount,
    )?;
    a.vault.free -= amount;
    let event = HouseWithdrawn {
        destination: a.destination.key(),
        amount_u: amount,
        vault_free: a.vault.free,
    };
    emit_cpi!(event);
    Ok(())
}
