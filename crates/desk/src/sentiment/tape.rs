//! Who holds what on each followed market, rebuilt from Pascal's public trade log (L5-4).
//!
//! Every [`PublicTrade`] carries both accounts' signed position *before* it
//! (`maker_position_size_prev`, `taker_position_size_prev`). An account's position now is
//! therefore its latest trade's `prev` plus that trade's signed size: the latest trade wins, in
//! whatever order trades arrive, and no account needs its full history. Trades are counted once
//! (by `trade_id`), so pages that overlap or arrive out of order change nothing.
//!
//! Signs: a taker `BID` buys the symbol (long), a taker `ASK` sells it (short); the maker takes
//! the other side. Long is the symbol's long team (the away team on NBA moneylines, D-021).
//!
//! Market makers are left out of the holders ([`MakerRule`], D-251): their inventory says
//! nothing about which team people back.
//!
//! 7.0c (L7-1): each market also keeps its [`RECENT_KEEP`] newest trades ([`TapeTrade`], by
//! `(seq, trade_id)`, so the set is the same whatever order pages arrive in), for the per-game
//! "recent stakes" list ([`TradeTape::recent`]: takers only, market makers and the desk left
//! out).

use std::collections::{HashMap, HashSet};

use pascal_rs::{PublicTrade, Side};

use crate::state::Symbol;

/// When an account counts as a market maker (D-251). Shares are whole percent; each share needs
/// a minimum number of trades behind it, so one resting order that got filled does not make a
/// person a market maker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MakerRule {
    /// Maker in at least this share of its own trades on the market…
    pub own_pct: u64,
    /// …with at least this many trades there.
    pub own_min: u64,
    /// Or maker in at least this share of all the market's trades…
    pub market_pct: u64,
    /// …with the market having at least this many trades.
    pub market_min: u64,
    /// Or maker in at least this share of its trades over every followed market…
    pub card_pct: u64,
    /// …with at least this many trades there.
    pub card_min: u64,
}

impl Default for MakerRule {
    /// L5-4's 80% / 50% shares with minimum samples of 3 trades, plus the card-wide 80% of at
    /// least 5 trades (D-251).
    fn default() -> Self {
        MakerRule {
            own_pct: 80,
            own_min: 3,
            market_pct: 50,
            market_min: 3,
            card_pct: 80,
            card_min: 5,
        }
    }
}

/// One account on one market.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountTape {
    /// Signed contracts after the latest trade seen (long > 0).
    pub position: i64,
    /// `(seq, trade_id)` of that trade.
    pub at: (u64, u64),
    /// Trades seen on this market.
    pub trades: u64,
    /// Of which as the resting order.
    pub maker: u64,
    /// The latest trade carried no `prev` for this account (archival data, or the mock's
    /// synthetic tape liquidity): its position is unknown and it is never counted as a holder.
    pub unknown: bool,
}

/// Newest trades kept per market for [`TradeTape::recent`].
pub const RECENT_KEEP: usize = 32;

/// One trade as the recent list needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeTrade {
    /// Matching-engine sequence.
    pub seq: u64,
    /// Trade id.
    pub trade_id: u64,
    /// Trade time, Unix ms.
    pub ts_ms: u64,
    /// The taker (whose stake it is).
    pub taker: String,
    /// The taker bought the symbol (backs the long team); else it sold (backs the other).
    pub taker_long: bool,
    /// Price, micro-USD.
    pub price_u: u64,
    /// Contracts.
    pub size: u64,
}

impl TapeTrade {
    /// What the taker paid for its side, micro-USD: `size × price` for a buy, `size × (1 −
    /// price)` for a sell (D-252's mapping, per trade).
    pub fn amount_u(&self) -> u64 {
        let p = if self.taker_long {
            self.price_u
        } else {
            1_000_000u64.saturating_sub(self.price_u)
        };
        self.size.saturating_mul(p)
    }
}

/// One market's trades as seen so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarketTape {
    /// `trade_id`s seen.
    seen: HashSet<u64>,
    /// Accounts by owner.
    pub accounts: HashMap<String, AccountTape>,
    /// Highest `seq` seen.
    pub max_seq: u64,
    /// Whether the follower has read this market's history down to its first trade, or down to
    /// the last pass (false while a backfill is still paging).
    pub synced: bool,
    /// The [`RECENT_KEEP`] newest trades (self-trades left out), oldest first.
    pub recent: Vec<TapeTrade>,
}

impl MarketTape {
    /// Trades seen.
    pub fn trades(&self) -> u64 {
        self.seen.len() as u64
    }
}

/// Holders of one side.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SideHolders {
    /// Accounts holding it.
    pub accounts: u64,
    /// Contracts they hold.
    pub contracts: u64,
}

/// A market's holders, makers left out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Holders {
    /// Long the symbol.
    pub long: SideHolders,
    /// Short the symbol.
    pub short: SideHolders,
    /// Market makers left out (holding or not).
    pub excluded_makers: u64,
    /// Trades seen on the market.
    pub trades_seen: u64,
    /// History read completely.
    pub synced: bool,
}

/// Every followed market's trades. Lives behind [`super::Sentiment::tape`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TradeTape {
    /// Markets by symbol.
    pub markets: HashMap<Symbol, MarketTape>,
}

