//! Placement progress (1 Nov, user request): a loading ring over a list of steps that tick off as
//! each operation completes (price, wallet, Solana, escrow, hedge, mint). The ring spins while a
//! step runs, turns into a green check once all are done and a red ✕ on a failure. Each step
//! carries `data-step` (its key) and a state class for the scripts.

use dioxus::prelude::*;

use super::icons::{Sym, SymIcon};

/// Where one step is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Done,
    Current,
    Todo,
    Failed,
}

impl StepState {
    fn class(self) -> &'static str {
        match self {
            StepState::Done => "done",
            StepState::Current => "current",
            StepState::Todo => "todo",
            StepState::Failed => "failed",
        }
    }
}

/// One step: a key for scripts ("escrowed"), its words, an optional note under it, its state.
#[derive(Debug, Clone, PartialEq)]
pub struct SeqStep {
    pub key: &'static str,
    pub label: String,
    pub note: Option<String>,
    pub state: StepState,
}

impl SeqStep {
    pub fn new(key: &'static str, label: impl Into<String>, state: StepState) -> Self {
        SeqStep {
            key,
            label: label.into(),
            note: None,
            state,
        }
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// The ring and the steps. `title` sits beside the ring ("Placing your bet"), `subtitle` under it
/// (the bet in one line). `id` goes on the step list.
#[component]
pub fn Sequencer(
    title: String,
    subtitle: Option<String>,
    steps: Vec<SeqStep>,
    #[props(default = "place-steps".to_string())] id: String,
) -> Element {
    let failed = steps.iter().any(|s| s.state == StepState::Failed);
    let done = !failed && steps.iter().all(|s| s.state == StepState::Done);
    let ring = if failed {
        "failed"
    } else if done {
        "done"
    } else {
        "spinning"
    };
    let current = steps
        .iter()
        .find(|s| matches!(s.state, StepState::Current | StepState::Failed))
        .map(|s| s.label.clone());
    rsx! {
        div { class: "k-seq",
            div { class: "k-seq-head",
                span { class: "k-ring {ring}", "aria-hidden": "true",
                    svg { view_box: "0 0 48 48", width: "48", height: "48",
                        circle { class: "k-ring-track", cx: "24", cy: "24", r: "20" }
                        circle { class: "k-ring-arc", cx: "24", cy: "24", r: "20" }
                    }
                    if done {
                        span { class: "k-ring-mark", SymIcon { sym: Sym::Check, size: 22 } }
                    } else if failed {
                        span { class: "k-ring-mark", SymIcon { sym: Sym::Close, size: 22 } }
                    }
                }
                div { class: "k-seq-titles",
                    h3 { class: "k-seq-title", "{title}" }
                    if let Some(s) = subtitle {
                        p { class: "k-seq-sub", "{s}" }
                    }
                }
            }
            ol { class: "k-seq-steps", id: "{id}", aria_label: "Progress",
                for s in steps.iter() {
                    li {
                        key: "{s.key}",
                        class: "k-seq-step {s.state.class()}",
                        "data-step": "{s.key}",
                        "aria-current": (s.state == StepState::Current).then_some("step"),
                        span { class: "k-seq-dot", "aria-hidden": "true",
                            match s.state {
                                StepState::Done => rsx! { SymIcon { sym: Sym::Check, size: 14 } },
                                StepState::Failed => rsx! { SymIcon { sym: Sym::Close, size: 14 } },
                                StepState::Current => rsx! { span { class: "k-seq-spin" } },
                                StepState::Todo => rsx! {},
                            }
                        }
                        span { class: "k-seq-text",
                            span { class: "k-seq-label", "{s.label}" }
                            if let Some(n) = &s.note {
                                span { class: "k-seq-note", "{n}" }
                            }
                        }
                    }
                }
            }
            if let Some(c) = current {
                p { class: "sr-only", role: "status", "{c}" }
            }
        }
    }
}
