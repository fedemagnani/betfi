//! A tipster's parlay (`/copy/:id`, L9, user request 1 Nov) and its locked games
//! (`/copy/:id/event/:game_id`).
//!
//! The parlay page: the tipster (to their page), the parlay's title, its logos and the total it
//! was placed at; "Copied by (n)", which opens the copiers sheet ([`super::copiers`]); then one
//! bar per game, the Sportsbook's match card with the tipster's pick selected at today's value and
//! the other outcomes off (the picks are locked); a bar opens that game's locked event page. The
//! pinned "Copy Schedina" opens the bet drawer in copy mode (`tray::open_copy_drawer`): the same
//! stake slider and coin pill as a bet, the tip's legs priced today with the 2% copy fee. The
//! button is off, with the reason, whenever the program would refuse the copy (D-181).
//!
//! The locked event page is the event page (`event::EventView`) with a [`LockedPick`]: the
//! tipster's pick fixed, its CTA the same copy drawer.

use dioxus::prelude::*;

use super::copiers::CopiersSheet;
use super::{ParlayLogos, Who, count, parlay_teams, tip_title, use_clock_tick};
use crate::Route;
use crate::desk::DeskError;
use crate::desk::types::{Card, Game, MarketKind, Pick, Ticket, TicketLeg};
use crate::flows;
use crate::format;
use crate::state::{Load, TrayLeg};
use crate::ui::copy::copy_blocker;
use crate::ui::event::{EventView, LockedPick, market_of_pick};
use crate::ui::kit::{
    self, Avatar, AvatarTone, MatchCard, OutcomeButton, PurpleCTA, Sym, SymIcon, TeamLine,
    outcome_tones,
};
use crate::ui::tray::{CopyTarget, open_copy_drawer};
use crate::ui::{use_app_state, use_desk};

// ---------------------------------------------------------------- the tip and its copy target

/// The tip `id`: the stream's copy when the viewer holds it (kept current), else the desk's.
fn use_tip(id: String) -> Option<Result<Ticket, DeskError>> {
    let desk = use_desk();
    let state = use_app_state();
    let fetch_id = id.clone();
    let fetched = use_resource(use_reactive!(|(fetch_id,)| {
        let desk = desk.clone();
        async move { desk.ticket(&fetch_id).await }
    }));
    let live = state.read().tickets.get(&id).cloned();
    match live {
        Some(t) => Some(Ok(t)),
        None => fetched.read().clone(),
    }
}

/// The tipster's handle, once the desk answers (`GET /profile/{wallet}`).
fn use_handle(wallet: Option<String>) -> Option<String> {
    let desk = use_desk();
    let profile = use_resource(use_reactive!(|(wallet,)| {
        let desk = desk.clone();
        async move {
            match wallet {
                Some(w) => desk.public_profile(&w).await.ok(),
                None => None,
            }
        }
    }));
    profile.read().clone().flatten().and_then(|p| p.handle)
}

fn card_of(state: &crate::state::AppState) -> Option<Card> {
    match &state.card {
        Load::Ready(c) => Some(c.clone()),
        _ => None,
    }
}

fn game_of<'a>(card: Option<&'a Card>, leg: &TicketLeg) -> Option<&'a Game> {
    card?.games.iter().find(|g| g.game_id == leg.game_id)
}

/// A leg as the drawer holds a pick (locked): its names as the event page writes them, and the
/// card's value now.
fn tray_leg(leg: &TicketLeg, card: Option<&Card>) -> TrayLeg {
    let game = game_of(card, leg);
    let kind = leg.kind.unwrap_or(MarketKind::Winner);
    let label = leg.label.clone().filter(|l| !l.trim().is_empty());
    let draw = label
        .as_deref()
        .is_some_and(|l| l.eq_ignore_ascii_case("draw"));
    let matchup = format!("{} vs {}", leg.home, leg.away);
    let (team, opponent) = match kind {
        MarketKind::Winner if draw => ("Draw".to_string(), matchup),
        MarketKind::Winner => (leg.picked_team().to_string(), leg.opponent().to_string()),
        MarketKind::Spread => (
            label.unwrap_or_else(|| leg.picked_team().to_string()),
            leg.opponent().to_string(),
        ),
        _ => (label.unwrap_or_else(|| matchup.clone()), matchup),
    };
    let card_odds_u = game
        .and_then(|g| market_of_pick(g, &leg.symbol, leg.side))
        .and_then(|(_, picks)| {
            picks
                .into_iter()
                .find(|(_, p)| p.symbol == leg.symbol && p.side == leg.side)
                .map(|(_, p)| p.odds_u)
        })
        .unwrap_or(0);
    TrayLeg {
        game_id: leg.game_id.clone(),
        symbol: leg.symbol.clone(),
        pick: leg.pick,
        side: leg.side,
        team: team.replace(" -", " \u{2212}"),
        opponent,
        start_ms: leg.start_ms,
        card_odds_u,
        kind,
    }
}

/// What the copy drawer copies: the tip, its legs locked, as the signed copy must name them.
fn copy_target(t: &Ticket, card: Option<&Card>) -> CopyTarget {
    CopyTarget {
        ticket: t.ticket.clone(),
        legs: t.legs.iter().map(|l| tray_leg(l, card)).collect(),
        shown: flows::ticket_shown_legs(t),
    }
}

