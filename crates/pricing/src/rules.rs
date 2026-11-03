//! Configuration (margin tiers, caps, desk knobs), the correlation rule and its typed rejections
//! (spec §2 "Correlation rule", §3 "Caps").

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::book::DEPTH_WINDOW_U;
use crate::quote::{FLOOR_MIN_RS_U, LegInput, LegMarket};
use crate::{Side, UNIT, UNIT_128};

/// One hour, ms.
const HOUR_MS: u64 = 3_600_000;

/// Length of an NBA game for the overlap test: 2.5 h ([`Params::game_length_ms`]'s default).
pub const NBA_GAME_LENGTH_MS: u64 = 5 * HOUR_MS / 2;

/// Length of a soccer match (90 min, half time, stoppage): 2 h, as Pascal's
/// `expected_resolution_time_ms − expected_event_start_time_ms` on every EPL, Bundesliga and
/// LaLiga winner market of 28 Oct (7.0b). The overlap test keeps one window for every pair of
/// legs, the longest sport's ([`Params::game_length_ms`], proposed D-460), so a soccer leg is
/// never judged sequential while its match can still be running; `Caps::in_play_max_ms` stays
/// at or below this, so an in-play soccer leg is always hedged at inception (D-322).
pub const SOCCER_GAME_LENGTH_MS: u64 = 2 * HOUR_MS;

/// Per-leg margin tiers `m₁` (spec §2 step 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarginTiers {
    /// A leg whose game overlaps no other leg's (hedged by the roll): 3%.
    pub sequential_ppm: u32,
    /// A leg whose game overlaps another leg's (hedged by the ladder): 5%.
    pub overlap_ppm: u32,
}

impl Default for MarginTiers {
    fn default() -> Self {
        MarginTiers {
            sequential_ppm: 30_000,
            overlap_ppm: 50_000,
        }
    }
}

/// The section 3 caps, MVP values by default.
///
/// Two caps are balances checked elsewhere and only have a helper here: the vault's free
/// balance (the program checks `vault.free ≥ P` at `finalize`) and the hedger's free collateral.
/// Overlapping legs have no count cap: they are priced at the overlap tier instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caps {
    /// Legs per ticket: 5 ("deltas double per won leg").
    pub max_legs: usize,
    /// Minimum stake: 5 USDC (integer-contract rounding under 10% of the hedge).
    pub min_stake_u: u64,
    /// Maximum `P` per ticket: 500 USDC (last-leg depth).
    pub max_payout_u: u64,
    /// Maximum `Tᵢ` per leg as a share of the executable depth within the depth window: 50%.
    pub max_target_depth_ppm: u32,
    /// Legs must start within this window from now: 24 h.
    pub start_window_ms: u64,
    /// A winner leg may be taken in play until this long after its start: 2 h (U-2, proposed
    /// D-322). Kept at or below [`Params::game_length_ms`] so every in-play leg overlaps the
    /// ticket's first leg and is hedged at inception.
    #[serde(default = "default_in_play_max_ms")]
    pub in_play_max_ms: u64,
}

fn default_in_play_max_ms() -> u64 {
    2 * HOUR_MS
}

impl Default for Caps {
    fn default() -> Self {
        Caps {
            max_legs: crate::MAX_LEGS,
            min_stake_u: 5 * UNIT,
            max_payout_u: 500 * UNIT,
            max_target_depth_ppm: 500_000,
            start_window_ms: 24 * HOUR_MS,
            in_play_max_ms: default_in_play_max_ms(),
        }
    }
}

impl Caps {
    /// Max `|Tᵢ|` for a leg whose book shows `depth` contracts within the depth window:
    /// `floor(depth × 50%)`.
    pub fn max_target(&self, depth: u64) -> u64 {
        (u128::from(depth) * u128::from(self.max_target_depth_ppm) / UNIT_128) as u64
    }

    /// Vault cap: free balance ≥ Σ `P` over open tickets (full collateralization).
    pub fn vault_covers(&self, vault_free_u: u64, sum_open_payouts_u: u64) -> bool {
        vault_free_u >= sum_open_payouts_u
    }

    /// Hedger cap: free collateral ≥ Σ next-rebalance cost (no `INSUFFICIENT_COLLATERAL`
    /// mid-game).
    pub fn hedger_covers(&self, free_collateral_u: u64, next_rebalance_cost_u: u64) -> bool {
        free_collateral_u >= next_rebalance_cost_u
    }
}

