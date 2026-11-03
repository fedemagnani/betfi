//! Outcome sentiment (L5-4, step 5.0c): what people think each game's result will be, per team,
//! from four plainly labelled signals.
//!
//! Pascal's open interest cannot say it: `long_interest == short_interest` on every market (688
//! of 688 on 28 Oct), so it measures stake, not direction. Every signal here is directional:
//!
//! | Field | Source | Per team |
//! | --- | --- | --- |
//! | `pascal_holders` | `GET /api/v1/trades` for the card's symbols, followed incrementally ([`follow`]); each account's position rebuilt from `*_position_size_prev` plus the trade ([`tape`]) | accounts holding it and their stake, contracts × that team's mid (micro-USD); market makers and the desk's own hedger left out (D-251) |
//! | `flow_24h` | `/markets` `stats.last_24h` (the catalogue's stored row, so no extra `/markets` call) | what takers paid to back each team: buys (toward the long team) at their notional, sells (toward the other team) at `volume − notional` (D-252) |
//! | `schedina` | the desk DB | Σ stake per pick over pending and open tickets (rS once minted), and the number of tickets |
//! | `price` | the quoter's published books, else the catalogue's bid/ask or mark | each team's mid, the reference line |
//! | `traded_all_time` (6.0c) | `/markets` `stats.all_time`, same stored row | as `flow_24h` over the market's life, plus `total_u = away_u + home_u` |
//! | `traders` (7.0c) | the tape and the desk DB | non-maker Pascal holders of either team plus distinct Schedina players with a (not refunded) ticket on the game |
//! | `recent` (7.0c) | the tape and the desk DB | the [`MAX_RECENT`] newest stakes on the game's winner symbol: Schedina tickets (`source: "schedina"`, S or rS, the leg's pick, opened time) and Pascal taker trades with makers and the desk left out (`source: "pascal"`, what the taker paid, its team, trade time) |
//!
//! Long is the symbol's long team (`long_team`, the away team on every NBA moneyline, D-021): a
//! long position, or a taker buy, backs that team; a short, or a taker sell, backs the other.
//!
//! `GET /sentiment?symbols=A,B` answers the named games (default: every game in the catalogue,
//! by start time). `pascal_holders.thin` is set while fewer than [`MIN_HOLDERS`] people hold
//! either team, or the history is still being read; the app then says "too few traders yet".
//!
//! The task follows the trades every `sentiment_period()` (15 s) with its own read client: one
//! call per second at most on the trades route (burst 3) inside the D-004 per-host bucket, at
//! most [`follow::PAGE_BUDGET`] pages of [`follow::PAGE_LIMIT`] per pass (D-250). Locks: the
//! tape's lock is taken alone and never across an `.await` (D-043).

pub mod follow;
pub mod tape;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use pascal_rs::ratelimit::{BucketConfig, RateLimits};
use pascal_rs::{MarketStats, MarketStatsSnapshot};
use serde::{Deserialize, Serialize};

use crate::api::ApiError;
use crate::api::types::TeamSide;
use crate::catalogue::CatalogueMarket;
use crate::config::Config;
use crate::now_ms;
use crate::state::{AppState, Locked, Symbol};
use follow::{Follower, TradeSource};
use tape::{Holders, MakerRule, TradeTape};

/// Fewer people than this holding either team: too few traders yet (L5-4).
pub const MIN_HOLDERS: u64 = 3;
/// The trades route, as the rate limiter keys it.
pub const TRADES_ROUTE: &str = "/api/v1/trades";
/// The trades route's own bucket in the desk's client (D-250).
pub const TRADES_ROUTE_LIMIT: BucketConfig = BucketConfig {
    per_second: 1.0,
    burst: 3.0,
};
/// 1.0 in micro-units.
const ONE_U: u64 = 1_000_000;
/// Most `recent` stakes per game (L7-1).
pub const MAX_RECENT: usize = 10;

// --- JSON (L5-4) --------------------------------------------------------------------------------

