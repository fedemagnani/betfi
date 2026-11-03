//! `GET /card?sport=nba[&date=YYYY-MM-DD][&lines=all]` (l1-contracts C4, l6-contracts L6-2,
//! l7-contracts L7-1: `sport` may be `nba`, `basketball`, `soccer`, or left out for both).
//! Without `date` (or with an empty one) the card holds every game Pascal lists that is not over
//! (2 Nov), whatever its day; with one, that US night's games only (a replayed tape's night).

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Query, State};

use super::Catalogue;
use super::parse::{CatalogueMarket, is_date};
use crate::api::ApiError;
use crate::api::types::{
    CardGame, CardMarket, CardMarketPick, CardPick, CardResponse, League, MarketKind, Side, Sport,
    Team, TeamSide, odds_at,
};
use crate::now_ms;
use crate::state::AppState;

/// Spread lines and total lines per game on the card, each (proposed D-320).
pub const CARD_LINES: usize = 3;

/// An NBA winner market's title (L7-1).
pub const MONEYLINE_TITLE: &str = "Moneyline";

/// A soccer winner market's title (L7-1).
pub const SOCCER_RESULT_TITLE: &str = "Full Time Result (1X2)";

/// How many lines of each kind a card game carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardLines {
    /// [`CARD_LINES`] around the line nearest even odds (the default card).
    Main,
    /// Every priced line (`lines=all`, for an event page).
    All,
}

