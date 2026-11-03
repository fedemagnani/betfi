//! Trades, fills and account state: `/trades`, `/accounts/{owner}`, `/fills`, `/transfers`,
//! `/v2/.../position-instances`, and the `all_trades` / `account` WebSocket channels.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::num::{MicroUsd, micros, opt_micros, opt_str_num, str_num};

/// Order side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub enum Side {
    /// Buy.
    #[serde(rename = "BID")]
    Bid,
    /// Sell.
    #[serde(rename = "ASK")]
    Ask,
}

/// Whether a fill provided or removed liquidity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub enum Liquidity {
    /// Removed liquidity.
    #[serde(rename = "TAKER")]
    Taker,
    /// Provided liquidity.
    #[serde(rename = "MAKER")]
    Maker,
}

/// A public trade (`PublicTradeMsg`), from `/api/v1/trades` and the `all_trades` channel.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PublicTrade {
    /// Unique trade id.
    #[serde(with = "str_num")]
    pub trade_id: u64,
    /// Market symbol.
    pub symbol: String,
    /// Side of the aggressive order.
    pub taker_side: Side,
    /// Price, micro-USD.
    #[serde(with = "micros")]
    pub price: MicroUsd,
    /// Size, contracts.
    #[serde(with = "str_num")]
    pub size: u64,
    /// Owner of the resting order.
    pub maker: String,
    /// Owner of the aggressive order.
    pub taker: String,
    /// Maker order's reduce-only flag (absent for archival data).
    #[serde(default)]
    pub maker_reduce_only: Option<bool>,
    /// Taker order's reduce-only flag (absent for archival data).
    #[serde(default)]
    pub taker_reduce_only: Option<bool>,
    /// Signed maker position before the trade.
    #[serde(default, with = "opt_str_num")]
    pub maker_position_size_prev: Option<i64>,
    /// Signed taker position before the trade.
    #[serde(default, with = "opt_str_num")]
    pub taker_position_size_prev: Option<i64>,
    /// Matching-engine sequence of the trade.
    #[serde(with = "str_num")]
    pub seq: u64,
    /// Trade time, Unix ms.
    #[serde(with = "str_num")]
    pub trade_ts_ms: u64,
}

/// Account state (`GET /api/v1/accounts/{owner}` and the `account` WebSocket channel).
///
/// A REST response or a WebSocket snapshot is the full state. A WebSocket update carries only
/// what changed: `orders` / `positions` hold changed entries, the optional totals are present only
/// when they changed, and the delta lists (`fills`, `resolutions`, …) are present when non-empty.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct AccountState {
    /// True after the first credited deposit.
    #[serde(default)]
    pub has_deposited: Option<bool>,
    /// Owner-chosen handle (REST and snapshots only).
    #[serde(default)]
    pub username: Option<String>,
    /// Connected X account (REST and snapshots only).
    #[serde(default)]
    pub twitter_account_connection: Option<TwitterAccountConnection>,
    /// Free collateral, micro-USD.
    #[serde(default, with = "opt_micros")]
    pub collateral_usd: Option<MicroUsd>,
    /// Cumulative reward totals.
    #[serde(default)]
    pub rewards: Option<AccountRewards>,
    /// Lifetime maker / taker totals.
    #[serde(default)]
    pub lifetime_stats: Option<AccountLifetimeStats>,
    /// Open orders (snapshot) or changed orders (update).
    pub orders: Vec<Order>,
    /// Open positions (snapshot) or changed positions (update).
    pub positions: Vec<Position>,
    /// Deposits not yet credited.
    pub pending_deposits: Vec<Deposit>,
    /// Active (or, on updates, just-terminated) withdrawals.
    pub pending_withdrawals: Vec<PendingWithdrawal>,
    /// WebSocket only: incremental order changes with a reason.
    #[serde(default)]
    pub order_updates: Option<Vec<OrderUpdate>>,
    /// WebSocket only: fills since the last message.
    #[serde(default)]
    pub fills: Option<Vec<Fill>>,
    /// WebSocket only: position resolutions since the last message.
    #[serde(default)]
    pub resolutions: Option<Vec<PositionResolution>>,
    /// WebSocket only: completed position instances, newest close first.
    #[serde(default)]
    pub closed_positions: Option<Vec<PositionHistory>>,
    /// WebSocket only: transfers since the last message.
    #[serde(default)]
    pub transfers: Option<Vec<Transfer>>,
}

