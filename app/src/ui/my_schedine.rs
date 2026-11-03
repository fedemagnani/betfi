//! Your bets: the bets section of Account (`/account#bets`; was the My Bets screen, step 7.1f;
//! 31 Oct, user request: the tab left the bottom nav and its Open / Settled switch moved under
//! the Faucet, L8-4). `/me` stays as a route for old links and redirects to `/account#bets`
//! ([`MySchedine`]). No mockup, V-7: built from the v2 kit in the style of the drawn screens.
//! Open / Settled tabs (the sportsbook's tab look); each open bet is a card with its legs and
//! their status, the live cash-out value and "Cash out €x"; each settled bet says Won, Lost,
//! Cashed out or Refunded with the amount paid and its closing transaction on Solscan. Fed by
//! `GET /tickets/:user` and `ticket` frames.
//!
//! Cash-out is one tap (1 Nov, user request; supersedes D-102's two): "Cash out €x" asks the desk
//! for a signed offer and signs exactly its `c_u` at once (`flows::cash_out`, which also passes
//! the tipster profile of an own tip, L2-4); the wallet's approval is the only other step. Only
//! an offer paying less than the figure tapped (beyond 1%) stops to show it, with its 30 s
//! countdown and "Confirm cash out €y". Once the transaction confirms, the cash-out celebration
//! (mockup `betfi_cash_out_celebration`) opens over the screen: the payout in € and ≈ SOL and
//! the transaction on Solscan. A bet with a lost leg moves to Settled at once (it can no longer
//! win; the chain settles it shortly). No scores (V-1): a started leg reads "Live", and for a
//! winner leg "favoured" or "underdog" from the game's live price (D-300).

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_primitives::toggle_group::{ToggleGroup, ToggleItem};

use super::celebrate::Celebration;
use super::kit::{
    Badge, BadgeKind, HeroIcon, LegRow, SectionTitle, Sym, SymIcon, money, outcome_tones,
};
use super::nav;
use super::{use_app_state, use_desk};
use crate::Route;
use crate::config;
use crate::desk::types::{
    Card, CashoutQuote, LegStatus, MarketKind, Ticket, TicketLeg, TicketState,
};
use crate::flows;
use crate::format;
use crate::state::Load;
use crate::wallet::use_wallet_api;

/// How long a signed cash-out offer is shown as valid, from when it arrived (D-102).
const OFFER_TTL_MS: f64 = 30_000.0;

/// The words for a ticket's state, and the class of its pill (`badge …`).
pub(super) fn state_label(t: &Ticket) -> (&'static str, &'static str) {
    match t.state {
        TicketState::Pending => ("Being placed", "badge wait"),
        TicketState::Open if has_lost_leg(t) => ("Lost", "badge lost"),
        TicketState::Open => ("Open", "badge open"),
        TicketState::Settled if t.paid_u.unwrap_or(0) > 0 => ("Won", "badge won"),
        TicketState::Settled => ("Lost", "badge lost"),
        TicketState::CashedOut => ("Cashed out", "badge done"),
        TicketState::Refunded => ("Refunded", "badge done"),
        TicketState::Other => ("Updating", "badge wait"),
    }
}

fn has_lost_leg(t: &Ticket) -> bool {
    t.legs.iter().any(|l| l.status == LegStatus::Lost)
}

/// Settled tab: every closed ticket, and an open one with a lost leg (it can no longer win).
pub(super) fn is_settled(t: &Ticket) -> bool {
    match t.state {
        TicketState::Open | TicketState::Pending | TicketState::Other => has_lost_leg(t),
        TicketState::Settled | TicketState::CashedOut | TicketState::Refunded => true,
    }
}

/// A leg's name: the market label ("Warriors +2.5", "Over 221.5", "Draw"), or the picked team
/// for a winner leg from a desk before L6 (D-312).
pub(super) fn leg_name(leg: &TicketLeg) -> String {
    match &leg.label {
        Some(l) if !l.is_empty() => l.clone(),
        _ if leg.is_unnamed() => format!("Pick {}", u32::from(leg.idx) + 1),
        _ => leg.picked_team().to_string(),
    }
}

/// The game of a leg as the mockup writes a match: "Real Madrid vs Manchester City".
pub(super) fn match_name(leg: &TicketLeg) -> String {
    format!("{} vs {}", leg.home, leg.away)
}

/// A leg's line under its name: "vs Fenerbahçe" when the pick is a team, else the match
/// ("Arsenal vs Bayern Munich" for a draw, a spread or a total).
fn leg_against(leg: &TicketLeg) -> String {
    if leg.is_unnamed() {
        "Game not named yet".to_string()
    } else if leg_name(leg) == leg.picked_team() {
        format!("vs {}", leg.opponent())
    } else {
        match_name(leg)
    }
}

