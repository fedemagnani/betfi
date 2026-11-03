//! Transfers out of the program's token accounts (vault and escrow; token authority =
//! the `Config` PDA) and into them from a user-signed account.

use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, TransferChecked};

use crate::constants::CONFIG_SEED;
use crate::state::Config;

/// Moves `amount` from a `Config`-owned token account `from` to `to`; zero is a no-op.
pub fn pay_from_program<'info>(
    token_program: &Program<'info, Token>,
    mint: &Account<'info, Mint>,
    config: &Account<'info, Config>,
    from: &Account<'info, TokenAccount>,
    to: &Account<'info, TokenAccount>,
    amount: u64,
) -> Result<()> {
    pay_from_program_to(
        token_program,
        mint,
        config,
        from,
        to.to_account_info(),
        amount,
    )
}

/// `pay_from_program` to an unchecked `to` (an account taken from
/// `remaining_accounts`, checked by the caller); zero is a no-op.
pub fn pay_from_program_to<'info>(
    token_program: &Program<'info, Token>,
    mint: &Account<'info, Mint>,
    config: &Account<'info, Config>,
    from: &Account<'info, TokenAccount>,
    to: AccountInfo<'info>,
    amount: u64,
) -> Result<()> {
    if amount == 0 {
        return Ok(());
    }
    let seeds: &[&[u8]] = &[CONFIG_SEED, &[config.bump]];
    token::transfer_checked(
        CpiContext::new_with_signer(
            token_program.key(),
            TransferChecked {
                from: from.to_account_info(),
                mint: mint.to_account_info(),
                to,
                authority: config.to_account_info(),
            },
            &[seeds],
        ),
        amount,
        mint.decimals,
    )
}

/// Moves `amount` from `from` (owned by the signer `authority`) to `to`.
pub fn pay_from_user<'info>(
    token_program: &Program<'info, Token>,
    mint: &Account<'info, Mint>,
    from: &Account<'info, TokenAccount>,
    to: &Account<'info, TokenAccount>,
    authority: AccountInfo<'info>,
    amount: u64,
) -> Result<()> {
    token::transfer_checked(
        CpiContext::new(
            token_program.key(),
            TransferChecked {
                from: from.to_account_info(),
                mint: mint.to_account_info(),
                to: to.to_account_info(),
                authority,
            },
        ),
        amount,
        mint.decimals,
    )
}
