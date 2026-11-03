//! Exchange arithmetic in micro-units: tick grid, fees, collateral.
//!
//! Prices are micro-USD (`1.0 = 1_000_000`), sizes whole contracts, rates micro-units
//! (`"0.020000"` = `20_000`). Intermediates are `i128`; every fee rounds toward zero, as Pascal
//! documents for builder fees and as its order example shows for the taker fee
//! (3 × 0.55 × 0.45 × 0.001 = 0.0007425 → `"0.000742"`).

use pascal_rs::num::decimal_to_micros;
use rust_decimal::Decimal;

/// Micro-units per unit.
pub const MICRO: i64 = 1_000_000;

/// Tick size at `price_u` (docs "Tick Sizes"): with `p' = min(p, 1 − p)` written in micros with
/// `d` digits, the significant-figure tick is `10^(d − sig_figs)` micros, floored at
/// `tick_size_min`.
pub fn tick_size_u(price_u: i64, tick_size_min_u: i64, tick_sig_figs: u8) -> i64 {
    let mirrored = price_u.min(MICRO - price_u);
    if mirrored <= 0 {
        return tick_size_min_u.max(1);
    }
    let digits = mirrored.ilog10() as i64 + 1;
    let exp = digits - i64::from(tick_sig_figs);
    let sig_tick = if exp >= 0 { 10i64.pow(exp as u32) } else { 0 };
    sig_tick.max(tick_size_min_u).max(1)
}

/// Whether `price_u` lies strictly inside (0, 1) and on the market's tick grid.
pub fn valid_price(price_u: i64, tick_size_min_u: i64, tick_sig_figs: u8) -> bool {
    price_u > 0
        && price_u < MICRO
        && price_u % tick_size_u(price_u, tick_size_min_u, tick_sig_figs) == 0
}

/// A rate (`Decimal`, e.g. `0.020000`) in micro-units; `None` if it is not exact in micros.
pub fn rate_u(rate: Decimal) -> Option<i64> {
    decimal_to_micros(rate).ok()
}

/// `size × p × (1 − p) × rate`, rounded toward zero, micro-USD. Pascal's formula for the taker
/// fee, the maker rebate (with `rate = taker_fee_rate × maker_rebate_share`) and the builder fee.
pub fn fee_u(size: u64, price_u: i64, rate_u: i64) -> i64 {
    let num =
        i128::from(size) * i128::from(price_u) * i128::from(MICRO - price_u) * i128::from(rate_u);
    // p, 1 − p and the rate each carry a 10^6 scale; the result keeps one. i128 division
    // truncates toward zero.
    clamp_i64(num / (i128::from(MICRO) * i128::from(MICRO)))
}

/// Maker rebate `size × p × (1 − p) × taker_fee_rate × maker_rebate_share`, rounded toward
/// zero once at the end, micro-USD (positive: the amount credited).
pub fn rebate_u(size: u64, price_u: i64, taker_rate_u: i64, rebate_share_u: i64) -> i64 {
    let m = i128::from(MICRO);
    let base = i128::from(size)
        * i128::from(price_u)
        * i128::from(MICRO - price_u)
        * i128::from(taker_rate_u);
    match base.checked_mul(i128::from(rebate_share_u)) {
        Some(n) => clamp_i64(n / (m * m * m)),
        // Only for absurd sizes: round in two steps.
        None => clamp_i64(base / (m * m) * i128::from(rebate_share_u) / m),
    }
}

/// `size × p × (1 − p)` in micro-contracts, toward zero (Pascal's weighted volume).
pub fn weighted_u(size: u64, price_u: i64) -> i64 {
    clamp_i64(
        i128::from(size) * i128::from(price_u) * i128::from(MICRO - price_u) / i128::from(MICRO),
    )
}

/// `size × price_u`: notional in micro-USD.
pub fn notional_u(size: u64, price_u: i64) -> i64 {
    clamp_i64(i128::from(size) * i128::from(price_u))
}

/// Saturating `i128 → i64`.
pub fn clamp_i64(v: i128) -> i64 {
    v.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_grid_matches_the_docs_table() {
        // tick_size_min 0.001, sig figs 2 (docs example).
        assert_eq!(tick_size_u(500_000, 1_000, 2), 10_000);
        assert_eq!(tick_size_u(850_000, 1_000, 2), 10_000);
        assert_eq!(tick_size_u(25_000, 1_000, 2), 1_000);
        assert_eq!(tick_size_u(975_000, 1_000, 2), 1_000);
        assert_eq!(tick_size_u(1_000, 1_000, 2), 1_000);
        // NBA moneylines: 0.001, 3 sig figs.
        assert_eq!(tick_size_u(770_000, 1_000, 3), 1_000);
        assert!(valid_price(770_000, 1_000, 3));
        assert!(!valid_price(770_500, 1_000, 3));
        assert!(!valid_price(0, 1_000, 3));
        assert!(!valid_price(MICRO, 1_000, 3));
        assert!(valid_price(510_000, 1_000, 2));
        assert!(!valid_price(515_000, 1_000, 2));
    }

    #[test]
    fn fees_round_toward_zero() {
        // Docs order example: 3 @ 0.55, rate 0.001 → 0.000742.
        assert_eq!(fee_u(3, 550_000, 1_000), 742);
        // Builder docs: 1% on 100 @ 0.50 → 0.25.
        assert_eq!(fee_u(100, 500_000, 10_000), 250_000);
        // NBA 2% taker at 0.77: 100 × 0.77 × 0.23 × 0.02 = 0.3542.
        assert_eq!(fee_u(100, 770_000, 20_000), 354_200);
        assert_eq!(fee_u(1, 770_000, 20_000), 3_542);
        // Rebate 5 × 0.695 × 0.305 × 0.02 × 0.25 = 0.005299375.
        assert_eq!(rebate_u(5, 695_000, 20_000, 250_000), 5_299);
        // A share that is not exact against the rate still rounds once:
        // 100 × 0.5 × 0.5 × 0.01 × 0.333333 = 0.08333325.
        assert_eq!(rebate_u(100, 500_000, 10_000, 333_333), 83_333);
        assert!(rebate_u(u64::MAX, 500_000, 1_000_000, 1_000_000) > 0);
        assert_eq!(fee_u(0, 500_000, 20_000), 0);
        assert_eq!(weighted_u(100, 400_000), 24_000_000);
        assert_eq!(notional_u(3, 550_000), 1_650_000);
    }
}
