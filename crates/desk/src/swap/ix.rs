//! Mock AMM instruction builders, in the IDL's account order with its signer and writable
//! flags (checked against `programs/idl/mock_amm.json` and `fixtures/swap/swap_ix.json`), plus
//! the address lookup table program's `CreateLookupTable` / `ExtendLookupTable`.

use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use super::AmmPdas;
use crate::program::{
    BPF_UPGRADEABLE_LOADER_ID, INSTRUCTIONS_SYSVAR_ID, Pdas, SYSTEM_PROGRAM_ID, TOKEN_PROGRAM_ID,
    associated_token_address,
};

/// Address lookup table program.
pub const ALT_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("AddressLookupTab1e1111111111111111111111111");

/// Anchor instruction discriminators (IDL `instructions[].discriminator`).
pub mod disc {
    /// `init_amm`.
    pub const INIT_AMM: [u8; 8] = [86, 210, 92, 250, 251, 155, 189, 27];
    /// `set_lookup_table`.
    pub const SET_LOOKUP_TABLE: [u8; 8] = [235, 249, 109, 59, 171, 208, 117, 65];
    /// `init_mock_mint`.
    pub const INIT_MOCK_MINT: [u8; 8] = [70, 216, 190, 134, 84, 23, 160, 198];
    /// `init_pool`.
    pub const INIT_POOL: [u8; 8] = [116, 233, 199, 204, 115, 159, 171, 36];
    /// `add_liquidity`.
    pub const ADD_LIQUIDITY: [u8; 8] = [181, 157, 89, 67, 143, 182, 52, 72];
    /// `swap`.
    pub const SWAP: [u8; 8] = [248, 198, 158, 145, 225, 117, 135, 200];
    /// `faucet`.
    pub const FAUCET: [u8; 8] = [0, 98, 59, 30, 144, 142, 113, 12];
}

fn w(k: Pubkey) -> AccountMeta {
    AccountMeta::new(k, false)
}

fn r(k: Pubkey) -> AccountMeta {
    AccountMeta::new_readonly(k, false)
}

fn data(d: [u8; 8], args: &[&[u8]]) -> Vec<u8> {
    let mut v = d.to_vec();
    for a in args {
        v.extend_from_slice(a);
    }
    v
}

/// `swap(amount_in, min_out)` on `mint`'s pool. `sell`: `user_in` holds `mint` and `user_out`
/// receives USDC; otherwise USDC in, `mint` out. The user signs; any output account of the
/// right mint is accepted by the program.
#[allow(clippy::too_many_arguments)]
pub fn swap(
    p: &AmmPdas,
    user: &Pubkey,
    mint: &Pubkey,
    user_in: &Pubkey,
    user_out: &Pubkey,
    sell: bool,
    amount_in: u64,
    min_out: u64,
) -> Instruction {
    let pool = p.pool(mint);
    let (vm, vu) = (p.vault(&pool, mint), p.vault(&pool, &p.usdc_mint));
    let (vault_in, vault_out) = if sell { (vm, vu) } else { (vu, vm) };
    Instruction {
        program_id: p.program,
        accounts: vec![
            AccountMeta::new_readonly(*user, true),
            w(pool),
            w(*user_in),
            w(*user_out),
            w(vault_in),
            w(vault_out),
            r(TOKEN_PROGRAM_ID),
        ],
        data: data(
            disc::SWAP,
            &[&amount_in.to_le_bytes(), &min_out.to_le_bytes()],
        ),
    }
}

/// [`swap`] selling `amount_in` of `mint` from `user`'s ATA of it into `user`'s mock-USDC ATA:
/// the instruction of the app's `[swap, Ed25519 verify, open_pending]`.
pub fn sell_to_usdc(
    p: &AmmPdas,
    user: &Pubkey,
    mint: &Pubkey,
    amount_in: u64,
    min_out: u64,
) -> Instruction {
    swap(
        p,
        user,
        mint,
        &associated_token_address(user, mint),
        &associated_token_address(user, &p.usdc_mint),
        true,
        amount_in,
        min_out,
    )
}

