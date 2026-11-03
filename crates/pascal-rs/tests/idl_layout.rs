//! The place-order permit against Pascal's on-chain IDL (`pascal-idl-permit.json`): the field
//! offsets follow from the Codama type nodes, and a signed permit decoded field by field with
//! that layout (independently of `signing::decode_place_order_permit`) gives back the order.

use std::collections::BTreeMap;
use std::path::PathBuf;

use pascal_rs::signing::{
    self, AuthParams, BuilderInfo, PlaceOrder, TradingSigner, base58, layout, verify_signature,
};
use pascal_rs::{Side, TimeInForce};
use serde_json::{Value, json};

fn idl() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/pascal/signing/pascal-idl-permit.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn size_of(node: &Value) -> usize {
    match node["kind"].as_str().unwrap() {
        "numberTypeNode" => match node["format"].as_str().unwrap() {
            "u8" | "i8" => 1,
            "u16" | "i16" => 2,
            "u32" | "i32" => 4,
            "u64" | "i64" => 8,
            f => panic!("unhandled number format {f}"),
        },
        "amountTypeNode" => size_of(&node["number"]),
        "publicKeyTypeNode" => 32,
        "fixedSizeTypeNode" => node["size"].as_u64().unwrap() as usize,
        k => panic!("unhandled IDL node {k}"),
    }
}

/// Reads one field the way a Codama client would.
fn read(node: &Value, bytes: &[u8]) -> Value {
    match node["kind"].as_str().unwrap() {
        "numberTypeNode" => {
            assert_eq!(node["endian"], "le");
            let mut b = [0u8; 8];
            b[..bytes.len()].copy_from_slice(bytes);
            match node["format"].as_str().unwrap() {
                "i32" => json!(i32::from_le_bytes(bytes.try_into().unwrap())),
                _ => json!(u64::from_le_bytes(b)),
            }
        }
        "amountTypeNode" => read(&node["number"], bytes),
        "publicKeyTypeNode" => json!(base58(bytes)),
        "fixedSizeTypeNode" => match node["type"]["kind"].as_str().unwrap() {
            "stringTypeNode" => {
                let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                json!(std::str::from_utf8(&bytes[..end]).unwrap())
            }
            _ => json!(signing::to_hex(bytes)),
        },
        k => panic!("unhandled IDL node {k}"),
    }
}

fn snake(name: &str) -> String {
    name.chars().fold(String::new(), |mut s, c| {
        if c.is_ascii_uppercase() {
            s.push('_');
            s.push(c.to_ascii_lowercase());
        } else {
            s.push(c);
        }
        s
    })
}

