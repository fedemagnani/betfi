//! The bet drawer (7.1b; mockup `betfi_sportsbook_bet_drawer_instagram_style`, guidelines §5, §6
//! and §9): an Instagram-comments style bottom sheet over the dimmed, blurred screen it was
//! opened from (the sportsbook, the event page). Its header has the drag pill, the receipt icon,
//! "Schedina" with its "3 PICKS" badge, CLEAR and ✕; then the picks (team, league, "vs
//! Opponent", the value toned lowest green / highest purple, a remove ✕), the
//! shared `StakeAmount` block ("Stake amount" and the `€ EUR ▾` pill, the big euro figure, its ≈
//! SOL, the green slider from the program's minimum stake to the payout cap at the quoted
//! total, the €10 €25 €50 €100 presets), "Total" and "Payout", the "Share as a tip" switch,
//! and the pinned purple "Bet • €25.00". No fee, sentiment or "Pay with" links under it
//! (31 Oct, user request).
//!
//! The slip pill (`ui/mod.rs`) and the `/tray` route open it ([`open_drawer`]); it closes on
//! the backdrop, the pill, ✕, Escape, or when the screen under it has no slip.
//!
//! The pill offers EUR (mock USDC, the stake in euros), USDC (mock USDC, shown in USDC) and, when
//! the desk lists mockSOL, SOL: the euros chosen become a mockSOL amount at `/fx`'s SOL price,
//! swapped into the stake in the same transaction (D-436). Another token the desk lists
//! (mockBONK) is the pill's BONK: its own block, the amount typed (L4-3).
//!
//! **Copy mode** (L9, [`open_copy_drawer`]): the same drawer over a tipster's parlay page or one of
//! its locked games, holding a [`CopyTarget`] instead of the slip. Its picks are the tip's, locked
//! (no ✕, no CLEAR, no "Share as a tip"); the desk prices them with `copied_from` (today's
//! values, the 2% copy fee as its own row), and "Copy Schedina • €x" places the copy in any coin
//! of the pill: paid in mockSOL or mockBONK, the swap pays the stake and the fee (L9).
//!
//! [`SlipEngine`] asks the desk: once whenever the picks, the stake or the payment change, then
//! every 5 s while the drawer is open. Every number shown comes from the desk; the client
//! computes no odds (spec 7). Bet runs `flows::start_play` (or the swap play): a fresh signed
//! quote, the signed-bytes checks, the wallet. Its progress (escrowed → hedging → minted) shows
//! in the CTA, then the placement celebration opens (`/confirm`, `ui/celebrate.rs`).

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_solana::rpc::use_rpc_context;
use schedina_tx::swap::parse_units;
use serde::Deserialize;

use super::kit::{
    self, Badge, BadgeKind, BottomSheet, Currency, Icon, IconButton, IconSize, OutcomeTone,
    PickRow, PriceMoved, PurpleCTA, StakeAmount, SummaryRow, SummaryRows, Sym, outcome_tones,
};
use super::nav::Chrome;
use super::tip_toggle::{TipToggle, name_problem, play_kind};
use super::token_select::{SwapLine, TokenLine, start_swap_play};
use super::{use_app_state, use_desk, use_stream_cmd};
use crate::Route;
use crate::desk::fixtures;
use crate::desk::types::{
    Card, Game, MarketKind, Pick, Quote, QuoteRequest, QuoteRequestLeg, Side, Stage, SwapQuote,
    TokenInfo,
};
use crate::flows;
use crate::format;
use crate::state::slip::SwapAnswer;
use crate::state::{AppState, Load, MIN_STAKE_U, PlayKind, SubmitStep, TrayLeg};
use crate::wallet::use_wallet_api;
use schedina_tx::ShownLeg;
use schedina_tx::swap::copy_stake_for;

const REQUOTE: Duration = Duration::from_secs(5);
/// How long the sheet takes to rise (`k-sheet-in`, 0.25 s in main.css), plus a frame: Bet arms
/// only then (proposed D-541).
const SHEET_RISE: Duration = Duration::from_millis(320);
/// Puts the focus on the drawer itself (`tabindex="-1"`, kit `BottomSheet`).
const FOCUS_DRAWER_JS: &str =
    "const d = document.getElementById('bet-drawer'); if (d) d.focus({ preventScroll: true });";
/// The presets (guidelines §6), whole euros.
const PRESETS: [u32; 4] = [10, 25, 50, 100];
/// The desk's payout cap per ticket, micro-USDC (`crates/pricing` `Caps::max_payout_u`, 500
/// USDC; above it the desk refuses "the payout is above the cap"). The slider stops at the stake
/// whose payout stays under it at the quoted total.
const MAX_PAYOUT_U: u64 = 500_000_000;
/// The slider's top before the slip has a total.
const MAX_STAKE_EUR: u32 = 200;

/// The drawer is open.
static DRAWER: GlobalSignal<bool> = Signal::global(|| false);
/// How a mock-USDC stake is shown (EUR or USDC); SOL while paying in mockSOL.
static SHOW: GlobalSignal<Currency> = Signal::global(|| Currency::Eur);
/// The schedina the drawer is placing: its picks as they were when Bet was tapped (the slip
/// empties when it lands), until it is minted (the celebration opens), refunded or failed.
static PLACING: GlobalSignal<Option<Placing>> = Signal::global(|| None);
/// The last submission started here and its picks: its failure shows while the slip still
/// holds them.
static LAST_TRY: GlobalSignal<Option<LastTry>> = Signal::global(|| None);

/// The tip the drawer copies, while it copies one (copy mode).
static COPYING: GlobalSignal<Option<CopyTarget>> = Signal::global(|| None);

/// A submission id and the picks it played (symbol, side).
type LastTry = (u64, Vec<(String, Side)>);

/// A tipster's open tip as the drawer copies it: its Ticket PDA, its legs as drawer picks
/// (locked), and as the signed copy must name them (`PlayKind::Copy`).
#[derive(Debug, Clone, PartialEq)]
pub struct CopyTarget {
    pub ticket: String,
    pub legs: Vec<TrayLeg>,
    pub shown: Vec<ShownLeg>,
}

/// Open the drawer over a tipster's parlay page (or one of its games) to copy `target`.
pub fn open_copy_drawer(target: CopyTarget) {
    *COPYING.write() = Some(target);
    *DRAWER.write() = true;
}

