//! The app's instructions against the program's own artifacts: byte equality with
//! `fixtures/schedina/quote_vector.json`, account metas (order, signer, writable, PDA seeds,
//! fixed addresses) against `programs/idl/schedina.json`.

use std::collections::HashMap;
use std::path::PathBuf;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use schedina_tx::*;
use serde_json::Value;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load(rel: &str) -> Value {
    let path = repo().join(rel);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

fn vector() -> Value {
    load("fixtures/schedina/quote_vector.json")
}

fn idl() -> Value {
    load("programs/idl/schedina.json")
}

fn hex(s: &str) -> Vec<u8> {
    assert!(s.len().is_multiple_of(2));
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn pk(v: &Value) -> Pubkey {
    v.as_str().unwrap().parse().unwrap()
}

/// The vector's signed message as the desk sends it: base64 on the wire, decoded by the app.
fn wire(signer: &str, msg_hex: &str, sig_hex: &str) -> SignedMsg {
    SignedMsg::from_wire(
        signer,
        &STANDARD.encode(hex(msg_hex)),
        &STANDARD.encode(hex(sig_hex)),
    )
    .unwrap()
}

fn quote_signed(v: &Value) -> SignedMsg {
    wire(
        v["signer_pubkey"].as_str().unwrap(),
        v["quote"]["message_hex"].as_str().unwrap(),
        v["quote"]["signature_hex"].as_str().unwrap(),
    )
}

fn cashout_signed(v: &Value) -> SignedMsg {
    wire(
        v["signer_pubkey"].as_str().unwrap(),
        v["cash_out"]["message_hex"].as_str().unwrap(),
        v["cash_out"]["signature_hex"].as_str().unwrap(),
    )
}

/// The vector's quote as the desk's JSON would show it.
fn shown_of(v: &Value) -> ShownQuote {
    let q = &v["quote"];
    ShownQuote {
        legs: q["inputs"]["legs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| ShownLeg {
                symbol: l["symbol"].as_str().unwrap().into(),
                game_id: l["game_id"].as_str().unwrap().into(),
                side: l["side"].as_u64().unwrap() as u8,
            })
            .collect(),
        stake_u: q["inputs"]["stake_u"].as_u64().unwrap(),
        odds_offered_u: q["inputs"]["odds_offered_u"].as_u64().unwrap(),
        quote_expires_at: q["inputs"]["quote_expires_at"].as_i64().unwrap(),
        min_payout_per_unit_u: q["open_pending"]["min_payout_per_unit_u"].as_u64().unwrap(),
    }
}

#[test]
fn program_ids_match_the_idl_and_vector() {
    let v = vector();
    assert_eq!(PROGRAM_ID, pk(&v["program_id"]));
    assert_eq!(PROGRAM_ID, pk(&idl()["address"]));
    assert_eq!(ED25519_PROGRAM_ID, pk(&v["ed25519_program_id"]));
}

#[test]
fn discriminators_match_the_idl() {
    let idl = idl();
    for (name, disc) in [
        ("open_pending", OPEN_PENDING_DISC),
        ("cash_out", CASH_OUT_DISC),
        ("faucet_usdc", FAUCET_USDC_DISC),
    ] {
        let ix = idl_ix(&idl, name);
        let want: Vec<u8> = ix["discriminator"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b.as_u64().unwrap() as u8)
            .collect();
        assert_eq!(disc.to_vec(), want, "{name}");
    }
}

#[test]
fn quote_verify_instruction_is_byte_equal() {
    let v = vector();
    let ix = ed25519_verify_ix(&quote_signed(&v)).unwrap();
    assert_eq!(ix.program_id, ED25519_PROGRAM_ID);
    assert!(ix.accounts.is_empty());
    assert_eq!(ix.data, hex(v["quote"]["ed25519_ix_hex"].as_str().unwrap()));
}

#[test]
fn cashout_verify_instruction_is_byte_equal() {
    let v = vector();
    let ix = ed25519_verify_ix(&cashout_signed(&v)).unwrap();
    assert_eq!(
        ix.data,
        hex(v["cash_out"]["ed25519_ix_hex"].as_str().unwrap())
    );
    assert_eq!(
        v["cash_out"]["message_offset_in_ix_data"].as_u64().unwrap(),
        8,
        "cash_out data = discriminator ‖ message"
    );
}

#[test]
fn open_pending_data_is_byte_equal() {
    let v = vector();
    let op = &v["quote"]["open_pending"];
    assert!(op["copied_from"].is_null());
    let args = OpenPendingArgs {
        nonce: op["nonce"].as_u64().unwrap(),
        min_payout_per_unit_u: op["min_payout_per_unit_u"].as_u64().unwrap(),
        expires_at: op["expires_at"].as_i64().unwrap(),
        is_tip: op["is_tip"].as_bool().unwrap(),
        copied_from: None,
    };
    assert_eq!(args.data(), hex(op["ix_data_hex"].as_str().unwrap()));
}

#[test]
fn open_pending_ixs_carry_the_quote_untouched() {
    let v = vector();
    let q = &v["quote"];
    let op = &q["open_pending"];
    let signed = quote_signed(&v);
    let view = QuoteMsgView::parse(&signed.msg).unwrap();
    assert_eq!(view.n, q["inputs"]["legs"].as_array().unwrap().len());
    assert_eq!(view.stake_u, q["inputs"]["stake_u"].as_u64().unwrap());
    assert_eq!(
        view.odds_offered_u,
        q["inputs"]["odds_offered_u"].as_u64().unwrap()
    );
    assert_eq!(
        view.quote_expires_at,
        q["inputs"]["quote_expires_at"].as_i64().unwrap()
    );
    let shown = shown_of(&v);
    let user: Pubkey = pk(&v["cash_out"]["inputs"]["user"]);
    let nonce = op["nonce"].as_u64().unwrap();
    // The vector's expires_at is quote_expires_at + 40; the app uses now + 45.
    let now = op["expires_at"].as_i64().unwrap() - PENDING_TTL_S;
    let [verify, open] = open_pending_ixs(
        &user,
        &user,
        &signed,
        &shown,
        nonce,
        now,
        OpenOptions::default(),
    )
    .unwrap();
    assert_eq!(verify.data, hex(q["ed25519_ix_hex"].as_str().unwrap()));
    assert_eq!(open.data, hex(op["ix_data_hex"].as_str().unwrap()));
    assert_eq!(open.accounts[2].pubkey, pda::pending(&user, nonce));
    assert_eq!(open.accounts[3].pubkey, pda::user_usdc(&user));

    // Refusals: anything that differs from what was shown, a bad floor, another program.
    let refused = |other: &ShownQuote| {
        open_pending_ixs(
            &user,
            &user,
            &signed,
            other,
            nonce,
            now,
            OpenOptions::default(),
        )
        .is_err()
    };
    let mut other = shown.clone();
    other.odds_offered_u += 1;
    assert!(refused(&other));
    let mut other = shown.clone();
    other.stake_u -= 1;
    assert!(refused(&other));
    let mut other = shown.clone();
    other.quote_expires_at += 1;
    assert!(refused(&other));
    let mut other = shown.clone();
    other.min_payout_per_unit_u = view.odds_offered_u + 1;
    assert!(refused(&other));
    let mut other = shown.clone();
    other.min_payout_per_unit_u = 0;
    assert!(refused(&other));
    // Leg by leg: another team's symbol, another game, the other side, a swapped order, a
    // missing leg.
    let mut other = shown.clone();
    other.legs[0].symbol = "NBA_26OCT28_PHIIND_ML.IND".into();
    assert!(refused(&other));
    let mut other = shown.clone();
    other.legs[1].game_id = "nba-cha-det-2026-10-29".into();
    assert!(refused(&other));
    let mut other = shown.clone();
    other.legs[0].side ^= 1;
    assert!(refused(&other));
    let mut other = shown.clone();
    other.legs.swap(0, 1);
    assert!(refused(&other));
    let mut other = shown.clone();
    other.legs.pop();
    assert!(refused(&other));
    let mut tampered = signed.clone();
    tampered.msg[20] ^= 1;
    assert!(
        open_pending_ixs(
            &user,
            &user,
            &tampered,
            &shown,
            nonce,
            now,
            OpenOptions::default()
        )
        .is_err()
    );
    let mut short = signed.clone();
    short.msg.pop();
    assert!(QuoteMsgView::parse(&short.msg).is_err());
    let mut cashout_as_quote = cashout_signed(&v);
    cashout_as_quote.msg.truncate(99);
    assert!(QuoteMsgView::parse(&cashout_as_quote.msg).is_err());
}

#[test]
fn cash_out_data_and_ticket_pda_match_the_vector() {
    let v = vector();
    let c = &v["cash_out"];
    let user = pk(&c["inputs"]["user"]);
    let nonce = c["inputs"]["nonce"].as_u64().unwrap();
    let ticket = pk(&c["inputs"]["ticket"]);
    // Seeds ["ticket", user, nonce u64 LE]: a wrong endianness gives another address.
    assert_eq!(pda::ticket(&user, nonce), ticket);
    assert_ne!(
        Pubkey::find_program_address(
            &[b"ticket", user.as_ref(), &nonce.to_be_bytes()],
            &PROGRAM_ID
        )
        .0,
        ticket
    );
    let signed = cashout_signed(&v);
    let view = CashOutMsgView::parse(&signed.msg).unwrap();
    assert_eq!(view.ticket, ticket);
    assert_eq!(view.c_u, c["inputs"]["c_u"].as_u64().unwrap());
    assert_eq!(view.ts, c["inputs"]["ts"].as_i64().unwrap());
    let t = CashOutTicket {
        user,
        nonce,
        ticket,
        own_tip: false,
    };
    let [verify, cash] = cash_out_ixs(&t, &signed, view.c_u).unwrap();
    assert_eq!(verify.data, hex(c["ed25519_ix_hex"].as_str().unwrap()));
    assert_eq!(cash.data, hex(c["cash_out_ix_data_hex"].as_str().unwrap()));

    // Refusals.
    assert!(cash_out_ixs(&t, &signed, view.c_u + 1).is_err());
    let wrong_nonce = CashOutTicket {
        nonce: nonce + 1,
        ..t
    };
    assert!(cash_out_ixs(&wrong_nonce, &signed, view.c_u).is_err());
    let other_ticket_pda = pda::ticket(&user, nonce + 1);
    let other = CashOutTicket {
        nonce: nonce + 1,
        ticket: other_ticket_pda,
        ..t
    };
    assert!(cash_out_ixs(&other, &signed, view.c_u).is_err());
}

#[test]
fn base64_is_strict() {
    let v = vector();
    let signer = v["signer_pubkey"].as_str().unwrap();
    let sig = STANDARD.encode(hex(v["quote"]["signature_hex"].as_str().unwrap()));
    let msg = STANDARD.encode(hex(v["quote"]["message_hex"].as_str().unwrap()));
    assert!(SignedMsg::from_wire(signer, &msg, &sig).is_ok());
    // Unpadded, URL-safe alphabet, a 63-byte signature, a bad key.
    if msg.ends_with('=') {
        assert!(SignedMsg::from_wire(signer, msg.trim_end_matches('='), &sig).is_err());
    }
    assert!(SignedMsg::from_wire(signer, &msg, sig.trim_end_matches('=')).is_err());
    let url = msg.replace('+', "-").replace('/', "_");
    if url != msg {
        assert!(SignedMsg::from_wire(signer, &url, &sig).is_err());
    }
    let short = STANDARD.encode([0u8; 63]);
    assert_eq!(
        SignedMsg::from_wire(signer, &msg, &short),
        Err(TxError::SignatureLength(63))
    );
    assert!(SignedMsg::from_wire("not-a-key", &msg, &sig).is_err());
}

// ------------------------------------------------------------------- account metas vs IDL

fn idl_ix<'a>(idl: &'a Value, name: &str) -> &'a Value {
    idl["instructions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == name)
        .unwrap_or_else(|| panic!("{name} not in IDL"))
}

