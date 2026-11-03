//! `/history` series and the small account / identity endpoints.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::num::{MicroUsd, format_micros, micros, parse_micros, str_num};

/// `GET /api/v1/time`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct ServerTime {
    /// Read-API wall clock, Unix ms.
    #[serde(with = "str_num")]
    pub server_time_ms: u64,
}

/// `GET /api/v1/accounts/{owner}/history`: all four timeframes in one response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct AccountHistory {
    /// One entry per timeframe.
    pub items: Vec<HistoryTimeseries>,
}

impl AccountHistory {
    /// The series of one timeframe, if present.
    pub fn timeframe(&self, tf: Timeframe) -> Option<&HistorySeries> {
        self.items
            .iter()
            .find(|i| i.timeframe == tf)
            .map(|i| &i.data)
    }
}

/// History lookback window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Timeframe {
    /// 24 h, 30-minute points.
    Day,
    /// 7 days, 4-hour points.
    Week,
    /// 30 days, 12-hour points.
    Month,
    /// All history, 24-hour points.
    AllTime,
}

impl Timeframe {
    /// All timeframes, shortest first.
    pub const ALL: [Timeframe; 4] = [
        Timeframe::Day,
        Timeframe::Week,
        Timeframe::Month,
        Timeframe::AllTime,
    ];

    /// The wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Timeframe::Day => "day",
            Timeframe::Week => "week",
            Timeframe::Month => "month",
            Timeframe::AllTime => "all_time",
        }
    }
}

/// One timeframe's series (`AccountHistoryTimeseries`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct HistoryTimeseries {
    /// Timeframe.
    pub timeframe: Timeframe,
    /// Chart lines.
    pub data: HistorySeries,
}

/// Account history chart lines (`AccountHistoryTimeseriesData`). Every value is micro-USD and
/// cumulative since account inception; the last point is the latest completed 30-minute
/// boundary, never a live value.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct HistorySeries {
    /// Free collateral plus marked positions.
    pub account_value_history: Vec<HistoryPoint>,
    /// Account value minus net deposits (includes rewards).
    pub total_pnl_history: Vec<HistoryPoint>,
    /// Trading PnL after fees, excluding rewards: the weekly-points input.
    pub trading_pnl_history: Vec<HistoryPoint>,
    /// Referral fee share received.
    pub referral_rewards_received_history: Vec<HistoryPoint>,
    /// Builder fees received.
    pub builder_fees_received_history: Vec<HistoryPoint>,
    /// Campaign rewards received.
    pub user_rewards_received_history: Vec<HistoryPoint>,
    /// Undocumented (seen in the 28 Oct fixtures, equal to `total_pnl_history` there).
    #[serde(default)]
    pub pnl_history: Option<Vec<HistoryPoint>>,
}

/// One chart point, serialised by Pascal as `["1793059200000", "-6381.119333"]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "[String; 2]", into = "[String; 2]")]
pub struct HistoryPoint {
    /// Point time, Unix ms.
    pub ts_ms: u64,
    /// Value, micro-USD.
    pub value: MicroUsd,
}

impl TryFrom<[String; 2]> for HistoryPoint {
    type Error = String;

    fn try_from([ts, value]: [String; 2]) -> Result<Self, Self::Error> {
        Ok(HistoryPoint {
            ts_ms: ts
                .parse()
                .map_err(|e| format!("invalid timestamp {ts:?}: {e}"))?,
            value: parse_micros(&value).map_err(|e| e.to_string())?,
        })
    }
}

impl From<HistoryPoint> for [String; 2] {
    fn from(p: HistoryPoint) -> Self {
        [p.ts_ms.to_string(), format_micros(p.value)]
    }
}

/// `GET /api/v1/usernames`: owners without a username are omitted.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Usernames {
    /// Username by owner.
    pub usernames: BTreeMap<String, String>,
}

/// One active trading key (`TradingKeyMsg`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TradingKey {
    /// Ed25519 public key.
    pub trading_key: String,
    /// Public label.
    pub name: String,
    /// Expiry, Unix ms.
    #[serde(with = "str_num")]
    pub expiration_ts_ms: u64,
    /// Open orders placed with this key.
    #[serde(with = "str_num")]
    pub open_order_count: u64,
}

/// `GET /api/v1/accounts/{owner}/builder-stats` (`AccountBuilderStatsMsg`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct BuilderStats {
    /// Contracts across attributed fills.
    #[serde(with = "str_num")]
    pub builder_volume_contracts: u128,
    /// Σ price × size, micro-USD.
    #[serde(with = "micros")]
    pub builder_volume_notional_usd: MicroUsd,
    /// Σ size × price × (1 − price), micro-USD.
    #[serde(with = "micros")]
    pub builder_fee_base_usd: MicroUsd,
    /// Builder fees received, micro-USD.
    #[serde(with = "micros")]
    pub builder_fees_received_usd: MicroUsd,
}

/// `GET /api/v1/accounts/{owner}/bio` (`AccountBioMsg`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct AccountBio {
    /// The bio, `None` when unset (served as `null`).
    #[serde(default)]
    pub account_bio: Option<String>,
}

/// `GET /api/v1/accounts/{owner}/deposit-address` (`DepositAddressMsg`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DepositAddress {
    /// Account owner.
    pub owner: String,
    /// Deposit address PDA, once registered.
    #[serde(default)]
    pub deposit_address: Option<String>,
    /// Whether registered.
    pub registered: bool,
}
