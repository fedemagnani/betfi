//! Inputs: the toggle.

use dioxus::prelude::*;

/// An on/off switch: a real checkbox with `role="switch"` behind the mockup's 44 x 26 track,
/// so keyboard and screen readers work. The whole row is the label.
#[component]
pub fn Toggle(
    label: String,
    checked: bool,
    #[props(default)] disabled: bool,
    id: Option<String>,
    onchange: EventHandler<bool>,
) -> Element {
    let off = if disabled { "off" } else { "" };
    rsx! {
        label { class: "k-toggle {off}",
            span { "{label}" }
            input {
                r#type: "checkbox",
                role: "switch",
                id,
                checked,
                disabled,
                onchange: move |e| onchange.call(e.checked()),
            }
            span { class: "k-toggle-track", "aria-hidden": "true",
                span { class: "k-toggle-knob" }
            }
        }
    }
}
