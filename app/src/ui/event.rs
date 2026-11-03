//! Event Detail (`/event/:game_id`, step 7.1c, BetFi C2 `betfi_event_detail_market_volume`): the
//! live card, the winner market as big outcome cards (team and value only; "Match Winner (1-2)"
//! or "Full Time Result (1-X-2)", with a draw for soccer, is its screen-reader heading), the
//! Staked Sentiment card (`sentiment.rs`), the game's spread and total lines under a collapsed "More markets", and the purple CTA.
//!
//! - **1-X-2 mapping.** Side 1 is the home team, X the draw, 2 the away team (`/card`'s `outcome`,
//!   l7-contracts L7-1), for every sport and in that order, left to right.
//! - **Live score** (31 Oct, replacing V-1's chances): a started game shows the match clock
//!   ("Live · 67′", "Live · Half-time", "Live · Q3 4:12"; the minutes since the start while the
//!   desk has no score) and, in the score slot, the desk's live score ("Live" until it has one).
//!   An unstarted game shows its start time there instead. A game that is over leaves the card.
//! - **Truth rule.** Never the word "odds" (guidelines §2), no "Solana Slipstream" chip, no points
//!   run; the sentiment chip says what the money is ("€x staked").
//! - **Picks.** A tap puts the pick in the slip in place of any other pick on the same game (one
//!   leg per game, D-008); tapping it again takes it out. Started spread and total markets are
//!   already gone from `/card` (U-2).
//! - **CTA.** "Select to bet" scrolls to the outcomes; once the slip holds picks it opens the bet
//!   drawer over this page. While the CTA shows, the shell hides its purple slip pill, so the page
//!   has one bottom action (D-505).
//! - **Locked** (L9, `/copy/:id/event/:game_id`, [`EventView`] with a [`LockedPick`]): a game of
//!   a tipster's parlay. The tipster's pick shows selected and the others are off; taps change
//!   nothing; a spread or total pick opens on its own line. The CTA copies the whole parlay (the
//!   copy drawer, `tray::open_copy_drawer`), at today's values.
//! - **Share.** The header's share button ([`EventShare`]) sits in the shell's
//!   `kit::SubHeader { end }` (D-500).

use dioxus::prelude::*;

use super::kit::{
    self, Badge, BadgeKind, OutcomeButton, OutcomeCard, PurpleCTA, Sym, SymIcon, outcome_tones,
};
use super::sentiment::StakedSentiment;
use super::{use_app_state, use_desk};
use crate::Route;
use crate::desk::types::{Game, Market, MarketKind, MarketPick, Pick, Side};
use crate::format;
use crate::state::{Load, MAX_LEGS, Tray, TrayLeg};

/// The event CTA is on screen: the shell hides its slip pill meanwhile (one bottom action, D-505).
static CTA_SHOWN: GlobalSignal<bool> = Signal::global(|| false);

/// Whether the event page's own CTA is showing (read by the shell).
pub fn cta_shown() -> bool {
    CTA_SHOWN()
}
/// Brings the outcome cards into view.
const SCROLL_JS: &str = "const e = document.getElementById('event-outcomes'); \
     if (e) { e.scrollIntoView({ behavior: 'smooth', block: 'center' }); }";

/// One outcome of a game's winner market, in the 1-X-2 order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    Home,
    Draw,
    Away,
}

impl Outcome {
    /// The pick's `outcome` from `/card` ("1", "X", "2"); a desk before L7 sends none, and then
    /// the same mapping is read from `team` (1 = home, 2 = away, no team = the draw).
    fn of(p: &MarketPick) -> Outcome {
        match (p.outcome.as_deref(), p.team) {
            (Some("1"), _) => Outcome::Home,
            (Some("X" | "x"), _) => Outcome::Draw,
            (Some("2"), _) => Outcome::Away,
            (_, Some(Pick::Home)) => Outcome::Home,
            (_, Some(Pick::Away)) => Outcome::Away,
            (_, None) => Outcome::Draw,
        }
    }

