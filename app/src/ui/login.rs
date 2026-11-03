//! Log in (1 Nov, user request): one "Log in" button wherever a wallet is needed (the app bar,
//! Account's wallet card, Confirm's wallet line) opens a popup in focus: "Continue with Google"
//! first (Privy, 8.D: an embedded Solana wallet, no extension needed), then Phantom, then any
//! other wallet the browser has (wallet-standard discovery). It replaces dioxus-solana's
//! "Select wallet" dropdown and the separate Google button.
//!
//! Logged in, the app bar's balance pill opens the same popup to change the wallet (1 Nov, user
//! request): it names the wallet in use and its address, marks that option "Connected", connects
//! another on a tap (the new one takes the seat), and offers "Log out".
//!
//! The popup is `dioxus_primitives::dialog` (L9 design step 2): modal, named by its title and
//! described by its line, focus trapped inside, closed by Escape or a tap outside, with an
//! open/close animation; the first usable option takes the focus. It closes by itself once a
//! wallet is connected. A failed attempt says why under the options. Demo builds (no wallet) show no button.

use dioxus::prelude::*;
use dioxus_solana::{WalletState, use_wallet};

use dioxus_primitives::dialog::{DialogContent, DialogDescription, DialogRoot, DialogTitle};

use super::kit::{Sym, SymIcon};
use super::use_desk;
use crate::privy::{self, PRIVY_STATUS, PRIVY_WALLET_NAME, PrivyStatus};
use crate::wallet::{Wallet, use_wallet_api};

/// The popup is open.
static LOGIN_OPEN: GlobalSignal<bool> = Signal::global(|| false);

/// Where Phantom is installed from when the browser has none.
const PHANTOM_DOWNLOAD: &str = "https://phantom.app/download";

/// Puts the focus on the first usable option (else the close button) once the dialog shows.
const FOCUS_FIRST_JS: &str = "setTimeout(() => { const d = document.getElementById('login-dialog'); \
     const o = d && (d.querySelector('.login-option:not([disabled])') || d.querySelector('.login-close')); \
     if (o) o.focus(); }, 30);";

/// Open the log-in popup.
pub fn open_login() {
    *LOGIN_OPEN.write() = true;
}

fn close_login() {
    *LOGIN_OPEN.write() = false;
}

/// "Continue with Google": Privy's login, or its stored session back (8.D). A root-level task:
/// the page may go to Google meanwhile.
fn start_google(wallet: Wallet) {
    if matches!(*PRIVY_STATUS.peek(), PrivyStatus::Busy(_)) {
        return;
    }
    *PRIVY_STATUS.write() = PrivyStatus::Busy("Opening Google…");
    dioxus::core::spawn_forever(async move {
        // Switching providers logs out of the previous one first (1 Nov).
        wallet.log_out().await;
        let result = privy::login_or_reconnect(wallet).await;
        let navigating = result.is_ok() && wallet.public_key().is_none();
        *PRIVY_STATUS.write() = match result {
            // Google is loading (the page navigates away), or the wallet is back.
            Ok(()) if navigating => PrivyStatus::Busy("Opening Google…"),
            Ok(()) => PrivyStatus::Idle,
            Err(e) => PrivyStatus::Failed(format!("Google sign-in failed: {e}")),
        };
        if navigating {
            // Back from Google without logging in (a restored page): tappable again.
            gloo_timers::future::sleep(std::time::Duration::from_secs(8)).await;
            if *PRIVY_STATUS.peek() == PrivyStatus::Busy("Opening Google…") {
                *PRIVY_STATUS.write() = PrivyStatus::Idle;
            }
        }
    });
}

/// The "Log in" button. `bar`: the app bar's pill (`.wallet`, which the scripts read as "no
/// wallet"). `manage`: Account's wallet card, which, once connected, names the wallet and offers
/// "Log out". Otherwise nothing shows while a wallet is connected.
#[component]
pub fn LoginButton(#[props(default)] bar: bool, #[props(default)] manage: bool) -> Element {
    let desk = use_desk();
    let wallet = use_wallet_api();
    if desk.is_demo() {
        return rsx! {};
    }
    let id = if bar {
        "login-bar"
    } else if manage {
        "login-account"
    } else {
        "login"
    };
    match wallet.state() {
        WalletState::Connected(acc) if manage => rsx! {
            span { class: "login-connected",
                span { class: "acct-net", id: "wallet-name", "{acc.wallet_name}" }
                button {
                    r#type: "button",
                    class: "k-btn secondary sm",
                    id: "logout",
                    onclick: move |_| wallet.disconnect(),
                    "Log out"
                }
            }
        },
        WalletState::Connected(_) => rsx! {},
        state => {
            let connecting = matches!(state, WalletState::Connecting);
            // A Google sign-in that failed after the page came back from Google says so here too
            // (the popup is closed then).
            let failed = match (&*PRIVY_STATUS.read(), bar) {
                (PrivyStatus::Failed(e), false) => Some(e.clone()),
                _ => None,
            };
            let class = if bar {
                "k-btn secondary sm wallet login-bar"
            } else {
                "k-btn primary sm login-inline"
            };
            rsx! {
                button {
                    r#type: "button",
                    class,
                    id,
                    aria_haspopup: "dialog",
                    disabled: connecting,
                    onclick: move |_| open_login(),
                    if connecting { "Connecting…" } else { "Log in" }
                }
                if let Some(e) = failed {
                    p { class: "error small", role: "alert", "{e}" }
                }
            }
        }
    }
}

