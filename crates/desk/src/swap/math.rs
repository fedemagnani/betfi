//! The mock AMM's constant-product arithmetic (`programs/mock-amm/src/math.rs`), rewritten
//! for the desk (separate workspace and toolchain) and checked against the same hand-computed
//! vectors (`fixtures/swap/vectors.json`). Rounding goes toward the pool: fee up, output down.

/// Basis-point denominator.
pub const BPS: u64 = 10_000;

/// A sell of `amount_in` into reserves `(r_in, r_out)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapOut {
    /// What the pool pays: `floor(a × r_out / (r_in + a))`, `a = amount_in − fee_in`.
    pub amount_out: u64,
    /// `ceil(amount_in × fee_bps / 10_000)`, kept by the pool.
    pub fee_in: u64,
    /// The output with no fee; `gross_out − amount_out` is the fee in output tokens.
    pub gross_out: u64,
}

fn cp_out(a: u64, r_in: u64, r_out: u64) -> Option<u64> {
    let den = u128::from(r_in) + u128::from(a);
    if den == 0 {
        return None;
    }
    u64::try_from(u128::from(a) * u128::from(r_out) / den).ok()
}

/// The program's `swap` result, or `None` where the program refuses (empty side, zero
/// output, fee ≥ 100%).
pub fn swap_out(amount_in: u64, r_in: u64, r_out: u64, fee_bps: u16) -> Option<SwapOut> {
    if amount_in == 0 || r_in == 0 || r_out == 0 || u64::from(fee_bps) >= BPS {
        return None;
    }
    let fee_in =
        u64::try_from((u128::from(amount_in) * u128::from(fee_bps)).div_ceil(u128::from(BPS)))
            .ok()?;
    let amount_out = cp_out(amount_in - fee_in, r_in, r_out)?;
    if amount_out == 0 || amount_out >= r_out {
        return None;
    }
    Some(SwapOut {
        amount_out,
        fee_in,
        gross_out: cp_out(amount_in, r_in, r_out)?,
    })
}

/// `floor(amount × (10_000 − slippage_bps) / 10_000)`.
pub fn min_out(amount: u64, slippage_bps: u16) -> u64 {
    let keep = BPS.saturating_sub(u64::from(slippage_bps));
    u64::try_from(u128::from(amount) * u128::from(keep) / u128::from(BPS)).unwrap_or(0)
}
