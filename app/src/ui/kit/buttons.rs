//! Buttons: primary (purple), secondary (dark), ghost; round icon buttons;
//! text links with a chevron.

use std::sync::atomic::{AtomicU32, Ordering};

use dioxus::prelude::*;

use super::icons::{Icon, Svg};
use crate::Route;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    /// Solana purple, white text: "Deposit".
    #[default]
    Primary,
    /// Dark surface, light text.
    Secondary,
    /// No fill, muted text: "Clear".
    Ghost,
}

impl ButtonVariant {
    fn class(self) -> &'static str {
        match self {
            ButtonVariant::Primary => "primary",
            ButtonVariant::Secondary => "secondary",
            ButtonVariant::Ghost => "ghost",
        }
    }
}

fn classes(variant: ButtonVariant, small: bool, wide: bool, extra: &str) -> String {
    let mut c = format!("k-btn {}", variant.class());
    if small {
        c.push_str(" sm");
    }
    if wide {
        c.push_str(" wide");
    }
    if !extra.is_empty() {
        c.push(' ');
        c.push_str(extra);
    }
    c
}

/// A pill button: 56 px (16 px, 800), or `small` 40 px (14 px) like the top bar's Deposit.
/// `wide` fills the row.
#[component]
pub fn Button(
    #[props(default)] variant: ButtonVariant,
    #[props(default)] small: bool,
    #[props(default)] wide: bool,
    #[props(default)] disabled: bool,
    id: Option<String>,
    aria_label: Option<String>,
    #[props(default)] class: String,
    onclick: EventHandler<MouseEvent>,
    children: Element,
) -> Element {
    rsx! {
        button {
            r#type: "button",
            class: classes(variant, small, wide, &class),
            id,
            aria_label,
            disabled,
            onclick: move |e| onclick.call(e),
            {children}
        }
    }
}

/// The same pill as a link to a route ("Deposit" to Account, "Pick a team").
#[component]
pub fn LinkButton(
    to: Route,
    #[props(default)] variant: ButtonVariant,
    #[props(default)] small: bool,
    #[props(default)] wide: bool,
    id: Option<String>,
    children: Element,
) -> Element {
    rsx! {
        Link { to, class: classes(variant, small, wide, ""), id, {children} }
    }
}

/// Round icon button sizes: `Lg` 44 px (back, close), `Sm` 32 px and `Xs` 28 px on a raised
/// surface with a muted icon (remove a pick).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IconSize {
    #[default]
    Lg,
    Sm,
    Xs,
}

impl IconSize {
    fn class(self) -> &'static str {
        match self {
            IconSize::Lg => "",
            IconSize::Sm => "sm",
            IconSize::Xs => "xs",
        }
    }

    /// The glyph size and stroke the mockup draws at this button size.
    fn glyph(self, icon: Icon) -> (u32, f32) {
        match (self, icon) {
            (IconSize::Lg, Icon::Back) => (20, 2.0),
            (IconSize::Lg, Icon::Close) => (18, 2.2),
            (IconSize::Sm, _) => (12, 2.6),
            (IconSize::Xs, _) => (10, 3.0),
            _ => (18, 2.0),
        }
    }
}

/// How many screens the app has shown since it loaded (the shell counts each route change).
static SCREENS_SHOWN: AtomicU32 = AtomicU32::new(0);

/// The shell calls this on every route it shows, the first one included.
pub fn note_screen_shown() {
    SCREENS_SHOWN.fetch_add(1, Ordering::Relaxed);
}

/// Whether going back in history most likely stays in the app: some screen of it was shown
/// before this one. A page opened from a shared link has none, so its back goes to a fallback.
fn app_page_behind() -> bool {
    SCREENS_SHOWN.load(Ordering::Relaxed) > 1
}

/// A round button holding one icon. `label` is its accessible name (required: the icon is
/// decorative). With `to` it is a link; with `back` it goes back in history (with `to` too,
/// only when the app has a screen behind, else to `to`); else `onclick`.
#[component]
pub fn IconButton(
    icon: Icon,
    label: String,
    #[props(default)] size: IconSize,
    to: Option<Route>,
    #[props(default)] back: bool,
    id: Option<String>,
    onclick: Option<EventHandler<MouseEvent>>,
) -> Element {
    let (px, stroke) = size.glyph(icon);
    let class = format!("k-icon-btn {}", size.class());
    let glyph = rsx! { Svg { icon, size: px, stroke } };
    if let Some(route) = to.clone().filter(|_| !back) {
        return rsx! {
            Link { to: route, class, aria_label: "{label}", title: "{label}", id, {glyph} }
        };
    }
    rsx! {
        button {
            r#type: "button",
            class,
            aria_label: "{label}",
            title: "{label}",
            id,
            onclick: move |e| {
                if back {
                    match to.clone() {
                        Some(fallback) if !app_page_behind() => {
                            navigator().push(fallback);
                        }
                        _ => navigator().go_back(),
                    }
                } else if let Some(h) = onclick {
                    h.call(e);
                }
            },
            {glyph}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Chevron {
    /// "Match & money flow ›", "+24 markets ›".
    #[default]
    Right,
    None,
}

/// A text link: 13 px bold in the text colour ("Match & money flow ›"), or `accent` 600 in
/// light purple ("+24 markets ›", "Share as a tipster slip"). `large` is 14 px. A route makes
/// it a link, otherwise a button calling `onclick`.
#[component]
pub fn TextLink(
    label: String,
    to: Option<Route>,
    onclick: Option<EventHandler<MouseEvent>>,
    #[props(default)] chevron: Chevron,
    #[props(default)] accent: bool,
    #[props(default)] large: bool,
    /// For a disclosure ("See picks"): whether it is open.
    expanded: Option<bool>,
    id: Option<String>,
) -> Element {
    let mut class = "k-link".to_string();
    if accent {
        class.push_str(" accent");
    }
    if large {
        class.push_str(" lg");
    }
    let tail = match chevron {
        Chevron::Right => rsx! { span { "aria-hidden": "true", "›" } },
        Chevron::None => rsx! {},
    };
    if let Some(route) = to {
        return rsx! {
            Link { to: route, class, id, "{label}" {tail} }
        };
    }
    rsx! {
        button {
            r#type: "button",
            class,
            id,
            aria_expanded: expanded.map(|e| e.to_string()),
            onclick: move |e| {
                if let Some(h) = onclick {
                    h.call(e);
                }
            },
            "{label}"
            {tail}
        }
    }
}