/// The popup, mounted once by the shell.
#[component]
pub fn LoginModal() -> Element {
    let wallet = use_wallet_api();
    let ds = use_wallet();
    let state = wallet.state();
    let connected = state.is_connected();
    // Logged in: the wallet in use (switch mode).
    let current = match &state {
        WalletState::Connected(acc) => Some((acc.wallet_name.clone(), acc.pubkey().to_string())),
        _ => None,
    };
    let is_current = {
        let current = current.clone();
        move |name: &str| {
            current
                .as_ref()
                .is_some_and(|(n, _)| n.eq_ignore_ascii_case(name))
        }
    };
    let google_current = is_current(PRIVY_WALLET_NAME);
    let phantom_current = is_current("phantom");
    use_effect(use_reactive!(|connected| {
        if connected && *LOGIN_OPEN.peek() {
            close_login();
        }
    }));
    let open = LOGIN_OPEN();
    use_effect(use_reactive!(|open| {
        if open {
            document::eval(FOCUS_FIRST_JS);
        }
    }));
    let found = ds.wallets();
    let phantom = found
        .iter()
        .find(|w| w.name.eq_ignore_ascii_case("phantom"))
        .cloned();
    let others: Vec<_> = found
        .iter()
        .filter(|w| !w.name.eq_ignore_ascii_case("phantom") && w.name != PRIVY_WALLET_NAME)
        .cloned()
        .collect();
    let privy = PRIVY_STATUS.read().clone();
    let google_busy = match &privy {
        PrivyStatus::Busy(t) => Some(*t),
        _ => None,
    };
    let connecting = matches!(state, WalletState::Connecting);
    let error = match (&privy, &state) {
        (PrivyStatus::Failed(e), _) => Some(e.clone()),
        (_, WalletState::Error(e)) => Some(format!("The wallet did not connect: {e}.")),
        _ => None,
    };
    // Another wallet: log out of the current provider (Phantom, Google…) first, then connect.
    let connect = move |name: String| {
        if !connecting {
            dioxus::core::spawn_forever(async move {
                wallet.log_out().await;
                ds.connect(name);
            });
        }
    };
    rsx! {
        DialogRoot {
            class: "login-backdrop",
            is_modal: true,
            open: Some(open),
            on_open_change: move |v: bool| {
                if !v {
                    close_login();
                }
            },
            DialogContent { id: "login-dialog", class: "login-dialog",
            div { class: "login-head",
                DialogTitle { id: "login-title", class: "login-title", if current.is_some() { "Your wallet" } else { "Log in" } }
                button {
                    r#type: "button",
                    class: "k-icon-btn login-close",
                    aria_label: "Close",
                    onclick: move |_| close_login(),
                    SymIcon { sym: Sym::Close, size: 18 }
                }
            }
            match &current {
                Some((name, addr)) => rsx! {
                    DialogDescription { id: "login-sub", class: "login-sub",
                        "Connected with "
                        b { "{name}" }
                        " \u{b7} {crate::format::short(addr)}. Choose another to switch."
                    }
                },
                None => rsx! {
                    DialogDescription { id: "login-sub", class: "login-sub", "Bet on devnet with test tokens. No real money." }
                },
            }
            div { class: "login-options", role: "group", aria_label: "Ways to log in",
                button {
                    r#type: "button",
                    class: if google_current { "login-option current" } else { "login-option" },
                    id: "login-google",
                    "aria-busy": google_busy.is_some(),
                    "aria-current": google_current,
                    disabled: google_busy.is_some() || connecting || google_current,
                    onclick: move |_| start_google(wallet),
                    span { class: "login-icon google", GoogleMark { size: 22 } }
                    span { class: "login-text",
                        span { class: "login-name", "{google_busy.unwrap_or(\"Continue with Google\")}" }
                        span { class: "login-note", if google_current { "Connected" } else { "No wallet needed" } }
                    }
                    SymIcon { sym: if google_current { Sym::CheckCircle } else { Sym::ChevronRight }, size: 20 }
                }
                match phantom {
                    Some(w) => rsx! {
                        button {
                            r#type: "button",
                            class: if phantom_current { "login-option current" } else { "login-option" },
                            id: "login-phantom",
                            "aria-current": phantom_current,
                            disabled: connecting || google_busy.is_some() || phantom_current,
                            onclick: {
                                let name = w.name.clone();
                                move |_| connect(name.clone())
                            },
                            span { class: "login-icon",
                                if let Some(icon) = &w.icon {
                                    img { src: "{icon}", alt: "", width: "28", height: "28" }
                                } else {
                                    SymIcon { sym: Sym::Wallet, size: 22 }
                                }
                            }
                            span { class: "login-text",
                                span { class: "login-name", "Phantom" }
                                span { class: "login-note",
                                    if phantom_current { "Connected" } else if connecting { "Approve in Phantom…" } else { "Browser wallet" }
                                }
                            }
                            SymIcon { sym: if phantom_current { Sym::CheckCircle } else { Sym::ChevronRight }, size: 20 }
                        }
                    },
                    None => rsx! {
                        a {
                            class: "login-option",
                            id: "login-phantom",
                            href: PHANTOM_DOWNLOAD,
                            target: "_blank",
                            rel: "noopener noreferrer",
                            span { class: "login-icon", SymIcon { sym: Sym::Wallet, size: 22 } }
                            span { class: "login-text",
                                span { class: "login-name", "Phantom" }
                                span { class: "login-note", "Not installed: get Phantom" }
                            }
                            SymIcon { sym: Sym::OpenInNew, size: 18 }
                        }
                    },
                }
                for w in others {
                    button {
                        key: "{w.name}",
                        r#type: "button",
                        class: if is_current(&w.name) { "login-option current" } else { "login-option" },
                        "aria-current": is_current(&w.name),
                        disabled: connecting || google_busy.is_some() || is_current(&w.name),
                        onclick: {
                            let name = w.name.clone();
                            move |_| connect(name.clone())
                        },
                        span { class: "login-icon",
                            if let Some(icon) = &w.icon {
                                img { src: "{icon}", alt: "", width: "28", height: "28" }
                            } else {
                                SymIcon { sym: Sym::Wallet, size: 22 }
                            }
                        }
                        span { class: "login-text",
                            span { class: "login-name", "{w.name}" }
                            span { class: "login-note", if is_current(&w.name) { "Connected" } else { "Browser wallet" } }
                        }
                        SymIcon { sym: if is_current(&w.name) { Sym::CheckCircle } else { Sym::ChevronRight }, size: 20 }
                    }
                }
            }
            if let Some(e) = error {
                p { class: "login-error", id: "login-error", role: "alert", "{e}" }
            }
            if current.is_some() {
                button {
                    r#type: "button",
                    class: "k-btn ghost login-logout",
                    id: "login-logout",
                    onclick: move |_| {
                        wallet.disconnect();
                        close_login();
                    },
                    "Log out"
                }
            }
            }
        }
    }
}