/// Connected X account (`TwitterAccountConnection`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TwitterAccountConnection {
    /// X's numeric account id.
    #[serde(with = "str_num")]
    pub twitter_user_id: u64,
    /// Handle at the last connect or refresh.
    pub twitter_handle: String,
    /// Sequence of that connect or refresh.
    #[serde(default, with = "opt_str_num")]
    pub connect_seq: Option<u64>,
}

/// Cumulative reward totals (`AccountRewardsMsg`), micro-USD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct AccountRewards {
    /// Referral fee share received.
    #[serde(with = "micros")]
    pub referral_rewards_received_usd: MicroUsd,
    /// Builder fees received.
    #[serde(with = "micros")]
    pub builder_fees_received_usd: MicroUsd,
    /// Campaign rewards received.
    #[serde(with = "micros")]
    pub user_rewards_received_usd: MicroUsd,
}

/// Lifetime maker and taker totals (`AccountLifetimeStatsMsg`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct AccountLifetimeStats {
    /// As maker.
    pub maker: LifetimeStats,
    /// As taker.
    pub taker: LifetimeStats,
}

/// Lifetime totals for one liquidity side (`LifetimeStatsMsg`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LifetimeStats {
    /// Whole contracts traded.
    #[serde(with = "str_num")]
    pub volume_contracts: u64,
    /// Weighted volume in fractional contracts (not USD).
    #[serde(default, with = "opt_str_num")]
    pub weighted_volume: Option<Decimal>,
    /// Σ price × size, micro-USD.
    #[serde(with = "micros")]
    pub volume_notional_usd: MicroUsd,
    /// Venue fees paid (rebates negative), micro-USD.
    #[serde(with = "micros")]
    pub fees_paid_usd: MicroUsd,
}

/// Time in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TimeInForce {
    /// Good 'til cancelled.
    Gtc,
    /// Good 'til time.
    Gtt,
    /// Immediate or cancel.
    Ioc,
}

/// Order display type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum OrderType {
    /// Limit order.
    Limit,
    /// Marketable IOC order with a limit price.
    Market,
}

/// An order (`OrderMsg`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Order {
    /// Exchange order id.
    #[serde(with = "str_num")]
    pub id: u64,
    /// Client order id (a client may pack its own index into the top 16 bits).
    #[serde(with = "str_num")]
    pub client_order_id: u64,
    /// Present when the order replaced another.
    #[serde(default, with = "opt_str_num")]
    pub replace_client_order_id: Option<u64>,
    /// Market symbol.
    pub symbol: String,
    /// Side.
    pub side: Side,
    /// Limit price, micro-USD.
    #[serde(with = "micros")]
    pub price: MicroUsd,
    /// Size at placement, contracts.
    #[serde(with = "str_num")]
    pub size_original: u64,
    /// Remaining open size, contracts.
    #[serde(with = "str_num")]
    pub size_remaining: u64,
    /// Cumulative filled size, contracts.
    #[serde(with = "str_num")]
    pub size_filled: u64,
    /// Σ fill price × fill size, micro-USD.
    #[serde(with = "micros")]
    pub notional_filled: MicroUsd,
    /// Time in force.
    pub tif: TimeInForce,
    /// Display type.
    #[serde(rename = "type")]
    pub order_type: OrderType,
    /// Maker-only.
    pub post_only: bool,
    /// Reduce-only.
    pub reduce_only: bool,
    /// Expiry for GTT orders, Unix ms.
    #[serde(default, with = "opt_str_num")]
    pub expires_ts_ms: Option<u64>,
    /// Builder named by the order.
    #[serde(default)]
    pub builder_pubkey: Option<String>,
    /// Builder fee rate signed into the order.
    #[serde(default, with = "opt_str_num")]
    pub builder_fee_rate: Option<Decimal>,
    /// Sequence at placement.
    #[serde(with = "str_num")]
    pub seq: u64,
    /// Sequence of the last change.
    #[serde(with = "str_num")]
    pub update_seq: u64,
    /// Placement time, Unix ms.
    #[serde(with = "str_num")]
    pub place_ts_ms: u64,
    /// Last change, Unix ms.
    #[serde(with = "str_num")]
    pub update_ts_ms: u64,
}

