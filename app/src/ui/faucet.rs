//! The wallet and its test money: Account's "Your wallet" and "Faucet" cards (31 Oct, user
//! request: the only two), and the wallet line with the faucet button that Confirm shows
//! (`WalletLine`, ids kept for the devnet e2e).
//!
//! The faucet on devnet (V-8): mock USDC from the program's faucet, and mockBONK from the AMM's
//! faucet when the desk lists it (no mockSOL on Account). The faucet
//! runs only on devnet or localnet ([`faucet_allowed`]); the RPC is never mainnet either way
//! (D-046: `config::cluster` refuses a mainnet URL and `Wallet` refuses a mainnet genesis hash
//! before any send). Hidden or disabled in demo mode, which has no wallet.

use dioxus::prelude::*;
use dioxus_solana::Cluster;
use schedina_tx::Pubkey;
use schedina_tx::swap::mock_faucet_ixs;

use super::kit;
use super::nav::{self, Balance};
use super::{use_app_state, use_desk};
use crate::Route;
use crate::config;
use crate::desk::types::TokenInfo;
use crate::flows;
use crate::format;
use crate::wallet::{Wallet, use_wallet_api};

/// True only on devnet or localnet, by chain and by URL: the faucet is never offered or sent
/// anywhere else.
pub fn faucet_allowed(cluster: &Cluster) -> bool {
    matches!(cluster.chain_id(), "solana:devnet" | "solana:localnet")
        && !config::is_mainnet_url(cluster.rpc_url())
}

