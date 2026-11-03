//! Picks and tips as rows and cards (7.0a): the drawer's pick row, the copy confirm's leg card,
//! a receipt's leg row, the event page's outcome card, and a tipster's tip card.

use dioxus::prelude::*;

use super::icons::{Sym, SymIcon};
use super::market::OutcomeTone;
use super::social::{Badge, BadgeKind, TeamChips};

fn tone_class(t: OutcomeTone) -> &'static str {
    match t {
        OutcomeTone::Neutral => "tone-neutral",
        OutcomeTone::Lowest => "tone-lowest",
        OutcomeTone::Highest => "tone-highest",
    }
}

/// A pick in the bet drawer: the team in bold with "• EUROLEAGUE" in green, the line under it
/// ("vs Fenerbahçe"), the value toned across the slip's picks, and a remove ✕
/// (`onremove`, named "Remove <team>").
#[component]
pub fn PickRow(
    team: String,
    league: Option<String>,
    detail: String,
    value: String,
    #[props(default)] tone: OutcomeTone,
    #[props(default)] live: bool,
    onremove: Option<EventHandler<MouseEvent>>,
) -> Element {
    let remove = format!("Remove {team}");
    rsx! {
        div { class: "k-pick",
            div { class: "k-pick-main",
                div { class: "k-pick-top",
                    span { class: "k-pick-team", "{team}" }
                    if let Some(l) = league {
                        span { class: "k-pick-league", "• {l}" }
                    }
                    if live {
                        Badge { kind: BadgeKind::Live }
                    }
                }
                span { class: "k-pick-detail", "{detail}" }
            }
            div { class: "k-pick-end",
                span { class: "k-pick-value {tone_class(tone)}", "{value}" }
                if let Some(h) = onremove {
                    button {
                        r#type: "button",
                        class: "k-pick-remove",
                        aria_label: "{remove}",
                        title: "{remove}",
                        onclick: move |e| h.call(e),
                        SymIcon { sym: Sym::Close, size: 16 }
                    }
                }
            }
        }
    }
}

/// A leg of a copied tip (copy confirm): the sport icon and "EUROLEAGUE • Q3 04:12", the
/// LIVE IN-PLAY or PREMATCH tag, the team, "vs Fenerbahçe Beko", and the boxed "PICK (1)" value.
#[component]
pub fn LegCard(
    sport: Sym,
    meta: String,
    #[props(default)] live: bool,
    team: String,
    detail: String,
    pick: String,
    value: String,
    #[props(default)] tone: OutcomeTone,
) -> Element {
    rsx! {
        div { class: "k-leg-card",
            div { class: "k-leg-card-top",
                span { class: "k-leg-card-meta", SymIcon { sym: sport, size: 16 } "{meta}" }
                Badge { kind: if live { BadgeKind::LiveInPlay } else { BadgeKind::Prematch } }
            }
            div { class: "k-leg-card-body",
                div { class: "k-leg-card-names",
                    p { class: "k-leg-card-team", "{team}" }
                    p { class: "k-leg-card-detail", "{detail}" }
                }
                div { class: "k-leg-card-pick",
                    span { class: "k-leg-card-pick-label", "{pick}" }
                    span { class: "k-leg-card-pick-value {tone_class(tone)}", "{value}" }
                }
            }
        }
    }
}

/// A leg on a receipt or a celebration: a status dot, the pick ("Panathinaikos ML"), the line
/// under it ("vs Olympiacos • EuroLeague"), the value. `won`/`lost` colour the dot (green,
/// red); default is green-pending as drawn.
#[component]
pub fn LegRow(
    title: String,
    detail: String,
    value: String,
    #[props(default)] tone: OutcomeTone,
    #[props(default)] lost: bool,
) -> Element {
    let l = if lost { "lost" } else { "" };
    rsx! {
        div { class: "k-leg-row",
            span { class: "k-leg-row-dot {l}", "aria-hidden": "true" }
            span { class: "k-leg-row-main",
                span { class: "k-leg-row-title", "{title}" }
                span { class: "k-leg-row-detail", "{detail}" }
            }
            span { class: "k-leg-row-value {tone_class(tone)}", "{value}" }
        }
    }
}

/// The event page's big outcome (`betfi_event_detail_market_volume`): an optional label
/// ("OUTCOME 1"; the event page passes none, 31 Oct, and the team takes the label's row), the team,
/// the value at 28 px coloured by tone; selected draws the gradient border and the tick. A
/// toggle button (`aria-pressed`). The word "odds" is not shown (guidelines §2).
#[component]
pub fn OutcomeCard(
    outcome: String,
    team: String,
    value: String,
    #[props(default)] tone: OutcomeTone,
    #[props(default)] selected: bool,
    #[props(default)] disabled: bool,
    id: Option<String>,
    #[props(default)] class: String,
    onclick: EventHandler<MouseEvent>,
) -> Element {
    let sel = if selected { "selected" } else { "" };
    rsx! {
        button {
            r#type: "button",
            class: "k-outcome-card {tone_class(tone)} {sel} {class}",
            id,
            disabled,
            aria_pressed: "{selected}",
            onclick: move |e| onclick.call(e),
            span { class: "k-outcome-card-top",
                if outcome.is_empty() {
                    span { class: "k-outcome-card-team", "{team}" }
                } else {
                    span { class: "k-outcome-card-label", "{outcome}" }
                }
                SymIcon { sym: if selected { Sym::CheckCircle } else { Sym::TouchApp }, size: 18 }
            }
            if !outcome.is_empty() {
                span { class: "k-outcome-card-team", "{team}" }
            }
            span { class: "k-outcome-card-value", "{value}" }
        }
    }
}

/// A tipster's open tip (profile "Active Parlays"): the title with its "3 LEGS" badge, "€500.00
/// Staked • €28.4k Pool", the team chips and the total in a box; `selected` raises it with a
/// purple edge and shows `detail` (the legs, "Real Madrid ML • …") and `pool` (the "View Pool
/// (1,284) →" action) under a rule. The card itself is a button choosing the tip (`onclick`).
#[component]
pub fn TipCard(
    title: String,
    legs: u32,
    staked: String,
    pool: String,
    teams: Vec<String>,
    total: String,
    #[props(default)] selected: bool,
    detail: Option<String>,
    pool_action: Option<Element>,
    id: Option<String>,
    onclick: EventHandler<MouseEvent>,
) -> Element {
    let sel = if selected { "selected" } else { "" };
    rsx! {
        div { class: "k-tip {sel}", id,
            button {
                r#type: "button",
                class: "k-tip-main",
                aria_pressed: "{selected}",
                onclick: move |e| onclick.call(e),
                span { class: "k-tip-names",
                    span { class: "k-tip-title-row",
                        span { class: "k-tip-title", "{title}" }
                        Badge { kind: BadgeKind::Legs(legs) }
                    }
                    span { class: "k-tip-money", "{staked} Staked • {pool} Pool" }
                }
                span { class: "k-tip-end",
                    TeamChips { codes: teams }
                    span { class: "k-tip-total", "{total}" }
                }
            }
            if selected && (detail.is_some() || pool_action.is_some()) {
                div { class: "k-tip-detail",
                    span { "{detail.clone().unwrap_or_default()}" }
                    {pool_action}
                }
            }
        }
    }
}
