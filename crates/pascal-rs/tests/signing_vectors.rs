//! Byte-for-byte reproduction of Pascal's published signing vectors and of the JS reference
//! implementation's builder vectors (`fixtures/pascal/signing/vectors.json`, whose values are
//! cross-checked against the verbatim `vectors.mjs` copy here), plus verification.

use std::path::PathBuf;

use pascal_rs::signing::{
    self, AuthParams, BuilderInfo, CancelOrder, CancelOrderRequest, MassCancel, PermitError,
    PermitHeader, PlaceOrder, PlaceOrderRequest, Pubkey, TradingSigner, base58, decode_pubkey,
    from_hex, layout, to_hex, verify_cancel, verify_mass_cancel, verify_place_order,
    verify_place_order_body,
};
use pascal_rs::{ApiErrorCode, OrderType, Side, TimeInForce};
use serde_json::Value;
use sha2::Digest;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pascal/signing")
}

fn vectors() -> Value {
    serde_json::from_slice(&std::fs::read(dir().join("vectors.json")).unwrap()).unwrap()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}

fn trading() -> TradingSigner {
    TradingSigner::from_seed(&[8; 32])
}

fn owner() -> Pubkey {
    TradingSigner::from_seed(&[7; 32]).pubkey()
}

fn builder_key() -> Pubkey {
    TradingSigner::from_seed(&[9; 32]).pubkey()
}

fn docs_auth() -> AuthParams {
    AuthParams::new(owner(), 1_731_536_000_000)
}

/// The docs "Place Order" example order (GTT, replace 43, post-only, reduce-only,
/// allow_missing_replace).
fn docs_order() -> PlaceOrder {
    PlaceOrder {
        replace_client_order_id: Some(43),
        tif: TimeInForce::Gtt,
        expires_ts_ms: Some(1_731_539_600_000),
        post_only: true,
        reduce_only: true,
        allow_missing_replace: true,
        ..PlaceOrder::limit("SIM_EVENT_1.MARKET_1", Side::Bid, 550_000, 10, 42)
    }
}

/// The docs request example, as `JSON.stringify` prints it (field order matters).
const DOCS_REQUEST_JSON: &str = r#"{"client_order_id":"42","replace_client_order_id":"43","symbol":"SIM_EVENT_1.MARKET_1","side":"BID","price":"0.550000","size":"10","tif":"GTT","post_only":true,"reduce_only":true,"allow_missing_replace":true,"expires_ts_ms":"1731539600000","auth":{"client_ts_ms":"1731536000000","recv_window_ms":"5000","owner":"GmaDrppBC7P5ARKV8g3djiwP89vz1jLK23V2GBjuAEGB","signer":"2KW2XRd9kwqet15Aha2oK3tYvd3nWbTFH1MBiRAv1BE1","signature":"46gXGMwTxEckctHD3tjVyedhGG95VK2F97F3idkSYFR3WNEX4rG9g6g8KDTQPoxzLieudk8WHDXw7xNV3R2WwKCd"}}"#;

#[test]
fn json_vectors_match_the_verbatim_mjs_copy() {
    // Join the JS string concatenations ("…" +\n    "…") so split hex strings compare whole.
    let mjs = std::fs::read_to_string(dir().join("vectors.mjs")).unwrap();
    let joined: String = {
        let mut out = String::new();
        let mut rest = mjs.as_str();
        while let Some(i) = rest.find("\" +") {
            out.push_str(&rest[..i]);
            let after = &rest[i + 3..];
            let j = after.find('"').unwrap();
            rest = &after[j + 1..];
        }
        out.push_str(rest);
        out
    };
    let v = vectors();
    let from_mjs = [
        &v["keys"]["owner_pubkey"],
        &v["keys"]["trading_pubkey"],
        &v["docs_place_order"]["message_hex"],
        &v["docs_place_order"]["signature"],
        &v["quickstart_place_order"]["message_hex"],
        &v["quickstart_place_order"]["signature"],
        &v["docs_cancel"]["cancels"][0]["message_hex"],
        &v["quickstart_cancel"]["message_hex"],
        &v["docs_mass_cancel"]["message_hex"],
    ];
    for value in from_mjs {
        assert!(joined.contains(s(value)), "{value} is not in vectors.mjs");
    }
}