/// (name, offset, node) of header + body fields, in IDL order.
fn fields() -> Vec<(String, usize, Value)> {
    let idl = idl();
    let types: BTreeMap<String, Value> = idl["definedTypes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap().to_owned(), t["type"].clone()))
        .collect();
    let mut at = 0;
    let mut out = Vec::new();
    for t in ["permitHeader", "placeOrderPermitBody"] {
        for f in types[t]["fields"].as_array().unwrap() {
            let name = snake(f["name"].as_str().unwrap());
            out.push((name, at, f["type"].clone()));
            at += size_of(&f["type"]);
        }
    }
    assert_eq!(at, signing::PLACE_ORDER_PERMIT_LEN);
    out
}

#[test]
fn layout_matches_the_idl() {
    let offsets: BTreeMap<String, usize> = fields().into_iter().map(|(n, at, _)| (n, at)).collect();
    let ours = BTreeMap::from([
        ("version".to_owned(), layout::VERSION),
        ("cmd_type".to_owned(), layout::CMD_TYPE),
        ("deployment_id".to_owned(), layout::DEPLOYMENT_ID),
        ("padding0".to_owned(), layout::PADDING0),
        ("recv_window_ms".to_owned(), layout::RECV_WINDOW_MS),
        ("client_ts_ms".to_owned(), layout::CLIENT_TS_MS),
        ("owner".to_owned(), layout::OWNER),
        ("signer".to_owned(), layout::SIGNER),
        ("size".to_owned(), layout::SIZE),
        ("price".to_owned(), layout::PRICE),
        ("client_order_id".to_owned(), layout::CLIENT_ORDER_ID),
        (
            "replace_client_order_id".to_owned(),
            layout::REPLACE_CLIENT_ORDER_ID,
        ),
        ("expire_ts_ms".to_owned(), layout::EXPIRE_TS_MS),
        ("builder_pubkey".to_owned(), layout::BUILDER_PUBKEY),
        ("symbol".to_owned(), layout::SYMBOL),
        ("builder_fee_rate".to_owned(), layout::BUILDER_FEE_RATE),
        (
            "offchain_fields_digest".to_owned(),
            layout::OFFCHAIN_FIELDS_DIGEST,
        ),
        ("metadata".to_owned(), layout::METADATA),
        ("side".to_owned(), layout::SIDE),
        ("order_type".to_owned(), layout::ORDER_TYPE),
        ("tif".to_owned(), layout::TIF),
        ("flags".to_owned(), layout::FLAGS),
        ("padding1".to_owned(), layout::PADDING1),
    ]);
    assert_eq!(offsets, ours);
    let body: BTreeMap<String, Value> = fields().into_iter().map(|(n, _, t)| (n, t)).collect();
    assert_eq!(body["builder_fee_rate"]["number"]["format"], "i32");
    assert_eq!(body["builder_fee_rate"]["decimals"], 6);
    assert_eq!(body["price"]["decimals"], 6);
}

#[test]
fn signed_permit_decodes_with_the_idl_layout() {
    let owner = TradingSigner::from_seed(&[7; 32]).pubkey();
    let trading = TradingSigner::from_seed(&[8; 32]);
    let builder = TradingSigner::from_seed(&[9; 32]).pubkey();
    let order = PlaceOrder {
        reduce_only: true,
        replace_client_order_id: Some(77),
        ..PlaceOrder::ioc(
            "NBA_26OCT28_PHIIND_ML.PHI",
            Side::Ask,
            305_000,
            12,
            1_234_567,
        )
        .with_builder(BuilderInfo {
            pubkey: builder,
            fee_rate_u: 1_000,
        })
    };
    let auth = AuthParams::new(owner, 1_793_213_000_123).recv_window_ms(2_500);
    let signed = trading.sign_place_order(&auth, &order).unwrap();
    verify_signature(&signed.permit, &signed.signature, &trading.pubkey()).unwrap();

    let decoded: BTreeMap<String, Value> = fields()
        .into_iter()
        .map(|(name, at, node)| {
            let v = read(&node, &signed.permit[at..at + size_of(&node)]);
            (name, v)
        })
        .collect();
    let expected = json!({
        "version": 1, "cmd_type": 1, "deployment_id": 3, "padding0": "00",
        "recv_window_ms": 2500, "client_ts_ms": 1_793_213_000_123u64,
        "owner": base58(&owner), "signer": trading.pubkey_base58(),
        "size": 12, "price": 305_000, "client_order_id": 1_234_567,
        "replace_client_order_id": 77, "expire_ts_ms": 0,
        "builder_pubkey": base58(&builder), "symbol": "NBA_26OCT28_PHIIND_ML.PHI",
        "builder_fee_rate": 1_000,
        "offchain_fields_digest": signing::to_hex(&signing::OFFCHAIN_FIELDS_DIGEST),
        "metadata": 0, "side": 1, "order_type": 0, "tif": 2, "flags": 2,
        "padding1": "00000000000000",
    });
    let expected: BTreeMap<String, Value> = serde_json::from_value(expected).unwrap();
    assert_eq!(decoded, expected);

    // And our own decoder agrees.
    let (header, back) = signing::decode_place_order_permit(&signed.permit).unwrap();
    assert_eq!(back, order);
    assert_eq!(header.recv_window_ms, 2_500);
    assert_eq!(back.tif, TimeInForce::Ioc);
}