// ---------------------------------------------------------------- the parlay page

/// `/copy/:id`.
#[component]
pub fn CopyTip(id: String) -> Element {
    let state = use_app_state();
    let tip = use_tip(id);
    let card = card_of(&state.read());
    rsx! {
        section { class: "screen-parlay", id: "parlay",
            match tip {
                None => rsx! { kit::Skeleton { kind: kit::SkeletonKind::Match, count: 3, label: "Loading the parlay…" } },
                Some(Ok(t)) => rsx! { ParlayBody { key: "{t.ticket}", t } },
                Some(Err(e)) => rsx! {
                    // `#copy-missing`: scripts wait for it or the copy CTA, so a tip this build
                    // does not hold fails at once instead of timing out (polish 16).
                    p { class: "tp-status error", id: "copy-missing", role: "alert", "{e.plain(card.as_ref())}" }
                    kit::LinkButton { to: Route::Tipsters {}, variant: kit::ButtonVariant::Secondary, "Tipsters" }
                },
            }
        }
    }
}

#[component]
fn ParlayBody(t: Ticket) -> Element {
    let state = use_app_state();
    let desk = use_desk();
    use_clock_tick();
    let mut sheet = use_signal(|| false);
    let tipster = t.tipster.clone().unwrap_or_else(|| t.user.clone());
    let handle = use_handle(Some(tipster.clone()));
    let who = Who::new(handle, &tipster);
    // The tip as the tipster's page lists it: its copies and pool (`/ticket` carries neither).
    let w = tipster.clone();
    let profile_desk = desk.clone();
    let profile = use_resource(use_reactive!(|(w,)| {
        let desk = profile_desk.clone();
        async move { desk.tipster(&w).await.ok() }
    }));
    let listed = profile
        .read()
        .clone()
        .flatten()
        .and_then(|p| p.open.into_iter().find(|o| o.ticket == t.ticket));
    let t = Ticket {
        copies: listed.as_ref().and_then(|o| o.copies).or(t.copies),
        pool_u: listed.as_ref().and_then(|o| o.pool_u).or(t.pool_u),
        teams: listed
            .map(|o| o.teams)
            .filter(|v| !v.is_empty())
            .unwrap_or(t.teams),
        ..t
    };
    let n = t.copies.unwrap_or(0);
    let s = state.read();
    let card = card_of(&s);
    let now = s.now_ms(desk.is_demo());
    drop(s);
    let blocker = copy_blocker(&t);
    let target = copy_target(&t, card.as_ref());
    let title = tip_title(&t);
    rsx! {
        div { class: "pl-head", id: "parlay-head",
            Link { to: Route::TipsterPage { wallet: tipster.clone() }, class: "pl-who", id: "parlay-tipster",
                Avatar {
                    initials: who.initials.clone(),
                    size: 36,
                    tone: if who.green { AvatarTone::Green } else { AvatarTone::Purple },
                }
                span { class: "pl-who-name", "{who.name}" }
                SymIcon { sym: Sym::ChevronRight, size: 18 }
            }
            div { class: "pl-title-row",
                h1 { class: "pl-title", id: "parlay-title", "{title}" }
                ParlayLogos { teams: parlay_teams(&t), large: true }
            }
            p { class: "pl-meta",
                "{format::picks(t.n)} \u{2022} placed at "
                b { "{format::odds(t.odds_offered_u)}" }
            }
            button {
                r#type: "button",
                class: "pl-copiers",
                id: "parlay-copiers",
                aria_haspopup: "dialog",
                onclick: move |_| sheet.set(true),
                SymIcon { sym: Sym::Group, size: 18 }
                "Copied by {count(n)}"
                SymIcon { sym: Sym::ChevronRight, size: 18 }
            }
        }
        ol { class: "pl-legs", id: "parlay-legs", aria_label: "The games in this parlay",
            for leg in t.legs.iter() {
                li { key: "{leg.idx}", class: "pl-leg", "data-game-id": "{leg.game_id}",
                    LockedBar { tip: t.ticket.clone(), leg: leg.clone(), game: game_of(card.as_ref(), leg).cloned(), now_ms: now }
                }
            }
        }
        div { class: "pl-dock", id: "parlay-dock",
            PurpleCTA {
                label: "Copy Schedina",
                icon: Sym::ContentCopy,
                arrow: true,
                disabled: blocker.is_some(),
                id: "tip-copy",
                onclick: move |_| open_copy_drawer(target.clone()),
            }
            if let Some(why) = blocker.as_ref() {
                p { class: "pl-why", id: "tip-copy-why", role: "status", "Copies closed: {why}." }
            }
        }
        if sheet() {
            CopiersSheet { tipster: tipster.clone(), tip: t.clone(), onclose: move |_| sheet.set(false) }
        }
    }
}

