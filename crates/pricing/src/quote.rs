//! Section 2 steps 1–4 and the top-level [`quote()`] that fills contract C4's `/quote` response
//! (all but the signature fields, which step 2.A adds).

use std::collections::BTreeMap;

use pascal_rs::num::decimal_to_micros;
use pascal_rs::{Market, MarketType, OrderBook, Outcome, Sport, SportsMarket};
use serde::{Deserialize, Serialize};

use crate::book::{opening_depth, side_mid};
use crate::desk::{HedgeLeg, LegState, delta_i, hedged_set};
use crate::payout::{PayoutLeg, floor_ok, m_n_ppm, m1_ppm, payout};
use crate::rules::{Params, RejectCode, Rejection, check_correlation, usdc};
use crate::tick::TickSpec;
use crate::{MAX_LEGS, Pick, PricingError, Side, UNIT, UNIT_128, ask, bid, div_ceil};

/// Smallest `rS` (micro-USDC) for which [`p_max`] guarantees the floor: 0.01 USDC.
///
/// With 3% slippage the guarantee holds from about 0.0065 USDC on any 5-leg ticket, so this
/// costs nothing; it only matters for a zero or tiny slippage, where [`p_max`] tightens the max
/// prices until it holds.
pub const FLOOR_MIN_RS_U: u64 = 10_000;

/// Fixed-point scale of the floor bound in [`p_max`].
const BOUND_SCALE: u128 = 1_000_000_000_000;

/// Both teams of a game, for the "no team on two legs the same day" rule.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Teams {
    /// `market_type.teams.away_subject_id`, e.g. `philadelphia-76ers`.
    pub away_subject_id: String,
    /// `market_type.teams.home_subject_id`.
    pub home_subject_id: String,
    /// Away abbreviation, e.g. `PHI`.
    pub away_abbr: String,
    /// Home abbreviation, e.g. `IND`.
    pub home_abbr: String,
}

impl Teams {
    /// A team (subject id or abbreviation, compared case-insensitively; empty values ignored)
    /// present in both games.
    pub fn shared_with(&self, other: &Teams) -> Option<String> {
        let ids = |t: &Teams| [t.away_subject_id.clone(), t.home_subject_id.clone()];
        let abbrs = |t: &Teams| [t.away_abbr.to_uppercase(), t.home_abbr.to_uppercase()];
        let (a_ids, b_ids) = (ids(self), ids(other));
        if let Some(id) = a_ids.iter().find(|x| !x.is_empty() && b_ids.contains(x)) {
            return Some(id.clone());
        }
        let (a_ab, b_ab) = (abbrs(self), abbrs(other));
        a_ab.iter()
            .find(|x| !x.is_empty() && b_ab.contains(x))
            .cloned()
    }
}

/// The kind of market a leg is on (spec §2 correlation rule, widened by 6.0b).
///
/// Every kind is a binary contract with one symbol: the long side is the market's own outcome
/// (the team for a winner, the team covering its line for a spread, Over for a total) and the
/// short side its complement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegMarket {
    /// Full-game moneyline with a home or away outcome (D-008).
    Winner,
    /// Full-game point spread (`market.type == "spread"`).
    Spread,
    /// Full-game total points (`market.type == "total_points"`).
    Total,
    /// Anything else (props, other sports' market types): refused.
    Other,
    /// One outcome of a three-way full-time result (soccer 1-X-2, 7.0b, D-435): Pascal lists one
    /// yes-market per outcome (home, draw, away), so a leg is a long on its outcome's own symbol
    /// and is never short ([`check_correlation`](crate::check_correlation) refuses a short). For
    /// timing it is a winner: it may be bet in play (U-2, D-322).
    ThreeWay,
}

impl LegMarket {
    /// Lowercase name, as on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            LegMarket::Winner => "winner",
            LegMarket::Spread => "spread",
            LegMarket::Total => "total",
            LegMarket::Other => "other",
            LegMarket::ThreeWay => "three_way",
        }
    }

    /// Whether the market decides who wins the game (a two-way winner or a 1-X-2 outcome): the
    /// kinds that may be bet in play (U-2).
    pub fn is_winner(self) -> bool {
        matches!(self, LegMarket::Winner | LegMarket::ThreeWay)
    }
}

