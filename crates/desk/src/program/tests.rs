//! Everything in [`super`] against the committed IDL: discriminators, every builder's account
//! list (order, signer, writable, fixed addresses) and argument bytes, and every decoder against
//! account and event images that an IDL-driven borsh encoder writes from sample values. The
//! quote encoder and the ed25519 instruction are also checked byte for byte against
//! `fixtures/schedina/quote_vector.json`.

use serde_json::{Map, Value, json};
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

use super::accounts::{self, disc as acc_disc};
use super::events::{self as ev, disc as ev_disc};
use super::ix::{self, Params, QuoteMsg, disc as ix_disc};
use super::*;

const IDL: &str = include_str!("../../../../programs/idl/schedina.json");
const VECTOR: &str = include_str!("../../../../fixtures/schedina/quote_vector.json");

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

fn program() -> Pubkey {
    crate::config::DEFAULT_PROGRAM_ID.parse().unwrap()
}

// --- an IDL-driven borsh encoder and sample values ----------------------------------------------

/// Borsh-encodes `v` as IDL type `ty`.
fn enc(idl: &Value, ty: &Value, v: &Value, out: &mut Vec<u8>) {
    match ty {
        Value::String(s) => {
            match s.as_str() {
                "u8" => out.push(u8::try_from(v.as_u64().unwrap()).unwrap()),
                "bool" => out.push(u8::from(v.as_bool().unwrap())),
                "u16" => out
                    .extend_from_slice(&u16::try_from(v.as_u64().unwrap()).unwrap().to_le_bytes()),
                "u32" => out
                    .extend_from_slice(&u32::try_from(v.as_u64().unwrap()).unwrap().to_le_bytes()),
                "u64" => out.extend_from_slice(&v.as_u64().unwrap().to_le_bytes()),
                "i64" => out.extend_from_slice(&v.as_i64().unwrap().to_le_bytes()),
                "pubkey" => {
                    out.extend_from_slice(v.as_str().unwrap().parse::<Pubkey>().unwrap().as_ref())
                }
                other => panic!("type {other}"),
            }
        }
        Value::Object(o) if o.contains_key("array") => {
            let a = o["array"].as_array().unwrap();
            let items = v.as_array().unwrap();
            assert_eq!(items.len() as u64, a[1].as_u64().unwrap());
            for item in items {
                enc(idl, &a[0], item, out);
            }
        }
        Value::Object(o) if o.contains_key("vec") => {
            let items = v.as_array().unwrap();
            out.extend_from_slice(&u32::try_from(items.len()).unwrap().to_le_bytes());
            for item in items {
                enc(idl, &o["vec"], item, out);
            }
        }
        Value::Object(o) if o.contains_key("option") => {
            if v.is_null() {
                out.push(0);
            } else {
                out.push(1);
                enc(idl, &o["option"], v, out);
            }
        }
        Value::Object(o) if o.contains_key("defined") => {
            let def = find(&idl["types"], o["defined"]["name"].as_str().unwrap());
            match def["type"]["kind"].as_str().unwrap() {
                "struct" => {
                    for f in def["type"]["fields"].as_array().unwrap() {
                        let name = f["name"].as_str().unwrap();
                        let fv = v.get(name).unwrap_or_else(|| panic!("no value for {name}"));
                        enc(idl, &f["type"], fv, out);
                    }
                }
                "enum" => {
                    let variants = def["type"]["variants"].as_array().unwrap();
                    let i = variants.iter().position(|x| x["name"] == *v).unwrap();
                    out.push(u8::try_from(i).unwrap());
                }
                k => panic!("kind {k}"),
            }
        }
        other => panic!("type {other}"),
    }
}

/// A sample value of IDL type `ty`, distinct per call (`seed` advances), options `Some`, vecs
/// of two items.
fn sample(idl: &Value, ty: &Value, seed: &mut u64) -> Value {
    *seed += 1;
    let s = *seed;
    match ty {
        Value::String(t) => match t.as_str() {
            "u8" => json!(s % 251),
            "bool" => json!(s % 2 == 1),
            "u16" => json!(1_000 + s),
            "u32" => json!(100_000 + s * 7),
            "u64" => json!(10_000_000_000 + s * 1_000_003),
            "i64" => json!(-1_000_000 - i64::try_from(s).unwrap() * 13),
            "pubkey" => {
                json!(Pubkey::new_from_array([u8::try_from(s % 251).unwrap(); 32]).to_string())
            }
            other => panic!("type {other}"),
        },
        Value::Object(o) if o.contains_key("array") => {
            let a = o["array"].as_array().unwrap();
            let n = a[1].as_u64().unwrap();
            Value::Array((0..n).map(|_| sample(idl, &a[0], seed)).collect())
        }
        Value::Object(o) if o.contains_key("vec") => {
            Value::Array((0..2).map(|_| sample(idl, &o["vec"], seed)).collect())
        }
        Value::Object(o) if o.contains_key("option") => sample(idl, &o["option"], seed),
        Value::Object(o) if o.contains_key("defined") => {
            let def = find(&idl["types"], o["defined"]["name"].as_str().unwrap());
            match def["type"]["kind"].as_str().unwrap() {
                "struct" => {
                    let mut m = Map::new();
                    for f in def["type"]["fields"].as_array().unwrap() {
                        m.insert(
                            f["name"].as_str().unwrap().to_owned(),
                            sample(idl, &f["type"], seed),
                        );
                    }
                    Value::Object(m)
                }
                "enum" => {
                    let variants = def["type"]["variants"].as_array().unwrap();
                    let i = usize::try_from(s).unwrap() % variants.len();
                    variants[i]["name"].clone()
                }
                k => panic!("kind {k}"),
            }
        }
        other => panic!("type {other}"),
    }
}

