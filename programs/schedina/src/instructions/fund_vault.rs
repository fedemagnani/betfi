//! `fund_vault(amount)`: anyone deposits USDC into the house vault (`Vault.free`).

use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token, TokenAccount};

use crate::constants::*;
use crate::error::SchedinaError;
use crate::events::VaultFunded;
use crate::state::{Config, Vault};
use crate::vault::pay_from_user;

#[event_cpi]
#[derive(Accounts)]
pub struct FundVault<'info> {
    pub funder: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump, has_one = usdc_mint)]
    pub config: Box<Account<'info, Config>>,

    #[account(mut, seeds = [VAULT_SEED], bump = vault.bump)]
    pub vault: Box<Account<'info, Vault>>,

    pub usdc_mint: Box<Account<'info, Mint>>,

    #[account(mut, token::mint = usdc_mint, token::authority = funder)]
    pub funder_usdc: Box<Account<'info, TokenAccount>>,

    #[account(mut, address = vault.usdc_ata)]
    pub vault_usdc: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(mut ctx: Context<FundVault>, amount: u64) -> Result<()> {
    require!(amount > 0, SchedinaError::ZeroAmount);
    let a = &mut ctx.accounts;
    pay_from_user(
        &a.token_program,
        &a.usdc_mint,
        &a.funder_usdc,
        &a.vault_usdc,
        a.funder.to_account_info(),
        amount,
    )?;
    a.vault.free = a
        .vault
        .free
        .checked_add(amount)
        .ok_or(SchedinaError::MathOverflow)?;
    let event = VaultFunded {
        funder: a.funder.key(),
        amount_u: amount,
        vault_free: a.vault.free,
    };
    emit_cpi!(event);
    Ok(())
}
