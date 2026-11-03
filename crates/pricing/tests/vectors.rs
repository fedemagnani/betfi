//! Every vector of `fixtures/schedina/payout_vectors.json` (contract C2), reproduced bit for bit.

use std::path::PathBuf;

use pricing::{PayoutLeg, floor_ok, payout, settle};
use serde_json::Value;

fn vectors() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/schedina/payout_vectors.json");
    let text = std::fs::read_to_string(&path).expect("read payout_vectors.json");
    serde_json::from_str(&text).expect("parse payout_vectors.json")
}

fn u(v: &Value, key: &str) -> u64 {
    v[key]
        .as_u64()
        .unwrap_or_else(|| panic!("{key} missing in {v}"))
}

#[test]
fn payout_vectors() {
    let all = vectors();
    let cases = all["payout"].as_array().expect("payout array");
    assert!(!cases.is_empty());
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let legs: Vec<PayoutLeg> = case["legs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| PayoutLeg {
                m1_ppm: u(l, "m1_ppm") as u32,
                f_u: u(l, "f_u"),
            })
            .collect();
        let p = payout(u(case, "stake_u"), u(case, "r_ppm") as u32, &legs)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(p.rs_u, u(case, "rs_u"), "{name}: rs_u");
        assert_eq!(p.refund_u, u(case, "refund_u"), "{name}: refund_u");
        assert_eq!(p.payout_u, u(case, "payout_u"), "{name}: payout_u");
        assert_eq!(p.odds_u, u(case, "odds_u"), "{name}: odds_u");
    }
}

#[test]
fn settle_vectors() {
    let all = vectors();
    let cases = all["settle"].as_array().expect("settle array");
    assert!(!cases.is_empty());
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let rt: Vec<u64> = case["rtilde_ppm"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_u64().unwrap())
            .collect();
        assert_eq!(
            settle(u(case, "payout_u"), &rt).unwrap(),
            u(case, "paid_u"),
            "{name}"
        );
    }
}

#[test]
fn floor_vectors() {
    let all = vectors();
    let cases = all["floor"].as_array().expect("floor array");
    assert!(!cases.is_empty());
    for case in cases {
        let name = case["name"].as_str().unwrap();
        assert_eq!(
            floor_ok(
                u(case, "payout_u"),
                u(case, "rs_u"),
                u(case, "min_payout_per_unit_u")
            ),
            case["ok"].as_bool().unwrap(),
            "{name}"
        );
    }
}