/// Every knob of the pricing and desk formulas, spec values by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Params {
    /// `m₁` tiers.
    pub tiers: MarginTiers,
    /// Game length for the overlap test: 2.5 h for NBA ([`NBA_GAME_LENGTH_MS`]). One window for
    /// every pair of legs, whatever their sports, so it must be the longest offered sport's
    /// (soccer's is [`SOCCER_GAME_LENGTH_MS`], proposed D-460).
    pub game_length_ms: u64,
    /// Slippage `s` on the whole payout, bps: 300 (3%).
    pub slippage_bps: u32,
    /// Cash-out fee `c`, ppm: 20_000 (2%).
    pub cash_out_fee_ppm: u32,
    /// Executable-depth window, micro-units ([`DEPTH_WINDOW_U`]).
    pub depth_window_u: u64,
    /// Rebalance when off target by more than this share of `|Tᵢ|`, ppm: 100_000 (10%) …
    pub rebalance_rel_ppm: u32,
    /// … and by more than this many contracts: 2.
    pub rebalance_min_contracts: u64,
    /// Pre-hedge an unhedged leg whose price moved more than this against the desk: 0.05.
    pub prehedge_move_u: u64,
    /// Smallest `rS` for which [`p_max`](crate::p_max) guarantees the floor.
    pub floor_min_rs_u: u64,
    /// The section 3 caps.
    pub caps: Caps,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            tiers: MarginTiers::default(),
            game_length_ms: NBA_GAME_LENGTH_MS,
            slippage_bps: 300,
            cash_out_fee_ppm: 20_000,
            depth_window_u: DEPTH_WINDOW_U,
            rebalance_rel_ppm: 100_000,
            rebalance_min_contracts: 2,
            prehedge_move_u: 50_000,
            floor_min_rs_u: FLOOR_MIN_RS_U,
            caps: Caps::default(),
        }
    }
}

/// HTTP class of a rejection (contract C4): `rejected` (422, the ticket itself is not allowed)
/// or `unavailable` (503, the desk cannot price it right now).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RejectStatus {
    /// 422.
    Rejected,
    /// 503.
    Unavailable,
}

/// Why a quote or a cash-out was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectCode {
    /// No legs.
    NoLegs,
    /// More than `max_legs`.
    TooManyLegs,
    /// Stake below `min_stake`.
    StakeTooSmall,
    /// A leg that is not a two-way winner, a spread, a total (6.0b; was `NotMoneyline`) or a
    /// 1-X-2 outcome (7.0b).
    UnsupportedMarket,
    /// Two legs on one `game_id`.
    SameGame,
    /// A spread or total on a game that has started, or a winner past the in-play window.
    AlreadyStarted,
    /// A game starting more than 24 h from now.
    StartsTooLate,
    /// One team on two legs the same day.
    SharedTeam,
    /// A leg's market has no book.
    NoBook,
    /// A book without both sides (no mid).
    OneSidedBook,
    /// Not enough executable depth for the hedge (or, at cash-out, to close it).
    NotEnoughDepth,
    /// `P` above `max_payout`.
    PayoutTooLarge,
    /// The prices give no valid payout (overflow, no max price on tick).
    Unpriceable,
    /// Cash-out: a leg is lost, the ticket is worth 0.
    LegLost,
    /// Cash-out: every leg is resolved; the ticket settles instead.
    AllResolved,
    /// A short leg on a 1-X-2 outcome (7.0b): soccer outcomes are backed on their own symbol,
    /// never laid.
    LongOnly,
}

impl RejectCode {
    /// The HTTP class.
    pub fn status(self) -> RejectStatus {
        match self {
            RejectCode::NoBook | RejectCode::OneSidedBook => RejectStatus::Unavailable,
            _ => RejectStatus::Rejected,
        }
    }
}

/// A typed rejection with the reason string shown to the user (C4:
/// `{"status":"rejected","reason":"two legs on game nba-phi-ind-2026-10-28"}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejection {
    /// Machine-readable cause.
    pub code: RejectCode,
    /// Human-readable reason.
    pub reason: String,
}

