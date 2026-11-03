//! Markets: `GET /api/v1/markets`, `/markets/resolved`, `POST /markets/search`, books, and the
//! `market_update` WebSocket channel.

use std::collections::BTreeMap;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::num::{MicroUsd, format_micros, micros, opt_micros, opt_str_num, parse_micros, str_num};

/// One market (`MarketStateMsg`): spec, display attributes and current state.
///
/// The same shape is returned by the live list, the resolved list, search, and the
/// `market_update` channel (which omits `stats`, `best_bid` and `best_ask`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Market {
    /// Globally unique `EVENT.MARKET` symbol.
    pub symbol: String,
    /// Base taker fee rate (fraction, e.g. `0.001` = 10 bps).
    #[serde(with = "str_num")]
    pub taker_fee_rate: Decimal,
    /// Share of the taker fee rebated to makers (fraction).
    #[serde(with = "str_num")]
    pub maker_rebate_share: Decimal,
    /// Minimum tick size, micro-USD.
    #[serde(with = "micros")]
    pub tick_size_min: MicroUsd,
    /// Significant figures of the tick-size formula, 1..=6.
    pub tick_sig_figs: u8,
    /// Display metadata (topic, tags, market type, expected resolution time, …).
    pub display_attributes: DisplayAttributes,
    /// Listing time, Unix ms.
    #[serde(default, with = "opt_str_num")]
    pub listing_ts_ms: Option<u64>,
    /// URL of the resolution rules document.
    #[serde(default)]
    pub rules_url: Option<String>,
    /// Rolling 24 h and all-time statistics (absent on `market_update`).
    #[serde(default)]
    pub stats: Option<MarketStats>,
    /// Median of best bid, best ask and last trade, micro-USD.
    #[serde(default, with = "opt_micros")]
    pub mark_price: Option<MicroUsd>,
    /// Best bid, micro-USD; absent when the book has no bids.
    #[serde(default, with = "opt_micros")]
    pub best_bid: Option<MicroUsd>,
    /// Best ask, micro-USD; absent when the book has no asks.
    #[serde(default, with = "opt_micros")]
    pub best_ask: Option<MicroUsd>,
    /// Legacy open interest (= `long_interest`), contracts. Deprecated by Pascal.
    #[serde(with = "str_num")]
    pub open_interest: u64,
    /// Contracts held long.
    #[serde(default, with = "opt_str_num")]
    pub long_interest: Option<u64>,
    /// Contracts held short.
    #[serde(default, with = "opt_str_num")]
    pub short_interest: Option<u64>,
    /// Final resolution, present once the market has resolved.
    #[serde(default)]
    pub resolution: Option<MarketResolution>,
}

impl Market {
    /// The event part of the symbol (`NYCMAYOR_25NOV04` for `NYCMAYOR_25NOV04.MAMDANI`).
    pub fn event(&self) -> &str {
        self.symbol
            .split_once('.')
            .map_or(self.symbol.as_str(), |(e, _)| e)
    }

    /// The topic id, if the market has a topic.
    pub fn topic_id(&self) -> Option<&str> {
        self.display_attributes
            .topic
            .as_ref()
            .map(|t| t.id.as_str())
    }
}

/// Final market resolution (`MarketResolutionMsg`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct MarketResolution {
    /// Settlement price, micro-USD (`1_000_000` = the market's side won).
    #[serde(with = "micros")]
    pub price: MicroUsd,
    /// Matching-engine sequence at resolution.
    #[serde(with = "str_num")]
    pub seq: u64,
    /// Resolution time, Unix ms.
    #[serde(with = "str_num")]
    pub market_resolution_ts_ms: u64,
}

/// Market statistics (`MarketStatsMsg`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct MarketStats {
    /// Rolling 24 h window.
    pub last_24h: MarketStatsSnapshot,
    /// All-time window.
    pub all_time: MarketStatsSnapshot,
}