/// Resolve an IDL PDA seed list: `const` bytes, `arg` (u64 LE by name), `account` (a key by
/// path, e.g. `user` or `ticket.user`, or a u64 LE path like `ticket.nonce`).
fn idl_pda(acc: &Value, keys: &HashMap<&str, Pubkey>, ints: &HashMap<&str, u64>) -> Option<Pubkey> {
    let pda = acc.get("pda")?;
    let seeds: Vec<Vec<u8>> = pda["seeds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| match s["kind"].as_str().unwrap() {
            "const" => s["value"]
                .as_array()
                .unwrap()
                .iter()
                .map(|b| b.as_u64().unwrap() as u8)
                .collect(),
            "arg" | "account" => {
                let path = s["path"].as_str().unwrap();
                if let Some(k) = keys.get(path) {
                    k.to_bytes().to_vec()
                } else if let Some(n) = ints.get(path) {
                    n.to_le_bytes().to_vec()
                } else {
                    panic!("no value for seed path {path}")
                }
            }
            k => panic!("seed kind {k}"),
        })
        .collect();
    assert!(pda.get("program").is_none(), "PDA of another program");
    let refs: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
    Some(Pubkey::find_program_address(&refs, &PROGRAM_ID).0)
}

/// Compare an instruction's metas with the IDL: same names in order, same signer/writable
/// flags, fixed addresses and PDAs equal. `named` gives the expected key of accounts the IDL
/// leaves open (`user_usdc`, `event_authority`, …). Returns how many metas the IDL declares.
fn assert_matches_idl(
    name: &str,
    ix: &Instruction,
    named: &HashMap<&str, Pubkey>,
    ints: &HashMap<&str, u64>,
) -> usize {
    let idl = idl();
    let accounts = idl_ix(&idl, name)["accounts"].as_array().unwrap();
    assert!(ix.accounts.len() >= accounts.len(), "{name}: too few metas");
    for (i, (acc, meta)) in accounts.iter().zip(&ix.accounts).enumerate() {
        let an = acc["name"].as_str().unwrap();
        let signer = acc.get("signer").and_then(Value::as_bool).unwrap_or(false);
        let writable = acc
            .get("writable")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        assert_eq!(meta.is_signer, signer, "{name}[{i}] {an} signer");
        assert_eq!(meta.is_writable, writable, "{name}[{i}] {an} writable");
        if let Some(addr) = acc.get("address") {
            assert_eq!(meta.pubkey, pk(addr), "{name}[{i}] {an} address");
        } else if let Some(p) = idl_pda(acc, named, ints) {
            assert_eq!(meta.pubkey, p, "{name}[{i}] {an} pda");
        } else if an == "program" {
            assert_eq!(meta.pubkey, PROGRAM_ID, "{name}[{i}] program");
        } else {
            let want = named
                .get(an)
                .unwrap_or_else(|| panic!("{name}[{i}] {an}: no expected key"));
            assert_eq!(meta.pubkey, *want, "{name}[{i}] {an}");
        }
    }
    accounts.len()
}

