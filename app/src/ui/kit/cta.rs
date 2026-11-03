//! Actions and outcomes (7.0a): the purple CTA and the dark secondary button (guidelines §8–9),
//! the summary rows (Total, Payout), and the celebration pieces (the hero, the confirmed-tx
//! badge, the payout figure).

use dioxus::prelude::*;

use super::icons::{Sym, SymIcon, XLogo};
use crate::Route;

/// The one primary action at the bottom of a screen or sheet, Solana purple (#9945FF, the
/// mockup's gradient to #7F21E5 and its glow), 48 px. Verb-first copy ("Bet • €25.00", "Copy
/// Schedina • Total 4.55", "View Transaction"); `icon` before the label (bolt, content_copy),
/// `arrow` a trailing →, `trailing` another trailing icon (open_in_new for View Transaction).
/// `busy` replaces the label while a step runs ("Hedging…") and disables it.
///
/// One of: `href` (an external page, e.g. Solscan: new tab), `to` (a route), else a button
/// calling `onclick`. `tall` is the copy confirm's 56 px bar.
#[component]
pub fn PurpleCTA(
    label: String,
    icon: Option<Sym>,
    #[props(default)] arrow: bool,
    trailing: Option<Sym>,
    #[props(default)] disabled: bool,
    busy: Option<String>,
    href: Option<String>,
    to: Option<Route>,
    onclick: Option<EventHandler<MouseEvent>>,
    #[props(default)] tall: bool,
    id: Option<String>,
) -> Element {
    let t = if tall { "tall" } else { "" };
    let class = format!("k-cta {t}");
    let off = disabled || busy.is_some();
    let inner = rsx! {
        if let Some(i) = icon {
            SymIcon { sym: i, size: 20 }
        }
        span { class: "k-cta-label", {busy.clone().unwrap_or(label.clone())} }
        if arrow {
            SymIcon { sym: Sym::ArrowForward, size: 18 }
        }
        if let Some(tr) = trailing {
            SymIcon { sym: tr, size: 18 }
        }
    };
    action(class, off, href, to, onclick, id, inner)
}

/// The dark secondary button under a purple CTA, 48 px, same targets as [`PurpleCTA`]. `x_logo`
/// ends the label with the X mark: `label: "Brag on", x_logo: true` is the mockup's "Brag on 𝕏"
/// (its accessible name says "Brag on X").
#[component]
pub fn SecondaryButton(
    label: String,
    icon: Option<Sym>,
    trailing: Option<Sym>,
    #[props(default)] x_logo: bool,
    #[props(default)] disabled: bool,
    href: Option<String>,
    to: Option<Route>,
    onclick: Option<EventHandler<MouseEvent>>,
    id: Option<String>,
) -> Element {
    let inner = rsx! {
        if let Some(i) = icon {
            SymIcon { sym: i, size: 20 }
        }
        span { class: "k-cta-label", "{label}" }
        if x_logo {
            XLogo { size: 15 }
            span { class: "sr-only", " X" }
        }
        if let Some(tr) = trailing {
            SymIcon { sym: tr, size: 18 }
        }
    };
    action(
        "k-cta secondary".to_string(),
        disabled,
        href,
        to,
        onclick,
        id,
        inner,
    )
}

fn action(
    class: String,
    disabled: bool,
    href: Option<String>,
    to: Option<Route>,
    onclick: Option<EventHandler<MouseEvent>>,
    id: Option<String>,
    inner: Element,
) -> Element {
    if let Some(h) = href.filter(|_| !disabled) {
        return rsx! {
            a { class, id, href: "{h}", target: "_blank", rel: "noopener noreferrer", {inner} }
        };
    }
    if let Some(route) = to.filter(|_| !disabled) {
        return rsx! {
            Link { to: route, class, id, {inner} }
        };
    }
    rsx! {
        button {
            r#type: "button",
            class,
            id,
            disabled,
            onclick: move |e| {
                if let Some(h) = onclick {
                    h.call(e);
                }
            },
            {inner}
        }
    }
}

/// What a summary row shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RowKind {
    /// Muted label, plain value ("Stake €25.00", "Copy fee 2% · 80% to @lorenzo_hoops").
    #[default]
    Plain,
    /// The combined value ("Total 4.55"), never called odds (guidelines §2).
    Total,
    /// The payout, in green; in the full variant it closes the block below a rule, bigger, with
    /// its ≈ SOL beneath the label.
    Payout,
}