impl TradeTape {
    /// Adds a trade. Returns false when it was already seen.
    pub fn ingest(&mut self, t: &PublicTrade) -> bool {
        let m = self.markets.entry(t.symbol.clone()).or_default();
        if !m.seen.insert(t.trade_id) {
            return false;
        }
        m.max_seq = m.max_seq.max(t.seq);
        let size = i64::try_from(t.size).unwrap_or(i64::MAX);
        let taker_delta = match t.taker_side {
            Side::Bid => size,
            Side::Ask => -size,
        };
        let at = (t.seq, t.trade_id);
        if t.maker != t.taker {
            let pos = m.recent.partition_point(|r| (r.seq, r.trade_id) < at);
            m.recent.insert(
                pos,
                TapeTrade {
                    seq: t.seq,
                    trade_id: t.trade_id,
                    ts_ms: t.trade_ts_ms,
                    taker: t.taker.clone(),
                    taker_long: t.taker_side == Side::Bid,
                    price_u: u64::try_from(t.price).unwrap_or(0),
                    size: t.size,
                },
            );
            if m.recent.len() > RECENT_KEEP {
                m.recent.remove(0);
            }
        }
        if t.maker == t.taker {
            // A self-trade moves nothing; count it once.
            let a = m.accounts.entry(t.maker.clone()).or_default();
            a.trades += 1;
            a.maker += 1;
            return true;
        }
        for (owner, prev, delta, is_maker) in [
            (&t.maker, t.maker_position_size_prev, -taker_delta, true),
            (&t.taker, t.taker_position_size_prev, taker_delta, false),
        ] {
            let a = m.accounts.entry(owner.clone()).or_default();
            a.trades += 1;
            a.maker += u64::from(is_maker);
            if at > a.at {
                // Without `prev` the position cannot be known from one trade (and summing deltas
                // would depend on reading every trade, in order): it is left out.
                a.unknown = prev.is_none();
                a.position = prev.unwrap_or(0).saturating_add(delta);
                a.at = at;
            }
        }
        true
    }

    /// Forgets markets not in `keep` (games that left the catalogue), so the tape stays the
    /// card's size.
    pub fn retain(&mut self, keep: &[Symbol]) {
        self.markets.retain(|s, _| keep.contains(s));
    }

    /// Marks markets as fully read (or not).
    pub fn set_synced<'a>(&mut self, symbols: impl IntoIterator<Item = &'a Symbol>, synced: bool) {
        for s in symbols {
            self.markets.entry(s.clone()).or_default().synced = synced;
        }
    }

    /// Accounts that are maker in at least `card_pct` of at least `card_min` trades over every
    /// market here.
    pub fn card_makers(&self, rule: &MakerRule) -> HashSet<String> {
        let mut all: HashMap<&str, (u64, u64)> = HashMap::new();
        for m in self.markets.values() {
            for (owner, a) in &m.accounts {
                let e = all.entry(owner.as_str()).or_default();
                e.0 += a.trades;
                e.1 += a.maker;
            }
        }
        all.into_iter()
            .filter(|(_, (n, mk))| *n >= rule.card_min && mk * 100 >= rule.card_pct * n)
            .map(|(o, _)| o.to_owned())
            .collect()
    }

    /// Whether `owner` is a market maker on `symbol` by the market rules alone.
    fn market_maker(m: &MarketTape, a: &AccountTape, rule: &MakerRule) -> bool {
        let n = m.trades();
        (a.trades >= rule.own_min && a.maker * 100 >= rule.own_pct * a.trades)
            || (n >= rule.market_min && a.maker * 100 >= rule.market_pct * n)
    }

    /// Up to `n` newest trades on `symbol` whose taker is neither a market maker (by `rule` and
    /// `card_makers`) nor in `own` (the desk's hedger), newest first.
    pub fn recent(
        &self,
        symbol: &str,
        rule: &MakerRule,
        card_makers: &HashSet<String>,
        own: &HashSet<String>,
        n: usize,
    ) -> Vec<TapeTrade> {
        let Some(m) = self.markets.get(symbol) else {
            return Vec::new();
        };
        m.recent
            .iter()
            .rev()
            .filter(|t| !own.contains(&t.taker) && !card_makers.contains(&t.taker))
            .filter(|t| {
                m.accounts
                    .get(&t.taker)
                    .is_none_or(|a| !Self::market_maker(m, a, rule))
            })
            .take(n)
            .cloned()
            .collect()
    }

    /// The holders of `symbol`, market makers (by `rule`, with `card_makers` from
    /// [`Self::card_makers`]) and the accounts in `own` (the desk's hedger) left out.
    pub fn holders(
        &self,
        symbol: &str,
        rule: &MakerRule,
        card_makers: &HashSet<String>,
        own: &HashSet<String>,
    ) -> Holders {
        let Some(m) = self.markets.get(symbol) else {
            return Holders::default();
        };
        let mut h = Holders {
            trades_seen: m.trades(),
            synced: m.synced,
            ..Holders::default()
        };
        for (owner, a) in &m.accounts {
            if own.contains(owner) {
                continue;
            }
            if card_makers.contains(owner) || Self::market_maker(m, a, rule) {
                h.excluded_makers += 1;
                continue;
            }
            if a.unknown {
                continue;
            }
            let side = match a.position {
                0 => continue,
                p if p > 0 => &mut h.long,
                _ => &mut h.short,
            };
            side.accounts += 1;
            side.contracts += a.position.unsigned_abs();
        }
        h
    }
}
