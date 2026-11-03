//! Receipt of one bet (`/ticket/:id`; restyled for v2 by 7.1f in the bet cards' look, no
//! mockup): reached from Account's bets, a tipster's page or a shared link; back goes to
//! Account's bets (`/account#bets`, through `/me`). The bet's head (title, state, stake
//! and total), what it pays or paid in € and ≈ SOL, for an open own tip "Copy this schedina"
//! (`/copy/<id>`, 6.1e) or why it cannot be copied now, the money rows, the legs with their
//! results and prices, the hedge fills (each marked mock-filled when the mock filled it, D-306,
//! V-8), every transaction on Solscan (V-9: each `resolve_leg` too), the settlement terms. Never
//! the word "odds" (guidelines §2): the combined value is "Total".

use dioxus::prelude::*;

use super::copy::{copy_blocker, is_own_tip};
use super::kit::{
    PayoutHero, PurpleCTA, SecondaryButton, SectionTitle, SolscanLink, SolscanStyle, SummaryRow,
    SummaryRows, money,
};
use super::my_schedine::{TicketLegs, leg_name, state_label, ticket_title};
use super::{use_app_state, use_desk};
use crate::Route;
use crate::desk::is_signature;
use crate::desk::types::{LegStatus, Ticket, TicketState, Venue};
use crate::format;
use crate::state::Load;

#[component]
pub fn TicketScreen(id: String) -> Element {
    let desk = use_desk();
    let state = use_app_state();
    let fetch_id = id.clone();
    let fetched = use_resource(use_reactive!(|(fetch_id,)| {
        let desk = desk.clone();
        async move { desk.ticket(&fetch_id).await }
    }));
    // A ticket the stream keeps current (the viewer's own) wins over the one-off fetch.
    let live = state.read().tickets.get(&id).cloned();
    let fetched = fetched.read().clone();
    let card = match &state.read().card {
        Load::Ready(c) => Some(c.clone()),
        _ => None,
    };
    rsx! {
        div { class: "rc",
            match (live, fetched) {
                (Some(t), _) | (None, Some(Ok(t))) => rsx! { Receipt { t } },
                (None, None) => rsx! { super::kit::Skeleton { kind: super::kit::SkeletonKind::Match, count: 2, label: "Loading the receipt…" } },
                (None, Some(Err(e))) => rsx! {
                    section { class: "rc-card",
                        h1 { class: "rc-title", "Receipt" }
                        p { class: "rc-error", role: "alert", "{e.plain(card.as_ref())}" }
                        SecondaryButton { label: "Your bets", to: Route::MySchedine {}, id: "receipt-bets" }
                    }
                },
            }
        }
    }
}

/// One transaction: what it did, then its short signature linking to Solscan (V-9). Nothing for
/// a value that is not a signature.
#[component]
fn TxRow(label: String, sig: String) -> Element {
    if !is_signature(&sig) {
        return rsx! {};
    }
    rsx! {
        li { class: "rc-tx",
            span { class: "rc-tx-label", "{label}" }
            SolscanLink { sig: sig.clone(), style: SolscanStyle::Inline, label: format::short(&sig) }
        }
    }
}

/// "€113.75" and its "≈ 0.838 SOL" for a USDC amount, at the shell's rates.
fn eur_sol(usdc_u: u64, fx: Option<u64>, sol_rate: Option<u64>) -> (String, Option<String>) {
    match fx {
        Some(r) => {
            let e = money::eur_u(usdc_u, r);
            (format::eur(e), money::approx_sol(e, sol_rate))
        }
        None => (money::usdc_text(usdc_u), None),
    }
}

