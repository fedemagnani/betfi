//! Catalogue service (spec section 5, adapted from the sibling's `jobs/markets.rs`).
//!
//! Every 60 s (never more than once per 30 s, D-004) it pulls `GET /markets`, keeps the NBA
//! full-game moneylines (D-008: `game_page.game_id`, `market_type` sports/basketball/moneyline),
//! spreads and totals (6.0b, half-point lines only), groups them by game (one moneyline symbol
//! per game, D-021, plus its lines), upserts them into `markets` with their tick and fee
//! parameters (D-009: the quoter's and the desk's only source for them) and replaces the
//! in-memory [`Catalogue`]. `GET /card` is served from that cache.
//!
//! It also keeps the full-time result (1-X-2) of EPL, Bundesliga, LaLiga and Champions League
//! matches (7.0b, D-435): three yes-markets per game, one per outcome, grouped in
//! [`Catalogue::soccer`].
//!
//! Markets that leave the list keep their row with `live = 0`; looking them up by symbol for a
//! resolution is the crank's job (D-012, 2.C).

mod card;
mod parse;
pub mod synthetic;

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Router;
use axum::routing::get;
use pascal_rs::{Lenient, Market, Response};
use serde_json::Value;

pub use card::{
    CARD_LINES, CardLines, MONEYLINE_TITLE, SOCCER_RESULT_TITLE, card, card_at, card_for,
    card_market, card_sport, pick_lines, soccer_card_market, with_scores,
};
pub use parse::{
    CatalogueMarket, TeamInfo, catalogue_market, club_short, fmt_line, is_half_point, nba_market,
    nba_moneyline, parse_line, soccer_league, soccer_market,
};

use crate::api::types::{MarketKind, Sport, TeamSide};
use crate::now_ms;
use crate::state::{AppState, DeskEvent};

/// The three outcome symbols of a soccer match's full-time result (7.0b).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoccerGame {
    /// The home win ("1").
    pub home: String,
    /// The draw ("X").
    pub draw: String,
    /// The away win ("2").
    pub away: String,
}

impl SoccerGame {
    /// The symbols in card order: home, draw, away (L7-1).
    pub fn symbols(&self) -> [&str; 3] {
        [&self.home, &self.draw, &self.away]
    }
}

/// The catalogue cache: NBA winners, spreads and totals, and soccer 1-X-2 outcomes, of the
/// latest good `/markets` pass.
#[derive(Debug, Clone, PartialEq)]
pub struct Catalogue {
    /// Envelope `state_time_ms` of the list (receive time if absent).
    pub as_of_ms: u64,
    /// Local time the pass finished, Unix ms.
    pub fetched_ms: u64,
    /// Envelope `seq`.
    pub seq: Option<u64>,
    /// Markets by symbol, every kind and sport.
    pub markets: BTreeMap<String, CatalogueMarket>,
    /// Game id → its moneyline symbol (NBA winner markets only, as before 6.0b).
    pub games: BTreeMap<String, String>,
    /// Game id → its spread and total symbols, in symbol order.
    pub lines: BTreeMap<String, Vec<String>>,
    /// Game id → a soccer match's three outcome symbols (7.0b). Kept apart from `games`, which
    /// holds two-way moneylines only (sentiment reads it).
    pub soccer: BTreeMap<String, SoccerGame>,
}

