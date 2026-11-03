//! `faucet_usdc(amount)`: devnet only (`devnet` cargo feature). Mints up to
//! `FAUCET_MAX` mock USDC into any token account of the mint, as often as asked (D-661:
//! no cooldown). `FaucetClaim` still records each owner's last claim. The caller creates
//! the token account (e.g. its ATA) first and pays the `FaucetClaim` rent.

use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, MintTo, Token, TokenAccount};

use crate::constants::*;
use crate::error::SchedinaError;
use crate::state::{Config, FaucetClaim};

#[derive(Accounts)]
pub struct FaucetUsdc<'info> {
    #[account(mut)]
    pub caller: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump, has_one = usdc_mint)]
    pub config: Box<Account<'info, Config>>,

    #[account(mut)]
    pub usdc_mint: Box<Account<'info, Mint>>,

    #[account(mut, token::mint = usdc_mint)]
    pub recipient: Box<Account<'info, TokenAccount>>,

    #[account(
        init_if_needed,
        payer = caller,
        space = 8 + FaucetClaim::INIT_SPACE,
        seeds = [FAUCET_SEED, recipient.owner.as_ref()],
        bump,
    )]
    pub faucet_claim: Box<Account<'info, FaucetClaim>>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

pub fn handler(mut ctx: Context<FaucetUsdc>, amount: u64) -> Result<()> {
    require!(
        amount > 0 && amount <= FAUCET_MAX,
        SchedinaError::FaucetAmount
    );
    let now = Clock::get()?.unix_timestamp;
    let a = &mut ctx.accounts;
    let claim = &mut a.faucet_claim;
    if claim.owner == Pubkey::default() {
        claim.owner = a.recipient.owner;
        claim.bump = ctx.bumps.faucet_claim;
    }
    claim.last_claim_ts = now;

    let seeds: &[&[u8]] = &[CONFIG_SEED, &[a.config.bump]];
    token::mint_to(
        CpiContext::new_with_signer(
            a.token_program.key(),
            MintTo {
                mint: a.usdc_mint.to_account_info(),
                to: a.recipient.to_account_info(),
                authority: a.config.to_account_info(),
            },
            &[seeds],
        ),
        amount,
    )
}
