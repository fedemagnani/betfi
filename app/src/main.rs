//! Schedina web app, shown as BetFi (plan 1.F, U-6): the shell, the screens on C4 data, the
//! global `Signal<AppState>` fed by one `/stream` socket, the desk client, the PWA manifest and
//! the wallet (dioxus-solana on devnet) behind one Rust surface, `wallet::Wallet`. The look is
//! the BetFi design system (`ui::kit`, `assets/main.css`, 6.0a).

mod burner;
mod config;
mod desk;
mod flows;
mod format;
mod privy;
mod state;
mod stream;
mod ui;
mod wallet;

use dioxus::prelude::*;
use dioxus_solana::rpc::RpcProvider;
use dioxus_solana::{AppIdentity, WalletProvider};

use desk::Desk;
use state::{AppState, Load};
use stream::{StreamCmd, use_stream};
use ui::{
    Account, Card, Confirm, CopyEvent, CopyTip, DevWallet, EventScreen, House, KitGallery,
    KitShell, MySchedine, ProfileEdit, Shell, TicketScreen, TipsterPage, Tipsters, Tray,
};

const MAIN_CSS: Asset = asset!("/assets/main.css");
/// One stylesheet per L6-1 screen step, so parallel steps never edit the same CSS file (L6-6).
const SCREEN_CSS: [Asset; 7] = [
    asset!("/assets/screens/home.css"),
    asset!("/assets/screens/event.css"),
    asset!("/assets/screens/slip.css"),
    asset!("/assets/screens/my-bets.css"),
    asset!("/assets/screens/tipsters.css"),
    asset!("/assets/screens/account.css"),
    asset!("/assets/screens/celebration.css"),
];

/// Registers `public/sw.js` (served at the site root so its scope is the whole app). Failure is
/// silent: the app works without it.
const SW_REGISTER: &str = "if ('serviceWorker' in navigator) { \
     navigator.serviceWorker.register('/sw.js').catch(() => {}); }";

/// The four MVP screens and the v1 ones (spec 7), Account (6.0a placeholder until 6.1f), and the
/// dev-only wallet page and component kit.
#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[layout(Shell)]
        #[route("/")]
        Card {},
        /// One game's markets (Winner / Spread / Total) and money traded (6.1b).
        #[route("/event/:game_id")]
        EventScreen { game_id: String },
        #[route("/tray")]
        Tray {},
        #[route("/confirm")]
        Confirm {},
        #[route("/me")]
        MySchedine {},
        /// A schedina's receipt; shareable (`/ticket/<Ticket PDA>`).
        #[route("/ticket/:id")]
        TicketScreen { id: String },
        #[route("/tipsters")]
        Tipsters {},
        #[route("/tipsters/:wallet")]
        TipsterPage { wallet: String },
        /// A tipster's parlay (`/copy/<Ticket PDA>`, L9): its games locked, its copiers, Copy.
        #[route("/copy/:id")]
        CopyTip { id: String },
        /// One game of a tipster's parlay, the tipster's pick locked (L9).
        #[route("/copy/:id/event/:game_id")]
        CopyEvent { id: String, game_id: String },
        /// The wallet (its worth in €) and the faucet, nothing else (31 Oct, user request).
        #[route("/account")]
        Account {},
        /// The house's numbers (spec 7: vault, bets, hedger, fees), off Account since 31 Oct;
        /// no screen links here.
        #[route("/house")]
        House {},
        /// The profile editor (handle and links, D-471…D-473), off Account since 31 Oct.
        #[route("/profile")]
        ProfileEdit {},
        #[route("/dev/wallet")]
        DevWallet {},
        /// Every kit component in every state (dev tools only).
        #[route("/dev/kit")]
        KitGallery {},
        /// The shell around an empty screen (dev tools only).
        #[route("/dev/shell")]
        KitShell {},
}

fn main() {
    // A panic in a release build aborts as a bare "RuntimeError: unreachable": say what and where
    // in the console first.
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&format!("panic: {info}").into());
    }));
    privy::capture_oauth_return();
    dioxus::launch(App);
}