    fn code(self) -> &'static str {
        match self {
            Outcome::Home => "1",
            Outcome::Draw => "X",
            Outcome::Away => "2",
        }
    }
}

/// A tipster's pick on this game, fixed (L9): what the locked page shows selected, and the
/// parlay its CTA copies. `blocked` says why the tip cannot be copied now.
#[derive(Debug, Clone, PartialEq)]
pub struct LockedPick {
    pub symbol: String,
    pub side: Side,
    pub kind: MarketKind,
    pub who: String,
    pub target: super::tray::CopyTarget,
    pub blocked: Option<String>,
}

/// `/event/:game_id`.
#[component]
pub fn EventScreen(game_id: String) -> Element {
    rsx! { EventView { game_id, locked: None } }
}

/// The event page; `locked` makes it a game of a tipster's parlay (see the module docs).
#[component]
pub fn EventView(game_id: String, locked: Option<LockedPick>) -> Element {
    let mut state = use_app_state();
    let demo = use_desk().is_demo();
    // The open "More markets" kind and the game it was opened on (another game opens collapsed).
    let mut more = use_signal(|| (String::new(), None::<MarketKind>));
    // The game on which a tap could not add a pick because the slip is full.
    let mut full = use_signal(|| None::<String>);
    // A live desk's clock moves: redraw the minutes since the start every 30 s.
    let mut tick = use_signal(|| 0u32);
    use_future(move || async move {
        if demo {
            return;
        }
        loop {
            gloo_timers::future::sleep(std::time::Duration::from_secs(30)).await;
            *tick.write() += 1;
        }
    });
    let _ = tick();
    let fx = kit::money::use_fx();
    let sol = kit::money::use_sol_rate();

    let s = state.read();
    let game = match &s.card {
        Load::Ready(c) => c.games.iter().find(|g| g.game_id == game_id).cloned(),
        Load::Loading => {
            return rsx! {
                section { class: "screen-event", id: "event",
                    kit::Skeleton { kind: kit::SkeletonKind::Match, count: 2, label: "Loading the game…" }
                }
            };
        }
        Load::Failed(e) => {
            return rsx! {
                section { class: "screen-event", id: "event",
                    p { class: "ev-note error", role: "alert", "Could not load tonight's games: {e}" }
                }
            };
        }
    };
    let Some(game) = game else {
        return rsx! {
            section { class: "screen-event", id: "event",
                p { class: "ev-note", "This game is no longer on the card." }
                Link { to: Route::Card {}, class: "ev-note-link", "See tonight's games" }
            }
        };
    };
    let loaded = s.sentiment.is_some();
    let sentiment = s
        .sentiment
        .as_ref()
        .and_then(|r| r.game(&game.game_id))
        .cloned();
    let now = s.now_ms(demo);
    let chosen: Vec<(String, Side)> = match &locked {
        Some(l) => vec![(l.symbol.clone(), l.side)],
        None => s
            .tray
            .legs
            .iter()
            .filter(|l| l.game_id == game.game_id)
            .map(|l| (l.symbol.clone(), l.side))
            .collect(),
    };
    let picks = s.tray.legs.len();
    let total = s.slip.bar_odds_u(&s.tray).map(format::odds);
    drop(s);

    let markets = markets_of(&game);
    // Started: only the winner market is still offered (U-2), even before the card refreshes.
    let live = game.in_play || game.start_ms <= now;
    let soccer = is_soccer(&game);
    let winner = winner_picks(&markets);
    let minutes = (now - game.started_ms.unwrap_or(game.start_ms)).max(0) / 60_000;
    let is_chosen = move |mp: &MarketPick| {
        chosen
            .iter()
            .any(|(sym, side)| *sym == mp.symbol && *side == mp.side)
    };
    let tones = outcome_tones(&winner.iter().map(|(_, p)| p.odds_u).collect::<Vec<_>>());
    let locked_kind = locked.as_ref().map(|l| l.kind);
    let line_kinds: Vec<MarketKind> = [MarketKind::Spread, MarketKind::Total]
        .into_iter()
        .filter(|_| !live)
        .filter(|k| locked_kind.is_none_or(|lk| lk == *k))
        .filter(|k| markets.iter().any(|m| m.kind == *k && !m.picks.is_empty()))
        .collect();
    let (more_game, more_kind) = more();
    let open = match locked_kind {
        // Locked on a line: that market, open; on the winner: none.
        Some(k) => line_kinds.contains(&k).then_some(k),
        None => more_kind.filter(|k| more_game == game.game_id && line_kinds.contains(k)),
    };
    let locked_symbol = locked.as_ref().map(|l| l.symbol.clone());
    let (home, away) = (game.home.name.clone(), game.away.name.clone());
    let title = if soccer {
        "Full Time Result (1-X-2)"
    } else {
        "Match Winner (1-2)"
    };

    // The tap: one leg per game, replacing any other pick on it, or out again if already in.
    let is_locked = locked.is_some();
    let tap = {
        let game = game.clone();
        move |(kind, mp): (MarketKind, MarketPick)| {
            if is_locked {
                return;
            }
            let tray = &mut state.write().tray;
            // An outcome card (1, X or 2, draws included) as the Sportsbook takes it (7.1a).
            let added = if kind == MarketKind::Winner {
                tray.toggle_outcome(&game, &mp)
            } else {
                choose(tray, &game, kind, &mp)
            };
            full.set((!added).then(|| game.game_id.clone()));
        }
    };
    let cta_label = match (&locked, picks, total) {
        (Some(_), _, _) => "Copy Schedina".to_string(),
        (None, 0, _) => "Select to bet".to_string(),
        (None, _, Some(t)) => format!("Bet • Total {t}"),
        (None, 1, None) => "Bet • 1 Pick".to_string(),
        (None, n, None) => format!("Bet • {n} Picks"),
    };
    let copy_target = locked.as_ref().map(|l| l.target.clone());
    let blocked = locked.as_ref().and_then(|l| l.blocked.clone());

    rsx! {
        section { class: "screen-event", id: "event", aria_labelledby: "event-title",
            h2 { class: "sr-only", id: "event-title", "{home} vs {away}" }
            LiveCard {
                game: game.clone(),
                live,
                soccer,
                minutes,
            }
            section { class: "ev-market", id: "event-market", aria_labelledby: "event-market-title",
                h2 { class: "sr-only", id: "event-market-title", "{title}" }
                div {
                    class: if winner.len() == 3 { "ev-outcomes three" } else { "ev-outcomes" },
                    id: "event-outcomes",
                    role: "group",
                    aria_label: "{title}",
                    for (i, (o, mp)) in winner.iter().cloned().enumerate() {
                        OutcomeCard {
                            key: "{o.code()}",
                            outcome: "",
                            team: outcome_name(&game, o),
                            value: odds_text(mp.odds_u),
                            tone: tones.get(i).copied().unwrap_or_default(),
                            selected: is_chosen(&mp),
                            disabled: mp.odds_u == 0 || (is_locked && !is_chosen(&mp)),
                            id: "event-outcome-{o.code().to_lowercase()}",
                            class: "ev-pick",
                            onclick: {
                                let mut tap = tap.clone();
                                move |_| tap((MarketKind::Winner, mp.clone()))
                            },
                        }
                    }
                }
                if winner.is_empty() {
                    p { class: "ev-note", "No price on this game right now." }
                }
            }
            if let Some(l) = locked.as_ref() {
                p { class: "ev-note ev-locked", id: "event-locked", role: "note",
                    SymIcon { sym: Sym::Lock, size: 16 }
                    " Locked to {l.who}'s pick. Copies take the same picks at today's values."
                }
            }
            if full().as_deref() == Some(game.game_id.as_str()) {
                p { class: "ev-note", role: "status", id: "event-full",
                    "A slip holds up to {MAX_LEGS} picks. Remove one to add this game."
                }
            }
            StakedSentiment { game: game.clone(), sentiment, loaded, fx, sol, now_ms: now }
            if !line_kinds.is_empty() {
                section { class: "ev-card ev-more", id: "event-more", aria_labelledby: "event-more-title",
                    div { class: "ev-more-head",
                        h2 { class: "ev-more-title", id: "event-more-title", "More markets" }
                        div { class: "ev-more-tabs", id: "event-tabs", role: "group", aria_label: "More markets",
                            for k in line_kinds.iter().copied() {
                                button {
                                    key: "{tab_id(k)}",
                                    r#type: "button",
                                    class: if open == Some(k) { "ev-more-tab active" } else { "ev-more-tab" },
                                    id: "event-tab-{tab_id(k)}",
                                    aria_pressed: "{open == Some(k)}",
                                    aria_controls: "event-lines",
                                    onclick: {
                                        let id = game.game_id.clone();
                                        move |_| {
                                            if is_locked {
                                                return;
                                            }
                                            let next = (open != Some(k)).then_some(k);
                                            more.set((id.clone(), next));
                                        }
                                    },
                                    "{tab_name(k)}"
                                    SymIcon { sym: if open == Some(k) { Sym::ExpandLess } else { Sym::ExpandMore }, size: 16 }
                                }
                            }
                        }
                    }
                    if let Some(kind) = open {
                        div { class: "ev-lines", id: "event-lines",
                            for (line, picks) in markets
                                .iter()
                                .filter(|m| m.kind == kind && !m.picks.is_empty())
                                // Locked: only the tipster's line.
                                .filter(|m| {
                                    locked_symbol
                                        .as_ref()
                                        .is_none_or(|sym| m.picks.iter().any(|p| &p.symbol == sym))
                                })
                                .map(|m| (m.line.clone().unwrap_or_default(), home_first(&m.picks)))
                            {
                                LinePair {
                                    key: "{line}",
                                    kind,
                                    on: picks.iter().map(&is_chosen).collect::<Vec<bool>>(),
                                    picks,
                                    ontap: tap.clone(),
                                }
                            }
                        }
                        p { class: "ev-caption", "{tab_caption(kind)}" }
                    }
                }
            }
            div { class: "ev-cta",
                CtaShown {}
                PurpleCTA {
                    label: cta_label,
                    icon: if is_locked { Sym::ContentCopy } else if picks == 0 { Sym::PostAdd } else { Sym::Receipt },
                    id: "event-cta",
                    disabled: blocked.is_some(),
                    onclick: move |_| {
                        if let Some(t) = copy_target.clone() {
                            super::tray::open_copy_drawer(t);
                            return;
                        }
                        if picks == 0 {
                            document::eval(SCROLL_JS);
                            return;
                        }
                        super::tray::open_drawer();
                    },
                }
                if let Some(why) = blocked.as_ref() {
                    p { class: "ev-note", id: "event-copy-why", role: "status", "Copies closed: {why}." }
                }
            }
        }
    }
}