/// One statistics window (`MarketStatsSnapshotMsg`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct MarketStatsSnapshot {
    /// Mark price at the window start, micro-USD.
    #[serde(default, with = "opt_micros")]
    pub mark_price_start: Option<MicroUsd>,
    /// Contracts bought by takers.
    #[serde(with = "str_num")]
    pub taker_buy_volume: u64,
    /// Contracts sold by takers.
    #[serde(with = "str_num")]
    pub taker_sell_volume: u64,
    /// Σ price × size of taker buys, micro-USD.
    #[serde(with = "micros")]
    pub taker_buy_volume_notional: MicroUsd,
    /// Σ price × size of taker sells, micro-USD.
    #[serde(with = "micros")]
    pub taker_sell_volume_notional: MicroUsd,
    /// Number of taker-buy trades.
    #[serde(with = "str_num")]
    pub taker_buy_trade_count: u64,
    /// Number of taker-sell trades.
    #[serde(with = "str_num")]
    pub taker_sell_trade_count: u64,
    /// Exclusive end of the window, Unix ms.
    #[serde(with = "str_num")]
    pub window_end_ts_ms_exclusive: u64,
}

impl MarketStatsSnapshot {
    /// Total trades in the window.
    pub fn trade_count(&self) -> u64 {
        self.taker_buy_trade_count + self.taker_sell_trade_count
    }
}

/// Display metadata (`MarketDisplayAttributes`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DisplayAttributes {
    /// Event title, e.g. "2025 NYC Mayoral Election".
    pub event_description: String,
    /// Market label within the event, e.g. "Mamdani".
    pub market_description: String,
    /// Label when the UI is reversed.
    #[serde(default)]
    pub reverse_description: Option<String>,
    /// Names of the long and short contracts (default Yes / No).
    #[serde(default)]
    pub outcome_labels: Option<OutcomeLabels>,
    /// Theme colours for head-to-head sections.
    #[serde(default)]
    pub colors: Option<DisplayColors>,
    /// Higher-level topic.
    #[serde(default)]
    pub topic: Option<Topic>,
    /// Game-line grouping for sports.
    #[serde(default)]
    pub game_page: Option<GamePage>,
    /// Typed market semantics (sports, company action).
    #[serde(default)]
    pub market_type: Option<MarketType>,
    /// Capitalised tags for filtering.
    pub tags: Vec<String>,
    /// Expected event start, Unix ms (indicative).
    #[serde(default, with = "opt_str_num")]
    pub expected_event_start_time_ms: Option<u64>,
    /// Expected resolution date, Unix ms (indicative).
    #[serde(with = "str_num")]
    pub expected_resolution_time_ms: u64,
    /// Default UI sort order.
    pub sort_order: MarketSort,
    /// Scalar-market display bounds.
    #[serde(default)]
    pub scalar: Option<RangeDisplay>,
    /// Undocumented: duplicate of `scalar` on range markets (seen in the 28 Oct fixtures).
    #[serde(default)]
    pub range: Option<RangeDisplay>,
    /// External reference that informs resolution.
    #[serde(default)]
    pub reference: Option<Reference>,
    /// Whether the market resolves only after `expected_resolution_time_ms`.
    #[serde(default)]
    pub resolves_at_expected_time: Option<bool>,
}

/// Higher-level topic (`TopicInfo`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Topic {
    /// Stable identifier.
    pub id: String,
    /// Human-readable label.
    pub description: String,
}

/// Long and short contract labels (`MarketOutcomeLabels`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct OutcomeLabels {
    /// Long (Yes) contract.
    pub long: OutcomeLabel,
    /// Short (No) contract.
    pub short: OutcomeLabel,
}

/// Names for one outcome (`MarketOutcomeLabel`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct OutcomeLabel {
    /// Full name, up to 14 characters.
    pub extended: String,
    /// Compact name, up to 4 characters.
    pub abbreviated: String,
}

/// Outcome colours (`MarketDisplayColors`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DisplayColors {
    /// Colours of the `market_description` outcome.
    pub market: ThemeColors,
    /// Colours of the `reverse_description` outcome.
    #[serde(default)]
    pub reverse: Option<ThemeColors>,
}

/// One `#RRGGBB` colour per theme (`ThemeDisplayColors`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ThemeColors {
    /// Light theme.
    pub light: String,
    /// Dark theme.
    pub dark: String,
}

/// Game-line display metadata (`GamePageDisplayInfo`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct GamePage {
    /// Stable game id, e.g. `nba-bkn-bos-2026-10-27`.
    pub game_id: String,
    /// Game title.
    pub game_description: String,
    /// Section (game line) id, e.g. `moneyline`.
    pub section_id: String,
    /// Section label.
    pub section_description: String,
    /// Section order within the game.
    pub section_display_order: i64,
    /// Row order within the section.
    #[serde(default)]
    pub row_display_order: Option<i64>,
    /// Subject slug for team/player rows.
    #[serde(default)]
    pub subject_id: Option<String>,
    /// Subject label.
    #[serde(default)]
    pub subject_description: Option<String>,
    /// Normalised numeric line, e.g. `-1.5` or `219.5`.
    #[serde(default)]
    pub value: Option<String>,
}