fn defined(name: &str) -> Value {
    json!({"defined": {"name": name}})
}

/// Discriminator ‖ borsh of `v` as type `name`.
fn image(idl: &Value, name: &str, d: [u8; 8], v: &Value) -> Vec<u8> {
    let mut out = d.to_vec();
    enc(idl, &defined(name), v, &mut out);
    out
}

fn pk(v: &Value) -> Pubkey {
    v.as_str().unwrap().parse().unwrap()
}

fn u(v: &Value) -> u64 {
    v.as_u64().unwrap()
}

fn i(v: &Value) -> i64 {
    v.as_i64().unwrap()
}

fn bytes32(v: &Value) -> [u8; 32] {
    let b: Vec<u8> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|x| u8::try_from(x.as_u64().unwrap()).unwrap())
        .collect();
    b.try_into().unwrap()
}

fn opt_pk(v: &Value) -> Option<Pubkey> {
    (!v.is_null()).then(|| pk(v))
}

fn quote_leg(v: &Value) -> accounts::QuoteLeg {
    accounts::QuoteLeg {
        symbol_hash: bytes32(&v["symbol_hash"]),
        game_id_hash: bytes32(&v["game_id_hash"]),
        start_ts: i(&v["start_ts"]),
        side: u8::try_from(u(&v["side"])).unwrap(),
        price_u: u(&v["price_u"]),
        m1_ppm: u32::try_from(u(&v["m1_ppm"])).unwrap(),
    }
}

// --- discriminators -----------------------------------------------------------------------------

#[test]
fn discriminators_match_idl() {
    let idl = idl();
    assert_eq!(idl["address"], crate::config::DEFAULT_PROGRAM_ID);
    for (name, d) in [
        ("Config", acc_disc::CONFIG),
        ("Vault", acc_disc::VAULT),
        ("Pending", acc_disc::PENDING),
        ("Ticket", acc_disc::TICKET),
        ("FaucetClaim", acc_disc::FAUCET_CLAIM),
    ] {
        assert_eq!(disc_of(find(&idl["accounts"], name)), d, "{name}");
    }
    for (name, d) in [
        ("init_config", ix_disc::INIT_CONFIG),
        ("fund_vault", ix_disc::FUND_VAULT),
        ("faucet_usdc", ix_disc::FAUCET_USDC),
        ("open_pending", ix_disc::OPEN_PENDING),
        ("finalize", ix_disc::FINALIZE),
        ("refund_pending", ix_disc::REFUND_PENDING),
        ("resolve_leg", ix_disc::RESOLVE_LEG),
        ("settle", ix_disc::SETTLE),
        ("cash_out", ix_disc::CASH_OUT),
        ("pause", ix_disc::PAUSE),
    ] {
        assert_eq!(disc_of(find(&idl["instructions"], name)), d, "{name}");
    }
    for (name, d) in [
        ("PendingOpened", ev_disc::PENDING_OPENED),
        ("PendingRefunded", ev_disc::PENDING_REFUNDED),
        ("TicketMinted", ev_disc::TICKET_MINTED),
        ("LegResolved", ev_disc::LEG_RESOLVED),
        ("TicketSettled", ev_disc::TICKET_SETTLED),
        ("TicketCashedOut", ev_disc::TICKET_CASHED_OUT),
        ("VaultFunded", ev_disc::VAULT_FUNDED),
    ] {
        assert_eq!(disc_of(find(&idl["events"], name)), d, "{name}");
    }
}

// --- instructions -------------------------------------------------------------------------------

fn flat_accounts(list: &Value, out: &mut Vec<Value>) {
    for a in list.as_array().unwrap() {
        if let Some(inner) = a.get("accounts") {
            flat_accounts(inner, out);
        } else {
            out.push(a.clone());
        }
    }
}