#[test]
fn open_pending_accounts_match_the_idl() {
    let user: Pubkey = "J2xccRtuG43drESLYznHhLhQkLTdfepcKYbiQ9BsJVaf"
        .parse()
        .unwrap();
    let nonce = 0x0102_0304_0506_0708;
    let args = OpenPendingArgs {
        nonce,
        min_payout_per_unit_u: 1,
        expires_at: 0,
        is_tip: false,
        copied_from: None,
    };
    // L8: a sponsor pays the rent; the user signs read-only.
    let sponsor = Pubkey::new_from_array([7; 32]);
    let ix = open_pending_ix(&user, &sponsor, &pda::user_usdc(&user), &args);
    assert_eq!(ix.program_id, PROGRAM_ID);
    let named = HashMap::from([
        ("user", user),
        ("payer", sponsor),
        ("user_usdc", pda::user_usdc(&user)),
        ("event_authority", pda::event_authority()),
    ]);
    let ints = HashMap::from([("nonce", nonce)]);
    let n = assert_matches_idl("open_pending", &ix, &named, &ints);
    assert_eq!(
        n,
        ix.accounts.len(),
        "no remaining accounts on open_pending"
    );
    // Endianness of the nonce seed is pinned by the IDL derivation above; check LE explicitly.
    assert_eq!(
        ix.accounts[2].pubkey,
        Pubkey::find_program_address(
            &[b"pending", user.as_ref(), &nonce.to_le_bytes()],
            &PROGRAM_ID
        )
        .0
    );
}

