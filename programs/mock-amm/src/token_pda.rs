//! SPL mints and token accounts at PDAs, created with direct System + SPL Token CPIs
//! (no Anchor `mint::`/`token::` init constraints, which pull in Token-2022 code and cost
//! program size; schedina's pattern).

use anchor_lang::prelude::*;
use anchor_lang::system_program::{self, Allocate, Assign, CreateAccount, Transfer};
use anchor_spl::token::{self, InitializeAccount3, InitializeMint2, Mint, TokenAccount};

/// Creates PDA `target` (signed by `seeds`) with `space` bytes owned by SPL Token, paid by
/// `payer`. Tolerates lamports sent to the address beforehand.
fn create<'info>(
    payer: &AccountInfo<'info>,
    target: &AccountInfo<'info>,
    system_program: &AccountInfo<'info>,
    space: usize,
    seeds: &[&[u8]],
) -> Result<()> {
    let sys = system_program.key();
    let rent = Rent::get()?.minimum_balance(space);
    let signer: &[&[&[u8]]] = &[seeds];
    if target.lamports() == 0 {
        return system_program::create_account(
            CpiContext::new_with_signer(
                sys,
                CreateAccount {
                    from: payer.clone(),
                    to: target.clone(),
                },
                signer,
            ),
            rent,
            space as u64,
            &token::ID,
        );
    }
    let top_up = rent.saturating_sub(target.lamports());
    if top_up > 0 {
        system_program::transfer(
            CpiContext::new(
                sys,
                Transfer {
                    from: payer.clone(),
                    to: target.clone(),
                },
            ),
            top_up,
        )?;
    }
    system_program::allocate(
        CpiContext::new_with_signer(
            sys,
            Allocate {
                account_to_allocate: target.clone(),
            },
            signer,
        ),
        space as u64,
    )?;
    system_program::assign(
        CpiContext::new_with_signer(
            sys,
            Assign {
                account_to_assign: target.clone(),
            },
            signer,
        ),
        &token::ID,
    )
}

/// An SPL mint at PDA `target` (`seeds`), `decimals`, mint authority `authority`, no
/// freeze authority.
pub fn create_mint<'info>(
    payer: &AccountInfo<'info>,
    target: &AccountInfo<'info>,
    authority: &Pubkey,
    decimals: u8,
    system_program: &AccountInfo<'info>,
    token_program: &AccountInfo<'info>,
    seeds: &[&[u8]],
) -> Result<()> {
    create(payer, target, system_program, Mint::LEN, seeds)?;
    token::initialize_mint2(
        CpiContext::new(
            token_program.key(),
            InitializeMint2 {
                mint: target.clone(),
            },
        ),
        decimals,
        authority,
        None,
    )
}

/// A token account of `mint` at PDA `target` (`seeds`), owner `authority`.
#[allow(clippy::too_many_arguments)]
pub fn create_token_account<'info>(
    payer: &AccountInfo<'info>,
    target: &AccountInfo<'info>,
    mint: &AccountInfo<'info>,
    authority: &AccountInfo<'info>,
    system_program: &AccountInfo<'info>,
    token_program: &AccountInfo<'info>,
    seeds: &[&[u8]],
) -> Result<()> {
    create(payer, target, system_program, TokenAccount::LEN, seeds)?;
    token::initialize_account3(CpiContext::new(
        token_program.key(),
        InitializeAccount3 {
            account: target.clone(),
            mint: mint.clone(),
            authority: authority.clone(),
        },
    ))
}