/// The declared accounts (count, order by fixed address, signer and writable flags) and the
/// data (discriminator ‖ IDL-encoded `args`) of `ix` match the IDL. Returns the number of
/// declared accounts (the rest are `remaining_accounts`).
fn check_ix(name: &str, ix: &Instruction, args: &Value) -> usize {
    let idl = idl();
    let def = find(&idl["instructions"], name);
    let mut accounts = vec![];
    flat_accounts(&def["accounts"], &mut accounts);
    assert!(ix.accounts.len() >= accounts.len(), "{name} account count");
    assert_eq!(ix.program_id, program(), "{name} program");
    for (meta, a) in ix.accounts.iter().zip(&accounts) {
        let n = a["name"].as_str().unwrap();
        let writable = a["writable"].as_bool().unwrap_or(false);
        let is_signer = a["signer"].as_bool().unwrap_or(false);
        assert_eq!(meta.is_writable, writable, "{name}.{n} writable");
        assert_eq!(meta.is_signer, is_signer, "{name}.{n} signer");
        if let Some(addr) = a["address"].as_str() {
            assert_eq!(meta.pubkey.to_string(), addr, "{name}.{n} address");
        }
        if n == "event_authority" {
            assert_eq!(meta.pubkey, Pdas::new(program()).event_authority());
        }
        if n == "program" {
            assert_eq!(meta.pubkey, program());
        }
    }
    let mut want = disc_of(def).to_vec();
    for a in def["args"].as_array().unwrap() {
        let an = a["name"].as_str().unwrap();
        enc(&idl, &a["type"], &args[an], &mut want);
    }
    assert_eq!(ix.data, want, "{name} data");
    accounts.len()
}

/// The pubkey of `ix`'s declared account `account`.
fn account_of(name: &str, ix: &Instruction, account: &str) -> Pubkey {
    let idl = idl();
    let mut accounts = vec![];
    flat_accounts(&find(&idl["instructions"], name)["accounts"], &mut accounts);
    let at = accounts.iter().position(|a| a["name"] == account).unwrap();
    ix.accounts[at].pubkey
}

fn sample_pending(user: Pubkey, nonce: u64) -> PendingAccount {
    PendingAccount {
        user,
        nonce,
        stake_u: 5_000_000,
        copy_fee_u: 0,
        legs: vec![accounts::QuoteLeg::default(); 2],
        odds_offered_u: 3_000_000,
        min_payout_per_unit_u: 2_900_000,
        quote_expires_at: 10,
        quote_hash: [3; 32],
        expires_at: 50,
        created_at: 5,
        is_tip: false,
        tipster: None,
        copied_from: None,
        rent_payer: Pubkey::default(),
    }
}