/// The leg's own value from its quoted chance (1 / p, to the nearest micro-unit).
pub(super) fn leg_value_u(leg: &TicketLeg) -> u64 {
    let p = leg.p_quote_u;
    (1_000_000_000_000 + p / 2).checked_div(p).unwrap_or(0)
}

/// Why cash-out is off, in one plain line: the desk's reason without its own "cash-out
/// unavailable:" prefix; a reason that names a leg only by its chain hash says nothing more.
fn cash_note(reason: Option<&str>, card: Option<&Card>) -> String {
    const PREFIX: &str = "cash-out unavailable:";
    let r = reason.unwrap_or_default().trim();
    let r = match r.get(..PREFIX.len()) {
        Some(p) if p.eq_ignore_ascii_case(PREFIX) => r[PREFIX.len()..].trim(),
        _ => r,
    };
    if r.is_empty() || r.contains("sha256:") {
        return "Cash-out unavailable right now".into();
    }
    format!(
        "Cash-out unavailable: {}",
        crate::desk::plain_reason(r, card)
    )
}

/// A start time as a caption: "01:00" within the next 20 hours, else "Fri 01:00".
fn start_caption(start_ms: i64, now_ms: i64) -> String {
    if start_ms - now_ms < 20 * 3_600_000 {
        format::clock(start_ms)
    } else {
        let day = format::day(start_ms);
        let weekday = day.split(' ').next().unwrap_or_default();
        format!("{weekday} {}", format::clock(start_ms))
    }
}

/// A started, undecided leg (V-1, D-300): "Live · favoured" or "Live · underdog" from the
/// game's live winner price for the picked team; plain "Live" for a spread or total (their
/// markets close at tip-off, L6-2), a draw, or when no live price is known.
fn live_words(leg: &TicketLeg, card: Option<&Card>) -> String {
    let winner =
        matches!(leg.kind, None | Some(MarketKind::Winner)) && leg_name(leg) == leg.picked_team();
    let chance = card
        .filter(|_| winner)
        .and_then(|c| c.games.iter().find(|g| g.game_id == leg.game_id))
        .filter(|g| g.in_play)
        .and_then(|g| g.pick(leg.pick))
        .map(|p| p.price_u)
        .filter(|p| *p > 0);
    match chance {
        Some(p) if p > 500_000 => "Live · favoured".into(),
        Some(p) if p < 500_000 => "Live · underdog".into(),
        _ => "Live".into(),
    }
}

/// After this long from its start a game is over (an NBA game runs ~2.5 h, soccer ~2 h).
const OVER_AFTER_MS: i64 = 3 * 3_600_000;

/// A leg's status in words: "Won", "Lost", "Live · favoured", "Awaiting result", "Starts 20:45".
pub(super) fn leg_status(leg: &TicketLeg, now_ms: i64, card: Option<&Card>) -> String {
    match leg.status {
        LegStatus::Won => "Won".into(),
        LegStatus::Lost => "Lost".into(),
        // A cancelled game counts as half (settlement terms): decided, not lost.
        LegStatus::Half => "Half (cancelled)".into(),
        LegStatus::Void => "Void".into(),
        // Past any game's length and still open: the result is on its way, not live (31 Oct).
        LegStatus::Open | LegStatus::Other if leg.start_ms + OVER_AFTER_MS <= now_ms => {
            "Awaiting result".into()
        }
        LegStatus::Open | LegStatus::Other if leg.start_ms <= now_ms => live_words(leg, card),
        LegStatus::Open | LegStatus::Other => {
            format!("Starts {}", start_caption(leg.start_ms, now_ms))
        }
    }
}

/// "€25" for a whole-euro amount, else "€25.40".
pub(super) fn eur_whole(s: String) -> String {
    match s.strip_suffix(".00") {
        Some(w) if s.starts_with('€') => w.to_string(),
        _ => s,
    }
}

/// A bet's name: an own tip's title, a single's pick, else "3-leg schedina".
pub(super) fn ticket_title(t: &Ticket) -> String {
    if let Some(title) = t.title.clone().filter(|s| !s.trim().is_empty()) {
        return title;
    }
    match (t.n, t.legs.first()) {
        (1, Some(l)) => leg_name(l),
        (n, _) => format!("{n}-leg schedina"),
    }
}