impl Rejection {
    /// A rejection with `code` and `reason`.
    pub fn new(code: RejectCode, reason: impl Into<String>) -> Self {
        Rejection {
            code,
            reason: reason.into(),
        }
    }

    /// The HTTP class.
    pub fn status(&self) -> RejectStatus {
        self.code.status()
    }
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for Rejection {}

/// `12.345678` USDC as `"12.34"` (truncated: reasons only).
pub(crate) fn usdc(u: u64) -> String {
    format!("{}.{:02}", u / UNIT, (u % UNIT) / 10_000)
}

/// Correlation rule and ticket-shape caps (spec §2, D-008), checked in this order:
///
/// 1. `1 ≤ n ≤ max_legs` (5);
/// 2. stake ≥ `min_stake` (5 USDC);
/// 3. every leg a two-way winner (moneyline), a spread or a total points market (6.0b), or a
///    long on a 1-X-2 outcome (soccer, 7.0b: never short);
/// 4. every leg starts within `start_window` (24 h) of `now`, and after `now`, except that a
///    winner leg (two-way or 1-X-2) may be in play for up to `in_play_max_ms` (2 h) after its
///    start (U-2, relaxing D-031 for winners only; spread and total legs close at tip-off);
/// 5. one leg per `game_id`, whatever the market kinds (a winner and a spread on one game are
///    correlated);
/// 6. no team, by subject id or abbreviation, on two legs starting less than 24 h apart ("the
///    same day"; with every start inside the 24 h window this covers every pair of pre-game
///    legs).
pub fn check_correlation(
    legs: &[LegInput],
    stake_u: u64,
    now_ms: u64,
    caps: &Caps,
) -> Result<(), Rejection> {
    use RejectCode::*;
    if legs.is_empty() {
        return Err(Rejection::new(NoLegs, "a schedina needs at least one leg"));
    }
    if legs.len() > caps.max_legs {
        return Err(Rejection::new(
            TooManyLegs,
            format!("{} legs, at most {} allowed", legs.len(), caps.max_legs),
        ));
    }
    if stake_u < caps.min_stake_u {
        return Err(Rejection::new(
            StakeTooSmall,
            format!(
                "stake {} USDC is below the {} USDC minimum",
                usdc(stake_u),
                usdc(caps.min_stake_u)
            ),
        ));
    }
    for leg in legs {
        if leg.market == LegMarket::Other {
            return Err(Rejection::new(
                UnsupportedMarket,
                format!("{} is not a winner, spread or total market", leg.symbol),
            ));
        }
        if leg.market == LegMarket::ThreeWay && leg.side == Side::Short {
            return Err(Rejection::new(
                LongOnly,
                format!(
                    "{} cannot be laid: a soccer result is backed on its own outcome (home, draw or away), never short",
                    leg.symbol
                ),
            ));
        }
        if leg.start_ms <= now_ms {
            if !leg.market.is_winner() {
                return Err(Rejection::new(
                    AlreadyStarted,
                    format!(
                        "game {} has already started: {} bets close at tip-off",
                        leg.game_id,
                        leg.market.as_str()
                    ),
                ));
            }
            if now_ms - leg.start_ms >= caps.in_play_max_ms {
                return Err(Rejection::new(
                    AlreadyStarted,
                    format!(
                        "game {} started more than {} min ago: too late to bet on the winner",
                        leg.game_id,
                        caps.in_play_max_ms / 60_000
                    ),
                ));
            }
            continue;
        }
        if leg.start_ms - now_ms > caps.start_window_ms {
            return Err(Rejection::new(
                StartsTooLate,
                format!(
                    "game {} starts more than {} h from now",
                    leg.game_id,
                    caps.start_window_ms / HOUR_MS
                ),
            ));
        }
    }
    for (i, a) in legs.iter().enumerate() {
        for b in &legs[i + 1..] {
            if a.game_id == b.game_id {
                return Err(Rejection::new(
                    SameGame,
                    format!("two legs on game {}", a.game_id),
                ));
            }
            if a.start_ms.abs_diff(b.start_ms) < 24 * HOUR_MS
                && let Some(team) = a.teams.shared_with(&b.teams)
            {
                return Err(Rejection::new(
                    SharedTeam,
                    format!(
                        "{team} plays in two legs the same day ({} and {})",
                        a.game_id, b.game_id
                    ),
                ));
            }
        }
    }
    Ok(())
}