#[test]
fn instructions_match_idl() {
    let p = Pdas::new(program());
    let (a, b, c) = (
        Pubkey::new_unique(),
        Pubkey::new_unique(),
        Pubkey::new_unique(),
    );
    let mint = p.usdc_mint();

    let params = Params {
        crank: a,
        quote_signer: b,
        max_legs: 5,
        max_payout: 10_000_000_000,
        min_stake: 1_000_000,
        reserve: 100_000_000,
        copy_fee_bps: 200,
        tipster_share_bps: 8_000,
    };
    let ixn = ix::init_config(&p, &c, &b, &params);
    check_ix(
        "init_config",
        &ixn,
        &json!({"authority": b.to_string(), "params": {
            "crank": a.to_string(), "quote_signer": b.to_string(), "max_legs": 5,
            "max_payout": 10_000_000_000_u64, "min_stake": 1_000_000, "reserve": 100_000_000,
            "copy_fee_bps": 200, "tipster_share_bps": 8_000}}),
    );
    assert_eq!(
        account_of("init_config", &ixn, "program_data"),
        p.program_data()
    );
    assert_eq!(account_of("init_config", &ixn, "usdc_mint"), mint);

    let ixn = ix::fund_vault(&p, &a, &b, 77);
    check_ix("fund_vault", &ixn, &json!({"amount": 77}));
    assert_eq!(account_of("fund_vault", &ixn, "funder_usdc"), b);
    assert_eq!(account_of("fund_vault", &ixn, "vault_usdc"), p.vault_usdc());

    let ixn = ix::faucet_usdc(&p, &a, &b, 9);
    check_ix("faucet_usdc", &ixn, &json!({"amount": 9}));
    assert_eq!(
        account_of("faucet_usdc", &ixn, "recipient"),
        associated_token_address(&b, &mint)
    );
    assert_eq!(
        account_of("faucet_usdc", &ixn, "faucet_claim"),
        p.faucet_claim(&b)
    );

    let ixn = ix::open_pending(&p, &a, &c, 7, 3_057_291, 1_793_213_040, true, Some(&b));
    check_ix(
        "open_pending",
        &ixn,
        &json!({"nonce": 7, "min_payout_per_unit_u": 3_057_291, "expires_at": 1_793_213_040_i64,
                "is_tip": true, "copied_from": b.to_string()}),
    );
    assert_eq!(
        account_of("open_pending", &ixn, "pending"),
        p.pending(&a, 7)
    );
    assert_eq!(
        account_of("open_pending", &ixn, "escrow_usdc"),
        p.escrow_usdc()
    );
    // L8: the user signs read-only, the payer (here a sponsor) signs writable.
    assert_eq!(account_of("open_pending", &ixn, "user"), a);
    assert_eq!(account_of("open_pending", &ixn, "payer"), c);

    // finalize: plain, own tip, copy.
    let pending_key = p.pending(&a, 7);
    let mut pend = sample_pending(a, 7);
    let ixn = ix::finalize(&p, &c, &pending_key, &pend, &[500_000, 0], 900_000, vec![]);
    let declared = check_ix(
        "finalize",
        &ixn,
        &json!({"fills": [500_000, 0], "r_ppm": 900_000}),
    );
    assert_eq!(ixn.accounts.len(), declared);
    assert_eq!(account_of("finalize", &ixn, "crank"), c);
    assert_eq!(account_of("finalize", &ixn, "pending"), pending_key);
    assert_eq!(account_of("finalize", &ixn, "ticket"), p.ticket(&a, 7));
    // A pre-L8 Pending (zero rent_payer) sends the rent to the user; an L8 one to its payer.
    assert_eq!(account_of("finalize", &ixn, "rent_payer"), a);
    pend.rent_payer = b;
    let ixn = ix::finalize(&p, &c, &pending_key, &pend, &[500_000, 0], 900_000, vec![]);
    assert_eq!(account_of("finalize", &ixn, "rent_payer"), b);
    pend.rent_payer = Pubkey::default();
    assert_eq!(
        account_of("finalize", &ixn, "user_usdc"),
        associated_token_address(&a, &mint)
    );
    assert_eq!(account_of("finalize", &ixn, "escrow_usdc"), p.escrow_usdc());
    assert_eq!(account_of("finalize", &ixn, "vault_usdc"), p.vault_usdc());
    pend.is_tip = true;
    pend.tipster = Some(a);
    let rem = ix::finalize_remaining(&p, &pend, None);
    assert_eq!(rem.len(), 1);
    assert_eq!(rem[0].pubkey, p.tipster(&a));
    assert!(rem[0].is_writable && !rem[0].is_signer);
    pend.is_tip = false;
    pend.tipster = Some(b);
    pend.copied_from = Some(c);
    let rem = ix::finalize_remaining(&p, &pend, Some(&b));
    assert_eq!(rem.len(), 2);
    assert_eq!(rem[0].pubkey, c);
    assert!(!rem[0].is_writable);
    assert_eq!(rem[1].pubkey, associated_token_address(&b, &mint));
    assert!(rem[1].is_writable);
    let ixn = ix::finalize(&p, &c, &pending_key, &pend, &[1, 2], 1_000_000, rem);
    assert_eq!(ixn.accounts.len(), declared + 2);

    let ixn = ix::refund_pending(&p, &c, &a, 7, &b);
    check_ix("refund_pending", &ixn, &json!({}));
    assert_eq!(account_of("refund_pending", &ixn, "pending"), pending_key);
    assert_eq!(account_of("refund_pending", &ixn, "rent_payer"), b);
    assert_eq!(
        account_of("refund_pending", &ixn, "user_usdc"),
        associated_token_address(&a, &mint)
    );

    let ixn = ix::resolve_leg(&p, &c, &b, 3, 1_000_000);
    check_ix(
        "resolve_leg",
        &ixn,
        &json!({"leg_index": 3, "r_ppm": 1_000_000}),
    );
    assert_eq!(account_of("resolve_leg", &ixn, "ticket"), b);

    let mut t = sample_ticket(a);
    let tk = p.ticket(&a, 7);
    let ixn = ix::settle(&p, &c, &tk, &t);
    let declared = check_ix("settle", &ixn, &json!({}));
    assert_eq!(ixn.accounts.len(), declared);
    assert_eq!(account_of("settle", &ixn, "ticket"), tk);
    assert_eq!(
        account_of("settle", &ixn, "user_usdc"),
        associated_token_address(&a, &mint)
    );
    t.tipster = Some(a);
    let ixn = ix::settle(&p, &c, &tk, &t);
    assert_eq!(ixn.accounts.len(), declared + 1);
    assert_eq!(ixn.accounts[declared].pubkey, p.tipster(&a));
    assert!(ixn.accounts[declared].is_writable);
    t.copied_from = Some(b);
    assert_eq!(ix::settle(&p, &c, &tk, &t).accounts.len(), declared);

    // cash_out: the desk's 99 bytes are the argument; an own tip appends its profile.
    let mut t = sample_ticket(a);
    let mut msg = consts::CASHOUT_TAG.to_vec();
    msg.extend_from_slice(program().as_ref());
    msg.extend_from_slice(tk.as_ref());
    msg.extend_from_slice(&12_000_000_u64.to_le_bytes());
    msg.extend_from_slice(&1_793_216_600_i64.to_le_bytes());
    let ixn = ix::cash_out(&p, &tk, &t, &msg);
    let declared = check_ix(
        "cash_out",
        &ixn,
        &json!({"quote": {"tag": consts::CASHOUT_TAG.to_vec(), "program_id": program().to_string(),
                "ticket": tk.to_string(), "c_u": 12_000_000, "ts": 1_793_216_600_i64}}),
    );
    assert_eq!(ixn.accounts.len(), declared);
    assert_eq!(account_of("cash_out", &ixn, "user"), a);
    assert_eq!(account_of("cash_out", &ixn, "ticket"), tk);
    assert_eq!(
        account_of("cash_out", &ixn, "user_usdc"),
        associated_token_address(&a, &mint)
    );
    assert_eq!(account_of("cash_out", &ixn, "usdc_mint"), mint);
    assert_eq!(account_of("cash_out", &ixn, "vault_usdc"), p.vault_usdc());
    t.tipster = Some(a);
    let ixn = ix::cash_out(&p, &tk, &t, &msg);
    assert_eq!(ixn.accounts.len(), declared + 1);
    assert_eq!(ixn.accounts[declared].pubkey, p.tipster(&a));
    assert!(ixn.accounts[declared].is_writable && !ixn.accounts[declared].is_signer);

    // pause: authority signs, config written.
    for paused in [true, false] {
        let ixn = ix::pause(&p, &a, paused);
        let declared = check_ix("pause", &ixn, &json!({ "paused": paused }));
        assert_eq!(ixn.accounts.len(), declared);
        assert_eq!(account_of("pause", &ixn, "authority"), a);
        assert_eq!(account_of("pause", &ixn, "config"), p.config());
    }

    // ATA create: payer signs, the ATA address is derived.
    let ata = ix::create_ata_idempotent(&c, &a, &mint);
    assert_eq!(ata.program_id, ASSOCIATED_TOKEN_PROGRAM_ID);
    assert_eq!(ata.accounts[1].pubkey, associated_token_address(&a, &mint));
    assert_eq!(ata.data, vec![1]);
}