/// The legs of a bet as receipt rows (kit `LegRow`): status dot (red when lost), the pick, "vs
/// <opponent> · <status>", and its value, toned across the bet's legs like the drawer's picks
/// (lowest green, highest purple; guidelines §2). `id` names the list for the gates.
#[component]
pub(super) fn TicketLegs(
    t: Ticket,
    now_ms: i64,
    card: Option<Card>,
    id: Option<String>,
) -> Element {
    let values: Vec<u64> = t.legs.iter().map(leg_value_u).collect();
    let tones = outcome_tones(&values);
    rsx! {
        ul { class: "mb-legs", id, aria_label: "Legs",
            for (i, leg) in t.legs.iter().enumerate() {
                li {
                    key: "{leg.idx}",
                    class: "mb-leg",
                    "data-status": "{leg_status(leg, now_ms, card.as_ref())}",
                    LegRow {
                        title: leg_name(leg),
                        detail: format!("{} · {}", leg_against(leg), leg_status(leg, now_ms, card.as_ref())),
                        value: if values[i] > 0 { format::odds(values[i]) } else { String::new() },
                        tone: tones[i],
                        lost: leg.status == LegStatus::Lost,
                    }
                }
            }
        }
    }
}

/// The viewer's tickets, the card and "now", read once per render.
struct Snapshot {
    tickets: Vec<Ticket>,
    card: Option<Card>,
    now_ms: i64,
}

fn snapshot(state: Signal<crate::state::AppState>, demo: bool) -> Snapshot {
    let s = state.read();
    Snapshot {
        tickets: s.my_tickets().into_iter().cloned().collect(),
        card: match &s.card {
            Load::Ready(c) => Some(c.clone()),
            _ => None,
        },
        now_ms: s.now_ms(demo),
    }
}

/// Re-render every `every` (live legs follow the clock).
fn use_clock(every: Duration) {
    let mut tick = use_signal(|| 0u32);
    use_future(move || async move {
        loop {
            gloo_timers::future::sleep(every).await;
            tick.with_mut(|t| *t = t.wrapping_add(1));
        }
    });
    let _ = tick();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Open,
    Settled,
}

/// A landed cash-out, for the celebration: the ticket, the signature, what it paid, and the
/// wallet's mock-USDC balance read from the chain before signing and after confirmation
/// (`None` when a read failed: the banner then shows less, never a made-up figure).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Landed {
    pub ticket: Ticket,
    pub sig: String,
    pub paid_u: u64,
}

/// Where every "Your bets" link lands: Account, scrolled to its bets section.
pub(super) const BETS_HREF: &str = "/account#bets";

/// `/me` (the My Bets screen until 31 Oct): old links, shared posts and the kit's buttons, which
/// take a `Route`, land here and are sent on to [`BETS_HREF`] in place (no history entry).
#[component]
pub fn MySchedine() -> Element {
    use_effect(|| {
        navigator().replace(BETS_HREF);
    });
    rsx! {}
}

/// Scrolls `#bets` under the sticky header (its `scroll-margin-top`) when the page was opened at
/// `#bets`, and keeps it there while the cards above and below it load and the page grows: every
/// 100 ms until it has not moved for 0.8 s (at most 4 s). The person's own wheel, touch, key or
/// click stops it at once.
const SCROLL_TO_BETS: &str = r##"
if (location.hash === "#bets") {
  const evs = ["wheel", "touchstart", "keydown", "pointerdown"];
  let stop = false, n = 0, still = 0;
  const quit = () => { stop = true; };
  evs.forEach((e) => addEventListener(e, quit, { passive: true }));
  const go = () => {
    const el = document.getElementById("bets");
    if (!stop && el) {
      const before = el.getBoundingClientRect().top;
      el.scrollIntoView({ block: "start" });
      still = Math.abs(el.getBoundingClientRect().top - before) < 1 ? still + 1 : 0;
      if (++n < 40 && still < 8) return void setTimeout(go, 100);
    }
    evs.forEach((e) => removeEventListener(e, quit));
  };
  go();
}
"##;

