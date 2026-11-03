//! Celebrations (7.1b; guidelines §8, mockup `betfi_bet_placement_celebration`): the shared
//! [`Celebration`] frame, and the bet placement celebration ([`PlacedCelebration`]) shown on
//! `/confirm` once a schedina is minted.
//!
//! # The `Celebration` frame (for 7.1e copy and 7.1f cash-out)
//!
//! One full-width column on the ambient glow, top to bottom: the hero icon, the title, the hero
//! amount with its ≈ SOL, the screen's own body, then the two buttons and nothing else
//! (guidelines §8): primary purple **View Transaction ↗** (Solscan devnet) and secondary dark
//! **Brag on 𝕏** (an X post intent). No status badge, slot, honesty line or slip id (31 Oct,
//! user request: only what celebrates).
//!
//! **Motion** (31 Oct, user request): a confetti burst from the hero, the hero popping in with a
//! shock ring, each block rising in turn, the amount counting up from zero ([`kit::CountUp`])
//! then pulsing once with a shine across it, the legs sliding in, a short vibration on phones.
//! All of it once per mount, none of it under `prefers-reduced-motion`. Props:
//!
//! | prop | type | what |
//! | --- | --- | --- |
//! | `icon` | `kit::HeroIcon` | `Ticket` (placement) or `Check` (cash-out orb); default `Ticket` |
//! | `title` | `String` | "Schedina Locked In!", "Cash Out Secured!" |
//! | `emoji` | `Option<String>` | "💸" (after the title) |
//! | `amount_label` | `String` | "Maximum Potential Payout", "Total Payout Received" |
//! | `amount` | `String` | the euro figure, "€113.75" |
//! | `sol` | `Option<String>` | "≈ 0.838 SOL" (`kit::money::approx_sol`); `None` while the SOL rate is unknown |
//! | `rate` | `Option<String>` | "@ €135.80" ([`rate_text`]) |
//! | `green` | `bool` | default `true`: the placement's green figure with its glow; `false` = white figure, green label (cash-out) |
//! | `tx` | `Option<String>` | the transaction signature: View Transaction opens `config::solscan_tx(sig)`; `None` disables it |
//! | `brag` | `String` | empty: no Brag button (the faucet's, 31 Oct); else the post's text, e.g. "Cashed out €61.20 on BetFi (devnet test money)"; the frame adds the app's URL (`url=` of the intent) and draws no second X logo |
//! | `children` | `Element` | the body between the amount and the buttons (the slip card, the match, slot and confirm time) |
//! | `id` | `Option<String>` | on the root (default `celebration`; the copy's is `copy-celebration`; the cash-out's frame sits in `#cashout-celebration`); inside each the amount is `#celebrate-amount` and the buttons `#celebrate-tx` and `#celebrate-brag` |
//!
//! ```ignore
//! Celebration {
//!     icon: kit::HeroIcon::Check,
//!     title: "Cash Out Secured!", emoji: "💸",
//!     amount_label: "Total Payout Received", amount: "€61.20",
//!     sol: kit::money::approx_sol(eur_u, sol_rate), green: false,
//!     tx: Some(sig), brag: "Cashed out €61.20 on BetFi (devnet test money)",
//!     /* body */
//! }
//! ```
//!
//! Its styles are `celebration.css` (`.cel-*`); the pieces inside are the kit's
//! (`CelebrationHero`, `PayoutHero`, `PurpleCTA`, `SecondaryButton`).

use dioxus::prelude::*;

use super::kit::{
    self, HeroIcon, LegRow, OutcomeTone, PayoutHero, PurpleCTA, SecondaryButton, StatTile,
    StatTone, Sym, money, outcome_tones,
};
use super::{use_app_state, use_desk};
use crate::config;
use crate::desk::types::{Card, MarketKind, Quote, Ticket, TicketLeg};
use crate::format;
use crate::state::{Load, Submission};

/// "@ €135.80": the SOL price in euros (`/fx` `eur_per_sol_ppm`) beside a ≈ SOL figure.
pub fn rate_text(eur_per_sol_ppm: Option<u64>) -> Option<String> {
    eur_per_sol_ppm
        .filter(|r| *r > 0)
        .map(|r| format!("@ {}", format::eur(r)))
}

/// Percent-encoding for a URL query value (RFC 3986 unreserved characters kept).
fn query_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The X post intent for `text`, with `url` as the post's link.
pub fn brag_url(text: &str, url: &str) -> String {
    format!(
        "https://x.com/intent/post?text={}&url={}",
        query_value(text),
        query_value(url)
    )
}

