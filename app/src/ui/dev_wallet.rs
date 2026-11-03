//! Dev-only page for the wallet spike (plan 1.F): connect, airdrop, and sign a devnet memo
//! transaction through `Wallet::sign_and_send`, the same path 2.E's `open_pending` takes; and the
//! in-tab test wallet (D-150) that headless and phone runs drive. 6.1f: restyled in the design
//! system; the ids the devnet e2e selects on are kept (`#burner`, `#wallet-address`,
//! `#sol-balance`, `#sol-refresh`, `#test-signature`, `#test-result`).

use dioxus::prelude::*;
use dioxus_solana::{WalletButton, WalletState};

use super::kit::{Button, ButtonVariant, Panel};
use super::{use_app_state, use_desk};
use crate::burner::{BURNER_NAME, Burner};
use crate::config;
use crate::wallet::{Wallet, memo_transaction, use_wallet_api};

#[derive(Debug, Clone, PartialEq)]
enum Status {
    Idle,
    Busy(String),
    Done { text: String, sig: Option<String> },
    Failed(String),
}

#[component]
pub fn DevWallet() -> Element {
    if !config::dev_tools() {
        return rsx! {
            div { class: "dw",
                h1 { "Not available" }
            }
        };
    }
    let wallet = use_wallet_api();
    let state = use_app_state();
    let desk = use_desk();
    // mockSOL for paying in SOL (31 Oct: Account's faucet offers USDC and BONK only).
    let mocksol = use_resource(move || {
        let desk = desk.clone();
        async move {
            desk.tokens()
                .await
                .unwrap_or_default()
                .into_iter()
                .find(|t| !t.usdc_mint && t.symbol.contains("SOL"))
        }
    });
    let mocksol = mocksol.read().clone().flatten();
    let mut status = use_signal(|| Status::Idle);
    let cluster = wallet.cluster();
    let chain = cluster.chain_id().to_string();
    let rpc_url = cluster.rpc_url().to_string();
    let pk = wallet.public_key();
    let wallet_state = match wallet.state() {
        WalletState::Disconnected => "Disconnected".to_string(),
        WalletState::Connecting => "Connecting".to_string(),
        WalletState::Connected(acc) => format!("Connected ({})", acc.wallet_name),
        WalletState::Error(e) => format!("Error: {e}"),
    };
    let busy = matches!(status(), Status::Busy(_));
    let mut sol = use_resource(move || async move {
        // Re-read when the wallet changes.
        wallet.public_key()?;
        Some(wallet.sol_balance().await.map_err(|e| e.to_string()))
    });
    let sol_now = sol.read().clone().flatten();
    let (link, bad, unknown) = {
        let s = state.read();
        (format!("{:?}", s.link), s.bad_frames, s.unknown_frames)
    };

    rsx! {
        div { class: "dw",
            div { class: "dw-head",
                h1 { "Wallet check" }
                p { class: "dw-sub", "Dev only. Chain " span { class: "num", "{chain}" } ", RPC " span { class: "num", "{rpc_url}" } "." }
            }
            Panel { aria_label: "Wallet", class: "dw-card",
                div { class: "dw-card-head",
                    h2 { "Wallet" }
                    span { class: "dw-state", "{wallet_state}" }
                }
                WalletButton { class: "wallet inline" }
                if let Some(pk) = pk {
                    p { class: "dw-addr num", id: "wallet-address", "{pk}" }
                    div { class: "dw-sol",
                        span { class: "dw-sol-label", "SOL" }
                        span { class: "num", id: "sol-balance",
                            match sol_now {
                                Some(Ok(l)) => format!("{}.{:09}", l / 1_000_000_000, l % 1_000_000_000),
                                Some(Err(e)) => format!("unavailable ({e})"),
                                None => "…".to_string(),
                            }
                        }
                        Button {
                            id: "sol-refresh",
                            variant: ButtonVariant::Ghost,
                            onclick: move |_| sol.restart(),
                            "Refresh"
                        }
                    }
                }
                // A fresh in-page keypair (crate::burner): the secret stays in this tab's memory.
                Button {
                    id: "burner",
                    variant: ButtonVariant::Secondary,
                    wide: true,
                    disabled: busy,
                    onclick: move |_| {
                        status.set(match Burner::generate() {
                            Ok(b) => {
                                let pk = b.address();
                                wallet.connect_signer(BURNER_NAME, b);
                                Status::Done { text: format!("Fresh test wallet {pk}: send it a little devnet SOL"), sig: None }
                            }
                            Err(e) => Status::Failed(e),
                        });
                    },
                    "Create a fresh test wallet (in this tab only)"
                }
                div { class: "dw-row",
                    Button {
                        variant: ButtonVariant::Secondary,
                        small: true,
                        disabled: busy || pk.is_some(),
                        onclick: move |_| {
                            spawn(async move {
                                status.set(Status::Busy("Connecting…".into()));
                                status.set(match wallet.connect().await {
                                    Ok(pk) => Status::Done { text: format!("Connected {pk}"), sig: None },
                                    Err(e) => Status::Failed(e.to_string()),
                                });
                            });
                        },
                        "Connect"
                    }
                    Button {
                        variant: ButtonVariant::Secondary,
                        small: true,
                        disabled: busy || pk.is_none(),
                        onclick: move |_| {
                            wallet.disconnect();
                            status.set(Status::Idle);
                        },
                        "Disconnect"
                    }
                    Button {
                        variant: ButtonVariant::Secondary,
                        small: true,
                        disabled: busy || pk.is_none(),
                        onclick: move |_| {
                            spawn(async move {
                                status.set(Status::Busy("Asking the faucet for 1 SOL…".into()));
                                status.set(match wallet.airdrop(1_000_000_000).await {
                                    Ok(sig) => Status::Done { text: "Airdrop confirmed".into(), sig: Some(sig) },
                                    Err(e) => Status::Failed(format!("{e} (or use faucet.solana.com)")),
                                });
                            });
                        },
                        "Airdrop 1 SOL"
                    }
                }
                if let Some(t) = mocksol {
                    Button {
                        id: "dev-mocksol",
                        variant: ButtonVariant::Secondary,
                        wide: true,
                        disabled: busy || pk.is_none(),
                        onclick: move |_| {
                            let t = t.clone();
                            spawn(async move {
                                status.set(Status::Busy(format!("Asking the faucet for {}…", t.symbol)));
                                status.set(match super::faucet::token_faucet(wallet, &t).await {
                                    Ok(sig) => {
                                        *super::faucet::TOKENS_EPOCH.write() += 1;
                                        super::nav::balance_changed();
                                        Status::Done { text: format!("Test {} received.", t.symbol), sig: Some(sig) }
                                    }
                                    Err(e) => Status::Failed(e),
                                });
                            });
                        },
                        "Get 1 test mockSOL (pays a stake in SOL)"
                    }
                }
                Button {
                    id: "test-signature",
                    wide: true,
                    disabled: busy || pk.is_none(),
                    onclick: move |_| {
                        spawn(async move {
                            status.set(Status::Busy("Approve the memo in your wallet…".into()));
                            status.set(match test_signature(wallet, status).await {
                                Ok(sig) => Status::Done { text: "Memo confirmed on chain".into(), sig: Some(sig) },
                                Err(e) => Status::Failed(e),
                            });
                        });
                    },
                    "Test signature (devnet memo)"
                }
                match status() {
                    Status::Idle => rsx! {},
                    Status::Busy(t) => rsx! { p { class: "dw-sub", role: "status", "{t}" } },
                    Status::Failed(e) => rsx! { p { class: "dw-result error", id: "test-result", role: "alert", "{e}" } },
                    Status::Done { text, sig } => {
                        let link = sig.as_ref().map(|s| config::solscan_tx(s));
                        rsx! {
                            p { class: "dw-result ok", id: "test-result", role: "status", "{text}" }
                            if let (Some(sig), Some(link)) = (sig, link) {
                                p { class: "dw-addr num", "{sig}" }
                                a { class: "k-link accent", href: "{link}", target: "_blank", rel: "noopener", "Open in Solana Explorer" }
                            }
                        }
                    }
                }
            }
            Panel { aria_label: "Stream", class: "dw-card",
                h2 { "Stream" }
                p { class: "dw-sub",
                    "Link {link}; undecodable frames "
                    span { class: "num", "{bad}" }
                    "; unknown frame types "
                    span { class: "num", "{unknown}" }
                    "."
                }
            }
        }
    }
}

/// Build a memo transaction, sign and send it through `sign_and_send`, wait for confirmation.
async fn test_signature(wallet: Wallet, mut status: Signal<Status>) -> Result<String, String> {
    let payer = wallet.public_key().ok_or("Connect a wallet first")?;
    let blockhash = wallet.latest_blockhash().await.map_err(|e| e.to_string())?;
    let text = format!("schedina 1.F wallet spike {}", js_sys::Date::now() as u64);
    let bytes = memo_transaction(payer, blockhash, &text).map_err(|e| e.to_string())?;
    let sig = wallet
        .sign_and_send(bytes)
        .await
        .map_err(|e| e.to_string())?;
    status.set(Status::Busy(format!("Sent {sig}, confirming…")));
    wallet
        .confirm(&sig)
        .await
        .map_err(|e| format!("{e} (signature {sig})"))?;
    Ok(sig.to_string())
}