/// One leg as the quoter sees it: the user's pick plus the catalogue data (D-008, D-009).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegInput {
    /// Pascal symbol, e.g. `NBA_26OCT28_PHIIND_ML.PHI`.
    pub symbol: String,
    /// The user's side on that symbol.
    pub side: Side,
    /// `display_attributes.game_page.game_id`.
    pub game_id: String,
    /// `display_attributes.expected_event_start_time_ms`.
    pub start_ms: u64,
    /// Winner, spread, total or 1-X-2 outcome ([`LegMarket::Other`] is refused by
    /// [`check_correlation`]).
    pub market: LegMarket,
    /// The team of the market's long side: `market.outcome` for a winner (away on NBA) or a
    /// 1-X-2 outcome, `market.team` for a spread. A total has no team: its long side (Over) is
    /// recorded as [`Pick::Away`] by convention (proposed D-321), and `pick` means nothing for
    /// it. A 1-X-2 draw has no team either: it is recorded as [`Pick::Home`] (proposed D-461).
    pub long_pick: Pick,
    /// Both teams.
    pub teams: Teams,
    /// Tick parameters from `/markets`.
    pub tick: TickSpec,
    /// `taker_fee_rate` from `/markets`, ppm (20_000 on NBA).
    pub taker_fee_rate_ppm: u32,
}

impl LegInput {
    /// The leg for `side` of a catalogue market. A market without a game id or start time is a
    /// rejection; a soccer moneyline (any outcome, the draw included) is a
    /// [`LegMarket::ThreeWay`]; a sports market that is not a moneyline, a spread or a total
    /// points market gets [`LegMarket::Other`], which [`check_correlation`] rejects with its
    /// reason.
    pub fn from_market(m: &Market, side: Side) -> Result<LegInput, Rejection> {
        let da = &m.display_attributes;
        let not_game = || {
            Rejection::new(
                RejectCode::UnsupportedMarket,
                format!("{} is not a market on a game", m.symbol),
            )
        };
        let game_id = da.game_page.as_ref().ok_or_else(not_game)?.game_id.clone();
        let start_ms = da.expected_event_start_time_ms.ok_or_else(not_game)?;
        let (market, long_pick, mut teams) = match &da.market_type {
            Some(MarketType::Sports(s)) => {
                let teams = Teams {
                    away_subject_id: s.teams.away_subject_id.clone(),
                    home_subject_id: s.teams.home_subject_id.clone(),
                    ..Teams::default()
                };
                match &s.market {
                    // One yes-market per outcome (7.0b): long only, the draw has no team.
                    SportsMarket::Moneyline { outcome } if s.sport == Sport::Soccer => (
                        LegMarket::ThreeWay,
                        match outcome {
                            Outcome::Away => Pick::Away,
                            Outcome::Home | Outcome::Draw => Pick::Home,
                        },
                        teams,
                    ),
                    // Soccer is winner-only (D-435).
                    _ if s.sport == Sport::Soccer => (LegMarket::Other, Pick::Away, teams),
                    SportsMarket::Moneyline {
                        outcome: Outcome::Away,
                    } => (LegMarket::Winner, Pick::Away, teams),
                    SportsMarket::Moneyline {
                        outcome: Outcome::Home,
                    } => (LegMarket::Winner, Pick::Home, teams),
                    SportsMarket::Spread { team, .. } => (
                        LegMarket::Spread,
                        match team {
                            pascal_rs::TeamSide::Away => Pick::Away,
                            pascal_rs::TeamSide::Home => Pick::Home,
                        },
                        teams,
                    ),
                    SportsMarket::TotalPoints { .. } => (LegMarket::Total, Pick::Away, teams),
                    _ => (LegMarket::Other, Pick::Away, teams),
                }
            }
            _ => (LegMarket::Other, Pick::Away, Teams::default()),
        };
        // Moneylines name both teams in `outcome_labels`; spreads and totals list none.
        if let (LegMarket::Winner, Some(labels)) = (market, &da.outcome_labels) {
            let (long, short) = (
                labels.long.abbreviated.clone(),
                labels.short.abbreviated.clone(),
            );
            (teams.away_abbr, teams.home_abbr) = match long_pick {
                Pick::Away => (long, short),
                Pick::Home => (short, long),
            };
        }
        let fee = decimal_to_micros(m.taker_fee_rate)
            .unwrap_or(0)
            .clamp(0, 1_000_000);
        Ok(LegInput {
            symbol: m.symbol.clone(),
            side,
            game_id,
            start_ms,
            market,
            long_pick,
            teams,
            tick: TickSpec::from_market(m),
            taker_fee_rate_ppm: fee as u32,
        })
    }