#[test]
fn cash_out_accounts_match_the_idl_and_l2_4() {
    let v = vector();
    let user = pk(&v["cash_out"]["inputs"]["user"]);
    let nonce = v["cash_out"]["inputs"]["nonce"].as_u64().unwrap();
    let ticket = pda::ticket(&user, nonce);
    let named = HashMap::from([
        ("user", user),
        ("ticket.user", user),
        ("user_usdc", pda::user_usdc(&user)),
        ("usdc_mint", pda::usdc_mint()),
        ("vault_usdc", pda::vault_usdc()),
        ("event_authority", pda::event_authority()),
    ]);
    let ints = HashMap::from([("ticket.nonce", nonce)]);
    let msg = hex(v["cash_out"]["message_hex"].as_str().unwrap());

    let plain = cash_out_ix(&user, &ticket, &msg, false);
    let n = assert_matches_idl("cash_out", &plain, &named, &ints);
    assert_eq!(
        plain.accounts.len(),
        n,
        "no remaining accounts for a plain ticket"
    );

    // L2-4: an own tip appends the tipster profile, writable, as the first remaining account.
    let tip = cash_out_ix(&user, &ticket, &msg, true);
    assert_matches_idl("cash_out", &tip, &named, &ints);
    assert_eq!(tip.accounts.len(), n + 1);
    let extra = &tip.accounts[n];
    assert_eq!(
        extra.pubkey,
        Pubkey::find_program_address(&[b"tipster", user.as_ref()], &PROGRAM_ID).0
    );
    assert!(extra.is_writable && !extra.is_signer);

    let other: Pubkey = "GmaDrppBC7P5ARKV8g3djiwP89vz1jLK23V2GBjuAEGB"
        .parse()
        .unwrap();
    assert!(is_own_tip(&user, Some(&user), None));
    assert!(!is_own_tip(&user, None, None));
    assert!(!is_own_tip(&user, Some(&other), None));
    assert!(!is_own_tip(&user, Some(&user), Some(&other)));
}

