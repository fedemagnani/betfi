//! The stake block, one layout for the bet drawer and the copy confirm (guidelines §6): "Stake
//! Amount" with the currency pill, the big centred amount with its ≈ SOL line, the green slider
//! with its bounds, and the quick presets.

use dioxus::prelude::*;
use dioxus_primitives::popover::{PopoverContent, PopoverRoot};

use super::icons::{Sym, SymIcon};

/// What the stake is paid in (V-3): euros (mock USDC), SOL (mockSOL through `mock-amm`), USDC,
/// or mockBONK (also through `mock-amm`, an amount typed; the drawer lists it when the desk does).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Currency {
    #[default]
    Eur,
    Sol,
    Usdc,
    Bonk,
}

impl Currency {
    pub const ALL: [Currency; 3] = [Currency::Eur, Currency::Sol, Currency::Usdc];

    pub fn code(self) -> &'static str {
        match self {
            Currency::Eur => "EUR",
            Currency::Sol => "SOL",
            Currency::Usdc => "USDC",
            Currency::Bonk => "BONK",
        }
    }

    /// The menu's line under the symbol.
    pub fn name(self) -> &'static str {
        match self {
            Currency::Eur => "Euro",
            Currency::Sol => "Solana",
            Currency::Usdc => "USD Coin",
            Currency::Bonk => "Bonk",
        }
    }

    /// The pill's badge glyph.
    pub fn symbol(self) -> &'static str {
        match self {
            Currency::Eur => "€",
            Currency::Sol => "◎",
            Currency::Usdc => "$",
            Currency::Bonk => "B",
        }
    }

    /// The coin's logo in the pill (31 Oct, user request), from the Solana token list
    /// (`assets/coin-logos/` at the repo root, 64 px copies in `public/coins/`); the euro keeps
    /// its glyph.
    pub fn logo(self) -> Option<&'static str> {
        match self {
            Currency::Sol => Some("/coins/sol.png"),
            Currency::Usdc => Some("/coins/usdc.png"),
            Currency::Eur | Currency::Bonk => None,
        }
    }
}

/// The coin pill ("€ EUR ▾", the USDC and SOL logos) and its own menu (31 Oct, user request:
/// not the system's select): a card under the pill listing each coin with its logo (or glyph),
/// symbol and name, the current one ticked. A listbox for assistive tech: the pill says what it
/// picks (`label`) and the coin; arrows move between coins, Enter or a tap picks one, Escape or
/// a tap outside closes it (`dioxus_primitives::popover`, L9 design step 2: the innermost layer
/// takes Escape, so the sheet around the menu stays open). One coin only: no menu, no chevron. `id` is the pill's (it carries
/// the coin in `data-code` and every coin in `data-codes`); the menu is `{id}-list`, each coin
/// `[data-code]` in it. The stake block's currency, the drawer's token mode and the faucet.
#[component]
pub fn CoinPill(
    currency: Currency,
    currencies: Vec<Currency>,
    id: String,
    label: String,
    onchange: EventHandler<Currency>,
    #[props(default)] disabled: bool,
) -> Element {
    let mut open = use_signal(|| false);
    let single = currencies.len() <= 1;
    let list = format!("{id}-list");
    let codes = currencies
        .iter()
        .map(|c| c.code())
        .collect::<Vec<_>>()
        .join(" ");
    // Focus the ticked coin when the menu opens; back on the pill when it closes by keyboard.
    let focus_list = format!(
        "requestAnimationFrame(() => document.querySelector('#{list} [aria-selected=\"true\"]')?.focus())"
    );
    let focus_pill = format!("document.getElementById('{id}')?.focus()");
    let n = currencies.len();
    rsx! {
        PopoverRoot {
            class: if open() { "k-coin open" } else { "k-coin" },
            open: Some(open()),
            on_open_change: {
                let focus_pill = focus_pill.clone();
                move |v: bool| {
                    open.set(v);
                    if !v {
                        document::eval(&focus_pill);
                    }
                }
            },
            button {
                r#type: "button",
                class: "k-currency",
                id: "{id}",
                "data-code": currency.code(),
                "data-codes": "{codes}",
                aria_haspopup: "listbox",
                aria_expanded: "{open()}",
                aria_controls: "{list}",
                aria_label: "{label}: {currency.name()}",
                disabled: disabled || single,
                onclick: {
                    let focus_list = focus_list.clone();
                    move |_| {
                        let now = !open();
                        open.set(now);
                        if now {
                            document::eval(&focus_list);
                        }
                    }
                },
                onkeydown: {
                    let focus_list = focus_list.clone();
                    move |e: KeyboardEvent| {
                        match e.key() {
                            Key::ArrowDown | Key::ArrowUp => {
                                e.prevent_default();
                                open.set(true);
                                document::eval(&focus_list);
                            }
                            _ => {}
                        }
                    }
                },
                CoinMark { currency }
                span { class: "k-currency-code", "aria-hidden": "true", "{currency.code()}" }
                if !single {
                    SymIcon { sym: if open() { Sym::ExpandLess } else { Sym::ExpandMore }, size: 16 }
                }
            }
            PopoverContent { class: "k-coin-pop",
            if open() {
                div { class: "k-coin-list", id: "{list}", role: "listbox", aria_label: "{label}",
                    for (i, cur) in currencies.iter().copied().enumerate() {
                        button {
                            key: "{cur.code()}",
                            r#type: "button",
                            role: "option",
                            class: if cur == currency { "k-coin-opt selected" } else { "k-coin-opt" },
                            "data-code": cur.code(),
                            aria_selected: "{cur == currency}",
                            onclick: move |_| {
                                open.set(false);
                                if cur != currency {
                                    onchange.call(cur);
                                }
                            },
                            onkeydown: {
                                let list = list.clone();
                                let focus_pill = focus_pill.clone();
                                move |e: KeyboardEvent| {
                                    let step = match e.key() {
                                        Key::ArrowDown => Some((i + 1) % n),
                                        Key::ArrowUp => Some((i + n - 1) % n),
                                        // Escape is the popover's (it closes the menu only).
                                        Key::Tab => {
                                            open.set(false);
                                            document::eval(&focus_pill);
                                            None
                                        }
                                        _ => None,
                                    };
                                    if let Some(j) = step {
                                        e.prevent_default();
                                        document::eval(&format!(
                                            "document.querySelectorAll('#{list} [role=option]')[{j}]?.focus()"
                                        ));
                                    }
                                }
                            },
                            CoinMark { currency: cur, size: 28 }
                            span { class: "k-coin-opt-text",
                                span { class: "k-coin-opt-code", "{cur.code()}" }
                                span { class: "k-coin-opt-name", "{cur.name()}" }
                            }
                            if cur == currency {
                                SymIcon { sym: Sym::Check, size: 18 }
                            }
                        }
                    }
                }
            }
            }
        }
    }
}