/// The card handler. Query parameters are read leniently so a bad one answers with the C4 error
/// body (422) rather than axum's plain-text 400. `sport=nba` (the C4 value) and
/// `sport=basketball` both give the NBA card; without `sport` the card holds every sport.
pub async fn card(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Json<CardResponse>, ApiError> {
    let sport = match q.get("sport").map(String::as_str) {
        None => None,
        Some("nba" | "basketball") => Some(Sport::Basketball),
        Some("soccer") => Some(Sport::Soccer),
        Some(other) => {
            return Err(ApiError::rejected(format!(
                "unsupported sport {other:?} (nba, basketball or soccer)"
            )));
        }
    };
    let date = q.get("date").map(String::as_str).filter(|d| !d.is_empty());
    if let Some(d) = date.filter(|d| !is_date(d)) {
        return Err(ApiError::rejected(format!("date {d:?} is not YYYY-MM-DD")));
    }
    let lines = match q.get("lines").map(String::as_str) {
        None | Some("main") => CardLines::Main,
        Some("all") => CardLines::All,
        Some(other) => {
            return Err(ApiError::rejected(format!(
                "lines {other:?} is not main or all"
            )));
        }
    };
    let Some(cat) = state.catalogue() else {
        return Err(ApiError::unavailable("catalogue not loaded yet"));
    };
    let now = now_ms();
    let age_ms = now.saturating_sub(cat.fetched_ms);
    if u128::from(age_ms) > state.config.card_max_age.as_millis() {
        return Err(ApiError::unavailable(format!(
            "catalogue is {} s old",
            age_ms / 1000
        )));
    }
    let venue_now = cat.venue_now(now);
    let mut resp = card_sport(&cat, date, venue_now, lines, sport);
    with_scores(&mut resp, |id| state.scores.get(id), venue_now, now);
    if let Some(s) = q.get("sport") {
        resp.sport.clone_from(s);
    }
    Ok(Json(resp))
}

/// Leaves out the games that are over ([`crate::scores::finished`]: among them every game whose
/// winner book is decided, started or not) and gives each live one its score while the feed's reading is fresh (31 Oct). `reading` is the feed's latest per
/// game id; `venue_now` and `wall_now` as in [`crate::scores::finished`].
pub fn with_scores(
    resp: &mut CardResponse,
    reading: impl Fn(&str) -> Option<crate::scores::Reading>,
    venue_now: u64,
    wall_now: u64,
) {
    resp.games.retain_mut(|g| {
        let r = reading(&g.game_id);
        let prices: Vec<u64> = g
            .markets
            .first()
            .map(|m| m.picks.iter().map(|p| p.price_u).collect())
            .unwrap_or_default();
        let over = crate::scores::finished(
            r.as_ref(),
            &prices,
            g.start_ms,
            g.expected_resolution_ms,
            venue_now,
            wall_now,
        );
        if g.in_play {
            g.score = r.filter(|r| r.fresh(wall_now)).and_then(|r| r.score);
        }
        !over
    });
}

/// [`card_at`] on the venue clock now, with the main lines.
pub fn card_for(cat: &Catalogue, date: Option<&str>) -> CardResponse {
    card_at(cat, date, cat.venue_now(now_ms()), CardLines::Main)
}

/// The NBA card of `date` (every date when `None`) at venue time `now_ms` (`sport: "nba"`):
/// [`card_sport`] for basketball.
pub fn card_at(cat: &Catalogue, date: Option<&str>, now_ms: u64, lines: CardLines) -> CardResponse {
    let mut c = card_sport(cat, date, now_ms, lines, Some(Sport::Basketball));
    c.sport = "nba".to_owned();
    c
}

/// The card of `date` at venue time `now_ms` for `sport` (`None`: every sport; the response's
/// `sport` is then `all`, else the sport's name): games whose game id ends with `date` (every
/// game when `None`), by start time, then game id. A game whose winner lacks an indicative price
/// on any pick is left out; one that is over goes in [`with_scores`].
///
/// An NBA game keeps its away and home pick priced from the one moneyline symbol (D-021) and
/// lists its markets: the winner, then (before the start) the spread and total lines picked by
/// [`pick_lines`]. A soccer game (7.0b) lists its 1-X-2 winner alone ([`soccer_card_market`]).
/// A started game (`start_ms <= now_ms`) offers its winner only, flagged `in_play` (U-2).
pub fn card_sport(
    cat: &Catalogue,
    date: Option<&str>,
    now_ms: u64,
    lines: CardLines,
    sport: Option<Sport>,
) -> CardResponse {
    let on = |d: Option<&str>| date.is_none_or(|want| d == Some(want));
    let wants = |s: Sport| sport.is_none_or(|w| w == s);
    let mut games: Vec<CardGame> = Vec::new();
    if wants(Sport::Basketball) {
        games.extend(
            cat.games
                .values()
                .filter_map(|symbol| cat.markets.get(symbol))
                .filter(|m| on(m.date()))
                .filter_map(|m| nba_game(cat, m, now_ms, lines)),
        );
    }
    if wants(Sport::Soccer) {
        games.extend(
            cat.soccer
                .keys()
                .filter_map(|game_id| cat.soccer_game(game_id))
                .filter(|[home, ..]| on(home.date()))
                .filter_map(|outcomes| soccer_game(outcomes, now_ms)),
        );
    }
    games.sort_by(|a, b| (a.start_ms, &a.game_id).cmp(&(b.start_ms, &b.game_id)));
    CardResponse {
        sport: sport.map_or("all", Sport::as_str).to_owned(),
        date: date.unwrap_or_default().to_owned(),
        as_of_ms: cat.as_of_ms,
        games,
    }
}

fn team(t: &super::TeamInfo) -> Team {
    Team {
        name: t.name.clone(),
        abbr: t.abbr.clone(),
    }
}

/// One NBA game from its moneyline `m`.
fn nba_game(
    cat: &Catalogue,
    m: &CatalogueMarket,
    now_ms: u64,
    lines: CardLines,
) -> Option<CardGame> {
    let picks = [TeamSide::Away, TeamSide::Home]
        .into_iter()
        .map(|pick| {
            let side = m.side_for(pick);
            let price_u = m.indicative_price_u(side)?;
            Some(CardPick {
                pick,
                side,
                price_u,
                odds_u: odds_at(price_u),
                outcome: m.outcome(side),
            })
        })
        .collect::<Option<Vec<_>>>();
    let Some(picks) = picks else {
        tracing::debug!(symbol = %m.symbol, "no indicative price; game left off the card");
        return None;
    };
    let in_play = m.start_ms <= now_ms;
    let mut markets: Vec<CardMarket> = card_market(m).into_iter().collect();
    let markets_count = if in_play {
        1
    } else {
        let all: Vec<&CatalogueMarket> = cat.lines_of(&m.game_id).collect();
        for kind in [MarketKind::Spread, MarketKind::Total] {
            let of_kind: Vec<&CatalogueMarket> =
                all.iter().copied().filter(|l| l.kind == kind).collect();
            markets.extend(
                pick_lines(&of_kind, lines)
                    .into_iter()
                    .filter_map(card_market),
            );
        }
        u32::try_from(1 + all.len()).unwrap_or(u32::MAX)
    };
    Some(CardGame {
        game_id: m.game_id.clone(),
        symbol: m.symbol.clone(),
        start_ms: m.start_ms,
        expected_resolution_ms: m.expected_resolution_ms,
        away: team(&m.away),
        home: team(&m.home),
        long_team: m.long_team,
        picks,
        markets_count,
        in_play,
        started_ms: in_play.then_some(m.start_ms),
        markets,
        sport: Sport::Basketball,
        league: League::Nba,
        score: None,
    })
}

/// One soccer game from its home, draw and away outcome markets. Start and resolution times
/// are the home symbol's (Pascal lists the same on all three).
fn soccer_game(outcomes: [&CatalogueMarket; 3], now_ms: u64) -> Option<CardGame> {
    let [home, ..] = outcomes;
    let Some(market) = soccer_card_market(outcomes) else {
        tracing::debug!(game_id = %home.game_id, "no indicative price; game left off the card");
        return None;
    };
    let in_play = home.start_ms <= now_ms;
    Some(CardGame {
        game_id: home.game_id.clone(),
        symbol: home.symbol.clone(),
        start_ms: home.start_ms,
        expected_resolution_ms: home.expected_resolution_ms,
        away: team(&home.away),
        home: team(&home.home),
        long_team: TeamSide::Home,
        picks: Vec::new(),
        markets_count: 1,
        in_play,
        started_ms: in_play.then_some(home.start_ms),
        markets: vec![market],
        sport: Sport::Soccer,
        league: home.league,
        score: None,
    })
}

/// One outcome of a market as a card pick.
fn pick(m: &CatalogueMarket, side: Side) -> Option<CardMarketPick> {
    let price_u = m.indicative_price_u(side)?;
    Some(CardMarketPick {
        label: m.label(side),
        team: m.pick_team(side),
        symbol: m.symbol.clone(),
        side,
        price_u,
        odds_u: odds_at(price_u),
        outcome: m.outcome(side),
    })
}

/// One market as the card shows it, `None` when either side has no indicative price. Picks
/// are away then home (winner, spread), Over then Under (total). A winner carries its title
/// and each pick its 1-X-2 code (L7-1).
pub fn card_market(m: &CatalogueMarket) -> Option<CardMarket> {
    let sides = match m.kind {
        MarketKind::Total => [Side::Long, Side::Short],
        _ => [m.side_for(TeamSide::Away), m.side_for(TeamSide::Home)],
    };
    let picks = sides
        .into_iter()
        .map(|side| pick(m, side))
        .collect::<Option<Vec<_>>>()?;
    Some(CardMarket {
        kind: m.kind,
        line: m.line(),
        picks,
        market_title: (m.kind == MarketKind::Winner).then(|| MONEYLINE_TITLE.to_owned()),
    })
}

/// A soccer game's 1-X-2 winner as the card shows it (L7-1): three picks, home ("1"), draw
/// ("X"), away ("2"), each a long on its own outcome symbol priced from that symbol's ask
/// (never a short: D-435). `None` when any outcome has no indicative price.
pub fn soccer_card_market(outcomes: [&CatalogueMarket; 3]) -> Option<CardMarket> {
    let picks = outcomes
        .into_iter()
        .map(|m| pick(m, Side::Long))
        .collect::<Option<Vec<_>>>()?;
    Some(CardMarket {
        kind: MarketKind::Winner,
        line: None,
        picks,
        market_title: Some(SOCCER_RESULT_TITLE.to_owned()),
    })
}

/// The lines of one kind (all spreads or all totals of a game) the card shows (proposed
/// D-320): the priced lines, ordered by [`CatalogueMarket::ladder_key`] (the away team's
/// handicap, or the total); the main line is the one whose long-side chance
/// ([`CatalogueMarket::long_mid_u`]) is nearest 0.5 (the lower line on a tie); the card keeps
/// [`CARD_LINES`] consecutive lines centred on it, shifted inward at either end of the ladder.
/// [`CardLines::All`] keeps every priced line.
pub fn pick_lines<'a>(
    markets: &[&'a CatalogueMarket],
    lines: CardLines,
) -> Vec<&'a CatalogueMarket> {
    let mut ladder: Vec<&CatalogueMarket> = markets
        .iter()
        .copied()
        .filter(|m| {
            m.long_mid_u().is_some()
                && m.indicative_price_u(Side::Long).is_some()
                && m.indicative_price_u(Side::Short).is_some()
        })
        .collect();
    ladder.sort_by(|a, b| (a.ladder_key(), &a.symbol).cmp(&(b.ladder_key(), &b.symbol)));
    if lines == CardLines::All || ladder.len() <= CARD_LINES {
        return ladder;
    }
    let distance = |m: &CatalogueMarket| m.long_mid_u().map_or(i64::MAX, |p| (p - 500_000).abs());
    let main = (0..ladder.len())
        .min_by_key(|&i| (distance(ladder[i]), i))
        .unwrap_or(0);
    let start = main
        .saturating_sub(CARD_LINES / 2)
        .min(ladder.len() - CARD_LINES);
    ladder[start..start + CARD_LINES].to_vec()
}
