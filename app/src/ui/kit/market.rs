//! The sportsbook's pieces (7.0a, BetFi C2 `betfi_sportsbook_equal_odds`): sport tabs, the match
//! card (upcoming row and live hero), the outcome button with its value tone, a section title
//! and the green action link.

use dioxus::prelude::*;
use dioxus_primitives::toggle_group::{ToggleGroup, ToggleItem};

use super::icons::{Sym, SymIcon};
use crate::Route;

/// A sport the board lists (V-2: basketball 1–2, soccer 1–X–2; no tennis).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sport {
    Basketball,
    Soccer,
}

impl Sport {
    pub const ALL: [Sport; 2] = [Sport::Basketball, Sport::Soccer];

    /// "Basketball", "Soccer" (31 Oct, user request: no "(1–2)" / "(1–X–2)"; the outcome buttons
    /// say it).
    pub fn label(self) -> &'static str {
        match self {
            Sport::Basketball => "Basketball",
            Sport::Soccer => "Soccer",
        }
    }

    pub fn sym(self) -> Sym {
        match self {
            Sport::Basketball => Sym::Basketball,
            Sport::Soccer => Sym::Soccer,
        }
    }

    /// The desk's `sport` value.
    pub fn key(self) -> &'static str {
        match self {
            Sport::Basketball => "basketball",
            Sport::Soccer => "soccer",
        }
    }
}