/// One row of [`SummaryRows`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SummaryRow {
    pub label: String,
    pub value: String,
    pub kind: RowKind,
    /// A line under the label (the payout's "≈ 0.838 SOL").
    pub sub: Option<String>,
    /// An id for the value (gates).
    pub id: Option<String>,
}

impl SummaryRow {
    pub fn plain(label: impl Into<String>, value: impl Into<String>) -> Self {
        SummaryRow {
            label: label.into(),
            value: value.into(),
            ..SummaryRow::default()
        }
    }

    /// "Total 4.55".
    pub fn total(value: impl Into<String>) -> Self {
        SummaryRow {
            label: "Total".into(),
            value: value.into(),
            kind: RowKind::Total,
            ..SummaryRow::default()
        }
    }

    /// "Payout €113.75" (the drawer) or, with a label, "Potential Payout €113.75 ≈ 0.838 SOL".
    pub fn payout(label: impl Into<String>, value: impl Into<String>, sol: Option<String>) -> Self {
        SummaryRow {
            label: label.into(),
            value: value.into(),
            kind: RowKind::Payout,
            sub: sol,
            id: None,
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }
}

/// The summary block: label left, value right, one row each. `compact` is the drawer's (dark
/// inset, 12 px labels, payout 18 px green); the default is the copy confirm's card (14 px rows,
/// a rule, then the payout at 28 px with its ≈ SOL).
#[component]
pub fn SummaryRows(
    rows: Vec<SummaryRow>,
    #[props(default)] compact: bool,
    id: Option<String>,
) -> Element {
    let c = if compact { "compact" } else { "" };
    rsx! {
        dl { class: "k-summary {c}", id,
            for (i, r) in rows.into_iter().enumerate() {
                div {
                    key: "{i}",
                    class: match r.kind {
                        RowKind::Payout if i > 0 => "k-summary-row payout ruled",
                        RowKind::Payout => "k-summary-row payout",
                        RowKind::Total => "k-summary-row total",
                        RowKind::Plain => "k-summary-row",
                    },
                    dt {
                        "{r.label}"
                        if let Some(s) = &r.sub {
                            span { class: "k-summary-sub", "{s}" }
                        }
                    }
                    dd { id: r.id.clone(), "{r.value}" }
                }
            }
        }
    }
}

/// The celebration's hero figure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HeroIcon {
    /// "Schedina Locked In!": the tile with the schedina's ticket.
    #[default]
    Ticket,
    /// "Cash Out Secured! 💸": the gradient orb with a tick.
    Check,
}

/// The celebration's top: the glowing icon and the title with its emoji ("Schedina Locked In!",
/// "💸"). `subtitle` is one quiet line (V-8's "Hedge mock-filled · test money"); no marketing
/// lines (guidelines §9).
#[component]
pub fn CelebrationHero(
    title: String,
    emoji: Option<String>,
    #[props(default)] icon: HeroIcon,
    subtitle: Option<String>,
) -> Element {
    rsx! {
        div { class: "k-celebrate",
            match icon {
                HeroIcon::Ticket => rsx! {
                    div { class: "k-celebrate-tile", "aria-hidden": "true",
                        span { class: "k-celebrate-halo" }
                        span { class: "k-celebrate-tile-in",
                            SymIcon { sym: Sym::Receipt, size: 44 }
                            span { class: "k-celebrate-dots", span {} span {} span {} }
                        }
                        span { class: "k-celebrate-spark", SymIcon { sym: Sym::StarShine, size: 22 } }
                    }
                },
                HeroIcon::Check => rsx! {
                    div { class: "k-celebrate-orb", "aria-hidden": "true",
                        span { class: "k-celebrate-ring" }
                        span { class: "k-celebrate-orb-in",
                            span { class: "k-celebrate-orb-core", SymIcon { sym: Sym::Check, size: 32 } }
                        }
                    }
                },
            }
            h2 { class: "k-celebrate-title",
                "{title}"
                if let Some(e) = emoji {
                    span { class: "k-celebrate-emoji", "aria-hidden": "true", " {e}" }
                }
            }
            if let Some(s) = subtitle {
                p { class: "k-celebrate-sub", "{s}" }
            }
        }
    }
}

