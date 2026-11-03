//! Pascal's price grid: tick size at a price, validation, and snapping.
//!
//! Port of `tickSizeMicros` / `validatePrice` in the reference `pascal-sign.js`, from the docs
//! section "Tick Sizes": with `p' = min(p, 1 - p)`,
//! `tick = max(tick_size_min, 10^(floor(log10 p') - tick_sig_figs + 1))`. In integer micro-USD,
//! when `p'` has `d` digits the significant-figure tick is `10^(d - tick_sig_figs)`.
//!
//! A valid price is strictly inside (0, 1) and a multiple of the tick at that price. Pascal
//! rejects anything else with `INVALID_PRICE`, so every price the desk signs goes through
//! [`TickRule::snap`] first: down for a bid limit (never pay more than the max), up for an ask.

use crate::num::{MICROS_PER_UNIT, MicroUsd, format_micros};
use crate::types::{Market, MarketSpec, Side};

/// Why a tick rule or a price was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TickError {
    /// `tick_size_min` not positive or `tick_sig_figs` outside 1..=6.
    #[error("invalid tick rule: {0}")]
    Rule(String),
    /// Price outside (0, 1).
    #[error("price {} must be strictly between 0 and 1", format_micros(*.0))]
    OutOfRange(MicroUsd),
    /// Price not a multiple of the tick at that price.
    #[error("price {} is not a multiple of the tick size {}", format_micros(*.price), format_micros(*.tick))]
    OffGrid {
        /// The price.
        price: MicroUsd,
        /// The tick at that price.
        tick: MicroUsd,
    },
    /// No grid price exists on the requested side of the price inside (0, 1).
    #[error("no valid price at or {} {}", if *.up { "above" } else { "below" }, format_micros(*.price))]
    NoValidPrice {
        /// The price being snapped.
        price: MicroUsd,
        /// Direction of the snap.
        up: bool,
    },
}

/// Which way [`TickRule::snap`] rounds an off-grid price.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rounding {
    /// To the highest valid price at or below.
    Down,
    /// To the lowest valid price at or above.
    Up,
}

impl Rounding {
    /// The conservative direction for a limit price: down for a bid, up for an ask.
    pub fn passive(side: Side) -> Self {
        match side {
            Side::Bid => Rounding::Down,
            Side::Ask => Rounding::Up,
        }
    }
}

/// A market's tick parameters (`tick_size_min`, `tick_sig_figs` of `MarketSpec`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickRule {
    min_tick: MicroUsd,
    sig_figs: u8,
}

impl TickRule {
    /// Validates the parameters: `tick_size_min > 0`, `tick_sig_figs` in 1..=6.
    pub fn new(tick_size_min: MicroUsd, tick_sig_figs: u8) -> Result<Self, TickError> {
        if tick_size_min <= 0 || tick_size_min >= MICROS_PER_UNIT {
            return Err(TickError::Rule(format!(
                "tick_size_min {} must be in (0, 1)",
                format_micros(tick_size_min)
            )));
        }
        if !(1..=6).contains(&tick_sig_figs) {
            return Err(TickError::Rule(format!(
                "tick_sig_figs {tick_sig_figs} must be in [1, 6]"
            )));
        }
        Ok(TickRule {
            min_tick: tick_size_min,
            sig_figs: tick_sig_figs,
        })
    }

    /// The rule of a market from `/markets` or `/markets/search`.
    pub fn from_market(m: &Market) -> Result<Self, TickError> {
        Self::new(m.tick_size_min, m.tick_sig_figs)
    }

    /// The rule of a book's `spec`.
    pub fn from_spec(s: &MarketSpec) -> Result<Self, TickError> {
        Self::new(s.tick_size_min, s.tick_sig_figs)
    }

    /// `tick_size_min`, micro-USD.
    pub fn tick_size_min(&self) -> MicroUsd {
        self.min_tick
    }

