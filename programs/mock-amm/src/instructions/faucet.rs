//! `faucet(amount)`: mints up to `MockToken.faucet_max` of a mock mint into any token
//! account of it, at most once per `FAUCET_COOLDOWN` per (mint, signer). Keyed on the signer,
//! not the recipient, so nobody can spend someone else's hourly claim by minting to them.
//! The admin is exempt from cap and cooldown (it seeds the pools). The token account must
//! exist first. `payer` pays the `FaucetClaim` rent (L8): the caller themself (the same key
//! twice), or the desk's sponsor key when the desk relays a sponsored claim.

use anchor_lang::prelude::*;
use anchor_spl::token::{self, MintTo, Token, TokenAccount};

use crate::constants::*;
use crate::error::AmmError;
use crate::events::FaucetMinted;
use crate::state::{AmmConfig, FaucetClaim, MockToken};

#[derive(Accounts)]
pub struct Faucet<'info> {
    /// Whose hourly claim this is; pays nothing (L8: `payer` pays the rent).
    pub caller: Signer<'info>,

    #[account(seeds = [AMM_CONFIG_SEED], bump = amm_config.bump)]
    pub amm_config: Box<Account<'info, AmmConfig>>,

    #[account(seeds = [TOKEN_SEED, mint.key().as_ref()], bump = mock_token.bump, has_one = mint)]
    pub mock_token: Box<Account<'info, MockToken>>,

    /// CHECK: `mock_token.mint`, a mint whose authority is `amm_config` (SPL Token checks).
    #[account(mut)]
    pub mint: UncheckedAccount<'info>,

    #[account(mut, token::mint = mint)]
    pub recipient: Box<Account<'info, TokenAccount>>,

    #[account(
        init_if_needed,
        payer = payer,
        space = 8 + FaucetClaim::INIT_SPACE,
        seeds = [FAUCET_SEED, mint.key().as_ref(), caller.key().as_ref()],
        bump,
    )]
    pub faucet_claim: Box<Account<'info, FaucetClaim>>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,

    /// Pays the `FaucetClaim` rent (L8). Last, so the earlier account indices are unchanged.
    #[account(mut)]
    pub payer: Signer<'info>,
}

pub fn handler(mut ctx: Context<Faucet>, amount: u64) -> Result<()> {
    let a = &mut ctx.accounts;
    let admin = a.caller.key() == a.amm_config.admin;
    require!(amount > 0, AmmError::ZeroAmount);
    require!(
        admin || amount <= a.mock_token.faucet_max,
        AmmError::FaucetAmount
    );
    let now = Clock::get()?.unix_timestamp;
    let claim = &mut a.faucet_claim;
    if claim.owner == Pubkey::default() {
        // New claim: `last_claim_ts` is 0, so the first claim always passes.
        claim.owner = a.caller.key();
        claim.mint = a.mint.key();
        claim.bump = ctx.bumps.faucet_claim;
    }
    // The admin's mints neither check nor move a cooldown.
    if !admin {
        let next = claim
            .last_claim_ts
            .checked_add(FAUCET_COOLDOWN)
            .ok_or(AmmError::MathOverflow)?;
        require!(now >= next, AmmError::FaucetCooldown);
        claim.last_claim_ts = now;
    }

    let seeds: &[&[u8]] = &[AMM_CONFIG_SEED, &[a.amm_config.bump]];
    token::mint_to(
        CpiContext::new_with_signer(
            a.token_program.key(),
            MintTo {
                mint: a.mint.to_account_info(),
                to: a.recipient.to_account_info(),
                authority: a.amm_config.to_account_info(),
            },
            &[seeds],
        ),
        amount,
    )?;
    let event = FaucetMinted {
        mint: a.mint.key(),
        recipient: a.recipient.key(),
        amount,
    };
    emit!(event);
    Ok(())
}