#[test]
fn keys_derive_from_the_test_seeds() {
    let v = vectors();
    assert_eq!(base58(&owner()), s(&v["keys"]["owner_pubkey"]));
    assert_eq!(trading().pubkey_base58(), s(&v["keys"]["trading_pubkey"]));
    assert_eq!(base58(&builder_key()), s(&v["keys"]["builder_pubkey"]));
    let seed_hex = s(&v["keys"]["trading_seed_hex"]);
    assert_eq!(
        TradingSigner::from_secret_str(seed_hex).unwrap().pubkey(),
        trading().pubkey()
    );
}

#[test]
fn offchain_digest_is_the_documented_sha256() {
    let d = sha2::Sha256::digest(b"pascal.place_order.offchain_fields.v1");
    assert_eq!(d.as_slice(), signing::OFFCHAIN_FIELDS_DIGEST);
}

#[test]
fn headers_match_every_vector() {
    let v = vectors();
    let cases = [
        (
            signing::CMD_PLACE_ORDER,
            &v["docs_place_order"]["message_hex"],
        ),
        (
            signing::CMD_PLACE_ORDER,
            &v["quickstart_place_order"]["message_hex"],
        ),
        (
            signing::CMD_CANCEL_ORDER,
            &v["docs_cancel"]["cancels"][0]["message_hex"],
        ),
        (
            signing::CMD_CANCEL_ORDER,
            &v["quickstart_cancel"]["message_hex"],
        ),
        (
            signing::CMD_MASS_CANCEL,
            &v["docs_mass_cancel"]["message_hex"],
        ),
    ];
    for (cmd, hex) in cases {
        let header = PermitHeader {
            version: 1,
            cmd_type: cmd,
            deployment_id: 3,
            recv_window_ms: 5_000,
            client_ts_ms: 1_731_536_000_000,
            owner: owner(),
            signer: trading().pubkey(),
        };
        assert_eq!(
            to_hex(&header.encode().unwrap()),
            s(hex)[..160],
            "cmd {cmd}"
        );
        assert_eq!(
            PermitHeader::decode(&from_hex(s(hex)).unwrap()).unwrap(),
            header
        );
    }
}

#[test]
fn docs_place_order_vector() {
    let v = vectors();
    let signed = trading()
        .sign_place_order(&docs_auth(), &docs_order())
        .unwrap();
    assert_eq!(signed.permit.len(), signing::PLACE_ORDER_PERMIT_LEN);
    assert_eq!(
        signed.permit_hex(),
        s(&v["docs_place_order"]["message_hex"])
    );
    assert_eq!(
        signed.signature_base58(),
        s(&v["docs_place_order"]["signature"])
    );
    assert_eq!(
        serde_json::to_string(&signed.request).unwrap(),
        DOCS_REQUEST_JSON
    );
    assert_eq!(
        serde_json::to_value(&signed.request).unwrap(),
        v["docs_place_order"]["request"]
    );
    // The docs request parses and verifies.
    let req: PlaceOrderRequest = serde_json::from_str(DOCS_REQUEST_JSON).unwrap();
    let ok = verify_place_order(&req, 3).unwrap();
    assert_eq!(ok.body, docs_order());
    assert_eq!(ok.header.owner, owner());
    assert_eq!(ok.header.signer, trading().pubkey());
    assert_eq!(to_hex(&ok.permit), s(&v["docs_place_order"]["message_hex"]));
}