    /// `tick_sig_figs`.
    pub fn tick_sig_figs(&self) -> u8 {
        self.sig_figs
    }

    /// Tick size at `price`, micro-USD (`tickSizeMicros`). At or outside the endpoints the
    /// minimum tick is returned.
    pub fn tick_at(&self, price: MicroUsd) -> MicroUsd {
        let mirrored = price.min(MICROS_PER_UNIT - price);
        if mirrored <= 0 {
            return self.min_tick;
        }
        let digits = mirrored.ilog10() + 1;
        let sig_tick = digits
            .checked_sub(u32::from(self.sig_figs))
            .map_or(0, |exp| 10_i64.pow(exp));
        sig_tick.max(self.min_tick)
    }

    /// Whether `price` is inside (0, 1) and on the grid.
    pub fn is_valid(&self, price: MicroUsd) -> bool {
        self.validate(price).is_ok()
    }

    /// `validatePrice`: `Ok(tick)` when `price` is inside (0, 1) and a multiple of its tick.
    pub fn validate(&self, price: MicroUsd) -> Result<MicroUsd, TickError> {
        if price <= 0 || price >= MICROS_PER_UNIT {
            return Err(TickError::OutOfRange(price));
        }
        let tick = self.tick_at(price);
        if price % tick != 0 {
            return Err(TickError::OffGrid { price, tick });
        }
        Ok(tick)
    }

    /// The nearest valid price at or below (`Down`) or at or above (`Up`) `price`. A price at
    /// or beyond an endpoint is first brought just inside (0, 1) on the side being snapped to.
    pub fn snap(&self, price: MicroUsd, rounding: Rounding) -> Result<MicroUsd, TickError> {
        let up = rounding == Rounding::Up;
        let none = || TickError::NoValidPrice { price, up };
        let mut p = match rounding {
            Rounding::Down if price <= 0 => return Err(none()),
            Rounding::Down => price.min(MICROS_PER_UNIT - 1),
            Rounding::Up if price >= MICROS_PER_UNIT => return Err(none()),
            Rounding::Up => price.max(1),
        };
        // Each pass moves strictly in one direction (an off-grid p is not a multiple of its
        // own tick), so the loop ends within the (0, 1) range.
        loop {
            if self.is_valid(p) {
                return Ok(p);
            }
            let tick = self.tick_at(p);
            let rem = p.rem_euclid(tick);
            p = match rounding {
                Rounding::Down => p - rem,
                Rounding::Up => p + (tick - rem),
            };
            if p <= 0 || p >= MICROS_PER_UNIT {
                return Err(none());
            }
        }
    }

