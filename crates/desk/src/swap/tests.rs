//! [`super`] against `programs/idl/mock_amm.json` (discriminators, account lists and flags,
//! argument bytes, account layouts), `fixtures/swap/swap_ix.json` (the swap instruction as
//! Anchor's client builds it) and `fixtures/swap/vectors.json` (hand-computed x·y = k), plus the
//! L4-3 JSON shapes.

use serde_json::{Value, json};
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

use super::accounts::{self, disc as acc_disc};
use super::ix::{self, disc as ix_disc};
use super::*;
use crate::solana::rpc::RawAccount;

const IDL: &str = include_str!("../../../../programs/idl/mock_amm.json");
const SWAP_IX: &str = include_str!("../../../../fixtures/swap/swap_ix.json");
const VECTORS: &str = include_str!("../../../../fixtures/swap/vectors.json");

fn idl() -> Value {
    serde_json::from_str(IDL).unwrap()
}

fn find<'a>(list: &'a Value, name: &str) -> &'a Value {
    list.as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == name)
        .unwrap_or_else(|| panic!("{name} not in IDL"))
}

fn disc_of(v: &Value) -> [u8; 8] {
    let b: Vec<u8> = v["discriminator"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| u8::try_from(x.as_u64().unwrap()).unwrap())
        .collect();
    b.try_into().unwrap()
}

fn pk(v: &Value) -> Pubkey {
    v.as_str().unwrap().parse().unwrap()
}

fn pdas() -> AmmPdas {
    AmmPdas::new(
        DEFAULT_PROGRAM_ID.parse().unwrap(),
        &Pdas::new(crate::config::DEFAULT_PROGRAM_ID.parse().unwrap()),
    )
}

#[test]
fn ids_and_discriminators_match_the_idl() {
    let idl = idl();
    assert_eq!(idl["address"], DEFAULT_PROGRAM_ID);
    for (name, d) in [
        ("AmmConfig", acc_disc::AMM_CONFIG),
        ("MockToken", acc_disc::MOCK_TOKEN),
        ("Pool", acc_disc::POOL),
        ("FaucetClaim", acc_disc::FAUCET_CLAIM),
    ] {
        assert_eq!(disc_of(find(&idl["accounts"], name)), d, "{name}");
    }
    for (name, d) in [
        ("init_amm", ix_disc::INIT_AMM),
        ("set_lookup_table", ix_disc::SET_LOOKUP_TABLE),
        ("init_mock_mint", ix_disc::INIT_MOCK_MINT),
        ("init_pool", ix_disc::INIT_POOL),
        ("add_liquidity", ix_disc::ADD_LIQUIDITY),
        ("swap", ix_disc::SWAP),
        ("faucet", ix_disc::FAUCET),
    ] {
        assert_eq!(disc_of(find(&idl["instructions"], name)), d, "{name}");
    }
    // Seeds as the IDL's constants spell them.
    for (name, seed) in [
        ("AMM_CONFIG_SEED", seeds::AMM_CONFIG),
        ("MOCK_SEED", seeds::MOCK),
        ("TOKEN_SEED", seeds::TOKEN),
        ("POOL_SEED", seeds::POOL),
        ("VAULT_SEED", seeds::VAULT),
        ("FAUCET_SEED", seeds::FAUCET),
    ] {
        let c = find(&idl["constants"], name);
        let bytes: Vec<u8> = serde_json::from_str(c["value"].as_str().unwrap()).unwrap();
        assert_eq!(bytes, seed, "{name}");
    }
    assert_eq!(find(&idl["constants"], "FEE_BPS")["value"], "50");
}

/// Same number of metas as the IDL, same signer/writable flags in order, fixed addresses equal;
/// `args` borsh-encoded after the discriminator equal the data.
fn check_ix(name: &str, ix: &Instruction, args: &[u8]) {
    let idl = idl();
    let def = find(&idl["instructions"], name);
    let accs = def["accounts"].as_array().unwrap();
    assert_eq!(ix.program_id, pdas().program, "{name} program");
    assert_eq!(ix.accounts.len(), accs.len(), "{name}: account count");
    for (i, (a, m)) in accs.iter().zip(&ix.accounts).enumerate() {
        let an = a["name"].as_str().unwrap();
        let flag = |k: &str| a.get(k).and_then(Value::as_bool).unwrap_or(false);
        assert_eq!(m.is_signer, flag("signer"), "{name}[{i}] {an} signer");
        assert_eq!(m.is_writable, flag("writable"), "{name}[{i}] {an} writable");
        if let Some(addr) = a.get("address") {
            assert_eq!(m.pubkey, pk(addr), "{name}[{i}] {an} address");
        }
    }
    let mut want = disc_of(def).to_vec();
    want.extend_from_slice(args);
    assert_eq!(ix.data, want, "{name} data");
}

