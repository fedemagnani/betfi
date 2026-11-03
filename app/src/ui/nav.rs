//! The shell's pieces (7.0a): which chrome each route gets, the balance and the app bar around
//! it, the ticker's lines, the honesty pill. The Account screen is `house::Account` (7.1f).

use dioxus::prelude::*;

use super::kit::{self, NavTab, TickerLine};
use super::{use_app_state, use_desk};
use crate::Route;
use crate::config;
use crate::desk::fixtures;
use crate::flows;
use crate::format;
use crate::wallet::use_wallet_api;

/// What surrounds a screen (V-7). Top-level screens wear the app bar, the live ticker and the
/// bottom nav, as the sportsbook and the tipster profile are drawn; sub-pages (the event, the
/// slip, a receipt, copy) get a back header instead, as the event, copy and celebration screens
/// are. Desktop is the same single column, centred at 480 px.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chrome {
    /// The tab to mark active in the bottom nav.
    pub tab: Option<NavTab>,
    /// The app bar and the ticker on top (else the back header).
    pub app_header: bool,
    /// The bottom nav.
    pub nav: bool,
    /// The floating slip pill, when the slip holds picks.
    pub slip_bar: bool,
    /// The back header's title ("Event Detail", "Copy Confirm").
    pub title: Option<&'static str>,
}

impl Chrome {
    pub fn for_route(route: &Route) -> Chrome {
        let top = |tab, slip_bar| Chrome {
            tab: Some(tab),
            app_header: true,
            nav: true,
            slip_bar,
            title: None,
        };
        let sub = |tab, title| Chrome {
            tab,
            app_header: false,
            nav: false,
            slip_bar: false,
            title,
        };
        match route {
            Route::Card {} | Route::KitShell {} => top(NavTab::Home, true),
            // The tipster profile wears the app bar and the nav, as drawn.
            Route::Tipsters {} | Route::TipsterPage { .. } => top(NavTab::Tipsters, false),
            // `/me` redirects to Account's bets (L8-4).
            Route::MySchedine {}
            | Route::Account {}
            | Route::House {}
            | Route::ProfileEdit {}
            | Route::DevWallet {}
            | Route::KitGallery {} => top(NavTab::Account, false),
            Route::Tray {} => sub(Some(NavTab::Home), Some("Your slip")),
            Route::Confirm {} => sub(Some(NavTab::Home), Some("Bet Success")),
            Route::TicketScreen { .. } => sub(Some(NavTab::Account), Some("Receipt")),
            // A tipster's parlay and its locked games (L9): no slip pill, the copy drawer instead.
            Route::CopyTip { .. } => sub(Some(NavTab::Tipsters), Some("Parlay")),
            Route::CopyEvent { .. } => sub(Some(NavTab::Tipsters), Some("Event Detail")),
            // The event page keeps the drawer; its pill shows only while the page's own CTA does
            // not (D-505, the shell).
            Route::EventScreen { .. } => Chrome {
                slip_bar: true,
                ..sub(Some(NavTab::Home), Some("Event Detail"))
            },
        }
    }
}

/// The wallet's mock-USDC balance as the app bar shows it.
#[derive(Debug, Clone, PartialEq)]
pub enum Balance {
    /// Demo mode: no wallet, no chain.
    Demo,
    Disconnected,
    Loading,
    Ready(u64),
    Unknown,
}

/// Bumped when something moved the balance that no ticket shows (a faucet claim), so every
/// [`use_balance`] reads it again.
static BALANCE_EPOCH: GlobalSignal<u32> = Signal::global(|| 0);

/// Ask every balance on screen (app bar, Account) to read the chain again.
pub fn balance_changed() {
    *BALANCE_EPOCH.write() += 1;
}