/// One game of the parlay as the Sportsbook draws it, locked: the market of the tipster's pick
/// with that pick selected and the others off, at today's values; the header opens the game's
/// locked event page. A game the card no longer lists shows the pick alone.
#[component]
fn LockedBar(tip: String, leg: TicketLeg, game: Option<Game>, now_ms: i64) -> Element {
    let to = Route::CopyEvent {
        id: tip,
        game_id: leg.game_id.clone(),
    };
    let soccer = game
        .as_ref()
        .and_then(|g| g.sport.as_deref())
        .is_some_and(|s| s.eq_ignore_ascii_case("soccer"));
    let line = |name: &str| TeamLine {
        logo: kit::team_logo(name, soccer),
        ..TeamLine::named(name.to_string())
    };
    let Some(g) = game else {
        let (pick, _) = super::leg_line(&leg);
        return rsx! {
            MatchCard {
                meta: format::clock(leg.start_ms),
                to,
                first: line(&leg.home),
                second: line(&leg.away),
                class: "pl-bar",
                OutcomeButton { outcome: pick, value: "\u{2013}".to_string(), selected: true, disabled: true, class: "pl-pick", onclick: |_| {} }
            }
        };
    };
    let live = g.in_play || g.start_ms <= now_ms;
    let league = g.league.clone().unwrap_or_default();
    let meta = if live {
        league
    } else if league.is_empty() {
        format::clock(g.start_ms)
    } else {
        format!("{league} \u{2022} {}", format::clock(g.start_ms))
    };
    let buttons = market_of_pick(&g, &leg.symbol, leg.side);
    let tones = buttons
        .as_ref()
        .map(|(_, b)| outcome_tones(&b.iter().map(|(_, p)| p.odds_u).collect::<Vec<_>>()))
        .unwrap_or_default();
    let (first, second) = (line(&g.home.name), line(&g.away.name));
    let kind_note = match buttons.as_ref().map(|(k, _)| *k) {
        Some(MarketKind::Spread) => Some("Spread"),
        Some(MarketKind::Total) => Some("Total"),
        _ => None,
    };
    rsx! {
        MatchCard {
            meta,
            live,
            right: kind_note.map(str::to_string),
            to,
            first,
            second,
            class: "pl-bar",
            match buttons {
                Some((_, picks)) => rsx! {
                    for (i, (label, p)) in picks.into_iter().enumerate() {
                        {
                            let mine = p.symbol == leg.symbol && p.side == leg.side;
                            let value = if p.odds_u > 0 { format::odds(p.odds_u) } else { "\u{2013}".to_string() };
                            let who = match p.team {
                                Some(Pick::Home) => g.home.name.clone(),
                                Some(Pick::Away) => g.away.name.clone(),
                                None => p.label.clone(),
                            };
                            rsx! {
                                OutcomeButton {
                                    key: "{i}",
                                    outcome: label,
                                    value: value.clone(),
                                    tone: tones.get(i).copied().unwrap_or_default(),
                                    selected: mine,
                                    disabled: !mine,
                                    aria_label: if mine { format!("The tipster's pick: {who}, {value}") } else { format!("{who}, {value} (locked)") },
                                    class: if mine { "pl-pick mine" } else { "pl-pick" },
                                    onclick: |_| {},
                                }
                            }
                        }
                    }
                },
                None => rsx! {
                    OutcomeButton { outcome: super::leg_line(&leg).0, value: "\u{2013}".to_string(), selected: true, disabled: true, class: "pl-pick mine", onclick: |_| {} }
                },
            }
        }
    }
}

// ---------------------------------------------------------------- a locked game

/// `/copy/:id/event/:game_id`: the event page with the tipster's pick on that game fixed.
#[component]
pub fn CopyEvent(id: String, game_id: String) -> Element {
    let state = use_app_state();
    let tip = use_tip(id.clone());
    let tipster = match &tip {
        Some(Ok(t)) => Some(t.tipster.clone().unwrap_or_else(|| t.user.clone())),
        _ => None,
    };
    let handle = use_handle(tipster.clone());
    let s = state.read();
    let card = card_of(&s);
    drop(s);
    match tip {
        None => rsx! {
            section { class: "screen-event", id: "event",
                p { class: "ev-note", role: "status", "Loading the parlay…" }
            }
        },
        Some(Err(e)) => rsx! {
            section { class: "screen-event", id: "event",
                p { class: "ev-note error", id: "copy-missing", role: "alert", "{e.plain(card.as_ref())}" }
            }
        },
        Some(Ok(t)) => {
            let Some(leg) = t.legs.iter().find(|l| l.game_id == game_id) else {
                return rsx! {
                    section { class: "screen-event", id: "event",
                        p { class: "ev-note", "This game is not in the parlay." }
                        Link { to: Route::CopyTip { id }, class: "ev-note-link", "Back to the parlay" }
                    }
                };
            };
            let who = Who::new(handle, tipster.as_deref().unwrap_or_default());
            let locked = LockedPick {
                symbol: leg.symbol.clone(),
                side: leg.side,
                kind: leg.kind.unwrap_or(MarketKind::Winner),
                who: who.name,
                target: copy_target(&t, card.as_ref()),
                blocked: copy_blocker(&t),
            };
            rsx! { EventView { game_id, locked: Some(locked) } }
        }
    }
}
