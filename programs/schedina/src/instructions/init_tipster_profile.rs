//! `init_tipster_profile()`: anyone creates the `TipsterProfile` of `wallet` (PDA
//! `["tipster", wallet]`), paying its rent; a no-op when it exists. Optional: `finalize`
//! creates the profile of an own tip itself. It lets the crank or the app pre-create
//! one, and it puts `TipsterProfile` in the IDL (D-053).

use anchor_lang::prelude::*;

use crate::constants::*;
use crate::state::TipsterProfile;

#[derive(Accounts)]
pub struct InitTipsterProfile<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    /// CHECK: any wallet; only its address is used (the PDA seed and `wallet` field).
    pub wallet: UncheckedAccount<'info>,

    #[account(
        init_if_needed,
        payer = payer,
        space = 8 + TipsterProfile::INIT_SPACE,
        seeds = [TIPSTER_SEED, wallet.key().as_ref()],
        bump,
    )]
    pub tipster_profile: Box<Account<'info, TipsterProfile>>,

    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<InitTipsterProfile>) -> Result<()> {
    let wallet = ctx.accounts.wallet.key();
    let profile = &mut ctx.accounts.tipster_profile;
    // A fresh account is all zeros; an existing one already carries `wallet` (the PDA
    // binds the address to it), so this only fills a new profile.
    if profile.wallet != wallet {
        profile.wallet = wallet;
        profile.bump = ctx.bumps.tipster_profile;
    }
    Ok(())
}