/// Scalar-market bounds (`RangeDisplayAttributes`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RangeDisplay {
    /// Upper bound before scaling.
    #[serde(with = "str_num")]
    pub upper: i16,
    /// Lower bound before scaling.
    #[serde(with = "str_num")]
    pub lower: i16,
    /// Decimal places to shift the bounds by.
    #[serde(with = "str_num")]
    pub scale: u8,
    /// Display prefix, e.g. `$`.
    pub prefix: String,
    /// Display suffix, e.g. `K`.
    pub suffix: String,
}

/// Default UI sort order (`MarketSort`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MarketSort {
    /// By price.
    Price,
    /// By symbol ascending.
    SymbolAsc,
    /// By symbol descending.
    SymbolDesc,
    /// By market description ascending.
    MarketDescriptionAsc,
    /// By market description descending.
    MarketDescriptionDesc,
    /// By volume.
    Volume,
    /// By expected resolution time.
    ExpectedResolutionTime,
}

/// External resolution reference (`ReferenceAttributes`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reference {
    /// A mirrored Polymarket market.
    Polymarket {
        /// Polymarket market slug.
        market_slug: String,
        /// Polymarket condition id.
        condition_id: String,
        /// CLOB token id of this market's side (resolves the market to 1).
        #[serde(default)]
        market_outcome_token_id: Option<String>,
        /// CLOB token id of the other side (resolves the market to 0).
        #[serde(default)]
        reverse_outcome_token_id: Option<String>,
    },
}

/// Typed market category (`MarketType`), tagged by `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MarketType {
    /// A sports fixture market.
    Sports(SportsMarketType),
    /// A company announcing an action before a deadline.
    CompanyAction(CompanyActionMarketType),
}

/// Sports market details. Pascal's per-sport variants share these fields; `period` is set for
/// team sports, `series_format` for esports and `match_format` for tennis.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SportsMarketType {
    /// The sport.
    pub sport: Sport,
    /// Portion of the game the market settles on.
    #[serde(default)]
    pub period: Option<SportPeriod>,
    /// Best-of format (esports).
    #[serde(default)]
    pub series_format: Option<BestOfSeriesFormat>,
    /// Number of sets (tennis).
    #[serde(default)]
    pub match_format: Option<TennisMatchFormat>,
    /// Home and away competitors.
    pub teams: HomeAwayTeams,
    /// The market type within the game.
    pub market: SportsMarket,
}

/// Sport discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Sport {
    /// Soccer.
    Soccer,
    /// Baseball.
    Baseball,
    /// League of Legends.
    LeagueOfLegends,
    /// Counter-Strike.
    CounterStrike,
    /// Dota 2.
    #[serde(rename = "dota_2")]
    Dota2,
    /// Valorant.
    Valorant,
    /// Tennis.
    Tennis,
    /// American football.
    Football,
    /// Basketball.
    Basketball,
    /// Hockey.
    Hockey,
}

/// Portion of a game a market settles on (union of the per-sport period enums).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SportPeriod {
    /// Soccer regular time plus stoppage.
    RegTime,
    /// Soccer first half.
    FirstHalf,
    /// Soccer second half.
    SecondHalf,
    /// Full game including overtime (baseball, football, basketball, hockey).
    FullGame,
    /// Full tennis match.
    FullMatch,
}

/// Best-of series format (esports).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BestOfSeriesFormat {
    /// Bo1.
    BestOfOne,
    /// Bo3.
    BestOfThree,
    /// Bo5.
    BestOfFive,
}

/// Tennis match format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TennisMatchFormat {
    /// Best of three sets.
    BestOfThree,
    /// Best of five sets.
    BestOfFive,
}

/// Home and away competitors (`HomeAwayTeams`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct HomeAwayTeams {
    /// Full home name.
    pub home_name: String,
    /// Short home name.
    #[serde(default)]
    pub home_short_name: Option<String>,
    /// Home subject slug.
    pub home_subject_id: String,
    /// Full away name.
    pub away_name: String,
    /// Short away name.
    #[serde(default)]
    pub away_short_name: Option<String>,
    /// Away subject slug.
    pub away_subject_id: String,
}

