//! `resolve_leg(leg_index, r_ppm)`: the crank (or the authority, for the spec's
//! "market removed without resolution" 0.5 case) records Pascal's resolution price `r`
//! of one leg's symbol. Once per leg; immutable afterwards.

use anchor_lang::prelude::*;

use crate::constants::*;
use crate::error::SchedinaError;
use crate::events::LegResolved;
use crate::math;
use crate::state::{Config, LegStatus, Ticket, TicketState};

#[event_cpi]
#[derive(Accounts)]
pub struct ResolveLeg<'info> {
    /// `Config.crank` or `Config.authority`.
    pub resolver: Signer<'info>,

    #[account(
        seeds = [CONFIG_SEED],
        bump = config.bump,
        constraint = resolver.key() == config.crank || resolver.key() == config.authority
            @ SchedinaError::NotResolver,
    )]
    pub config: Box<Account<'info, Config>>,

    #[account(
        mut,
        seeds = [TICKET_SEED, ticket.user.as_ref(), &ticket.nonce.to_le_bytes()],
        bump = ticket.bump,
    )]
    pub ticket: Box<Account<'info, Ticket>>,
}

pub fn handler(mut ctx: Context<ResolveLeg>, leg_index: u8, r_ppm: u32) -> Result<()> {
    let a = &mut ctx.accounts;
    let t = &mut a.ticket;
    require!(t.state == TicketState::Open, SchedinaError::TicketNotOpen);
    require!(leg_index < t.n, SchedinaError::BadLegIndex);
    require!(u64::from(r_ppm) <= ONE, SchedinaError::BadResolution);
    let leg = &mut t.legs[usize::from(leg_index)];
    require!(
        leg.status == LegStatus::Open,
        SchedinaError::LegAlreadyResolved
    );
    let rtilde_ppm = math::rtilde(leg.side, r_ppm);
    leg.resolution_ppm = r_ppm;
    leg.status = match u64::from(rtilde_ppm) {
        0 => LegStatus::Lost,
        ONE => LegStatus::Won,
        _ => LegStatus::Partial,
    };
    let event = LegResolved {
        ticket: t.key(),
        leg_index,
        r_ppm,
        rtilde_ppm,
        resolver: a.resolver.key(),
    };
    emit_cpi!(event);
    Ok(())
}