/// Account's bets section (`#bets`, under the Faucet): the Open / Settled switch, the bets'
/// cards, the cash-out and its celebration.
#[component]
pub(super) fn YourBets() -> Element {
    let state = use_app_state();
    let desk = use_desk();
    let fx = money::use_fx();
    let mut tab = use_signal(|| Tab::Open);
    use_effect(|| {
        document::eval(SCROLL_TO_BETS);
    });
    // The ticket being cashed out: once it closes, show it under Settled with its transaction.
    let mut cashing = use_signal(|| None::<String>);
    // The cash-out celebration, once a cash-out lands.
    let mut landed = use_signal(|| None::<Landed>);
    use_clock(Duration::from_secs(30));
    use_effect(move || {
        let Some(id) = cashing() else {
            return;
        };
        let closed = state
            .read()
            .tickets
            .get(&id)
            .is_some_and(|t| t.state == TicketState::CashedOut);
        if closed {
            tab.set(Tab::Settled);
            cashing.set(None);
        }
    });

    let demo = desk.is_demo();
    let (has_user, load) = {
        let s = state.read();
        (s.user.is_some(), s.tickets_load.clone())
    };
    let snap = snapshot(state, demo);
    let empty = snap.tickets.is_empty();
    let (settled, open): (Vec<Ticket>, Vec<Ticket>) =
        snap.tickets.iter().cloned().partition(is_settled);
    let current = tab();
    let (n_open, n_settled) = (open.len(), settled.len());

    rsx! {
        section { class: "acct-bets", id: "bets", aria_labelledby: "bets-title",
            // The title and the Open / Settled switch on one row, the cards (or an empty card) below.
            div { class: "acct-bets-head",
                SectionTitle { title: "Your bets", small: true, id: "bets-title" }
                // `dioxus_primitives::toggle_group` (L9 design step 2): arrows between the two; a tap
                // on the active one keeps it.
                ToggleGroup {
                    class: "mb-switch",
                    role: "group",
                    aria_label: "Show open or settled bets",
                    id: "my-bets-tabs",
                    horizontal: true,
                    pressed: Some(std::collections::HashSet::from([usize::from(current == Tab::Settled)])),
                    on_pressed_change: move |set: std::collections::HashSet<usize>| {
                        let now = usize::from(current == Tab::Settled);
                        match set.iter().find(|i| **i != now) {
                            Some(0) => tab.set(Tab::Open),
                            Some(_) => tab.set(Tab::Settled),
                            None => {}
                        }
                    },
                    for (i, (t, label, n, tab_id)) in [
                        (Tab::Open, "Open", n_open, "mybets-tab-open"),
                        (Tab::Settled, "Settled", n_settled, "mybets-tab-settled"),
                    ].into_iter().enumerate() {
                        ToggleItem {
                            key: "{label}",
                            index: i,
                            id: tab_id,
                            class: if current == t { "mb-switch-btn active" } else { "mb-switch-btn" },
                            span { "{label}" }
                            if has_user && n > 0 {
                                span { class: "mb-tab-n num", "{n}" }
                            }
                        }
                    }
                }
            }
            if !has_user {
                // The wallet's connect button is in the app bar (and on the wallet card above).
                EmptyBets { id: "bets-connect", text: "Connect a wallet to see your bets.", cta: false }
            } else {
                match load {
                    Load::Loading if empty => rsx! { super::kit::Skeleton { kind: super::kit::SkeletonKind::Row, count: 2, label: "Loading your bets…" } },
                    Load::Failed(e) if empty => rsx! { p { class: "mb-note mb-error", role: "alert", "{e}" } },
                    _ => rsx! {
                        if current == Tab::Open {
                            if n_open == 0 {
                                EmptyBets { text: "No open bets.", cta: true }
                            } else {
                                ul { class: "mb-list", aria_label: "Open bets",
                                    for t in open {
                                        OpenCard { key: "{t.ticket}", t, now_ms: snap.now_ms, card: snap.card.clone(), cashing, landed }
                                    }
                                }
                            }
                        } else if n_settled == 0 {
                            EmptyBets { text: "No settled bets yet.", cta: false }
                        } else {
                            ul { class: "mb-list settled", aria_label: "Settled bets",
                                for t in settled.iter().cloned() {
                                    SettledCard { key: "{t.ticket}", t, fx }
                                }
                            }
                        }
                    },
                }
            }
            if demo && !config::DESIGN_FIXTURES {
                p { class: "mb-note", "Demo data: the desk's example bets." }
            }
            if let Some(l) = landed() {
                CashOutCelebration { landed: l, onclose: move |_| landed.set(None) }
            }
        }
    }
}

/// The bets' empty card: a ticket icon, the line, and on Open a way to the games.
#[component]
fn EmptyBets(#[props(default)] id: Option<&'static str>, text: &'static str, cta: bool) -> Element {
    rsx! {
        div { class: "mb-empty", id,
            span { class: "mb-empty-icon", "aria-hidden": "true", SymIcon { sym: Sym::Ticket, size: 20 } }
            p { class: "mb-empty-text", "{text}" }
            if cta {
                Link { to: Route::Card {}, class: "k-link accent mb-empty-cta", "Browse tonight's games" }
            }
        }
    }
}

