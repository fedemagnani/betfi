//! `fixtures/swap/swap_ix.json` is the `swap` instruction exactly as Anchor's generated client
//! builds it (`mock_amm::instruction::Swap`, `mock_amm::accounts::Swap`) for fixed inputs:
//! the desk (`crates/desk/src/swap`) and the app (`app/tx/src/swap.rs`) test their builders
//! against it, and against `programs/idl/mock_amm.json`. `UPDATE_SWAP_FIXTURE=1` rewrites it.

mod common;

use anchor_lang::prelude::Pubkey;
use anchor_spl::token;
use common::*;
use serde_json::{json, Value};

const ATA_PROGRAM: Pubkey =
    anchor_lang::prelude::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

fn ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[owner.as_ref(), token::ID.as_ref(), mint.as_ref()],
        &ATA_PROGRAM,
    )
    .0
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn swap_instruction_vector() {
    let user: Pubkey = "J2xccRtuG43drESLYznHhLhQkLTdfepcKYbiQ9BsJVaf"
        .parse()
        .unwrap();
    let bonk = mock_mint("BONK");
    let (amount_in, min_out) = (1_000_000_000u64, 9_880_586u64);
    let p = pool(&bonk);
    let ix = ix(
        mock_amm::ID,
        mock_amm::instruction::Swap { amount_in, min_out },
        mock_amm::accounts::Swap {
            user,
            pool: p,
            user_in: ata(&user, &bonk),
            user_out: ata(&user, &usdc_mint()),
            vault_in: vault(&p, &bonk),
            vault_out: vault(&p, &usdc_mint()),
            token_program: token::ID,
        },
    );
    let v = json!({
        "about": "mock_amm swap, built by Anchor's generated client (programs/mock-amm/tests/swap_ix_vector.rs): sell amount_in mockBONK from the user's mockBONK ATA into the user's mock-USDC ATA. Regenerate with UPDATE_SWAP_FIXTURE=1.",
        "program_id": mock_amm::ID.to_string(),
        "schedina_program_id": schedina::ID.to_string(),
        "user": user.to_string(),
        "mint": bonk.to_string(),
        "usdc_mint": usdc_mint().to_string(),
        "amm_config": amm_config().to_string(),
        "pool": p.to_string(),
        "amount_in": amount_in,
        "min_out": min_out,
        "data_hex": hex(&ix.data),
        "accounts": ix.accounts.iter().map(|m| json!({
            "pubkey": m.pubkey.to_string(),
            "signer": m.is_signer,
            "writable": m.is_writable,
        })).collect::<Vec<_>>(),
    });
    let path = repo_root().join("fixtures/swap/swap_ix.json");
    let text = serde_json::to_string_pretty(&v).unwrap() + "\n";
    if std::env::var_os("UPDATE_SWAP_FIXTURE").is_some() {
        std::fs::write(&path, &text).unwrap();
    }
    let committed: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap())
        .expect("fixtures/swap/swap_ix.json (UPDATE_SWAP_FIXTURE=1 writes it)");
    assert_eq!(committed, v, "fixture out of date: UPDATE_SWAP_FIXTURE=1");
}

/// The committed IDL is the one `anchor build` writes (`target/idl/mock_amm.json`).
#[test]
fn committed_idl_is_current() {
    let built = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/idl/mock_amm.json");
    let committed = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../idl/mock_amm.json");
    let read = |p: &PathBuf| -> Value {
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    };
    let c = read(&committed);
    assert_eq!(c["address"], mock_amm::ID.to_string());
    if built.exists() {
        assert_eq!(
            read(&built),
            c,
            "copy target/idl/mock_amm.json to programs/idl/"
        );
    }
}

use std::path::PathBuf;