#[test]
fn builders_match_the_idl() {
    let p = pdas();
    let k = Pubkey::new_from_array([3; 32]);
    let mint = p.mock_mint("BONK");
    let pool = p.pool(&mint);
    let mut args = 5u64.to_le_bytes().to_vec();
    args.extend_from_slice(&4u64.to_le_bytes());
    let s = ix::swap(&p, &k, &mint, &k, &k, true, 5, 4);
    check_ix("swap", &s, &args);
    assert_eq!(s.accounts[1].pubkey, pool);
    assert_eq!(s.accounts[4].pubkey, p.vault(&pool, &mint));
    assert_eq!(s.accounts[5].pubkey, p.vault(&pool, &p.usdc_mint));
    let b = ix::swap(&p, &k, &mint, &k, &k, false, 5, 4);
    assert_eq!(b.accounts[4].pubkey, p.vault(&pool, &p.usdc_mint));
    assert_eq!(b.accounts[5].pubkey, p.vault(&pool, &mint));

    check_ix(
        "faucet",
        &ix::faucet(&p, &k, &k, &mint, &k, 9),
        &9u64.to_le_bytes(),
    );
    check_ix("init_amm", &ix::init_amm(&p, &k), p.usdc_mint.as_ref());
    let mut a = 4u32.to_le_bytes().to_vec();
    a.extend_from_slice(b"BONK");
    a.push(5);
    a.extend_from_slice(&7u64.to_le_bytes());
    let im = ix::init_mock_mint(&p, &k, "BONK", 5, 7);
    check_ix("init_mock_mint", &im, &a);
    assert_eq!(im.accounts[2].pubkey, mint);
    assert_eq!(im.accounts[3].pubkey, p.mock_token(&mint));
    check_ix("init_pool", &ix::init_pool(&p, &k, &mint), &[]);
    let mut a = 1u64.to_le_bytes().to_vec();
    a.extend_from_slice(&2u64.to_le_bytes());
    check_ix("add_liquidity", &ix::add_liquidity(&p, &k, &mint, 1, 2), &a);
    check_ix("set_lookup_table", &ix::set_lookup_table(&p, &k, &k), &[]);
}

/// Byte for byte the instruction Anchor's generated client builds (fixture written by
/// `programs/mock-amm/tests/swap_ix_vector.rs`).
#[test]
fn sell_to_usdc_equals_the_anchor_client_vector() {
    let v: Value = serde_json::from_str(SWAP_IX).unwrap();
    let p = pdas();
    assert_eq!(p.program, pk(&v["program_id"]));
    assert_eq!(p.usdc_mint, pk(&v["usdc_mint"]));
    assert_eq!(p.amm_config(), pk(&v["amm_config"]));
    let mint = pk(&v["mint"]);
    assert_eq!(mint, p.mock_mint("BONK"));
    assert_eq!(p.pool(&mint), pk(&v["pool"]));
    let ix = ix::sell_to_usdc(
        &p,
        &pk(&v["user"]),
        &mint,
        v["amount_in"].as_u64().unwrap(),
        v["min_out"].as_u64().unwrap(),
    );
    assert_eq!(
        crate::program::hex(&ix.data),
        v["data_hex"].as_str().unwrap()
    );
    let metas: Vec<Value> = ix
        .accounts
        .iter()
        .map(|m| json!({"pubkey": m.pubkey.to_string(), "signer": m.is_signer, "writable": m.is_writable}))
        .collect();
    assert_eq!(Value::Array(metas), v["accounts"]);
}

