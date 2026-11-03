//! `/confirm` (7.1b): where a placed schedina lands. Minted, it is the placement celebration
//! (`ui/celebrate.rs`, "Bet Success" in the back header). Before that (a copy started on the
//! copy page comes here at once; the drawer keeps its own bets until they are minted) it shows
//! where the bet is: signed, escrowed, hedging, minted, or why it was not placed. With nothing
//! placed it is an alias: the sportsbook with the bet drawer open (a design build shows the
//! mockup's ticket instead, for the visual review).

use dioxus::prelude::*;

use super::celebrate::{DesignCelebration, PlacedCelebration};
use super::kit::{PurpleCTA, SecondaryButton, SeqStep, Sequencer, StepState};
use super::use_app_state;
use crate::Route;
use crate::config;
use crate::desk::types::Stage;
use crate::format;
use crate::state::{PlayKind, Submission, SubmitStep};
use crate::wallet::use_wallet_api;

/// The placement's steps as the sequencer shows them (1 Nov): a fresh price, the wallet, Solana,
/// the escrow, the hedge, the mint; demo builds skip the first three (no wallet, no chain). A
/// failure marks the step it happened at; a refund marks the hedge.
pub fn place_steps(
    sub: &Submission,
    stage: Option<Stage>,
    mock: bool,
    sponsored: bool,
) -> Vec<SeqStep> {
    use StepState::{Current, Done, Failed, Todo};
    let failed = matches!(sub.step, SubmitStep::Failed(_));
    let refunded = stage == Some(Stage::Refunded);
    // How far it got: 0 price, 1 wallet, 2 Solana, 3 escrow, 4 hedge, 5 minted.
    let reached = match (stage, &sub.step) {
        (Some(Stage::Minted), _) => 6,
        (Some(Stage::Hedging), _) => 4,
        (Some(Stage::Refunded), _) => 4,
        (Some(Stage::Escrowed | Stage::Other), _) | (None, SubmitStep::Landed) => 4,
        (None, SubmitStep::Confirming) => 2,
        (None, SubmitStep::Signing) => 1,
        (None, SubmitStep::Quoting) => 0,
        // A failure before anything landed: where it stopped is the last step under way.
        (None, SubmitStep::Failed(_)) => {
            if sub.sig.is_some() {
                2
            } else if sub.pending.is_some() {
                1
            } else {
                0
            }
        }
    };
    let state = |i: usize| -> StepState {
        if i < reached {
            Done
        } else if i == reached && (failed || (refunded && i == 4)) {
            Failed
        } else if i == reached && !failed {
            Current
        } else {
            Todo
        }
    };
    // Under way in the present ("Hedging…"), done in the past ("Hedged").
    let word = |i: usize, doing: &str, done: &str| -> String {
        if state(i) == Done {
            done.to_string()
        } else {
            format!("{doing}\u{2026}")
        }
    };
    let mut steps = Vec::new();
    if !sub.demo {
        steps.push(SeqStep::new(
            "price",
            word(0, "Getting a fresh price", "Price locked"),
            state(0),
        ));
        let wallet = SeqStep::new(
            "wallet",
            word(1, "Approve in your wallet", "Signed in your wallet"),
            state(1),
        );
        steps.push(if sponsored {
            wallet.note("BetFi pays the network fee")
        } else {
            wallet
        });
        steps.push(SeqStep::new(
            "solana",
            word(2, "Sending to Solana", "Confirmed on Solana"),
            state(2),
        ));
    }
    steps.push(SeqStep::new(
        "escrowed",
        word(3, "Escrowing your stake", "Stake escrowed"),
        state(3),
    ));
    let hedge = if refunded {
        SeqStep::new("hedging", "Not filled: stake refunded", Failed)
    } else {
        if mock {
            SeqStep::new(
                "hedging",
                word(4, "Hedging (mock fills)", "Hedged (mock fills)"),
                state(4),
            )
        } else {
            SeqStep::new(
                "hedging",
                word(4, "Hedging on Pascal", "Hedged on Pascal"),
                state(4),
            )
        }
    };
    steps.push(hedge);
    let minted = if refunded { Todo } else { state(5) };
    steps.push(SeqStep::new(
        "minted",
        if minted == Done {
            "Bet minted"
        } else {
            "Minting your bet\u{2026}"
        },
        minted,
    ));
    steps
}