impl Catalogue {
    /// Builds the cache from parsed markets, one moneyline per game: when a game lists more
    /// than one (not seen on 28 Oct, D-021), the one long on the away team wins, then the lowest
    /// symbol. Spreads and totals are kept per game and take their game's moneyline team
    /// abbreviations (their own listing has no `outcome_labels`). Soccer outcomes are grouped
    /// per game; a game is kept only with all three (the lowest symbol wins a duplicate), and
    /// each outcome takes the abbreviations of the game's two team symbols.
    pub fn build(
        as_of_ms: u64,
        fetched_ms: u64,
        seq: Option<u64>,
        list: Vec<CatalogueMarket>,
    ) -> Self {
        let mut games: BTreeMap<String, String> = BTreeMap::new();
        let mut lines: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut markets = BTreeMap::new();
        // Game id → [home, draw, away] outcome markets.
        let mut outcomes: BTreeMap<String, [Option<CatalogueMarket>; 3]> = BTreeMap::new();
        for m in list {
            if m.is_three_way() {
                let slot = match (m.draw, m.long_team) {
                    (true, _) => 1,
                    (false, TeamSide::Home) => 0,
                    (false, TeamSide::Away) => 2,
                };
                let slots = outcomes.entry(m.game_id.clone()).or_default();
                match &slots[slot] {
                    Some(prev) => {
                        tracing::warn!(game_id = %m.game_id, a = %prev.symbol, b = %m.symbol,
                            "two symbols for one soccer outcome; keeping the lower");
                        if m.symbol < prev.symbol {
                            slots[slot] = Some(m);
                        }
                    }
                    None => slots[slot] = Some(m),
                }
                continue;
            }
            if m.kind != MarketKind::Winner {
                lines
                    .entry(m.game_id.clone())
                    .or_default()
                    .push(m.symbol.clone());
                markets.insert(m.symbol.clone(), m);
                continue;
            }
            match games.get(&m.game_id) {
                Some(prev) => {
                    let p: &CatalogueMarket = &markets[prev];
                    let better = (m.long_team == TeamSide::Away && p.long_team != TeamSide::Away)
                        || (m.long_team == p.long_team && m.symbol < p.symbol);
                    tracing::warn!(game_id = %m.game_id, a = %p.symbol, b = %m.symbol,
                        "two moneylines for one game; keeping one");
                    if better {
                        games.insert(m.game_id.clone(), m.symbol.clone());
                    }
                }
                None => {
                    games.insert(m.game_id.clone(), m.symbol.clone());
                }
            }
            markets.insert(m.symbol.clone(), m);
        }
        for (game_id, symbols) in &mut lines {
            symbols.sort();
            let Some((away, home)) = games
                .get(game_id)
                .and_then(|s| markets.get(s))
                .map(|w: &CatalogueMarket| (w.away.clone(), w.home.clone()))
            else {
                continue;
            };
            for s in symbols.iter() {
                if let Some(m) = markets.get_mut(s) {
                    m.away.abbr.clone_from(&away.abbr);
                    m.home.abbr.clone_from(&home.abbr);
                }
            }
        }
        let mut soccer = BTreeMap::new();
        for (game_id, slots) in outcomes {
            let [Some(mut home), Some(mut draw), Some(mut away)] = slots else {
                let have: Vec<&str> = slots.iter().flatten().map(|m| m.symbol.as_str()).collect();
                tracing::warn!(%game_id, ?have, "soccer game without all three outcomes; left out");
                continue;
            };
            // A team symbol's suffix is its own abbreviation (the draw's pair split is a guess).
            let (h, a) = (home.home.abbr.clone(), away.away.abbr.clone());
            for m in [&mut home, &mut draw, &mut away] {
                m.home.abbr.clone_from(&h);
                m.away.abbr.clone_from(&a);
            }
            soccer.insert(
                game_id,
                SoccerGame {
                    home: home.symbol.clone(),
                    draw: draw.symbol.clone(),
                    away: away.symbol.clone(),
                },
            );
            for m in [home, draw, away] {
                markets.insert(m.symbol.clone(), m);
            }
        }
        Catalogue {
            as_of_ms,
            fetched_ms,
            seq,
            markets,
            games,
            lines,
            soccer,
        }
    }

    /// The three outcome markets of the soccer game `game_id`: home, draw, away.
    pub fn soccer_game(&self, game_id: &str) -> Option<[&CatalogueMarket; 3]> {
        let g = self.soccer.get(game_id)?;
        Some([
            self.markets.get(&g.home)?,
            self.markets.get(&g.draw)?,
            self.markets.get(&g.away)?,
        ])
    }

    /// The market behind `symbol`.
    pub fn market(&self, symbol: &str) -> Option<&CatalogueMarket> {
        self.markets.get(symbol)
    }

    /// The moneyline of `game_id`.
    pub fn game(&self, game_id: &str) -> Option<&CatalogueMarket> {
        self.games.get(game_id).and_then(|s| self.markets.get(s))
    }

    /// The spreads and totals of `game_id`, in symbol order.
    pub fn lines_of(&self, game_id: &str) -> impl Iterator<Item = &CatalogueMarket> {
        self.lines
            .get(game_id)
            .into_iter()
            .flatten()
            .filter_map(|s| self.markets.get(s))
    }

    /// The venue's clock now (D-060): the list's `state_time_ms` moved on by the desk's
    /// elapsed time since the pass.
    pub fn venue_now(&self, now_ms: u64) -> u64 {
        self.as_of_ms
            .saturating_add(now_ms.saturating_sub(self.fetched_ms))
    }
}

