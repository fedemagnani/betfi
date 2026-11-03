//! Screens and the shell. Visible text follows spec 7's rules: team names, numbers and euros; no
//! sides, no symbols, no order-book words; never the word "odds" on a v2 screen (guidelines §2).

mod card;
mod celebrate;
mod confirm;
mod copy;
mod dev_wallet;
mod event;
mod faucet;
mod house;
pub mod kit;
mod login;
mod my_schedine;
mod nav;
mod sentiment;
mod ticket;
mod tip_toggle;
mod tipsters;
mod token_select;
mod tray;

pub use card::Card;
pub use confirm::Confirm;
pub use dev_wallet::DevWallet;
pub use event::EventScreen;
pub use house::{Account, House, ProfileEdit};
pub use kit::{KitGallery, KitShell};
pub use my_schedine::MySchedine;
pub use ticket::TicketScreen;
pub use tipsters::{CopyEvent, CopyTip, TipsterPage, Tipsters};
pub use tray::Tray;

use dioxus::prelude::*;

use crate::Route;
use crate::desk::Desk;
use crate::state::{AppState, Link as StreamLink};
use crate::stream::StreamCmd;

pub fn use_app_state() -> Signal<AppState> {
    use_context::<Signal<AppState>>()
}

pub fn use_desk() -> Desk {
    use_context::<Desk>()
}

pub fn use_stream_cmd() -> Coroutine<StreamCmd> {
    use_context::<Coroutine<StreamCmd>>()
}

/// The BetFi shell (7.0a, V-7): one column, 390 px design width, centred at 480 px on wider
/// screens. Top-level screens: the app bar and the live ticker (sticky), the routed screen, then
/// the floating slip pill above the bottom nav (fixed). Sub-pages: a back header with the
/// page's title. Which chrome a route gets is [`nav::Chrome::for_route`].
#[component]
pub fn Shell() -> Element {
    let route = use_route::<Route>();
    let chrome = nav::Chrome::for_route(&route);
    let desk = use_desk();
    let state = use_app_state();

    // U-4: the euro rate, once, for every screen (`kit::money::use_fx`).
    let mut fx: kit::money::FxSignal = use_context_provider(|| Signal::new(None));
    // V-3: the SOL rate, for every "≈ SOL" (`kit::money::use_sol_rate`).
    let mut sol: kit::money::SolRateSignal = use_context_provider(|| Signal::new(None));
    {
        let desk = desk.clone();
        use_hook(move || {
            spawn(async move {
                if let Ok(f) = desk.fx().await {
                    fx.set(Some(f));
                }
                sol.set(sol_rate(&desk).await);
            })
        });
    }
    let balance = nav::use_balance();

    let picks = state.read().tray.legs.len();
    // The event page's own CTA opens the drawer, so the pill steps aside there (one bottom
    // action, D-505); the drawer itself stays available (`chrome.slip_bar`).
    let event_cta = matches!(route, Route::EventScreen { .. }) && event::cta_shown();
    let slip_bar = chrome.slip_bar && picks > 0 && !event_cta;
    let mut class = String::from("app");
    if chrome.nav {
        class.push_str(" with-nav");
    }
    if slip_bar {
        class.push_str(" with-slipbar");
    }
    // Every screen shown counts, so a back button knows whether the app has one behind it.
    use_effect(use_reactive!(|route| {
        let _ = route;
        kit::note_screen_shown();
    }));
    // A sub-page's way back, by route (a shared link has no history to go back to). The copy
    // page goes back where it was opened from (a receipt, a tipster, the board), else the board.
    let (back_to, close, history) = match &route {
        Route::Tray {} => (Route::Card {}, true, false),
        // Through `/me` to Account's bets (`/account#bets`, L8-4).
        Route::TicketScreen { .. } => (Route::MySchedine {}, false, false),
        Route::CopyTip { .. } => (Route::Tipsters {}, false, true),
        _ => (Route::Card {}, false, false),
    };
    nav::use_simulated_ticker();
    let ticker: Vec<kit::TickerLine> = nav::ticker_lines(
        &nav::ticker_frames(),
        fx.read().as_ref().map(|f| f.eur_per_usd_ppm),
        *sol.read(),
    )
    .into_iter()
    .zip(nav::ticker_keys())
    .map(|(l, key)| kit::TickerLine { key, ..l })
    .collect();
    // Design builds say so, for the visual scripts: they must not change the mockup's seeded
    // slip (l6-contracts L6-7, items 13 and 19).
    let fixtures = crate::config::DESIGN_FIXTURES.then_some("design");
    let title = chrome.title.map(str::to_string);
    // The sub-page header's end slot: the event page's share button (D-500).
    let end = match &route {
        Route::EventScreen { game_id } => Some(rsx! {
            event::EventShare { game_id: game_id.clone() }
        }),
        _ => None,
    };
    rsx! {
        div { class, "data-fixtures": fixtures,
            header { class: if chrome.app_header { "app-mobile-top app-top" } else { "app-mobile-top app-top sub" },
                if chrome.app_header {
                    nav::ShellAppBar { balance }
                    kit::LiveTicker { lines: ticker, idle: "Bets placed on BetFi show here" }
                } else {
                    kit::SubHeader { to: back_to, close, history, title, end }
                }
            }
            main { class: "app-main screen", id: "main", Outlet::<Route> {} }
            // 7.1b: the bet drawer (and the slip's engine), a bottom sheet over this screen.
            tray::BetDrawer {}
            // 1 Nov: the log-in popup (Google, Phantom, other wallets), opened by any "Log in".
            login::LoginModal {}
            if slip_bar || chrome.nav {
                div { class: "app-bottom",
                    if slip_bar && !tray::drawer_open() {
                        div { class: "app-pill",
                            kit::SlipPill {
                                picks,
                                total: tray::slip_bar_odds(&state.read()),
                                onclick: move |_| tray::open_drawer(),
                            }
                        }
                    }
                    if chrome.nav {
                        kit::BottomNav { active: chrome.tab }
                    }
                }
            }
        }
    }
}

