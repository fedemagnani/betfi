//! Admin set-up: `init_amm` (upgrade authority only, so nobody front-runs the deploy),
//! `set_lookup_table`, `init_mock_mint`, `init_pool`.

use anchor_lang::prelude::*;
use anchor_spl::token::Token;

use crate::constants::*;
use crate::error::AmmError;
use crate::events::{MockMintCreated, PoolCreated};
use crate::program::MockAmm;
use crate::state::{AmmConfig, MockToken, Pool};
use crate::token_pda::{create_mint, create_token_account};

#[derive(Accounts)]
pub struct InitAmm<'info> {
    /// Upgrade authority of this program; becomes `AmmConfig.admin` and pays rent.
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        init,
        payer = admin,
        space = 8 + AmmConfig::INIT_SPACE,
        seeds = [AMM_CONFIG_SEED],
        bump,
    )]
    pub amm_config: Box<Account<'info, AmmConfig>>,

    #[account(constraint = program.programdata_address()? == Some(program_data.key()))]
    pub program: Program<'info, MockAmm>,

    #[account(
        constraint = program_data.upgrade_authority_address == Some(admin.key())
            @ AmmError::NotUpgradeAuthority,
    )]
    pub program_data: Account<'info, ProgramData>,

    pub system_program: Program<'info, System>,
}

pub fn init_amm_handler(ctx: Context<InitAmm>, usdc_mint: Pubkey) -> Result<()> {
    require!(usdc_mint != Pubkey::default(), AmmError::SameMint);
    let c = &mut ctx.accounts.amm_config;
    c.admin = ctx.accounts.admin.key();
    c.usdc_mint = usdc_mint;
    c.lookup_table = Pubkey::default();
    c.bump = ctx.bumps.amm_config;
    Ok(())
}

#[derive(Accounts)]
pub struct SetLookupTable<'info> {
    pub admin: Signer<'info>,

    #[account(mut, seeds = [AMM_CONFIG_SEED], bump = amm_config.bump, has_one = admin @ AmmError::NotAdmin)]
    pub amm_config: Box<Account<'info, AmmConfig>>,

    /// CHECK: only its owner is checked; clients read its addresses.
    #[account(owner = ALT_PROGRAM_ID @ AmmError::BadLookupTable)]
    pub lookup_table: UncheckedAccount<'info>,
}

pub fn set_lookup_table_handler(ctx: Context<SetLookupTable>) -> Result<()> {
    ctx.accounts.amm_config.lookup_table = ctx.accounts.lookup_table.key();
    Ok(())
}

#[derive(Accounts)]
#[instruction(symbol: String)]
pub struct InitMockMint<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(seeds = [AMM_CONFIG_SEED], bump = amm_config.bump, has_one = admin @ AmmError::NotAdmin)]
    pub amm_config: Box<Account<'info, AmmConfig>>,

    /// CHECK: created here as an SPL mint (authority `amm_config`).
    #[account(mut, seeds = [MOCK_SEED, symbol.as_bytes()], bump)]
    pub mint: UncheckedAccount<'info>,

    #[account(
        init,
        payer = admin,
        space = 8 + MockToken::INIT_SPACE,
        seeds = [TOKEN_SEED, mint.key().as_ref()],
        bump,
    )]
    pub mock_token: Box<Account<'info, MockToken>>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

pub fn init_mock_mint_handler(
    mut ctx: Context<InitMockMint>,
    symbol: String,
    decimals: u8,
    faucet_max: u64,
) -> Result<()> {
    require!(
        !symbol.is_empty()
            && symbol.len() <= MAX_SYMBOL_LEN
            && symbol
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()),
        AmmError::BadSymbol
    );
    require!(decimals <= MAX_DECIMALS, AmmError::BadDecimals);
    require!(faucet_max > 0, AmmError::ZeroAmount);
    let a = &mut ctx.accounts;
    let mint_bump = ctx.bumps.mint;
    create_mint(
        &a.admin.to_account_info(),
        &a.mint.to_account_info(),
        &a.amm_config.key(),
        decimals,
        &a.system_program.to_account_info(),
        &a.token_program.to_account_info(),
        &[MOCK_SEED, symbol.as_bytes(), &[mint_bump]],
    )?;
    let t = &mut a.mock_token;
    t.mint = a.mint.key();
    t.symbol = symbol.clone();
    t.decimals = decimals;
    t.faucet_max = faucet_max;
    t.bump = ctx.bumps.mock_token;
    t.mint_bump = mint_bump;
    let event = MockMintCreated {
        mint: t.mint,
        symbol,
        decimals,
        faucet_max,
    };
    emit!(event);
    Ok(())
}

