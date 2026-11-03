//! Price ticks (spec §1 "Order conventions", D-009).
//!
//! Pascal's tick at price `p` is `tick_sig_figs` significant figures of `min(p, 1 − p)`, never
//! below `tick_size_min` (the formula of the reference `tickSizeMicros`, also in `pascal-cli
//! books`). On NBA moneylines (`0.001`, 3) the tick is 0.001 everywhere. The rule is symmetric,
//! so `p` is on tick exactly when `1 − p` is: a short leg's max price snapped down on tick is the
//! long symbol's ask price snapped up on tick.
//!
//! Step 1.A writes the same port into `pascal-rs` (`tick.rs`); this copy keeps 1.B independent
//! of it, and the two should be merged after L1.

use pascal_rs::Market;
use serde::{Deserialize, Serialize};

use crate::UNIT;

/// A market's tick parameters, from `GET /markets` (D-009: `/books` carries no spec).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TickSpec {
    /// `tick_size_min`, micro-units (1_000 = 0.001).
    pub tick_size_min_u: u64,
    /// `tick_sig_figs`, 1..=6.
    pub tick_sig_figs: u8,
}

impl TickSpec {
    /// Pascal's NBA moneyline parameters: 0.001 and 3 significant figures.
    pub const NBA: TickSpec = TickSpec {
        tick_size_min_u: 1_000,
        tick_sig_figs: 3,
    };

    /// The market's tick parameters (a non-positive `tick_size_min` becomes 1 micro-unit).
    pub fn from_market(m: &Market) -> TickSpec {
        TickSpec {
            tick_size_min_u: u64::try_from(m.tick_size_min).unwrap_or(1).max(1),
            tick_sig_figs: m.tick_sig_figs,
        }
    }

    /// Tick size at `price_u`: `sig_figs` significant figures of `min(p, 1 − p)`, at least
    /// `tick_size_min`.
    pub fn tick_at(&self, price_u: u64) -> u64 {
        let min = self.tick_size_min_u.max(1);
        if price_u == 0 || price_u >= UNIT {
            return min;
        }
        let mirrored = price_u.min(UNIT - price_u);
        let digits = mirrored.ilog10() + 1;
        let sig_tick = digits
            .checked_sub(u32::from(self.tick_sig_figs))
            .map_or(1, |exp| 10_u64.pow(exp));
        sig_tick.max(min)
    }

    /// Whether `price_u` is a valid limit price: in `(0, 1)` and a multiple of its tick.
    pub fn is_on_tick(&self, price_u: u64) -> bool {
        price_u > 0 && price_u < UNIT && price_u.is_multiple_of(self.tick_at(price_u))
    }

    /// The largest valid price `≤ price_u`, or `None` when there is none (`price_u` below the
    /// first tick).
    pub fn snap_down(&self, price_u: u64) -> Option<u64> {
        let mut p = price_u.min(UNIT - 1);
        // The tick grows by powers of ten as `min(p, 1 − p)` crosses a power of ten; one or two
        // steps settle on a price whose own tick divides it.
        for _ in 0..8 {
            if p == 0 {
                return None;
            }
            if self.is_on_tick(p) {
                return Some(p);
            }
            let t = self.tick_at(p);
            let down = p / t * t;
            p = if down == p { p - 1 } else { down };
        }
        // Fall back to a linear search (unreachable with power-of-ten ticks).
        (1..=p).rev().find(|&q| self.is_on_tick(q))
    }

    /// The smallest valid price `≥ price_u`, or `None` when there is none below 1.
    pub fn snap_up(&self, price_u: u64) -> Option<u64> {
        // Mirror: p on tick ⇔ 1 − p on tick.
        if price_u >= UNIT {
            return None;
        }
        let mirrored = UNIT - price_u.max(1);
        self.snap_down(mirrored).map(|m| UNIT - m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_follows_sig_figs_and_floor() {
        let nba = TickSpec::NBA;
        assert_eq!(nba.tick_at(654_000), 1_000);
        assert_eq!(nba.tick_at(995_000), 1_000);
        let two = TickSpec {
            tick_size_min_u: 1_000,
            tick_sig_figs: 2,
        };
        assert_eq!(two.tick_at(260_000), 10_000);
        assert_eq!(two.tick_at(905_000), 1_000);
    }

    #[test]
    fn snapping() {
        let nba = TickSpec::NBA;
        assert_eq!(nba.snap_down(715_850), Some(715_000));
        assert_eq!(nba.snap_down(715_000), Some(715_000));
        assert_eq!(nba.snap_down(999), None);
        assert_eq!(nba.snap_down(1_000_000), Some(999_000));
        assert_eq!(nba.snap_up(715_001), Some(716_000));
        assert_eq!(nba.snap_up(999_500), None);
        let two = TickSpec {
            tick_size_min_u: 1_000,
            tick_sig_figs: 2,
        };
        // 0.1003: tick 0.01 → 0.10; 0.9005: mirrored 0.0995, tick 0.001 → 0.900.
        assert_eq!(two.snap_down(100_300), Some(100_000));
        assert_eq!(two.snap_down(900_500), Some(900_000));
        assert_eq!(two.snap_down(899_500), Some(890_000));
        for p in (1..1_000_000).step_by(997) {
            let d = two.snap_down(p);
            if let Some(d) = d {
                assert!(d <= p && two.is_on_tick(d), "{p} -> {d}");
            }
            if let Some(u) = two.snap_up(p) {
                assert!(u >= p && two.is_on_tick(u), "{p} -> {u}");
            }
        }
    }
}
