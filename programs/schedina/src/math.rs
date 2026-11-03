//! Payout arithmetic, L1 contract C2, bit for bit with the `pricing` crate: `u128`,
//! a floor after every leg, legs in ticket order. `None` means overflow (or a zero
//! divisor), which the instructions turn into `MathOverflow`.

use crate::constants::ONE;

const ONE128: u128 = ONE as u128;

/// `rs_u = floor(stake_u * r_ppm / 1e6)`.
pub fn rs(stake_u: u64, r_ppm: u32) -> Option<u64> {
    let v = u128::from(stake_u).checked_mul(u128::from(r_ppm))? / ONE128;
    u64::try_from(v).ok()
}

/// `acc = rs_u; for each (m1_ppm, f_u): acc = floor(acc * (1e6 - m1_ppm) / f_u)`.
pub fn payout<I: IntoIterator<Item = (u32, u64)>>(rs_u: u64, legs: I) -> Option<u64> {
    let mut acc = u128::from(rs_u);
    for (m1_ppm, f_u) in legs {
        let keep = ONE128.checked_sub(u128::from(m1_ppm))?;
        acc = acc.checked_mul(keep)?.checked_div(u128::from(f_u))?;
    }
    u64::try_from(acc).ok()
}

/// `odds_u = floor(payout_u * 1e6 / rs_u)`.
pub fn odds(payout_u: u64, rs_u: u64) -> Option<u64> {
    let v = u128::from(payout_u)
        .checked_mul(ONE128)?
        .checked_div(u128::from(rs_u))?;
    u64::try_from(v).ok()
}

/// `payout_u * 1e6 >= min_payout_per_unit_u * rs_u` (no division).
pub fn floor_ok(payout_u: u64, rs_u: u64, min_payout_per_unit_u: u64) -> bool {
    u128::from(payout_u) * ONE128 >= u128::from(min_payout_per_unit_u) * u128::from(rs_u)
}

/// `acc = payout_u; for each leg: acc = floor(acc * rtilde_ppm / 1e6)`.
pub fn settle<I: IntoIterator<Item = u32>>(payout_u: u64, rtilde_ppm: I) -> Option<u64> {
    let mut acc = u128::from(payout_u);
    for r in rtilde_ppm {
        acc = acc.checked_mul(u128::from(r))? / ONE128;
    }
    u64::try_from(acc).ok()
}

/// `rtilde = r` on a long leg, `1e6 - r` on a short one (`r <= 1e6`).
pub fn rtilde(side: u8, r_ppm: u32) -> u32 {
    if side == crate::constants::SIDE_LONG {
        r_ppm
    } else {
        (ONE as u32).saturating_sub(r_ppm)
    }
}

/// The margin in money, `rS × m(n)` with `m(n) = 1 - Π(1 - m1_i)`, as
/// `rs_u - acc` where `acc = rs_u` floored through `× (1e6 - m1_i) / 1e6` per leg.
/// Recorded for the books only.
pub fn fee<I: IntoIterator<Item = u32>>(rs_u: u64, m1_ppm: I) -> Option<u64> {
    let mut acc = u128::from(rs_u);
    for m1 in m1_ppm {
        acc = acc.checked_mul(ONE128.checked_sub(u128::from(m1))?)? / ONE128;
    }
    rs_u.checked_sub(u64::try_from(acc).ok()?)
}

/// Copy fee escrowed by `open_pending` for a copy (2.F):
/// `copy_fee_u = ceil(stake_u * copy_fee_bps / 10_000)`. Charged to the user, so it
/// rounds up (C1: the house's favour).
pub fn copy_fee(stake_u: u64, copy_fee_bps: u16) -> Option<u64> {
    let bps = u128::from(crate::constants::BPS);
    let num = u128::from(stake_u).checked_mul(u128::from(copy_fee_bps))?;
    u64::try_from(num.div_ceil(bps)).ok()
}

/// The copy fee at `finalize` (2.F), split as `(used, tipster, house)`:
/// `used = floor(copy_fee_u * r_ppm / 1e6)` (the same scaling as `rs_u`; the user gets
/// `copy_fee_u - used` back with the stake refund),
/// `tipster = floor(used * tipster_share_bps / 10_000)` (rounds down: the remainder
/// stays with the house), `house = used - tipster`.
pub fn copy_split(copy_fee_u: u64, r_ppm: u32, tipster_share_bps: u16) -> Option<(u64, u64, u64)> {
    let used = rs(copy_fee_u, r_ppm)?;
    let tipster = u128::from(used).checked_mul(u128::from(tipster_share_bps))?
        / u128::from(crate::constants::BPS);
    let tipster = u64::try_from(tipster).ok()?;
    Some((used, tipster, used.checked_sub(tipster)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_fee_rounds_up_and_split_rounds_down() {
        assert_eq!(copy_fee(10_000_000, 200), Some(200_000));
        assert_eq!(copy_fee(1_000_001, 200), Some(20_001));
        assert_eq!(copy_fee(0, 200), Some(0));
        assert_eq!(copy_fee(u64::MAX, 10_000), Some(u64::MAX));
        assert_eq!(
            copy_split(200_000, 1_000_000, 8_000),
            Some((200_000, 160_000, 40_000))
        );
        assert_eq!(
            copy_split(200_000, 600_000, 8_000),
            Some((120_000, 96_000, 24_000))
        );
        assert_eq!(
            copy_split(20_001, 333_333, 8_000),
            Some((6_666, 5_332, 1_334))
        );
    }

    #[test]
    fn one_leg_at_half() {
        assert_eq!(payout(10_000_000, [(30_000, 500_000)]), Some(19_400_000));
        assert_eq!(odds(19_400_000, 10_000_000), Some(1_940_000));
        assert_eq!(fee(10_000_000, [30_000]), Some(300_000));
    }

    #[test]
    fn overflow_is_none() {
        assert_eq!(payout(u64::MAX, [(0, 1); 5]), None);
        assert_eq!(payout(1, [(0, 0)]), None);
        assert_eq!(odds(1, 0), None);
    }

    #[test]
    fn rtilde_sides() {
        assert_eq!(rtilde(0, 300_000), 300_000);
        assert_eq!(rtilde(1, 300_000), 700_000);
        assert_eq!(rtilde(1, 1_000_000), 0);
    }
}
