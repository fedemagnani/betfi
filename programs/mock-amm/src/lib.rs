//! Mock AMM for Schedina's "pay in any token" (spec section 8, plan step 4.D). Devnet and
//! localnet only: mock mints (mockBONK, mockSOL) with a rate-limited faucet, and one
//! constant-product pool (x·y = k, 0.5% fee kept in the pool) per mock mint against
//! schedina's mock USDC. It never touches the schedina program or its mint authority;
//! pools hold mock USDC bought through schedina's faucet (D-052).
//!
//! The app sends `[swap, Ed25519 verify, open_pending]` in one transaction, so the stake
//! is escrowed only if the swap paid at least `min_out` (L4-3). That transaction is a v0
//! one: `AmmConfig.lookup_table` names a lookup table with the static accounts.

use anchor_lang::prelude::*;

pub mod constants;
pub mod error;
pub mod events;
pub mod instructions;
pub mod math;
pub mod state;
mod token_pda;

pub use constants::*;
pub use instructions::*;
pub use state::*;

declare_id!("5WhsDY2XjzJoWDbe1AgJnnNgJccQwte9TgsvG4yRtHbf");

#[program]
pub mod mock_amm {
    use super::*;

    /// Creates `AmmConfig` with the signer as admin and `usdc_mint` as every pool's quote
    /// mint. Upgrade authority only.
    pub fn init_amm(ctx: Context<InitAmm>, usdc_mint: Pubkey) -> Result<()> {
        admin::init_amm_handler(ctx, usdc_mint)
    }

    /// Points `AmmConfig.lookup_table` at an address lookup table. Admin only.
    pub fn set_lookup_table(ctx: Context<SetLookupTable>) -> Result<()> {
        admin::set_lookup_table_handler(ctx)
    }

    /// Creates the mock mint `["mock", symbol]` and its `MockToken`. Admin only.
    pub fn init_mock_mint(
        ctx: Context<InitMockMint>,
        symbol: String,
        decimals: u8,
        faucet_max: u64,
    ) -> Result<()> {
        admin::init_mock_mint_handler(ctx, symbol, decimals, faucet_max)
    }

    /// Creates the pool of a mock mint against the USDC mint, with empty vaults. Admin only.
    pub fn init_pool(ctx: Context<InitPool>) -> Result<()> {
        admin::init_pool_handler(ctx)
    }

    /// Deposits both sides at the pool price (the first deposit sets it: admin only).
    pub fn add_liquidity(
        ctx: Context<AddLiquidity>,
        mint_amount: u64,
        max_usdc: u64,
    ) -> Result<()> {
        liquidity::handler(ctx, mint_amount, max_usdc)
    }

    /// Constant-product swap with a 0.5% fee; fails below `min_out`. User signs.
    pub fn swap(ctx: Context<Swap>, amount_in: u64, min_out: u64) -> Result<()> {
        swap::handler(ctx, amount_in, min_out)
    }

    /// Mints mock tokens to any token account, capped and once an hour per owner.
    pub fn faucet(ctx: Context<Faucet>, amount: u64) -> Result<()> {
        faucet::handler(ctx, amount)
    }
}