/// Open the bet drawer over the current screen (the slip pill, `/tray`, `/confirm` with nothing
/// placed).
pub fn open_drawer() {
    *DRAWER.write() = true;
}

/// The drawer is open (the shell hides the slip pill meanwhile).
pub fn drawer_open() -> bool {
    DRAWER()
}

fn close_drawer() {
    *DRAWER.write() = false;
    if COPYING.peek().is_some() {
        *COPYING.write() = None;
    }
}

/// The picks the drawer prices: the copied tip's, else the slip's.
fn legs_of<'a>(s: &'a AppState, copy: Option<&'a CopyTarget>) -> &'a [TrayLeg] {
    copy.map_or(&s.tray.legs, |c| &c.legs)
}

// ---------------------------------------------------------------- derived values

/// The two rates every amount here needs: euros per USD (ECB, U-4) and euros per SOL (V-3).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Rates {
    eur_per_usd: Option<u64>,
    eur_per_sol: Option<u64>,
}

/// Micro-USDC to stake for `eur_u` micro-euros at `rate` ppm (euros per USD, U-4), rounded up to
/// the cent, so the stake shown in euros is never below the euros chosen. Without a rate yet,
/// one to one.
fn usdc_for_eur(eur_u: u64, rate: Option<u64>) -> u64 {
    let Some(r) = rate.filter(|r| *r > 0) else {
        return eur_u;
    };
    let usdc = (u128::from(eur_u) * 1_000_000).div_ceil(u128::from(r));
    u64::try_from(usdc.div_ceil(10_000) * 10_000).unwrap_or(u64::MAX)
}

/// A mockSOL amount for `eur_u` micro-euros at `eur_per_sol` ppm, in base units of `decimals`,
/// truncated to 1/10,000 SOL so it reads cleanly ("0.1840").
fn sol_amount(eur_u: u64, eur_per_sol: u64, decimals: u8) -> Option<u64> {
    if eur_per_sol == 0 {
        return None;
    }
    let units =
        u128::from(eur_u) * 10u128.pow(u32::from(decimals.min(18))) / u128::from(eur_per_sol);
    let step = 10u128.pow(u32::from(decimals.saturating_sub(4)));
    u64::try_from(units / step * step).ok().filter(|a| *a > 0)
}

fn card_of(s: &AppState) -> Option<Card> {
    match &s.card {
        Load::Ready(c) => Some(c.clone()),
        _ => None,
    }
}

/// The euros chosen on the slider, or what is wrong with them.
fn chosen_eur(s: &AppState) -> Result<u64, String> {
    if s.tray.stake_u < MIN_STAKE_U {
        Err(format!(
            "The minimum stake is {}.",
            format::eur(MIN_STAKE_U).trim_end_matches(".00")
        ))
    } else {
        Ok(s.tray.stake_u)
    }
}

/// mockSOL, if the desk lists it (V-3: "SOL" in the pill).
fn sol_token(s: &AppState) -> Option<TokenInfo> {
    s.slip
        .tokens
        .iter()
        .find(|t| !t.usdc_mint && t.symbol.contains("SOL"))
        .cloned()
}

/// The other tokens the desk can swap into the stake (mockBONK): the coin selector's BONK.
fn other_tokens(s: &AppState) -> Vec<TokenInfo> {
    s.slip
        .tokens
        .iter()
        .filter(|t| !t.usdc_mint && !t.symbol.contains("SOL"))
        .cloned()
        .collect()
}

fn is_sol(t: &TokenInfo) -> bool {
    t.symbol.contains("SOL")
}

/// The token being paid in, if the desk still lists it (`None`: mock USDC).
fn paying(s: &AppState) -> Option<TokenInfo> {
    let p = s.tray.pay.clone()?;
    s.slip
        .tokens
        .iter()
        .any(|t| t.mint == p.mint && !t.usdc_mint)
        .then_some(p)
}

/// The amount of `t` paid: for mockSOL, the euros chosen at the SOL price; for another token,
/// the amount typed.
fn amount_in(s: &AppState, t: &TokenInfo, rates: Rates) -> Option<u64> {
    if is_sol(t) {
        let eur = chosen_eur(s).ok()?;
        return sol_amount(eur, rates.eur_per_sol?, t.decimals);
    }
    parse_units(&s.slip.pay_amount, t.decimals).filter(|a| *a > 0)
}

/// The swap quote for exactly the amount paid (an older answer is not used).
fn swap_now(s: &AppState, rates: Rates) -> Option<Result<SwapQuote, String>> {
    let t = paying(s)?;
    let a = amount_in(s, &t, rates)?;
    let w = s
        .slip
        .swap
        .as_ref()
        .filter(|w| w.mint == t.mint && w.amount_in == a)?;
    Some(w.quote.clone())
}

/// The USDC stake the desk is asked for: the euros chosen at the rate, or, paying in another
/// token, the swap's `min_out_u` (L4-3); a copy's leaves room for its fee in the swap
/// ([`copy_stake_for`], L9). `None` below the minimum.
fn quote_stake(s: &AppState, rates: Rates, copy: bool) -> Option<u64> {
    let stake = match paying(s) {
        None => usdc_for_eur(chosen_eur(s).ok()?, rates.eur_per_usd),
        Some(_) => match swap_now(s, rates)? {
            Ok(q) if copy => copy_stake_for(q.min_out_u),
            Ok(q) => q.min_out_u,
            Err(_) => return None,
        },
    };
    (stake >= MIN_STAKE_U).then_some(stake)
}

/// What the desk is asked: the parlay of every pick (the drawer has no singles, D-490); in copy
/// mode the tip's legs with `copied_from`, which the desk prices as a copy (its fee, its checks).
fn request(s: &AppState, rates: Rates, copy: Option<&CopyTarget>) -> Option<QuoteRequest> {
    let legs = legs_of(s, copy);
    if legs.is_empty() {
        return None;
    }
    let stake_u = quote_stake(s, rates, copy.is_some())?;
    Some(QuoteRequest {
        legs: legs
            .iter()
            .map(|l| QuoteRequestLeg {
                symbol: l.symbol.clone(),
                side: l.side,
            })
            .collect(),
        stake_u,
        token_in: None,
        copied_from: copy.map(|c| c.ticket.clone()),
        title: None,
        wallet: None,
    })
}

