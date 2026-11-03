//! `open_pending(nonce, min_payout_per_unit_u, expires_at, is_tip, copied_from)`: the
//! user escrows the stake of a desk-signed quote and stores the quote and their floor.
//!
//! The quote is not an instruction argument: it is the message of the
//! `Ed25519Program` verify instruction right before this one (`sigverify`), decoded
//! here as a borsh `QuoteMsg`. The signed bytes are carried once, untouched, so a
//! 5-leg quote fits in one transaction with room for wallet-prepended
//! compute-budget instructions, and the program acts on exactly what the desk signed.
//!
//! A copy (`copied_from = Some(original ticket)`, 2.F) escrows `stake + copy_fee`,
//! `copy_fee = ceil(stake × Config.copy_fee_bps / 10_000)`. The original is not read
//! here (the account list is frozen and a 5-leg copy has no room for another
//! account): `finalize` checks it against the original and sets the tipster.
//!
//! `payer` pays the Pending's rent and gets it back when the Pending closes (L8): the user
//! themself (pass the same key twice), or the desk's sponsor key when the desk relays a
//! sponsored transaction, so a user needs no SOL.

use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

use crate::constants::*;
use crate::error::SchedinaError;
use crate::events::PendingOpened;
use crate::math;
use crate::messages::{QuoteLeg, QuoteMsg};
use crate::sigverify::verified_message;
use crate::state::{Config, Pending};

#[event_cpi]
#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct OpenPending<'info> {
    /// Authority of `user_usdc`; pays nothing (L8: `payer` pays the rent).
    pub user: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    #[account(
        init,
        payer = payer,
        space = 8 + Pending::INIT_SPACE,
        seeds = [PENDING_SEED, user.key().as_ref(), &nonce.to_le_bytes()],
        bump,
    )]
    pub pending: Box<Account<'info, Pending>>,

    /// The user's USDC. No mint account in this instruction (it saves 33 bytes of a
    /// tight transaction): SPL Token's `transfer` into the escrow, a mock-USDC account,
    /// fails for any other mint.
    #[account(mut, token::authority = user)]
    pub user_usdc: Box<Account<'info, TokenAccount>>,

    #[account(mut, seeds = [ESCROW_USDC_SEED], bump)]
    pub escrow_usdc: Box<Account<'info, TokenAccount>>,

    /// CHECK: the instructions sysvar (address-checked), read by `sigverify`.
    #[account(address = INSTRUCTIONS_SYSVAR_ID)]
    pub instructions: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,

    /// Pays the Pending's rent, recorded as `Pending.rent_payer` (L8). Last before the
    /// event-CPI accounts, so the earlier account indices are unchanged.
    #[account(mut)]
    pub payer: Signer<'info>,
}

fn check_legs(legs: &[QuoteLeg], max_legs: u8) -> Result<()> {
    let n = legs.len();
    require!(
        n >= 1 && n <= usize::from(max_legs) && n <= MAX_LEGS,
        SchedinaError::TooManyLegs
    );
    for (i, leg) in legs.iter().enumerate() {
        require!(
            leg.side <= SIDE_SHORT
                && leg.price_u > 0
                && leg.price_u <= ONE
                && u64::from(leg.m1_ppm) < ONE,
            SchedinaError::InvalidLeg
        );
        require!(
            legs[..i].iter().all(|l| l.game_id_hash != leg.game_id_hash),
            SchedinaError::DuplicateGame
        );
    }
    Ok(())
}

