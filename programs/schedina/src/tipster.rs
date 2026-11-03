//! Tipster bookkeeping and copy checks (step 2.F, contract L2-4).
//!
//! Every account here arrives through `remaining_accounts`, so the declared account
//! lists of `finalize`, `settle` and `cash_out` stay as in 1.C. Each one is checked by
//! hand: address (PDA or the pubkey stored on the pending), owner program,
//! discriminator, writability, and for token accounts mint and token owner.
//!
//! `TipsterProfile` (PDA `["tipster", wallet]`) counts only the wallet's own tips
//! (`ticket.tipster == Some(ticket.user)`, `copied_from == None`): `finalize` adds the
//! ticket to the `open_*` fields, `settle`/`cash_out` move it to the `settled_*`
//! fields. Copies never touch a profile; the tipster's copy-fee income is the sum of
//! `TipsterPaid` events.

use anchor_lang::prelude::*;
use anchor_lang::system_program::{self, Allocate, Assign, CreateAccount, Transfer};
use anchor_spl::token::{self, TokenAccount};

use crate::constants::*;
use crate::error::SchedinaError;
use crate::state::{LegStatus, Pending, Ticket, TicketState, TipsterProfile};

/// Space of a `TipsterProfile` account, discriminator included.
pub const PROFILE_SPACE: usize = 8 + TipsterProfile::INIT_SPACE;

/// An own tip: flagged at `open_pending` by its user, not a copy (L2-4).
pub fn is_own_tip(tipster: Option<Pubkey>, user: &Pubkey, copied_from: Option<Pubkey>) -> bool {
    copied_from.is_none() && tipster.as_ref() == Some(user)
}

fn profile_err() -> Error {
    error!(SchedinaError::BadTipsterProfile)
}

/// Reads an existing profile of `wallet` from `ai`: writable, owned by this program,
/// `TipsterProfile` discriminator, `wallet` field, and at the PDA of its stored bump.
fn read_profile(ai: &AccountInfo, wallet: &Pubkey) -> Result<TipsterProfile> {
    require!(
        ai.is_writable && ai.owner == &crate::ID,
        SchedinaError::BadTipsterProfile
    );
    let profile = {
        let data = ai.try_borrow_data()?;
        TipsterProfile::try_deserialize(&mut &data[..]).map_err(|_| profile_err())?
    };
    require!(profile.wallet == *wallet, SchedinaError::BadTipsterProfile);
    let pda = Pubkey::create_program_address(
        &[TIPSTER_SEED, wallet.as_ref(), &[profile.bump]],
        &crate::ID,
    )
    .map_err(|_| profile_err())?;
    require!(pda == ai.key(), SchedinaError::BadTipsterProfile);
    Ok(profile)
}

fn write_profile(ai: &AccountInfo, profile: &TipsterProfile) -> Result<()> {
    let mut data = ai.try_borrow_mut_data()?;
    let mut dst: &mut [u8] = &mut data;
    profile.try_serialize(&mut dst)
}

/// `remaining_accounts[0]` as the profile of `wallet`, for `settle` and `cash_out`.
/// Missing or wrong: `BadTipsterProfile` (an own tip cannot leave the record).
pub fn require_profile<'a, 'info>(
    remaining: &'a [AccountInfo<'info>],
    wallet: &Pubkey,
) -> Result<(&'a AccountInfo<'info>, TipsterProfile)> {
    let ai = remaining.first().ok_or_else(profile_err)?;
    let profile = read_profile(ai, wallet)?;
    Ok((ai, profile))
}

/// `remaining_accounts[0]` as the profile of `wallet` for `finalize`, created (rent
/// paid by `payer`) when the PDA holds no account yet.
pub fn profile_for_finalize<'info>(
    remaining: &[AccountInfo<'info>],
    wallet: &Pubkey,
    payer: &AccountInfo<'info>,
    system: &AccountInfo<'info>,
) -> Result<(AccountInfo<'info>, TipsterProfile)> {
    let ai = remaining.first().ok_or_else(profile_err)?;
    if ai.owner == &crate::ID {
        let profile = read_profile(ai, wallet)?;
        return Ok((ai.clone(), profile));
    }
    // Not created yet: it must be the system-owned, empty PDA.
    let (pda, bump) = Pubkey::find_program_address(&[TIPSTER_SEED, wallet.as_ref()], &crate::ID);
    require!(
        ai.key() == pda && ai.is_writable && ai.owner == &system_program::ID && ai.data_is_empty(),
        SchedinaError::BadTipsterProfile
    );
    let seeds: &[&[u8]] = &[TIPSTER_SEED, wallet.as_ref(), &[bump]];
    let rent = Rent::get()?.minimum_balance(PROFILE_SPACE);
    let lamports = ai.lamports();
    if lamports == 0 {
        system_program::create_account(
            CpiContext::new_with_signer(
                system.key(),
                CreateAccount {
                    from: payer.clone(),
                    to: ai.clone(),
                },
                &[seeds],
            ),
            rent,
            PROFILE_SPACE as u64,
            &crate::ID,
        )?;
    } else {
        // Someone pre-funded the address (create_account would fail): top up, then
        // allocate and assign, as Anchor's `init` does.
        if lamports < rent {
            system_program::transfer(
                CpiContext::new(
                    system.key(),
                    Transfer {
                        from: payer.clone(),
                        to: ai.clone(),
                    },
                ),
                rent - lamports,
            )?;
        }
        system_program::allocate(
            CpiContext::new_with_signer(
                system.key(),
                Allocate {
                    account_to_allocate: ai.clone(),
                },
                &[seeds],
            ),
            PROFILE_SPACE as u64,
        )?;
        system_program::assign(
            CpiContext::new_with_signer(
                system.key(),
                Assign {
                    account_to_assign: ai.clone(),
                },
                &[seeds],
            ),
            &crate::ID,
        )?;
    }
    let profile = TipsterProfile {
        wallet: *wallet,
        open_count: 0,
        open_staked: 0,
        settled_count: 0,
        won: 0,
        settled_staked: 0,
        returned: 0,
        fees_earned: 0,
        bump,
        reserved: [0; 32],
    };
    Ok((ai.clone(), profile))
}

