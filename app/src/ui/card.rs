//! The Sportsbook (`/`, 7.1a, BetFi C2 `betfi_sportsbook_equal_odds`): under the shell's app bar
//! and live ticker, the sport tabs (V-2: Basketball 1–2, Soccer 1–X–2; a tab shows only when the
//! card has games of that sport), a live card for each started game of the chosen sport, then
//! "UPCOMING MATCHES · Today" with one card per game to come. A live card that lists more than
//! one day (2 Nov: no slate date) groups them by local day, "Upcoming · Today", then
//! "TOMORROW", "TUE 3 NOV"…; a demo build or a replayed night stays one "Today" group.
//!
//! - A card's outcome buttons are its winner market, "1 / X / 2" in outcome order (1 = home,
//!   2 = away, the European convention the desk follows for the NBA too). Lowest value green,
//!   highest purple, equal neutral (guidelines §2); a tap adds or removes that pick on the slip
//!   (one leg per game, in-play winners allowed, U-2), and the selected one wears the gradient
//!   border (D-452). The floating slip pill (the shell's) follows the slip.
//! - The card's header (league, time, teams) opens the event page, where the spreads and totals
//!   are (L6); a live card says so in green, "View Market Volume →".
//! - Live scores (31 Oct, replacing V-1's chances): a live card's score slot shows the desk's
//!   live score ("–" until it has one), its meta the league and the match clock ("EPL • 67′",
//!   "EPL • Half-time", "NBA • Q3 4:12"; the minutes since the start with no score). A game that is
//!   over is not on the desk's card.
//! - Never the word "odds" (guidelines §2).

use dioxus::prelude::*;

use super::kit::{
    self, MatchCard, OutcomeButton, SectionTitle, Sport, SportTabs, TeamLine, outcome_tones,
};
use super::{use_app_state, use_desk};
use crate::Route;
use crate::desk::fixtures;
use crate::desk::types::{Game, Market, MarketKind, MarketPick, Pick};
use crate::format;
use crate::state::Load;

/// The sport a game is listed under (7.0b's `sport`; a desk before L7 lists NBA games only).
fn sport_of(game: &Game) -> Sport {
    match game.sport.as_deref() {
        Some("soccer") => Sport::Soccer,
        _ => Sport::Basketball,
    }
}

/// "NBA", "EuroLeague", "EPL": the desk's league, else read from the game id.
fn league_of(game: &Game) -> String {
    if let Some(l) = game.league.as_deref().filter(|l| !l.is_empty()) {
        return l.to_string();
    }
    if game.game_id.starts_with("el-") {
        "EuroLeague".to_string()
    } else {
        "NBA".to_string()
    }
}

/// Whole minutes since the start (V-1: "Live · 50′").
fn minutes_in(game: &Game, now_ms: i64) -> i64 {
    (now_ms - game.started_ms.unwrap_or(game.start_ms)).max(0) / 60_000
}

/// Where an outcome sits in "1 / X / 2".
fn outcome_rank(outcome: &str) -> u8 {
    match outcome {
        "1" => 0,
        "X" => 1,
        _ => 2,
    }
}

/// A pick's outcome as the desk names it, else from its team (home "1", away "2", none "X").
fn outcome_of(mp: &MarketPick) -> String {
    mp.outcome.clone().unwrap_or_else(|| {
        match mp.team {
            Some(Pick::Home) => "1",
            Some(Pick::Away) => "2",
            None => "X",
        }
        .to_string()
    })
}

/// The game's winner market, its picks in "1 / X / 2" order. A desk before L6 sends no
/// `markets`: the winner is then built from the game's two `picks` (one symbol, D-021).
fn winner_of(game: &Game) -> Option<Market> {
    let mut market = game
        .markets
        .iter()
        .find(|m| m.kind == MarketKind::Winner && !m.picks.is_empty())
        .cloned()
        .or_else(|| {
            (!game.picks.is_empty()).then(|| Market {
                kind: MarketKind::Winner,
                line: None,
                picks: game
                    .picks
                    .iter()
                    .map(|p| MarketPick {
                        label: game.team(p.pick).name.clone(),
                        team: Some(p.pick),
                        symbol: game.symbol.clone(),
                        side: p.side,
                        price_u: p.price_u,
                        odds_u: p.odds_u,
                        outcome: p.outcome.clone(),
                    })
                    .collect(),
                market_title: None,
            })
        })?;
    market.picks.sort_by_key(|p| outcome_rank(&outcome_of(p)));
    Some(market)
}