/// The app's own address (the page's origin), for the brag's link.
fn app_url() -> String {
    web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default()
}

/// The shared celebration frame (see the module docs for its props).
#[component]
pub fn Celebration(
    #[props(default)] icon: HeroIcon,
    title: String,
    emoji: Option<String>,
    amount_label: String,
    amount: String,
    sol: Option<String>,
    rate: Option<String>,
    #[props(default = true)] green: bool,
    tx: Option<String>,
    brag: String,
    #[props(default = "celebration".to_string())] id: String,
    children: Element,
) -> Element {
    let href = tx.as_deref().map(config::solscan_tx);
    let post = brag_url(&brag, &app_url());
    // A short buzz with the burst (phones that have it; the tap on Bet or Cash out gave the page
    // its user activation).
    use_hook(|| {
        if !kit::reduced_motion() {
            document::eval("navigator.vibrate && navigator.vibrate([24, 60, 48])");
        }
    });
    rsx! {
        div { class: "cel", id: "{id}",
            div { class: "cel-glow", "aria-hidden": "true", span {} span {} span {} }
            div { class: "cel-confetti", "aria-hidden": "true",
                for (i, style) in confetti().into_iter().enumerate() {
                    span { key: "{i}", style }
                }
            }
            div { class: "cel-stream",
                kit::CelebrationHero { title, emoji, icon }
                PayoutHero { label: amount_label, eur: amount, sol, rate, green, count_up: true, id: "celebrate-amount" }
                {children}
                div { class: "cel-actions",
                    PurpleCTA {
                        label: "View Transaction",
                        icon: Sym::CheckCircle,
                        trailing: Sym::OpenInNew,
                        href,
                        disabled: tx.is_none(),
                        id: "celebrate-tx",
                    }
                    if !brag.is_empty() {
                        SecondaryButton { label: "Brag on", x_logo: true, href: post, id: "celebrate-brag" }
                    }
                }
            }
        }
    }
}

/// The confetti pieces' inline styles: where each flies (`--x`, `--y`, px from the hero's
/// centre), its spin, its delay (two bursts) and its size. Spread by the golden angle, so the
/// burst looks random but is the same every time.
fn confetti() -> Vec<String> {
    (0..36u32)
        .map(|i| {
            let angle = f64::from(i) * 137.508_f64.to_radians();
            let dist = 110.0 + f64::from((i * 53) % 120);
            let x = angle.cos() * dist;
            // Biased upward: the gravity in the keyframes brings them down.
            let y = angle.sin() * dist * 0.8 - 70.0;
            let spin = 180 + (i * 97) % 540;
            let delay = if i % 3 == 0 { 260 } else { (i % 4) * 25 };
            let (w, h) = if i % 2 == 0 { (7, 14) } else { (9, 9) };
            format!(
                "--x: {x:.0}px; --y: {y:.0}px; --r: {spin}deg; --d: {delay}ms; width: {w}px; height: {h}px"
            )
        })
        .collect()
}

// ---------------------------------------------------------------- the placed schedina

/// One leg as the celebration lists it.
#[derive(Debug, Clone, PartialEq)]
struct PlacedLeg {
    title: String,
    detail: String,
    /// The leg's own value, micro-units (0 when unknown).
    value_u: u64,
}

/// What the placement celebration shows, from the minted ticket (best) or, until the desk's
/// ticket arrives, from the signed quote the transaction carried.
#[derive(Debug, Clone, PartialEq)]
struct Placed {
    stake_u: u64,
    payout_u: u64,
    total_u: u64,
    legs: Vec<PlacedLeg>,
    /// The `open_pending` signature.
    sig: Option<String>,
}

/// A leg's own value from its quoted chance (1 / p), rounded to the cent so a chance quoted from
/// a two-decimal value reads as that value (657,895 → 1.52, not 1.51).
fn value_of(price_u: u64) -> u64 {
    1_000_000_000_000u64
        .checked_div(price_u)
        .map_or(0, |v| (v + 5_000) / 10_000 * 10_000)
}

fn league_of(card: Option<&Card>, game_id: &str) -> Option<String> {
    card?
        .games
        .iter()
        .find(|g| g.game_id == game_id)
        .and_then(|g| g.league.clone())
}

