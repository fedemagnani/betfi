//! Desk signatures through the `Ed25519Program` precompile (L1 contract C3).
//!
//! Rule (position-independent, so wallets may prepend compute-budget or guard
//! instructions): the transaction is `[..., Ed25519Program verify, schedina ix]`. The
//! program takes its own index from the instructions sysvar and requires, at
//! `current - 1`, an `Ed25519Program` instruction with exactly one signature entry
//! whose three instruction-index fields are all `u16::MAX` (public key, signature
//! and message inside that same instruction, as solana's
//! `new_ed25519_instruction_with_signature` builds it). The runtime has verified that
//! signature before this instruction runs (a failing precompile aborts the
//! transaction), so the message returned here was signed by the returned key.

use anchor_lang::prelude::*;
use solana_instructions_sysvar::{load_current_index_checked, load_instruction_at_checked};

use crate::constants::ED25519_PROGRAM_ID;
use crate::error::SchedinaError;

/// Header of a single-signature entry: `num_signatures u8, padding u8`, then seven
/// `u16` LE: signature offset/ix, public key offset/ix, message offset/size/ix.
const HEADER: usize = 16;

fn u16_at(d: &[u8], at: usize) -> usize {
    usize::from(u16::from_le_bytes([d[at], d[at + 1]]))
}

fn get(d: &[u8], off: usize, len: usize) -> Result<&[u8]> {
    let end = off
        .checked_add(len)
        .ok_or(error!(SchedinaError::SignatureMalformed))?;
    d.get(off..end)
        .ok_or(error!(SchedinaError::SignatureMalformed))
}

/// Returns the message and signature of the `Ed25519Program` instruction right
/// before the current one, after checking its public key is `signer`.
///
/// Errors: `SignatureMissing` (the previous instruction is not `Ed25519Program`, or
/// there is none), `SignatureMalformed` (not exactly one signature, or an offset
/// pointing at another instruction), `WrongQuoteSigner` (another key).
pub fn verified_message(sysvar: &AccountInfo, signer: &Pubkey) -> Result<(Vec<u8>, [u8; 64])> {
    let current =
        load_current_index_checked(sysvar).map_err(|_| error!(SchedinaError::SignatureMissing))?;
    let prev = current
        .checked_sub(1)
        .ok_or(error!(SchedinaError::SignatureMissing))?;
    let ix = load_instruction_at_checked(usize::from(prev), sysvar)
        .map_err(|_| error!(SchedinaError::SignatureMissing))?;
    require_keys_eq!(
        ix.program_id,
        ED25519_PROGRAM_ID,
        SchedinaError::SignatureMissing
    );
    let d = &ix.data;
    require!(
        d.len() >= HEADER && d[0] == 1,
        SchedinaError::SignatureMalformed
    );
    let (sig_off, sig_ix) = (u16_at(d, 2), u16_at(d, 4));
    let (pk_off, pk_ix) = (u16_at(d, 6), u16_at(d, 8));
    let (msg_off, msg_len, msg_ix) = (u16_at(d, 10), u16_at(d, 12), u16_at(d, 14));
    let own = usize::from(u16::MAX);
    require!(
        sig_ix == own && pk_ix == own && msg_ix == own,
        SchedinaError::SignatureMalformed
    );
    require!(
        get(d, pk_off, 32)? == signer.as_ref(),
        SchedinaError::WrongQuoteSigner
    );
    let mut sig = [0u8; 64];
    sig.copy_from_slice(get(d, sig_off, 64)?);
    Ok((get(d, msg_off, msg_len)?.to_vec(), sig))
}