/// The balance, read from chain when a wallet is connected (live mode), and again whenever one
/// of the user's tickets changes state or a faucet claim lands. A design build shows the
/// mockup's balance.
pub fn use_balance() -> Balance {
    let desk = use_desk();
    let wallet = use_wallet_api();
    let state = use_app_state();
    let demo = desk.is_demo();
    let ledger = use_memo(move || {
        state
            .read()
            .my_tickets()
            .iter()
            .map(|t| (t.ticket.clone(), t.state, t.paid_u))
            .collect::<Vec<_>>()
    });
    let read = use_resource(move || async move {
        let _ = ledger();
        let _ = BALANCE_EPOCH();
        match wallet.public_key() {
            Some(_) if !demo => Some(flows::usdc_balance(wallet).await),
            _ => None,
        }
    });
    if let Some(w) = fixtures::design::WALLET {
        #[derive(serde::Deserialize)]
        struct DesignWallet {
            usdc_u: u64,
        }
        return serde_json::from_str::<DesignWallet>(w)
            .map(|d| Balance::Ready(d.usdc_u))
            .unwrap_or(Balance::Unknown);
    }
    if demo {
        return Balance::Demo;
    }
    if wallet.public_key().is_none() {
        return Balance::Disconnected;
    }
    match &*read.read() {
        None | Some(None) => Balance::Loading,
        Some(Some(Ok(Some(u)))) => Balance::Ready(*u),
        Some(Some(Ok(None))) => Balance::Ready(0),
        Some(Some(Err(_))) => Balance::Unknown,
    }
}

/// The app bar's figure (user request, 31 Oct): every asset in the wallet at what it would fetch
/// now, in micro-USDC: the test USDC plus, for each other listed token held (mockSOL, mockBONK),
/// the desk's swap quote for the whole balance (`usdc_out_u`, after the pool's fee). A token whose
/// quote fails is left out. `None` until the reads land, or without a wallet or in demo mode.
pub fn use_portfolio_usdc(usdc_u: Option<u64>) -> Option<u64> {
    let desk = use_desk();
    let wallet = use_wallet_api();
    let state = use_app_state();
    let demo = desk.is_demo();
    let ledger = use_memo(move || {
        state
            .read()
            .my_tickets()
            .iter()
            .map(|t| (t.ticket.clone(), t.state, t.paid_u))
            .collect::<Vec<_>>()
    });
    let others = use_resource(move || {
        let desk = desk.clone();
        async move {
            let _ = ledger();
            let _ = BALANCE_EPOCH();
            let _ = super::faucet::TOKENS_EPOCH();
            let user = wallet.public_key().filter(|_| !demo)?;
            let tokens = desk.tokens().await.ok()?;
            let mut total: u64 = 0;
            for t in tokens.iter().filter(|t| !t.usdc_mint) {
                let Ok(mint) = t.mint.parse::<schedina_tx::Pubkey>() else {
                    continue;
                };
                let held = match wallet
                    .token_balance(&schedina_tx::pda::ata(&user, &mint))
                    .await
                {
                    Ok(Some(b)) if b > 0 => b,
                    _ => continue,
                };
                if let Ok(q) = desk.swap_quote(&t.mint, held).await {
                    total = total.saturating_add(q.usdc_out_u);
                }
            }
            Some(total)
        }
    });
    let usdc = usdc_u?;
    match &*others.read() {
        Some(Some(o)) => Some(usdc.saturating_add(*o)),
        Some(None) => Some(usdc),
        None => None,
    }
}

/// The app bar's balance (user request, 31 Oct): one figure, every asset in the wallet in euros
/// ([`use_portfolio_usdc`]), with no TEST chip and no SOL beside it. The honesty pill stays on
/// Account (D-311) and on every receipt and celebration (V-8). Demo builds show "Demo"; without
/// a wallet the connect button takes the pill's place.
#[component]
pub fn ShellAppBar(balance: Balance) -> Element {
    let fx = kit::money::use_fx();
    let usdc = match &balance {
        Balance::Ready(u) => Some(*u),
        _ => None,
    };
    let portfolio = use_portfolio_usdc(usdc);
    match balance {
        Balance::Demo => rsx! {
            kit::AppBar {
                balance: rsx! {
                    Link { to: Route::Account {}, class: "k-balance-pill", id: "balance", aria_label: "Demo mode: example data, no wallet",
                        span { class: "k-balance-eur", "Demo" }
                    }
                },
            }
        },
        Balance::Disconnected => rsx! {
            kit::AppBar { balance: rsx! { super::login::LoginButton { bar: true } }, avatar: false }
        },
        // Logged in, the figure not read yet: the pill still changes the wallet.
        Balance::Loading | Balance::Unknown => rsx! {
            kit::AppBar { onbalance: kit::Action(super::login::open_login) }
        },
        Balance::Ready(u) => {
            let total = portfolio.unwrap_or(u);
            let eur = match fx {
                Some(rate) => format::eur(kit::money::eur_u(total, rate)),
                None => kit::money::usdc_text(total),
            };
            rsx! {
                kit::AppBar { eur, test: false, onbalance: kit::Action(super::login::open_login) }
            }
        }
    }
}