/// `faucet(amount)` of `mint` into `owner`'s ATA of it, signed by `caller` (the hourly claim is
/// the caller's); `payer` pays the claim's rent (L8: `caller` itself, or the desk's sponsor).
pub fn faucet(
    p: &AmmPdas,
    caller: &Pubkey,
    payer: &Pubkey,
    mint: &Pubkey,
    owner: &Pubkey,
    amount: u64,
) -> Instruction {
    Instruction {
        program_id: p.program,
        accounts: vec![
            AccountMeta::new_readonly(*caller, true),
            r(p.amm_config()),
            r(p.mock_token(mint)),
            w(*mint),
            w(associated_token_address(owner, mint)),
            w(p.faucet_claim(mint, caller)),
            r(TOKEN_PROGRAM_ID),
            r(SYSTEM_PROGRAM_ID),
            AccountMeta::new(*payer, true),
        ],
        data: data(disc::FAUCET, &[&amount.to_le_bytes()]),
    }
}

/// `init_amm(usdc_mint)`, signed by the program's upgrade authority.
pub fn init_amm(p: &AmmPdas, admin: &Pubkey) -> Instruction {
    let program_data =
        Pubkey::find_program_address(&[p.program.as_ref()], &BPF_UPGRADEABLE_LOADER_ID).0;
    Instruction {
        program_id: p.program,
        accounts: vec![
            AccountMeta::new(*admin, true),
            w(p.amm_config()),
            r(p.program),
            r(program_data),
            r(SYSTEM_PROGRAM_ID),
        ],
        data: data(disc::INIT_AMM, &[p.usdc_mint.as_ref()]),
    }
}

/// `init_mock_mint(symbol, decimals, faucet_max)`, admin.
pub fn init_mock_mint(
    p: &AmmPdas,
    admin: &Pubkey,
    symbol: &str,
    decimals: u8,
    faucet_max: u64,
) -> Instruction {
    let mint = p.mock_mint(symbol);
    let len = u32::try_from(symbol.len()).unwrap_or(u32::MAX);
    Instruction {
        program_id: p.program,
        accounts: vec![
            AccountMeta::new(*admin, true),
            r(p.amm_config()),
            w(mint),
            w(p.mock_token(&mint)),
            r(TOKEN_PROGRAM_ID),
            r(SYSTEM_PROGRAM_ID),
        ],
        data: data(
            disc::INIT_MOCK_MINT,
            &[
                &len.to_le_bytes(),
                symbol.as_bytes(),
                &[decimals],
                &faucet_max.to_le_bytes(),
            ],
        ),
    }
}

/// `init_pool()` for `mint`, admin.
pub fn init_pool(p: &AmmPdas, admin: &Pubkey, mint: &Pubkey) -> Instruction {
    let pool = p.pool(mint);
    Instruction {
        program_id: p.program,
        accounts: vec![
            AccountMeta::new(*admin, true),
            r(p.amm_config()),
            r(p.mock_token(mint)),
            r(*mint),
            r(p.usdc_mint),
            w(pool),
            w(p.vault(&pool, mint)),
            w(p.vault(&pool, &p.usdc_mint)),
            r(TOKEN_PROGRAM_ID),
            r(SYSTEM_PROGRAM_ID),
        ],
        data: data(disc::INIT_POOL, &[]),
    }
}

/// `add_liquidity(mint_amount, max_usdc)` from `provider`'s ATAs.
pub fn add_liquidity(
    p: &AmmPdas,
    provider: &Pubkey,
    mint: &Pubkey,
    mint_amount: u64,
    max_usdc: u64,
) -> Instruction {
    let pool = p.pool(mint);
    Instruction {
        program_id: p.program,
        accounts: vec![
            AccountMeta::new_readonly(*provider, true),
            r(p.amm_config()),
            r(pool),
            w(associated_token_address(provider, mint)),
            w(associated_token_address(provider, &p.usdc_mint)),
            w(p.vault(&pool, mint)),
            w(p.vault(&pool, &p.usdc_mint)),
            r(TOKEN_PROGRAM_ID),
        ],
        data: data(
            disc::ADD_LIQUIDITY,
            &[&mint_amount.to_le_bytes(), &max_usdc.to_le_bytes()],
        ),
    }
}

