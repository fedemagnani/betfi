//! `set_params(params)` and `pause(paused)`: authority only. Both emit `ConfigUpdated`.

use anchor_lang::prelude::*;

use super::init_config::Params;
use crate::constants::CONFIG_SEED;
use crate::error::SchedinaError;
use crate::events::ConfigUpdated;
use crate::state::Config;

#[event_cpi]
#[derive(Accounts)]
pub struct SetParams<'info> {
    pub authority: Signer<'info>,

    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = authority @ SchedinaError::NotAuthority,
    )]
    pub config: Box<Account<'info, Config>>,
}

pub fn updated(c: &Config) -> ConfigUpdated {
    ConfigUpdated {
        crank: c.crank,
        quote_signer: c.quote_signer,
        max_legs: c.max_legs,
        max_payout: c.max_payout,
        min_stake: c.min_stake,
        reserve: c.reserve,
        copy_fee_bps: c.copy_fee_bps,
        tipster_share_bps: c.tipster_share_bps,
        paused: c.paused,
    }
}

pub fn handler(ctx: Context<SetParams>, params: Params) -> Result<()> {
    params.validate()?;
    params.apply(&mut ctx.accounts.config);
    let event = updated(&ctx.accounts.config);
    emit_cpi!(event);
    Ok(())
}

/// `pause(paused)`: same accounts as `set_params`.
pub fn pause_handler(ctx: Context<SetParams>, paused: bool) -> Result<()> {
    ctx.accounts.config.paused = paused;
    let event = updated(&ctx.accounts.config);
    emit_cpi!(event);
    Ok(())
}