/// Marks the event CTA as shown while it is mounted (see [`cta_shown`]).
#[component]
fn CtaShown() -> Element {
    use_effect(|| *CTA_SHOWN.write() = true);
    use_drop(|| *CTA_SHOWN.write() = false);
    rsx! {}
}

/// The share button of the "Event Detail" header (the mockup's `share`, left of the avatar),
/// passed as the shell's `kit::SubHeader { end }` (D-500): the system share sheet where there is
/// one, else the page's link is copied to the clipboard.
#[component]
pub fn EventShare(game_id: String) -> Element {
    let state = use_app_state();
    let text = match &state.read().card {
        Load::Ready(c) => c
            .games
            .iter()
            .find(|g| g.game_id == game_id)
            .map(|g| format!("{} vs {} on BetFi", g.home.name, g.away.name)),
        _ => None,
    }
    .unwrap_or_else(|| "BetFi".to_string());
    let share = move |_| {
        let text = text.clone();
        spawn(async move {
            let js = format!(
                "const url = location.href; const text = {}; \
                 if (navigator.share) {{ try {{ await navigator.share({{ title: text, url }}); }} catch (e) {{}} return false; }} \
                 if (navigator.clipboard) {{ try {{ await navigator.clipboard.writeText(url); return true; }} catch (e) {{}} }} \
                 return false;",
                serde_json::to_string(&text).unwrap_or_else(|_| "\"BetFi\"".into())
            );
            let _ = document::eval(&js).join::<bool>().await;
        });
    };
    rsx! {
        span { class: "ev-share-wrap",
            button {
                r#type: "button",
                class: "ev-share",
                id: "event-share",
                aria_label: "Share this game",
                title: "Share this game",
                onclick: share,
                SymIcon { sym: Sym::Share, size: 20 }
            }
        }
    }
}

