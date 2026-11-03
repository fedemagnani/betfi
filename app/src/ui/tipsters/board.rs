//! The Tipsters list (`/tipsters`, L9, user request 1 Nov): the tipsters and nothing else. One row
//! per tipster with at least one own tip (`GET /tipsters?all=1`): avatar, handle (PRO when
//! verified), the 30-day return and the all-time win rate; best 30-day return first. A row opens
//! the tipster's page, where the parlays are.

use dioxus::prelude::*;

use super::{Who, me, up_down};
use crate::Route;
use crate::desk::types::{TipsterRow, TipsterSort};
use crate::format;
use crate::state::Load;
use crate::ui::kit::{Avatar, AvatarTone, Badge, BadgeKind, Skeleton, SkeletonKind, Sym, SymIcon};
use crate::ui::{use_app_state, use_desk};
use crate::wallet::use_wallet_api;

/// Best 30-day return first; ties by win rate, then by wallet (a stable order).
fn by_roi_30d(mut rows: Vec<TipsterRow>) -> Vec<TipsterRow> {
    rows.sort_by(|a, b| {
        b.roi_30d_ppm
            .cmp(&a.roi_30d_ppm)
            .then(b.hit_rate_ppm.cmp(&a.hit_rate_ppm))
            .then(a.wallet.cmp(&b.wallet))
    });
    rows
}

#[component]
pub fn Tipsters() -> Element {
    let desk = use_desk();
    let state = use_app_state();
    let wallet = use_wallet_api();
    let board = use_resource(move || {
        let desk = desk.clone();
        async move { desk.tipsters(TipsterSort::Roi, true).await }
    });
    let card = match &state.read().card {
        Load::Ready(c) => Some(c.clone()),
        _ => None,
    };
    let mine = me(&wallet);
    rsx! {
        section { class: "screen-tipsters", id: "tipsters",
            div { class: "tp-head",
                h1 { "Tipsters" }
                if let Some(w) = mine {
                    Link { to: Route::TipsterPage { wallet: w }, class: "k-action tp-mine", id: "my-tipster-page",
                        "Your page"
                        SymIcon { sym: Sym::ArrowForward, size: 14 }
                    }
                }
            }
            match &*board.read() {
                None => rsx! { Skeleton { kind: SkeletonKind::Row, count: 4, label: "Loading tipsters…" } },
                Some(Err(e)) => rsx! { p { class: "tp-status error", role: "alert", "{e.plain(card.as_ref())}" } },
                Some(Ok(resp)) if resp.tipsters.is_empty() => rsx! {
                    p { class: "tp-status", id: "board-empty", "No tipsters yet." }
                },
                Some(Ok(resp)) => rsx! {
                    ol { class: "tp-board", id: "board", aria_label: "Tipsters, best 30-day return first",
                        for t in by_roi_30d(resp.tipsters.clone()) {
                            BoardRow { key: "{t.wallet}", t }
                        }
                    }
                },
            }
        }
    }
}

/// One tipster: avatar, name, 30D ROI and win rate; the whole row opens their page.
#[component]
fn BoardRow(t: TipsterRow) -> Element {
    let who = Who::new(t.handle.clone(), &t.wallet);
    let roi = format::pct_signed(t.roi_30d_ppm);
    let win = format::pct_ppm(t.hit_rate_ppm);
    let tone = up_down(t.roi_30d_ppm);
    rsx! {
        li { class: "tp-row", "data-wallet": "{t.wallet}",
            Link { to: Route::TipsterPage { wallet: t.wallet.clone() }, class: "tp-row-link",
                aria_label: "{who.name}: 30-day return {roi}, win rate {win}",
                Avatar {
                    initials: who.initials.clone(),
                    size: 44,
                    tone: if who.green { AvatarTone::Green } else { AvatarTone::Purple },
                    verified: t.verified,
                }
                span { class: "tp-row-main",
                    span { class: "tp-row-name",
                        span { class: "tp-row-handle", "{who.name}" }
                        if t.verified {
                            Badge { kind: BadgeKind::Pro }
                        }
                    }
                    span { class: "tp-row-stats",
                        span { class: "tp-row-stat tp-row-roi",
                            span { class: "tp-row-label", "30D ROI" }
                            span { class: "tp-row-value {tone}", "{roi}" }
                        }
                        span { class: "tp-row-stat tp-row-win",
                            span { class: "tp-row-label", "Win rate" }
                            span { class: "tp-row-value", "{win}" }
                        }
                    }
                }
                SymIcon { sym: Sym::ChevronRight, size: 20 }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(wallet: &str, roi: i64, win: u64) -> TipsterRow {
        TipsterRow {
            wallet: wallet.into(),
            roi_30d_ppm: roi,
            hit_rate_ppm: win,
            ..TipsterRow::default()
        }
    }

    #[test]
    fn best_thirty_day_return_first() {
        let rows = by_roi_30d(vec![
            row("a", -50_000, 900_000),
            row("b", 248_000, 760_000),
            row("c", 248_000, 800_000),
        ]);
        let order: Vec<&str> = rows.iter().map(|r| r.wallet.as_str()).collect();
        assert_eq!(order, ["c", "b", "a"]);
    }
}