/// `set_lookup_table()`, admin.
pub fn set_lookup_table(p: &AmmPdas, admin: &Pubkey, lookup_table: &Pubkey) -> Instruction {
    Instruction {
        program_id: p.program,
        accounts: vec![
            AccountMeta::new_readonly(*admin, true),
            w(p.amm_config()),
            r(*lookup_table),
        ],
        data: data(disc::SET_LOOKUP_TABLE, &[]),
    }
}

/// The static accounts of `[swap(mint), Ed25519 verify, open_pending]` for every pool in
/// `mints`, in the order the lookup table holds them: schedina's config, escrow, instructions
/// sysvar, token and system programs, schedina's event authority, the USDC mint, then per pool
/// the mint, pool and both vaults. Signers, the user's token accounts and invoked program ids
/// cannot come from a table.
pub fn lookup_table_addresses(s: &Pdas, p: &AmmPdas, mints: &[Pubkey]) -> Vec<Pubkey> {
    let mut v = vec![
        s.config(),
        s.escrow_usdc(),
        INSTRUCTIONS_SYSVAR_ID,
        TOKEN_PROGRAM_ID,
        SYSTEM_PROGRAM_ID,
        s.event_authority(),
        p.usdc_mint,
    ];
    for m in mints {
        let pool = p.pool(m);
        v.extend([*m, pool, p.vault(&pool, m), p.vault(&pool, &p.usdc_mint)]);
    }
    v
}

/// The lookup table `authority` creates at `recent_slot` (a slot in `SlotHashes`).
pub fn lookup_table_address(authority: &Pubkey, recent_slot: u64) -> Pubkey {
    Pubkey::find_program_address(
        &[authority.as_ref(), &recent_slot.to_le_bytes()],
        &ALT_PROGRAM_ID,
    )
    .0
}

/// ALT `CreateLookupTable { recent_slot, bump_seed }` (bincode: `u32` variant 0), with the
/// authority signing (accepted with or without the relaxed-signer feature).
pub fn create_lookup_table(authority: &Pubkey, payer: &Pubkey, recent_slot: u64) -> Instruction {
    let (table, bump) = Pubkey::find_program_address(
        &[authority.as_ref(), &recent_slot.to_le_bytes()],
        &ALT_PROGRAM_ID,
    );
    let mut d = 0u32.to_le_bytes().to_vec();
    d.extend_from_slice(&recent_slot.to_le_bytes());
    d.push(bump);
    Instruction {
        program_id: ALT_PROGRAM_ID,
        accounts: vec![
            w(table),
            AccountMeta::new_readonly(*authority, true),
            AccountMeta::new(*payer, true),
            r(SYSTEM_PROGRAM_ID),
        ],
        data: d,
    }
}

/// ALT `ExtendLookupTable { new_addresses }` (bincode: `u32` variant 2, `u64` length).
pub fn extend_lookup_table(
    table: &Pubkey,
    authority: &Pubkey,
    payer: &Pubkey,
    addresses: &[Pubkey],
) -> Instruction {
    let mut d = 2u32.to_le_bytes().to_vec();
    d.extend_from_slice(&(addresses.len() as u64).to_le_bytes());
    for a in addresses {
        d.extend_from_slice(a.as_ref());
    }
    Instruction {
        program_id: ALT_PROGRAM_ID,
        accounts: vec![
            w(*table),
            AccountMeta::new_readonly(*authority, true),
            AccountMeta::new(*payer, true),
            r(SYSTEM_PROGRAM_ID),
        ],
        data: d,
    }
}
