//! Layout: the card (`Panel`).

use dioxus::prelude::*;

/// A card (`.k-panel`, 20 px padding). `section` names it as a region when `aria_label` is set.
#[component]
pub fn Panel(
    id: Option<String>,
    aria_label: Option<String>,
    #[props(default)] class: String,
    children: Element,
) -> Element {
    rsx! {
        section { class: "k-panel {class}", id, aria_label, {children} }
    }
}