/// A coin's round mark: its logo (USDC, SOL) or its glyph on the tinted disc (€, B).
#[component]
fn CoinMark(currency: Currency, #[props(default = 20)] size: u32) -> Element {
    rsx! {
        if let Some(src) = currency.logo() {
            img { class: "k-currency-badge logo", src: "{src}", alt: "", "aria-hidden": "true", width: "{size}", height: "{size}", style: "width: {size}px; height: {size}px" }
        } else {
            span { class: "k-currency-badge {currency.code().to_lowercase()}", "aria-hidden": "true", style: "width: {size}px; height: {size}px", "{currency.symbol()}" }
        }
    }
}

/// The stake block. Controlled: the screen owns the numbers and formats them.
///
/// - `amount`: the big figure ("€25.00"); `approx`: the line under it ("≈ 0.184 SOL (1 SOL =
///   €135.80)"), `None` while the SOL rate is unknown.
/// - `value`, `min`, `max`, `step`: the slider, in whole euros ("Min: €5", "Max: €250" beneath).
///   `presets` (€10 €25 €50 €100) set it too; the one equal to `value` shows chosen. Both call
///   `onchange` with whole euros.
/// - `currency` and `currencies` (EUR · SOL · USDC by default): the pill, a native select under
///   the drawn "€ EUR ▾" (phones open their own picker); a choice calls `oncurrency`.
/// - `compact`: the drawer's smaller type (16 px title, 13 px presets); default is the copy
///   confirm's (18 px title, 18 px presets).
/// - `error`: one line under the presets ("The minimum stake is €5.").
/// - `id`: on the block; the slider is `{id}-slider`, the presets group `{id}-presets`.
#[component]
pub fn StakeAmount(
    amount: String,
    approx: Option<String>,
    value: u32,
    min: u32,
    max: u32,
    #[props(default = 1)] step: u32,
    presets: Vec<u32>,
    #[props(default)] currency: Currency,
    #[props(default = Currency::ALL.to_vec())] currencies: Vec<Currency>,
    oncurrency: EventHandler<Currency>,
    onchange: EventHandler<u32>,
    #[props(default)] compact: bool,
    error: Option<String>,
    #[props(default = "stake".to_string())] id: String,
) -> Element {
    let c = if compact { "compact" } else { "" };
    let (lo, hi) = (min.min(max), max.max(min));
    let pct = (value.clamp(lo, hi) - lo) * 100 / (hi - lo).max(1);
    rsx! {
        section { class: "k-stake {c}", id: "{id}", aria_labelledby: "{id}-title",
            div { class: "k-stake-head",
                h3 { class: "k-stake-title", id: "{id}-title", "Stake amount" }
                CoinPill { currency, currencies, id: "{id}-currency", label: "Pay in", onchange: move |c| oncurrency.call(c) }
            }
            div { class: "k-stake-amount",
                span { class: "k-stake-big", aria_live: "polite", "{amount}" }
                if let Some(a) = approx {
                    span { class: "k-stake-approx", "{a}" }
                }
            }
            div { class: "k-stake-slider",
                input {
                    r#type: "range",
                    id: "{id}-slider",
                    aria_label: "Stake in euros",
                    min: "{min}",
                    max: "{max}",
                    step: "{step}",
                    value: "{value}",
                    style: "--pct: {pct}%",
                    oninput: move |e| {
                        if let Ok(v) = e.value().parse::<u32>() {
                            onchange.call(v);
                        }
                    },
                }
                div { class: "k-stake-bounds", "aria-hidden": "true",
                    span { "Min: €{min}" }
                    span { "Max: €{max}" }
                }
            }
            div { class: "k-stake-presets", role: "group", aria_label: "Quick stakes", id: "{id}-presets",
                for p in presets {
                    button {
                        key: "{p}",
                        r#type: "button",
                        aria_pressed: "{p == value}",
                        onclick: move |_| onchange.call(p),
                        "€{p}"
                    }
                }
            }
            if let Some(err) = error {
                p { class: "k-stake-error", role: "alert", id: "{id}-error", "{err}" }
            }
        }
    }
}