fn with_league(detail: String, league: Option<String>) -> String {
    match league {
        Some(l) => format!("{detail} \u{2022} {l}"),
        None => detail,
    }
}

fn ticket_leg(l: &TicketLeg, card: Option<&Card>) -> PlacedLeg {
    let league = league_of(card, &l.game_id);
    let label = l.label.clone().map(|s| s.replace(" -", " \u{2212}"));
    let (title, detail) = match (l.kind, label) {
        (Some(MarketKind::Total), Some(lbl)) => (lbl, format!("{} vs {}", l.home, l.away)),
        (Some(MarketKind::Spread), Some(lbl)) => (lbl, format!("vs {}", l.opponent())),
        // A soccer draw (7.0b): its label is "Draw".
        (_, Some(lbl)) if lbl == "Draw" => (lbl, format!("{} vs {}", l.home, l.away)),
        _ => (l.picked_team().to_string(), format!("vs {}", l.opponent())),
    };
    PlacedLeg {
        title,
        detail: with_league(detail, league),
        value_u: value_of(l.p_quote_u),
    }
}

impl Placed {
    fn from_ticket(t: &Ticket, card: Option<&Card>) -> Placed {
        Placed {
            stake_u: t.stake_u,
            payout_u: t.payout_u,
            total_u: t.odds_offered_u,
            legs: t.legs.iter().map(|l| ticket_leg(l, card)).collect(),
            sig: t.tx.open_pending.clone(),
        }
    }

    fn from_quote(q: &Quote, card: Option<&Card>, sub: &Submission) -> Placed {
        let legs = q
            .legs
            .iter()
            .map(|l| {
                let game = card.and_then(|c| c.games.iter().find(|g| g.game_id == l.game_id));
                let (title, detail) = match game {
                    Some(g) => {
                        let mp = g
                            .markets
                            .iter()
                            .flat_map(|m| m.picks.iter().map(move |p| (m.kind, p)))
                            .find(|(_, p)| p.symbol == l.symbol && p.side == l.side);
                        match mp {
                            Some((MarketKind::Winner, p)) if p.team.is_some() => {
                                let other = match l.pick {
                                    crate::desk::types::Pick::Away => &g.home.name,
                                    crate::desk::types::Pick::Home => &g.away.name,
                                };
                                (g.team(l.pick).name.clone(), format!("vs {other}"))
                            }
                            Some((_, p)) => (
                                p.label.replace(" -", " \u{2212}"),
                                format!("{} vs {}", g.home.name, g.away.name),
                            ),
                            None => (
                                g.team(l.pick).name.clone(),
                                format!("vs {}", g.team(other_pick(l.pick)).name),
                            ),
                        }
                    }
                    None => (format::short(&l.symbol), String::new()),
                };
                PlacedLeg {
                    title,
                    detail: with_league(detail, game.and_then(|g| g.league.clone())),
                    value_u: value_of(l.price_u),
                }
            })
            .collect::<Vec<_>>();
        Placed {
            stake_u: q.stake_u,
            payout_u: q.payout_u,
            total_u: q.odds_offered_u,
            legs,
            sig: sub.sig.clone(),
        }
    }
}

fn other_pick(p: crate::desk::types::Pick) -> crate::desk::types::Pick {
    match p {
        crate::desk::types::Pick::Away => crate::desk::types::Pick::Home,
        crate::desk::types::Pick::Home => crate::desk::types::Pick::Away,
    }
}