#[test]
fn faucet_accounts_match_the_idl() {
    let user: Pubkey = "J2xccRtuG43drESLYznHhLhQkLTdfepcKYbiQ9BsJVaf"
        .parse()
        .unwrap();
    let [create, faucet]: [Instruction; 2] = faucet_ixs(&user, &user, FAUCET_AMOUNT_U)
        .try_into()
        .unwrap();
    let ata = pda::user_usdc(&user);
    assert_eq!(create.program_id, ATA_PROGRAM_ID);
    assert_eq!(create.data, vec![1]);
    assert_eq!(create.accounts[1].pubkey, ata);
    let named = HashMap::from([
        ("caller", user),
        ("usdc_mint", pda::usdc_mint()),
        ("recipient", ata),
        ("recipient.owner", user),
    ]);
    let n = assert_matches_idl("faucet_usdc", &faucet, &named, &HashMap::new());
    assert_eq!(n, faucet.accounts.len());
    let mut data = FAUCET_USDC_DISC.to_vec();
    data.extend_from_slice(&FAUCET_AMOUNT_U.to_le_bytes());
    assert_eq!(faucet.data, data);
}

#[test]
fn compiled_transaction_keeps_verify_right_before_the_schedina_instruction() {
    use solana_hash::Hash;
    use solana_message::Message;

    let v = vector();
    let signed = quote_signed(&v);
    let shown = shown_of(&v);
    let user: Pubkey = pk(&v["cash_out"]["inputs"]["user"]);
    let ixs = open_pending_ixs(
        &user,
        &user,
        &signed,
        &shown,
        42,
        1_793_213_000,
        OpenOptions::default(),
    )
    .unwrap();
    let msg = Message::new_with_blockhash(&ixs, Some(&user), &Hash::default());
    assert_eq!(msg.header.num_required_signatures, 1);
    assert_eq!(msg.account_keys[0], user, "the user pays and signs");
    let programs: Vec<Pubkey> = msg
        .instructions
        .iter()
        .map(|c| msg.account_keys[usize::from(c.program_id_index)])
        .collect();
    assert_eq!(programs, vec![ED25519_PROGRAM_ID, PROGRAM_ID]);
    assert_eq!(
        msg.instructions[0].data, ixs[0].data,
        "verify data untouched"
    );
    // Every writable account of open_pending stays writable in the compiled message (header:
    // signed-writable, signed-readonly, unsigned-writable, unsigned-readonly). A self-paying
    // user is both `user` (read-only) and `payer` (writable): the message keeps it writable.
    let h = &msg.header;
    let signed = usize::from(h.num_required_signatures);
    let writable = |i: usize| {
        if i < signed {
            i < signed - usize::from(h.num_readonly_signed_accounts)
        } else {
            i < msg.account_keys.len() - usize::from(h.num_readonly_unsigned_accounts)
        }
    };
    for meta in &ixs[1].accounts {
        let i = msg
            .account_keys
            .iter()
            .position(|k| *k == meta.pubkey)
            .unwrap();
        let any_writable = ixs[1]
            .accounts
            .iter()
            .any(|m| m.pubkey == meta.pubkey && m.is_writable);
        assert_eq!(writable(i), any_writable, "{} writable", meta.pubkey);
        assert_eq!(i < signed, meta.is_signer, "{} signer", meta.pubkey);
    }
}