/// "€25.00 staked • pays €113.75" under a bet's title.
fn stake_line(t: &Ticket, fx: Option<u64>) -> String {
    let stake = money::pair(t.stake_u, fx).0;
    let pays = money::pair(t.payout_u, fx).0;
    format!("{stake} staked • pays {pays}")
}

/// An open bet: title with its legs badge and total box (the tipster card's head), the stake and
/// what it pays, the legs with their status, the cash-out, the receipt link.
#[component]
fn OpenCard(
    t: Ticket,
    now_ms: i64,
    card: Option<Card>,
    cashing: Signal<Option<String>>,
    landed: Signal<Option<Landed>>,
) -> Element {
    let fx = money::use_fx();
    let kind = if t.copied_from.is_some() {
        Some("Copy")
    } else if t.tipster.as_deref() == Some(t.user.as_str()) {
        Some("Tip")
    } else {
        None
    };
    rsx! {
        li { class: "ticket mb-item", id: "ticket-{t.ticket}",
            article { class: "mb-card",
                div { class: "mb-card-head",
                    div { class: "mb-card-main",
                        div { class: "mb-card-titleline",
                            h2 { class: "mb-card-title", "{ticket_title(&t)}" }
                            if t.n > 1 {
                                Badge { kind: BadgeKind::Legs(u32::from(t.n)) }
                            }
                            if let Some(k) = kind {
                                span { class: "mb-kind", "{k}" }
                            }
                        }
                        span { class: "mb-card-sub", "{stake_line(&t, fx)}" }
                    }
                    span { class: "mb-total", aria_label: "Total {format::odds(t.odds_offered_u)}",
                        span { class: "num", "{format::odds(t.odds_offered_u)}" }
                    }
                }
                TicketLegs { t: t.clone(), now_ms, card: card.clone() }
                CashOut { t: t.clone(), cashing, landed }
                div { class: "mb-card-foot",
                    Link {
                        to: Route::TicketScreen { id: t.ticket.clone() },
                        class: "mb-receipt receipt-link",
                        "Receipt"
                        SymIcon { sym: Sym::ChevronRight, size: 18 }
                    }
                }
            }
        }
    }
}

/// Where the cash-out of one ticket is (D-102).
#[derive(Debug, Clone, PartialEq)]
enum Cash {
    Idle,
    /// Asking the desk for a signed offer.
    Asking,
    /// A signed offer and when it arrived (browser ms): the second tap signs exactly `q.c_u`.
    Offer(CashoutQuote, f64),
    Signing(u64),
    Done,
    Failed(String),
}

/// Sets `s` unless its component is gone (the cash-out task outlives the bet's card, 7.2a-r).
fn set_alive<T: 'static>(mut s: Signal<T>, v: T) {
    if let Ok(mut w) = s.try_write() {
        *w = v;
    }
}

