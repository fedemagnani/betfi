//! "Share as a tip" (7.1b: a quiet switch under the drawer's summary; was "Share as a tipster
//! slip"): `open_pending(is_tip = true)`. A tip goes on the tipster's public record (Tipsters)
//! and others can copy it while every game in it is undecided, live games included (D-660). The
//! switch is off, with the reason beneath, only while the stake is paid in mockSOL or mockBONK
//! (D-221: tips are staked in mock USDC). On, an optional name field (L9):
//! the name rides on the last quote before signing and becomes the tip's title for everyone.

use dioxus::prelude::*;

use super::kit::Toggle;
use super::tipsters::profile_msg::{self, MAX_TITLE_CHARS};
use super::use_app_state;
use crate::state::{AppState, PlayKind};

/// The kind of bet "Bet" opens: a tip only when sharing is on and allowed now, with its name
/// when one is typed.
pub fn play_kind(s: &AppState) -> PlayKind {
    if s.tray.tip && s.tray.tip_blocker().is_none() {
        PlayKind::Tip {
            title: profile_msg::clean_title(&s.tray.tip_name).ok(),
        }
    } else {
        PlayKind::Plain
    }
}

/// Why the typed name cannot be sent, while the tip would be shared with it; "Bet" waits for a
/// fix rather than dropping the name. An empty name is fine: the name is optional.
pub fn name_problem(s: &AppState) -> Option<String> {
    let typed = &s.tray.tip_name;
    let shared = s.tray.tip && s.tray.tip_blocker().is_none();
    (shared && !typed.trim().is_empty())
        .then(|| profile_msg::clean_title(typed).err())
        .flatten()
}

/// The switch (`#share-tip`, a checkbox with `role="switch"`) and, while it cannot be on, one
/// quiet line saying why (`#tip-why`). On, the optional name field (`#tip-name`, L9) and
/// nothing more (31 Oct, user request).
#[component]
pub fn TipToggle() -> Element {
    let mut state = use_app_state();
    let (on, blocker, name, problem) = {
        let s = state.read();
        (
            s.tray.tip,
            s.tray.tip_blocker(),
            s.tray.tip_name.clone(),
            name_problem(&s),
        )
    };
    let allowed = blocker.is_none();
    rsx! {
        div { class: if allowed { "slip-tip" } else { "slip-tip blocked" },
            Toggle {
                label: "Share as a tip",
                checked: on && allowed,
                disabled: !allowed,
                id: "share-tip",
                onchange: move |v: bool| {
                    state.write().tray.tip = v;
                },
            }
            match blocker {
                Some(why) => rsx! {
                    p { class: "slip-line", id: "tip-why", role: "status", "Tips are off: {why}." }
                },
                None if on => rsx! {
                    label { class: "slip-tip-name",
                        span { class: "sr-only", "Name your tip (optional)" }
                        input {
                            r#type: "text",
                            id: "tip-name",
                            autocomplete: "off",
                            maxlength: "{MAX_TITLE_CHARS}",
                            placeholder: "Name it (optional), e.g. Thursday treble",
                            aria_invalid: "{problem.is_some()}",
                            value: "{name}",
                            oninput: move |e| state.write().tray.tip_name = e.value(),
                        }
                    }
                    if let Some(p) = problem.as_ref() {
                        p { class: "slip-error", id: "tip-name-why", role: "alert", "Not a name: {p}." }
                    }
                },
                None => rsx! {},
            }
        }
    }
}
