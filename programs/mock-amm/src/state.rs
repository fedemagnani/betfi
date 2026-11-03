//! Accounts. Pool reserves are the balances of the pool's two token accounts, so there is
//! no stored reserve to fall out of sync; `Pool` keeps the fee totals for accounting.

use anchor_lang::prelude::*;

use crate::constants::MAX_SYMBOL_LEN;

/// PDA `["amm_config"]`.
#[account]
#[derive(InitSpace)]
pub struct AmmConfig {
    /// Creates mints and pools, seeds first liquidity, sets the lookup table; faucet-exempt.
    pub admin: Pubkey,
    /// The quote currency of every pool: schedina's mock USDC (`["mock","USDC"]` of schedina).
    pub usdc_mint: Pubkey,
    /// Address lookup table holding the static accounts of `[swap, verify, open_pending]`
    /// (`Pubkey::default()` until set). Clients read it to build a v0 transaction.
    pub lookup_table: Pubkey,
    pub bump: u8,
    pub reserved: [u8; 32],
}

/// PDA `["token", mint]`: one per mock mint.
#[account]
#[derive(InitSpace)]
pub struct MockToken {
    /// The SPL mint, PDA `["mock", symbol]`, mint authority `AmmConfig`, no freeze authority.
    pub mint: Pubkey,
    #[max_len(MAX_SYMBOL_LEN)]
    pub symbol: String,
    pub decimals: u8,
    /// Largest faucet claim, base units.
    pub faucet_max: u64,
    pub bump: u8,
    pub mint_bump: u8,
}

/// PDA `["pool", mint]`: constant-product pool of `mint` against `usdc_mint`.
#[account]
#[derive(InitSpace)]
pub struct Pool {
    pub mint: Pubkey,
    pub usdc_mint: Pubkey,
    /// Token account of `mint`, PDA `["vault", pool, mint]`, owner this pool.
    pub vault: Pubkey,
    /// Token account of `usdc_mint`, PDA `["vault", pool, usdc_mint]`, owner this pool.
    pub vault_usdc: Pubkey,
    /// Fee in bps of the amount in (`FEE_BPS`).
    pub fee_bps: u16,
    /// Fees kept so far, in `mint` (sells of the mock token) and in USDC (buys).
    pub fees_mint: u64,
    pub fees_usdc: u64,
    pub swaps: u64,
    pub bump: u8,
    pub vault_bump: u8,
    pub vault_usdc_bump: u8,
}

/// PDA `["faucet", mint, caller]`: last faucet claim of `mint` signed by `owner`.
#[account]
#[derive(InitSpace)]
pub struct FaucetClaim {
    pub owner: Pubkey,
    pub mint: Pubkey,
    pub last_claim_ts: i64,
    pub bump: u8,
}
