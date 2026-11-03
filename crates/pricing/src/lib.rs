//! Schedina pricing: pure functions from Pascal books to parlay quotes (plan step 1.B).
//!
//! Every formula of the spec's sections 2 (pricing), 3 (risk and hedging desk) and 10
//! (settlement) is a function here, named after its symbol and documented with the formula.
//! There is no I/O: callers pass books, legs and the clock in.
//!
//! # Units (contract C1)
//!
//! - Money is micro-USDC and prices, probabilities and ratios are micro-units, all with the
//!   `_u` / `_ppm` suffix: `1.0 = 1_000_000` ([`UNIT`]). Odds are micro-units too
//!   (`1_940_000` is 1.94).
//! - Contracts are `u64` integers; one contract pays 1 USD (`UNIT` micro-USDC) when its side wins.
//! - Times are Unix milliseconds (`*_ms`).
//! - Rounding is always in the house's favour: payouts, odds and cash-out values round **down**;
//!   contract sizes, average ask fills and the user-side prices `pᵢ` round **up** (so an average
//!   bid rounds down, because a short leg's price is `1 − bid`).
//! - Products are taken in `u128`; a result that does not fit its `u64` field is an error
//!   ([`PricingError::Overflow`]), never a wrap.
//!
//! # Map
//!
//! | Module | Spec | Contents |
//! | --- | --- | --- |
//! | [`book`] | §2 inputs | depth walk [`ask`] / [`bid`], mids, executable depth within [`DEPTH_WINDOW_U`] |
//! | [`tick`] | §1 | tick size at a price, snapping |
//! | [`payout`](mod@payout) | §2 step 3, §10, C2 | [`m1_ppm`], [`m_n_ppm`], [`payout()`], [`floor_ok`], [`r_tilde`], [`settle`] |
//! | [`quote`](mod@quote) | §2 steps 1–4 | [`side_price`], [`sizing_delta`], [`overlaps`], [`min_payout_per_unit`], [`p_max`], [`taker_fee_u`], [`quote()`] |
//! | [`finalize`](mod@finalize) | §2 step 5 | [`finalize()`] |
//! | [`cashout`] | §2 cash-out | [`cash_out`] |
//! | [`desk`] | §3 | [`delta_i`], [`net_target`], [`hedged_set`], [`needs_rebalance`], [`prehedge_trigger`] |
//! | [`rules`] | §2 correlation, §3 caps | [`check_correlation`], [`Caps`], [`Params`], [`Rejection`] |

pub mod book;
pub mod cashout;
pub mod desk;
pub mod finalize;
pub mod payout;
pub mod quote;
pub mod rules;
pub mod tick;

use serde::{Deserialize, Serialize};

pub use book::{
    DEPTH_WINDOW_U, ask, bid, close_price, executable_depth, mid, opening_depth, side_mid,
};
pub use cashout::{CashOut, CashOutLeg, LegStatus, cash_out};
pub use desk::{
    HedgeLeg, LegState, TargetEntry, delta_i, delta_i_micro, hedged_set, needs_rebalance,
    net_target, prehedge_trigger,
};
pub use finalize::{Finalized, LegFill, finalize, side_notional_u};
pub use payout::{Payout, PayoutLeg, floor_ok, m_n_ppm, m1_ppm, odds_u, payout, r_tilde, settle};
pub use quote::{
    FLOOR_MIN_RS_U, LegInput, LegMarket, PMaxLeg, Quote, QuoteLeg, Teams, min_payout_per_unit,
    overlaps, p_max, pascal_fees_est_u, quote, side_price, sizing_delta, taker_fee_u,
};
pub use rules::{
    Caps, MarginTiers, NBA_GAME_LENGTH_MS, Params, RejectCode, RejectStatus, Rejection,
    SOCCER_GAME_LENGTH_MS, check_correlation,
};
pub use tick::TickSpec;

/// Most legs a ticket may hold (spec §3 caps); the `u128` bounds in this crate assume it.
pub const MAX_LEGS: usize = 5;

/// One whole unit in micro-units: 1 USDC, a price of 1.0, a ratio of 100%.
pub const UNIT: u64 = 1_000_000;

/// [`UNIT`] as `u128`, for products.
pub(crate) const UNIT_128: u128 = UNIT as u128;

/// The user's side of a leg on the market's single symbol (spec §2 step 1, D-021).
///
/// NBA moneylines list one symbol per game whose long side is the away team, so a home pick is
/// a `Short` at `1 − p`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// The user backs the market's long outcome; the desk hedges with a `BID` at `p`.
    Long,
    /// The user backs the other outcome; the desk hedges with an `ASK` at `1 − p`.
    Short,
}

impl Side {
    /// `σ` of the net-target formula: `+1` long, `−1` short.
    pub fn sigma(self) -> i64 {
        match self {
            Side::Long => 1,
            Side::Short => -1,
        }
    }

    /// Lowercase name, as on the wire (`"long"`, `"short"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Long => "long",
            Side::Short => "short",
        }
    }
}

/// Which team the user picked (contract C4's `pick`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pick {
    /// The away team.
    Away,
    /// The home team.
    Home,
}

impl Pick {
    /// The other team.
    pub fn other(self) -> Pick {
        match self {
            Pick::Away => Pick::Home,
            Pick::Home => Pick::Away,
        }
    }
}

/// Arithmetic or input errors of the pure functions (rejections a user can see are
/// [`Rejection`]s instead).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PricingError {
    /// A price outside `(0, 1]` (`f_u`, `pᵢ`) or a ratio above `1e6`.
    #[error("invalid price or ratio {0} (must be in (0, 1_000_000])")]
    InvalidPrice(u64),
    /// A result does not fit its integer type.
    #[error("arithmetic overflow")]
    Overflow,
    /// No legs, or more legs than a ticket may hold.
    #[error("invalid leg count {0}")]
    LegCount(usize),
    /// `rS = 0`: odds are undefined.
    #[error("zero hedged stake (rS = 0)")]
    ZeroStake,
    /// A leg in the hedged set has no fill, so `r = 0` and the ticket cannot be minted.
    #[error("hedged leg {0} has no fill")]
    NoFill(usize),
    /// No leg is marked hedged, so `r` is undefined.
    #[error("no hedged leg")]
    NoHedgedLeg,
    /// A hedged leg has a zero target size `Δᵢ`.
    #[error("hedged leg {0} has a zero target size")]
    ZeroDelta(usize),
    /// A leg index outside the ticket.
    #[error("leg index {0} out of range")]
    LegIndex(usize),
    /// No max price on tick keeps the floor (only with a zero or tiny slippage).
    #[error("no max price satisfies the floor")]
    Unpriceable,
}

/// `u128` → `u64`, or [`PricingError::Overflow`].
pub(crate) fn to_u64(x: u128) -> Result<u64, PricingError> {
    u64::try_from(x).map_err(|_| PricingError::Overflow)
}

/// `ceil(a / b)` for `b > 0`.
pub(crate) fn div_ceil(a: u128, b: u128) -> u128 {
    a.div_ceil(b)
}