/// The live card (V-1): LIVE IN-PLAY once started, the league and the clock slot; side 1 (home)
/// on the left, side 2 (away) on the right with their badges; between them the live score, or the
/// start time before the game starts.
#[component]
fn LiveCard(game: Game, live: bool, soccer: bool, minutes: i64) -> Element {
    let sport = if soccer { Sym::Soccer } else { Sym::Basketball };
    let league = league(&game);
    let team = |o: Outcome| {
        let (side, name) = match o {
            Outcome::Away => ("second", game.away.name.clone()),
            _ => ("first", game.home.name.clone()),
        };
        let logo = kit::team_logo(&name, soccer);
        rsx! {
            div { class: "ev-team {side}",
                span { class: if logo.is_some() { "ev-badge logo" } else { "ev-badge" }, "aria-hidden": "true",
                    if let Some(src) = logo {
                        img { src: "{src}", alt: "", decoding: "async", width: "36", height: "36" }
                    } else {
                        SymIcon { sym: sport, size: 20 }
                    }
                }
                span { class: "ev-team-name", "{name}" }
            }
        }
    };
    let score = game.score.clone().filter(|_| live);
    let lead = |mine: u32, other: u32| if mine > other { "lead" } else { "" };
    let (clock_icon, clock) = if live {
        let c = format::match_clock(
            score.as_ref().and_then(|s| s.period.as_deref()),
            score.as_ref().and_then(|s| s.elapsed.as_deref()),
            minutes,
        );
        (Sym::Timer, format!("Live · {c}"))
    } else {
        (Sym::Schedule, format::day(game.start_ms))
    };
    let start_word = if soccer { "Kick-off" } else { "Tip-off" };
    let summary = match &score {
        Some(s) => format!(
            "{clock}. {} {}, {} {}",
            game.home.name, s.home, game.away.name, s.away
        ),
        None if live => clock.clone(),
        None => format!("{start_word} {}", format::when(game.start_ms)),
    };
    rsx! {
        div { class: "ev-card ev-live", id: "event-live",
            div { class: "ev-live-top",
                span { class: "ev-live-tags",
                    if live {
                        Badge { kind: BadgeKind::LiveInPlay }
                    }
                    span { class: "ev-league", id: "event-league", "{league}" }
                }
                span { class: "ev-clock-slot", id: "event-clock",
                    SymIcon { sym: clock_icon, size: 16 }
                    span { "{clock}" }
                }
            }
            div { class: "ev-teams", id: "event-teams",
                {team(Outcome::Home)}
                div { class: "ev-mid", role: "img", aria_label: "{summary}",
                    if let Some(sc) = &score {
                        span { class: "ev-score", "aria-hidden": "true",
                            span { class: "ev-score-n {lead(sc.home, sc.away)}", "{sc.home}" }
                            span { class: "ev-score-sep", ":" }
                            span { class: "ev-score-n {lead(sc.away, sc.home)}", "{sc.away}" }
                        }
                    } else if live {
                        span { class: "ev-score", "aria-hidden": "true", "Live" }
                    } else {
                        span { class: "ev-score", "aria-hidden": "true", "{format::clock(game.start_ms)}" }
                    }
                }
                {team(Outcome::Away)}
            }
        }
    }
}