pub fn handler(
    mut ctx: Context<OpenPending>,
    nonce: u64,
    min_payout_per_unit_u: u64,
    expires_at: i64,
    is_tip: bool,
    copied_from: Option<Pubkey>,
) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let a = &mut ctx.accounts;
    let cfg = &a.config;
    require!(!cfg.paused, SchedinaError::Paused);

    // The desk signature first: nothing below is trusted until it holds.
    let (msg, quote_sig) = verified_message(&a.instructions, &cfg.quote_signer)?;
    // `try_from_slice` rejects trailing bytes; the borsh encoding of these types is
    // canonical, so `msg` is exactly `borsh(quote)`.
    let quote =
        QuoteMsg::try_from_slice(&msg).map_err(|_| error!(SchedinaError::QuoteMalformed))?;
    require!(quote.tag == QUOTE_TAG, SchedinaError::BadTag);
    require!(quote.program_id == crate::ID, SchedinaError::WrongProgramId);
    require!(now <= quote.quote_expires_at, SchedinaError::QuoteExpired);

    check_legs(&quote.legs, cfg.max_legs)?;
    require!(quote.stake_u >= cfg.min_stake, SchedinaError::StakeTooSmall);
    require!(
        min_payout_per_unit_u > 0 && min_payout_per_unit_u <= quote.odds_offered_u,
        SchedinaError::FloorAboveOffered
    );
    let quoted_payout =
        u128::from(quote.stake_u) * u128::from(quote.odds_offered_u) / u128::from(ONE);
    require!(
        quoted_payout <= u128::from(cfg.max_payout),
        SchedinaError::PayoutTooLarge
    );
    let lo = now
        .checked_add(PENDING_TTL_MIN)
        .ok_or(SchedinaError::MathOverflow)?;
    let hi = now
        .checked_add(PENDING_TTL_MAX)
        .ok_or(SchedinaError::MathOverflow)?;
    require!(
        expires_at >= lo && expires_at <= hi,
        SchedinaError::BadExpiry
    );
    // A tip may hold legs already in play, like any schedina: the desk's signature is what
    // decides a leg is still on offer (D-660). Only a copy cannot be a tip.
    require!(
        !(is_tip && copied_from.is_some()),
        SchedinaError::TipNotAllowed
    );

    let copy_fee_u = if copied_from.is_some() {
        math::copy_fee(quote.stake_u, cfg.copy_fee_bps).ok_or(SchedinaError::MathOverflow)?
    } else {
        0
    };
    let escrow_u = quote
        .stake_u
        .checked_add(copy_fee_u)
        .ok_or(SchedinaError::MathOverflow)?;

    token::transfer(
        CpiContext::new(
            a.token_program.key(),
            Transfer {
                from: a.user_usdc.to_account_info(),
                to: a.escrow_usdc.to_account_info(),
                authority: a.user.to_account_info(),
            },
        ),
        escrow_u,
    )?;

    let mut legs = [QuoteLeg::default(); MAX_LEGS];
    legs[..quote.legs.len()].copy_from_slice(&quote.legs);
    let quote_hash = solana_sha256_hasher::hash(&msg).to_bytes();
    let user = a.user.key();
    let p = &mut a.pending;
    p.user = user;
    p.nonce = nonce;
    p.stake_u = quote.stake_u;
    p.copy_fee_u = copy_fee_u;
    p.n = quote.legs.len() as u8;
    p.legs = legs;
    p.odds_offered_u = quote.odds_offered_u;
    p.min_payout_per_unit_u = min_payout_per_unit_u;
    p.quote_expires_at = quote.quote_expires_at;
    p.quote_hash = quote_hash;
    p.quote_sig = quote_sig;
    p.expires_at = expires_at;
    p.created_at = now;
    p.is_tip = is_tip;
    p.tipster = is_tip.then_some(user);
    p.copied_from = copied_from;
    p.bump = ctx.bumps.pending;
    p.rent_payer = a.payer.key();

    let event = PendingOpened {
        pending: p.key(),
        user,
        nonce,
        legs: quote.legs,
        stake_u: quote.stake_u,
        copy_fee_u,
        odds_offered_u: quote.odds_offered_u,
        min_payout_per_unit_u,
        quote_expires_at: quote.quote_expires_at,
        expires_at,
        quote_hash,
        is_tip,
        copied_from,
    };
    emit_cpi!(event);
    Ok(())
}