fn sample_ticket(user: Pubkey) -> TicketAccount {
    TicketAccount {
        user,
        nonce: 7,
        legs: vec![],
        stake_u: 1,
        refund_u: 0,
        payout_u: 2,
        odds_u: 2,
        odds_offered_u: 2,
        min_payout_per_unit_u: 1,
        r_ppm: 1_000_000,
        fee_u: 0,
        copy_fee_u: 0,
        state: TicketState::Open,
        tipster: None,
        copied_from: None,
        quote_hash: [0; 32],
        minted_at: 0,
        resolved_at: 0,
        paid_u: 0,
    }
}

#[test]
fn pdas_use_little_endian_nonces() {
    let p = Pdas::new(program());
    let user = Pubkey::new_from_array([4; 32]);
    let nonce = 0x0102_0304_0506_0708_u64;
    let want = Pubkey::find_program_address(
        &[b"ticket", user.as_ref(), &[8, 7, 6, 5, 4, 3, 2, 1]],
        &program(),
    )
    .0;
    assert_eq!(p.ticket(&user, nonce), want);
    assert_ne!(p.pending(&user, nonce), want);
    assert_eq!(
        p.usdc_mint(),
        Pubkey::find_program_address(&[b"mock", b"USDC"], &program()).0
    );
}

// --- accounts -----------------------------------------------------------------------------------