/// One spread or total line in "More markets": its two sides, each named by its label
/// ("Real Madrid −3.5", "Over 221.5"), values toned as a market.
#[component]
fn LinePair(
    kind: MarketKind,
    picks: Vec<MarketPick>,
    on: Vec<bool>,
    ontap: EventHandler<(MarketKind, MarketPick)>,
) -> Element {
    let tones = outcome_tones(&picks.iter().map(|p| p.odds_u).collect::<Vec<_>>());
    rsx! {
        div { class: "ev-line", role: "group",
            for (i, mp) in picks.into_iter().enumerate() {
                OutcomeButton {
                    key: "{i}",
                    outcome: pretty(&mp.label),
                    value: odds_text(mp.odds_u),
                    tone: tones.get(i).copied().unwrap_or_default(),
                    selected: on.get(i).copied().unwrap_or(false),
                    disabled: mp.odds_u == 0,
                    hero: true,
                    aria_label: "{pretty(&mp.label)}, {odds_text(mp.odds_u)}",
                    class: "ev-pick",
                    onclick: move |_| ontap.call((kind, mp.clone())),
                }
            }
        }
    }
}

/// Put a spread or total pick `mp` in the slip as this game's one leg: replaces another pick on
/// the game in place, or takes the pick out when it is already there. `false` when the slip is
/// full (nothing changed). Winner picks go through `Tray::toggle_outcome` (7.1a).
fn choose(tray: &mut Tray, game: &Game, kind: MarketKind, mp: &MarketPick) -> bool {
    let at = tray.legs.iter().position(|l| l.game_id == game.game_id);
    if let Some(i) = at
        && tray.legs[i].symbol == mp.symbol
        && tray.legs[i].side == mp.side
    {
        tray.legs.remove(i);
        return true;
    }
    // A total has no team: the desk's placeholder is Over = away, Under = home (D-321).
    let pick = mp.team.unwrap_or(match mp.side {
        Side::Long => Pick::Away,
        Side::Short => Pick::Home,
    });
    let other = |p: Pick| match p {
        Pick::Away => game.home.name.clone(),
        Pick::Home => game.away.name.clone(),
    };
    let (team, opponent) = match kind {
        MarketKind::Winner => (game.team(pick).name.clone(), other(pick)),
        MarketKind::Spread => (pretty(&mp.label), other(pick)),
        MarketKind::Total | MarketKind::Other => (pretty(&mp.label), matchup(game)),
    };
    let leg = TrayLeg {
        game_id: game.game_id.clone(),
        symbol: mp.symbol.clone(),
        pick,
        side: mp.side,
        team,
        opponent,
        start_ms: game.start_ms,
        card_odds_u: mp.odds_u,
        kind,
    };
    match at {
        Some(i) => tray.legs[i] = leg,
        None if tray.legs.len() >= MAX_LEGS => return false,
        None => tray.legs.push(leg),
    }
    true
}

