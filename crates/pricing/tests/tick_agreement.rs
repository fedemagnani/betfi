//! `pricing::TickSpec` (used for quotes) and `pascal_rs::tick::TickRule` (used to validate
//! order prices) are two implementations of Pascal's tick formula. They must agree on every
//! price, or the desk would quote a `p_max` that Pascal rejects with `INVALID_PRICE`.

use pascal_rs::tick::TickRule;
use pricing::TickSpec;

#[test]
fn tick_spec_and_tick_rule_agree_on_every_price() {
    for &min in &[1_u64, 10, 100, 1_000, 10_000] {
        for figs in 1..=6_u8 {
            let spec = TickSpec {
                tick_size_min_u: min,
                tick_sig_figs: figs,
            };
            let rule = TickRule::new(min as i64, figs).expect("valid rule");
            for p in 1..1_000_000_u64 {
                let a = spec.tick_at(p);
                let b = rule.tick_at(p as i64);
                assert_eq!(a as i64, b, "tick_at({p}) min={min} figs={figs}");
                assert_eq!(
                    spec.is_on_tick(p),
                    rule.is_valid(p as i64),
                    "on-tick({p}) min={min} figs={figs}"
                );
            }
        }
    }
}
