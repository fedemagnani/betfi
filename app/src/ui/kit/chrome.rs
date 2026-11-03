//! Chrome (7.0a, BetFi C2): the app bar with the balance pill and its TEST chip, the live ticker,
//! the bottom nav, a sub-page's header, and the floating slip pill. Every screen of the mockup
//! that is not a sub-page wears the app bar, the ticker and the nav; `ui::Shell` places them.

use dioxus::prelude::*;

use super::buttons::{IconButton, IconSize};
use super::icons::{Icon, Sym, SymIcon};
use crate::Route;

/// The three destinations of the bottom nav (guidelines §4: Sports, Tipsters, Account; L8-4: My
/// Bets left it, its bets are a section of Account).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavTab {
    /// "Sports": the sportsbook (`/`).
    Home,
    Tipsters,
    Account,
}

impl NavTab {
    pub const ALL: [NavTab; 3] = [NavTab::Home, NavTab::Tipsters, NavTab::Account];

    pub fn label(self) -> &'static str {
        match self {
            NavTab::Home => "Sports",
            NavTab::Tipsters => "Tipsters",
            NavTab::Account => "Account",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            NavTab::Home => Icon::Home,
            NavTab::Tipsters => Icon::Tipsters,
            NavTab::Account => Icon::Account,
        }
    }

    pub fn route(self) -> Route {
        match self {
            NavTab::Home => Route::Card {},
            NavTab::Tipsters => Route::Tipsters {},
            NavTab::Account => Route::Account {},
        }
    }
}

/// The app bar (mockup header, top row, 64 px): the BetFi wordmark of the slides ("Bet" white,
/// "Fi" green, a link home); the balance pill "€1,240.50 / 9.13 SOL" with the TEST chip
/// (V-8); the purple `+ DEPOSIT` (to the test faucet on Account); the account avatar.
///
/// - `eur`: the settled balance, formatted ("€1,240.50"); `None` shows "€ –" (not known yet).
/// - `sol`: its SOL equivalent without the unit ("9.13"), shown in green beside it; `None` hides
///   it (the SOL rate is unknown, V-3).
/// - `test`: the TEST chip (mode M: test money, mock-filled hedges). On by default.
/// - `balance`: replaces the whole pill (the wallet's connect button when none is connected).
/// - `initials`: the avatar's letters; without them the person icon.
/// - `onbalance`: the pill becomes a button calling it (1 Nov: the log-in popup, to change the
///   wallet); without it the pill links to Account. A plain `fn`, not an `EventHandler`: the
///   handler of a parent's earlier render is dropped when it renders again (the ticker re-renders
///   the shell every few seconds), and calling it panicked (`ValueDroppedError`).
#[component]
pub fn AppBar(
    eur: Option<String>,
    sol: Option<String>,
    #[props(default = true)] test: bool,
    balance: Option<Element>,
    #[props(default = Route::Account {})] account_to: Route,
    initials: Option<String>,
    onbalance: Option<Action>,
    /// The person button to Account (off beside "Log in", 1 Nov: Account is in the bottom nav).
    #[props(default = true)]
    avatar: bool,
) -> Element {
    let eur_text = eur.unwrap_or_else(|| "€ –".to_string());
    let mut name = format!("Balance {eur_text}");
    if let Some(s) = &sol {
        name.push_str(&format!(", about {s} SOL"));
    }
    if test {
        name.push_str(", test money");
    }
    rsx! {
        div { class: "k-appbar",
            Link { to: Route::Card {}, class: "k-appbar-brand", aria_label: "BetFi, Sports",
                span { class: "k-appbar-word", "aria-hidden": "true", span { class: "bet", "Bet" } span { class: "fi", "Fi" } }
            }
            div { class: "k-appbar-end",
                if let Some(b) = balance {
                    {b}
                } else {
                    {
                        let inner = rsx! {
                            span { class: "k-balance-eur", "{eur_text}" }
                            if let Some(s) = sol {
                                span { class: "k-balance-slash", "aria-hidden": "true", "/" }
                                span { class: "k-balance-sol", "aria-hidden": "true",
                                    span { "{s}" }
                                    span { "SOL" }
                                }
                            }
                            if test {
                                span { class: "k-test-chip", "aria-hidden": "true", title: "Test money: hedges mock-filled", "TEST" }
                            }
                        };
                        match onbalance {
                            Some(h) => rsx! {
                                button {
                                    r#type: "button",
                                    class: "k-balance-pill",
                                    id: "balance",
                                    aria_label: "{name}. Change wallet",
                                    aria_haspopup: "dialog",
                                    onclick: move |_| (h.0)(),
                                    {inner}
                                }
                            },
                            None => rsx! {
                                Link { to: Route::Account {}, class: "k-balance-pill", id: "balance", aria_label: "{name}", {inner} }
                            },
                        }
                    }
                }
                if avatar {
                    Link { to: account_to, class: "k-appbar-avatar", aria_label: "Account",
                        if let Some(i) = initials {
                            span { "aria-hidden": "true", "{i}" }
                        } else {
                            SymIcon { sym: Sym::Person, size: 18 }
                        }
                    }
                }
            }
        }
    }
}