#[test]
fn math_matches_the_hand_computed_vectors() {
    let v: Value = serde_json::from_str(VECTORS).unwrap();
    let fee = u16::try_from(v["fee_bps"].as_u64().unwrap()).unwrap();
    let slip = u16::try_from(v["slippage_bps"].as_u64().unwrap()).unwrap();
    assert_eq!(slip, SLIPPAGE_BPS);
    let u = |c: &Value, k: &str| c[k].as_u64().unwrap();
    for c in v["swaps"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let got = math::swap_out(u(c, "amount_in"), u(c, "r_in"), u(c, "r_out"), fee);
        if u(c, "amount_out") == 0 {
            assert_eq!(got, None, "{name}");
            continue;
        }
        let s = got.unwrap();
        assert_eq!(s.amount_out, u(c, "amount_out"), "{name}");
        assert_eq!(s.fee_in, u(c, "fee_in"), "{name}");
        assert_eq!(s.gross_out - s.amount_out, u(c, "fee_out"), "{name}");
        assert_eq!(math::min_out(s.amount_out, slip), u(c, "min_out"), "{name}");
    }
}

fn snap() -> Snapshot {
    let p = pdas();
    let bonk = p.mock_mint("BONK");
    Snapshot {
        usdc_mint: p.usdc_mint,
        usdc_decimals: 6,
        pools: vec![PoolSnap {
            pool: p.pool(&bonk),
            mint: bonk,
            symbol: "mockBONK".into(),
            decimals: 5,
            r_mint: 500_000_000_000,
            r_usdc: 5_000_000_000,
            fee_bps: 50,
        }],
    }
}

#[test]
fn tokens_and_swap_quote_have_the_l4_3_shapes() {
    let s = snap();
    let p = pdas();
    let bonk = p.mock_mint("BONK");
    let t = serde_json::to_value(s.tokens()).unwrap();
    assert_eq!(
        t,
        json!({"tokens": [
            {"mint": p.usdc_mint.to_string(), "symbol": "mockUSDC", "decimals": 6, "usdc_mint": true},
            {"mint": bonk.to_string(), "symbol": "mockBONK", "decimals": 5, "usdc_mint": false},
        ]})
    );
    let q = s
        .quote(&SwapQuoteRequest {
            mint: bonk.to_string(),
            amount_in: "1000000000".into(),
        })
        .unwrap();
    assert_eq!(
        serde_json::to_value(q).unwrap(),
        json!({
            "mint": bonk.to_string(), "amount_in": "1000000000", "usdc_out_u": 9_930_238u64,
            "fee_u": 49_801u64, "min_out_u": 9_880_586u64, "slippage_bps": 50,
            "route": "mock-amm", "pool": p.pool(&bonk).to_string(),
        })
    );
    for (mint, amount) in [
        (bonk.to_string(), "1"),
        (bonk.to_string(), "0"),
        (bonk.to_string(), "-5"),
        (bonk.to_string(), "1e9"),
        (bonk.to_string(), "18446744073709551616"),
        (p.usdc_mint.to_string(), "1000"),
        (p.mock_mint("SOL").to_string(), "1000"),
        ("not-a-key".into(), "1000"),
    ] {
        let e = s
            .quote(&SwapQuoteRequest {
                mint,
                amount_in: amount.into(),
            })
            .unwrap_err();
        assert_eq!(e.status_code().as_u16(), 422, "{amount}: {e:?}");
    }
}

/// Account images written field by field from the IDL type definitions.
fn image(name: &str, d: [u8; 8], fields: &[(&str, Value)]) -> Vec<u8> {
    let idl = idl();
    let def = find(&idl["types"], name);
    let mut out = d.to_vec();
    for f in def["type"]["fields"].as_array().unwrap() {
        let fname = f["name"].as_str().unwrap();
        let v = fields
            .iter()
            .find(|(n, _)| *n == fname)
            .map_or(Value::Null, |(_, v)| v.clone());
        match &f["type"] {
            Value::String(t) => match t.as_str() {
                "pubkey" => out.extend_from_slice(pk(&v).as_ref()),
                "u8" => out.push(u8::try_from(v.as_u64().unwrap_or(0)).unwrap()),
                "u16" => out.extend_from_slice(
                    &u16::try_from(v.as_u64().unwrap_or(0))
                        .unwrap()
                        .to_le_bytes(),
                ),
                "u64" => out.extend_from_slice(&v.as_u64().unwrap_or(0).to_le_bytes()),
                "i64" => out.extend_from_slice(&v.as_i64().unwrap_or(0).to_le_bytes()),
                "string" => {
                    let s = v.as_str().unwrap();
                    out.extend_from_slice(&u32::try_from(s.len()).unwrap().to_le_bytes());
                    out.extend_from_slice(s.as_bytes());
                }
                other => panic!("{other}"),
            },
            Value::Object(o) if o.contains_key("array") => {
                let n = o["array"][1].as_u64().unwrap() as usize;
                out.extend(std::iter::repeat_n(0u8, n));
            }
            other => panic!("{other}"),
        }
    }
    out
}