    /// The team the user picked: the long team for a long leg, the other one for a short leg
    /// (a convention without meaning on a total, see [`LegInput::long_pick`]).
    pub fn pick(&self) -> Pick {
        match self.side {
            Side::Long => self.long_pick,
            Side::Short => self.long_pick.other(),
        }
    }
}

/// One leg of a [`Quote`]: C4's leg fields, plus `mid_u`, `size` and `hedged`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuoteLeg {
    /// Pascal symbol.
    pub symbol: String,
    /// `game_id`.
    pub game_id: String,
    /// The user's side.
    pub side: Side,
    /// The team picked.
    pub pick: Pick,
    /// Start, Unix ms.
    pub start_ms: u64,
    /// `pᵢ`: ask-side quote price of the user's side at depth `size` (signed into the quote).
    pub price_u: u64,
    /// `pᵢᵐᵃˣ`: the IOC limit for the user's side, on tick (a short leg's order is an `ASK` at
    /// `1 − p_max_u` on the symbol).
    pub p_max_u: u64,
    /// `m₁,ᵢ`, ppm.
    pub m1_ppm: u32,
    /// Whether the leg's game overlaps another leg's.
    pub overlaps: bool,
    /// `p̃ᵢ`: indicative mid of the user's side.
    pub mid_u: u64,
    /// `Δᵢ`: contracts to hedge at inception (or at the roll), sized from mids.
    pub size: u64,
    /// Whether the leg is in the hedged set at inception.
    pub hedged: bool,
    /// Whether its game had started at the quote's `now`: an in-play winner leg (U-2, D-301).
    #[serde(default)]
    pub in_play: bool,
}

/// Every number of C4's `/quote` response except `quote_expires_at` and the signature fields,
/// plus `margin_ppm` and `payout_fair_u`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quote {
    /// Number of legs.
    pub n: u8,
    /// `S`, micro-USDC.
    pub stake_u: u64,
    /// Legs in ticket order.
    pub legs: Vec<QuoteLeg>,
    /// Fair odds: C2 with `m₁ = 0` at the side mids.
    pub odds_fair_u: u64,
    /// Offered odds: C2 with `r = 1` at the quote prices `pᵢ`.
    pub odds_offered_u: u64,
    /// Offered payout `P`, micro-USDC.
    pub payout_u: u64,
    /// The margin in money: fair payout − offered payout.
    pub fee_u: u64,
    /// Estimated Pascal taker fees on the hedge ([`pascal_fees_est_u`]).
    pub pascal_fees_est_u: u64,
    /// Copy fee (v1; always 0 here).
    pub copy_fee_u: u64,
    /// Slippage `s`, bps.
    pub slippage_bps: u32,
    /// The floor the user signs.
    pub min_payout_per_unit_u: u64,
    /// `m(n)`, ppm (display).
    pub margin_ppm: u32,
    /// Fair payout, micro-USDC.
    pub payout_fair_u: u64,
}

/// Step 1: `pᵢ = askᵢ(Δᵢ)` for a long leg, `1 − bidᵢ(Δᵢ)` for a short leg (both round up).
/// `None` when the book side holds fewer than `q` contracts.
pub fn side_price(book: &OrderBook, side: Side, q: u64) -> Option<u64> {
    match side {
        Side::Long => ask(book, q),
        Side::Short => bid(book, q).map(|b| UNIT - b),
    }
}

/// Step 2: `Δᵢ = ⌈ S/Πⱼp̃ⱼ · (1 − m(n)) · Πⱼ≠ᵢ p̃ⱼ ⌉`, i.e. `⌈P̃ · Πⱼ≠ᵢ p̃ⱼ⌉` with `P̃` the C2
/// payout at the mids (`legs[i].f_u = p̃ᵢ`); `≈ ⌈S(1 − m(n))/p̃ᵢ⌉`. One value per leg.
pub fn sizing_delta(stake_u: u64, legs: &[PayoutLeg]) -> Result<Vec<u64>, PricingError> {
    let p_tilde = payout(stake_u, UNIT as u32, legs)?.payout_u;
    let states: Vec<LegState> = legs
        .iter()
        .map(|l| LegState::Unresolved { price_u: l.f_u })
        .collect();
    (0..legs.len())
        .map(|i| delta_i(p_tilde, &states, i))
        .collect()
}