/// `/fx`'s `eur_per_sol_ppm` (L7-1, 7.0c), read on its own so the app need not wait for the
/// typed mirror: the bundled answer in demo and design builds, the desk's otherwise. `None` if
/// absent (the desk predates 7.0c) or unreachable: no "≈ SOL" is shown then.
async fn sol_rate(desk: &Desk) -> Option<u64> {
    #[derive(serde::Deserialize)]
    struct FxSol {
        eur_per_sol_ppm: Option<u64>,
    }
    if desk.is_demo() {
        return serde_json::from_str::<FxSol>(crate::desk::fixtures::FX)
            .ok()?
            .eur_per_sol_ppm;
    }
    let base = crate::config::desk_url()?;
    let resp = gloo_net::http::Request::get(&format!("{base}/fx"))
        .send()
        .await
        .ok()?;
    if !resp.ok() {
        return None;
    }
    resp.json::<FxSol>().await.ok()?.eur_per_sol_ppm
}

/// Live-update indicator for the `/stream` socket (shown on Account).
#[component]
pub fn LinkDot() -> Element {
    let state = use_app_state();
    let (class, title) = match state.read().link {
        StreamLink::Live => ("dot live", "Live updates on".to_string()),
        StreamLink::Demo => ("dot demo", "Demo: scripted updates".to_string()),
        StreamLink::Connecting => ("dot wait", "Connecting".to_string()),
        StreamLink::Retrying { in_ms, .. } => (
            "dot down",
            format!("Offline, retrying in {} s", in_ms.div_ceil(1000)),
        ),
    };
    rsx! {
        span { style: "display: inline-flex; align-items: center; gap: 8px; font-size: var(--fs-md); color: var(--on-surface-variant)",
            span { class: "{class}", "aria-hidden": "true" }
            "{title}"
        }
    }
}