/// `GET /sentiment`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentimentResponse {
    /// Desk time of the answer, Unix ms.
    pub as_of_ms: u64,
    /// [`MIN_HOLDERS`] (additive).
    #[serde(default)]
    pub min_holders: u64,
    /// One per game.
    pub games: Vec<GameSentiment>,
}

/// One game's signals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameSentiment {
    /// Game id.
    pub game_id: String,
    /// Its moneyline symbol.
    pub symbol: String,
    /// Each team's mid, micro-USD (0 = no price).
    pub price: AwayHome,
    /// Pascal holders per team.
    pub pascal_holders: PascalHolders,
    /// Last 24 h of takers per team, micro-USD.
    pub flow_24h: AwayHome,
    /// Schedina players' stake per pick.
    pub schedina: SchedinaStake,
    /// All-time takers per team on the winner market, micro-USD (l6-contracts L6-2, 6.0c).
    #[serde(default)]
    pub traded_all_time: Traded,
    /// Non-maker Pascal holders plus distinct Schedina players on the game (l7-contracts L7-1).
    #[serde(default)]
    pub traders: u64,
    /// The newest stakes on the game, newest first, at most [`MAX_RECENT`] (L7-1).
    #[serde(default)]
    pub recent: Vec<RecentStake>,
}

/// One recent stake on a game (L7-1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentStake {
    /// Micro-USD (Pascal) or micro-USDC (Schedina).
    pub amount_u: u64,
    /// The team it backs: `away` or `home` (the leg's pick on a Schedina ticket).
    pub team: String,
    /// Unix ms.
    pub ts_ms: u64,
    /// `pascal` or `schedina`.
    pub source: String,
}

/// `pascal` and `schedina` stakes merged: newest first (then Schedina first, then by amount, so
/// the order is total), at most [`MAX_RECENT`].
pub fn merge_recent(mut all: Vec<RecentStake>) -> Vec<RecentStake> {
    all.sort_by(|a, b| {
        (b.ts_ms, &b.source, b.amount_u, &b.team).cmp(&(a.ts_ms, &a.source, a.amount_u, &a.team))
    });
    all.truncate(MAX_RECENT);
    all
}

/// The Pascal side of `recent` for a market long on `long_team`.
pub fn pascal_recent(long_team: TeamSide, trades: &[tape::TapeTrade]) -> Vec<RecentStake> {
    trades
        .iter()
        .map(|t| {
            let team = if t.taker_long {
                long_team
            } else {
                long_team.other()
            };
            RecentStake {
                amount_u: t.amount_u(),
                team: match team {
                    TeamSide::Away => "away".to_owned(),
                    TeamSide::Home => "home".to_owned(),
                },
                ts_ms: t.ts_ms,
                source: "pascal".to_owned(),
            }
        })
        .collect()
}

/// What takers paid toward each team over a window, and the total (6.0c).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Traded {
    /// Toward the away team.
    pub away_u: u64,
    /// Toward the home team.
    pub home_u: u64,
    /// `away_u + home_u`: the money traded on the market.
    pub total_u: u64,
}

/// `stats.all_time` (or any window) per team, mapped exactly as `flow_24h` ([`flow_u`], D-252).
pub fn traded_u(long_team: TeamSide, s: &MarketStatsSnapshot) -> Traded {
    let (long, short) = flow_u(s);
    let (away_u, home_u) = by_team(long_team, long, short);
    Traded {
        away_u,
        home_u,
        total_u: away_u.saturating_add(home_u),
    }
}

/// A micro-unit amount per team.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AwayHome {
    /// Away team.
    pub away_u: u64,
    /// Home team.
    pub home_u: u64,
}

/// People holding one team on Pascal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamHolders {
    /// Accounts.
    pub accounts: u64,
    /// Contracts × the team's mid, micro-USD.
    pub stake_u: u64,
}

/// Pascal holders per team.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PascalHolders {
    /// Away team.
    pub away: TeamHolders,
    /// Home team.
    pub home: TeamHolders,
    /// Market makers left out.
    pub excluded_makers: u64,
    /// Trades read for this market.
    pub trades_seen: u64,
    /// The history has been read completely (additive).
    #[serde(default)]
    pub synced: bool,
    /// Too few holders (or history still loading): show "too few traders yet" (additive).
    #[serde(default)]
    pub thin: bool,
}