/// Reason attached to an order update (`OrderUpdateMsg.update_reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OrderUpdateReason {
    /// Cancelled by the user.
    UserCancel,
    /// Mass cancel.
    MassCancelled,
    /// GTT expiry.
    Expired,
    /// Collateral insufficient.
    Undercollateralized,
    /// Self-trade prevention.
    SelfTrade,
    /// IOC remainder.
    IocRemainderCanceled,
    /// Market resolved.
    MarketResolved,
    /// Position overflow.
    PositionOverflow,
    /// Trading key expired.
    TradingKeyExpired,
    /// Trading key revoked.
    TradingKeyRevoked,
    /// Trading key replaced.
    TradingKeyReplaced,
    /// Reduce-only with nothing to reduce.
    NoReduciblePosition,
    /// Reduce-only resized.
    ReduceOnlyResize,
    /// Resting order limit.
    RestingOrderLimit,
    /// Replaced by another order.
    Replaced,
}

/// One incremental order change (`OrderUpdateMsg`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct OrderUpdate {
    /// Order state after the change.
    pub order: Order,
    /// Classification of the change.
    #[serde(default)]
    pub update_reason: Option<OrderUpdateReason>,
}

/// A current position (`PositionMsg`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Position {
    /// Market symbol.
    pub symbol: String,
    /// Signed size, contracts (negative = short).
    #[serde(with = "str_num")]
    pub size: i64,
    /// Contracts added in this lifecycle.
    #[serde(with = "str_num")]
    pub cumulative_increasing_contracts: u64,
    /// USD value of contracts added, micro-USD.
    #[serde(with = "micros")]
    pub cumulative_increasing_notional_usd: MicroUsd,
    /// Contracts reduced in this lifecycle.
    #[serde(with = "str_num")]
    pub cumulative_reducing_contracts: u64,
    /// USD value of contracts reduced, micro-USD.
    #[serde(with = "micros")]
    pub cumulative_reducing_notional_usd: MicroUsd,
    /// Signed fees (rebates negative), micro-USD.
    #[serde(with = "micros")]
    pub cumulative_fees_usd: MicroUsd,
    /// Realized PnL excluding fees, micro-USD.
    #[serde(with = "micros")]
    pub realized_pnl_usd: MicroUsd,
    /// Unrealized PnL at mark excluding fees, micro-USD.
    #[serde(with = "micros")]
    pub unrealized_pnl_usd: MicroUsd,
    /// Current mark, micro-USD.
    #[serde(with = "micros")]
    pub mark_price: MicroUsd,
    /// Average entry (floored), micro-USD; absent when size is zero.
    #[serde(default, with = "opt_micros")]
    pub average_entry_price: Option<MicroUsd>,
    /// Undocumented (seen in the 28 Oct fixtures): entry notional of the open remainder, micro-USD.
    #[serde(default, with = "opt_micros")]
    pub remaining_entry_notional_usd: Option<MicroUsd>,
    /// Sequence when this position opened.
    #[serde(with = "str_num")]
    pub seq: u64,
    /// Sequence of the last change.
    #[serde(with = "str_num")]
    pub update_seq: u64,
    /// Open time, Unix ms.
    #[serde(with = "str_num")]
    pub open_ts_ms: u64,
    /// Last change, Unix ms.
    #[serde(with = "str_num")]
    pub update_ts_ms: u64,
}