#[test]
fn account_decoders_match_idl_layouts() {
    let idl = idl();
    let p = program();
    let mut seed = 0;

    let v = sample(&idl, &defined("Config"), &mut seed);
    let c = ConfigAccount::decode(&image(&idl, "Config", acc_disc::CONFIG, &v), &p, &p).unwrap();
    assert_eq!(c.authority, pk(&v["authority"]));
    assert_eq!(c.crank, pk(&v["crank"]));
    assert_eq!(c.quote_signer, pk(&v["quote_signer"]));
    assert_eq!(c.usdc_mint, pk(&v["usdc_mint"]));
    assert_eq!(u64::from(c.copy_fee_bps), u(&v["copy_fee_bps"]));
    assert_eq!(u64::from(c.tipster_share_bps), u(&v["tipster_share_bps"]));
    assert_eq!(u64::from(c.max_legs), u(&v["max_legs"]));
    assert_eq!(c.max_payout, u(&v["max_payout"]));
    assert_eq!(c.min_stake, u(&v["min_stake"]));
    assert_eq!(c.reserve, u(&v["reserve"]));
    assert_eq!(c.paused, v["paused"].as_bool().unwrap());

    let v = sample(&idl, &defined("Vault"), &mut seed);
    let vt = VaultAccount::decode(&image(&idl, "Vault", acc_disc::VAULT, &v), &p, &p).unwrap();
    assert_eq!(vt.usdc_ata, pk(&v["usdc_ata"]));
    assert_eq!(vt.escrow_ata, pk(&v["escrow_ata"]));
    assert_eq!(vt.locked, u(&v["locked"]));
    assert_eq!(vt.free, u(&v["free"]));
    assert_eq!(vt.lp_shares, u(&v["lp_shares"]));

    // Pending, with both options Some and then both None (the fields after them move).
    for (tipster, copied) in [(true, true), (false, false), (true, false)] {
        let mut v = sample(&idl, &defined("Pending"), &mut seed);
        v["n"] = json!(3);
        if !tipster {
            v["tipster"] = Value::Null;
        }
        if !copied {
            v["copied_from"] = Value::Null;
        }
        let d = image(&idl, "Pending", acc_disc::PENDING, &v);
        let pd = PendingAccount::decode(&d, &p, &p).unwrap();
        assert_eq!(pd.user, pk(&v["user"]));
        assert_eq!(pd.nonce, u(&v["nonce"]));
        assert_eq!(pd.stake_u, u(&v["stake_u"]));
        assert_eq!(pd.copy_fee_u, u(&v["copy_fee_u"]));
        assert_eq!(pd.legs.len(), 3);
        for (k, l) in pd.legs.iter().enumerate() {
            assert_eq!(*l, quote_leg(&v["legs"][k]));
        }
        assert_eq!(pd.odds_offered_u, u(&v["odds_offered_u"]));
        assert_eq!(pd.min_payout_per_unit_u, u(&v["min_payout_per_unit_u"]));
        assert_eq!(pd.quote_expires_at, i(&v["quote_expires_at"]));
        assert_eq!(pd.quote_hash, bytes32(&v["quote_hash"]));
        assert_eq!(pd.expires_at, i(&v["expires_at"]));
        assert_eq!(pd.created_at, i(&v["created_at"]));
        assert_eq!(pd.is_tip, v["is_tip"].as_bool().unwrap());
        assert_eq!(pd.tipster, opt_pk(&v["tipster"]));
        assert_eq!(pd.copied_from, opt_pk(&v["copied_from"]));
    }

    for (tipster, copied) in [(true, true), (false, false), (false, true)] {
        let mut v = sample(&idl, &defined("Ticket"), &mut seed);
        v["n"] = json!(4);
        v["state"] = json!("CashedOut");
        if !tipster {
            v["tipster"] = Value::Null;
        }
        if !copied {
            v["copied_from"] = Value::Null;
        }
        let d = image(&idl, "Ticket", acc_disc::TICKET, &v);
        assert_eq!(
            d[TicketAccount::STATE_OFFSET],
            2,
            "STATE_OFFSET points at `state`"
        );
        let t = TicketAccount::decode(&d, &p, &p).unwrap();
        assert_eq!(t.user, pk(&v["user"]));
        assert_eq!(t.nonce, u(&v["nonce"]));
        assert_eq!(t.legs.len(), 4);
        for (k, l) in t.legs.iter().enumerate() {
            let lv = &v["legs"][k];
            assert_eq!(l.symbol_hash, bytes32(&lv["symbol_hash"]));
            assert_eq!(l.game_id_hash, bytes32(&lv["game_id_hash"]));
            assert_eq!(l.start_ts, i(&lv["start_ts"]));
            assert_eq!(u64::from(l.side), u(&lv["side"]));
            assert_eq!(l.price_u, u(&lv["price_u"]));
            assert_eq!(l.fill_u, u(&lv["fill_u"]));
            assert_eq!(u64::from(l.m1_ppm), u(&lv["m1_ppm"]));
            assert_eq!(l.hedged, lv["hedged"].as_bool().unwrap());
            let status = match l.status {
                LegStatus::Open => "Open",
                LegStatus::Won => "Won",
                LegStatus::Lost => "Lost",
                LegStatus::Partial => "Partial",
            };
            assert_eq!(lv["status"], status);
            assert_eq!(u64::from(l.resolution_ppm), u(&lv["resolution_ppm"]));
        }
        assert_eq!(t.stake_u, u(&v["stake_u"]));
        assert_eq!(t.refund_u, u(&v["refund_u"]));
        assert_eq!(t.payout_u, u(&v["payout_u"]));
        assert_eq!(t.odds_u, u(&v["odds_u"]));
        assert_eq!(t.odds_offered_u, u(&v["odds_offered_u"]));
        assert_eq!(t.min_payout_per_unit_u, u(&v["min_payout_per_unit_u"]));
        assert_eq!(u64::from(t.r_ppm), u(&v["r_ppm"]));
        assert_eq!(t.fee_u, u(&v["fee_u"]));
        assert_eq!(t.copy_fee_u, u(&v["copy_fee_u"]));
        assert_eq!(t.state, TicketState::CashedOut);
        assert_eq!(t.tipster, opt_pk(&v["tipster"]));
        assert_eq!(t.copied_from, opt_pk(&v["copied_from"]));
        assert_eq!(t.quote_hash, bytes32(&v["quote_hash"]));
        assert_eq!(t.minted_at, i(&v["minted_at"]));
        assert_eq!(t.resolved_at, i(&v["resolved_at"]));
        assert_eq!(t.paid_u, u(&v["paid_u"]));
    }

    // Foreign, mistyped or truncated accounts are rejected.
    let v = sample(&idl, &defined("Vault"), &mut seed);
    let d = image(&idl, "Vault", acc_disc::VAULT, &v);
    let other = Pubkey::new_unique();
    assert_eq!(
        VaultAccount::decode(&d, &other, &p),
        Err(DecodeError::WrongOwner)
    );
    assert_eq!(
        ConfigAccount::decode(&d, &p, &p),
        Err(DecodeError::WrongDiscriminator)
    );
    assert_eq!(
        VaultAccount::decode(&d[..40], &p, &p),
        Err(DecodeError::TooShort)
    );
}

