//! `cash_out(quote)`: the ticket's user takes the desk-signed cash-out value `C`
//! early. Verifies the desk's ed25519 signature over the borsh bytes of `quote`
//! (`ticket`, `c_u`, `ts`), its freshness, `C <= P` and that no leg is lost; pays `C`
//! from the vault, returns `P - C` to `Vault.free`, marks the ticket `CashedOut`.
//! A quote can be used once: the ticket leaves `Open`. An own tip (2.F) needs
//! `remaining_accounts = [tipster_profile (w)]` and moves to its `settled_*` fields
//! with `returned = C`.

use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token, TokenAccount};

use crate::constants::*;
use crate::error::SchedinaError;
use crate::events::TicketCashedOut;
use crate::messages::CashOutMsg;
use crate::sigverify::verified_message;
use crate::state::{Config, LegStatus, Ticket, TicketState, Vault};
use crate::tipster;
use crate::vault::pay_from_program;

#[event_cpi]
#[derive(Accounts)]
pub struct CashOut<'info> {
    #[account(address = ticket.user)]
    pub user: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump, has_one = usdc_mint)]
    pub config: Box<Account<'info, Config>>,

    #[account(mut, seeds = [VAULT_SEED], bump = vault.bump)]
    pub vault: Box<Account<'info, Vault>>,

    #[account(
        mut,
        seeds = [TICKET_SEED, ticket.user.as_ref(), &ticket.nonce.to_le_bytes()],
        bump = ticket.bump,
    )]
    pub ticket: Box<Account<'info, Ticket>>,

    #[account(mut, token::mint = usdc_mint, token::authority = ticket.user)]
    pub user_usdc: Box<Account<'info, TokenAccount>>,

    pub usdc_mint: Box<Account<'info, Mint>>,

    #[account(mut, address = vault.usdc_ata)]
    pub vault_usdc: Box<Account<'info, TokenAccount>>,

    /// CHECK: the instructions sysvar (address-checked), read by `sigverify`.
    #[account(address = INSTRUCTIONS_SYSVAR_ID)]
    pub instructions: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(mut ctx: Context<CashOut>, quote: CashOutMsg) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let a = &mut ctx.accounts;

    require!(quote.tag == CASHOUT_TAG, SchedinaError::BadTag);
    require!(quote.program_id == crate::ID, SchedinaError::WrongProgramId);
    let mut msg = Vec::with_capacity(99);
    quote.serialize(&mut msg)?;
    let (signed, _) = verified_message(&a.instructions, &a.config.quote_signer)?;
    require!(signed == msg, SchedinaError::QuoteMismatch);
    require!(quote.ticket == a.ticket.key(), SchedinaError::WrongTicket);
    let age = now
        .checked_sub(quote.ts)
        .ok_or(SchedinaError::CashOutStale)?;
    require!(
        (-CASHOUT_TTL..=CASHOUT_TTL).contains(&age),
        SchedinaError::CashOutStale
    );

    let t = &a.ticket;
    require!(t.state == TicketState::Open, SchedinaError::TicketNotOpen);
    require!(quote.c_u <= t.payout_u, SchedinaError::CashOutAbovePayout);
    require!(
        t.legs[..usize::from(t.n)]
            .iter()
            .all(|l| l.status != LegStatus::Lost),
        SchedinaError::LegLost
    );

    // An own tip cannot leave the record (L2-4): its profile is required.
    let profile = if tipster::is_own_tip(t.tipster, &t.user, t.copied_from) {
        Some(tipster::require_profile(ctx.remaining_accounts, &t.user)?)
    } else {
        None
    };

    let payout_u = t.payout_u;
    let c_u = quote.c_u;
    a.vault.locked = a
        .vault
        .locked
        .checked_sub(payout_u)
        .ok_or(SchedinaError::MathOverflow)?;
    a.vault.free = a
        .vault
        .free
        .checked_add(payout_u - c_u)
        .ok_or(SchedinaError::MathOverflow)?;
    pay_from_program(
        &a.token_program,
        &a.usdc_mint,
        &a.config,
        &a.vault_usdc,
        &a.user_usdc,
        c_u,
    )?;
    if let Some((ai, p)) = profile {
        tipster::record_settled(ai, p, a.ticket.stake_u, c_u)?;
    }
    let t = &mut a.ticket;
    t.state = TicketState::CashedOut;
    t.paid_u = c_u;
    t.resolved_at = now;
    let event = TicketCashedOut {
        ticket: t.key(),
        user: t.user,
        payout_u,
        c_u,
        ts: quote.ts,
        vault_locked: a.vault.locked,
        vault_free: a.vault.free,
    };
    emit_cpi!(event);
    Ok(())
}