/// Lifecycle state of a deposit, withdrawal or transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TransferStatus {
    /// Being verified.
    Verifying,
    /// Being processed on chain.
    Processing,
    /// Rejected.
    Rejected,
    /// Completed.
    Completed,
}

/// A pending deposit (`DepositMsg`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Deposit {
    /// Deposit id.
    #[serde(with = "str_num")]
    pub deposit_id: u64,
    /// Account owner.
    pub owner: String,
    /// Amount, micro-USD.
    #[serde(with = "micros")]
    pub amount: MicroUsd,
    /// Lifecycle state.
    pub transfer_status: TransferStatus,
}

/// A pending withdrawal (`PendingWithdrawalMsg`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PendingWithdrawal {
    /// Account owner.
    pub owner: String,
    /// Lifecycle state.
    pub status: TransferStatus,
    /// Amount, micro-USD.
    #[serde(with = "micros")]
    pub amount: MicroUsd,
    /// Wallet receiving the withdrawal (the withdrawal-sink link of spec 1.3).
    pub destination_authority: String,
    /// Chain sequence, when PROCESSING or COMPLETED.
    #[serde(default, with = "opt_str_num")]
    pub chain_seq: Option<u64>,
}

/// One side of a trade from an account's view (`FillMsg`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Fill {
    /// Trade id.
    #[serde(with = "str_num")]
    pub trade_id: u64,
    /// Market symbol.
    pub symbol: String,
    /// Side of the filled order.
    pub side: Side,
    /// Filled order's reduce-only flag (absent for archival data).
    #[serde(default)]
    pub reduce_only: Option<bool>,
    /// Execution price, micro-USD.
    #[serde(with = "micros")]
    pub fill_price: MicroUsd,
    /// Filled contracts.
    #[serde(with = "str_num")]
    pub fill_size: u64,
    /// Maker or taker.
    pub liquidity: Liquidity,
    /// Owner on the other side.
    pub counterparty: String,
    /// Exchange order id.
    #[serde(with = "str_num")]
    pub order_id: u64,
    /// Client order id.
    #[serde(with = "str_num")]
    pub client_order_id: u64,
    /// Venue fee (rebates negative), micro-USD.
    #[serde(with = "micros")]
    pub fee_usd: MicroUsd,
    /// Collateral change of this side, micro-USD.
    #[serde(with = "micros")]
    pub collateral_change_usd: MicroUsd,
    /// Signed position before the fill.
    #[serde(with = "str_num")]
    pub position_size_prev: i64,
    /// Realized PnL of the fill excluding fees, micro-USD.
    #[serde(with = "micros")]
    pub realized_pnl_usd: MicroUsd,
    /// Opening sequence of the position instance.
    #[serde(default, with = "opt_str_num")]
    pub position_open_seq: Option<u64>,
    /// Opening time of the position instance, Unix ms.
    #[serde(default, with = "opt_str_num")]
    pub position_open_ts_ms: Option<u64>,
    /// Builder named by the order.
    #[serde(default)]
    pub builder_pubkey: Option<String>,
    /// Builder fee paid, micro-USD.
    #[serde(default, with = "opt_micros")]
    pub builder_fee_paid_usd: Option<MicroUsd>,
    /// Sequence of the fill.
    #[serde(with = "str_num")]
    pub seq: u64,
    /// Fill time, Unix ms.
    #[serde(with = "str_num")]
    pub trade_ts_ms: u64,
}

/// Kind of balance movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TransferType {
    /// Deposit.
    Deposit,
    /// Withdrawal.
    Withdrawal,
    /// Campaign reward.
    Reward,
}

