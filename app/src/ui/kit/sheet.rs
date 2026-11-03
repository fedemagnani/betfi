//! The Instagram-comments style bottom sheet (guidelines §5): the screen stays visible under a
//! dim, blurred backdrop; the sheet rises from the bottom with a drag pill, a header, a scrolling
//! body and a footer pinned above the bottom nav for the purple CTA.

use dioxus::prelude::*;

use dioxus_primitives::dialog::{DialogContent, DialogRoot, DialogTitle};

use super::icons::{Sym, SymIcon};

/// A bottom sheet, shown while `open`. Dismissed by a tap on the backdrop or the drag pill, the
/// ✕, or Escape: each calls `onclose` (the caller clears `open`).
///
/// - `title` ("Schedina", "Copiers (1,284)"), with an optional `icon` tile before it (the slip's
///   receipt) and `badge` after it (`Badge { kind: BadgeKind::Picks(3) }`, "● Live Pool").
/// - `subtitle`: a second header line (the copiers' "€28,450 Pool Volume • 208.8 SOL • Master
///   Treble").
/// - `onclear`: shows CLEAR before the ✕.
/// - `footer`: pinned under the body (the purple CTA). With `above_nav` the whole sheet stops
///   above the bottom nav, so the footer is never under it and the nav stays usable.
/// - `raised`: the copiers sheet's lighter surface (surface-container instead of -low).
///
/// Over the page it is `dioxus_primitives::dialog` (L9 design step 2): modal, named by the title,
/// focus trapped inside, closed by Escape anywhere or a tap outside it (the backdrop); the ✕
/// takes the focus when it opens. `contained` (the kit gallery) draws the same markup without
/// the dialog behaviour. `id` goes on the sheet.
#[component]
pub fn BottomSheet(
    open: bool,
    onclose: EventHandler<()>,
    title: String,
    icon: Option<Sym>,
    badge: Option<Element>,
    subtitle: Option<Element>,
    onclear: Option<EventHandler<()>>,
    footer: Option<Element>,
    #[props(default)] above_nav: bool,
    #[props(default)] raised: bool,
    /// Drawn inside its positioned parent instead of over the page (the kit gallery's specimens).
    #[props(default)]
    contained: bool,
    id: Option<String>,
    children: Element,
) -> Element {
    if !open {
        return rsx! {};
    }
    let title_id = format!("{}-title", id.as_deref().unwrap_or("sheet"));
    let mut class = String::from("k-sheet");
    if above_nav {
        class.push_str(" above-nav");
    }
    if raised {
        class.push_str(" raised");
    }
    let backdrop = if contained {
        "k-sheet-backdrop contained"
    } else {
        "k-sheet-backdrop"
    };
    if contained {
        class.push_str(" contained");
    }
    let head = rsx! {
        button {
            r#type: "button",
            class: "k-sheet-pill",
            aria_label: "Close",
            tabindex: "-1",
            onclick: move |_| onclose.call(()),
            span {}
        }
        div { class: "k-sheet-head",
            div { class: "k-sheet-head-main",
                div { class: "k-sheet-title-row",
                    if let Some(i) = icon {
                        span { class: "k-sheet-icon", SymIcon { sym: i, size: 16 } }
                    }
                    if contained {
                        h3 { class: "k-sheet-title", id: "{title_id}", "{title}" }
                    } else {
                        DialogTitle { id: title_id.clone(), class: "k-sheet-title", "{title}" }
                    }
                    {badge}
                }
                if let Some(s) = subtitle {
                    div { class: "k-sheet-subtitle", {s} }
                }
            }
            div { class: "k-sheet-actions",
                if let Some(clear) = onclear {
                    button {
                        r#type: "button",
                        class: "k-sheet-clear",
                        id: "sheet-clear",
                        onclick: move |_| clear.call(()),
                        "Clear"
                    }
                }
                button {
                    r#type: "button",
                    class: "k-sheet-close",
                    aria_label: "Close",
                    onmounted: move |e| async move {
                        if !contained {
                            let _ = e.data().set_focus(true).await;
                        }
                    },
                    onclick: move |_| onclose.call(()),
                    SymIcon { sym: Sym::Close, size: 18 }
                }
            }
        }
        div { class: "k-sheet-body", {children} }
        if let Some(f) = footer {
            div { class: "k-sheet-foot", {f} }
        }
    };
    if contained {
        return rsx! {
            div { class: backdrop, "aria-hidden": "true" }
            div { class, id, role: "dialog", aria_labelledby: "{title_id}", tabindex: "-1", {head} }
        };
    }
    rsx! {
        DialogRoot {
            class: backdrop,
            is_modal: true,
            open: Some(true),
            on_open_change: move |v: bool| {
                if !v {
                    onclose.call(());
                }
            },
            // Focusable itself, so a screen can keep the focus (and Escape) in the sheet when the
            // focused control goes away (the drawer's Accept).
            DialogContent { id, class, tabindex: "-1", {head} }
        }
    }
}