/// The desk's answer for exactly `req`, if it has arrived.
fn current_answer(s: &AppState, req: &QuoteRequest) -> Option<Result<Quote, String>> {
    s.slip
        .lines
        .first()
        .filter(|l| l.req == *req && s.slip.lines.len() == 1)
        .map(|l| l.quote.clone())
}

/// The shell's slip pill: "Total 4.55", the parlay's total as the user last saw it.
pub fn slip_bar_odds(s: &AppState) -> Option<String> {
    s.slip.bar_odds_u(&s.tray).map(format::odds)
}

/// The slider's top, whole euros: the stake whose payout stays under the desk's cap at the
/// slip's total (truncated), at least the minimum; the default before any total.
fn max_stake_eur(total_u: Option<u64>, eur_per_usd: Option<u64>) -> u32 {
    let min = u32::try_from(MIN_STAKE_U / format::MICRO).unwrap_or(5);
    let Some(t) = total_u.filter(|t| *t > format::MICRO) else {
        return MAX_STAKE_EUR;
    };
    let usdc = u128::from(MAX_PAYOUT_U) * u128::from(format::MICRO) / u128::from(t);
    let eur = match eur_per_usd {
        Some(r) => usdc * u128::from(r) / 1_000_000,
        None => usdc,
    };
    u32::try_from(eur / u128::from(format::MICRO))
        .unwrap_or(u32::MAX)
        .max(min)
}

// ---------------------------------------------------------------- picks as the drawer names them

/// A spread line with a real minus sign ("Real Madrid −3.5").
fn pretty(label: &str) -> String {
    label.replace(" -", " \u{2212}")
}

/// The card's odds now for a leg (0 when the card does not price it).
fn card_odds_now(g: &Game, leg: &TrayLeg) -> u64 {
    let listed = g
        .markets
        .iter()
        .flat_map(|m| &m.picks)
        .find(|p| p.symbol == leg.symbol && p.side == leg.side)
        .map(|p| p.odds_u);
    // A desk before L6: winner picks only.
    listed.unwrap_or_else(|| {
        if leg.symbol == g.symbol {
            g.pick(leg.pick).map_or(0, |p| p.odds_u)
        } else {
            0
        }
    })
}

/// One pick as the drawer shows it.
#[derive(Debug, Clone, PartialEq)]
struct PickView {
    key: String,
    team: String,
    league: Option<String>,
    /// "vs Fenerbahçe", "vs Olympiacos (Spread)": a winner pick names no market (31 Oct, user
    /// request: no "(Outcome 1)" / "(Moneyline)").
    detail: String,
    /// The card's value now (display; the quote is binding).
    value_u: u64,
    /// The value the user last saw, when the card's has moved since.
    was_u: Option<u64>,
}

fn pick_view(leg: &TrayLeg, card: Option<&Card>) -> PickView {
    let game = card.and_then(|c| c.games.iter().find(|g| g.game_id == leg.game_id));
    let now_u = game.map_or(0, |g| card_odds_now(g, leg));
    let other = match leg.pick {
        Pick::Away => Pick::Home,
        Pick::Home => Pick::Away,
    };
    let (team, detail) = match (leg.kind, game) {
        (MarketKind::Winner, Some(g)) => {
            // The market pick names the outcome: home "1", away "2", a draw (no team) "X".
            let mp = g
                .markets
                .iter()
                .filter(|m| m.kind == MarketKind::Winner)
                .flat_map(|m| &m.picks)
                .find(|p| p.symbol == leg.symbol && p.side == leg.side);
            match mp.map(|p| p.team) {
                Some(None) => (
                    mp.map_or_else(|| "Draw".to_string(), |p| p.label.clone()),
                    format!("{} vs {}", g.home.name, g.away.name),
                ),
                _ => (
                    g.team(leg.pick).name.clone(),
                    format!("vs {}", g.team(other).name),
                ),
            }
        }
        (MarketKind::Spread, Some(g)) => (
            pretty(&leg.team),
            format!("vs {} (Spread)", g.team(other).name),
        ),
        // A soccer draw (7.1a's `Tray::toggle_outcome`): "Draw", the matchup alone beneath.
        (MarketKind::Winner, None) if leg.team == "Draw" => {
            (leg.team.clone(), leg.opponent.clone())
        }
        (MarketKind::Winner, None) => (leg.team.clone(), format!("vs {}", leg.opponent)),
        (MarketKind::Spread, None) => (pretty(&leg.team), format!("vs {} (Spread)", leg.opponent)),
        (MarketKind::Total, _) => (pretty(&leg.team), format!("{} (Total)", leg.opponent)),
        (MarketKind::Other, _) => (pretty(&leg.team), leg.opponent.clone()),
    };
    let value_u = if now_u > 0 { now_u } else { leg.card_odds_u };
    let was_u =
        (now_u > 0 && leg.card_odds_u > 0 && now_u != leg.card_odds_u).then_some(leg.card_odds_u);
    PickView {
        key: leg.game_id.clone(),
        team,
        league: game.and_then(|g| g.league.clone()),
        detail,
        value_u,
        was_u,
    }
}