/// The cash-out block: "CASH OUT NOW €x" (USDC beneath) and "Cash out €x", which cashes out in one
/// tap; a lower offer waits for "Confirm cash out €y". On success the celebration opens.
#[component]
fn CashOut(t: Ticket, cashing: Signal<Option<String>>, landed: Signal<Option<Landed>>) -> Element {
    let state = use_app_state();
    let desk = use_desk();
    let wallet = use_wallet_api();
    let fx = money::use_fx();
    let cash = use_signal(|| Cash::Idle);
    // The offer's countdown: a tick a second while an offer is shown.
    let mut tick = use_signal(|| 0u32);
    use_future(move || async move {
        loop {
            gloo_timers::future::sleep(Duration::from_secs(1)).await;
            if matches!(*cash.peek(), Cash::Offer(..)) {
                tick.with_mut(|t| *t = t.wrapping_add(1));
            }
        }
    });
    let _ = tick();

    if t.state == TicketState::Pending {
        return rsx! { p { class: "mb-cash-note", role: "status", "Being placed: cash-out opens once it is minted." } };
    }
    if t.state != TicketState::Open {
        return rsx! {};
    }
    let card = match &state.read().card {
        Load::Ready(c) => Some(c.clone()),
        _ => None,
    };
    let live = t
        .cash_out
        .as_ref()
        .filter(|c| c.available)
        .and_then(|c| c.c_u);
    let unavailable = t
        .cash_out
        .as_ref()
        .filter(|c| !c.available)
        .map(|c| cash_note(c.reason.as_deref(), card.as_ref()));

    // Sign exactly the offer's c_u; the balance is read before and after.
    let sign_ticket = t.clone();
    let ask_desk = desk.clone();
    let sign = move |offer: CashoutQuote, at: f64| {
        let desk = desk.clone();
        let ticket = sign_ticket.clone();
        async move {
            // A tap that lands after the 30 s (between two countdown ticks) signs nothing: the
            // screen then says the offer expired and offers a new one.
            if js_sys::Date::now() >= at + OFFER_TTL_MS {
                set_alive(cash, Cash::Offer(offer, at));
                return;
            }
            if config::DESIGN_FIXTURES {
                // Design builds sign nothing: the celebration shows the design data's cashed-out
                // bet (the mockup's) against the design wallet (fixtures/design, D-530).
                let shown = state
                    .peek()
                    .my_tickets()
                    .into_iter()
                    .find(|x| x.state == TicketState::CashedOut)
                    .cloned();
                if let Some(c) = shown {
                    set_alive(
                        landed,
                        Some(Landed {
                            sig: c.tx.cash_out.clone().unwrap_or_default(),
                            paid_u: c.paid_u.unwrap_or(0),
                            ticket: c,
                        }),
                    );
                }
                set_alive(cash, Cash::Idle);
                return;
            }
            set_alive(cash, Cash::Signing(offer.c_u));
            set_alive(cashing, Some(ticket.ticket.clone()));
            let r = flows::cash_out(desk, wallet, state, ticket.clone(), offer.clone()).await;
            let sig = match r {
                Ok(sig) => sig,
                Err(e) => {
                    set_alive(cashing, None);
                    set_alive(cash, Cash::Failed(e));
                    return;
                }
            };
            set_alive(cash, Cash::Done);
            set_alive(
                landed,
                Some(Landed {
                    ticket,
                    sig,
                    paid_u: offer.c_u,
                }),
            );
            nav::balance_changed();
        }
    };
    // The tap: the desk's signed offer, signed at once unless it pays less than the figure tapped
    // (`seen`, beyond 1%); a root task, like the signing (the card may leave Open meanwhile).
    let ask_ticket = t.ticket.clone();
    let sign_for_ask = sign.clone();
    let ask = move |seen: Option<u64>| {
        let desk = ask_desk.clone();
        let id = ask_ticket.clone();
        let card = card.clone();
        let sign = sign_for_ask.clone();
        move |_: MouseEvent| {
            let desk = desk.clone();
            let id = id.clone();
            let card = card.clone();
            let sign = sign.clone();
            dioxus::core::spawn_forever(async move {
                set_alive(cash, Cash::Asking);
                let offer = match desk.cashout_quote(&id).await {
                    Ok(q) if q.ticket == id => q,
                    Ok(_) => {
                        return set_alive(
                            cash,
                            Cash::Failed("The desk's offer is for another bet.".into()),
                        );
                    }
                    Err(e) => return set_alive(cash, Cash::Failed(e.plain(card.as_ref()))),
                };
                let at = js_sys::Date::now();
                let lower = seen.is_some_and(|s| u128::from(offer.c_u) * 100 < u128::from(s) * 99);
                if lower {
                    set_alive(cash, Cash::Offer(offer, at));
                } else {
                    sign(offer, at).await;
                }
            });
        }
    };

    // The figure shown: the signed offer once there is one, else the desk's live value.
    let step = cash();
    let offered = matches!(step, Cash::Offer(..) | Cash::Signing(_));
    let shown_u = match &step {
        Cash::Offer(q, _) => Some(q.c_u),
        Cash::Signing(c) => Some(*c),
        _ => live,
    };
    rsx! {
        div { class: "mb-cash",
            if let Some(u) = shown_u {
                {
                    let (eur, usdc) = money::pair(u, fx);
                    rsx! {
                        div { class: if offered { "mb-cash-value offered" } else { "mb-cash-value" }, id: "cashout-value-{t.ticket}",
                            span { class: "mb-cash-label", if offered { "Your offer" } else { "Cash out now" } }
                            span { class: "mb-cash-money",
                                span { class: "mb-cash-eur", "{eur}" }
                                if let Some(u) = usdc {
                                    span { class: "mb-cash-usdc", "{u}" }
                                }
                            }
                        }
                    }
                }
            }
            match step {
                Cash::Idle => rsx! {
                    if let Some(u) = live {
                        CashButton { id: format!("cashout-{}", t.ticket), class: "mb-cashout", onclick: ask(Some(u)), label: format!("Cash out {}", money::pair(u, fx).0) }
                    } else if let Some(r) = unavailable.clone() {
                        p { class: "mb-cash-note", "{r}" }
                    }
                },
                Cash::Asking => rsx! {
                    CashButton { class: "mb-cashout", disabled: true, label: "Cashing out…" }
                },
                Cash::Offer(offer, at) => {
                    let left = ((at + OFFER_TTL_MS - js_sys::Date::now()) / 1000.0).ceil().max(0.0) as u32;
                    let eur = money::pair(offer.c_u, fx).0;
                    let c_u = offer.c_u;
                    if left == 0 {
                        rsx! {
                            p { class: "mb-cash-note", role: "status", "Offer expired." }
                            CashButton { class: "mb-cashout", onclick: ask(None), label: "Get a new offer" }
                        }
                    } else {
                        rsx! {
                            CashButton {
                                id: format!("cashout-confirm-{}", t.ticket),
                                class: "mb-cashout mb-cashout-confirm",
                                // A root task (as flows.rs runs every flow): on a live desk the
                                // ticket closes, and this card leaves Open, before the wallet's
                                // confirmation returns; a task of this component would be dropped
                                // with it and the celebration never open (7.2a-r).
                                onclick: move |_| {
                                    dioxus::core::spawn_forever(sign(offer.clone(), at));
                                },
                                label: format!("Confirm cash out {eur}"),
                            }
                            p { class: "mb-cash-note mb-offer", role: "timer", "data-c-u": "{c_u}",
                                "The value dropped: offer valid for "
                                span { class: "num", "{left}" }
                                " s"
                            }
                        }
                    }
                }
                Cash::Signing(u) => rsx! {
                    CashButton { class: "mb-cashout", disabled: true, label: format!("Approve {} in your wallet…", money::pair(u, fx).0) }
                },
                Cash::Done => rsx! {
                    p { class: "mb-cash-note ok", role: "status", "Cashed out." }
                },
                Cash::Failed(e) => rsx! {
                    p { class: "mb-cash-note mb-error", role: "alert", "{e}" }
                    if let Some(u) = live {
                        CashButton { class: "mb-cashout", onclick: ask(Some(u)), label: format!("Try again {}", money::pair(u, fx).0) }
                    }
                },
            }
        }
    }
}