#[derive(Accounts)]
pub struct InitPool<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        seeds = [AMM_CONFIG_SEED],
        bump = amm_config.bump,
        has_one = admin @ AmmError::NotAdmin,
        has_one = usdc_mint @ AmmError::MintMismatch,
    )]
    pub amm_config: Box<Account<'info, AmmConfig>>,

    /// Pools exist only for this program's mock mints.
    #[account(seeds = [TOKEN_SEED, mint.key().as_ref()], bump = mock_token.bump, has_one = mint)]
    pub mock_token: Box<Account<'info, MockToken>>,

    /// CHECK: the mock mint (`mock_token.mint`).
    pub mint: UncheckedAccount<'info>,

    /// CHECK: `amm_config.usdc_mint`; SPL Token's `InitializeAccount3` refuses a non-mint.
    pub usdc_mint: UncheckedAccount<'info>,

    #[account(
        init,
        payer = admin,
        space = 8 + Pool::INIT_SPACE,
        seeds = [POOL_SEED, mint.key().as_ref()],
        bump,
    )]
    pub pool: Box<Account<'info, Pool>>,

    /// CHECK: created here as the pool's token account of `mint`.
    #[account(mut, seeds = [VAULT_SEED, pool.key().as_ref(), mint.key().as_ref()], bump)]
    pub vault: UncheckedAccount<'info>,

    /// CHECK: created here as the pool's token account of `usdc_mint`.
    #[account(mut, seeds = [VAULT_SEED, pool.key().as_ref(), usdc_mint.key().as_ref()], bump)]
    pub vault_usdc: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

pub fn init_pool_handler(mut ctx: Context<InitPool>) -> Result<()> {
    let a = &mut ctx.accounts;
    let b = &ctx.bumps;
    require_keys_neq!(a.mint.key(), a.usdc_mint.key(), AmmError::SameMint);
    let pool_key = a.pool.key();
    let payer = a.admin.to_account_info();
    let sys = a.system_program.to_account_info();
    let tok = a.token_program.to_account_info();
    let pool_info = a.pool.to_account_info();
    let mint_key = a.mint.key();
    let usdc_key = a.usdc_mint.key();
    create_token_account(
        &payer,
        &a.vault.to_account_info(),
        &a.mint.to_account_info(),
        &pool_info,
        &sys,
        &tok,
        &[VAULT_SEED, pool_key.as_ref(), mint_key.as_ref(), &[b.vault]],
    )?;
    create_token_account(
        &payer,
        &a.vault_usdc.to_account_info(),
        &a.usdc_mint.to_account_info(),
        &pool_info,
        &sys,
        &tok,
        &[
            VAULT_SEED,
            pool_key.as_ref(),
            usdc_key.as_ref(),
            &[b.vault_usdc],
        ],
    )?;
    let p = &mut a.pool;
    p.mint = mint_key;
    p.usdc_mint = usdc_key;
    p.vault = a.vault.key();
    p.vault_usdc = a.vault_usdc.key();
    p.fee_bps = FEE_BPS;
    p.fees_mint = 0;
    p.fees_usdc = 0;
    p.swaps = 0;
    p.bump = b.pool;
    p.vault_bump = b.vault;
    p.vault_usdc_bump = b.vault_usdc;
    let event = PoolCreated {
        pool: pool_key,
        mint: mint_key,
        usdc_mint: usdc_key,
        fee_bps: FEE_BPS,
    };
    emit!(event);
    Ok(())
}
