//! Property: whenever every hedged leg fills at or below its `p_max` (and every unhedged leg
//! takes its quote price, spec §2 step 5), the minted ticket satisfies the floor the user signed, `P_final / (rS) ≥ min_payout_per_unit` (exact integer check of C2), for
//! every `rS ≥ FLOOR_MIN_RS_U`.

use pricing::{
    FLOOR_MIN_RS_U, PMaxLeg, PayoutLeg, TickSpec, UNIT, floor_ok, min_payout_per_unit, p_max,
    payout,
};
use proptest::prelude::*;

fn tick() -> impl Strategy<Value = TickSpec> {
    prop_oneof![
        3 => Just(TickSpec::NBA),
        1 => Just(TickSpec { tick_size_min_u: 1_000, tick_sig_figs: 2 }),
        1 => Just(TickSpec { tick_size_min_u: 10_000, tick_sig_figs: 2 }),
        1 => Just(TickSpec { tick_size_min_u: 1, tick_sig_figs: 6 }),
    ]
}

/// `(pᵢ, m₁, tick, fill fraction of p_max in ppm, hedged)`.
fn leg() -> impl Strategy<Value = (u64, u32, TickSpec, u64, bool)> {
    (
        // Quote price: anywhere a book can put an average, on or off tick.
        prop_oneof![1_000_u64..999_000, 300_000_u64..800_000],
        prop_oneof![Just(30_000_u32), Just(50_000_u32)],
        tick(),
        // Fill as a fraction of p_max (ppm); 1e6 = exactly at p_max, the worst case.
        prop_oneof![2 => Just(UNIT), 3 => 900_000_u64..=UNIT, 1 => 0_u64..=UNIT],
        // A leg the roll leaves unhedged takes its quote price pᵢ instead.
        prop_oneof![4 => Just(true), 1 => Just(false)],
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4_000))]

    #[test]
    fn floor_holds_below_p_max(
        legs in prop::collection::vec(leg(), 1..=5),
        stake_u in prop_oneof![Just(5_000_000_u64), 5_000_000_u64..=500_000_000],
        slippage_bps in prop_oneof![4 => Just(300_u32), 1 => 0_u32..=1_000],
        r_ppm in prop_oneof![Just(1_000_000_u32), 1_u32..=1_000_000],
    ) {
        let quote_legs: Vec<PayoutLeg> =
            legs.iter().map(|&(p, m1, _, _, _)| PayoutLeg { m1_ppm: m1, f_u: p }).collect();
        // Quotes whose payout overflows are rejected long before (500 USDC cap).
        let Ok(offered) = payout(stake_u, 1_000_000, &quote_legs) else {
            return Err(TestCaseError::reject("overflow"));
        };
        let floor = min_payout_per_unit(offered.odds_u, slippage_bps);
        let pm_legs: Vec<PMaxLeg> = legs
            .iter()
            .map(|&(p_u, m1_ppm, tick, _, _)| PMaxLeg { p_u, m1_ppm, tick })
            .collect();
        let pm = p_max(&pm_legs, slippage_bps, floor, stake_u, FLOOR_MIN_RS_U);
        // Only a zero or tiny slippage on very cheap legs can leave no max price on tick.
        prop_assume!(pm.is_ok());
        let pm = pm.unwrap();
        for (i, (&m, l)) in pm.iter().zip(&pm_legs).enumerate() {
            prop_assert!(l.tick.is_on_tick(m), "leg {i}: p_max {m} off tick");
        }
        let rs = u128::from(stake_u) * u128::from(r_ppm) / 1_000_000;
        prop_assume!(rs >= u128::from(FLOOR_MIN_RS_U));
        let fills: Vec<PayoutLeg> = legs
            .iter()
            .zip(&pm)
            .map(|(&(p, m1, _, frac, hedged), &m)| PayoutLeg {
                m1_ppm: m1,
                f_u: if hedged {
                    (u128::from(m) * u128::from(frac) / 1_000_000).max(1) as u64
                } else {
                    p
                },
            })
            .collect();
        let Ok(minted) = payout(stake_u, r_ppm, &fills) else {
            return Err(TestCaseError::reject("overflow at tiny fills"));
        };
        prop_assert!(
            floor_ok(minted.payout_u, minted.rs_u, floor),
            "payout {} rs {} floor {} pm {:?} fills {:?}",
            minted.payout_u, minted.rs_u, floor, pm, fills
        );
    }
}

/// With 3% slippage the max prices sit close to `p (1.03)^(1/n)`: never above it, and within a
/// tick of it for NBA prices (the guarantee costs nothing at the default slippage).
#[test]
fn p_max_is_close_to_the_formula() {
    for n in 1..=5_usize {
        for &p in &[150_000_u64, 333_000, 500_000, 695_000, 900_000] {
            let legs = vec![
                PMaxLeg {
                    p_u: p,
                    m1_ppm: 50_000,
                    tick: TickSpec::NBA
                };
                n
            ];
            let quote_legs = vec![
                PayoutLeg {
                    m1_ppm: 50_000,
                    f_u: p
                };
                n
            ];
            let off = payout(10_000_000, 1_000_000, &quote_legs).unwrap();
            let floor = min_payout_per_unit(off.odds_u, 300);
            let pm = p_max(&legs, 300, floor, 10_000_000, FLOOR_MIN_RS_U).unwrap();
            let ideal = p as f64 * 1.03_f64.powf(1.0 / n as f64);
            for &m in &pm {
                assert!(m as f64 <= ideal, "n={n} p={p}: {m} > {ideal}");
                assert!(ideal - m as f64 <= 1_000.0, "n={n} p={p}: {m} vs {ideal}");
            }
        }
    }
}