/// The drawer's picks, toned across the slip (guidelines §2).
#[component]
fn Picks(views: Vec<PickView>, #[props(default)] locked: bool) -> Element {
    let mut state = use_app_state();
    let tones = outcome_tones(&views.iter().map(|v| v.value_u).collect::<Vec<_>>());
    rsx! {
        div { class: "slip-picks", id: "slip-picks", role: "list", aria_label: "Your picks",
            for (i, v) in views.into_iter().enumerate() {
                {
                    let key = v.key.clone();
                    rsx! {
                        div { key: "{v.key}", class: "slip-pick", role: "listitem",
                            PickRow {
                                team: v.team.clone(),
                                league: v.league.clone(),
                                detail: v.detail.clone(),
                                value: format::odds(v.value_u),
                                tone: tones.get(i).copied().unwrap_or(OutcomeTone::Neutral),
                                onremove: if locked {
                                    None
                                } else {
                                    Some(EventHandler::new(move |_: MouseEvent| state.write().tray.remove(&key)))
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- actions

/// "Accept": the desk's total now is the one the user saw, and so are the card's for each pick.
fn accept(mut state: Signal<AppState>) {
    let card = card_of(&state.peek());
    let mut s = state.write();
    s.slip.accept();
    let Some(card) = card else {
        return;
    };
    let mut copying = COPYING.write();
    let legs = match copying.as_mut() {
        Some(c) => &mut c.legs,
        None => &mut s.tray.legs,
    };
    for leg in legs {
        let now = card
            .games
            .iter()
            .find(|g| g.game_id == leg.game_id)
            .map_or(0, |g| card_odds_now(g, leg));
        if now > 0 {
            leg.card_odds_u = now;
        }
    }
}

fn clear(mut state: Signal<AppState>) {
    let mut s = state.write();
    s.tray.legs.clear();
    s.tray.tip = false;
    s.tray.tip_name.clear();
}

/// Design builds (`SCHEDINA_DESIGN_FIXTURES`): the mockup's three picks and €25, once, when the
/// card and the rate are known.
fn seed_design(mut state: Signal<AppState>, rate: Option<u64>) {
    #[derive(Deserialize)]
    struct DesignSlip {
        stake_u: u64,
        always_accept_better: bool,
        odds_before_u: u64,
        picks: Vec<DesignPick>,
    }
    #[derive(Deserialize)]
    struct DesignPick {
        game_id: String,
        symbol: String,
        side: Side,
        odds_u: u64,
        odds_before_u: Option<u64>,
    }
    let (Some(json), Some(rate)) = (fixtures::design::SLIP, rate) else {
        return;
    };
    let card = {
        let s = state.peek();
        if s.slip.seeded || !s.tray.legs.is_empty() {
            return;
        }
        let Some(card) = card_of(&s) else {
            return;
        };
        card
    };
    let mut s = state.write();
    s.slip.seeded = true;
    let Ok(d) = serde_json::from_str::<DesignSlip>(json) else {
        return;
    };
    for p in d.picks {
        let Some(g) = card.games.iter().find(|g| g.game_id == p.game_id) else {
            continue;
        };
        let Some((kind, mp)) = g.markets.iter().find_map(|m| {
            m.picks
                .iter()
                .find(|x| x.symbol == p.symbol && x.side == p.side)
                .map(|x| (m.kind, x))
        }) else {
            continue;
        };
        // As the event page (6.1b) writes a leg: a total has no team (Over stands on the away
        // side, D-321); a spread's or total's `team` is its label, a total's `opponent` the
        // matchup.
        let pick = mp.team.unwrap_or(match p.side {
            Side::Long => Pick::Away,
            Side::Short => Pick::Home,
        });
        let other = match pick {
            Pick::Away => Pick::Home,
            Pick::Home => Pick::Away,
        };
        let (team, opponent) = match kind {
            MarketKind::Winner => (g.team(pick).name.clone(), g.team(other).name.clone()),
            MarketKind::Spread => (pretty(&mp.label), g.team(other).name.clone()),
            _ => (pretty(&mp.label), super::event::matchup(g)),
        };
        s.tray.legs.push(TrayLeg {
            game_id: g.game_id.clone(),
            symbol: p.symbol,
            pick,
            side: p.side,
            team,
            opponent,
            start_ms: g.start_ms,
            card_odds_u: p.odds_before_u.unwrap_or(p.odds_u),
            kind,
        });
    }
    // The fixture's stake is USDC; the slip chooses euros (truncated to the cent: €25).
    let eur = kit::money::eur_u(d.stake_u, rate);
    s.tray.stake_u = eur - eur % 10_000;
    s.slip.accept_better = d.always_accept_better;
    s.slip.seed_seen_odds_u = Some(d.odds_before_u);
}

// ---------------------------------------------------------------- the engine

/// Asks the desk for the slip's quote (and the swap's, paying in a token). Renders nothing;
/// mounted once, by [`BetDrawer`].
#[component]
fn SlipEngine() -> Element {
    let mut state = use_app_state();
    let desk = use_desk();
    let demo = desk.is_demo();
    let fx_sig = try_use_context::<kit::money::FxSignal>();
    let sol_sig = try_use_context::<kit::money::SolRateSignal>();
    let rates = use_memo(move || Rates {
        eur_per_usd: fx_sig.and_then(|f| f.read().as_ref().map(|x| x.eur_per_usd_ppm)),
        eur_per_sol: sol_sig.and_then(|r| *r.read()),
    });

    let tokens_desk = desk.clone();
    use_hook(move || {
        spawn(async move {
            if let Ok(list) = tokens_desk.tokens().await {
                state.write().slip.tokens = list;
            }
        })
    });
    use_effect(move || {
        // Subscribe to the card and the rate; the seed runs once.
        let _ = state.read().card;
        seed_design(state, rates().eur_per_usd);
    });

    let req = use_memo(move || request(&state.read(), rates(), COPYING.read().as_ref()));
    let swap_want = use_memo(move || {
        let s = state.read();
        let t = paying(&s)?;
        Some((t.mint.clone(), amount_in(&s, &t, rates())?))
    });
    let swap_desk = desk.clone();
    let mut swap_res = use_resource(move || {
        let desk = swap_desk.clone();
        let want = swap_want();
        async move {
            let Some((mint, amount_in)) = want else {
                return;
            };
            let quote = desk
                .swap_quote(&mint, amount_in)
                .await
                .map_err(|e| e.plain(None));
            state.write().slip.swap = Some(SwapAnswer {
                mint,
                amount_in,
                quote,
            });
        }
    });
    let quote_desk = desk.clone();
    let mut quotes = use_resource(move || {
        let desk = quote_desk.clone();
        let req = req();
        async move {
            let Some(r) = req else {
                return;
            };
            let card = card_of(&state.peek());
            let q = desk.quote(&r).await.map_err(|e| e.plain(card.as_ref()));
            state.write().slip.apply(vec![(r, q)]);
        }
    });
    // Live desk only: ask again every 5 s while the drawer is open (the bundled demo answers
    // never change).
    use_future(move || async move {
        if demo {
            return;
        }
        loop {
            gloo_timers::future::sleep(REQUOTE).await;
            if !*DRAWER.peek() || PLACING.peek().is_some() {
                continue;
            }
            if swap_want.peek().is_some() {
                swap_res.restart();
            }
            if req.peek().is_some() {
                quotes.restart();
            }
        }
    });
    rsx! {}
}

// ---------------------------------------------------------------- the drawer

/// The schedina being placed, as the drawer showed it when Bet was tapped.
#[derive(Debug, Clone, PartialEq)]
struct Placing {
    /// The submission it follows (`AppState::submission`).
    id: u64,
    views: Vec<PickView>,
    total: String,
    payout: String,
    /// The CTA's words when it was tapped ("Bet • €25.00").
    label: String,
    /// The stake as tapped ("€25.00", "0.1840 SOL").
    stake: String,
}

/// The bet drawer, mounted once by the shell (its engine runs whether it is open or not). It
/// follows the schedina it placed: minted opens the celebration.
#[component]
pub fn BetDrawer() -> Element {
    let state = use_app_state();
    let route = use_route::<Route>();
    let chrome = Chrome::for_route(&route);
    // Only screens with the slip pill show the drawer; `/tray` and `/confirm` hand over to the
    // sportsbook with it open.
    let copy_page = matches!(route, Route::CopyTip { .. } | Route::CopyEvent { .. });
    let here = chrome.slip_bar || copy_page || matches!(route, Route::Tray {} | Route::Confirm {});
    use_effect(use_reactive!(|here| {
        if !here && *DRAWER.peek() {
            close_drawer();
        }
    }));
    let n = match COPYING.read().as_ref() {
        Some(c) => c.legs.len(),
        None => state.read().tray.legs.len(),
    };
    let placing = PLACING.read().is_some();
    use_effect(use_reactive!(|n, placing| {
        // An emptied slip closes the drawer (the next pick must not pop it open).
        if n == 0 && !placing && *DRAWER.peek() {
            close_drawer();
        }
    }));
    // Follow the schedina being placed: minted opens the celebration; a failure before it landed
    // shows in the drawer (the slip still holds its picks); a refund keeps the drawer's view.
    use_effect(move || {
        let s = state.read();
        let Some(p) = PLACING.read().clone() else {
            return;
        };
        let sub = s.submission.as_ref().filter(|x| x.id == p.id);
        let stage = sub
            .and_then(|x| x.pending.as_ref())
            .and_then(|pd| s.progress.get(pd))
            .map(|x| x.stage);
        let failed = sub.is_none_or(|x| matches!(x.step, SubmitStep::Failed(_)));
        drop(s);
        if stage == Some(Stage::Minted) {
            *PLACING.write() = None;
            close_drawer();
            navigator().push(Route::Confirm {});
        } else if failed {
            *PLACING.write() = None;
        }
    });

    let open = DRAWER() && here && (n > 0 || placing);
    let above_nav = chrome.nav;
    rsx! {
        SlipEngine {}
        if open {
            if placing {
                PlacingSheet { above_nav }
            } else {
                DrawerSheet { above_nav }
            }
        }
    }
}

/// The drawer while a schedina is being placed (1 Nov, user request): a small sheet with the
/// loading ring and the steps ticking off as they complete (`kit::Sequencer`, the same steps as
/// `/confirm`), the bet in one line under its title. Minted, the celebration opens; a refund says
/// so and "Done" closes the sheet; a failure returns to the drawer with its reason.
#[component]
fn PlacingSheet(above_nav: bool) -> Element {
    let state = use_app_state();
    let wallet = use_wallet_api();
    let Some(p) = PLACING() else {
        return rsx! {};
    };
    let (steps, refunded, copy) = {
        let s = state.read();
        let sub = s.submission.as_ref().filter(|x| x.id == p.id);
        let stage = sub
            .and_then(|x| x.pending.as_ref())
            .and_then(|pd| s.progress.get(pd))
            .map(|x| x.stage);
        let steps = sub
            .map(|x| super::confirm::place_steps(x, stage, s.mock_fills(), wallet.sponsored()))
            .unwrap_or_default();
        let copy = sub.is_some_and(|x| matches!(x.kind, PlayKind::Copy { .. }));
        (steps, stage == Some(Stage::Refunded), copy)
    };
    let n = p.views.len();
    let title = match (refunded, copy) {
        (true, _) => "Not filled",
        (false, true) => "Copying the schedina",
        (false, false) => "Placing your bet",
    };
    let subtitle = format!(
        "{} \u{2022} Total {} \u{2022} {}",
        format::picks(n as u64),
        p.total,
        p.stake
    );
    let done = move |_| {
        *PLACING.write() = None;
        close_drawer();
    };
    let footer = refunded.then(|| rsx! { PurpleCTA { label: "Done", id: "gioca", onclick: done } });
    rsx! {
        BottomSheet {
            open: true,
            id: "bet-drawer",
            title: "Schedina",
            icon: Sym::Receipt,
            badge: rsx! { Badge { kind: BadgeKind::Picks(n as u32) } },
            above_nav,
            onclose: move |_| close_drawer(),
            footer,
            span { class: "slip-placing", hidden: true }
            div { class: "slip-progress",
                kit::Sequencer { title: title.to_string(), subtitle, steps }
            }
            if refunded {
                p { class: "slip-line", id: "refunded", role: "status",
                    "The hedge did not go through, so your stake was refunded in full."
                }
            }
        }
    }
}

/// The drawer itself (see the module docs).
#[component]
fn DrawerSheet(above_nav: bool) -> Element {
    let mut state = use_app_state();
    let desk = use_desk();
    let wallet = use_wallet_api();
    let stream = use_stream_cmd();
    let rpc = use_rpc_context();
    let fx = kit::money::use_fx();
    let sol_rate = kit::money::use_sol_rate();
    let rates = Rates {
        eur_per_usd: fx,
        eur_per_sol: sol_rate,
    };
    let demo = desk.is_demo();
    // Bet arms once the sheet has risen (proposed D-541): the slip pill and the docked CTA share
    // the spot above the nav, so a double tap on the pill must not place a bet, and nothing is
    // tapped (or captured) while the CTA is still sliding up behind the nav.
    let mut risen = use_signal(|| false);
    use_hook(move || {
        spawn(async move {
            gloo_timers::future::sleep(SHEET_RISE).await;
            risen.set(true);
        })
    });

    let copy = COPYING();
    let s = state.read();
    let legs = legs_of(&s, copy.as_ref()).to_vec();
    let n = legs.len();
    let card = card_of(&s);
    let req = request(&s, rates, copy.as_ref());
    let answer = req.as_ref().and_then(|r| current_answer(&s, r));
    let quote: Option<Quote> = answer.as_ref().and_then(|a| a.as_ref().ok().cloned());
    let quote_error: Option<String> = answer.as_ref().and_then(|a| a.as_ref().err().cloned());
    let views: Vec<PickView> = legs.iter().map(|l| pick_view(l, card.as_ref())).collect();
    let accept_better = s.slip.accept_better;
    // D-361: a re-quote below the total the user accepted, or a pick whose card value left the
    // one the user saw (a better one passes silently), waits for Accept.
    let desk_moved = if answer.is_some() { s.slip.moved() } else { 0 };
    let legs_moved = views
        .iter()
        .filter(|v| v.was_u.is_some_and(|w| !(accept_better && v.value_u > w)))
        .count();
    let moved = if answer.is_some() {
        desk_moved.max(legs_moved)
    } else {
        0
    };
    let seen_total = if copy.is_some() {
        None
    } else {
        s.slip.bar_odds_u(&s.tray)
    };
    let pay = paying(&s);
    let sol = sol_token(&s);
    let others_tokens = other_tokens(&s);
    let show = match &pay {
        Some(t) if is_sol(t) => Currency::Sol,
        _ => match SHOW() {
            Currency::Sol => Currency::Eur,
            c => c,
        },
    };
    let token_mode = pay.as_ref().is_some_and(|t| !is_sol(t));
    let token_amount = pay.as_ref().and_then(|t| amount_in(&s, t, rates));
    let swap = swap_now(&s, rates);
    let swap_line: Option<SwapLine> = match (pay.clone(), token_amount, swap.clone()) {
        (Some(token), Some(amount_in), Some(Ok(quote))) => Some(SwapLine {
            token,
            amount_in,
            quote,
        }),
        _ => None,
    };
    let pay_amount = s.slip.pay_amount.clone();
    let stake_eur = s.tray.stake_u;
    let busy = s.submission.as_ref().is_some_and(|x| x.busy());
    let last_failure = LAST_TRY.read().as_ref().and_then(|(id, picks)| {
        let sub = s.submission.as_ref().filter(|x| x.id == *id)?;
        let holds = picks.len() == legs.len()
            && picks
                .iter()
                .zip(&legs)
                .all(|((sym, side), l)| *sym == l.symbol && *side == l.side);
        match &sub.step {
            SubmitStep::Failed(e) if holds => Some(e.clone()),
            _ => None,
        }
    });
    drop(s);

    // The slider: whole euros, from the minimum stake to the payout cap at the slip's total.
    let min = u32::try_from(MIN_STAKE_U / format::MICRO).unwrap_or(5);
    let max = max_stake_eur(quote.as_ref().map(|q| q.odds_offered_u).or(seen_total), fx);
    let value = u32::try_from(stake_eur / format::MICRO)
        .unwrap_or(u32::MAX)
        .clamp(min, max);
    use_effect(use_reactive!(|max| {
        let over = state.peek().tray.stake_u > u64::from(max) * format::MICRO;
        if over {
            state.write().tray.stake_u = u64::from(max) * format::MICRO;
        }
    }));

    let eur_text = format::eur(stake_eur);
    let usdc_stake = usdc_for_eur(stake_eur, fx);
    let rate_line = sol_rate.map(|r| format!("(1 SOL = {})", format::eur(r)));
    let (amount, approx) = match (&pay, show) {
        (Some(t), Currency::Sol) => (
            eur_text.clone(),
            token_amount.map(|a| match &rate_line {
                Some(r) => format!("Pay {} SOL {r}", format::token(a, t.decimals)),
                None => format!("Pay {} SOL", format::token(a, t.decimals)),
            }),
        ),
        (_, Currency::Usdc) => (
            kit::money::usdc_text(usdc_stake),
            Some(format!("\u{2248} {eur_text}")),
        ),
        _ => (
            eur_text.clone(),
            kit::money::approx_sol(stake_eur, sol_rate).map(|a| match &rate_line {
                Some(r) => format!("{a} {r}"),
                None => a,
            }),
        ),
    };
    let verb = if copy.is_some() {
        "Copy Schedina"
    } else {
        "Bet"
    };
    let label = match (&pay, show) {
        (Some(t), _) => match token_amount {
            Some(a) if is_sol(t) => format!("{verb} \u{2022} {} SOL", format::token(a, t.decimals)),
            Some(a) => format!(
                "{verb} \u{2022} {} {}",
                format::token(a, t.decimals),
                t.symbol
            ),
            None => format!(
                "{verb} with {}",
                if is_sol(t) { "SOL" } else { t.symbol.as_str() }
            ),
        },
        (None, Currency::Usdc) => format!("{verb} \u{2022} {}", kit::money::usdc_text(usdc_stake)),
        (None, _) => format!("{verb} \u{2022} {eur_text}"),
    };
    // mockBONK sits in the coin selector (31 Oct: no "Pay with mockBONK" link under the stake).
    let bonk = others_tokens.first().cloned();
    let currencies: Vec<Currency> = Currency::ALL
        .into_iter()
        .filter(|c| *c != Currency::Sol || sol.is_some())
        .chain(bonk.is_some().then_some(Currency::Bonk))
        .collect();
    // A coin from the pill (the stake block's or the token block's): BONK opens the token block,
    // SOL pays in mockSOL, EUR and USDC in mock USDC. Either block replaces the other: the focus
    // stays in the sheet.
    let pick_coin = move |c: Currency| {
        let was_token = state.peek().tray.pay.as_ref().is_some_and(|t| !is_sol(t));
        if c == Currency::Bonk {
            state.write().tray.pay = bonk.clone();
            document::eval(FOCUS_DRAWER_JS);
            return;
        }
        let sol = sol_token(&state.peek());
        *SHOW.write() = c;
        state.write().tray.pay = if c == Currency::Sol { sol } else { None };
        if was_token {
            document::eval(FOCUS_DRAWER_JS);
        }
    };
    let total = quote
        .as_ref()
        .map(|q| format::odds(q.odds_offered_u))
        .unwrap_or_else(|| "\u{2013}".to_string());
    let payout = quote
        .as_ref()
        .map(|q| kit::money::pair(q.payout_u, fx).0)
        .unwrap_or_else(|| "\u{2013}".to_string());
    // The swap's own fee line (spec 8), only while paying in a token: its fee, what is paid, the
    // stake it makes.
    let staked_u = req.as_ref().map(|r| r.stake_u);
    let swap_words = swap_line.as_ref().map(|l| {
        let sym = if is_sol(&l.token) {
            "SOL"
        } else {
            l.token.symbol.as_str()
        };
        (
            kit::money::pair(l.quote.fee_u, fx).0,
            format!("{} {sym}", format::token(l.amount_in, l.token.decimals)),
            kit::money::usdc_text(staked_u.unwrap_or(l.quote.min_out_u)),
        )
    });
    let name_ok = copy.is_some() || name_problem(&state.read()).is_none();
    let ready = quote.is_some() && moved == 0 && (pay.is_none() || swap_line.is_some()) && name_ok;
    let stake_error = if pay.is_none() {
        chosen_eur(&state.peek()).err()
    } else {
        None
    };
    let total_moved = quote.as_ref().map(|q| format::odds(q.odds_offered_u));
    let copy_fee = copy.as_ref().map(|_| {
        quote
            .as_ref()
            .map(|q| kit::money::pair(q.copy_fee_u, fx).0)
            .unwrap_or_else(|| "\u{2013}".to_string())
    });
    let picks_moved = views.iter().filter(|v| v.was_u.is_some()).count();

    let place = {
        let desk = desk.clone();
        let req = req.clone();
        let quote = quote.clone();
        let pay = pay.clone();
        let swap_line = swap_line.clone();
        let views = views.clone();
        let total = total.clone();
        let payout = payout.clone();
        let label = label.clone();
        let copy = copy.clone();
        move |_: MouseEvent| {
            let (Some(req), Some(shown)) = (req.clone(), quote.clone()) else {
                return;
            };
            let kind = match &copy {
                Some(c) => PlayKind::Copy {
                    original: c.ticket.clone(),
                    legs: c.shown.clone(),
                },
                None => play_kind(&state.peek()),
            };
            let picks: Vec<(String, Side)> = req
                .legs
                .iter()
                .map(|l| (l.symbol.clone(), l.side))
                .collect();
            match (pay.clone(), swap_line.clone()) {
                (None, _) => {
                    flows::start_play(desk.clone(), wallet, stream, state, req, shown, kind)
                }
                // Demo mode: no program, the example pending plays as for euros.
                (Some(_), Some(_)) if demo => {
                    let kind = if copy.is_some() {
                        kind
                    } else {
                        PlayKind::Plain
                    };
                    flows::start_play(desk.clone(), wallet, stream, state, req, shown, kind)
                }
                (Some(_), Some(line)) => {
                    start_swap_play(desk.clone(), wallet, rpc, state, line, req, shown, kind)
                }
                (Some(_), None) => return,
            }
            let (id, pending) = {
                let s = state.peek();
                let sub = s.submission.as_ref();
                (sub.map(|x| x.id), sub.and_then(|x| x.pending.clone()))
            };
            // Demo: the stream replays the example pending from its start; an earlier replay's
            // frames for it must not count.
            if demo && let Some(p) = pending {
                state.write().progress.remove(&p);
            }
            let Some(id) = id else {
                return;
            };
            *LAST_TRY.write() = Some((id, picks));
            *PLACING.write() = Some(Placing {
                id,
                views: views.clone(),
                total: total.clone(),
                payout: payout.clone(),
                label: label.clone(),
                stake: label
                    .split(" \u{2022} ")
                    .nth(1)
                    .unwrap_or_default()
                    .to_string(),
            });
        }
    };

    let footer = rsx! {
        PurpleCTA {
            label: label.clone(),
            id: "gioca",
            disabled: !ready || busy || !risen(),
            onclick: place,
        }
    };

    let copying = copy.is_some();
    let onclear = (!copying).then(|| {
        EventHandler::new(move |_: ()| {
            clear(state);
            close_drawer();
        })
    });
    let mut rows = vec![
        SummaryRow::total(total.clone()).with_id("slip-total"),
        SummaryRow::payout("Payout", payout.clone(), None).with_id("slip-payout"),
    ];
    if let Some(fee) = copy_fee.clone() {
        rows.insert(
            1,
            SummaryRow::plain("Copy fee (2%)", fee).with_id("slip-copy-fee"),
        );
    }
    rsx! {
        BottomSheet {
            open: true,
            id: "bet-drawer",
            title: if copying { "Copy Schedina" } else { "Schedina" },
            icon: if copying { Sym::ContentCopy } else { Sym::Receipt },
            badge: rsx! { Badge { kind: BadgeKind::Picks(n as u32) } },
            above_nav,
            onclose: move |_| close_drawer(),
            onclear,
            footer,
            Picks { views: views.clone(), locked: copying }
            if moved > 0 {
                if let Some(t) = total_moved.clone() {
                    PriceMoved {
                        moved: picks_moved.max(1),
                        odds: t,
                        compact: true,
                        id: "price-moved",
                        onaccept: move |_| {
                            accept(state);
                            // The banner (and its focused Accept) goes: keep the focus in the
                            // sheet, so Escape still closes it.
                            document::eval(FOCUS_DRAWER_JS);
                        },
                    }
                }
            }
            if token_mode {
                if let Some(t) = pay.clone() {
                    section { class: "k-stake compact slip-token-stake", id: "stake", aria_labelledby: "stake-title",
                        div { class: "k-stake-head",
                            h3 { class: "k-stake-title", id: "stake-title", "Stake amount" }
                            div { class: "slip-token-end",
                                kit::CoinPill {
                                    currency: Currency::Bonk,
                                    currencies: currencies.clone(),
                                    id: "stake-currency",
                                    label: "Pay in",
                                    disabled: busy,
                                    onchange: pick_coin.clone(),
                                }
                                IconButton {
                                    icon: Icon::Close,
                                    label: "Pay in euros",
                                    size: IconSize::Sm,
                                    id: "pay-eur",
                                    onclick: move |_| {
                                        state.write().tray.pay = None;
                                        // The button goes with the token block: keep the focus
                                        // (and Escape) in the sheet, as after Accept (7.2a-r).
                                        document::eval(FOCUS_DRAWER_JS);
                                    },
                                }
                            }
                        }
                        div { class: "slip-token-field",
                            input {
                                class: "slip-token-amount",
                                id: "token-amount",
                                r#type: "text",
                                inputmode: "decimal",
                                autocomplete: "off",
                                placeholder: "0",
                                aria_label: "Amount of {t.symbol} to pay",
                                value: "{pay_amount}",
                                oninput: move |e| state.write().slip.pay_amount = e.value(),
                            }
                            if !pay_amount.is_empty() {
                                IconButton {
                                    icon: Icon::Close,
                                    label: "Clear the amount",
                                    size: IconSize::Xs,
                                    id: "token-amount-clear",
                                    onclick: move |_| state.write().slip.pay_amount.clear(),
                                }
                            }
                        }
                        StakeHint { token: t.clone(), text: pay_amount.clone(), amount_in: token_amount, swap: swap.clone() }
                    }
                    TokenLine { token: t.clone(), demo }
                }
            } else {
                StakeAmount {
                    id: "stake",
                    compact: true,
                    amount,
                    approx,
                    value,
                    min,
                    max,
                    presets: PRESETS.to_vec(),
                    currency: show,
                    currencies,
                    error: stake_error,
                    oncurrency: pick_coin,
                    onchange: move |v: u32| {
                        state.write().tray.stake_u = u64::from(v.clamp(min, max)) * format::MICRO;
                    },
                }
                if let Some(t) = pay.clone().filter(is_sol) {
                    StakeHint { token: t, text: String::new(), amount_in: token_amount, swap: swap.clone(), sol: true }
                }
            }
            SummaryRows { compact: true, id: "slip-summary", rows }
            div { class: "slip-quiet",
                if !copying {
                    TipToggle {}
                }
                if let Some((fee_text, paid, stake_text)) = swap_words.clone() {
                    p { class: "slip-line", id: "swap-fee",
                        "Swap fee {fee_text} \u{b7} {paid} stakes "
                        span { id: "swap-stake", "{stake_text}" }
                    }
                }
                if demo && !crate::config::DESIGN_FIXTURES {
                    if let Some(q) = quote.as_ref() {
                        p { class: "slip-line", id: "demo-note",
                            "Demo: the desk's example quote ({format::picks(q.n)}, {kit::money::usdc_text(q.stake_u)}), whatever you picked."
                        }
                    }
                }
            }
            if let Some(e) = last_failure {
                p { class: "slip-error", id: "submit-error", role: "alert", "{e}" }
                // A failure for want of test money says "Get test USDC": the faucet is here.
                if !demo {
                    super::faucet::WalletLine {}
                }
            } else if let Some(e) = quote_error {
                p { class: "slip-error", id: "quote-error", role: "alert", "{e}" }
            } else if answer.is_none() && req.is_some() {
                p { class: "slip-line", id: "pricing", role: "status", "Pricing\u{2026}" }
            }
        }
    }
}

/// While paying in `token`: what is missing before the desk can be asked (an amount, a valid
/// one, a swap big enough for the minimum stake), the swap quote's refusal, or what the swap
/// makes of it.
#[component]
fn StakeHint(
    token: TokenInfo,
    text: String,
    amount_in: Option<u64>,
    swap: Option<Result<SwapQuote, String>>,
    /// Paying in mockSOL: the amount comes from the euros chosen, and a good swap needs no line
    /// (the swap fee line says what it makes).
    #[props(default)]
    sol: bool,
) -> Element {
    let t = token;
    let fx = kit::money::use_fx();
    if sol && amount_in.is_none() {
        return rsx! {
            p { class: "k-stake-approx slip-hint", role: "status", "Waiting for the SOL price\u{2026}" }
        };
    }
    if !sol && text.trim().is_empty() {
        return rsx! {
            p { class: "k-stake-approx slip-hint", "Enter how much {t.symbol} to pay: it is swapped into test USDC for the stake." }
        };
    }
    if amount_in.is_none() {
        return rsx! {
            p { class: "slip-error", role: "alert", "Enter an amount of {t.symbol}, up to {t.decimals} decimals." }
        };
    }
    match swap {
        None => {
            rsx! { p { class: "k-stake-approx slip-hint", role: "status", "Pricing the swap\u{2026}" } }
        }
        Some(Err(e)) => rsx! { p { class: "slip-error", role: "alert", "{e}" } },
        Some(Ok(q)) if q.min_out_u < MIN_STAKE_U => rsx! {
            p { class: "slip-error", role: "alert",
                "This makes a stake of {kit::money::usdc_text(q.min_out_u)}; the minimum is {kit::money::usdc_text(MIN_STAKE_U)}. Pay more {t.symbol}."
            }
        },
        Some(Ok(_)) if sol => rsx! {},
        Some(Ok(q)) => rsx! {
            p { class: "k-stake-approx slip-hint", "\u{2248} {kit::money::pair(q.min_out_u, fx).0} stake after the swap" }
        },
    }
}

// ---------------------------------------------------------------- routes

/// `/tray` (kept as an alias): the sportsbook with the drawer open.
#[component]
pub fn Tray() -> Element {
    use_hook(|| {
        open_drawer();
        navigator().replace(Route::Card {});
    });
    rsx! {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stake_slider_tops_at_the_payout_cap() {
        // 4.55 at €0.921068 per USDC: 500 / 4.55 = 109.89 USDC = €101.21.
        assert_eq!(max_stake_eur(Some(4_550_000), Some(921_068)), 101);
        assert_eq!(max_stake_eur(None, Some(921_068)), MAX_STAKE_EUR);
        // A long shot never goes under the minimum.
        assert_eq!(max_stake_eur(Some(400_000_000), None), 5);
    }

    #[test]
    fn sol_amounts_read_cleanly() {
        // €25 at €135.80 per SOL, 9 decimals: 0.18409… → 0.1840.
        assert_eq!(sol_amount(25_000_000, 135_800_000, 9), Some(184_000_000));
        assert_eq!(sol_amount(25_000_000, 0, 9), None);
        assert_eq!(usdc_for_eur(25_000_000, Some(921_068)), 27_150_000);
    }
}