/// "● ✓ SOLANA TX CONFIRMED · SLOT 284,912,041": the confirmed transaction's badge. `slot` is
/// formatted by the caller; without it only the words show. (Truth rule: the slot of the
/// transaction, not a made-up hash.)
#[component]
pub fn TxBadge(
    slot: Option<String>,
    #[props(default = "Solana Tx Confirmed".to_string())] label: String,
    id: Option<String>,
) -> Element {
    rsx! {
        div { class: "k-txbadge", id, role: "status",
            span { class: "k-pulse", "aria-hidden": "true" }
            SymIcon { sym: Sym::VerifiedFill, size: 15 }
            span { class: "k-txbadge-text",
                "{label}"
                if let Some(s) = slot {
                    span { class: "k-txbadge-slot", " · Slot {s}" }
                }
            }
        }
    }
}

/// The big payout: a caps label ("Maximum Potential Payout", "Total Payout Received"), the euro
/// figure, and a chip with its ≈ SOL (and the rate, "@ €135.80"). `green` is the placement's
/// green figure with its glow; otherwise the figure is white and the label green (cash-out).
/// `count_up`: the figure counts up from zero ([`CountUp`], the celebrations).
#[component]
pub fn PayoutHero(
    label: String,
    eur: String,
    sol: Option<String>,
    rate: Option<String>,
    #[props(default)] green: bool,
    #[props(default)] count_up: bool,
    id: Option<String>,
) -> Element {
    let g = if green { "green" } else { "" };
    rsx! {
        div { class: "k-payout {g}", id,
            span { class: "k-payout-label", "{label}" }
            if count_up {
                span { class: "k-payout-eur", aria_label: "{eur}", CountUp { text: eur.clone() } }
            } else {
                span { class: "k-payout-eur", "{eur}" }
            }
            if let Some(s) = sol {
                span { class: "k-payout-chip",
                    SymIcon { sym: Sym::TokenFill, size: 14 }
                    span { "{s}" }
                    if let Some(r) = rate {
                        span { class: "k-payout-rate", "{r}" }
                    }
                }
            }
        }
    }
}

/// How long [`CountUp`] runs.
const COUNT_MS: f64 = 1100.0;

/// The user asked the system for less motion.
pub fn reduced_motion() -> bool {
    web_sys::window()
        .and_then(|w| {
            w.match_media("(prefers-reduced-motion: reduce)")
                .ok()
                .flatten()
        })
        .is_some_and(|m| m.matches())
}

/// A figure counting up from zero to `text` ("€113.75", "61.20 USDC") with an ease-out, once
/// per mount (the celebrations, 31 Oct). Reduced motion, or a figure it cannot read, shows
/// `text` at once; a `text` that changes meanwhile (the rate arrives) is followed.
#[component]
pub fn CountUp(text: String) -> Element {
    let mut progress = use_signal(|| if reduced_motion() { 1.0 } else { 0.0 });
    use_hook(move || {
        if *progress.peek() >= 1.0 {
            return;
        }
        spawn(async move {
            let start = js_sys::Date::now();
            loop {
                gloo_timers::future::sleep(std::time::Duration::from_millis(16)).await;
                let p = ((js_sys::Date::now() - start) / COUNT_MS).min(1.0);
                progress.set(p);
                if p >= 1.0 {
                    break;
                }
            }
        });
    });
    let shown = counted(&text, progress());
    rsx! { "{shown}" }
}

/// `text` with its number at `p` (0…1, eased) of its value, digits grouped and decimals kept as
/// written: ("€1,240.50", 0.5) → "€1,085.43"-ish. Unreadable or `p` ≥ 1: `text` itself.
fn counted(text: &str, p: f64) -> String {
    if p >= 1.0 {
        return text.to_string();
    }
    let Some(start) = text.find(|c: char| c.is_ascii_digit()) else {
        return text.to_string();
    };
    let end = text[start..]
        .find(|c: char| !(c.is_ascii_digit() || c == ',' || c == '.'))
        .map_or(text.len(), |i| start + i);
    let number = text[start..end].trim_end_matches(['.', ',']);
    let end = start + number.len();
    let (whole, frac) = number.split_once('.').unwrap_or((number, ""));
    let Ok(units) = format!("{}{frac}", whole.replace(',', "")).parse::<u64>() else {
        return text.to_string();
    };
    let eased = 1.0 - (1.0 - p.max(0.0)).powi(3);
    let now = (units as f64 * eased).round() as u64;
    let scale = 10u64.pow(frac.len() as u32);
    let mut grouped = String::new();
    let digits = (now / scale).to_string();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) && whole.contains(',') {
            grouped.push(',');
        }
        grouped.push(c);
    }
    if !frac.is_empty() {
        grouped.push_str(&format!(".{:0w$}", now % scale, w = frac.len()));
    }
    format!("{}{grouped}{}", &text[..start], &text[end..])
}