/// `finalize` of an own tip: the ticket (stake `rs_u`) joins the open fields.
pub fn record_open(ai: &AccountInfo, mut profile: TipsterProfile, rs_u: u64) -> Result<()> {
    profile.open_count = profile
        .open_count
        .checked_add(1)
        .ok_or(SchedinaError::MathOverflow)?;
    profile.open_staked = profile
        .open_staked
        .checked_add(rs_u)
        .ok_or(SchedinaError::MathOverflow)?;
    write_profile(ai, &profile)
}

/// `settle` / `cash_out` of an own tip: the ticket leaves the open fields and enters
/// the scored ones with `returned_u` (the settle payment or `C`); `won` counts tickets
/// that returned more than their stake. The `open_*` decrements saturate so a ticket
/// can always leave `Open` (exits never block on bookkeeping).
pub fn record_settled(
    ai: &AccountInfo,
    mut profile: TipsterProfile,
    stake_u: u64,
    returned_u: u64,
) -> Result<()> {
    profile.open_count = profile.open_count.saturating_sub(1);
    profile.open_staked = profile.open_staked.saturating_sub(stake_u);
    profile.settled_count = profile
        .settled_count
        .checked_add(1)
        .ok_or(SchedinaError::MathOverflow)?;
    if returned_u > stake_u {
        profile.won = profile
            .won
            .checked_add(1)
            .ok_or(SchedinaError::MathOverflow)?;
    }
    profile.settled_staked = profile
        .settled_staked
        .checked_add(stake_u)
        .ok_or(SchedinaError::MathOverflow)?;
    profile.returned = profile
        .returned
        .checked_add(returned_u)
        .ok_or(SchedinaError::MathOverflow)?;
    write_profile(ai, &profile)
}

/// The original ticket of a copy and the tipster's USDC account, from
/// `remaining_accounts = [orig_ticket (r), tipster_usdc (w)]`, checked against the
/// pending. Returns the tipster (the original's user).
pub fn check_copy<'info>(
    remaining: &[AccountInfo<'info>],
    p: &Pending,
    orig_key: &Pubkey,
    usdc_mint: &Pubkey,
) -> Result<(Pubkey, AccountInfo<'info>)> {
    let [orig_ai, tipster_usdc, ..] = remaining else {
        return err!(SchedinaError::BadCopyAccounts);
    };
    require!(
        orig_ai.key() == *orig_key && orig_ai.owner == &crate::ID,
        SchedinaError::BadCopyAccounts
    );
    let orig = {
        let data = orig_ai.try_borrow_data()?;
        Ticket::try_deserialize(&mut &data[..])
            .map_err(|_| error!(SchedinaError::BadCopyAccounts))?
    };
    // Defence in depth: only `finalize` writes Ticket accounts, always at this PDA.
    let pda = Pubkey::create_program_address(
        &[
            TICKET_SEED,
            orig.user.as_ref(),
            &orig.nonce.to_le_bytes(),
            &[orig.bump],
        ],
        &crate::ID,
    )
    .map_err(|_| error!(SchedinaError::BadCopyAccounts))?;
    require!(pda == *orig_key, SchedinaError::BadCopyAccounts);

    require!(
        orig.state == TicketState::Open && is_own_tip(orig.tipster, &orig.user, orig.copied_from),
        SchedinaError::OriginalNotCopyable
    );
    // A leg in play can still be copied, at the copier's own live prices (D-660); a decided
    // one cannot.
    let orig_legs = &orig.legs[..usize::from(orig.n)];
    require!(
        orig_legs.iter().all(|l| l.status == LegStatus::Open),
        SchedinaError::CopyTooLate
    );
    // Same legs as a set (game ids are unique within a ticket): same count, and every
    // leg of the copy matches the original's leg on that game in symbol and side.
    require!(p.n == orig.n, SchedinaError::CopyMismatch);
    for q in &p.legs[..usize::from(p.n)] {
        require!(
            orig_legs.iter().any(|o| o.game_id_hash == q.game_id_hash
                && o.symbol_hash == q.symbol_hash
                && o.side == q.side),
            SchedinaError::CopyMismatch
        );
    }

    // The fee goes only to a USDC account owned by the original's user (D-005).
    require!(
        tipster_usdc.is_writable && tipster_usdc.owner == &token::ID,
        SchedinaError::BadCopyAccounts
    );
    let acc = {
        let data = tipster_usdc.try_borrow_data()?;
        TokenAccount::try_deserialize(&mut &data[..])
            .map_err(|_| error!(SchedinaError::BadCopyAccounts))?
    };
    require!(
        acc.mint == *usdc_mint && acc.owner == orig.user,
        SchedinaError::BadCopyAccounts
    );
    Ok((orig.user, tipster_usdc.clone()))
}