/// Step 3: whether each leg's game window `[start, start + game_length)` overlaps another
/// leg's, i.e. `|startᵢ − startⱼ| < game_length` for some `j ≠ i`.
pub fn overlaps(starts_ms: &[u64], game_length_ms: u64) -> Vec<bool> {
    starts_ms
        .iter()
        .enumerate()
        .map(|(i, &a)| {
            starts_ms
                .iter()
                .enumerate()
                .any(|(j, &b)| j != i && a.abs_diff(b) < game_length_ms)
        })
        .collect()
}

/// Step 4: `min_payout_per_unit = offered odds × (1 − s)`, rounded down.
pub fn min_payout_per_unit(odds_offered_u: u64, slippage_bps: u32) -> u64 {
    let keep = UNIT_128.saturating_sub(u128::from(slippage_bps) * 100);
    (u128::from(odds_offered_u) * keep / UNIT_128) as u64
}

/// Largest `x` with `xⁿ ≤ v` (binary search; `x ≤ 2·10⁶` here).
fn nth_root_floor(v: u128, n: u32) -> u128 {
    let (mut lo, mut hi) = (0_u128, 2 * UNIT_128 + 1);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        match mid.checked_pow(n) {
            Some(x) if x <= v => lo = mid,
            _ => hi = mid,
        }
    }
    lo
}

/// One leg's input to [`p_max`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PMaxLeg {
    /// `pᵢ`, the quote price.
    pub p_u: u64,
    /// `m₁,ᵢ`.
    pub m1_ppm: u32,
    /// The market's tick parameters.
    pub tick: TickSpec,
}

/// Whether max prices `pm` guarantee the floor for every `rS ≥ min_rs_u` and every fill
/// vector with `fᵢ ≤ pmᵢ` on hedged legs and `fᵢ = pᵢ` on unhedged ones.
///
/// C2's payout is non-increasing in each `fᵢ`, so the worst fills are `fᵢ = max(pmᵢ, pᵢ)`. With
/// `aⱼ = (1 − m₁,ⱼ)/fⱼ`, each floor loses less than one micro-unit, which later legs multiply, so
/// `payout > rS·A − E` with `A = Πⱼ aⱼ` and `E = Σₖ Πⱼ>ₖ aⱼ`. The floor
/// `payout·1e6 ≥ F·rS` then holds whenever `rS·(A·1e6 − F) ≥ E·1e6`. `A` is bounded below and
/// `E` above in fixed point, and the exact C2 check is also run at `rS = stake`.
fn floor_guaranteed(
    legs: &[PMaxLeg],
    pm: &[u64],
    min_ppu_u: u64,
    stake_u: u64,
    min_rs_u: u64,
) -> Result<bool, PricingError> {
    let pl: Vec<PayoutLeg> = legs
        .iter()
        .zip(pm)
        .map(|(l, &f)| PayoutLeg {
            m1_ppm: l.m1_ppm,
            // A leg left unhedged takes its quote price, which can sit above a max price
            // snapped below it.
            f_u: f.max(l.p_u),
        })
        .collect();
    let at_stake = payout(stake_u, UNIT as u32, &pl)?;
    if !floor_ok(at_stake.payout_u, at_stake.rs_u, min_ppu_u) {
        return Ok(false);
    }
    let keep = |l: &PayoutLeg| UNIT_128 - u128::from(l.m1_ppm);
    let mut a_lo = BOUND_SCALE;
    for l in &pl {
        a_lo = a_lo.checked_mul(keep(l)).ok_or(PricingError::Overflow)? / u128::from(l.f_u);
    }
    let (mut e, mut e_hi) = (BOUND_SCALE, BOUND_SCALE);
    for l in pl.iter().skip(1).rev() {
        e = div_ceil(
            e.checked_mul(keep(l)).ok_or(PricingError::Overflow)?,
            u128::from(l.f_u),
        );
        e_hi = e_hi.checked_add(e).ok_or(PricingError::Overflow)?;
    }
    let lhs = a_lo.checked_mul(UNIT_128).ok_or(PricingError::Overflow)?;
    let rhs = u128::from(min_ppu_u)
        .checked_mul(BOUND_SCALE)
        .ok_or(PricingError::Overflow)?;
    if lhs <= rhs {
        return Ok(false);
    }
    let rs_needed = div_ceil(
        e_hi.checked_mul(UNIT_128).ok_or(PricingError::Overflow)?,
        lhs - rhs,
    );
    Ok(rs_needed <= u128::from(min_rs_u.max(1)))
}