/// One `/stream` feed frame as the ticker reads it (L7-1: kinds `mint` and `copy` carry `who`,
/// `amount_u`, `team` and `odds_u`). Only "placed" lines are shown (guidelines §3).
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct PlacedFrame {
    pub kind: String,
    #[serde(default)]
    pub who: Option<String>,
    #[serde(default)]
    pub amount_u: Option<u64>,
    #[serde(default)]
    pub team: Option<String>,
    #[serde(default)]
    pub odds_u: Option<u64>,
}

/// Design builds: the mockup's ticker (`fixtures/design/ticker.json`, feed frames). `None`
/// otherwise, so the design data never enters a demo or live build (D-314).
const DESIGN_TICKER: Option<&str> = if config::DESIGN_FIXTURES {
    Some(include_str!("../../fixtures/design/ticker.json"))
} else {
    None
};

/// The ticker's frames: the design fixture in a design build (fixed, for the visual scripts);
/// elsewhere the simulated bets ([`use_simulated_ticker`]), newest first.
pub fn ticker_frames() -> Vec<PlacedFrame> {
    match DESIGN_TICKER {
        Some(j) => serde_json::from_str::<Vec<PlacedFrame>>(j).unwrap_or_default(),
        None => SIMULATED.read().iter().map(|(_, f)| f.clone()).collect(),
    }
}

/// The ticker's keys, aligned with [`ticker_frames`]: a new bet is a new element (it slides in).
pub fn ticker_keys() -> Vec<u64> {
    match DESIGN_TICKER {
        Some(_) => (0..64).collect(),
        None => SIMULATED.read().iter().map(|(k, _)| *k).collect(),
    }
}

/// Simulated bets (1 Nov, user request: "stream fake live bets by different traders"), newest
/// first, with a running key. Test-token devnet only; none of them is a real bet.
static SIMULATED: GlobalSignal<Vec<(u64, PlacedFrame)>> = Signal::global(Vec::new);
/// Lines kept (more than one screen holds).
const SIMULATED_KEPT: usize = 8;

/// Traders the simulated bets come from: handles, and short addresses as the copiers sheet shows
/// wallets without one.
const TRADERS: [&str; 28] = [
    "sol_bull99",
    "degencall",
    "elon_parlay",
    "ser_crypto",
    "lorenzo_hoops",
    "giulia_moneyline",
    "parlay_pete",
    "nba_oracle",
    "luca_bets",
    "tifoso88",
    "anon_whale",
    "bonk_maxi",
    "chiara_odds",
    "rafa_picks",
    "gm_sports",
    "dunkwatch",
    "ultras_09",
    "fade_me",
    "moonbag_marco",
    "ste_value",
    "7xK9…94Lp",
    "3bF8…12qR",
    "Fq3m…8uWc",
    "9vQa…Lm3T",
    "Hk4P…x9Zs",
    "Bn7R…k2Yd",
    "Ux9e…T4hQ",
    "Jr2L…v6Np",
];

/// A random index below `n` (`n` > 0).
fn pick(n: usize) -> usize {
    ((js_sys::Math::random() * n as f64) as usize).min(n.saturating_sub(1))
}

/// One simulated bet on tonight's card: a trader, a priced pick (its team, a line's label or the
/// draw) at the card's value now, a stake from €5 to €600, small ones far more often.
fn simulated_frame(card: &crate::desk::types::Card) -> Option<PlacedFrame> {
    let picks: Vec<(String, u64)> = card
        .games
        .iter()
        .flat_map(|g| {
            g.markets.iter().flat_map(move |m| {
                m.picks.iter().filter(|p| p.odds_u > 0).map(move |p| {
                    let name = match p.team {
                        Some(t) if m.kind == crate::desk::types::MarketKind::Winner => {
                            g.team(t).name.clone()
                        }
                        _ => p.label.replace(" -", " \u{2212}"),
                    };
                    (name, p.odds_u)
                })
            })
        })
        .collect();
    // A desk before L6 (the demo card): the winner picks only.
    let picks = if picks.is_empty() {
        card.games
            .iter()
            .flat_map(|g| {
                g.picks
                    .iter()
                    .filter(|p| p.odds_u > 0)
                    .map(move |p| (g.team(p.pick).name.clone(), p.odds_u))
            })
            .collect()
    } else {
        picks
    };
    if picks.is_empty() {
        return None;
    }
    let (team, odds_u) = picks[pick(picks.len())].clone();
    // Log-uniform from 5 to 600 USDC, to the cent.
    let usdc = 5.0 * (120f64).powf(js_sys::Math::random());
    let amount_u = ((usdc * 100.0).round() as u64) * 10_000;
    Some(PlacedFrame {
        kind: "mint".into(),
        who: Some(TRADERS[pick(TRADERS.len())].to_string()),
        amount_u: Some(amount_u),
        team: Some(team),
        odds_u: Some(odds_u),
    })
}