#[test]
fn quickstart_place_order_vector() {
    let v = vectors();
    let order = PlaceOrder {
        allow_missing_replace: false,
        metadata: 2,
        ..docs_order()
    };
    let signed = trading().sign_place_order(&docs_auth(), &order).unwrap();
    assert_eq!(
        signed.permit_hex(),
        s(&v["quickstart_place_order"]["message_hex"])
    );
    assert_eq!(
        signed.signature_base58(),
        s(&v["quickstart_place_order"]["signature"])
    );
    assert_eq!(signed.request.metadata, Some(2));
    assert!(
        serde_json::to_string(&signed.request)
            .unwrap()
            .contains(r#""metadata":"2""#)
    );
    assert_eq!(verify_place_order(&signed.request, 3).unwrap().body, order);
}

#[test]
fn builder_changes_exactly_the_builder_bytes() {
    let v = vectors();
    let order = docs_order().with_builder(BuilderInfo {
        pubkey: builder_key(),
        fee_rate_u: 10_000,
    });
    let signed = trading().sign_place_order(&docs_auth(), &order).unwrap();
    let mut expected = from_hex(s(&v["docs_place_order"]["message_hex"])).unwrap();
    expected[layout::BUILDER_PUBKEY..layout::BUILDER_PUBKEY + 32].copy_from_slice(&builder_key());
    expected[layout::BUILDER_FEE_RATE..layout::BUILDER_FEE_RATE + 4]
        .copy_from_slice(&10_000i32.to_le_bytes());
    assert_eq!(signed.permit, expected);
    assert_eq!(&signed.permit_hex()[2 * 184..2 * 188], "10270000");
    // Schedina's 0.1% is 1_000 micro = e8 03 00 00.
    let order = docs_order().with_builder(BuilderInfo {
        pubkey: builder_key(),
        fee_rate_u: 1_000,
    });
    let signed = trading().sign_place_order(&docs_auth(), &order).unwrap();
    assert_eq!(&signed.permit_hex()[2 * 184..2 * 188], "e8030000");
    let json = serde_json::to_string(&signed.request).unwrap();
    assert!(
        json.contains(&format!(
            r#""builder_info":{{"builder_pubkey":"{}","builder_fee_rate":"0.001000"}},"auth":"#,
            base58(&builder_key())
        )),
        "{json}"
    );
}

#[test]
fn js_reference_vectors_with_and_without_builder() {
    let v = vectors();
    let cases = v["js_reference_place_orders"].as_array().unwrap();
    assert_eq!(cases.len(), 4);
    for case in cases {
        let name = s(&case["name"]);
        let req: PlaceOrderRequest = serde_json::from_value(case["request"].clone()).unwrap();
        // Rebuild from the decoded fields and sign: the bytes, signature and JSON must match.
        let order = PlaceOrder::from_request(&req).unwrap();
        let auth = AuthParams {
            owner: decode_pubkey(&req.auth.owner, "owner").unwrap(),
            deployment_id: from_hex(s(&case["message_hex"])).unwrap()[2],
            recv_window_ms: req.auth.recv_window_ms,
            client_ts_ms: req.auth.client_ts_ms,
        };
        let signed = trading().sign_place_order(&auth, &order).unwrap();
        assert_eq!(signed.permit_hex(), s(&case["message_hex"]), "{name}");
        assert_eq!(signed.signature_base58(), s(&case["signature"]), "{name}");
        assert_eq!(
            serde_json::to_value(&signed.request).unwrap(),
            case["request"],
            "{name}"
        );
        let ok = verify_place_order(&req, auth.deployment_id).unwrap();
        assert_eq!(ok.body, order, "{name}");
    }
    // One case built field by field, independently of the JSON: the desk's inception IOC.
    let ioc = PlaceOrder::ioc("NBA_26OCT28_PHIIND_ML.PHI", Side::Bid, 715_000, 14, 7).with_builder(
        BuilderInfo {
            pubkey: builder_key(),
            fee_rate_u: 1_000,
        },
    );
    let auth = AuthParams::new(owner(), 1_793_213_000_123).recv_window_ms(3_000);
    let signed = trading().sign_place_order(&auth, &ioc).unwrap();
    let case = &cases[1];
    assert_eq!(s(&case["name"]), "ioc_bid_builder_0_1pct");
    assert_eq!(signed.permit_hex(), s(&case["message_hex"]));
    assert_eq!(signed.signature_base58(), s(&case["signature"]));
    // A reduce-only unwind on the ask side, client_order_id 2^64 - 2 (the largest allowed).
    let unwind = PlaceOrder {
        reduce_only: true,
        ..PlaceOrder::ioc(
            "NBA_26OCT28_PHIIND_ML.PHI",
            Side::Ask,
            305_000,
            3,
            u64::MAX - 1,
        )
        .with_builder(BuilderInfo {
            pubkey: builder_key(),
            fee_rate_u: 1_000,
        })
    };
    let signed = trading()
        .sign_place_order(&AuthParams::new(owner(), 1_793_213_000_456), &unwind)
        .unwrap();
    assert_eq!(signed.permit_hex(), s(&cases[2]["message_hex"]));
}

#[test]
fn cancel_vectors() {
    let v = vectors();
    let cancels = v["docs_cancel"]["cancels"].as_array().unwrap();
    for c in cancels {
        let id = c["client_order_id"].as_u64().unwrap();
        let signed = trading()
            .sign_cancel(&docs_auth(), &CancelOrder::by_client_order_id(id))
            .unwrap();
        if let Some(hex) = c.get("message_hex") {
            assert_eq!(signed.permit_hex(), s(hex));
        }
        assert_eq!(signed.signature_base58(), s(&c["signature"]), "cancel {id}");
        let json = serde_json::to_string(&signed.request).unwrap();
        assert!(
            json.starts_with(&format!(r#"{{"client_order_id":"{id}","auth":{{"#)),
            "{json}"
        );
        let ok = verify_cancel(&signed.request, 3).unwrap();
        assert_eq!(ok.body, CancelOrder::by_client_order_id(id));
        let (_, decoded) = signing::decode_cancel_permit(&signed.permit).unwrap();
        assert_eq!(decoded, CancelOrder::by_client_order_id(id));
    }
    // The quickstart writes its metadata byte 2 at offset 81.
    let qs = CancelOrder {
        metadata: 2,
        ..CancelOrder::by_client_order_id(42)
    };
    let signed = trading().sign_cancel(&docs_auth(), &qs).unwrap();
    assert_eq!(
        signed.permit_hex(),
        s(&v["quickstart_cancel"]["message_hex"])
    );
    // By exchange id: cancel_by 1.
    let signed = trading()
        .sign_cancel(&docs_auth(), &CancelOrder::by_order_id(987_654_321))
        .unwrap();
    assert_eq!(signed.permit[80], 1);
    assert_eq!(signed.request.order_id, Some(987_654_321));
    assert!(verify_cancel(&signed.request, 3).is_ok());
    // Exactly one id.
    let both = CancelOrderRequest {
        client_order_id: Some(1),
        ..signed.request.clone()
    };
    assert_eq!(
        verify_cancel(&both, 3).unwrap_err().api_code(),
        ApiErrorCode::InvalidRequest
    );
}

#[test]
fn mass_cancel_vector() {
    let v = vectors();
    let signed = trading()
        .sign_mass_cancel(&docs_auth(), &MassCancel::new("SIM_EVENT_1.MARKET_1"))
        .unwrap();
    assert_eq!(signed.permit.len(), signing::MASS_CANCEL_PERMIT_LEN);
    assert_eq!(
        signed.permit_hex(),
        s(&v["docs_mass_cancel"]["message_hex"])
    );
    assert_eq!(
        signed.signature_base58(),
        s(&v["docs_mass_cancel"]["signature"])
    );
    assert_eq!(
        serde_json::to_string(&signed.request).unwrap(),
        format!(
            r#"{{"symbol":"SIM_EVENT_1.MARKET_1","auth":{{"client_ts_ms":"1731536000000","recv_window_ms":"5000","owner":"GmaDrppBC7P5ARKV8g3djiwP89vz1jLK23V2GBjuAEGB","signer":"2KW2XRd9kwqet15Aha2oK3tYvd3nWbTFH1MBiRAv1BE1","signature":"{}"}}}}"#,
            s(&v["docs_mass_cancel"]["signature"])
        )
    );
    let ok = verify_mass_cancel(&signed.request, 3).unwrap();
    assert_eq!(ok.body.symbol, "SIM_EVENT_1.MARKET_1");
    let (_, decoded) = signing::decode_mass_cancel_permit(&signed.permit).unwrap();
    assert_eq!(decoded, MassCancel::new("SIM_EVENT_1.MARKET_1"));
}

#[test]
fn verification_rejects_tampering_and_other_signers() {
    let signed = trading()
        .sign_place_order(
            &AuthParams::new(owner(), 1_793_213_000_000),
            &PlaceOrder::ioc("A.B", Side::Bid, 420_000, 5, 11).with_builder(BuilderInfo {
                pubkey: builder_key(),
                fee_rate_u: 1_000,
            }),
        )
        .unwrap();
    let ok = verify_place_order(&signed.request, 3).unwrap();
    assert_eq!(ok.body.order_type, OrderType::Limit);
    assert!(ok.clone().require_signer(&trading().pubkey()).is_ok());
    assert_eq!(
        ok.require_signer(&owner()).unwrap_err().api_code(),
        ApiErrorCode::Unauthorized
    );

    let unauthorized = |req: &PlaceOrderRequest, dep: u8| {
        assert_eq!(
            verify_place_order(req, dep).unwrap_err().api_code(),
            ApiErrorCode::Unauthorized,
            "{req:?}"
        );
    };
    unauthorized(&signed.request, 0); // another deployment
    let mut r = signed.request.clone();
    r.price = 430_000;
    unauthorized(&r, 3);
    let mut r = signed.request.clone();
    r.reduce_only = true;
    unauthorized(&r, 3);
    let mut r = signed.request.clone();
    r.builder_info.as_mut().unwrap().builder_fee_rate = 2_000;
    unauthorized(&r, 3);
    let mut r = signed.request.clone();
    r.builder_info = None;
    unauthorized(&r, 3);
    let mut r = signed.request.clone();
    r.auth.client_ts_ms += 1;
    unauthorized(&r, 3);
    let mut r = signed.request.clone();
    r.auth.signer = base58(&owner());
    unauthorized(&r, 3);
    let mut r = signed.request.clone();
    r.auth.signature = "abc".into();
    unauthorized(&r, 3);
    // Invalid content is reported with its own code before the signature is checked.
    let mut r = signed.request.clone();
    r.price = 1_000_000;
    assert_eq!(
        verify_place_order(&r, 3).unwrap_err(),
        PermitError::Price("price 1.000000 must be strictly between 0 and 1".into())
    );

    // A whole body: one good order, one tampered, one malformed.
    let mut bad = serde_json::to_value(&signed.request).unwrap();
    bad["size"] = "6".into();
    let body =
        serde_json::to_vec(&serde_json::json!([signed.request, bad, {"symbol": 1}])).unwrap();
    let results = verify_place_order_body(&body, 3).unwrap();
    assert_eq!(results.len(), 3);
    assert!(results[0].is_ok());
    assert_eq!(
        results[1].as_ref().unwrap_err().api_code(),
        ApiErrorCode::Unauthorized
    );
    assert_eq!(
        results[2].as_ref().unwrap_err().api_code(),
        ApiErrorCode::InvalidRequest
    );
    assert!(verify_place_order_body(b"{}", 3).is_err());
}