/// The winner market's picks in the 1-X-2 order (home, draw, away).
fn winner_picks(markets: &[Market]) -> Vec<(Outcome, MarketPick)> {
    let mut out: Vec<(Outcome, MarketPick)> = markets
        .iter()
        .find(|m| m.kind == MarketKind::Winner)
        .map(|m| {
            m.picks
                .iter()
                .map(|p| (Outcome::of(p), p.clone()))
                .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|(o, _)| *o);
    out.dedup_by_key(|(o, _)| *o);
    out
}

/// A market's picks home team first (the 1-2 order); a total's Over and Under keep theirs.
fn home_first(picks: &[MarketPick]) -> Vec<MarketPick> {
    let mut out = picks.to_vec();
    out.sort_by_key(|p| match p.team {
        Some(Pick::Home) => 0,
        Some(Pick::Away) => 1,
        None => 0,
    });
    out
}

/// The game's markets, winner first. A desk from before L6 sends no `markets`: the winner market
/// is then built from `picks`, named by the teams.
fn markets_of(game: &Game) -> Vec<Market> {
    let mut out: Vec<Market> = Vec::new();
    let has_winner = game
        .markets
        .iter()
        .any(|m| m.kind == MarketKind::Winner && !m.picks.is_empty());
    if !has_winner && !game.picks.is_empty() {
        out.push(Market {
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
        });
    }
    out.extend(
        game.markets
            .iter()
            .filter(|m| m.kind != MarketKind::Other)
            .cloned(),
    );
    out.sort_by_key(|m| match m.kind {
        MarketKind::Winner => 0,
        MarketKind::Spread => 1,
        _ => 2,
    });
    out
}

/// A soccer game (V-2): the desk says so, or its winner market has a draw.
fn is_soccer(game: &Game) -> bool {
    game.sport.as_deref() == Some("soccer")
        || game
            .markets
            .iter()
            .any(|m| m.kind == MarketKind::Winner && m.picks.iter().any(|p| p.team.is_none()))
}

/// An outcome card's name: the team, or "Draw".
fn outcome_name(game: &Game, o: Outcome) -> String {
    match o {
        Outcome::Home => game.home.name.clone(),
        Outcome::Away => game.away.name.clone(),
        Outcome::Draw => "Draw".to_string(),
    }
}

/// "Celtics vs Knicks": a total's matchup, the teams in their listed order (`Game::listed`), as
/// the slip's design seed names it too.
pub(super) fn matchup(game: &Game) -> String {
    let [a, b] = game.listed().map(|p| short_name(game, p));
    format!("{a} vs {b}")
}

/// A team's short name as the desk labels its winner pick ("76ers"), else its full name.
fn short_name(game: &Game, pick: Pick) -> String {
    game.markets
        .iter()
        .filter(|m| m.kind == MarketKind::Winner)
        .flat_map(|m| m.picks.iter())
        .find(|p| p.team == Some(pick) && !p.label.is_empty())
        .map(|p| p.label.clone())
        .unwrap_or_else(|| game.team(pick).name.clone())
}

/// The competition: the desk's `league` (L7-1), else from the Pascal symbol's prefix.
fn league(game: &Game) -> String {
    if let Some(l) = game.league.as_deref().filter(|l| !l.is_empty()) {
        return l.to_string();
    }
    let s = &game.symbol;
    if s.starts_with("NBA_") {
        "NBA"
    } else if s.starts_with("EL_") {
        "EuroLeague"
    } else if s.starts_with("EPL_") {
        "EPL"
    } else if is_soccer(game) {
        "Soccer"
    } else {
        "Basketball"
    }
    .to_string()
}

/// A label with a typographic minus: "76ers -3.5" → "76ers −3.5".
fn pretty(label: &str) -> String {
    label.replace(" -", " \u{2212}")
}

fn odds_text(odds_u: u64) -> String {
    if odds_u == 0 {
        "–".to_string()
    } else {
        format::odds(odds_u)
    }
}

fn tab_name(k: MarketKind) -> &'static str {
    match k {
        MarketKind::Spread => "Spread",
        _ => "Total",
    }
}