// --- events -------------------------------------------------------------------------------------

#[test]
fn event_decoders_match_idl_layouts() {
    let idl = idl();
    let mut seed = 500;

    let v = sample(&idl, &defined("PendingOpened"), &mut seed);
    match ProgramEvent::decode(&image(&idl, "PendingOpened", ev_disc::PENDING_OPENED, &v)).unwrap()
    {
        ProgramEvent::PendingOpened(e) => {
            assert_eq!(e.pending, pk(&v["pending"]));
            assert_eq!(e.user, pk(&v["user"]));
            assert_eq!(e.nonce, u(&v["nonce"]));
            assert_eq!(e.legs.len(), 2);
            assert_eq!(e.legs[1], quote_leg(&v["legs"][1]));
            assert_eq!(e.stake_u, u(&v["stake_u"]));
            assert_eq!(e.copy_fee_u, u(&v["copy_fee_u"]));
            assert_eq!(e.odds_offered_u, u(&v["odds_offered_u"]));
            assert_eq!(e.min_payout_per_unit_u, u(&v["min_payout_per_unit_u"]));
            assert_eq!(e.quote_expires_at, i(&v["quote_expires_at"]));
            assert_eq!(e.expires_at, i(&v["expires_at"]));
            assert_eq!(e.quote_hash, bytes32(&v["quote_hash"]));
            assert_eq!(e.is_tip, v["is_tip"].as_bool().unwrap());
            assert_eq!(e.copied_from, opt_pk(&v["copied_from"]));
        }
        e => panic!("{e:?}"),
    }

    let v = sample(&idl, &defined("PendingRefunded"), &mut seed);
    assert_eq!(
        ProgramEvent::decode(&image(
            &idl,
            "PendingRefunded",
            ev_disc::PENDING_REFUNDED,
            &v
        ))
        .unwrap(),
        ProgramEvent::PendingRefunded {
            pending: pk(&v["pending"]),
            user: pk(&v["user"]),
            nonce: u(&v["nonce"]),
            amount_u: u(&v["amount_u"]),
        }
    );

    let mut v = sample(&idl, &defined("TicketMinted"), &mut seed);
    v["tipster"] = Value::Null;
    match ProgramEvent::decode(&image(&idl, "TicketMinted", ev_disc::TICKET_MINTED, &v)).unwrap() {
        ProgramEvent::TicketMinted(e) => {
            let want = ev::TicketMintedEvent {
                ticket: pk(&v["ticket"]),
                pending: pk(&v["pending"]),
                user: pk(&v["user"]),
                nonce: u(&v["nonce"]),
                fills: v["fills"].as_array().unwrap().iter().map(u).collect(),
                r_ppm: u32::try_from(u(&v["r_ppm"])).unwrap(),
                stake_u: u(&v["stake_u"]),
                refund_u: u(&v["refund_u"]),
                payout_u: u(&v["payout_u"]),
                odds_u: u(&v["odds_u"]),
                fee_u: u(&v["fee_u"]),
                tipster: None,
                copied_from: opt_pk(&v["copied_from"]),
                vault_locked: u(&v["vault_locked"]),
                vault_free: u(&v["vault_free"]),
            };
            assert_eq!(*e, want);
        }
        e => panic!("{e:?}"),
    }

    let v = sample(&idl, &defined("LegResolved"), &mut seed);
    assert_eq!(
        ProgramEvent::decode(&image(&idl, "LegResolved", ev_disc::LEG_RESOLVED, &v)).unwrap(),
        ProgramEvent::LegResolved {
            ticket: pk(&v["ticket"]),
            leg_index: u8::try_from(u(&v["leg_index"])).unwrap(),
            r_ppm: u32::try_from(u(&v["r_ppm"])).unwrap(),
            rtilde_ppm: u32::try_from(u(&v["rtilde_ppm"])).unwrap(),
            resolver: pk(&v["resolver"]),
        }
    );

    let v = sample(&idl, &defined("TicketSettled"), &mut seed);
    assert_eq!(
        ProgramEvent::decode(&image(&idl, "TicketSettled", ev_disc::TICKET_SETTLED, &v)).unwrap(),
        ProgramEvent::TicketSettled {
            ticket: pk(&v["ticket"]),
            user: pk(&v["user"]),
            payout_u: u(&v["payout_u"]),
            paid_u: u(&v["paid_u"]),
        }
    );

    let v = sample(&idl, &defined("TicketCashedOut"), &mut seed);
    assert_eq!(
        ProgramEvent::decode(&image(
            &idl,
            "TicketCashedOut",
            ev_disc::TICKET_CASHED_OUT,
            &v
        ))
        .unwrap(),
        ProgramEvent::TicketCashedOut {
            ticket: pk(&v["ticket"]),
            user: pk(&v["user"]),
            payout_u: u(&v["payout_u"]),
            c_u: u(&v["c_u"]),
            ts: i(&v["ts"]),
        }
    );

    let v = sample(&idl, &defined("VaultFunded"), &mut seed);
    assert_eq!(
        ProgramEvent::decode(&image(&idl, "VaultFunded", ev_disc::VAULT_FUNDED, &v)).unwrap(),
        ProgramEvent::VaultFunded {
            funder: pk(&v["funder"]),
            amount_u: u(&v["amount_u"]),
            vault_free: u(&v["vault_free"]),
        }
    );

    // Unknown events pass through; truncated ones fail.
    let tipster_paid = disc_of(find(&idl["events"], "TipsterPaid"));
    assert_eq!(
        ProgramEvent::decode(&tipster_paid).unwrap(),
        ProgramEvent::Other(tipster_paid)
    );
    assert!(ProgramEvent::decode(&ev_disc::LEG_RESOLVED).is_err());
    assert!(ProgramEvent::decode(&[1, 2]).is_err());
}

