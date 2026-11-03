//! Constant-product arithmetic (x·y = k), shared by `swap`, `add_liquidity`, the host
//! tests and the hand-computed vectors in `fixtures/swap/`. Every rounding goes toward
//! the pool: the fee is rounded up, the output down, the second side of a deposit up.

/// Basis-point denominator.
pub const BPS: u64 = 10_000;

/// `ceil(a × b / d)` in 128 bits; `None` on `d == 0` or a result above `u64`.
pub fn mul_div_ceil(a: u64, b: u64, d: u64) -> Option<u64> {
    if d == 0 {
        return None;
    }
    let n = u128::from(a) * u128::from(b);
    let d = u128::from(d);
    u64::try_from(n.div_ceil(d)).ok()
}

/// `floor(a × b / d)` in 128 bits; `None` on `d == 0` or a result above `u64`.
pub fn mul_div_floor(a: u64, b: u64, d: u64) -> Option<u64> {
    if d == 0 {
        return None;
    }
    u64::try_from(u128::from(a) * u128::from(b) / u128::from(d)).ok()
}

/// The fee kept by the pool on `amount_in`: `ceil(amount_in × fee_bps / 10_000)`.
pub fn fee_in(amount_in: u64, fee_bps: u16) -> Option<u64> {
    if u64::from(fee_bps) >= BPS {
        return None;
    }
    mul_div_ceil(amount_in, u64::from(fee_bps), BPS)
}

/// A swap's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapOut {
    /// Tokens sent to the user, `floor(a × r_out / (r_in + a))` with `a = amount_in − fee`.
    pub amount_out: u64,
    /// Input tokens kept by the pool as its fee (part of `amount_in`).
    pub fee_in: u64,
    /// What the user would get with no fee (`floor(amount_in × r_out / (r_in + amount_in))`);
    /// `gross_out − amount_out` is the fee counted in output tokens.
    pub gross_out: u64,
}

fn cp_out(a: u64, r_in: u64, r_out: u64) -> Option<u64> {
    let den = u128::from(r_in).checked_add(u128::from(a))?;
    if den == 0 {
        return None;
    }
    u64::try_from(u128::from(a) * u128::from(r_out) / den).ok()
}

/// Output of selling `amount_in` into reserves `(r_in, r_out)` at `fee_bps`. `None` on an
/// empty pool, an overflow, or an output of zero (dust is refused, never given away).
/// Guarantees `amount_out < r_out` and `(r_in + amount_in)(r_out − amount_out) ≥ r_in r_out`.
pub fn swap_out(amount_in: u64, r_in: u64, r_out: u64, fee_bps: u16) -> Option<SwapOut> {
    if amount_in == 0 || r_in == 0 || r_out == 0 {
        return None;
    }
    let fee = fee_in(amount_in, fee_bps)?;
    let a = amount_in.checked_sub(fee)?;
    let amount_out = cp_out(a, r_in, r_out)?;
    if amount_out == 0 || amount_out >= r_out {
        return None;
    }
    let gross_out = cp_out(amount_in, r_in, r_out)?;
    // k never decreases (holds by construction; checked so a future edit cannot break it).
    let k_before = u128::from(r_in) * u128::from(r_out);
    // A product past u128 is above any u64 × u64 `k_before`.
    let k_after =
        (u128::from(r_in) + u128::from(amount_in)).saturating_mul(u128::from(r_out - amount_out));
    if k_after < k_before {
        return None;
    }
    Some(SwapOut {
        amount_out,
        fee_in: fee,
        gross_out,
    })
}

/// USDC to deposit with `mock_amount` so the pool price does not move:
/// `ceil(mock_amount × r_usdc / r_mock)`. `None` on an empty side or overflow.
pub fn usdc_for_deposit(mock_amount: u64, r_mock: u64, r_usdc: u64) -> Option<u64> {
    if r_mock == 0 || r_usdc == 0 {
        return None;
    }
    mul_div_ceil(mock_amount, r_usdc, r_mock)
}

/// `amount × (10_000 − bps) / 10_000`, rounded down (the slippage floor a client signs).
pub fn min_out(amount: u64, slippage_bps: u16) -> Option<u64> {
    let keep = BPS.checked_sub(u64::from(slippage_bps))?;
    mul_div_floor(amount, keep, BPS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_toward_the_pool() {
        // 1 unit in: the fee rounds up to the whole unit, nothing goes out.
        assert_eq!(fee_in(1, 50), Some(1));
        assert_eq!(swap_out(1, 1_000, 1_000, 50), None);
        // 10_000 × 0.5% = 50 exactly; 10_001 rounds up to 51.
        assert_eq!(fee_in(10_000, 50), Some(50));
        assert_eq!(fee_in(10_001, 50), Some(51));
        let s = swap_out(1_000, 1_000_000, 1_000_000, 50).unwrap();
        // a = 995; 995 × 1e6 / 1_000_995 = 994.0099… → 994.
        assert_eq!(s.amount_out, 994);
        assert_eq!(s.fee_in, 5);
        assert_eq!(s.gross_out, 999);
        assert_eq!(usdc_for_deposit(3, 3, 2), Some(2));
        assert_eq!(usdc_for_deposit(1, 3, 2), Some(1));
        assert_eq!(min_out(1_000, 50), Some(995));
        assert_eq!(min_out(999, 50), Some(994));
    }

    #[test]
    fn never_drains_and_never_overflows() {
        let max = u64::MAX;
        // Selling the largest possible amount into a tiny pool still leaves a reserve.
        let s = swap_out(max, 1, max, 50).unwrap();
        assert!(s.amount_out < max);
        assert!(swap_out(max, max, max, 50).is_some());
        assert_eq!(swap_out(5, 0, 10, 50), None);
        assert_eq!(swap_out(5, 10, 0, 50), None);
        assert_eq!(fee_in(5, 10_000), None);
        for (a, x, y) in [(7, 3, 5), (1_000_000, 13, 17), (max / 3, max / 5, 11)] {
            if let Some(s) = swap_out(a, x, y, 50) {
                let k0 = u128::from(x) * u128::from(y);
                let k1 =
                    (u128::from(x) + u128::from(a)).saturating_mul(u128::from(y - s.amount_out));
                assert!(k1 >= k0);
            }
        }
    }
}
