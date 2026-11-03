//! Host-side: `schedina::math` reproduces every number in
//! `fixtures/schedina/payout_vectors.json` (L1 contract C2), the same file the
//! `pricing` crate checks.

use schedina::math;
use serde_json::Value;

fn vectors() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/schedina/payout_vectors.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn u(v: &Value) -> u64 {
    v.as_u64().unwrap()
}

#[test]
fn payout_vectors() {
    let all = vectors();
    let cases = all["payout"].as_array().unwrap();
    assert!(!cases.is_empty());
    for c in cases {
        let name = c["name"].as_str().unwrap();
        let stake = u(&c["stake_u"]);
        let r = u(&c["r_ppm"]) as u32;
        let legs: Vec<(u32, u64)> = c["legs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| (u(&l["m1_ppm"]) as u32, u(&l["f_u"])))
            .collect();
        let rs = math::rs(stake, r).unwrap();
        assert_eq!(rs, u(&c["rs_u"]), "{name}: rs_u");
        assert_eq!(stake - rs, u(&c["refund_u"]), "{name}: refund_u");
        let p = math::payout(rs, legs).unwrap();
        assert_eq!(p, u(&c["payout_u"]), "{name}: payout_u");
        assert_eq!(
            math::odds(p, rs).unwrap(),
            u(&c["odds_u"]),
            "{name}: odds_u"
        );
    }
}

#[test]
fn settle_vectors() {
    let all = vectors();
    for c in all["settle"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let rt: Vec<u32> = c["rtilde_ppm"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| u(x) as u32)
            .collect();
        let paid = math::settle(u(&c["payout_u"]), rt).unwrap();
        assert_eq!(paid, u(&c["paid_u"]), "{name}");
    }
}

#[test]
fn floor_vectors() {
    let all = vectors();
    for c in all["floor"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let got = math::floor_ok(
            u(&c["payout_u"]),
            u(&c["rs_u"]),
            u(&c["min_payout_per_unit_u"]),
        );
        assert_eq!(got, c["ok"].as_bool().unwrap(), "{name}");
    }
}