fn tab_id(k: MarketKind) -> &'static str {
    match k {
        MarketKind::Spread => "spread",
        _ => "total",
    }
}

fn tab_caption(k: MarketKind) -> &'static str {
    match k {
        MarketKind::Spread => {
            "A team covers when its points plus the line beat the other team's. One pick per game: a new one replaces the last."
        }
        _ => {
            "Both teams' points added up, over or under the line. One pick per game: a new one replaces the last."
        }
    }
}

/// The market that holds the pick `symbol`/`side` on `game`, as buttons in the page's order
/// (1-X-2 for the winner, home first for a spread, Over and Under for a total), each with its
/// label: "1", "X", "2", "Real Madrid −3.5", "Over 158.5". `None` once the card no longer prices
/// that market (L9: the locked bars of a tipster's parlay).
pub(super) fn market_of_pick(
    game: &Game,
    symbol: &str,
    side: Side,
) -> Option<(MarketKind, Vec<(String, MarketPick)>)> {
    let markets = markets_of(game);
    let m = markets
        .iter()
        .find(|m| m.picks.iter().any(|p| p.symbol == symbol && p.side == side))?;
    let buttons = match m.kind {
        MarketKind::Winner => winner_picks(&markets)
            .into_iter()
            .map(|(o, p)| (o.code().to_string(), p))
            .collect(),
        _ => home_first(&m.picks)
            .into_iter()
            .map(|p| (pretty(&p.label), p))
            .collect(),
    };
    Some((m.kind, buttons))
}
