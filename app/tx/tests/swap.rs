//! `schedina_tx::swap` against the mock AMM's artifacts: `programs/idl/mock_amm.json` (account
//! lists, flags, fixed addresses, PDA seeds, discriminators, `AmmConfig` layout) and
//! `fixtures/swap/swap_ix.json` (the swap as Anchor's client builds it); then the transaction
//! `[swap, Ed25519 verify, open_pending]`: order, verify immediately before `open_pending`,
//! writable flags kept through the lookup table, and the 1,232-byte limit for 5 legs.

use std::collections::HashMap;
use std::path::PathBuf;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use schedina_tx::swap::*;
use schedina_tx::*;
use serde_json::Value;
use solana_message::VersionedMessage;

fn load(rel: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn amm_idl() -> Value {
    load("programs/idl/mock_amm.json")
}

fn pk(v: &Value) -> Pubkey {
    v.as_str().unwrap().parse().unwrap()
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn ix_def<'a>(idl: &'a Value, name: &str) -> &'a Value {
    idl["instructions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == name)
        .unwrap()
}

fn disc(v: &Value) -> [u8; 8] {
    let b: Vec<u8> = v["discriminator"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_u64().unwrap() as u8)
        .collect();
    b.try_into().unwrap()
}

/// Metas vs the IDL: count, order, signer/writable, fixed addresses, and PDAs whose seeds are
/// constants or accounts named in `keys` (paths like `mint` or `pool.mint`).
fn assert_matches_idl(name: &str, ix: &Instruction, keys: &HashMap<&str, Pubkey>) {
    let idl = amm_idl();
    let def = ix_def(&idl, name);
    assert_eq!(ix.program_id, AMM_PROGRAM_ID);
    assert_eq!(ix.data[..8], disc(def), "{name} discriminator");
    let accs = def["accounts"].as_array().unwrap();
    assert_eq!(ix.accounts.len(), accs.len(), "{name} account count");
    for (i, (a, m)) in accs.iter().zip(&ix.accounts).enumerate() {
        let an = a["name"].as_str().unwrap();
        let flag = |k: &str| a.get(k).and_then(Value::as_bool).unwrap_or(false);
        assert_eq!(m.is_signer, flag("signer"), "{name}[{i}] {an} signer");
        assert_eq!(m.is_writable, flag("writable"), "{name}[{i}] {an} writable");
        if let Some(addr) = a.get("address") {
            assert_eq!(m.pubkey, pk(addr), "{name}[{i}] {an}");
        }
        if let Some(pda) = a.get("pda") {
            let seeds: Option<Vec<Vec<u8>>> = pda["seeds"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| match s["kind"].as_str().unwrap() {
                    "const" => Some(
                        s["value"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|b| b.as_u64().unwrap() as u8)
                            .collect(),
                    ),
                    _ => keys
                        .get(s["path"].as_str().unwrap())
                        .map(|k| k.to_bytes().to_vec()),
                })
                .collect();
            if let Some(seeds) = seeds {
                let refs: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
                let want = Pubkey::find_program_address(&refs, &AMM_PROGRAM_ID).0;
                assert_eq!(m.pubkey, want, "{name}[{i}] {an} pda");
            }
        }
        if let Some(k) = keys.get(an) {
            assert_eq!(m.pubkey, *k, "{name}[{i}] {an}");
        }
    }
}

#[test]
fn ids_and_layouts_match_the_idl() {
    let idl = amm_idl();
    assert_eq!(pk(&idl["address"]), AMM_PROGRAM_ID);
    assert_eq!(disc(ix_def(&idl, "swap")), SWAP_DISC);
    assert_eq!(disc(ix_def(&idl, "faucet")), FAUCET_DISC);
    let cfg = idl["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "AmmConfig")
        .unwrap();
    assert_eq!(disc(cfg), AMM_CONFIG_DISC);
    // AmmConfig = admin ‖ usdc_mint ‖ lookup_table ‖ …: the table is bytes 72..104.
    let ty = idl["types"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "AmmConfig")
        .unwrap();
    let names: Vec<&str> = ty["type"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .take(3)
        .map(|f| {
            assert_eq!(f["type"], "pubkey");
            f["name"].as_str().unwrap()
        })
        .collect();
    assert_eq!(names, ["admin", "usdc_mint", "lookup_table"]);
    let mut data = AMM_CONFIG_DISC.to_vec();
    data.extend_from_slice(&[1; 64]);
    data.extend_from_slice(&[7; 32]);
    data.extend_from_slice(&[0; 33]);
    assert_eq!(
        amm_config_lookup_table(&data),
        Ok(Some(Pubkey::new_from_array([7; 32])))
    );
    data[72..104].fill(0);
    assert_eq!(amm_config_lookup_table(&data), Ok(None));
    data[0] ^= 1;
    assert!(amm_config_lookup_table(&data).is_err());
}

#[test]
fn swap_ix_is_the_anchor_client_vector() {
    let v = load("fixtures/swap/swap_ix.json");
    let user = pk(&v["user"]);
    let mint = pk(&v["mint"]);
    assert_eq!(mint, amm_pda::mock_mint("BONK"));
    assert_eq!(pk(&v["usdc_mint"]), pda::usdc_mint());
    assert_eq!(pk(&v["amm_config"]), amm_pda::amm_config());
    assert_eq!(pk(&v["schedina_program_id"]), PROGRAM_ID);
    let ix = swap_ix(
        &user,
        &mint,
        v["amount_in"].as_u64().unwrap(),
        v["min_out"].as_u64().unwrap(),
    );
    assert_eq!(ix.program_id, pk(&v["program_id"]));
    assert_eq!(ix.data, hex(v["data_hex"].as_str().unwrap()));
    let want: Vec<(Pubkey, bool, bool)> = v["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                pk(&a["pubkey"]),
                a["signer"].as_bool().unwrap(),
                a["writable"].as_bool().unwrap(),
            )
        })
        .collect();
    let got: Vec<(Pubkey, bool, bool)> = ix
        .accounts
        .iter()
        .map(|m| (m.pubkey, m.is_signer, m.is_writable))
        .collect();
    assert_eq!(got, want);
    let pool = amm_pda::pool(&mint);
    let keys = HashMap::from([
        ("user", user),
        ("pool.mint", mint),
        ("user_in", pda::ata(&user, &mint)),
        ("user_out", pda::user_usdc(&user)),
        ("vault_in", amm_pda::vault(&pool, &mint)),
        ("vault_out", amm_pda::vault(&pool, &pda::usdc_mint())),
    ]);
    assert_matches_idl("swap", &ix, &keys);
}

#[test]
fn faucet_matches_the_idl() {
    let user: Pubkey = "J2xccRtuG43drESLYznHhLhQkLTdfepcKYbiQ9BsJVaf"
        .parse()
        .unwrap();
    let mint = amm_pda::mock_mint("BONK");
    let [create, faucet] = mock_faucet_ixs(&user, &user, &mint, 123);
    assert_eq!(create.program_id, ATA_PROGRAM_ID);
    assert_eq!(create.accounts[1].pubkey, pda::ata(&user, &mint));
    let keys = HashMap::from([
        ("caller", user),
        ("mint", mint),
        ("recipient", pda::ata(&user, &mint)),
    ]);
    assert_matches_idl("faucet", &faucet, &keys);
    assert_eq!(faucet.data[8..], 123u64.to_le_bytes());
}

/// A `n`-leg quote message (D-050 layout) for `stake_u`, signed by nobody in particular: the
/// app never verifies the signature itself, the precompile does.
fn quote(n: usize, stake_u: u64) -> (SignedMsg, ShownQuote) {
    let legs: Vec<ShownLeg> = (0..n)
        .map(|i| ShownLeg {
            symbol: format!("NBA_{i}"),
            game_id: format!("game-{i}"),
            side: 0,
        })
        .collect();
    let sha = |s: &str| -> [u8; 32] {
        use sha2::Digest;
        sha2::Sha256::digest(s.as_bytes()).into()
    };
    let mut msg = QUOTE_TAG.to_vec();
    msg.extend_from_slice(PROGRAM_ID.as_ref());
    msg.extend_from_slice(&(n as u32).to_le_bytes());
    for l in &legs {
        msg.extend_from_slice(&sha(&l.symbol));
        msg.extend_from_slice(&sha(&l.game_id));
        msg.extend_from_slice(&1_793_300_000i64.to_le_bytes());
        msg.push(l.side);
        msg.extend_from_slice(&500_000u64.to_le_bytes());
        msg.extend_from_slice(&50_000u32.to_le_bytes());
    }
    msg.extend_from_slice(&stake_u.to_le_bytes());
    msg.extend_from_slice(&30_000_000u64.to_le_bytes());
    msg.extend_from_slice(&1_793_213_010i64.to_le_bytes());
    let signer: Pubkey = "GmaDrppBC7P5ARKV8g3djiwP89vz1jLK23V2GBjuAEGB"
        .parse()
        .unwrap();
    let signed = SignedMsg::from_wire(
        &signer.to_string(),
        &STANDARD.encode(&msg),
        &STANDARD.encode([9u8; 64]),
    )
    .unwrap();
    let shown = ShownQuote {
        legs,
        stake_u,
        odds_offered_u: 30_000_000,
        quote_expires_at: 1_793_213_010,
        min_payout_per_unit_u: 29_000_000,
    };
    (signed, shown)
}

fn shown_swap(min_out_u: u64) -> ShownSwap {
    let mint = amm_pda::mock_mint("BONK");
    ShownSwap {
        mint,
        amount_in: 5_000_000_000,
        min_out_u,
        pool: amm_pda::pool(&mint),
    }
}

/// The lookup table the AMM's set-up writes (desk `swap::ix::lookup_table_addresses`).
fn table() -> (Pubkey, Vec<Pubkey>) {
    let mint = amm_pda::mock_mint("BONK");
    let usdc = pda::usdc_mint();
    let pool = amm_pda::pool(&mint);
    (
        Pubkey::new_from_array([42; 32]),
        vec![
            pda::config(),
            pda::escrow_usdc(),
            INSTRUCTIONS_SYSVAR_ID,
            TOKEN_PROGRAM_ID,
            SYSTEM_PROGRAM_ID,
            pda::event_authority(),
            usdc,
            mint,
            pool,
            amm_pda::vault(&pool, &mint),
            amm_pda::vault(&pool, &usdc),
        ],
    )
}

fn compute_budget() -> Vec<Instruction> {
    let id: Pubkey = "ComputeBudget111111111111111111111111111111"
        .parse()
        .unwrap();
    let mut lim = vec![2u8];
    lim.extend_from_slice(&400_000u32.to_le_bytes());
    let mut price = vec![3u8];
    price.extend_from_slice(&1_000u64.to_le_bytes());
    vec![
        Instruction {
            program_id: id,
            accounts: vec![],
            data: lim,
        },
        Instruction {
            program_id: id,
            accounts: vec![],
            data: price,
        },
    ]
}

/// Decodes our own wire bytes back (as the wallet does) and returns the v0 message.
fn decode(wire: &[u8]) -> solana_message::v0::Message {
    let n = usize::from(wire[0]);
    assert!(
        wire[1..1 + 64 * n].iter().all(|b| *b == 0),
        "signatures zeroed"
    );
    assert_eq!(wire[1 + 64 * n], 0x80, "v0 prefix");
    decode_v0(&wire[1 + 64 * n..])
}

/// A v0 message decoder written from the wire format (compact-u16 counts below 128 here, data
/// lengths up to two bytes), independent of the encoder under test.
fn decode_v0(b: &[u8]) -> solana_message::v0::Message {
    use solana_message::compiled_instruction::CompiledInstruction;
    use solana_message::v0::MessageAddressTableLookup;
    let mut at = 1; // 0x80
    let header = solana_message::MessageHeader {
        num_required_signatures: b[at],
        num_readonly_signed_accounts: b[at + 1],
        num_readonly_unsigned_accounts: b[at + 2],
    };
    at += 3;
    let mut take = |n: usize| {
        let s = &b[at..at + n];
        at += n;
        s.to_vec()
    };
    let nk = take(1)[0] as usize;
    let account_keys: Vec<Pubkey> = (0..nk)
        .map(|_| Pubkey::new_from_array(take(32).try_into().unwrap()))
        .collect();
    let recent_blockhash = solana_hash::Hash::new_from_array(take(32).try_into().unwrap());
    let ni = take(1)[0] as usize;
    let mut instructions = Vec::new();
    for _ in 0..ni {
        let program_id_index = take(1)[0];
        let na = take(1)[0] as usize;
        let accounts = take(na);
        let l0 = take(1)[0] as usize;
        let len = if l0 & 0x80 != 0 {
            (l0 & 0x7f) | ((take(1)[0] as usize) << 7)
        } else {
            l0
        };
        let data = take(len);
        instructions.push(CompiledInstruction {
            program_id_index,
            accounts,
            data,
        });
    }
    let nl = take(1)[0] as usize;
    let address_table_lookups = (0..nl)
        .map(|_| {
            let account_key = Pubkey::new_from_array(take(32).try_into().unwrap());
            let w = take(1)[0] as usize;
            let writable_indexes = take(w);
            let r = take(1)[0] as usize;
            let readonly_indexes = take(r);
            MessageAddressTableLookup {
                account_key,
                writable_indexes,
                readonly_indexes,
            }
        })
        .collect();
    assert_eq!(at, b.len(), "trailing bytes");
    solana_message::v0::Message {
        header,
        account_keys,
        recent_blockhash,
        instructions,
        address_table_lookups,
    }
}

#[test]
fn swap_verify_open_pending_in_one_v0_transaction() {
    let user: Pubkey = "J2xccRtuG43drESLYznHhLhQkLTdfepcKYbiQ9BsJVaf"
        .parse()
        .unwrap();
    let stake = 49_013_564;
    let (signed, shown) = quote(3, stake);
    let ixs = swap_open_pending_ixs(
        &user,
        &user,
        &shown_swap(stake),
        &signed,
        &shown,
        42,
        1_793_213_000,
    )
    .unwrap();
    assert_eq!(
        [ixs[0].program_id, ixs[1].program_id, ixs[2].program_id],
        [AMM_PROGRAM_ID, ED25519_PROGRAM_ID, PROGRAM_ID]
    );
    assert_eq!(
        ixs[0],
        swap_ix(&user, &shown_swap(stake).mint, 5_000_000_000, stake)
    );
    // The swap pays into the very account open_pending escrows from.
    assert_eq!(ixs[0].accounts[3].pubkey, ixs[2].accounts[3].pubkey);
    assert_eq!(ixs[2].accounts[3].pubkey, pda::user_usdc(&user));
    assert_eq!(ixs[1].data[112..], signed.msg[..], "quote bytes untouched");

    let (key, addresses) = table();
    let wire = compile_v0(&user, &ixs, Some((key, addresses.clone())), [7; 32]).unwrap();
    let msg = decode(&wire);
    assert_eq!(
        VersionedMessage::V0(msg.clone()).serialize(),
        wire[65..].to_vec(),
        "canonical encoding"
    );
    assert_eq!(msg.header.num_required_signatures, 1);
    assert_eq!(msg.account_keys[0], user, "the user pays and signs");
    // Resolve every index: static keys, then the table's writable then readonly picks.
    let l = &msg.address_table_lookups[0];
    assert_eq!(l.account_key, key);
    let mut all = msg.account_keys.clone();
    all.extend(
        l.writable_indexes
            .iter()
            .map(|i| addresses[usize::from(*i)]),
    );
    all.extend(
        l.readonly_indexes
            .iter()
            .map(|i| addresses[usize::from(*i)]),
    );
    let n_static = msg.account_keys.len();
    let n_w = l.writable_indexes.len();
    let h = &msg.header;
    let writable = |i: usize| {
        if i < usize::from(h.num_required_signatures) {
            i < usize::from(h.num_required_signatures - h.num_readonly_signed_accounts)
        } else if i < n_static {
            i < n_static - usize::from(h.num_readonly_unsigned_accounts)
        } else {
            i < n_static + n_w
        }
    };
    let programs: Vec<Pubkey> = msg
        .instructions
        .iter()
        .map(|c| all[usize::from(c.program_id_index)])
        .collect();
    assert_eq!(
        programs,
        vec![AMM_PROGRAM_ID, ED25519_PROGRAM_ID, PROGRAM_ID]
    );
    for (c, ix) in msg.instructions.iter().zip(&ixs) {
        assert_eq!(c.data, ix.data);
        assert_eq!(c.accounts.len(), ix.accounts.len());
        for (i, m) in c.accounts.iter().zip(&ix.accounts) {
            let i = usize::from(*i);
            assert_eq!(all[i], m.pubkey, "{}", ix.program_id);
            // Writable where the instruction needs it; read-only metas stay read-only unless
            // another instruction writes the same account (the fee payer, here).
            if m.is_writable {
                assert!(writable(i), "{} writable", m.pubkey);
            } else if m.pubkey != user {
                assert!(!writable(i), "{} read-only", m.pubkey);
            }
        }
    }
    // Invoked programs and the user's own accounts stay static.
    for k in [
        AMM_PROGRAM_ID,
        ED25519_PROGRAM_ID,
        PROGRAM_ID,
        pda::user_usdc(&user),
    ] {
        assert!(msg.account_keys.contains(&k), "{k} static");
    }
}

#[test]
fn five_legs_with_compute_budget_fit_one_packet() {
    let user: Pubkey = "J2xccRtuG43drESLYznHhLhQkLTdfepcKYbiQ9BsJVaf"
        .parse()
        .unwrap();
    let (signed, shown) = quote(5, 49_013_564);
    let three = swap_open_pending_ixs(
        &user,
        &user,
        &shown_swap(49_013_564),
        &signed,
        &shown,
        // The largest nonce the app draws (4.B's 53-bit limit); still 8 bytes on the wire.
        schedina_tx::NONCE_MAX,
        1_793_213_000,
    )
    .unwrap();
    let mut ixs = compute_budget();
    ixs.extend(three);
    let wire = compile_v0(&user, &ixs, Some(table()), [7; 32]).unwrap();
    println!(
        "5 legs + swap + 2 compute-budget, v0 + lookup table: {} bytes",
        wire.len()
    );
    assert!(wire.len() <= PACKET_DATA_SIZE);
    let msg = decode(&wire);
    // Verify sits immediately before open_pending even with prepended instructions.
    let keys = &msg.account_keys;
    let n = msg.instructions.len();
    assert_eq!(
        keys[usize::from(msg.instructions[n - 2].program_id_index)],
        ED25519_PROGRAM_ID
    );
    assert_eq!(
        keys[usize::from(msg.instructions[n - 1].program_id_index)],
        PROGRAM_ID
    );
    // Without the table, the same instructions are refused (over the packet limit).
    assert!(compile_v0(&user, &ixs, None, [7; 32]).is_err());
    // A first-time user's transaction also creates the mock-USDC account the swap pays into.
    ixs.insert(2, create_ata_idempotent_ix(&user, &user, &pda::usdc_mint()));
    let wire = compile_v0(&user, &ixs, Some(table()), [7; 32]).unwrap();
    println!("... plus create-ATA: {} bytes", wire.len());
    assert!(wire.len() <= PACKET_DATA_SIZE);
}

#[test]
fn refuses_a_mismatched_swap() {
    let user: Pubkey = "J2xccRtuG43drESLYznHhLhQkLTdfepcKYbiQ9BsJVaf"
        .parse()
        .unwrap();
    let (signed, shown) = quote(2, 10_000_000);
    let ok = shown_swap(10_000_000);
    assert!(swap_open_pending_ixs(&user, &user, &ok, &signed, &shown, 1, 0).is_ok());
    for bad in [
        ShownSwap {
            min_out_u: 10_000_001,
            ..ok
        },
        ShownSwap {
            pool: amm_pda::pool(&amm_pda::mock_mint("SOL")),
            ..ok
        },
        ShownSwap {
            mint: pda::usdc_mint(),
            pool: amm_pda::pool(&pda::usdc_mint()),
            ..ok
        },
        ShownSwap { amount_in: 0, ..ok },
    ] {
        assert!(
            swap_open_pending_ixs(&user, &user, &bad, &signed, &shown, 1, 0).is_err(),
            "{bad:?}"
        );
    }
    // The quote checks of open_pending_ixs still apply (other stake shown than signed).
    let mut other = shown.clone();
    other.stake_u = 9_999_999;
    let swap = ShownSwap {
        min_out_u: 9_999_999,
        ..ok
    };
    assert!(swap_open_pending_ixs(&user, &user, &swap, &signed, &other, 1, 0).is_err());
}

#[test]
fn reads_lookup_table_accounts() {
    let mut d = 1u32.to_le_bytes().to_vec();
    d.resize(56, 0);
    d.extend_from_slice(&[5; 32]);
    d.extend_from_slice(&[6; 32]);
    assert_eq!(
        lookup_table_addresses(&d).unwrap(),
        vec![
            Pubkey::new_from_array([5; 32]),
            Pubkey::new_from_array([6; 32])
        ]
    );
    assert!(lookup_table_addresses(&d[..70]).is_err());
    d[0] = 2;
    assert!(lookup_table_addresses(&d).is_err());
    assert_eq!(
        swap_error_words(6010),
        Some("the swap price moved: ask for a new quote")
    );
}

#[test]
fn token_amounts_round_trip() {
    assert_eq!(parse_units("50000", 5), Some(5_000_000_000));
    assert_eq!(parse_units("1,000.5", 5), Some(100_050_000));
    assert_eq!(parse_units(".25", 9), Some(250_000_000));
    assert_eq!(parse_units("0.000001", 5), None);
    assert_eq!(parse_units("0", 5), None);
    assert_eq!(parse_units("1e5", 5), None);
    assert_eq!(parse_units("-1", 5), None);
    assert_eq!(parse_units("", 5), None);
    assert_eq!(parse_units(".", 5), None);
    assert_eq!(parse_units("184467440737095516", 5), None);
    assert_eq!(format_units(5_000_000_000, 5), "50000");
    assert_eq!(format_units(100_050_000, 5), "1000.5");
    assert_eq!(format_units(1, 9), "0.000000001");
    assert_eq!(account_data_b64("AQID"), Ok(vec![1, 2, 3]));
    assert!(account_data_b64("not base64!").is_err());
}
