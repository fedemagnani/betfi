//! Program error codes (6000 + index).

use anchor_lang::prelude::*;

#[error_code]
pub enum AmmError {
    #[msg("signer is not the program's upgrade authority")]
    NotUpgradeAuthority,
    #[msg("signer is not AmmConfig.admin")]
    NotAdmin,
    #[msg("symbol must be 1-8 ASCII upper-case letters or digits")]
    BadSymbol,
    #[msg("decimals out of range")]
    BadDecimals,
    #[msg("amount must be positive")]
    ZeroAmount,
    #[msg("faucet amount above this mint's cap")]
    FaucetAmount,
    #[msg("faucet claimed less than an hour ago")]
    FaucetCooldown,
    #[msg("arithmetic overflow")]
    MathOverflow,
    #[msg("the pool has no liquidity on one side")]
    EmptyPool,
    #[msg("the swap would pay out nothing")]
    ZeroOut,
    #[msg("output below min_out")]
    SlippageExceeded,
    #[msg("vault accounts are not this pool's")]
    WrongVault,
    #[msg("token account mint does not match")]
    MintMismatch,
    #[msg("deposit needs more USDC than max_usdc")]
    DepositSlippage,
    #[msg("the pool's mint cannot be the USDC mint")]
    SameMint,
    #[msg("not an address lookup table")]
    BadLookupTable,
}
