//! Host-side: `schedina::math::{copy_fee, copy_split, rs}` reproduce every number in
//! `fixtures/schedina/copy_fee_vectors.json` (step 2.F), the file the desk's
//! copy-quote and the indexer reproduce. The LiteSVM side of the same arithmetic is in
//! `tests/tipster.rs` (10 USDC at r = 1 and r = 0.6, and the round-up case).

use schedina::math;
use serde_json::Value;

fn u(v: &Value) -> u64 {
    v.as_u64().unwrap()
}

#[test]
fn copy_fee_vectors() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/schedina/copy_fee_vectors.json"
    );
    let all: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let cases = all["copy_fee"].as_array().unwrap();
    assert!(!cases.is_empty());
    for c in cases {
        let name = c["name"].as_str().unwrap();
        let stake = u(&c["stake_u"]);
        let bps = u(&c["copy_fee_bps"]) as u16;
        let share = u(&c["tipster_share_bps"]) as u16;
        let r = u(&c["r_ppm"]) as u32;
        let fee = math::copy_fee(stake, bps).unwrap();
        assert_eq!(fee, u(&c["copy_fee_u"]), "{name}: copy_fee_u");
        assert_eq!(stake + fee, u(&c["escrow_u"]), "{name}: escrow_u");
        let rs = math::rs(stake, r).unwrap();
        assert_eq!(rs, u(&c["rs_u"]), "{name}: rs_u");
        let (used, tipster, house) = math::copy_split(fee, r, share).unwrap();
        assert_eq!(used, u(&c["copy_used_u"]), "{name}: copy_used_u");
        assert_eq!(tipster, u(&c["tipster_u"]), "{name}: tipster_u");
        assert_eq!(house, u(&c["house_u"]), "{name}: house_u");
        assert_eq!(
            (stake - rs) + (fee - used),
            u(&c["refund_u"]),
            "{name}: refund_u"
        );
    }
}