/// Market type within a game (union of the per-sport market enums), tagged by `type`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SportsMarket {
    /// Who wins.
    Moneyline {
        /// Home, away or draw.
        outcome: Outcome,
    },
    /// Point spread.
    Spread {
        /// Team the line applies to.
        team: TeamSide,
        /// Line, e.g. `-1.5`.
        line: String,
    },
    /// Baseball total runs.
    Total {
        /// Line.
        line: String,
    },
    /// Total points.
    TotalPoints {
        /// Line.
        line: String,
    },
    /// Total goals.
    TotalGoals {
        /// Line.
        line: String,
    },
    /// Soccer both teams to score.
    BothTeamsToScore {},
    /// Winner of one map (esports).
    MapWinner {
        /// Map number.
        map_number: u32,
        /// Home or away.
        outcome: Outcome,
    },
    /// Map handicap (esports).
    MapHandicap {
        /// Team the line applies to.
        team: TeamSide,
        /// Line.
        line: String,
    },
    /// Total maps (esports).
    TotalMaps {
        /// Line.
        line: String,
    },
    /// Tennis set handicap.
    SetHandicap {
        /// Team the line applies to.
        team: TeamSide,
        /// Line.
        line: String,
    },
    /// Tennis total sets.
    TotalSets {
        /// Line.
        line: String,
    },
    /// Tennis total games.
    TotalGames {
        /// Line.
        line: String,
    },
}

/// Moneyline / map-winner outcome (`ThreeWayOutcome`, a superset of `TeamSide`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Home wins.
    Home,
    /// Away wins.
    Away,
    /// Draw.
    Draw,
}

/// Home or away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamSide {
    /// Home.
    Home,
    /// Away.
    Away,
}

/// Company action market details (`CompanyActionMarketType`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CompanyActionMarketType {
    /// Company display name.
    pub company_name: String,
    /// The action, completing "officially announces …".
    pub action: String,
    /// Announcement cutoff, Unix ms.
    #[serde(with = "str_num")]
    pub announcement_deadline_time_ms: u64,
}

/// Order books keyed by symbol (`GET /api/v1/books`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Books {
    /// Book per requested symbol.
    pub books: BTreeMap<String, OrderBook>,
}

/// An L2 order book (`OrderBookMsg`); also the `book` WebSocket payload, where updates carry
/// changed levels only and a size of 0 removes a level.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct OrderBook {
    /// Asks, lowest price first.
    pub asks: Vec<Level>,
    /// Bids, highest price first.
    pub bids: Vec<Level>,
    /// Market spec, on first fetch or when it changes.
    #[serde(default)]
    pub spec: Option<MarketSpec>,
}

/// One price level, serialised by Pascal as `["0.540000", "15"]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "[String; 2]", into = "[String; 2]")]
pub struct Level {
    /// Price, micro-USD.
    pub price: MicroUsd,
    /// Size, contracts.
    pub size: u64,
}

impl TryFrom<[String; 2]> for Level {
    type Error = String;

    fn try_from([price, size]: [String; 2]) -> Result<Self, Self::Error> {
        Ok(Level {
            price: parse_micros(&price).map_err(|e| e.to_string())?,
            size: size
                .parse()
                .map_err(|e| format!("invalid level size {size:?}: {e}"))?,
        })
    }
}

impl From<Level> for [String; 2] {
    fn from(l: Level) -> Self {
        [format_micros(l.price), l.size.to_string()]
    }
}

/// Specification of a tradeable market (`MarketSpec`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct MarketSpec {
    /// Symbol.
    pub symbol: String,
    /// Base taker fee rate.
    #[serde(with = "str_num")]
    pub taker_fee_rate: Decimal,
    /// Maker rebate share.
    #[serde(with = "str_num")]
    pub maker_rebate_share: Decimal,
    /// Minimum tick, micro-USD.
    #[serde(with = "micros")]
    pub tick_size_min: MicroUsd,
    /// Significant figures of the tick formula.
    pub tick_sig_figs: u8,
}

/// Why a market appeared on the `market_update` channel (`MarketUpdateType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MarketUpdateType {
    /// New listing.
    New,
    /// Display attributes changed.
    DisplayAttributesChange,
    /// Market resolved.
    Resolved,
}

/// One entry of a `market_update` frame.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct MarketUpdate {
    /// Reason for the update.
    pub update_type: MarketUpdateType,
    /// Market state (no `stats`, `best_bid`, `best_ask`).
    pub market: Market,
}