    /// The next valid price strictly above (`Up`) or below (`Down`) `price`.
    pub fn step(&self, price: MicroUsd, direction: Rounding) -> Result<MicroUsd, TickError> {
        match direction {
            Rounding::Up => self.snap(price.saturating_add(1), Rounding::Up),
            Rounding::Down => self.snap(price.saturating_sub(1), Rounding::Down),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(min: MicroUsd, sf: u8) -> TickRule {
        TickRule::new(min, sf).unwrap()
    }

    #[test]
    fn tick_follows_the_docs_table() {
        // tick_size_min 0.001, tick_sig_figs 2 (the docs example).
        let r = rule(1_000, 2);
        for p in [100_000, 500_000, 850_000, 900_000] {
            assert_eq!(r.tick_at(p), 10_000, "{p}");
        }
        for p in [99_000, 25_000, 10_000, 901_000, 975_000, 990_000] {
            assert_eq!(r.tick_at(p), 1_000, "{p}");
        }
        for p in [5_000, 1_000, 997_000, 999_000] {
            assert_eq!(r.tick_at(p), 1_000, "{p}");
        }
        for p in [
            500_000, 510_000, 850_000, 25_000, 43_000, 975_000, 1_000, 997_000,
        ] {
            assert!(r.validate(p).is_ok(), "{p}");
        }
        for p in [505_000, 25_500, 900_500, 100_100] {
            assert!(
                matches!(r.validate(p), Err(TickError::OffGrid { .. })),
                "{p}"
            );
        }
        for p in [0, 1_000_000, -1, 1_000_001] {
            assert_eq!(r.validate(p), Err(TickError::OutOfRange(p)));
        }
        // Three significant figures with a finer floor.
        let fine = rule(100, 3);
        assert_eq!(fine.tick_at(555_000), 1_000);
        assert_eq!(fine.tick_at(5_550), 100);
        // The CLI's former private formula, same cases.
        assert_eq!(rule(1_000, 2).tick_at(260_000), 10_000);
        assert_eq!(rule(1_000, 3).tick_at(654_000), 1_000);
        assert_eq!(rule(1_000, 3).tick_at(995_000), 1_000);
        assert_eq!(rule(1_000, 3).tick_at(1_000_000), 1_000);
    }

    #[test]
    fn rules_are_validated() {
        assert!(TickRule::new(0, 2).is_err());
        assert!(TickRule::new(1_000, 0).is_err());
        assert!(TickRule::new(1_000, 7).is_err());
        assert!(TickRule::new(1_000_000, 2).is_err());
    }

    #[test]
    fn snap_examples() {
        let r = rule(1_000, 2);
        assert_eq!(r.snap(505_000, Rounding::Down), Ok(500_000));
        assert_eq!(r.snap(505_000, Rounding::Up), Ok(510_000));
        assert_eq!(r.snap(510_000, Rounding::Up), Ok(510_000));
        // Across a tick boundary: 0.0995 has tick 0.001, 0.1005 has tick 0.01.
        assert_eq!(r.snap(99_500, Rounding::Up), Ok(100_000));
        assert_eq!(r.snap(100_500, Rounding::Down), Ok(100_000));
        assert_eq!(r.snap(100_500, Rounding::Up), Ok(110_000));
        assert_eq!(r.snap(900_500, Rounding::Down), Ok(900_000));
        assert_eq!(r.snap(900_500, Rounding::Up), Ok(901_000));
        // Endpoints.
        assert_eq!(r.snap(1_200_000, Rounding::Down), Ok(999_000));
        assert_eq!(r.snap(-5, Rounding::Up), Ok(1_000));
        assert!(r.snap(500, Rounding::Down).is_err());
        assert!(r.snap(999_500, Rounding::Up).is_err());
        assert_eq!(r.step(500_000, Rounding::Up), Ok(510_000));
        assert_eq!(r.step(100_000, Rounding::Down), Ok(99_000));
        assert_eq!(Rounding::passive(Side::Bid), Rounding::Down);
    }

    /// Exhaustive over the whole micro grid: `snap` returns the nearest valid price on the
    /// requested side, for the rules seen on Pascal and a few odd ones.
    #[test]
    fn snap_is_the_nearest_valid_price_everywhere() {
        for r in [
            rule(1_000, 2),
            rule(1_000, 3),
            rule(100, 3),
            rule(10_000, 1),
            rule(2_500, 2),
        ] {
            let valid: Vec<MicroUsd> = (1..MICROS_PER_UNIT).filter(|&p| r.is_valid(p)).collect();
            let mut below = None; // highest valid <= p
            let mut idx = 0;
            for p in 1..MICROS_PER_UNIT {
                while idx < valid.len() && valid[idx] <= p {
                    below = Some(valid[idx]);
                    idx += 1;
                }
                // Lowest valid >= p.
                let above = if below == Some(p) {
                    Some(p)
                } else {
                    valid.get(idx).copied()
                };
                assert_eq!(r.snap(p, Rounding::Down).ok(), below, "{r:?} down {p}");
                assert_eq!(r.snap(p, Rounding::Up).ok(), above, "{r:?} up {p}");
            }
        }
    }
}