/// A transfer ledger entry (`TransferMsg`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Transfer {
    /// Kind.
    #[serde(rename = "type")]
    pub transfer_type: TransferType,
    /// Lifecycle state.
    pub status: TransferStatus,
    /// Signed collateral change, micro-USD.
    #[serde(with = "micros")]
    pub collateral_change_usd: MicroUsd,
    /// Fee paid, micro-USD.
    #[serde(with = "micros")]
    pub fee_usd: MicroUsd,
    /// Net amount (negative for withdrawals), micro-USD.
    #[serde(with = "micros")]
    pub net_usd: MicroUsd,
    /// Sequence of the collateral update.
    #[serde(with = "str_num")]
    pub seq: u64,
    /// Time of the collateral update, Unix ms.
    #[serde(with = "str_num")]
    pub transfer_ts_ms: u64,
    /// Withdrawal chain sequence.
    #[serde(default, with = "opt_str_num")]
    pub chain_seq: Option<u64>,
    /// Reward reason, e.g. `MIDTERMS_2026_TRADING_REWARD`.
    #[serde(default)]
    pub user_reward_reason: Option<String>,
}

/// A position-resolution payout (`PositionResolutionMsg`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PositionResolution {
    /// Market symbol.
    pub symbol: String,
    /// Settlement price, micro-USD.
    #[serde(with = "micros")]
    pub resolution: MicroUsd,
    /// Signed size settled.
    #[serde(with = "str_num")]
    pub size: i64,
    /// Net collateral change, micro-USD.
    #[serde(with = "micros")]
    pub collateral_change_usd: MicroUsd,
    /// Realized PnL excluding fees, micro-USD.
    #[serde(with = "micros")]
    pub realized_pnl_usd: MicroUsd,
    /// Opening sequence of the settled position.
    #[serde(default, with = "opt_str_num")]
    pub position_open_seq: Option<u64>,
    /// Opening time of the settled position, Unix ms.
    #[serde(default, with = "opt_str_num")]
    pub position_open_ts_ms: Option<u64>,
    /// Sequence of the resolution.
    #[serde(with = "str_num")]
    pub seq: u64,
    /// Resolution time, Unix ms.
    #[serde(with = "str_num")]
    pub position_resolution_ts_ms: u64,
}

/// Position direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum PositionSide {
    /// Long.
    Long,
    /// Short.
    Short,
}

/// A closed position instance (`PositionHistoryMsg`), from `/v2/.../position-instances` and the
/// `account` channel's `closed_positions`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PositionHistory {
    /// Market symbol.
    pub symbol: String,
    /// Direction.
    pub position_side: PositionSide,
    /// Number of instances aggregated (`1` for a single instance).
    #[serde(with = "str_num")]
    pub position_instance_count: u64,
    /// Held until market resolution.
    pub held_until_resolution: bool,
    /// Gross realized PnL excluding fees (includes resolution payout), micro-USD.
    #[serde(with = "micros")]
    pub realized_pnl_usd: MicroUsd,
    /// Contracts added over the lifecycle.
    #[serde(with = "str_num")]
    pub cumulative_increasing_contracts: u64,
    /// USD value of contracts added, micro-USD.
    #[serde(with = "micros")]
    pub cumulative_increasing_notional_usd: MicroUsd,
    /// USD value of contracts closed, micro-USD.
    #[serde(with = "micros")]
    pub cumulative_reducing_notional_usd: MicroUsd,
    /// Average increasing price (floored), micro-USD.
    #[serde(with = "micros")]
    pub average_increasing_price: MicroUsd,
    /// Average reducing price (floored), micro-USD.
    #[serde(with = "micros")]
    pub average_reducing_price: MicroUsd,
    /// Opening sequence.
    #[serde(with = "str_num")]
    pub open_seq: u64,
    /// Open time, Unix ms.
    #[serde(with = "str_num")]
    pub open_ts_ms: u64,
    /// Closing sequence.
    #[serde(with = "str_num")]
    pub close_seq: u64,
    /// Close time, Unix ms.
    #[serde(with = "str_num")]
    pub close_ts_ms: u64,
}