#[component]
pub fn Confirm() -> Element {
    let state = use_app_state();
    let wallet = use_wallet_api();
    let (submission, progress) = {
        let s = state.read();
        let sub = s.submission.clone();
        let progress = sub
            .as_ref()
            .and_then(|x| x.pending.as_ref())
            .and_then(|p| s.progress.get(p))
            .cloned();
        (sub, progress)
    };
    let alias = submission.is_none() && !config::DESIGN_FIXTURES;
    use_effect(use_reactive!(|alias| {
        if alias {
            super::tray::open_drawer();
            navigator().replace(Route::Card {});
        }
    }));
    let Some(sub) = submission else {
        return if config::DESIGN_FIXTURES {
            rsx! { DesignCelebration {} }
        } else {
            rsx! {}
        };
    };
    let stage = progress.as_ref().map(|p| p.stage);
    if stage == Some(Stage::Minted) {
        return rsx! {
            PlacedCelebration { sub, ticket_pda: progress.and_then(|p| p.ticket) }
        };
    }

    let failed = matches!(sub.step, SubmitStep::Failed(_));
    let refunded = stage == Some(Stage::Refunded);
    let mock = state.read().mock_fills();
    let sponsored = wallet.sponsored();
    let title = match (failed, refunded) {
        (true, _) => "Not placed",
        (_, true) => "Refunded",
        _ => "Placing your bet",
    };
    let back = match &sub.kind {
        PlayKind::Copy { original, .. } => rsx! {
            SecondaryButton { label: "Back to the tip", to: Route::CopyTip { id: original.clone() } }
        },
        _ => rsx! {
            PurpleCTA {
                label: "Back to your slip",
                onclick: move |_| {
                    super::tray::open_drawer();
                    navigator().push(Route::Card {});
                },
            }
        },
    };
    rsx! {
        div { class: "cel cel-progress", id: "confirm-progress",
            div { class: "cel-glow", "aria-hidden": "true", span {} span {} span {} }
            div { class: "cel-stream",
                // The signer and its test USDC, with the faucet (a failure for want of test money
                // says "Get test USDC").
                if !sub.demo {
                    super::faucet::WalletLine {}
                    super::login::LoginButton {}
                }
                Sequencer { title: title.to_string(), steps: place_steps(&sub, stage, mock, sponsored) }
                match &sub.kind {
                    PlayKind::Plain => rsx! {},
                    PlayKind::Tip { title: None } => rsx! {
                        p { class: "cel-progress-kind", id: "confirm-kind", "Shared as a tip" }
                    },
                    PlayKind::Tip { title: Some(name) } => rsx! {
                        p { class: "cel-progress-kind", id: "confirm-kind", "Shared as a tip: \u{201c}{name}\u{201d}" }
                    },
                    PlayKind::Copy { original, .. } => rsx! {
                        p { class: "cel-progress-kind", id: "confirm-kind",
                            "A copy of "
                            Link { to: Route::TicketScreen { id: original.clone() }, "bet {format::short(original)}" }
                        }
                    },
                }
                match sub.step.clone() {
                    SubmitStep::Quoting => rsx! { p { class: "cel-progress-line", role: "status", "Getting a fresh price\u{2026}" } },
                    SubmitStep::Signing => rsx! {
                        p { class: "cel-progress-line", role: "status",
                            "Approve in your wallet: the price holds for about 10 seconds."
                            // L8-2: the desk's sponsor pays; the wallet needs no SOL.
                            if sponsored { " BetFi pays the network fee." }
                        }
                    },
                    SubmitStep::Confirming => rsx! { p { class: "cel-progress-line", role: "status", "Sent, waiting for the network\u{2026}" } },
                    SubmitStep::Failed(e) => rsx! {
                        p { class: "slip-error", id: "submit-error", role: "alert", "{e}" }
                        {back}
                    },
                    SubmitStep::Landed if refunded => rsx! {
                        p { class: "cel-progress-line", "The hedge did not fill, so your stake was refunded in full." }
                        {back}
                    },
                    SubmitStep::Landed => rsx! {
                        p { class: "cel-progress-line", role: "status", "Your stake is escrowed; the bet is minted once the hedge fills." }
                    },
                }
                if let Some(sig) = sub.sig.clone() {
                    a {
                        class: "cel-progress-line",
                        href: "{config::solscan_tx(&sig)}",
                        target: "_blank",
                        rel: "noopener noreferrer",
                        "Transaction {format::short(&sig)} \u{2197}"
                    }
                }
            }
        }
    }
}
