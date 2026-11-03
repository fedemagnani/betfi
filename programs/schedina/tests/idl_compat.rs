//! Host-side: the committed IDL (`programs/idl/schedina.json`) is backward compatible
//! with the 1.C IDL that 2.C (crank) and 2.E (app) build transactions from (contract
//! L2-4). `tests/fixtures/idl_1c.json` is `git show 92ba2b1:programs/idl/schedina.json`
//! (sha256 0f8ec141…8943), frozen here so the check needs no git history.
//!
//! Allowed: new instructions, accounts, events, types and constants; errors appended
//! after the 1.C codes; changed doc comments. Everything else of 1.C must be
//! identical, in particular every instruction's discriminator, account list and args.
//!
//! L8 (sponsored transactions) is the one deliberate break, applied to the 1.C IDL by
//! [`with_l8`] before comparing, so nothing else may change: `open_pending` gains a
//! `payer` signer before the event-CPI accounts and its `user` is no longer writable;
//! `finalize` and `refund_pending` send the Pending rent to `rent_payer` (same position as
//! 1.C's `user`); `Pending.reserved: [u8; 32]` becomes `rent_payer: pubkey`. D-664 is the
//! other, applied the same way: `PENDING_TTL_MAX` goes from 60 to 360.

use serde_json::Value;

fn load(rel: &str) -> Value {
    let path = format!("{}/{rel}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn idls() -> (Value, Value) {
    (
        with_l8(load("tests/fixtures/idl_1c.json")),
        load("../idl/schedina.json"),
    )
}

fn entry<'a>(section: &'a mut Value, name: &str) -> &'a mut Value {
    section
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("`{name}` missing from the 1.C IDL"))
}

/// The 1.C IDL with L8's account changes (module docs) applied, docs as L8 writes them.
fn with_l8(mut idl: Value) -> Value {
    let ixs = &mut idl["instructions"];
    let open = entry(ixs, "open_pending")["accounts"]
        .as_array_mut()
        .unwrap();
    let user = open.iter_mut().find(|a| a["name"] == "user").unwrap();
    *user = serde_json::json!({
        "name": "user",
        "docs": ["Authority of `user_usdc`; pays nothing (L8: `payer` pays the rent)."],
        "signer": true,
    });
    let at = open
        .iter()
        .position(|a| a["name"] == "event_authority")
        .unwrap();
    open.insert(
        at,
        serde_json::json!({
            "name": "payer",
            "docs": [
                "Pays the Pending's rent, recorded as `Pending.rent_payer` (L8). Last before the",
                "event-CPI accounts, so the earlier account indices are unchanged."
            ],
            "writable": true,
            "signer": true,
        }),
    );
    for name in ["finalize", "refund_pending"] {
        let accounts = entry(ixs, name)["accounts"].as_array_mut().unwrap();
        let user = accounts.iter_mut().find(|a| a["name"] == "user").unwrap();
        *user = serde_json::json!({
            "name": "rent_payer",
            "docs": [
                "`pending.rent_receiver()`; any owner (a smart-wallet user must still get the refund)."
            ],
            "writable": true,
        });
    }
    let fields = entry(&mut idl["types"], "Pending")["type"]["fields"]
        .as_array_mut()
        .unwrap();
    let reserved = fields.iter_mut().find(|f| f["name"] == "reserved").unwrap();
    *reserved = serde_json::json!({"name": "rent_payer", "type": "pubkey"});
    // D-664: a wallet prompt may take as long as the 5 min quote.
    entry(&mut idl["constants"], "PENDING_TTL_MAX")["value"] = "360".into();
    idl
}

/// `v` with every `docs` key removed, recursively.
fn strip_docs(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(k, _)| k.as_str() != "docs")
                .map(|(k, v)| (k.clone(), strip_docs(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(strip_docs).collect()),
        other => other.clone(),
    }
}