#[test]
fn five_leg_quote_fits_the_verify_offsets() {
    // 77 + 85 * 5 = 502 bytes: offsets stay in u16 and the data length is exact.
    let mut msg = QUOTE_TAG.to_vec();
    msg.extend_from_slice(&PROGRAM_ID.to_bytes());
    msg.extend_from_slice(&5u32.to_le_bytes());
    msg.extend(std::iter::repeat_n(0u8, 85 * 5 + 24));
    assert_eq!(msg.len(), 502);
    let view = QuoteMsgView::parse(&msg).unwrap();
    assert_eq!(view.n, 5);
    let signed = SignedMsg {
        signer: PROGRAM_ID,
        msg,
        sig: [0; 64],
    };
    let ix = ed25519_verify_ix(&signed).unwrap();
    assert_eq!(ix.data.len(), 112 + 502);
    assert_eq!(&ix.data[12..14], &502u16.to_le_bytes());
}

// ------------------------------------------------------------- tips and copies (plan 4.B)

/// Borsh of `open_pending`'s arguments built from the IDL's own argument list (names, order,
/// types), so `OpenPendingArgs::data` is checked against the IDL rather than against itself.
fn idl_borsh_open_pending(args: &OpenPendingArgs) -> Vec<u8> {
    let idl = idl();
    let ix = idl_ix(&idl, "open_pending");
    let mut out: Vec<u8> = ix["discriminator"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b.as_u64().unwrap() as u8)
        .collect();
    let names: Vec<&str> = ix["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "nonce",
            "min_payout_per_unit_u",
            "expires_at",
            "is_tip",
            "copied_from"
        ],
        "open_pending argument order"
    );
    for a in ix["args"].as_array().unwrap() {
        let name = a["name"].as_str().unwrap();
        let ty = &a["type"];
        match (name, ty.as_str()) {
            ("nonce", Some("u64")) => out.extend_from_slice(&args.nonce.to_le_bytes()),
            ("min_payout_per_unit_u", Some("u64")) => {
                out.extend_from_slice(&args.min_payout_per_unit_u.to_le_bytes())
            }
            ("expires_at", Some("i64")) => out.extend_from_slice(&args.expires_at.to_le_bytes()),
            ("is_tip", Some("bool")) => out.push(u8::from(args.is_tip)),
            ("copied_from", None) => {
                assert_eq!(ty["option"], "pubkey", "copied_from is Option<Pubkey>");
                match args.copied_from {
                    None => out.push(0),
                    Some(k) => {
                        out.push(1);
                        out.extend_from_slice(k.as_ref());
                    }
                }
            }
            other => panic!("unexpected open_pending argument {other:?}"),
        }
    }
    out
}