/// Head tags and the wallet/RPC providers. The cluster is fixed for the app's lifetime.
#[component]
fn App() -> Element {
    let (cluster, rpc_warning) = use_hook(config::cluster);
    rsx! {
        document::Title { "BetFi" }
        document::Meta { name: "viewport", content: "width=device-width, initial-scale=1, viewport-fit=cover" }
        document::Meta { name: "theme-color", content: "#0B0D17" }
        document::Meta { name: "color-scheme", content: "dark" }
        document::Meta { name: "description", content: "Basketball parlays, priced live and settled on Solana." }
        document::Link { rel: "manifest", href: "/manifest.webmanifest" }
        document::Link { rel: "icon", href: "/icon.svg", r#type: "image/svg+xml" }
        document::Link { rel: "apple-touch-icon", href: "/icon-192.png" }
        ui::kit::Fonts {}
        document::Stylesheet { href: MAIN_CSS }
        for css in SCREEN_CSS {
            document::Stylesheet { href: css }
        }
        RpcProvider { cluster,
            WalletProvider { autoconnect: true, app_identity: Some(AppIdentity::named("BetFi")),
                Root { rpc_warning }
            }
        }
    }
}

/// Owns the global state, the desk client, the stream and the router.
#[component]
fn Root(rpc_warning: Option<String>) -> Element {
    let desk = use_context_provider(Desk::from_config);
    let mut state = use_signal(|| AppState {
        user: if desk.is_demo() {
            desk::demo_user()
        } else {
            None
        },
        ..AppState::default()
    });
    use_context_provider(|| state);
    use_hook(|| {
        document::eval(SW_REGISTER);
    });
    let stream = use_stream(state, desk.clone());
    use_context_provider(|| stream);

    // The Card loads at start (and again from its retry button), then a live desk's is read
    // again every 30 s while the app is on screen.
    {
        let desk = desk.clone();
        use_hook(move || spawn(load_card(desk, state)));
    }
    {
        let desk = desk.clone();
        use_hook(move || spawn(refresh_card(desk, state)));
    }
    // What others think about each game (L5-4), once the card is in.
    {
        let desk = desk.clone();
        use_hook(move || spawn(load_sentiment(desk, state)));
    }
    // The desk's profile, for the "mock" label: once, retried every 10 s until it answers.
    {
        let desk = desk.clone();
        use_hook(move || spawn(load_health(desk, state)));
    }

    // Follow the wallet: the user's tickets and the stream subscription. Demo mode keeps the
    // example tickets' owner whatever wallet is connected.
    let wallet = wallet::use_wallet_api();
    // 8.D: back from Google, or a Privy session stored here: reconnect the Privy wallet.
    use_hook(|| (!desk.is_demo()).then(|| spawn(privy::resume(wallet))));
    let effect_desk = desk.clone();
    use_effect(move || {
        // The only reactive read: the effect re-runs when the connected account changes.
        let user = wallet.public_key().map(|pk| pk.to_string());
        if effect_desk.is_demo() || state.peek().user == user {
            return;
        }
        {
            let mut s = state.write();
            s.user = user.clone();
            s.tickets_load = Load::Loading;
        }
        if let Some(user) = user {
            stream.send(StreamCmd::Subscribe(user.clone()));
            spawn(load_tickets(effect_desk.clone(), state, user));
        }
    });

    // Demo mode: the example tickets are loaded once.
    if desk.is_demo() {
        let desk = desk.clone();
        use_hook(move || {
            if let Some(user) = state.peek().user.clone() {
                spawn(load_tickets(desk, state, user));
            }
        });
    }

    rsx! {
        if let Some(w) = rpc_warning {
            div { class: "banner warn", "{w}" }
        }
        Router::<Route> {}
    }
}

/// Basketball is off the live Card for now (3 Nov, user request); demo builds keep their NBA
/// fixtures.
const SHOW_BASKETBALL: bool = false;

/// `card` as the app shows it: only soccer while [`SHOW_BASKETBALL`] is off, on a live desk (a
/// game without a sport is an NBA one, as `card::sport_of` reads it).
fn shown_card(mut card: desk::types::Card, demo: bool) -> desk::types::Card {
    if !SHOW_BASKETBALL && !demo {
        card.games.retain(|g| g.sport.as_deref() == Some("soccer"));
    }
    card
}

/// `GET /card` into the state.
pub async fn load_card(desk: Desk, mut state: Signal<AppState>) {
    state.write().card = Load::Loading;
    let result = desk.card("all", desk::slate_date().as_deref()).await;
    state.write().card = match result {
        Ok(card) => Load::Ready(shown_card(card, desk.is_demo())),
        Err(e) => Load::Failed(e.to_string()),
    };
}

/// How often a live app reads the Card again (20 s: the desk reads live scores every 20 s).
const CARD_EVERY: std::time::Duration = std::time::Duration::from_secs(20);

/// A live desk's prices and scores move: every 20 s, while the page is visible and the Card is loaded, read
/// it again, so each pick on the slip can show its odds now against the ones the user saw
/// (D-361, polish 14). A failed read keeps the last Card; an unchanged one is not written. Demo
/// builds never refresh (their Card is fixed).
async fn refresh_card(desk: Desk, mut state: Signal<AppState>) {
    if desk.is_demo() {
        return;
    }
    loop {
        gloo_timers::future::sleep(CARD_EVERY).await;
        if !page_visible() || !matches!(state.peek().card, Load::Ready(_)) {
            continue;
        }
        let Ok(card) = desk.card("all", desk::slate_date().as_deref()).await else {
            continue;
        };
        let card = shown_card(card, false);
        let changed = !matches!(&state.peek().card, Load::Ready(c) if *c == card);
        if changed {
            state.write().card = Load::Ready(card);
        }
    }
}

/// The page is on screen (not a background tab, not minimised).
fn page_visible() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .is_none_or(|d| d.visibility_state() == web_sys::VisibilityState::Visible)
}