/// A static action for a prop (the balance pill's): a plain `fn`, so it outlives every render.
/// Always equal: the function a call site passes never changes.
#[derive(Debug, Clone, Copy)]
pub struct Action(pub fn());

impl PartialEq for Action {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// One "placed" line of the ticker: `<who> placed <amount> on <team> @ <odds>` (guidelines §3).
/// Every field is already formatted ("sol_bull99", "5.0 SOL", "Real Madrid", "1.88").
#[derive(Debug, Clone, PartialEq)]
pub struct TickerLine {
    /// Stable per bet: a new bet is a new element, which slides in at the front.
    pub key: u64,
    /// The picked team's logo (1 Nov: shown in place of its name), and what follows it (a
    /// spread's "−3.5"); `None` keeps the words (a total, a draw, a team without a logo).
    pub logo: Option<(&'static str, String)>,
    pub who: String,
    pub amount: String,
    pub team: String,
    pub odds: String,
}

/// The single live feed (mockup header, second row, 32 px): a pulsing green dot, "LIVE TICKER",
/// then the lines, the amount in green. One line high; what does not fit is cut at the edge as
/// drawn. `marquee` scrolls it (off under reduced motion); `idle` is shown when there is no line
/// yet ("Waiting for the next bet").
#[component]
pub fn LiveTicker(
    lines: Vec<TickerLine>,
    idle: Option<String>,
    #[props(default)] marquee: bool,
) -> Element {
    let moving = marquee && !lines.is_empty();
    // Not scrolling: the newest bet only, so no bet is ever cut at the edge (1 Nov); a new one
    // replaces it with a short slide-up (keyed, so it is a new element).
    let shown: Vec<&TickerLine> = if moving {
        lines.iter().collect()
    } else {
        lines.iter().take(1).collect()
    };
    let run = |hidden: bool| {
        rsx! {
            div { class: "k-ticker-run", "aria-hidden": if hidden { "true" } else { "false" },
                for l in shown.iter() {
                    span { key: "{l.key}", class: "k-ticker-line",
                        span { class: "who", "{l.who}" }
                        " placed "
                        span { class: "amt", "{l.amount}" }
                        match &l.logo {
                            Some((src, rest)) => rsx! {
                                span { class: "on", " on " }
                                img { class: "k-ticker-logo", src: "{src}", alt: "{l.team}", title: "{l.team}", width: "18", height: "18", decoding: "async" }
                                span { class: "on", "{rest} @ {l.odds}" }
                            },
                            None => rsx! { span { class: "on", " on {l.team} @ {l.odds}" } },
                        }
                    }
                }
            }
        }
    };
    rsx! {
        div { class: "k-ticker", role: "region", aria_label: "Live ticker", id: "ticker-bar",
            span { class: "k-ticker-label",
                span { class: "k-pulse", "aria-hidden": "true" }
                "Live"
            }
            div { class: if moving { "k-ticker-track moving" } else { "k-ticker-track" },
                if lines.is_empty() {
                    span { class: "k-ticker-line idle", {idle.unwrap_or_default()} }
                } else {
                    {run(false)}
                    if moving {
                        {run(true)}
                    }
                }
            }
        }
    }
}

/// The bottom nav (mockup, 64 px): Sports, Tipsters, Account with their Material icons;
/// the active tab in Solana green, bold. `id="bottom-nav"` for the gates.
#[component]
pub fn BottomNav(active: Option<NavTab>) -> Element {
    rsx! {
        nav { class: "k-nav", aria_label: "Main", id: "bottom-nav",
            for tab in NavTab::ALL {
                Link {
                    key: "{tab.label()}",
                    to: tab.route(),
                    class: if active == Some(tab) { "active" } else { "" },
                    aria_current: if active == Some(tab) { "page" } else { "false" },
                    SymIcon { sym: tab.icon().sym(), size: 22 }
                    span { "{tab.label()}" }
                }
            }
        }
    }
}

/// A sub-page's header (the mockup's "Event Detail", "Copy Confirm", "Bet Success"): the back
/// arrow (44 px), the title, `end` (share, Clear) and the account avatar. Without `to`, back goes
/// back in history; with `history` too, it goes back when the app has a screen behind and to `to`
/// otherwise (a shared link's first screen). `close` draws ✕ instead of the arrow. `caption`
/// is a muted line in place of a title.
#[component]
pub fn SubHeader(
    title: Option<String>,
    caption: Option<String>,
    #[props(default)] close: bool,
    to: Option<Route>,
    #[props(default)] history: bool,
    end: Option<Element>,
    /// The account avatar on the right, as drawn. Off where the screen puts its own.
    #[props(default = true)]
    avatar: bool,
) -> Element {
    let (icon, label) = if close {
        (Icon::Close, "Close")
    } else {
        (Icon::Back, "Back")
    };
    let back = history || to.is_none();
    rsx! {
        div { class: "k-subheader",
            div { class: "k-subheader-start",
                IconButton { icon, label, size: IconSize::Lg, to, back }
                if let Some(t) = title {
                    h1 { class: "k-subheader-title", "{t}" }
                } else if let Some(c) = caption {
                    span { class: "k-subheader-caption", "{c}" }
                }
            }
            div { class: "k-subheader-end",
                if let Some(e) = end {
                    {e}
                }
                if avatar {
                    Link { to: Route::Account {}, class: "k-appbar-avatar", aria_label: "Account",
                        SymIcon { sym: Sym::Person, size: 18 }
                    }
                }
            }
        }
    }
}

/// The floating slip pill (purple like the CTA, 31 Oct; above the nav): "3 Picks • Total 4.55  SCHEDINA ›".
/// Never the word "odds" (guidelines §2). `total` is the combined value ("4.55"), `None` until
/// the slip is priced. With `to` it is a link, else a button calling `onclick` (to open the
/// drawer). `id="slip-bar"` for the gates.
#[component]
pub fn SlipPill(
    picks: usize,
    total: Option<String>,
    to: Option<Route>,
    onclick: Option<EventHandler<MouseEvent>>,
) -> Element {
    let count = match picks {
        1 => "1 Pick".to_string(),
        n => format!("{n} Picks"),
    };
    let inner = rsx! {
        span { class: "k-slip-pill-start",
            span { class: "k-slip-pill-icon", SymIcon { sym: Sym::Receipt, size: 16 } }
            span { class: "k-slip-pill-count", "{count}" }
            if let Some(t) = total {
                span { class: "k-slip-pill-dot", "aria-hidden": "true", "•" }
                span { class: "k-slip-pill-total", "Total " b { "{t}" } }
            }
        }
        span { class: "k-slip-pill-cta", "Schedina" SymIcon { sym: Sym::ChevronRight, size: 20 } }
    };
    match to {
        Some(route) => rsx! {
            Link { to: route, class: "k-slip-pill", id: "slip-bar", {inner} }
        },
        None => rsx! {
            button {
                r#type: "button",
                class: "k-slip-pill",
                id: "slip-bar",
                aria_haspopup: "dialog",
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