/// Step 4: per-leg max prices `pᵢᵐᵃˣ = pᵢ (1 + s)^(1/n)`, snapped down on tick, then tightened
/// one tick at a time (on the leg with the most room relative to its quote price) until the floor
/// is guaranteed: for every `rS ≥ min_rs_u`, every fill `fᵢ ≤ pᵢᵐᵃˣ` on a hedged leg and
/// `fᵢ = pᵢ` on a leg left unhedged (step 5), C2's payout satisfies
/// `payout·1e6 ≥ min_payout_per_unit·rS` (see the property test `floor_holds_below_p_max`).
///
/// `(1 + s)^(1/n)` is taken as the largest `g` (ppm) with `gⁿ ≤ (1 + s)`, so the product of the
/// unsnapped max prices is at most `(1 + s)·Πpᵢ`. A max price may land below its `pᵢ` when `pᵢ`
/// is an average between ticks on a cheap leg; the IOC then fills less and `r < 1`.
pub fn p_max(
    legs: &[PMaxLeg],
    slippage_bps: u32,
    min_payout_per_unit_u: u64,
    stake_u: u64,
    min_rs_u: u64,
) -> Result<Vec<u64>, PricingError> {
    let n = legs.len();
    // (1 + s)·1e6^(n−1) must fit u128: n ≤ 5 (the spec's cap) keeps it below 1e31.
    if n == 0 || n > MAX_LEGS {
        return Err(PricingError::LegCount(n));
    }
    for l in legs {
        if l.p_u == 0 || l.p_u >= UNIT {
            return Err(PricingError::InvalidPrice(l.p_u));
        }
    }
    let one_plus_s = UNIT_128 + u128::from(slippage_bps) * 100;
    let g = nth_root_floor(one_plus_s * UNIT_128.pow(n as u32 - 1), n as u32);
    let mut pm = Vec::with_capacity(n);
    for l in legs {
        let raw = (u128::from(l.p_u) * g / UNIT_128).min(UNIT_128 - 1) as u64;
        pm.push(l.tick.snap_down(raw).ok_or(PricingError::Unpriceable)?);
    }
    // Each pass lowers one price by at least one tick, so this terminates.
    loop {
        if floor_guaranteed(legs, &pm, min_payout_per_unit_u, stake_u, min_rs_u)? {
            return Ok(pm);
        }
        // Most room: largest pm_i / p_i among legs still above their quote price (lowering a
        // max price that is already at or below p_i cannot help: that leg's worst fill is p_i).
        let i = (0..n)
            .filter(|&k| pm[k] > legs[k].p_u)
            .max_by(|&a, &b| {
                (u128::from(pm[a]) * u128::from(legs[b].p_u))
                    .cmp(&(u128::from(pm[b]) * u128::from(legs[a].p_u)))
            })
            .ok_or(PricingError::Unpriceable)?;
        pm[i] = legs[i]
            .tick
            .snap_down(pm[i] - 1)
            .ok_or(PricingError::Unpriceable)?;
    }
}

/// Pascal's taker fee on `size` contracts at `price_u` (spec §3): `size × p × (1 − p) ×
/// taker_fee_rate`, rounded up (an estimate of a cost). The formula is symmetric, so the short
/// leg's `ASK` at `1 − p` costs the same.
pub fn taker_fee_u(size: u64, price_u: u64, taker_fee_rate_ppm: u32) -> u64 {
    let p = u128::from(price_u.min(UNIT));
    let rate = u128::from(taker_fee_rate_ppm).min(UNIT_128);
    // ≤ 1.8e19 · 2.5e11 · 1e6 < u128::MAX.
    let num = u128::from(size) * p * (UNIT_128 - p) * rate;
    u64::try_from(div_ceil(num, UNIT_128 * UNIT_128)).unwrap_or(u64::MAX)
}

/// Estimated Pascal fees on a ticket's hedge: `Σᵢ fee(Δᵢ, pᵢ)`, every leg's inception delta at
/// its quote price.
///
/// For a leg in the inception hedged set this is the inception order. For a later sequential leg
/// the roll buys `Δᵢ/Πⱼ<ᵢ pⱼ` contracts but only after every earlier leg won (probability
/// `Πⱼ<ᵢ pⱼ`), so its expected fee is the same term. Ladder top-ups and unwinds are not counted.
pub fn pascal_fees_est_u(sizes: &[u64], prices_u: &[u64], fee_rates_ppm: &[u32]) -> u64 {
    sizes
        .iter()
        .zip(prices_u)
        .zip(fee_rates_ppm)
        .map(|((&q, &p), &r)| taker_fee_u(q, p, r))
        .fold(0_u64, u64::saturating_add)
}