/// Schedina players per pick.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedinaStake {
    /// Σ stake on the away pick, micro-USDC.
    pub away_u: u64,
    /// Σ stake on the home pick.
    pub home_u: u64,
    /// Pending and open tickets with a leg on the game.
    pub tickets: u64,
}

// --- shared state -------------------------------------------------------------------------------

/// The sentiment service's state, in [`AppState::sentiment`](crate::state::Shared::sentiment).
#[derive(Debug, Default)]
pub struct Sentiment {
    /// Every followed market's trades.
    pub tape: Locked<TradeTape>,
    /// Desk time of the last completed pass (0 = none yet).
    pub last_pass_ms: AtomicU64,
    /// Market-maker rule.
    pub rule: MakerRule,
}

// --- pure computation ---------------------------------------------------------------------------

/// The long side's mid from best bid and ask (micro-USD), or `None`.
pub fn mid_u(bid: Option<i64>, ask: Option<i64>) -> Option<u64> {
    let ok = |p: i64| (p > 0 && p < ONE_U as i64).then_some(p.unsigned_abs());
    match (bid.and_then(ok), ask.and_then(ok)) {
        (Some(b), Some(a)) => Some((b + a) / 2),
        _ => None,
    }
}

/// The long side's mid for `m`: from `book` (best bid, best ask) when both sides are there,
/// else the catalogue's bid/ask, else its mark.
pub fn long_mid_u(m: &CatalogueMarket, book: Option<(Option<i64>, Option<i64>)>) -> Option<u64> {
    book.and_then(|(b, a)| mid_u(b, a))
        .or_else(|| mid_u(m.best_bid_u, m.best_ask_u))
        .or_else(|| {
            m.mark_price_u
                .filter(|p| *p > 0 && *p < ONE_U as i64)
                .map(i64::unsigned_abs)
        })
}

/// What takers paid toward each side in a stats window: `(long_u, short_u)` (D-252). A taker
/// buy backs the long team at its notional; a taker sell backs the other team at what the short
/// side costs, `volume × 1 − notional`.
pub fn flow_u(s: &MarketStatsSnapshot) -> (u64, u64) {
    let long = s.taker_buy_volume_notional.max(0).unsigned_abs();
    let sell_notional = s.taker_sell_volume_notional.max(0).unsigned_abs();
    let short = s
        .taker_sell_volume
        .saturating_mul(ONE_U)
        .saturating_sub(sell_notional);
    (long, short)
}

/// Splits a `(long, short)` pair into `(away, home)` by the market's long team.
fn by_team<T>(long_team: TeamSide, long: T, short: T) -> (T, T) {
    match long_team {
        TeamSide::Away => (long, short),
        TeamSide::Home => (short, long),
    }
}

/// One game's signals from its parts.
pub fn game_sentiment(
    m: &CatalogueMarket,
    long_mid: Option<u64>,
    holders: &Holders,
    last_24h: Option<&MarketStatsSnapshot>,
    schedina: SchedinaStake,
) -> GameSentiment {
    let long_p = long_mid.unwrap_or(0);
    let short_p = if long_p == 0 { 0 } else { ONE_U - long_p };
    let (away_p, home_p) = by_team(m.long_team, long_p, short_p);
    let side = |h: tape::SideHolders, p: u64| TeamHolders {
        accounts: h.accounts,
        stake_u: h.contracts.saturating_mul(p),
    };
    let (away, home) = by_team(
        m.long_team,
        side(holders.long, long_p),
        side(holders.short, short_p),
    );
    let (flow_long, flow_short) = last_24h.map(flow_u).unwrap_or_default();
    let (flow_away, flow_home) = by_team(m.long_team, flow_long, flow_short);
    let people = holders.long.accounts + holders.short.accounts;
    GameSentiment {
        game_id: m.game_id.clone(),
        symbol: m.symbol.clone(),
        price: AwayHome {
            away_u: away_p,
            home_u: home_p,
        },
        pascal_holders: PascalHolders {
            away,
            home,
            excluded_makers: holders.excluded_makers,
            trades_seen: holders.trades_seen,
            synced: holders.synced,
            thin: !holders.synced || people < MIN_HOLDERS,
        },
        flow_24h: AwayHome {
            away_u: flow_away,
            home_u: flow_home,
        },
        schedina,
        traded_all_time: Traded::default(),
        traders: people,
        recent: Vec::new(),
    }
}