/// The network's name for the wallet card.
fn network(cluster: &Cluster) -> &'static str {
    match cluster.chain_id() {
        "solana:devnet" => "Devnet",
        "solana:localnet" => "Localnet",
        _ => "Unsupported network",
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Status {
    Idle,
    Busy,
    Done,
    Failed(String),
}

/// The slip's and Your bets on Account's wallet line: address, test-USDC balance, "Get test USDC" (`#faucet`).
#[component]
pub fn WalletLine() -> Element {
    let desk = use_desk();
    let wallet = use_wallet_api();
    let mut status = use_signal(|| Status::Idle);
    let demo = desk.is_demo();
    // What moves the balance besides the faucet: a schedina placed, cashed out, settled or
    // refunded. The memo changes only when one of those does (not on every stream frame), so
    // the balance is re-read then and no more often.
    let state = use_app_state();
    let ledger = use_memo(move || {
        state
            .read()
            .my_tickets()
            .iter()
            .map(|t| (t.ticket.clone(), t.state, t.paid_u))
            .collect::<Vec<_>>()
    });
    let mut balance = use_resource(move || async move {
        let _ = ledger();
        // Re-read when the wallet changes; no RPC call in demo mode.
        match wallet.public_key() {
            Some(_) if !demo => Some(flows::usdc_balance(wallet).await),
            _ => None,
        }
    });

    if demo {
        return rsx! {};
    }
    let pk = wallet.public_key();
    let busy = status() == Status::Busy;
    let allowed = faucet_allowed(&wallet.cluster());
    rsx! {
        div { class: "wallet-line",
            match pk {
                None => rsx! {
                    button {
                        class: "button ghost",
                        onclick: move |_| async move {
                            if let Err(e) = wallet.connect().await {
                                status.set(Status::Failed(flows::explain(&e)));
                            }
                        },
                        "Connect wallet"
                    }
                },
                Some(pk) => rsx! {
                    span { class: "muted small", "{format::short(&pk.to_string())}" }
                    span { class: "balance",
                        match balance() {
                            Some(Some(Ok(Some(u)))) => format!("{} test USDC", format::eur(u)),
                            Some(Some(Ok(None))) => "no test USDC yet".to_string(),
                            Some(Some(Err(_))) => "balance unavailable".to_string(),
                            _ => "…".to_string(),
                        }
                    }
                    if allowed {
                        button {
                            class: "button ghost small",
                            id: "faucet",
                            disabled: busy,
                            // Leaving the screen only stops waiting for the confirmation.
                            onclick: move |_| async move {
                                status.set(Status::Busy);
                                status.set(match flows::faucet(wallet).await {
                                    Ok(_) => Status::Done,
                                    Err(e) => Status::Failed(e),
                                });
                                balance.restart();
                                nav::balance_changed();
                            },
                            if busy { "Asking the faucet…" } else { "Get test USDC" }
                        }
                    }
                },
            }
        }
        match status() {
            Status::Failed(e) => rsx! { p { class: "error small", "{e}" } },
            Status::Done => rsx! { p { class: "ok small", "Test USDC received." } },
            _ => rsx! {},
        }
    }
}

/// Bumped when a faucet claim lands, so the token balances on Account read the chain again
/// (the app bar's USDC follows `nav::balance_changed`).
pub(crate) static TOKENS_EPOCH: GlobalSignal<u32> = Signal::global(|| 0);

/// Account's "Your wallet" (V-7; 31 Oct: only what it is worth). The address with Solscan (V-9)
/// and the network, the wallet's worth in € (test USDC plus each mock token at what it would
/// swap for, as the app bar's pill), the connect button and, in dev builds, the in-tab test
/// wallet (D-150). `#account-balance` carries the USDC balance in `data-usdc-u` for the e2e.
#[component]
pub fn WalletCard() -> Element {
    let desk = use_desk();
    let state = use_app_state();
    let wallet = use_wallet_api();
    let fx = kit::money::use_fx();
    let balance = nav::use_balance();
    let demo = desk.is_demo();
    let cluster = wallet.cluster();
    let pk = wallet.public_key().map(|p| p.to_string());
    // The wallet shown: the connected one, or the demo's example owner.
    let shown = pk
        .clone()
        .or_else(|| state.read().user.clone().filter(|_| demo));
    let usdc = match &balance {
        Balance::Ready(u) => Some(*u),
        _ => None,
    };
    let worth = nav::use_portfolio_usdc(usdc);
    let hint = match &balance {
        Balance::Ready(_) => None,
        Balance::Demo => Some("Demo build: no wallet, no balance."),
        Balance::Disconnected => Some("No wallet connected."),
        Balance::Loading => Some("Reading the balance…"),
        Balance::Unknown => Some("Balance unavailable right now."),
    };
    let initials = shown
        .as_ref()
        .map_or_else(|| "?".to_string(), |w| w.chars().take(2).collect());
    let net = if demo && !config::DESIGN_FIXTURES {
        "Demo"
    } else {
        network(&cluster)
    };
    rsx! {
        section { id: "account-wallet", aria_labelledby: "account-wallet-title", class: "acct-card acct-wallet",
            span { class: "acct-glow a", "aria-hidden": "true" }
            span { class: "acct-glow b", "aria-hidden": "true" }
            div { class: "acct-who",
                kit::Avatar { initials, size: 48, tone: kit::AvatarTone::Gradient, mono: true }
                div { class: "acct-who-main",
                    h2 { class: "acct-name", id: "account-wallet-title", "Your wallet" }
                    match shown.clone() {
                        Some(w) => rsx! {
                            span { class: "acct-addr num", id: if pk.is_some() { "account-address" } else { "account-example" }, title: "{w}", "{format::short(&w)}" }
                        },
                        None => rsx! { span { class: "acct-addr", "Connect to see your address" } },
                    }
                }
                span { class: "acct-net", "{net}" }
            }
            div { class: "acct-balance", id: "account-balance", "data-usdc-u": usdc.map(|u| u.to_string()),
                span { class: "acct-label", "Worth" }
                match worth {
                    Some(u) => rsx! { span { class: "acct-balance-eur", "{kit::money::pair(u, fx).0}" } },
                    None => rsx! { span { class: "acct-balance-eur muted", "—" } },
                }
            }
            if let Some(h) = hint {
                p { class: "acct-sub", "{h}" }
            }
            div { class: "acct-wallet-row",
                if let Some(w) = shown.clone() {
                    kit::SolscanLink { account: w, id: "account-solscan" }
                }
                if !demo {
                    super::login::LoginButton { manage: true }
                }
            }
            if config::dev_tools() {
                kit::TextLink {
                    label: "Use a test wallet in this tab",
                    to: Route::DevWallet {},
                    accent: true,
                    id: "account-burner",
                }
            }
        }
    }
}

/// Test tokens per faucet tap, in whole tokens (the program caps each claim; as the slip's).
fn faucet_tokens(symbol: &str) -> u64 {
    if symbol.contains("SOL") { 1 } else { 50_000 }
}

/// The faucet's mock token for a coin of the pill: mockBONK for BONK (mockSOL is not offered
/// here, 31 Oct).
fn faucet_token(tokens: &[TokenInfo], coin: kit::Currency) -> Option<TokenInfo> {
    match coin {
        kit::Currency::Bonk => tokens
            .iter()
            .find(|t| !t.usdc_mint && t.symbol.to_ascii_uppercase().contains("BONK"))
            .cloned(),
        _ => None,
    }
}

/// An airdrop that landed: what came in and its transaction (the celebration's).
#[derive(Debug, Clone, PartialEq)]
struct Airdrop {
    amount: String,
    sig: String,
}

/// Account's "Faucet" (V-8; 31 Oct, user request): the coin pill of the bet drawer (USDC, and
/// BONK when the desk lists mockBONK; no SOL), the wallet's balance of that coin, one button
/// for its devnet airdrop, then a celebration with the transaction on the explorer. Disabled in
/// demo mode and off devnet/localnet.
#[component]
pub fn FaucetCard(tokens: Vec<TokenInfo>) -> Element {
    let desk = use_desk();
    let wallet = use_wallet_api();
    let demo = desk.is_demo();
    let allowed = faucet_allowed(&wallet.cluster());
    let balance = nav::use_balance();
    let mut coin = use_signal(|| kit::Currency::Usdc);
    let mut status = use_signal(|| Status::Idle);
    let mut note = use_signal(String::new);
    let mut done = use_signal(|| None::<Airdrop>);
    let bonk = faucet_token(&tokens, kit::Currency::Bonk);
    let coins: Vec<kit::Currency> = std::iter::once(kit::Currency::Usdc)
        .chain(bonk.is_some().then_some(kit::Currency::Bonk))
        .collect();
    let token = faucet_token(&tokens, coin());
    let mint = token.as_ref().map(|t| (t.mint.clone(), t.decimals));
    let held = use_resource(use_reactive!(|mint| async move {
        let _ = TOKENS_EPOCH();
        let user = wallet.public_key().filter(|_| !demo)?;
        let (mint, _) = mint?;
        let mint = mint.parse::<Pubkey>().ok()?;
        wallet
            .token_balance(&schedina_tx::pda::ata(&user, &mint))
            .await
            .ok()
            .map(|b| b.unwrap_or(0))
    }));
    let shown_balance = match &token {
        Some(t) => (*held.read())
            .flatten()
            .map(|b| format!("{} {}", format::token(b, t.decimals), t.symbol)),
        None => match &balance {
            Balance::Ready(u) => Some(kit::money::usdc_text(*u)),
            _ => None,
        },
    };
    let amount = match &token {
        Some(t) => {
            let unit = 10u64.pow(u32::from(t.decimals.min(19)));
            format!(
                "{} {}",
                format::token(faucet_tokens(&t.symbol).saturating_mul(unit), t.decimals),
                t.symbol
            )
        }
        None => kit::money::usdc_text(schedina_tx::FAUCET_AMOUNT_U),
    };
    let busy = status() == Status::Busy;
    let off = demo || !allowed;
    let lead = if demo {
        "Demo build: no wallet, nothing is sent."
    } else if !allowed {
        "The faucet works on devnet only."
    } else {
        "Free test tokens on devnet. They have no value."
    };
    let get = {
        let amount = amount.clone();
        move |_| {
            if !faucet_allowed(&wallet.cluster()) {
                return;
            }
            let token = token.clone();
            let amount = amount.clone();
            status.set(Status::Busy);
            note.set(format!("Airdropping {amount}…"));
            spawn(async move {
                let r = match &token {
                    Some(t) => token_faucet(wallet, t).await,
                    None => flows::faucet(wallet).await,
                };
                match r {
                    Ok(sig) => {
                        finish(status, note, Ok(String::new()));
                        done.set(Some(Airdrop { amount, sig }));
                    }
                    Err(e) => finish(status, note, Err(e)),
                }
            });
        }
    };
    rsx! {
        section { id: "account-faucet", aria_labelledby: "account-faucet-title", class: "acct-card acct-faucet",
            kit::SectionTitle { title: "Faucet", small: true, id: "account-faucet-title", end: rsx! { span { class: "acct-test-chip", "Test tokens" } } }
            p { class: "acct-sub", "{lead}" }
            div { class: "acct-faucet-row",
                kit::CoinPill {
                    currency: coin(),
                    currencies: coins,
                    id: "faucet-coin",
                    label: "Coin",
                    disabled: busy,
                    onchange: move |c| {
                        coin.set(c);
                        note.set(String::new());
                        status.set(Status::Idle);
                    },
                }
                div { class: "acct-faucet-balance", id: "faucet-balance",
                    span { class: "acct-label", "Balance" }
                    match shown_balance {
                        Some(b) => rsx! { span { class: "acct-faucet-amount num", "{b}" } },
                        None => rsx! { span { class: "acct-faucet-amount muted", "—" } },
                    }
                }
            }
            kit::PurpleCTA {
                id: "faucet-get",
                label: format!("Get {amount}"),
                icon: kit::Sym::Add,
                disabled: off || busy,
                busy: busy.then(|| "Airdropping…".to_string()),
                onclick: get,
            }
            if !note().is_empty() {
                p {
                    class: match status() {
                        Status::Failed(_) => "acct-status error",
                        _ => "acct-status",
                    },
                    id: "faucet-status",
                    role: "status",
                    "{note}"
                }
            }
        }
        if let Some(a) = done() {
            AirdropCelebration { airdrop: a, onclose: move |_| done.set(None) }
        }
    }
}

/// The airdrop's celebration (31 Oct): over the whole screen with a back header, the shared
/// `Celebration` frame (its confetti and count-up) with what came in, View Transaction on the
/// explorer, and no Brag button.
#[component]
fn AirdropCelebration(airdrop: Airdrop, onclose: EventHandler<()>) -> Element {
    rsx! {
        div { class: "cx", id: "faucet-celebration", role: "dialog", aria_modal: "true", aria_label: "Airdrop received",
            div { class: "cx-col",
                div { class: "k-subheader cx-head",
                    div { class: "k-subheader-start",
                        button { r#type: "button", class: "k-icon-btn lg cx-back", id: "faucet-close", aria_label: "Back to Account", title: "Back to Account",
                            onclick: move |_| onclose.call(()),
                            kit::SymIcon { sym: kit::Sym::ArrowBack, size: 20 }
                        }
                        span { class: "k-subheader-title", "Faucet" }
                    }
                }
                div { class: "cx-body",
                    super::celebrate::Celebration {
                        id: "faucet-frame",
                        icon: kit::HeroIcon::Check,
                        title: "Airdrop received!",
                        emoji: "🪂",
                        amount_label: "In your wallet",
                        amount: airdrop.amount.clone(),
                        green: false,
                        tx: Some(airdrop.sig.clone()).filter(|s| !s.is_empty()),
                        brag: String::new(),
                    }
                }
            }
        }
    }
}

fn finish(mut status: Signal<Status>, mut note: Signal<String>, r: Result<String, String>) {
    match r {
        Ok(text) => {
            status.set(Status::Done);
            note.set(text);
            nav::balance_changed();
            *TOKENS_EPOCH.write() += 1;
        }
        Err(e) => {
            note.set(e.clone());
            status.set(Status::Failed(e));
        }
    }
}

/// A mock token from the AMM's faucet into the wallet's associated account; its signature.
pub(crate) async fn token_faucet(wallet: Wallet, t: &TokenInfo) -> Result<String, String> {
    let user = match wallet.public_key() {
        Some(pk) => pk,
        None => wallet.connect().await.map_err(|e| flows::explain(&e))?,
    };
    let mint: Pubkey = t.mint.parse().map_err(|_| "bad mint address".to_string())?;
    let unit = 10u64.pow(u32::from(t.decimals.min(19)));
    let amount = faucet_tokens(&t.symbol).saturating_mul(unit);
    // Sponsored (L8-2), the sponsor pays the account's and the claim's rent.
    let sig = wallet
        .send_instructions(move |payer| Ok(mock_faucet_ixs(&user, payer, &mint, amount).to_vec()))
        .await
        .map_err(|e| flows::explain(&e))?;
    wallet.confirm(&sig).await.map_err(|e| flows::explain(&e))?;
    Ok(sig.to_string())
}
