//! `init_config(params)`: creates `Config`, `Vault`, the mock USDC mint
//! (`["mock", "USDC"]`) and the two program token accounts (vault, escrow). Only the
//! program's upgrade authority may call it, so nobody can front-run the deploy.
//!
//! The mint and token accounts are created with direct System + SPL Token CPIs
//! rather than Anchor's `mint::`/`token::` init constraints, which pull in the
//! Token-2022 code paths and cost program size (sibling pattern).

use anchor_lang::prelude::*;
use anchor_lang::system_program::{self, Allocate, Assign, CreateAccount, Transfer};
use anchor_spl::token::{self, InitializeAccount3, InitializeMint2, Mint, Token, TokenAccount};

use crate::constants::*;
use crate::error::SchedinaError;
use crate::program::Schedina;
use crate::state::{Config, Vault};

/// Everything `set_params` can change, plus the authority at init.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct Params {
    pub crank: Pubkey,
    pub quote_signer: Pubkey,
    pub max_legs: u8,
    pub max_payout: u64,
    pub min_stake: u64,
    pub reserve: u64,
    pub copy_fee_bps: u16,
    pub tipster_share_bps: u16,
}

impl Params {
    pub fn validate(&self) -> Result<()> {
        require!(
            self.crank != Pubkey::default()
                && self.quote_signer != Pubkey::default()
                && self.max_legs >= 1
                && usize::from(self.max_legs) <= MAX_LEGS
                && self.min_stake > 0
                && self.max_payout > 0
                && self.copy_fee_bps <= BPS
                && self.tipster_share_bps <= BPS,
            SchedinaError::InvalidParams
        );
        Ok(())
    }

    pub fn apply(&self, c: &mut Config) {
        c.crank = self.crank;
        c.quote_signer = self.quote_signer;
        c.max_legs = self.max_legs;
        c.max_payout = self.max_payout;
        c.min_stake = self.min_stake;
        c.reserve = self.reserve;
        c.copy_fee_bps = self.copy_fee_bps;
        c.tipster_share_bps = self.tipster_share_bps;
    }
}

#[derive(Accounts)]
pub struct InitConfig<'info> {
    /// Upgrade authority of this program; pays rent.
    #[account(mut)]
    pub deployer: Signer<'info>,

    #[account(
        init,
        payer = deployer,
        space = 8 + Config::INIT_SPACE,
        seeds = [CONFIG_SEED],
        bump,
    )]
    pub config: Box<Account<'info, Config>>,

    #[account(
        init,
        payer = deployer,
        space = 8 + Vault::INIT_SPACE,
        seeds = [VAULT_SEED],
        bump,
    )]
    pub vault: Box<Account<'info, Vault>>,

    /// CHECK: created here as an SPL mint (6 decimals, authority = `config`).
    #[account(mut, seeds = [MOCK_SEED, USDC_SYMBOL], bump)]
    pub usdc_mint: UncheckedAccount<'info>,

    /// CHECK: created here as a token account (mint = `usdc_mint`, owner = `config`).
    #[account(mut, seeds = [VAULT_USDC_SEED], bump)]
    pub vault_usdc: UncheckedAccount<'info>,

    /// CHECK: as `vault_usdc`.
    #[account(mut, seeds = [ESCROW_USDC_SEED], bump)]
    pub escrow_usdc: UncheckedAccount<'info>,

    #[account(constraint = program.programdata_address()? == Some(program_data.key()))]
    pub program: Program<'info, Schedina>,

    #[account(
        constraint = program_data.upgrade_authority_address == Some(deployer.key())
            @ SchedinaError::NotUpgradeAuthority,
    )]
    pub program_data: Account<'info, ProgramData>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

impl<'info> InitConfig<'info> {
    /// Creates PDA `target` (signed by `seeds`) with `space` bytes owned by the token
    /// program. Tolerates lamports sent to the address beforehand, which would make a
    /// plain `create_account` fail.
    fn create_token_pda(
        &self,
        target: &UncheckedAccount<'info>,
        space: usize,
        seeds: &[&[u8]],
    ) -> Result<()> {
        let sys = self.system_program.key();
        let info = target.to_account_info();
        let rent = Rent::get()?.minimum_balance(space);
        let signer: &[&[&[u8]]] = &[seeds];
        if info.lamports() == 0 {
            return system_program::create_account(
                CpiContext::new_with_signer(
                    sys,
                    CreateAccount {
                        from: self.deployer.to_account_info(),
                        to: info,
                    },
                    signer,
                ),
                rent,
                space as u64,
                &token::ID,
            );
        }
        let top_up = rent.saturating_sub(info.lamports());
        if top_up > 0 {
            system_program::transfer(
                CpiContext::new(
                    sys,
                    Transfer {
                        from: self.deployer.to_account_info(),
                        to: info.clone(),
                    },
                ),
                top_up,
            )?;
        }
        system_program::allocate(
            CpiContext::new_with_signer(
                sys,
                Allocate {
                    account_to_allocate: info.clone(),
                },
                signer,
            ),
            space as u64,
        )?;
        system_program::assign(
            CpiContext::new_with_signer(
                sys,
                Assign {
                    account_to_assign: info,
                },
                signer,
            ),
            &token::ID,
        )
    }

    /// Creates and initializes a token account (mint = `usdc_mint`, owner = `config`).
    fn create_token_account(
        &self,
        acc: &UncheckedAccount<'info>,
        seed: &[u8],
        bump: u8,
    ) -> Result<()> {
        self.create_token_pda(acc, TokenAccount::LEN, &[seed, &[bump]])?;
        token::initialize_account3(CpiContext::new(
            self.token_program.key(),
            InitializeAccount3 {
                account: acc.to_account_info(),
                mint: self.usdc_mint.to_account_info(),
                authority: self.config.to_account_info(),
            },
        ))
    }
}

pub fn handler(ctx: Context<InitConfig>, authority: Pubkey, params: Params) -> Result<()> {
    require!(authority != Pubkey::default(), SchedinaError::InvalidParams);
    params.validate()?;
    let a = &ctx.accounts;
    let b = &ctx.bumps;

    a.create_token_pda(
        &a.usdc_mint,
        Mint::LEN,
        &[MOCK_SEED, USDC_SYMBOL, &[b.usdc_mint]],
    )?;
    token::initialize_mint2(
        CpiContext::new(
            a.token_program.key(),
            InitializeMint2 {
                mint: a.usdc_mint.to_account_info(),
            },
        ),
        USDC_DECIMALS,
        &a.config.key(),
        None,
    )?;
    a.create_token_account(&a.vault_usdc, VAULT_USDC_SEED, b.vault_usdc)?;
    a.create_token_account(&a.escrow_usdc, ESCROW_USDC_SEED, b.escrow_usdc)?;

    let mut config = Config {
        authority,
        crank: Pubkey::default(),
        quote_signer: Pubkey::default(),
        usdc_mint: a.usdc_mint.key(),
        copy_fee_bps: 0,
        tipster_share_bps: 0,
        max_legs: 0,
        max_payout: 0,
        min_stake: 0,
        reserve: 0,
        paused: false,
        bump: b.config,
        usdc_mint_bump: b.usdc_mint,
        reserved: [0; 64],
    };
    params.apply(&mut config);
    let vault = Vault {
        usdc_ata: a.vault_usdc.key(),
        escrow_ata: a.escrow_usdc.key(),
        locked: 0,
        free: 0,
        lp_shares: 0,
        bump: b.vault,
        reserved: [0; 64],
    };
    ctx.accounts.config.set_inner(config);
    ctx.accounts.vault.set_inner(vault);
    Ok(())
}