// --- service ------------------------------------------------------------------------------------

/// Mounts `GET /sentiment`.
pub fn mount(router: Router<AppState>, _state: &AppState) -> Router<AppState> {
    router.route("/sentiment", get(sentiment))
}

/// The desk's trades client: `PASCAL_DATA_URL` with the D-004 per-host bucket and the trades
/// route's own ([`TRADES_ROUTE_LIMIT`]).
pub fn trades_client(config: &Config) -> pascal_rs::Result<pascal_rs::Client> {
    let mut limits = RateLimits::default();
    limits
        .per_route
        .insert(TRADES_ROUTE.to_owned(), TRADES_ROUTE_LIMIT);
    pascal_rs::Client::builder()
        .base_url(&config.pascal_data_url)
        .rate_limits(limits)
        .user_agent(concat!("schedina-desk/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// Follows the trades of the catalogue's symbols every `sentiment_period()` until shutdown
/// (`DESK_SENTIMENT_S=0`: idle).
pub async fn run(state: AppState) {
    if state.config.sentiment_every.is_zero() {
        tracing::info!("sentiment off (DESK_SENTIMENT_S=0)");
        state.shutdown_requested().await;
        return;
    }
    match trades_client(&state.config) {
        Ok(c) => run_with(state, Arc::new(c)).await,
        Err(e) => {
            tracing::error!(error = %e, "sentiment: cannot build the trades client; idle");
            state.shutdown_requested().await;
        }
    }
}

/// [`run`] with any [`TradeSource`].
pub async fn run_with(state: AppState, src: Arc<dyn TradeSource>) {
    let mut follower = Follower::new();
    // The first pass as soon as the catalogue has loaded.
    while state.catalogue().is_none() {
        tokio::select! {
            _ = state.shutdown_requested() => return,
            _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {}
        }
    }
    let mut tick = tokio::time::interval(state.config.sentiment_period());
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = state.shutdown_requested() => break,
            _ = tick.tick() => {}
        }
        let symbols: Vec<Symbol> = match state.catalogue() {
            Some(c) => c.games.values().cloned().collect(),
            None => continue,
        };
        if symbols.is_empty() {
            continue;
        }
        // Games that left the catalogue are forgotten (tape and marks together).
        follower.retain(&symbols);
        state.sentiment.tape.write().retain(&symbols);
        tokio::select! {
            _ = state.shutdown_requested() => break,
            r = follower.pass(src.as_ref(), &symbols, &state.sentiment.tape) => match r {
                Ok(s) => {
                    state.sentiment.last_pass_ms.store(now_ms(), Ordering::Relaxed);
                    metrics::counter!("desk_sentiment_pass_total", "result" => "ok").increment(1);
                    metrics::counter!("desk_sentiment_trades_total").increment(s.new_trades);
                    tracing::debug!(pages = s.pages, new = s.new_trades, backfilling = s.backfilling,
                        symbols = symbols.len(), "sentiment pass");
                }
                Err(e) => {
                    metrics::counter!("desk_sentiment_pass_total", "result" => "error").increment(1);
                    tracing::warn!(error = %e, "sentiment pass failed; keeping what was read");
                }
            },
        }
    }
    tracing::info!("sentiment stopped");
}

/// `GET /sentiment?symbols=A,B`.
pub async fn sentiment(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Json<SentimentResponse>, ApiError> {
    let Some(cat) = state.catalogue() else {
        return Err(ApiError::unavailable("catalogue not loaded yet"));
    };
    let wanted: Option<Vec<&str>> = q
        .get("symbols")
        .map(|s| s.split(',').filter(|s| !s.is_empty()).collect());
    if wanted
        .as_ref()
        .is_some_and(|w| w.len() > pascal_rs::rest::MAX_TRADES_SYMBOLS)
    {
        return Err(ApiError::rejected(format!(
            "at most {} symbols",
            pascal_rs::rest::MAX_TRADES_SYMBOLS
        )));
    }
    // The game's one moneyline per symbol asked (unknown symbols are left out). A spread or
    // total symbol (6.0b) answers for its game's winner market: sentiment is per game winner.
    let mut markets: Vec<&CatalogueMarket> = match &wanted {
        Some(w) => w
            .iter()
            .filter_map(|s| cat.market(s))
            .filter_map(|m| cat.games.get(&m.game_id))
            .filter_map(|winner| cat.market(winner))
            .collect(),
        None => cat.games.values().filter_map(|s| cat.market(s)).collect(),
    };
    markets.sort_by(|a, b| (a.start_ms, &a.game_id).cmp(&(b.start_ms, &b.game_id)));
    markets.dedup_by(|a, b| a.symbol == b.symbol);
    let symbols: Vec<String> = markets.iter().map(|m| m.symbol.clone()).collect();

    // One lock at a time, each copied out before the next (D-043).
    let books: HashMap<String, (Option<i64>, Option<i64>)> = {
        let b = state.books.read();
        symbols
            .iter()
            .filter_map(|s| {
                let book = b.get(s)?;
                Some((
                    s.clone(),
                    (
                        book.bids.first().map(|l| l.price),
                        book.asks.first().map(|l| l.price),
                    ),
                ))
            })
            .collect()
    };
    let own: HashSet<String> = state.config.hedger_owner.iter().cloned().collect();
    type TapeView = (Holders, Vec<tape::TapeTrade>);
    let tape_view: HashMap<String, TapeView> = {
        let t = state.sentiment.tape.read();
        let rule = state.sentiment.rule;
        let card = t.card_makers(&rule);
        symbols
            .iter()
            .map(|s| {
                (
                    s.clone(),
                    (
                        t.holders(s, &rule, &card, &own),
                        t.recent(s, &rule, &card, &own, MAX_RECENT),
                    ),
                )
            })
            .collect()
    };
    let stats = market_stats(&state, &symbols).await?;
    let players = schedina_stakes(&state, &symbols).await?;
    let mut activity = schedina_activity(&state, &cat, &markets).await?;

    let games = markets
        .iter()
        .map(|m| {
            let st = stats.get(&m.symbol);
            let (holders, trades) = tape_view.get(&m.symbol).cloned().unwrap_or_default();
            let mut g = game_sentiment(
                m,
                long_mid_u(m, books.get(&m.symbol).copied()),
                &holders,
                st.map(|s| &s.last_24h),
                players.get(&m.symbol).copied().unwrap_or_default(),
            );
            if let Some(s) = st {
                g.traded_all_time = traded_u(m.long_team, &s.all_time);
            }
            let (people, mut recent) = activity.remove(&m.symbol).unwrap_or_default();
            g.traders = g.traders.saturating_add(people);
            recent.extend(pascal_recent(m.long_team, &trades));
            g.recent = merge_recent(recent);
            g
        })
        .collect();
    Ok(Json(SentimentResponse {
        as_of_ms: now_ms(),
        min_holders: MIN_HOLDERS,
        games,
    }))
}

/// `stats` (`last_24h` and `all_time`) of `symbols` from the catalogue's stored `/markets` rows.
async fn market_stats(
    state: &AppState,
    symbols: &[String],
) -> Result<HashMap<String, MarketStats>, sqlx::Error> {
    let mut out = HashMap::new();
    if symbols.is_empty() {
        return Ok(out);
    }
    let marks = vec!["?"; symbols.len()].join(", ");
    let sql = format!("SELECT symbol, raw FROM markets WHERE symbol IN ({marks})");
    let mut q = sqlx::query_as::<_, (String, Option<String>)>(sqlx::AssertSqlSafe(sql));
    for s in symbols {
        q = q.bind(s);
    }
    for (symbol, raw) in q.fetch_all(&state.db).await? {
        let stats = raw
            .and_then(|r| serde_json::from_str::<serde_json::Value>(&r).ok())
            .and_then(|v| serde_json::from_value::<MarketStats>(v["stats"].clone()).ok());
        if let Some(s) = stats {
            out.insert(symbol, s);
        }
    }
    Ok(out)
}

/// Per winner market of `markets`: the distinct Schedina players with a ticket (not refunded) on
/// any of its game's symbols (winner, spreads, totals), and the newest [`MAX_RECENT`] Schedina
/// stakes on the winner symbol (S while pending, rS once minted; the leg's pick; the opening
/// time, else the mint time).
async fn schedina_activity(
    state: &AppState,
    cat: &crate::catalogue::Catalogue,
    markets: &[&CatalogueMarket],
) -> Result<HashMap<String, (u64, Vec<RecentStake>)>, sqlx::Error> {
    let mut out = HashMap::new();
    for m in markets {
        let mut game: Vec<&str> = vec![m.symbol.as_str()];
        game.extend(cat.lines_of(&m.game_id).map(|l| l.symbol.as_str()));
        let marks = vec!["?"; game.len()].join(", ");
        let sql = format!(
            "SELECT COUNT(DISTINCT t.user) FROM ticket_legs l JOIN tickets t ON t.ticket = l.ticket
              WHERE l.symbol IN ({marks}) AND t.state != 'refunded'"
        );
        let mut q = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql));
        for s in &game {
            q = q.bind(*s);
        }
        let players = q.fetch_one(&state.db).await?;
        let rows: Vec<(i64, String, i64)> = sqlx::query_as(
            "SELECT CASE WHEN t.state = 'pending' THEN t.stake_u
                         ELSE COALESCE(t.rs_u, t.stake_u) END,
                    l.pick,
                    COALESCE(t.opened_at, t.minted_at, 0) * 1000 AS ts
               FROM ticket_legs l JOIN tickets t ON t.ticket = l.ticket
              WHERE l.symbol = ? AND t.state != 'refunded'
              ORDER BY ts DESC, t.ticket DESC LIMIT ?",
        )
        .bind(&m.symbol)
        .bind(i64::try_from(MAX_RECENT).unwrap_or(10))
        .fetch_all(&state.db)
        .await?;
        let recent = rows
            .into_iter()
            .map(|(amount, team, ts)| RecentStake {
                amount_u: u64::try_from(amount).unwrap_or(0),
                team,
                ts_ms: u64::try_from(ts).unwrap_or(0),
                source: "schedina".to_owned(),
            })
            .collect();
        out.insert(
            m.symbol.clone(),
            (u64::try_from(players).unwrap_or(0), recent),
        );
    }
    Ok(out)
}