fn key(b: u8) -> Value {
    Value::String(Pubkey::new_from_array([b; 32]).to_string())
}

#[test]
fn decoders_match_the_idl_layouts() {
    let c = accounts::AmmConfigAccount::decode(&image(
        "AmmConfig",
        acc_disc::AMM_CONFIG,
        &[
            ("admin", key(1)),
            ("usdc_mint", key(2)),
            ("lookup_table", key(3)),
        ],
    ))
    .unwrap();
    assert_eq!(
        (c.admin, c.usdc_mint, c.lookup_table),
        (pk(&key(1)), pk(&key(2)), pk(&key(3)))
    );
    let t = accounts::MockTokenAccount::decode(&image(
        "MockToken",
        acc_disc::MOCK_TOKEN,
        &[
            ("mint", key(4)),
            ("symbol", json!("BONK")),
            ("decimals", json!(5)),
            ("faucet_max", json!(12345)),
        ],
    ))
    .unwrap();
    assert_eq!(
        (t.mint, t.symbol.as_str(), t.decimals, t.faucet_max),
        (pk(&key(4)), "BONK", 5, 12345)
    );
    let pl = accounts::PoolAccount::decode(&image(
        "Pool",
        acc_disc::POOL,
        &[
            ("mint", key(5)),
            ("usdc_mint", key(6)),
            ("vault", key(7)),
            ("vault_usdc", key(8)),
            ("fee_bps", json!(50)),
            ("fees_mint", json!(11)),
            ("fees_usdc", json!(22)),
            ("swaps", json!(33)),
        ],
    ))
    .unwrap();
    assert_eq!(
        (
            pl.vault_usdc,
            pl.fee_bps,
            pl.fees_mint,
            pl.fees_usdc,
            pl.swaps
        ),
        (pk(&key(8)), 50, 11, 22, 33)
    );
    assert!(accounts::PoolAccount::decode(&[0; 200]).is_err());
    let mut lut = 1u32.to_le_bytes().to_vec();
    lut.resize(56, 0);
    lut.extend_from_slice(&[9; 32]);
    assert_eq!(
        accounts::lookup_table_addresses(&lut).unwrap(),
        vec![Pubkey::new_from_array([9; 32])]
    );
    assert!(accounts::lookup_table_addresses(&lut[..60]).is_err());
}

fn raw(owner: Pubkey, data: Vec<u8>) -> Option<RawAccount> {
    Some(RawAccount {
        owner,
        lamports: 1,
        data,
    })
}

fn token_image(mint: &Pubkey, amount: u64) -> Vec<u8> {
    let mut d = vec![0u8; 165];
    d[0..32].copy_from_slice(mint.as_ref());
    d[64..72].copy_from_slice(&amount.to_le_bytes());
    d
}

