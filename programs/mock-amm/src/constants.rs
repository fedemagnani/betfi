//! PDA seeds and fixed parameters.

use anchor_lang::prelude::*;

/// `AmmConfig` PDA `["amm_config"]`: admin, the pinned USDC mint, the lookup table. Also
/// the mint authority of every mock mint.
#[constant]
pub const AMM_CONFIG_SEED: &[u8] = b"amm_config";
/// Mock mints `["mock", symbol]` (SPL mints, authority `AmmConfig`).
#[constant]
pub const MOCK_SEED: &[u8] = b"mock";
/// `MockToken` PDA `["token", mint]`: symbol, decimals and faucet cap of a mock mint.
#[constant]
pub const TOKEN_SEED: &[u8] = b"token";
/// `Pool` PDA `["pool", mint]`: one pool per mock mint, against the USDC mint.
#[constant]
pub const POOL_SEED: &[u8] = b"pool";
/// Pool token accounts `["vault", pool, mint]` and `["vault", pool, usdc_mint]`, owner `Pool`.
#[constant]
pub const VAULT_SEED: &[u8] = b"vault";
/// `FaucetClaim` PDA `["faucet", mint, caller]` (the signer of `faucet`).
#[constant]
pub const FAUCET_SEED: &[u8] = b"faucet";

/// Pool fee: 0.5% of the amount in (spec section 8), kept in the pool.
#[constant]
pub const FEE_BPS: u16 = 50;
/// Seconds between two faucet claims of one mint by one owner (admin exempt).
#[constant]
pub const FAUCET_COOLDOWN: i64 = 3_600;
/// Longest mock symbol (`"BONK"`, `"SOL"`, …), ASCII upper-case letters and digits.
pub const MAX_SYMBOL_LEN: usize = 8;
/// Largest decimals of a mock mint.
pub const MAX_DECIMALS: u8 = 9;

/// The address lookup table program (owner of `AmmConfig.lookup_table`).
pub const ALT_PROGRAM_ID: Pubkey = pubkey!("AddressLookupTab1e1111111111111111111111111");