/// The tab the user chose, kept while the app runs so that coming back from an event page shows
/// the same sport. `None` until a tab is tapped: the first sport the card holds.
static CHOSEN: GlobalSignal<Option<Sport>> = Signal::global(|| None);

#[component]
pub fn Card() -> Element {
    let state = use_app_state();
    let desk = use_desk();
    let card = state.read().card.clone();
    let now_ms = state.read().now_ms(desk.is_demo());
    rsx! {
        div { class: "home", id: "home",
            h1 { class: "sr-only", "Sports" }
            match card {
                Load::Loading => rsx! {
                    kit::Skeleton { kind: kit::SkeletonKind::Match, count: 3, label: "Loading games…" }
                },
                Load::Failed(e) => rsx! {
                    p { class: "home-note error", "{e}" }
                    button {
                        r#type: "button",
                        class: "home-retry",
                        // Root-level task: leaving the screen must not strand Home in Loading.
                        onclick: move |_| {
                            dioxus::core::spawn_forever(crate::load_card(desk.clone(), state));
                        },
                        "Try again"
                    }
                },
                Load::Ready(card) => {
                    let games: Vec<Game> = card
                        .games
                        .iter()
                        .filter(|g| g.offers_winner())
                        .cloned()
                        .collect();
                    let sports: Vec<Sport> = Sport::ALL
                        .into_iter()
                        .filter(|s| games.iter().any(|g| sport_of(g) == *s))
                        .collect();
                    let active = CHOSEN()
                        .filter(|s| sports.contains(s))
                        .or(sports.first().copied());
                    // Picks on the slip per sport, for the chips' counters.
                    let counts: Vec<(Sport, usize)> = {
                        let st = state.read();
                        sports
                            .iter()
                            .map(|s| {
                                let n = st
                                    .tray
                                    .legs
                                    .iter()
                                    .filter(|l| games.iter().any(|g| g.game_id == l.game_id && sport_of(g) == *s))
                                    .count();
                                (*s, n)
                            })
                            .collect()
                    };
                    let shown: Vec<Game> = games
                        .into_iter()
                        .filter(|g| Some(sport_of(g)) == active)
                        .collect();
                    let live: Vec<Game> = shown.iter().filter(|g| g.in_play).cloned().collect();
                    let mut upcoming: Vec<Game> = shown.into_iter().filter(|g| !g.in_play).collect();
                    // Design builds: the mockup's order of these cards (stable otherwise).
                    upcoming.sort_by_key(|g| fixtures::design::mobile_rank(&g.game_id).unwrap_or(usize::MAX));
                    let days = by_day(upcoming, now_ms, !desk.is_demo() && crate::config::slate_date_override().is_none());
                    rsx! {
                        if let Some(a) = active {
                            SportTabs { sports, active: a, counts, onselect: move |s| *CHOSEN.write() = Some(s) }
                        }
                        if live.is_empty() && days.is_empty() {
                            p { class: "home-note", "No games on the card right now." }
                        }
                        for (i, game) in live.into_iter().enumerate() {
                            LiveCard { key: "{game.game_id}", game, now_ms, first: i == 0 }
                        }
                        if !days.is_empty() {
                            section { class: "home-upcoming", aria_labelledby: "home-upcoming-title",
                                for (i, (label, games)) in days.into_iter().enumerate() {
                                    if i == 0 {
                                        SectionTitle { title: "Upcoming", end: rsx! { "{label}" }, id: "home-upcoming-title" }
                                    } else {
                                        SectionTitle { title: label, small: true }
                                    }
                                    div { class: "home-list",
                                        for game in games {
                                            UpcomingCard { key: "{game.game_id}", game }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The games to come under their day's title, in start order: "Today", "Tomorrow", "Tue 3 Nov"
/// ([`format::day_label`]) when `split`, else one "Today" group (a demo build's fixed card, a
/// replayed night). Empty when there is no game.
fn by_day(games: Vec<Game>, now_ms: i64, split: bool) -> Vec<(String, Vec<Game>)> {
    if !split {
        return if games.is_empty() {
            Vec::new()
        } else {
            vec![("Today".to_string(), games)]
        };
    }
    let mut days: Vec<(String, Vec<Game>)> = Vec::new();
    for game in games {
        let label = format::day_label(game.start_ms, now_ms);
        match days.last_mut() {
            Some((l, g)) if *l == label => g.push(game),
            _ => days.push((label, vec![game])),
        }
    }
    days
}

/// A started game (the mockup's featured card): LIVE, "League • 67′", "View Market
/// Volume →" to the event page; each team's code, name and chance; the outcome buttons. The
/// first one carries `id="home-hero"`.
#[component]
fn LiveCard(game: Game, now_ms: i64, first: bool) -> Element {
    let Some(market) = winner_of(&game) else {
        return rsx! {};
    };
    let mins = minutes_in(&game, now_ms);
    let score = game.score.as_ref();
    let clock = format::match_clock(
        score.and_then(|s| s.period.as_deref()),
        score.and_then(|s| s.elapsed.as_deref()),
        mins,
    );
    // The LIVE badge says it is live: the meta is the league and the clock, so it fits at 390 px.
    let meta = format!("{} • {clock}", league_of(&game));
    // The score (31 Oct), a dash on each side until the feed has one.
    let (home_pct, away_pct) = match score {
        Some(s) => (Some(s.home.to_string()), Some(s.away.to_string())),
        None => (Some("–".to_string()), Some("–".to_string())),
    };
    let soccer = sport_of(&game) == Sport::Soccer;
    let line = |p: Pick, value: Option<String>| TeamLine {
        name: game.team(p).name.clone(),
        code: Some(game.team(p).abbr.clone()),
        logo: kit::team_logo(&game.team(p).name, soccer),
        value,
    };
    let (home, away) = (line(Pick::Home, home_pct), line(Pick::Away, away_pct));
    let to = Route::EventScreen {
        game_id: game.game_id.clone(),
    };
    rsx! {
        div { class: "home-game home-live", "data-game-id": "{game.game_id}",
            MatchCard {
                meta,
                live: true,
                to,
                action: "Market volume",
                first: home,
                second: away,
                id: first.then(|| "home-hero".to_string()),
                Outcomes { game: game.clone(), market, hero: true }
            }
        }
    }
}

/// A game to come: "League • 20:45", the two teams, the outcome buttons (no market title: the
/// 1 / X / 2 buttons under the teams say it, 31 Oct).
#[component]
fn UpcomingCard(game: Game) -> Element {
    let Some(market) = winner_of(&game) else {
        return rsx! {};
    };
    let meta = format!("{} • {}", league_of(&game), format::clock(game.start_ms));
    let soccer = sport_of(&game) == Sport::Soccer;
    let to = Route::EventScreen {
        game_id: game.game_id.clone(),
    };
    rsx! {
        div { class: "home-game home-row", "data-game-id": "{game.game_id}",
            MatchCard {
                meta,
                to,
                first: TeamLine { logo: kit::team_logo(&game.home.name, soccer), ..TeamLine::named(game.home.name.clone()) },
                second: TeamLine { logo: kit::team_logo(&game.away.name, soccer), ..TeamLine::named(game.away.name.clone()) },
                Outcomes { game: game.clone(), market, hero: false }
            }
        }
    }
}

/// The winner market's outcome buttons, "1 / X / 2", toned by value; each toggles its pick on
/// the slip. `.pick` and `.pick-home` / `.pick-draw` / `.pick-away` are the hooks the gates and
/// the e2e scripts click (D-343).
#[component]
fn Outcomes(game: Game, market: Market, hero: bool) -> Element {
    let mut state = use_app_state();
    let tones = outcome_tones(&market.picks.iter().map(|p| p.odds_u).collect::<Vec<_>>());
    let chosen = state
        .read()
        .tray
        .legs
        .iter()
        .find(|l| l.game_id == game.game_id && l.kind == MarketKind::Winner)
        .map(|l| (l.symbol.clone(), l.side));
    rsx! {
        for (i, mp) in market.picks.into_iter().enumerate() {
            {
                let outcome = outcome_of(&mp);
                let side = match mp.team {
                    Some(Pick::Home) => "home",
                    Some(Pick::Away) => "away",
                    None => "draw",
                };
                let priced = mp.odds_u > 0;
                let value = if priced { format::odds(mp.odds_u) } else { "–".to_string() };
                let who = match mp.team {
                    Some(p) => format!("{} to win", game.team(p).name),
                    None => mp.label.clone(),
                };
                let aria = if priced { format!("{who}, {value}") } else { format!("{who}: no price yet") };
                let selected = chosen.as_ref().is_some_and(|(s, d)| *s == mp.symbol && *d == mp.side);
                let g = game.clone();
                rsx! {
                    OutcomeButton {
                        key: "{i}",
                        outcome,
                        value,
                        tone: tones[i],
                        selected,
                        disabled: !priced,
                        hero,
                        aria_label: aria,
                        class: "pick pick-{side}",
                        onclick: move |_| {
                            state.write().tray.toggle_outcome(&g, &mp);
                        },
                    }
                }
            }
        }
    }
}