#[test]
fn snapshot_reads_pools_and_skips_inconsistent_ones() {
    let p = pdas();
    let tok = crate::program::TOKEN_PROGRAM_ID;
    let mints: Vec<Pubkey> = MOCK_SYMBOLS.iter().map(|s| p.mock_mint(s)).collect();
    let s = |b: &Pubkey| Value::String(b.to_string());
    let cfg = image(
        "AmmConfig",
        acc_disc::AMM_CONFIG,
        &[
            ("admin", key(1)),
            ("usdc_mint", s(&p.usdc_mint)),
            ("lookup_table", key(3)),
        ],
    );
    let mut usdc_mint = vec![0u8; 82];
    usdc_mint[44] = 6;
    let mut accs = vec![raw(p.program, cfg.clone()), raw(tok, usdc_mint)];
    for (j, m) in mints.iter().enumerate() {
        let pool = p.pool(m);
        let mut mint_acc = vec![0u8; 82];
        mint_acc[44] = [5, 9][j];
        let sym = MOCK_SYMBOLS[j];
        // mockSOL's pool claims a foreign vault: skipped.
        let vault = if j == 0 {
            p.vault(&pool, m)
        } else {
            Pubkey::new_unique()
        };
        accs.extend([
            raw(tok, mint_acc),
            raw(
                p.program,
                image(
                    "MockToken",
                    acc_disc::MOCK_TOKEN,
                    &[
                        ("mint", s(m)),
                        ("symbol", json!(sym)),
                        ("decimals", json!([5, 9][j])),
                        ("faucet_max", json!(1)),
                    ],
                ),
            ),
            raw(
                p.program,
                image(
                    "Pool",
                    acc_disc::POOL,
                    &[
                        ("mint", s(m)),
                        ("usdc_mint", s(&p.usdc_mint)),
                        ("vault", s(&vault)),
                        ("vault_usdc", s(&p.vault(&pool, &p.usdc_mint))),
                        ("fee_bps", json!(50)),
                    ],
                ),
            ),
            raw(tok, token_image(m, 1_000 + j as u64)),
            raw(tok, token_image(&p.usdc_mint, 2_000 + j as u64)),
        ]);
    }
    let snap = snapshot_from(&p, &mints, &accs).unwrap();
    assert_eq!(snap.pools.len(), 1);
    assert_eq!(
        snap.pools[0],
        PoolSnap {
            pool: p.pool(&mints[0]),
            mint: mints[0],
            symbol: "mockBONK".into(),
            decimals: 5,
            r_mint: 1_000,
            r_usdc: 2_000,
            fee_bps: 50,
        }
    );
    // A pool account owned by another program is ignored.
    let mut forged = accs.clone();
    forged[4].as_mut().unwrap().owner = Pubkey::new_unique();
    assert!(snapshot_from(&p, &mints, &forged).unwrap().pools.is_empty());
    // No AmmConfig, or one quoting another USDC mint: mock USDC only.
    let mut none = accs.clone();
    none[0] = None;
    let only_usdc = snapshot_from(&p, &mints, &none).unwrap();
    assert!(only_usdc.pools.is_empty());
    assert_eq!(only_usdc.tokens().tokens.len(), 1);
    assert!(only_usdc.tokens().tokens[0].usdc_mint);
    let mut other = accs;
    other[0] = raw(
        p.program,
        image(
            "AmmConfig",
            acc_disc::AMM_CONFIG,
            &[
                ("admin", key(1)),
                ("usdc_mint", key(9)),
                ("lookup_table", key(3)),
            ],
        ),
    );
    assert!(snapshot_from(&p, &mints, &other).unwrap().pools.is_empty());
}

#[test]
fn lookup_table_instructions_follow_the_alt_layout() {
    let a = Pubkey::new_from_array([1; 32]);
    let c = ix::create_lookup_table(&a, &a, 77);
    assert_eq!(c.program_id, ix::ALT_PROGRAM_ID);
    assert_eq!(&c.data[..4], &[0, 0, 0, 0]);
    assert_eq!(&c.data[4..12], &77u64.to_le_bytes());
    assert_eq!(c.data.len(), 13);
    let (table, bump) =
        Pubkey::find_program_address(&[a.as_ref(), &77u64.to_le_bytes()], &ix::ALT_PROGRAM_ID);
    assert_eq!(c.data[12], bump);
    assert_eq!(c.accounts[0].pubkey, table);
    assert_eq!(ix::lookup_table_address(&a, 77), table);
    let e = ix::extend_lookup_table(&table, &a, &a, &[a, table]);
    assert_eq!(&e.data[..4], &[2, 0, 0, 0]);
    assert_eq!(&e.data[4..12], &2u64.to_le_bytes());
    assert_eq!(e.data.len(), 12 + 64);
    let s = Pdas::new(crate::config::DEFAULT_PROGRAM_ID.parse().unwrap());
    let p = pdas();
    let list = ix::lookup_table_addresses(&s, &p, &[p.mock_mint("BONK")]);
    assert_eq!(list.len(), 11);
    assert!(list.contains(&s.escrow_usdc()) && list.contains(&p.pool(&p.mock_mint("BONK"))));
}