fn by_name<'a>(section: &'a Value, name: &str) -> Option<&'a Value> {
    section
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"].as_str() == Some(name))
}

fn names(section: &Value) -> Vec<String> {
    section
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_string())
        .collect()
}

/// Every 1.C entry of `section` is still there, identical up to doc comments.
fn superset(section: &str, strip: bool) {
    let (old, new) = idls();
    for o in old[section].as_array().unwrap() {
        let name = o["name"].as_str().unwrap();
        let n = by_name(&new[section], name)
            .unwrap_or_else(|| panic!("{section}: `{name}` removed or renamed"));
        let (o, n) = if strip {
            (strip_docs(o), strip_docs(n))
        } else {
            (o.clone(), n.clone())
        };
        assert_eq!(o, n, "{section}: `{name}` changed");
    }
}

#[test]
fn program_address_is_unchanged() {
    let (old, new) = idls();
    assert_eq!(old["address"], new["address"]);
    assert_eq!(
        new["address"].as_str(),
        Some("HXdZFi7tR669G1hXogh3uGGqLYsoZPCSuiutAviLNToW")
    );
}

/// L2-4: arguments and declared account lists byte-identical (account docs included).
#[test]
fn instructions_keep_discriminators_accounts_and_args() {
    let (old, new) = idls();
    for o in old["instructions"].as_array().unwrap() {
        let name = o["name"].as_str().unwrap();
        let n = by_name(&new["instructions"], name)
            .unwrap_or_else(|| panic!("instruction `{name}` removed or renamed"));
        for key in ["discriminator", "accounts", "args", "returns"] {
            assert_eq!(
                o.get(key),
                n.get(key),
                "instruction `{name}`: `{key}` changed"
            );
        }
    }
    let added: Vec<String> = names(&new["instructions"])
        .into_iter()
        .filter(|n| by_name(&old["instructions"], n).is_none())
        .collect();
    assert_eq!(
        added,
        ["init_tipster_profile"],
        "unexpected new instructions"
    );
}

#[test]
fn accounts_events_types_and_constants_only_grow() {
    superset("accounts", false);
    superset("events", false);
    superset("types", true);
    superset("constants", true);
    let (_, new) = idls();
    // D-053: TipsterProfile is in the IDL now, as an account and as a type.
    assert!(by_name(&new["accounts"], "TipsterProfile").is_some());
    let ty = by_name(&new["types"], "TipsterProfile").expect("TipsterProfile type");
    let fields: Vec<(String, Value)> = ty["type"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["name"].as_str().unwrap().to_string(), f["type"].clone()))
        .collect();
    let want = [
        ("wallet", Value::from("pubkey")),
        ("open_count", Value::from("u32")),
        ("open_staked", Value::from("u64")),
        ("settled_count", Value::from("u32")),
        ("won", Value::from("u32")),
        ("settled_staked", Value::from("u64")),
        ("returned", Value::from("u64")),
        ("fees_earned", Value::from("u64")),
        ("bump", Value::from("u8")),
        ("reserved", serde_json::json!({"array": ["u8", 32]})),
    ];
    let want: Vec<(String, Value)> = want.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    assert_eq!(fields, want, "TipsterProfile layout (1.C source) changed");
}

#[test]
fn errors_are_appended_after_the_1c_codes() {
    let (old, new) = idls();
    let old = old["errors"].as_array().unwrap();
    let new = new["errors"].as_array().unwrap();
    assert!(new.len() > old.len());
    assert_eq!(&new[..old.len()], old.as_slice(), "1.C error codes moved");
    let names: Vec<&str> = new[old.len()..]
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "BadTipsterProfile",
            "BadCopyAccounts",
            "CopyMismatch",
            "OriginalNotCopyable",
            "CopyTooLate"
        ]
    );
    for (i, e) in new.iter().enumerate() {
        assert_eq!(e["code"].as_u64(), Some(6000 + i as u64));
    }
}