/// The cash-out's purple button (guidelines §9: the primary action is Solana purple), the kit's
/// `k-cta` look on a plain button so the gates keep finding `button.mb-cashout`.
#[component]
fn CashButton(
    label: String,
    class: String,
    #[props(default)] disabled: bool,
    id: Option<String>,
    onclick: Option<EventHandler<MouseEvent>>,
) -> Element {
    rsx! {
        button {
            r#type: "button",
            class: "k-cta {class}",
            id,
            disabled,
            onclick: move |e| {
                if let Some(h) = onclick {
                    h.call(e);
                }
            },
            span { class: "k-cta-label", "{label}" }
        }
    }
}

/// The transaction that closed a ticket, with its words.
fn closing_tx(t: &Ticket) -> Option<(&'static str, String)> {
    match t.state {
        TicketState::CashedOut => t.tx.cash_out.clone().map(|s| ("Cashed out", s)),
        TicketState::Settled => t.tx.settle.clone().map(|s| ("Settled", s)),
        TicketState::Refunded => t.tx.refund_pending.clone().map(|s| ("Refunded", s)),
        _ => None,
    }
}

/// What a closed bet paid: the payout, the cash-out, the refund (0 for a loss).
fn paid_of(t: &Ticket) -> u64 {
    match t.state {
        TicketState::Refunded => t.paid_u.unwrap_or(t.stake_u),
        TicketState::Settled | TicketState::CashedOut => t.paid_u.unwrap_or(0),
        _ => 0,
    }
}