/// The placement celebration for the app's current submission once minted (`/confirm`). Its
/// ticket comes from the stream's `ticket` frame or, on a live desk, `GET /ticket/:id` (asked
/// again every 2 s, up to 5 times, until it is known).
#[component]
pub fn PlacedCelebration(sub: Submission, ticket_pda: Option<String>) -> Element {
    let mut state = use_app_state();
    let desk = use_desk();
    let demo = desk.is_demo();
    let pda_for_fetch = ticket_pda.clone();
    use_resource(move || {
        let desk = desk.clone();
        let pda = pda_for_fetch.clone();
        async move {
            let Some(pda) = pda.filter(|_| !demo) else {
                return;
            };
            for _ in 0..5 {
                let known = state.peek().tickets.contains_key(&pda);
                if known {
                    return;
                }
                if let Ok(t) = desk.ticket(&pda).await
                    && t.ticket == pda
                {
                    state.write().tickets.insert(pda.clone(), t);
                }
                gloo_timers::future::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    });
    let placed = {
        let s = state.read();
        let card = match &s.card {
            Load::Ready(c) => Some(c),
            _ => None,
        };
        let ticket = ticket_pda.as_ref().and_then(|p| s.tickets.get(p));
        match (ticket, &sub.quote) {
            (Some(t), _) => Some(Placed::from_ticket(t, card)),
            (None, Some(q)) => Some(Placed::from_quote(q, card, &sub)),
            (None, None) => None,
        }
        // The desk's ticket has no signature when it learnt the bet from a resync (a refused
        // `logsSubscribe`); the app's own signature then opens View Transaction.
        .map(|mut p| {
            p.sig = p.sig.filter(|s| !s.is_empty()).or_else(|| sub.sig.clone());
            p
        })
    };
    let Some(placed) = placed else {
        return rsx! {};
    };
    rsx! {
        PlacedView { placed }
    }
}

/// Design builds only (`SCHEDINA_DESIGN_FIXTURES`): `/confirm` with nothing placed shows the
/// mockup's ticket (`fixtures/design/ticket.json`), for the visual review.
#[component]
pub fn DesignCelebration() -> Element {
    let state = use_app_state();
    let placed = {
        let s = state.read();
        let card = match &s.card {
            Load::Ready(c) => Some(c),
            _ => None,
        };
        serde_json::from_str::<Ticket>(crate::desk::fixtures::TICKET)
            .ok()
            .map(|t| Placed::from_ticket(&t, card))
    };
    match placed {
        Some(placed) => rsx! { PlacedView { placed } },
        None => rsx! {},
    }
}

#[component]
fn PlacedView(placed: Placed) -> Element {
    let fx = money::use_fx();
    let sol_rate = money::use_sol_rate();
    let eur = |usdc_u: u64| fx.map(|r| money::eur_u(usdc_u, r));
    let (payout, _) = money::pair(placed.payout_u, fx);
    let (stake, _) = money::pair(placed.stake_u, fx);
    let payout_sol = eur(placed.payout_u).and_then(|e| money::approx_sol(e, sol_rate));
    let stake_sol = eur(placed.stake_u)
        .zip(sol_rate.filter(|r| *r > 0))
        .map(|(e, r)| money::sol_text(money::sol_u(e, r)));
    let total = format::odds(placed.total_u);
    let n = placed.legs.len();
    let tones = outcome_tones(&placed.legs.iter().map(|l| l.value_u).collect::<Vec<_>>());
    let brag = format!(
        "Schedina locked in on BetFi: {n} legs \u{b7} Total {total} \u{b7} potential payout {payout} (devnet test money)"
    );
    rsx! {
        Celebration {
            title: "Schedina Locked In!",
            amount_label: "Maximum Potential Payout",
            amount: payout.clone(),
            sol: payout_sol,
            rate: rate_text(sol_rate),
            tx: placed.sig.clone(),
            brag,
            section { class: "cel-slip", id: "celebrate-slip", aria_label: "Your schedina",
                div { class: "cel-slip-grid",
                    StatTile { label: "Stake Placed", value: stake, sub: stake_sol, large: true, id: "celebrate-stake" }
                    StatTile { label: "Total", value: total.clone(), sub: format!("{n} legs"), tone: StatTone::Green, large: true, id: "celebrate-total" }
                }
                div { class: "cel-slip-legs",
                    for (i, l) in placed.legs.iter().enumerate() {
                        LegRow {
                            key: "{i}",
                            title: l.title.clone(),
                            detail: l.detail.clone(),
                            value: if l.value_u > 0 { format::odds(l.value_u) } else { "\u{2013}".to_string() },
                            tone: tones.get(i).copied().unwrap_or(OutcomeTone::Neutral),
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brag_is_an_encoded_intent() {
        let u = brag_url("Total 4.55 \u{b7} €113.75", "https://betfi.app");
        assert_eq!(
            u,
            "https://x.com/intent/post?text=Total%204.55%20%C2%B7%20%E2%82%AC113.75&url=https%3A%2F%2Fbetfi.app"
        );
    }

    #[test]
    fn leg_values_from_chances() {
        assert_eq!(format::odds(value_of(425_532)), "2.35");
        assert_eq!(format::odds(value_of(657_895)), "1.52");
        assert_eq!(value_of(0), 0);
        assert_eq!(rate_text(Some(135_800_000)).as_deref(), Some("@ €135.80"));
        assert_eq!(rate_text(Some(0)), None);
    }
}