fn some_original() -> Pubkey {
    // The desk's example ticket PDA (fixtures/desk/ticket.json).
    "HGMZNFqTMf4bcdUpSoZNG981rPiAYk6utvoPuSJ19DZy"
        .parse()
        .unwrap()
}

fn open_pending_named(user: Pubkey) -> HashMap<&'static str, Pubkey> {
    HashMap::from([
        ("user", user),
        ("payer", user),
        ("user_usdc", pda::user_usdc(&user)),
        ("event_authority", pda::event_authority()),
    ])
}

#[test]
fn open_pending_args_follow_the_idl_borsh_layout() {
    let base = OpenPendingArgs {
        nonce: NONCE_MAX,
        min_payout_per_unit_u: 3_057_291,
        expires_at: 1_793_213_040,
        is_tip: false,
        copied_from: None,
    };
    let tip = OpenPendingArgs {
        is_tip: true,
        ..base
    };
    let copy = OpenPendingArgs {
        copied_from: Some(some_original()),
        ..base
    };
    for args in [base, tip, copy] {
        assert_eq!(args.data(), idl_borsh_open_pending(&args), "{args:?}");
    }
    // disc 8 ‖ nonce 8 ‖ floor 8 ‖ expires_at 8 ‖ is_tip 1 ‖ option tag 1 [‖ pubkey 32].
    assert_eq!(base.data().len(), 34);
    assert_eq!(tip.data()[32], 1);
    assert_eq!(tip.data()[33], 0);
    let d = copy.data();
    assert_eq!(d.len(), 66);
    assert_eq!(d[32], 0);
    assert_eq!(d[33], 1);
    assert_eq!(&d[34..], some_original().as_ref());
}

#[test]
fn tip_open_pending_sets_is_tip_and_follows_the_program_rule() {
    let v = vector();
    let signed = quote_signed(&v);
    let shown = shown_of(&v);
    let user: Pubkey = pk(&v["cash_out"]["inputs"]["user"]);
    let legs = v["quote"]["inputs"]["legs"].as_array().unwrap();
    let first_start = legs
        .iter()
        .map(|l| l["start_ts"].as_i64().unwrap())
        .min()
        .unwrap();
    // The signed starts are read back from the bytes.
    let view = QuoteMsgView::parse(&signed.msg).unwrap();
    for (l, j) in view.legs.iter().zip(legs) {
        assert_eq!(l.start_ts, j["start_ts"].as_i64().unwrap());
    }
    let tip = OpenOptions {
        is_tip: true,
        copied_from: None,
    };
    let now = first_start - 600;
    let nonce = 9_007_199_254_740_991; // 2^53 - 1
    let [_, open] = open_pending_ixs(&user, &user, &signed, &shown, nonce, now, tip).unwrap();
    let want = OpenPendingArgs {
        nonce,
        min_payout_per_unit_u: shown.min_payout_per_unit_u,
        expires_at: now + PENDING_TTL_S,
        is_tip: true,
        copied_from: None,
    };
    assert_eq!(open.data, idl_borsh_open_pending(&want));
    // Same account list as a plain schedina, nothing appended (L2-4: the profile is a
    // finalize / settle / cash_out account, never an open_pending one).
    let ints = HashMap::from([("nonce", nonce)]);
    assert_eq!(
        assert_matches_idl("open_pending", &open, &open_pending_named(user), &ints),
        open.accounts.len()
    );

    // A tip with a leg in play is built like any schedina (D-660).
    for now in [first_start, first_start + 1] {
        assert!(open_pending_ixs(&user, &user, &signed, &shown, nonce, now, tip).is_ok());
    }
    // TipNotAllowed: a tip that copies.
    let tip_copy = OpenOptions {
        is_tip: true,
        copied_from: Some(some_original()),
    };
    assert!(open_pending_ixs(&user, &user, &signed, &shown, nonce, now, tip_copy).is_err());
    // A plain schedina at the same start (the desk decides what is still on offer).
    assert!(
        open_pending_ixs(
            &user,
            &user,
            &signed,
            &shown,
            nonce,
            first_start,
            OpenOptions::default()
        )
        .is_ok()
    );
}