/// Σ stake per pick of pending and open tickets with an open leg on each of `symbols`: the
/// escrowed S while pending, rS once minted.
async fn schedina_stakes(
    state: &AppState,
    symbols: &[String],
) -> Result<HashMap<String, SchedinaStake>, sqlx::Error> {
    let rows: Vec<(String, String, i64, i64)> = sqlx::query_as(
        "SELECT l.symbol, l.pick,
                COALESCE(SUM(CASE WHEN t.state = 'open' THEN COALESCE(t.rs_u, t.stake_u)
                                  ELSE t.stake_u END), 0),
                COUNT(DISTINCT t.ticket)
           FROM ticket_legs l JOIN tickets t ON t.ticket = l.ticket
          WHERE t.state IN ('pending', 'open') AND l.status = 'open'
          GROUP BY l.symbol, l.pick",
    )
    .fetch_all(&state.db)
    .await?;
    let mut out: BTreeMap<String, SchedinaStake> = BTreeMap::new();
    for (symbol, pick, stake, tickets) in rows {
        if !symbols.contains(&symbol) {
            continue;
        }
        let e = out.entry(symbol).or_default();
        let stake = u64::try_from(stake).unwrap_or(0);
        match pick.as_str() {
            "away" => e.away_u += stake,
            _ => e.home_u += stake,
        }
        // One leg per game per ticket (D-008), so the two picks never share a ticket.
        e.tickets += u64::try_from(tickets).unwrap_or(0);
    }
    Ok(out.into_iter().collect())
}