/// Streams simulated bets into the ticker: three at once when the card arrives, then one every
/// 3 to 6 s. Nothing in design builds (their fixed fixture line stays).
pub fn use_simulated_ticker() {
    let state = super::use_app_state();
    use_future(move || async move {
        if DESIGN_TICKER.is_some() {
            return;
        }
        let mut key = 0u64;
        loop {
            let card = match &state.peek().card {
                crate::state::Load::Ready(c) => Some(c.clone()),
                _ => None,
            };
            if let Some(card) = card {
                let n = if key == 0 { 3 } else { 1 };
                for _ in 0..n {
                    if let Some(f) = simulated_frame(&card) {
                        key += 1;
                        let mut lines = SIMULATED.write();
                        lines.insert(0, (key, f));
                        lines.truncate(SIMULATED_KEPT);
                    }
                }
            }
            let wait = if key == 0 {
                1_000.0
            } else {
                3_000.0 + 3_000.0 * js_sys::Math::random()
            };
            gloo_timers::future::sleep(std::time::Duration::from_millis(wait as u64)).await;
        }
    });
}

/// The ticker's lines: the stake in SOL when the SOL rate is known ("5.0 SOL", as drawn), else
/// in euros; a frame without the four fields is skipped.
pub fn ticker_lines(
    frames: &[PlacedFrame],
    eur_per_usd_ppm: Option<u64>,
    eur_per_sol_ppm: Option<u64>,
) -> Vec<TickerLine> {
    frames
        .iter()
        .filter(|f| f.kind == "mint" || f.kind == "copy")
        .filter_map(|f| {
            let (who, amount_u, team, odds_u) =
                (f.who.clone()?, f.amount_u?, f.team.clone()?, f.odds_u?);
            let amount = match eur_per_usd_ppm {
                Some(fx) => {
                    let eur_u = kit::money::eur_u(amount_u, fx);
                    match eur_per_sol_ppm.filter(|r| *r > 0) {
                        Some(r) => short_sol(kit::money::sol_u(eur_u, r)),
                        None => format::eur(eur_u),
                    }
                }
                None => kit::money::usdc_text(amount_u),
            };
            let logo = ticker_logo(&team);
            Some(TickerLine {
                key: 0,
                logo,
                who,
                amount,
                team,
                odds: format::odds(odds_u),
            })
        })
        .collect()
}

/// The logo for a ticker pick: the team's, or a spread's team with its line ("Real Madrid
/// −3.5" → the crest and " −3.5"). A total or a draw has none.
fn ticker_logo(team: &str) -> Option<(&'static str, String)> {
    let find = |name: &str| kit::team_logo(name, false).or_else(|| kit::team_logo(name, true));
    if let Some(src) = find(team) {
        return Some((src, String::new()));
    }
    let (name, line) = team.rsplit_once(' ')?;
    let numeric = line
        .trim_start_matches(['+', '-', '\u{2212}'])
        .parse::<f64>()
        .is_ok();
    numeric.then(|| find(name).map(|src| (src, format!(" {line}"))))?
}

/// The ticker's SOL: one decimal from 1 SOL up ("5.0 SOL"), else the money module's three.
fn short_sol(sol_u: u64) -> String {
    if sol_u >= 1_000_000 {
        let tenths = (sol_u + 50_000) / 100_000;
        format!("{}.{} SOL", tenths / 10, tenths % 10)
    } else {
        kit::money::sol_text(sol_u)
    }
}