/// The sport chips under the ticker: the active one raised with its icon in green. `sports` is
/// what the card offers (a sport with no game is left out by the caller, risk "few soccer
/// games"). Each chip is a toggle button (`aria-pressed`). `counts`: how many picks on the slip
/// come from each sport; a non-zero count sits on the chip's top edge as a small Solana-green
/// number (user request, 31 Oct).
#[component]
pub fn SportTabs(
    sports: Vec<Sport>,
    active: Sport,
    onselect: EventHandler<Sport>,
    #[props(default)] counts: Vec<(Sport, usize)>,
) -> Element {
    // `dioxus_primitives::toggle_group` (L9 design step 2): one Tab stop, arrows between sports;
    // a tap on the active sport keeps it.
    let index = sports.iter().position(|s| *s == active).unwrap_or(0);
    let order = sports.clone();
    rsx! {
        ToggleGroup {
            class: "k-sport-tabs",
            role: "group",
            aria_label: "Sport",
            id: "sport-tabs",
            horizontal: true,
            pressed: Some(std::collections::HashSet::from([index])),
            on_pressed_change: move |set: std::collections::HashSet<usize>| {
                if let Some(s) = set.iter().find(|i| **i != index).and_then(|i| order.get(*i)) {
                    onselect.call(*s);
                }
            },
            for (i, s) in sports.into_iter().enumerate() {
                {
                    let n = counts.iter().find(|(x, _)| *x == s).map_or(0, |(_, n)| *n);
                    let label = match n {
                        0 => s.label().to_string(),
                        1 => format!("{}, 1 pick selected", s.label()),
                        n => format!("{}, {n} picks selected", s.label()),
                    };
                    rsx! {
                        ToggleItem {
                            key: "{s.key()}",
                            index: i,
                            class: if s == active { "k-sport-tab active" } else { "k-sport-tab" },
                            id: "sport-{s.key()}",
                            aria_label: "{label}",
                            SymIcon { sym: s.sym(), size: 16 }
                            span { "{s.label()}" }
                            if n > 0 {
                                span { class: "k-sport-count", id: "sport-{s.key()}-count", "aria-hidden": "true", "{n}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The colour of an outcome's value (guidelines §2): the lowest value of a market in Solana
/// green, the highest in Solana purple, any other (and every value of an all-equal market)
/// neutral. Compute a market's with [`outcome_tones`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutcomeTone {
    #[default]
    Neutral,
    Lowest,
    Highest,
}

impl OutcomeTone {
    fn class(self) -> &'static str {
        match self {
            OutcomeTone::Neutral => "tone-neutral",
            OutcomeTone::Lowest => "tone-lowest",
            OutcomeTone::Highest => "tone-highest",
        }
    }
}

/// Tones for one market's values (micro-units, in the order shown). Unpriced values (0) are
/// neutral and ignored; ties share the tone; when every priced value is equal all are neutral.
pub fn outcome_tones(values_u: &[u64]) -> Vec<OutcomeTone> {
    let priced = values_u.iter().copied().filter(|v| *v > 0);
    let lo = priced.clone().min();
    let hi = priced.max();
    values_u
        .iter()
        .map(|&v| match (lo, hi) {
            (Some(l), Some(h)) if v > 0 && l < h && v == l => OutcomeTone::Lowest,
            (Some(l), Some(h)) if v > 0 && l < h && v == h => OutcomeTone::Highest,
            _ => OutcomeTone::Neutral,
        })
        .collect()
}

/// A value to tap: the outcome ("1", "X", "2") on the left, the value on the right in tabular
/// figures, coloured by `tone`. Selected draws the Solana gradient border (#9945FF → #14F195)
/// and keeps the tone. `hero` is the live card's taller button (padding 10 px instead of 8).
/// `aria_label` names it ("Panathinaikos to win, 2.35"); without one the visible text is read.
/// `class` adds the screen's hooks (e.g. `pick`).
#[component]
pub fn OutcomeButton(
    outcome: String,
    value: String,
    #[props(default)] tone: OutcomeTone,
    #[props(default)] selected: bool,
    #[props(default)] disabled: bool,
    #[props(default)] hero: bool,
    aria_label: Option<String>,
    id: Option<String>,
    #[props(default)] class: String,
    onclick: EventHandler<MouseEvent>,
) -> Element {
    let sel = if selected { "selected" } else { "" };
    let h = if hero { "hero" } else { "" };
    rsx! {
        button {
            r#type: "button",
            class: "k-outcome {tone.class()} {h} {sel} {class}",
            id,
            disabled,
            aria_pressed: "{selected}",
            aria_label,
            onclick: move |e| onclick.call(e),
            span { class: "k-outcome-label", "{outcome}" }
            span { class: "k-outcome-value", "{value}" }
        }
    }
}

/// One team line of a [`MatchCard`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TeamLine {
    /// "Panathinaikos".
    pub name: String,
    /// The live hero's short-code circle ("PAO"); `None` on an upcoming card.
    pub code: Option<String>,
    /// The team's logo URL ([`super::team_logo`]); shown in place of the code circle on the live
    /// hero and before the name on an upcoming card (31 Oct, user request).
    pub logo: Option<&'static str>,
    /// The live hero's right column. V-1: each side's chance from the live price ("59%"), never
    /// a score.
    pub value: Option<String>,
}

impl TeamLine {
    pub fn named(name: impl Into<String>) -> Self {
        TeamLine {
            name: name.into(),
            ..TeamLine::default()
        }
    }
}

/// A match: `meta` ("EuroLeague • 20:45", "EuroLeague • Live · 50′"), an optional `right` label,
/// the two teams side by side (31 Oct, user request: side 1 on the left over the "1" button,
/// side 2 on the right over the "2" button, "vs" between), then `children`: the row of 2 or 3
/// [`OutcomeButton`]s (laid out in equal columns).
///
/// `live` is the hero variant: the red LIVE badge, each team's logo or code circle above its
/// name and its chance under it (V-1), the header on a raised surface. `to` makes the header area
/// a link (to the event page); `action` is its green label with an arrow ("View Market Volume")
/// — part of that link, so there is never a link in a link. `id` and `class` are the screen's
/// hooks.
#[component]
pub fn MatchCard(
    meta: String,
    right: Option<String>,
    first: TeamLine,
    second: TeamLine,
    #[props(default)] live: bool,
    to: Option<Route>,
    action: Option<String>,
    id: Option<String>,
    #[props(default)] class: String,
    children: Element,
) -> Element {
    let variant = if live { "live" } else { "" };
    let team = |t: &TeamLine, second: bool| {
        let (side, muted) = if second {
            ("second", "muted")
        } else {
            ("first", "")
        };
        rsx! {
            div { class: "k-match-team {side}",
                if let Some(src) = t.logo {
                    span { class: "k-match-logo", "aria-hidden": "true",
                        img { src: "{src}", alt: "", decoding: "async", width: "24", height: "24" }
                    }
                } else if let Some(c) = &t.code {
                    span { class: "k-match-code", "aria-hidden": "true", "{c}" }
                }
                span { class: "k-match-team-text",
                    span { class: "k-match-name", "{t.name}" }
                    if let Some(v) = &t.value {
                        span { class: "k-match-value {muted}", "{v}" }
                    }
                }
            }
        }
    };
    let head = rsx! {
        div { class: "k-match-meta",
            span { class: "k-match-meta-start",
                if live {
                    span { class: "k-badge live", span { class: "k-badge-dot", "aria-hidden": "true" } "LIVE" }
                }
                span { class: "k-match-league", "{meta}" }
            }
            if let Some(a) = action {
                span { class: "k-action", "{a}" SymIcon { sym: Sym::ArrowForward, size: 14 } }
            } else if let Some(r) = right {
                span { class: "k-match-right", "{r}" }
            }
        }
        div { class: "k-match-teams",
            {team(&first, false)}
            span { class: "k-match-vs", "vs" }
            {team(&second, true)}
        }
    };
    rsx! {
        article { class: "k-match {variant} {class}", id,
            if let Some(route) = to {
                Link { to: route, class: "k-match-head", {head} }
            } else {
                div { class: "k-match-head", {head} }
            }
            div { class: "k-match-outcomes", {children} }
        }
    }
}

/// A green label-caps link with a trailing arrow ("View Pool (1,284) →", "View Market Volume →").
/// A route makes it a link, otherwise a button calling `onclick`.
#[component]
pub fn ActionLink(
    label: String,
    to: Option<Route>,
    onclick: Option<EventHandler<MouseEvent>>,
    id: Option<String>,
) -> Element {
    let inner = rsx! {
        "{label}"
        SymIcon { sym: Sym::ArrowForward, size: 14 }
    };
    match to {
        Some(route) => rsx! {
            Link { to: route, class: "k-action k-tap", id, {inner} }
        },
        None => rsx! {
            button {
                r#type: "button",
                class: "k-action k-tap",
                id,
                onclick: move |e| {
                    if let Some(h) = onclick {
                        h.call(e);
                    }
                },
                {inner}
            }
        },
    }
}

/// A section title: an optional icon, the title in caps ("UPCOMING MATCHES", "ACTIVE PARLAYS
/// (3)"), and `end` on the right ("Today", "● Live on Solana"). `small` is the 11 px label-caps
/// variant (Active Parlays); default is the 14 px bold one (Upcoming Matches).
#[component]
pub fn SectionTitle(
    title: String,
    icon: Option<Sym>,
    end: Option<Element>,
    #[props(default)] small: bool,
    id: Option<String>,
) -> Element {
    let s = if small { "sm" } else { "" };
    rsx! {
        div { class: "k-section-title {s}",
            h2 { id,
                if let Some(i) = icon {
                    SymIcon { sym: i, size: 18 }
                }
                "{title}"
            }
            if let Some(e) = end {
                span { class: "k-section-title-end", {e} }
            }
        }
    }
}

/// "● Live on Solana": a pulsing green dot and a short green label (section ends, pool header).
#[component]
pub fn LiveDot(label: String) -> Element {
    rsx! {
        span { class: "k-live-dot-label",
            span { class: "k-pulse", "aria-hidden": "true" }
            "{label}"
        }
    }
}