/// A settled bet: title and result, what it paid, stake and total, its closing transaction on
/// Solscan; the card opens the receipt.
#[component]
fn SettledCard(t: Ticket, fx: Option<u64>) -> Element {
    let (result, _) = state_label(&t);
    let paid_u = paid_of(&t);
    let (paid, paid_usdc) = money::pair(paid_u, fx);
    let stake = eur_whole(money::pair(t.stake_u, fx).0);
    let when = match (t.state, t.settled_at) {
        (TicketState::Open, _) => "settling".to_string(),
        (_, Some(s)) if s > 0 => format::day(s * 1000),
        _ => String::new(),
    };
    let tone = match result {
        "Won" | "Cashed out" => "won",
        "Lost" => "lost",
        _ => "",
    };
    let what = if t.n > 1 {
        format!("{} legs", t.n)
    } else {
        t.legs.first().map(match_name).unwrap_or_default()
    };
    let sub = if when.is_empty() {
        what
    } else {
        format!("{what} · {when}")
    };
    rsx! {
        li { class: "ticket mb-item mb-settled", id: "ticket-{t.ticket}",
            article { class: "mb-card compact",
                Link { to: Route::TicketScreen { id: t.ticket.clone() }, class: "mb-settled-link receipt-link",
                    span { class: "mb-settled-main",
                        span { class: "mb-settled-title",
                            span { class: "mb-card-title", "{ticket_title(&t)}" }
                            span { class: "mb-result {tone}", "{result}" }
                        }
                        span { class: "mb-card-sub", "{sub}" }
                        span { class: "mb-card-sub", "{stake} staked · Total " span { class: "num", "{format::odds(t.odds_offered_u)}" } }
                    }
                    span { class: "mb-settled-end",
                        span { class: "mb-paid {tone}", "data-paid-u": "{paid_u}", "{paid}" }
                        if let Some(u) = paid_usdc.filter(|_| paid_u > 0) {
                            span { class: "mb-paid-usdc", "{u}" }
                        }
                    }
                }
                if let Some((what, sig)) = closing_tx(&t) {
                    p { class: "mb-closing closing",
                        span { "{what}" }
                        a {
                            href: "{config::solscan_tx(&sig)}",
                            target: "_blank",
                            rel: "noopener noreferrer",
                            aria_label: "{what}: the transaction on Solscan (opens a new tab)",
                            span { class: "num", "{format::short(&sig)}" }
                            SymIcon { sym: Sym::OpenInNew, size: 13 }
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- the cash-out celebration
//
// Built on 7.1b's shared `Celebration` frame (`ui/celebrate.rs`): the frame draws the check,
// the title, the payout and the two buttons (View Transaction, then Brag on 𝕏), with its
// confetti and count-up. 31 Oct (user request): no settled-balance banner, honesty line, closed
// bet row or Solscan verification card. The frame sits in a full-screen overlay with the
// mockup's "Bet Success" back header; the cash-out's own layout over the frame is
// `#cashout-frame` in `assets/screens/celebration.css`.

/// "Cash Out Secured! 💸" (guidelines §8): the glowing check, "Total payout received" in € and
/// ≈ SOL, then View Transaction ↗ (purple) and Brag on 𝕏 (dark), in the guidelines' order. Over
/// the whole screen; Back closes it onto the bets' Settled tab on Account.
#[component]
fn CashOutCelebration(landed: Landed, onclose: EventHandler<()>) -> Element {
    let state = use_app_state();
    let fx = money::use_fx();
    let sol_rate = money::use_sol_rate();
    // The ticket as the stream keeps it, else as it was when signed.
    let t = state
        .read()
        .tickets
        .get(&landed.ticket.ticket)
        .cloned()
        .unwrap_or_else(|| landed.ticket.clone());
    let paid_u = t
        .paid_u
        .filter(|_| t.state == TicketState::CashedOut)
        .unwrap_or(landed.paid_u);
    let (eur, sol) = match fx {
        Some(r) => {
            let e = money::eur_u(paid_u, r);
            (format::eur(e), money::approx_sol(e, sol_rate))
        }
        None => (money::usdc_text(paid_u), None),
    };
    let sig = landed.sig.clone();
    let brag = format!("Cash out secured on BetFi: {eur} 💸 (test money on Solana devnet)");
    rsx! {
        div { class: "cx", id: "cashout-celebration", role: "dialog", aria_modal: "true", aria_label: "Cash Out Secured",
            div { class: "cx-col",
                // The sub-page header, as drawn ("Bet Success"): back closes the celebration.
                div { class: "k-subheader cx-head",
                    div { class: "k-subheader-start",
                        button { r#type: "button", class: "k-icon-btn lg cx-back", id: "cashout-close", aria_label: "Back to your bets", title: "Back to your bets",
                            onclick: move |_| onclose.call(()),
                            SymIcon { sym: Sym::ArrowBack, size: 20 }
                        }
                        span { class: "k-subheader-title", "Bet Success" }
                    }
                    div { class: "k-subheader-end",
                        Link { to: Route::Account {}, class: "k-appbar-avatar", aria_label: "Account",
                            SymIcon { sym: Sym::Person, size: 18 }
                        }
                    }
                }
                div { class: "cx-body",
                    Celebration {
                        id: "cashout-frame",
                        icon: HeroIcon::Check,
                        title: "Cash Out Secured!",
                        emoji: "💸",
                        amount_label: "Total payout received",
                        amount: eur.clone(),
                        sol,
                        green: false,
                        tx: Some(sig).filter(|s| !s.is_empty()),
                        brag,
                    }
                }
            }
        }
    }
}
