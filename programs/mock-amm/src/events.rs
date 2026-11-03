//! Events, logged with `emit!` (program log). Unlike schedina (D-005, `emit_cpi!`), no service
//! consumes them: the desk reads pool accounts. Logs keep two accounts off the swap and about
//! 11 KB off the `.so`. Reserves are the values after the instruction.

use anchor_lang::prelude::*;

#[event]
pub struct MockMintCreated {
    pub mint: Pubkey,
    pub symbol: String,
    pub decimals: u8,
    pub faucet_max: u64,
}

#[event]
pub struct PoolCreated {
    pub pool: Pubkey,
    pub mint: Pubkey,
    pub usdc_mint: Pubkey,
    pub fee_bps: u16,
}

#[event]
pub struct LiquidityAdded {
    pub pool: Pubkey,
    pub provider: Pubkey,
    pub mint_amount: u64,
    pub usdc_amount: u64,
    pub reserve_mint: u64,
    pub reserve_usdc: u64,
}

#[event]
pub struct Swapped {
    pub pool: Pubkey,
    pub user: Pubkey,
    pub mint_in: Pubkey,
    pub amount_in: u64,
    /// Part of `amount_in` kept by the pool.
    pub fee_in: u64,
    pub amount_out: u64,
    pub reserve_in: u64,
    pub reserve_out: u64,
}

#[event]
pub struct FaucetMinted {
    pub mint: Pubkey,
    pub recipient: Pubkey,
    pub amount: u64,
}