/// How often the live app asks `/sentiment` again (the desk follows Pascal every 15 s).
const SENTIMENT_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// `GET /sentiment` for the card's games into the state, then again every 30 s on a live desk.
/// Waits for the card; a failed call keeps the last answer.
async fn load_sentiment(desk: Desk, mut state: Signal<AppState>) {
    loop {
        let symbols: Option<Vec<String>> = match &state.peek().card {
            Load::Ready(c) => Some(c.games.iter().map(|g| g.symbol.clone()).collect()),
            _ => None,
        };
        let Some(symbols) = symbols.filter(|s| !s.is_empty()) else {
            gloo_timers::future::sleep(std::time::Duration::from_millis(500)).await;
            continue;
        };
        if let Ok(s) = desk.sentiment(&symbols).await {
            state.write().sentiment = Some(s);
        }
        if desk.is_demo() {
            return;
        }
        gloo_timers::future::sleep(SENTIMENT_EVERY).await;
    }
}

/// `GET /health` into the state (live desk only), retried every 10 s until it answers.
async fn load_health(desk: Desk, mut state: Signal<AppState>) {
    loop {
        match desk.health().await {
            Ok(Some(h)) => {
                state.write().health = Load::Ready(h);
                return;
            }
            Ok(None) => return,
            Err(e) => state.write().health = Load::Failed(e.to_string()),
        }
        gloo_timers::future::sleep(std::time::Duration::from_secs(10)).await;
    }
}

/// `GET /tickets/:user` into the state; dropped if the wallet changed meanwhile.
pub async fn load_tickets(desk: Desk, mut state: Signal<AppState>, user: String) {
    let result = desk.tickets(&user).await;
    if state.peek().user.as_deref() != Some(user.as_str()) {
        return;
    }
    let mut s = state.write();
    match result {
        Ok(resp) => s.set_tickets(&user, resp.tickets),
        Err(e) => s.tickets_load = Load::Failed(e.to_string()),
    }
}