// --- the C3 vector --------------------------------------------------------------------------------

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|k| u8::from_str_radix(&s[k..k + 2], 16).unwrap())
        .collect()
}

#[test]
fn quote_encoder_and_verify_ix_match_the_vector() {
    use ed25519_dalek::{Signer, SigningKey};

    let v: Value = serde_json::from_str(VECTOR).unwrap();
    let q = &v["quote"];
    let inputs = &q["inputs"];
    let legs = inputs["legs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            let symbol = l["symbol"].as_str().unwrap();
            let game = l["game_id"].as_str().unwrap();
            assert_eq!(hex(&sha256(symbol.as_bytes())), l["symbol_hash"]);
            assert_eq!(hex(&sha256(game.as_bytes())), l["game_id_hash"]);
            accounts::QuoteLeg {
                symbol_hash: sha256(symbol.as_bytes()),
                game_id_hash: sha256(game.as_bytes()),
                start_ts: i(&l["start_ts"]),
                side: u8::try_from(u(&l["side"])).unwrap(),
                price_u: u(&l["price_u"]),
                m1_ppm: u32::try_from(u(&l["m1_ppm"])).unwrap(),
            }
        })
        .collect();
    let msg = QuoteMsg {
        program_id: program(),
        legs,
        stake_u: u(&inputs["stake_u"]),
        odds_offered_u: u(&inputs["odds_offered_u"]),
        quote_expires_at: i(&inputs["quote_expires_at"]),
    }
    .encode();
    assert_eq!(hex(&msg), q["message_hex"]);
    assert_eq!(hex(&sha256(&msg)), q["message_sha256_hex"]);

    let seed: [u8; 32] = unhex(v["signer_seed_hex"].as_str().unwrap())
        .try_into()
        .unwrap();
    let key = SigningKey::from_bytes(&seed);
    let signer = Pubkey::new_from_array(key.verifying_key().to_bytes());
    assert_eq!(signer.to_string(), v["signer_pubkey"]);
    let sig = key.sign(&msg).to_bytes();
    assert_eq!(hex(&sig), q["signature_hex"]);
    let verify = ix::ed25519_verify(&msg, &sig, &signer);
    assert_eq!(verify.program_id, ED25519_PROGRAM_ID);
    assert_eq!(hex(&verify.data), q["ed25519_ix_hex"]);

    let op = &q["open_pending"];
    let user = Pubkey::new_unique();
    let ixn = ix::open_pending(
        &Pdas::new(program()),
        &user,
        &user,
        u(&op["nonce"]),
        u(&op["min_payout_per_unit_u"]),
        i(&op["expires_at"]),
        op["is_tip"].as_bool().unwrap(),
        None,
    );
    assert_eq!(hex(&ixn.data), op["ix_data_hex"]);

    // cash_out: the vector's ticket, c_u, ts → its instruction data.
    let co = &v["cash_out"];
    let ci = &co["inputs"];
    let user: Pubkey = ci["user"].as_str().unwrap().parse().unwrap();
    let p = Pdas::new(program());
    let tk = p.ticket(&user, u(&ci["nonce"]));
    assert_eq!(tk.to_string(), ci["ticket"]);
    let mut msg = consts::CASHOUT_TAG.to_vec();
    msg.extend_from_slice(program().as_ref());
    msg.extend_from_slice(tk.as_ref());
    msg.extend_from_slice(&u(&ci["c_u"]).to_le_bytes());
    msg.extend_from_slice(&i(&ci["ts"]).to_le_bytes());
    let mut t = sample_ticket(user);
    t.nonce = u(&ci["nonce"]);
    let ixn = ix::cash_out(&p, &tk, &t, &msg);
    assert_eq!(hex(&ixn.data), co["cash_out_ix_data_hex"]);
    let sig = key.sign(&msg).to_bytes();
    assert_eq!(
        hex(&ix::ed25519_verify(&msg, &sig, &signer).data),
        co["ed25519_ix_hex"]
    );
}
