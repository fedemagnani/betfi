//! The copiers drawer (mockup `betfi_tipster_copiers_drawer_comments_style`, guidelines §5): a
//! bottom sheet over a tipster's parlay (`/copy/:id`, L9), which stays lit and undimmed underneath
//! (the sheet's backdrop is clear but still closes it on a tap). Header: "Copiers (n)", "● Live Pool", and
//! "€x Pool Volume • ≈ y SOL • <tip>"; rows: avatar, handle or short address, € staked, ↗,
//! nothing else. A row opens its copy's transaction on Solscan (devnet, V-9).

use dioxus::prelude::*;

use super::{Who, count, tip_title};
use crate::desk::is_signature;
use crate::desk::types::{Copier, Ticket};
use crate::format;
use crate::ui::kit::{Avatar, AvatarTone, Badge, BadgeKind, BottomSheet, CopierRow, money};
use crate::ui::use_desk;

/// The drawer for `tip`, one of `tipster`'s open tips: who copied it, how much, and each copy's
/// transaction on Solscan.
#[component]
pub fn CopiersSheet(tipster: String, tip: Ticket, onclose: EventHandler<()>) -> Element {
    let desk = use_desk();
    let rate = money::use_fx();
    let sol_rate = money::use_sol_rate();
    let (w, id) = (tipster.clone(), tip.ticket.clone());
    let list = use_resource(use_reactive!(|(w, id)| {
        let desk = desk.clone();
        async move { desk.copiers(&w, &id).await }
    }));
    let pool_u = match &*list.read() {
        Some(Ok(r)) if r.pool_u > 0 => r.pool_u,
        _ => tip.pool_u.unwrap_or(0),
    };
    let volume = match rate {
        Some(r) => format!("{} Pool Volume", money::eur_whole(money::eur_u(pool_u, r))),
        None => format!("{} Pool Volume", money::usdc_text(pool_u)),
    };
    let sol = rate.and_then(|r| money::approx_sol(money::eur_u(pool_u, r), sol_rate));
    let title = tip_title(&tip);
    // The desk's count; the list is capped (`MAX_COPIERS`), so its length is only a fallback.
    let n = match (tip.copies, &*list.read()) {
        (Some(n), _) => n,
        (None, Some(Ok(r))) => u32::try_from(r.copiers.len()).unwrap_or(u32::MAX),
        (None, _) => 0,
    };
    rsx! {
        div { class: "tp-copiers-layer",
            BottomSheet {
                open: true,
                onclose: move |_| onclose.call(()),
                title: format!("Copiers ({})", count(n)),
                badge: rsx! { Badge { kind: BadgeKind::LivePool } },
                subtitle: rsx! {
                    b { id: "copiers-volume", "{volume}" }
                    if let Some(s) = sol {
                        span { class: "sep", "aria-hidden": "true", "•" }
                        span { class: "sol", "{s}" }
                    }
                    span { class: "sep", "aria-hidden": "true", "•" }
                    span { class: "cyan", "{title}" }
                },
                raised: true,
                id: "copiers-sheet",
                match &*list.read() {
                    None => rsx! { crate::ui::kit::Skeleton { kind: crate::ui::kit::SkeletonKind::Row, count: 4, label: "Loading copiers…" } },
                    Some(Err(e)) => rsx! { p { class: "tp-sheet-status error", role: "alert", "{e.plain(None)}" } },
                    Some(Ok(r)) if r.copiers.is_empty() => rsx! {
                        p { class: "tp-sheet-status", id: "copiers-empty", "No copies yet." }
                    },
                    Some(Ok(r)) => rsx! {
                        div { class: "tp-copiers", id: "copiers",
                            for (i, c) in r.copiers.iter().enumerate() {
                                CopierLine { key: "{i}", c: c.clone() }
                            }
                        }
                    },
                }
            }
        }
    }
}

/// One copier: avatar, handle (or short address), € staked, ↗; the whole row opens Solscan.
#[component]
fn CopierLine(c: Copier) -> Element {
    let rate = money::use_fx();
    let handle = c.handle.clone().filter(|h| !h.is_empty());
    let mono = handle.is_none();
    let (initials, name) = match &handle {
        Some(h) => (Who::new(Some(h.clone()), &c.wallet).initials, h.clone()),
        None => (c.wallet.chars().take(2).collect(), format::short(&c.wallet)),
    };
    let stake = money::pair(c.stake_u, rate).0;
    let tone = AvatarTone::tint_for(&c.wallet);
    match c.tx.filter(|t| is_signature(t)) {
        Some(sig) => rsx! {
            CopierRow { initials, name, stake, sig, tone, mono }
        },
        // No transaction on record yet: the same row, without a link.
        None => rsx! {
            div { class: "k-copier tp-copier-nolink",
                span { class: "k-copier-who",
                    Avatar { initials, size: 36, tone, mono }
                    span { class: if mono { "k-copier-name addr" } else { "k-copier-name" }, "{name}" }
                }
                span { class: "k-copier-end",
                    span { class: "k-copier-stake", "{stake}" }
                }
            }
        },
    }
}
