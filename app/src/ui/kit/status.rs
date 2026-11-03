//! Status: the price-moved banner.

use dioxus::prelude::*;

use super::icons::{Icon, Svg};

/// "One price moved. The total is now 5.82." with an Accept button, on amber. `compact` is
/// the one-line version ("One price moved · Total now 5.82").
#[component]
pub fn PriceMoved(
    /// How many picks changed price.
    moved: usize,
    /// The new total odds, already formatted ("5.82").
    odds: String,
    #[props(default)] compact: bool,
    id: Option<String>,
    onaccept: EventHandler<MouseEvent>,
) -> Element {
    let what = match moved {
        1 => "One price moved".to_string(),
        n => format!("{n} prices moved"),
    };
    let c = if compact { "compact" } else { "" };
    rsx! {
        div { class: "k-moved {c}", id, role: "status",
            if !compact {
                span { style: "color: var(--warning); display: flex",
                    Svg { icon: Icon::Refresh, size: 18 }
                }
            }
            if compact {
                span { class: "k-moved-text", "{what} · Total now " span { class: "num", "{odds}" } }
            } else {
                span { class: "k-moved-text",
                    "{what}. The total is now "
                    span { class: "num", "{odds}" }
                    "."
                }
            }
            button { r#type: "button", class: "k-moved-accept", onclick: move |e| onaccept.call(e), "Accept" }
        }
    }
}