/// Mounts `GET /card`.
pub fn mount(router: Router<AppState>, _state: &AppState) -> Router<AppState> {
    router.route("/card", get(card))
}

/// Refreshes the catalogue every `catalogue_period()` until shutdown. The first pass runs at
/// once; a failed pass keeps the previous cache (`/card` reports its age, and answers 503 once
/// it is older than `card_max_age`).
pub async fn run(state: AppState) {
    let mut tick = tokio::time::interval(state.config.catalogue_period());
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = state.shutdown_requested() => break,
            _ = tick.tick() => {}
        }
        // pascal-rs retries a failing call for minutes: never let that hold up shutdown.
        tokio::select! {
            _ = state.shutdown_requested() => break,
            r = refresh(&state) => match r {
                Ok(n) => {
                    metrics::counter!("desk_catalogue_refresh_total", "result" => "ok").increment(1);
                    tracing::info!(markets = n, "catalogue refreshed");
                }
                Err(e) => {
                    metrics::counter!("desk_catalogue_refresh_total", "result" => "error").increment(1);
                    tracing::warn!(error = %e, "catalogue refresh failed; keeping the last one");
                }
            },
        }
    }
    tracing::info!("catalogue stopped");
}

/// Catalogue errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Pascal (or the mock) failed.
    #[error("pascal: {0}")]
    Pascal(#[from] pascal_rs::Error),
    /// The database failed.
    #[error("db: {0}")]
    Db(#[from] sqlx::Error),
}

/// One pass: fetch, filter, swap the cache, upsert, emit. Returns the number of markets kept
/// (NBA winners, spreads and totals; soccer 1-X-2 outcomes).
pub async fn refresh(state: &AppState) -> Result<usize, Error> {
    let resp = fetch_markets(state).await?;
    let fetched_ms = now_ms();
    let as_of_ms = resp.state_time_ms.unwrap_or(fetched_ms);
    let raws = resp.raw_items();
    for f in &resp.data.failures {
        let symbol = raws.get(f.index).and_then(|r| r["symbol"].as_str());
        tracing::warn!(index = f.index, ?symbol, error = %f.error, "market did not decode");
    }
    let mut list = Vec::new();
    let mut raw_of = BTreeMap::new();
    for (i, m) in &resp.data.items {
        if let Some(cm) = catalogue_market(m) {
            raw_of.insert(
                cm.symbol.clone(),
                raws.get(*i).cloned().unwrap_or(Value::Null),
            );
            list.push(cm);
        }
    }
    let catalogue = Arc::new(Catalogue::build(as_of_ms, fetched_ms, resp.seq, list));
    let n = catalogue.markets.len();
    *state.catalogue.write() = Some(catalogue.clone());
    metrics::gauge!("desk_catalogue_markets").set(n as f64);
    metrics::gauge!("desk_catalogue_as_of_ms").set(as_of_ms as f64);
    state.emit(DeskEvent::CatalogueRefreshed {
        as_of_ms,
        markets: n,
    });
    upsert(state, &catalogue, &raw_of).await?;
    Ok(n)
}

/// `GET /markets`, decoded market by market. This is the catalogue's only Pascal call; if the
/// `PascalApi` trait (C5) has no lenient variant at merge time, this becomes `markets()`.
async fn fetch_markets(state: &AppState) -> pascal_rs::Result<Response<Lenient<Market>>> {
    state.pascal.markets_lenient().await
}

/// Upserts the cache into `markets` and marks markets that left the list as not live. A
/// total's `long_team` column holds `away` (the Over placeholder, proposed D-321), a soccer
/// draw's `home` (proposed D-461); `sport` is `nba` or `soccer`.
async fn upsert(
    state: &AppState,
    c: &Catalogue,
    raw_of: &BTreeMap<String, Value>,
) -> Result<(), sqlx::Error> {
    let now = i64::try_from(now_ms()).unwrap_or(i64::MAX);
    let seq = c.seq.and_then(|s| i64::try_from(s).ok());
    let mut tx = state.db.begin().await?;
    for m in c.markets.values() {
        let raw = raw_of
            .get(&m.symbol)
            .map_or_else(String::new, Value::to_string);
        write_market(&mut tx, m, &raw, now, seq, true).await?;
    }
    // Rows of this sport that are no longer listed (resolved or delisted).
    let listed: Vec<&str> = c.markets.keys().map(String::as_str).collect();
    let live: Vec<String> = sqlx::query_scalar("SELECT symbol FROM markets WHERE live = 1")
        .fetch_all(&mut *tx)
        .await?;
    for gone in live.iter().filter(|s| !listed.contains(&s.as_str())) {
        sqlx::query("UPDATE markets SET live = 0, updated_ms = ? WHERE symbol = ?")
            .bind(now)
            .bind(gone)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}

/// One `markets` row, inserted or refreshed. `live`: in the latest `GET /markets` list; a row
/// written with `live = false` (a market known from elsewhere, e.g. Pascal's resolved list)
/// never clears the flag of a listed one.
async fn write_market(
    conn: &mut sqlx::SqliteConnection,
    m: &CatalogueMarket,
    raw: &str,
    now: i64,
    seq: Option<i64>,
    live: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO markets (symbol, game_id, sport, event, event_desc, market_desc,
            away, away_abbr, home, home_abbr, long_team, start_ms, expected_resolution_ms,
            tick_size_min_u, tick_sig_figs, taker_fee_rate_ppm, maker_rebate_share_ppm,
            polymarket_slug, mark_price_u, best_bid_u, best_ask_u, open_interest, live,
            seq, first_seen_ms, updated_ms, raw)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (symbol) DO UPDATE SET
            game_id = excluded.game_id, sport = excluded.sport, event = excluded.event,
            event_desc = excluded.event_desc, market_desc = excluded.market_desc,
            away = excluded.away, away_abbr = excluded.away_abbr, home = excluded.home,
            home_abbr = excluded.home_abbr, long_team = excluded.long_team,
            start_ms = excluded.start_ms,
            expected_resolution_ms = excluded.expected_resolution_ms,
            tick_size_min_u = excluded.tick_size_min_u,
            tick_sig_figs = excluded.tick_sig_figs,
            taker_fee_rate_ppm = excluded.taker_fee_rate_ppm,
            maker_rebate_share_ppm = excluded.maker_rebate_share_ppm,
            polymarket_slug = excluded.polymarket_slug,
            mark_price_u = excluded.mark_price_u, best_bid_u = excluded.best_bid_u,
            best_ask_u = excluded.best_ask_u, open_interest = excluded.open_interest,
            live = MAX(markets.live, excluded.live), seq = excluded.seq, updated_ms = excluded.updated_ms,
            raw = excluded.raw",
    )
    .bind(&m.symbol)
    .bind(&m.game_id)
    .bind(match m.league.sport() {
        Sport::Basketball => "nba",
        Sport::Soccer => "soccer",
    })
    .bind(m.event())
    .bind(&m.event_desc)
    .bind(&m.market_desc)
    .bind(&m.away.name)
    .bind(&m.away.abbr)
    .bind(&m.home.name)
    .bind(&m.home.abbr)
    .bind(match m.long_team {
        TeamSide::Away => "away",
        TeamSide::Home => "home",
    })
    .bind(i64::try_from(m.start_ms).unwrap_or(i64::MAX))
    .bind(i64::try_from(m.expected_resolution_ms).unwrap_or(i64::MAX))
    .bind(m.tick_size_min_u)
    .bind(i64::from(m.tick_sig_figs))
    .bind(i64::from(m.taker_fee_rate_ppm))
    .bind(i64::from(m.maker_rebate_share_ppm))
    .bind(&m.polymarket_slug)
    .bind(m.mark_price_u)
    .bind(m.best_bid_u)
    .bind(m.best_ask_u)
    .bind(i64::try_from(m.open_interest).unwrap_or(i64::MAX))
    .bind(i64::from(live))
    .bind(seq)
    .bind(now)
    .bind(now)
    .bind(raw)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// A market of Pascal's resolved list as the catalogue parsed it while it was listed (the
/// parsers skip a market with a resolution: nothing can be quoted on it).
pub fn resolved_market(m: &Market) -> Option<CatalogueMarket> {
    let mut m = m.clone();
    m.resolution = None;
    catalogue_market(&m)
}

/// Markets known from outside the catalogue (Pascal's resolved list, 31 Oct): written as not
/// live, so a ticket leg named from them shows its teams. `raw` is each one's JSON.
pub async fn remember(
    state: &AppState,
    markets: &[(CatalogueMarket, Value)],
) -> Result<(), sqlx::Error> {
    let now = i64::try_from(now_ms()).unwrap_or(i64::MAX);
    let mut tx = state.db.begin().await?;
    for (m, raw) in markets {
        write_market(&mut tx, m, &raw.to_string(), now, None, false).await?;
    }
    tx.commit().await
}