/// Google's "G" (brand colours, as Google's sign-in guidelines ask).
#[component]
fn GoogleMark(size: u32) -> Element {
    rsx! {
        svg {
            width: "{size}",
            height: "{size}",
            view_box: "0 0 48 48",
            "aria-hidden": "true",
            "focusable": "false",
            path { fill: "#EA4335", d: "M24 9.5c3.54 0 6.71 1.22 9.21 3.6l6.85-6.85C35.9 2.38 30.47 0 24 0 14.62 0 6.51 5.38 2.56 13.22l7.98 6.19C12.43 13.72 17.74 9.5 24 9.5z" }
            path { fill: "#4285F4", d: "M46.98 24.55c0-1.57-.15-3.09-.38-4.55H24v9.02h12.94c-.58 2.96-2.26 5.48-4.78 7.18l7.73 6c4.51-4.18 7.09-10.36 7.09-17.65z" }
            path { fill: "#FBBC05", d: "M10.53 28.59c-.48-1.45-.76-2.99-.76-4.59s.27-3.14.76-4.59l-7.98-6.19C.92 16.46 0 20.12 0 24c0 3.88.92 7.54 2.56 10.78l7.97-6.19z" }
            path { fill: "#34A853", d: "M24 48c6.48 0 11.93-2.13 15.89-5.81l-7.73-6c-2.15 1.45-4.92 2.3-8.16 2.3-6.26 0-11.57-4.22-13.47-9.91l-7.98 6.19C6.51 42.62 14.62 48 24 48z" }
        }
    }
}
