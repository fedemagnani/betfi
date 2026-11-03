//! Loading placeholders (L9 design step 2): grey shapes of what is coming (match cards, list
//! rows, a profile card) instead of "Loading…" text, with a soft shimmer (off under reduced
//! motion). Each group says "Loading…" to screen readers (`role="status"`).

use dioxus::prelude::*;

/// The shape a [`Skeleton`] stands in for.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum SkeletonKind {
    /// A Sportsbook match card: meta, two teams, two outcome buttons.
    #[default]
    Match,
    /// A list row: avatar, two lines, a figure (tipsters, parlays, copiers, bets).
    Row,
    /// A profile header: avatar, name, three stat tiles.
    Profile,
}

/// `count` placeholders of `kind`; `label` is what screen readers hear.
#[component]
pub fn Skeleton(
    #[props(default)] kind: SkeletonKind,
    #[props(default = 3)] count: usize,
    #[props(default = "Loading…".to_string())] label: String,
) -> Element {
    rsx! {
        div { class: "k-skel-group", role: "status", aria_busy: "true",
            span { class: "sr-only", "{label}" }
            for i in 0..count {
                match kind {
                    SkeletonKind::Match => rsx! {
                        div { key: "{i}", class: "k-skel-card", "aria-hidden": "true",
                            span { class: "k-skel w-30 h-12" }
                            div { class: "k-skel-split",
                                span { class: "k-skel w-40 h-16" }
                                span { class: "k-skel w-40 h-16" }
                            }
                            div { class: "k-skel-split",
                                span { class: "k-skel w-full h-44" }
                                span { class: "k-skel w-full h-44" }
                            }
                        }
                    },
                    SkeletonKind::Row => rsx! {
                        div { key: "{i}", class: "k-skel-card row", "aria-hidden": "true",
                            span { class: "k-skel circle" }
                            div { class: "k-skel-lines",
                                span { class: "k-skel w-60 h-16" }
                                span { class: "k-skel w-40 h-12" }
                            }
                            span { class: "k-skel w-15 h-24" }
                        }
                    },
                    SkeletonKind::Profile => rsx! {
                        div { key: "{i}", class: "k-skel-card", "aria-hidden": "true",
                            div { class: "k-skel-split start",
                                span { class: "k-skel circle lg" }
                                div { class: "k-skel-lines",
                                    span { class: "k-skel w-60 h-20" }
                                    span { class: "k-skel w-40 h-12" }
                                }
                            }
                            div { class: "k-skel-split",
                                span { class: "k-skel w-full h-48" }
                                span { class: "k-skel w-full h-48" }
                                span { class: "k-skel w-full h-48" }
                            }
                        }
                    },
                }
            }
        }
    }
}