#[test]
fn copy_open_pending_carries_copied_from() {
    let v = vector();
    let signed = quote_signed(&v);
    let shown = shown_of(&v);
    let user: Pubkey = pk(&v["cash_out"]["inputs"]["user"]);
    let nonce = 1u64 << 52;
    let now = 1_793_213_000;
    let copy = OpenOptions {
        is_tip: false,
        copied_from: Some(some_original()),
    };
    let [verify, open] = open_pending_ixs(&user, &user, &signed, &shown, nonce, now, copy).unwrap();
    assert_eq!(
        verify.data,
        hex(v["quote"]["ed25519_ix_hex"].as_str().unwrap())
    );
    let want = OpenPendingArgs {
        nonce,
        min_payout_per_unit_u: shown.min_payout_per_unit_u,
        expires_at: now + PENDING_TTL_S,
        is_tip: false,
        copied_from: Some(some_original()),
    };
    assert_eq!(open.data, idl_borsh_open_pending(&want));
    // L2-4: the original and the tipster's USDC account are finalize's remaining accounts
    // (the crank's); a 5-leg copy's open_pending has no room for more (D-110).
    let ints = HashMap::from([("nonce", nonce)]);
    assert_eq!(
        assert_matches_idl("open_pending", &open, &open_pending_named(user), &ints),
        open.accounts.len()
    );
}

#[test]
fn nonces_stay_within_53_bits() {
    assert_eq!(NONCE_MAX, 9_007_199_254_740_991);
    assert_eq!(nonce_from_entropy([0xff; 8]), NONCE_MAX);
    assert_eq!(nonce_from_entropy([0; 8]), 0);
    assert_eq!(
        nonce_from_entropy([1, 2, 3, 4, 5, 6, 7, 0x80]),
        0x0007_0605_0403_0201
    );
    // A JavaScript number (f64) holds every such nonce exactly, and not the next odd one up.
    assert_eq!(NONCE_MAX as f64 as u64, NONCE_MAX);
    assert_ne!((NONCE_MAX + 2) as f64 as u64, NONCE_MAX + 2);
    let v = vector();
    let user: Pubkey = pk(&v["cash_out"]["inputs"]["user"]);
    assert!(
        open_pending_ixs(
            &user,
            &user,
            &quote_signed(&v),
            &shown_of(&v),
            NONCE_MAX + 1,
            1_793_213_000,
            OpenOptions::default(),
        )
        .is_err()
    );
}

#[test]
fn copies_match_their_original_as_a_set() {
    let leg = |sym: &str, game: &str, side: u8| ShownLeg {
        symbol: sym.into(),
        game_id: game.into(),
        side,
    };
    let a = leg("NBA_26OCT28_PHIIND_ML.PHI", "nba-phi-ind-2026-10-28", 0);
    let b = leg("NBA_26OCT28_HOUMIL_ML.HOU", "nba-hou-mil-2026-10-28", 1);
    let orig = vec![a.clone(), b.clone()];
    assert!(copy_matches(&orig, &[a.clone(), b.clone()]));
    assert!(
        copy_matches(&orig, &[b.clone(), a.clone()]),
        "order is free"
    );
    assert!(
        !copy_matches(&orig, std::slice::from_ref(&a)),
        "a leg missing"
    );
    let mut other_side = b.clone();
    other_side.side = 0;
    assert!(!copy_matches(&orig, &[a.clone(), other_side]));
    assert!(!copy_matches(&orig, &[a.clone(), a.clone(), b]));
}