#[component]
fn Receipt(t: Ticket) -> Element {
    let state = use_app_state();
    let demo = use_desk().is_demo();
    let fx = money::use_fx();
    let sol_rate = money::use_sol_rate();
    let (label, badge) = state_label(&t);
    let tone = badge.trim_start_matches("badge ");
    let own_tip = is_own_tip(&t);
    let builder_fee: u64 = t.fills.iter().map(|f| f.builder_fee_u).sum();
    let any_mock = t.fills.iter().any(|f| f.venue != Venue::Pascal);
    let resolved = t
        .legs
        .iter()
        .filter(|l| l.status != LegStatus::Open)
        .count();
    let n_resolve = t.tx.resolve_leg.len();
    let (now_ms, card, mock_profile) = {
        let s = state.read();
        let card = match &s.card {
            Load::Ready(c) => Some(c.clone()),
            _ => None,
        };
        (s.now_ms(demo), card, s.mock_fills())
    };
    let copy = (own_tip && t.state == TicketState::Open).then(|| copy_blocker(&t));
    let cash_now = t
        .cash_out
        .as_ref()
        .filter(|c| t.state == TicketState::Open && c.available)
        .and_then(|c| c.c_u);
    let pair = |u: u64| money::pair(u, fx).0;
    // The hero: what it pays if it wins, or what it paid.
    let (hero_label, hero_u, paid_id) = match (t.state, t.paid_u) {
        (TicketState::CashedOut, Some(p)) => ("Cashed out", p, true),
        (TicketState::Settled, Some(p)) => ("Paid", p, true),
        (TicketState::Settled, None) => ("Paid", 0, true),
        (TicketState::Refunded, p) => ("Refunded", p.unwrap_or(t.stake_u), false),
        _ => ("Pays if it wins", t.payout_u, false),
    };
    let (hero_eur, hero_sol) = eur_sol(hero_u, fx, sol_rate);
    let mut rows = vec![
        SummaryRow::plain("Stake", pair(t.stake_u)),
        SummaryRow::total(format::odds(t.odds_offered_u)),
    ];
    if let Some(fair) = t.odds_fair_u.filter(|f| *f > 0) {
        rows.push(SummaryRow::plain("Fair total", format::odds(fair)));
    }
    if let Some(fee) = t.fee_u.filter(|f| *f > 0) {
        rows.push(SummaryRow::plain("Our margin", pair(fee)));
    }
    if let Some(c) = cash_now {
        rows.push(SummaryRow::plain("Cash-out value now", pair(c)).with_id("receipt-cash-now"));
    }
    let honest = if any_mock || mock_profile {
        "Hedge mock-filled · test money"
    } else {
        "Test money on devnet"
    };

    rsx! {
        section { class: "rc-body", id: "receipt", aria_labelledby: "receipt-title",
            div { class: "rc-card rc-head",
                span { class: "rc-kicker", "Receipt" }
                div { class: "rc-titleline",
                    h1 { class: "rc-title", id: "receipt-title", "{ticket_title(&t)}" }
                    span { class: "rc-pill {tone}", "{label}" }
                    if own_tip {
                        span { class: "rc-pill tip", "Tip" }
                    } else if t.copied_from.is_some() {
                        span { class: "rc-pill tip", "Copy" }
                    }
                }
                span { class: "rc-meta",
                    "{format::picks(t.n)} · {pair(t.stake_u)} staked · Total "
                    span { class: "num", "{format::odds(t.odds_offered_u)}" }
                }
                if own_tip {
                    p { class: "rc-note",
                        "Shared as a tip by "
                        Link { to: Route::TipsterPage { wallet: t.user.clone() }, "{format::short(&t.user)}" }
                    }
                }
                if let Some(orig) = t.copied_from.clone() {
                    p { class: "rc-note",
                        "Copy of "
                        Link { to: Route::TicketScreen { id: orig.clone() }, "bet {format::short(&orig)}" }
                        if let Some(tipster) = t.tipster.clone() {
                            ", tipped by "
                            Link { to: Route::TipsterPage { wallet: tipster.clone() }, "{format::short(&tipster)}" }
                        }
                    }
                }
                div { class: "rc-hero",
                    PayoutHero {
                        label: hero_label,
                        eur: hero_eur,
                        sol: hero_sol,
                        id: if paid_id { Some("receipt-paid".to_string()) } else { None },
                    }
                }
                p { class: "rc-honest", "{honest}" }
            }

            match copy {
                Some(None) => rsx! {
                    PurpleCTA { label: "Copy this schedina", icon: super::kit::Sym::ContentCopy, arrow: true, to: Route::CopyTip { id: t.ticket.clone() }, id: "copy-link" }
                },
                Some(Some(why)) => rsx! {
                    p { class: "rc-note rc-why", id: "copy-why", "Copying closed: {why}." }
                },
                None => rsx! {},
            }

            SummaryRows { rows, id: "receipt-money" }

            section { class: "rc-card", aria_label: "Legs",
                SectionTitle {
                    title: format!("Legs ({})", t.n),
                    small: true,
                    end: rsx! { span { class: "rc-end", "{resolved} of {t.n} decided" } },
                }
                TicketLegs { t: t.clone(), now_ms, card: card.clone(), id: "receipt-legs" }
                ul { class: "rc-prices",
                    for leg in t.legs.iter() {
                        li { key: "{leg.idx}",
                            "{leg_name(leg)} · {format::when(leg.start_ms)} · priced {format::chance(leg.p_quote_u)}"
                            if let Some(p) = leg.p_fill_u {
                                ", hedged {format::chance(p)}"
                            }
                        }
                    }
                }
            }

            section { class: "rc-card", aria_label: "Hedge",
                SectionTitle { title: "Hedge", small: true }
                if t.fills.is_empty() {
                    p { class: "rc-note", "No hedge fills on record." }
                } else {
                    if any_mock {
                        p { class: "rc-mock", id: "receipt-mock",
                            span { class: "rc-tag", "Mock-filled" }
                            " Signed for Pascal and filled by a mock: no order reached Pascal."
                        }
                    }
                    ul { class: "rc-fills",
                        for f in t.fills.iter() {
                            {
                                let team = t
                                    .legs
                                    .iter()
                                    .find(|l| l.symbol == f.symbol)
                                    .map(leg_name)
                                    .unwrap_or_else(|| "another pick".into());
                                let who = if f.venue == Venue::Pascal { "Pascal " } else { "" };
                                let fee = if f.fee_u < 0 {
                                    format!("{who}rebate {}", format::eur_fine(f.fee_u.unsigned_abs()))
                                } else {
                                    format!("{who}fee {}", format::eur_fine(f.fee_u.unsigned_abs()))
                                };
                                rsx! {
                                    li { key: "{f.trade_id}",
                                        span { class: "rc-fill-name",
                                            if f.venue == Venue::Pascal {
                                                "Pascal fill #{f.trade_id} · {team}"
                                            } else {
                                                span { class: "rc-tag", "Mock-filled" }
                                                span { class: "mock-fill", " #{f.trade_id} · {team}" }
                                            }
                                        }
                                        span { class: "rc-fill-sub",
                                            "{format::contracts(f.size)} · {fee} · builder fee {format::eur_fine(f.builder_fee_u)}"
                                        }
                                    }
                                }
                            }
                        }
                        li { class: "rc-fill-total",
                            span { "Builder fee" }
                            span { class: "num", "{format::eur_fine(builder_fee)}" }
                        }
                    }
                }
            }

            section { class: "rc-card", aria_label: "Results and transactions",
                SectionTitle { title: "Results", small: true }
                if resolved == 0 {
                    p { class: "rc-note", "No game decided yet." }
                } else {
                    p { class: "rc-note",
                        "{resolved} of {t.n} decided"
                        if n_resolve > 0 {
                            ", each result posted on chain below."
                        } else {
                            "."
                        }
                    }
                }
                SectionTitle { title: "Transactions", small: true }
                if t.tx.open_pending.is_none() && t.tx.finalize.is_none() {
                    p { class: "rc-note", "No transaction on record yet." }
                }
                ul { class: "rc-txs", id: "receipt-txs",
                    if let Some(s) = t.tx.open_pending.clone() {
                        TxRow { label: "Stake escrowed", sig: s }
                    }
                    if let Some(s) = t.tx.finalize.clone() {
                        TxRow { label: "Minted", sig: s }
                    }
                    for (i, s) in t.tx.resolve_leg.iter().enumerate() {
                        TxRow { key: "{s}", label: format!("Result posted ({} of {})", i + 1, n_resolve), sig: s.clone() }
                    }
                    if let Some(s) = t.tx.cash_out.clone() {
                        TxRow { label: "Cashed out", sig: s }
                    }
                    if let Some(s) = t.tx.settle.clone() {
                        TxRow { label: "Settled", sig: s }
                    }
                    if let Some(s) = t.tx.refund_pending.clone() {
                        TxRow { label: "Refunded", sig: s }
                    }
                }
            }

            details { class: "rc-terms",
                summary { "Settlement terms" }
                p {
                    "Each game settles on Pascal's published result. A game cancelled with no make-up counts as half; a postponed game keeps the bet open, and cash-out stays available. Mirrored markets follow the reference venue's result, even if disputed. The bet pays when every game is decided."
                }
            }
            p { class: "rc-id", "Bet " span { class: "num", "{t.ticket}" } }
        }
    }
}