/// The whole `/quote` computation (spec §2 steps 1–4, C2, C4):
///
/// 1. [`check_correlation`] (shape, stake, market kind, timing, games, teams);
/// 2. side mids `p̃ᵢ` from `books` (a missing or one-sided book is `unavailable`);
/// 3. [`overlaps`] → `m₁,ᵢ`; [`sizing_delta`] `Δᵢ` at the mids;
/// 4. depth gate: `Δᵢ ≤` [`Caps::max_target`](crate::Caps::max_target) of the opening side's
///    executable depth; then `pᵢ` = [`side_price`] at `Δᵢ`;
/// 5. offered payout and odds per C2 with `r = 1`, `f = p`; `P ≤ max_payout`;
/// 6. fair payout and odds per C2 with `m₁ = 0`, `f = p̃`; `fee_u` = fair − offered payout;
/// 7. [`min_payout_per_unit`], [`p_max`], [`pascal_fees_est_u`], the inception hedged set.
pub fn quote(
    legs: &[LegInput],
    books: &BTreeMap<String, OrderBook>,
    stake_u: u64,
    now_ms: u64,
    params: &Params,
) -> Result<Quote, Rejection> {
    let caps = &params.caps;
    check_correlation(legs, stake_u, now_ms, caps)?;
    let unpriceable = |e: PricingError| {
        Rejection::new(
            RejectCode::Unpriceable,
            format!("this schedina cannot be priced ({e})"),
        )
    };

    let mut leg_books = Vec::with_capacity(legs.len());
    let mut mids = Vec::with_capacity(legs.len());
    for leg in legs {
        let book = books.get(&leg.symbol).ok_or_else(|| {
            Rejection::new(
                RejectCode::NoBook,
                format!("no book for {} right now", leg.symbol),
            )
        })?;
        let mid = side_mid(book, leg.side).ok_or_else(|| {
            Rejection::new(
                RejectCode::OneSidedBook,
                format!("{} has no two-sided book right now", leg.symbol),
            )
        })?;
        leg_books.push(book);
        mids.push(mid);
    }

    let starts: Vec<u64> = legs.iter().map(|l| l.start_ms).collect();
    let ov = overlaps(&starts, params.game_length_ms);
    let m1s: Vec<u32> = ov.iter().map(|&o| m1_ppm(o, &params.tiers)).collect();
    let mid_legs: Vec<PayoutLeg> = m1s
        .iter()
        .zip(&mids)
        .map(|(&m1_ppm, &f_u)| PayoutLeg { m1_ppm, f_u })
        .collect();
    let too_large = || {
        Rejection::new(
            RejectCode::PayoutTooLarge,
            format!(
                "the payout is above the {} USDC cap; lower the stake",
                usdc(caps.max_payout_u)
            ),
        )
    };
    // An overflowing payout at the mids means a payout far above the cap.
    let sizes = sizing_delta(stake_u, &mid_legs).map_err(|e| match e {
        PricingError::Overflow => too_large(),
        e => unpriceable(e),
    })?;

    let mut prices = Vec::with_capacity(legs.len());
    for ((leg, book), &size) in legs.iter().zip(&leg_books).zip(&sizes) {
        let depth = opening_depth(book, leg.side, params.depth_window_u);
        let price = (size <= caps.max_target(depth))
            .then(|| side_price(book, leg.side, size))
            .flatten()
            .ok_or_else(|| {
                Rejection::new(
                    RejectCode::NotEnoughDepth,
                    format!(
                        "not enough depth on {}: {size} contracts needed, {depth} within {} of the best price",
                        leg.symbol,
                        usdc(params.depth_window_u)
                    ),
                )
            })?;
        prices.push(price);
    }

    let quote_legs: Vec<PayoutLeg> = m1s
        .iter()
        .zip(&prices)
        .map(|(&m1_ppm, &f_u)| PayoutLeg { m1_ppm, f_u })
        .collect();
    let offered = match payout(stake_u, UNIT as u32, &quote_legs) {
        Ok(p) => p,
        Err(PricingError::Overflow) => return Err(too_large()),
        Err(e) => return Err(unpriceable(e)),
    };
    if offered.payout_u > caps.max_payout_u {
        return Err(too_large());
    }
    let fair_legs: Vec<PayoutLeg> = mids
        .iter()
        .map(|&f_u| PayoutLeg { m1_ppm: 0, f_u })
        .collect();
    let fair = payout(stake_u, UNIT as u32, &fair_legs).map_err(unpriceable)?;

    let min_ppu = min_payout_per_unit(offered.odds_u, params.slippage_bps);
    let pm_legs: Vec<PMaxLeg> = legs
        .iter()
        .zip(&prices)
        .zip(&m1s)
        .map(|((l, &p_u), &m1_ppm)| PMaxLeg {
            p_u,
            m1_ppm,
            tick: l.tick,
        })
        .collect();
    let pm = p_max(
        &pm_legs,
        params.slippage_bps,
        min_ppu,
        stake_u,
        params.floor_min_rs_u,
    )
    .map_err(unpriceable)?;

    let rates: Vec<u32> = legs.iter().map(|l| l.taker_fee_rate_ppm).collect();
    let fees = pascal_fees_est_u(&sizes, &prices, &rates);
    let hedge_legs: Vec<HedgeLeg> = starts
        .iter()
        .map(|&start_ms| HedgeLeg {
            start_ms,
            resolved: false,
            prehedged: false,
        })
        .collect();
    let hedged = hedged_set(&hedge_legs, params.game_length_ms);
    // An in-play leg resolves first, so no roll can reach it: it must be hedged at inception.
    // It is whenever `in_play_max_ms <= game_length_ms` (every in-play leg then overlaps the
    // earliest leg, which is in the set); refuse a configuration that breaks this.
    let in_play: Vec<bool> = legs.iter().map(|l| l.start_ms <= now_ms).collect();
    if let Some(i) = (0..legs.len()).find(|&i| in_play[i] && !hedged[i]) {
        return Err(Rejection::new(
            RejectCode::Unpriceable,
            format!(
                "game {} has started and cannot be hedged at once",
                legs[i].game_id
            ),
        ));
    }

    let out_legs = legs
        .iter()
        .enumerate()
        .map(|(i, l)| QuoteLeg {
            symbol: l.symbol.clone(),
            game_id: l.game_id.clone(),
            side: l.side,
            pick: l.pick(),
            start_ms: l.start_ms,
            price_u: prices[i],
            p_max_u: pm[i],
            m1_ppm: m1s[i],
            overlaps: ov[i],
            mid_u: mids[i],
            size: sizes[i],
            hedged: hedged[i],
            in_play: in_play[i],
        })
        .collect();
    Ok(Quote {
        n: legs.len() as u8,
        stake_u,
        legs: out_legs,
        odds_fair_u: fair.odds_u,
        odds_offered_u: offered.odds_u,
        payout_u: offered.payout_u,
        fee_u: fair.payout_u.saturating_sub(offered.payout_u),
        pascal_fees_est_u: fees,
        copy_fee_u: 0,
        slippage_bps: params.slippage_bps,
        min_payout_per_unit_u: min_ppu,
        margin_ppm: m_n_ppm(&m1s),
        payout_fair_u: fair.payout_u,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_is_strict() {
        let h = 3_600_000;
        let ov = overlaps(&[0, 2 * h, 4 * h, 20 * h], 5 * h / 2);
        assert_eq!(ov, vec![true, true, true, false]);
        // Exactly one game length apart: no overlap.
        assert_eq!(overlaps(&[0, 5 * h / 2], 5 * h / 2), vec![false, false]);
        assert_eq!(overlaps(&[0], 5 * h / 2), vec![false]);
    }

    #[test]
    fn floor_example_c4() {
        // C4: offered odds 1.395683, 3% slippage -> 1.353812.
        assert_eq!(min_payout_per_unit(1_395_683, 300), 1_353_812);
        assert_eq!(min_payout_per_unit(6_859_000, 300), 6_653_230);
    }

    #[test]
    fn p_max_c4_single_leg() {
        // C4 example: p = 0.695, n = 1 -> 0.695 · 1.03 = 0.71585 -> 0.715 on tick.
        let leg = PMaxLeg {
            p_u: 695_000,
            m1_ppm: 30_000,
            tick: TickSpec::NBA,
        };
        let pm = p_max(&[leg], 300, 1_353_812, 10_000_000, FLOOR_MIN_RS_U).unwrap();
        assert_eq!(pm, vec![715_000]);
    }

    #[test]
    fn p_max_default_and_tightening() {
        let legs = [PMaxLeg {
            p_u: 500_000,
            m1_ppm: 50_000,
            tick: TickSpec::NBA,
        }; 3];
        let off = payout(
            10_000_000,
            1_000_000,
            &[PayoutLeg {
                m1_ppm: 50_000,
                f_u: 500_000,
            }; 3],
        )
        .unwrap();
        // 3% slippage at 0.50: (1.03)^(1/3) = 1.00990 -> 0.50495 -> 0.504.
        let floor = min_payout_per_unit(off.odds_u, 300);
        let pm = p_max(&legs, 300, floor, 10_000_000, FLOOR_MIN_RS_U).unwrap();
        assert_eq!(pm, vec![504_000; 3]);
        // Zero slippage: no max price can protect a partial fill, so the ticket is unpriceable.
        let floor = min_payout_per_unit(off.odds_u, 0);
        assert_eq!(
            p_max(&legs, 0, floor, 10_000_000, FLOOR_MIN_RS_U),
            Err(PricingError::Unpriceable)
        );
        // 1 bp on a 1-micro tick: 0.5 · 1.0001 = 0.50005 is too close to the floor for small
        // partial fills, so the max price is tightened below it (but stays above p).
        let fine = PMaxLeg {
            p_u: 500_000,
            m1_ppm: 30_000,
            tick: TickSpec {
                tick_size_min_u: 1,
                tick_sig_figs: 6,
            },
        };
        let floor = min_payout_per_unit(1_940_000, 1);
        let pm = p_max(&[fine], 1, floor, 10_000_000, FLOOR_MIN_RS_U).unwrap();
        assert!(pm[0] > 500_000 && pm[0] < 500_050, "{pm:?}");
    }

    #[test]
    fn p_max_rejects_more_than_five_legs() {
        let leg = PMaxLeg {
            p_u: 500_000,
            m1_ppm: 50_000,
            tick: TickSpec::NBA,
        };
        assert_eq!(
            p_max(&[leg; 6], 300, 0, 10_000_000, FLOOR_MIN_RS_U),
            Err(PricingError::LegCount(6))
        );
        assert_eq!(
            p_max(&[], 300, 0, 10_000_000, FLOOR_MIN_RS_U),
            Err(PricingError::LegCount(0))
        );
    }

    #[test]
    fn nth_root() {
        assert_eq!(nth_root_floor(1_030_000, 1), 1_030_000);
        // sqrt(1.03) = 1.014889..
        assert_eq!(nth_root_floor(1_030_000 * UNIT_128, 2), 1_014_889);
        // 1.03^(1/5) = 1.005929..
        assert_eq!(nth_root_floor(1_030_000 * UNIT_128.pow(4), 5), 1_005_929);
    }

    #[test]
    fn taker_fee() {
        // 15 contracts at 0.695, 2%: 15 · 0.695 · 0.305 · 0.02 = 0.0635925.
        assert_eq!(taker_fee_u(15, 695_000, 20_000), 63_593);
        assert_eq!(taker_fee_u(15, 305_000, 20_000), 63_593);
        // 20 at 0.50: 0.1 exactly.
        assert_eq!(taker_fee_u(20, 500_000, 20_000), 100_000);
        assert_eq!(taker_fee_u(u64::MAX, 500_000, 1_000_000), u64::MAX);
        assert_eq!(taker_fee_u(u64::MAX, 500_000, u32::MAX), u64::MAX);
    }

    #[test]
    fn teams_shared() {
        let t = |a: &str, h: &str, aa: &str, ha: &str| Teams {
            away_subject_id: a.into(),
            home_subject_id: h.into(),
            away_abbr: aa.into(),
            home_abbr: ha.into(),
        };
        let x = t("philadelphia-76ers", "indiana-pacers", "PHI", "IND");
        assert_eq!(
            x.shared_with(&t("indiana-pacers", "boston-celtics", "IND", "BOS")),
            Some("indiana-pacers".into())
        );
        assert_eq!(x.shared_with(&t("", "", "bos", "phi")), Some("PHI".into()));
        assert_eq!(x.shared_with(&t("", "", "", "")), None);
    }
}
