//! Pay in another token (spec section 8, plan 4.D, contract L4-3), in the bet drawer (7.1b).
//! Mock USDC is the default (the stake in euros, the pill's EUR or USDC). Paying in mockSOL (the
//! pill's SOL, D-436) turns the euros chosen into a mockSOL amount at `/fx`'s SOL price; another
//! token `GET /tokens` lists (mockBONK, BONK on the coin pill) takes a typed amount; the same
//! pill switches back. Either way the desk's `POST /swap-quote` turns the
//! amount into USDC, its fee is a line of its own (spec 8), and the desk is quoted with
//! `stake_u = min_out_u`. The one "Bet" then signs one transaction,
//! `[swap, Ed25519 verify, open_pending]` (v0, with the AMM's lookup table): a swap paying less
//! than `min_out_u` fails it whole, so nothing is pending. While the desk sponsors transactions
//! (L8-2) its sponsor pays the fee and the rent; the user only signs. The chip is not shown
//! when `/tokens` lists only mock USDC (no AMM on the cluster).
//!
//! The client computes no price: the swap amounts come from the desk's swap quote, the total
//! from its quote. Transaction bytes come from `schedina_tx::swap` (host-tested).

use dioxus::prelude::*;
use dioxus_solana::rpc::RpcContext;
use schedina_tx::swap::{
    ShownSwap, copy_stake_for, mock_faucet_ixs, swap_error_words, swap_open_pending_ixs_with,
};
use schedina_tx::{
    OpenOptions, Pubkey, ShownLeg, ShownQuote, SignedMsg, copy_matches, create_ata_idempotent_ix,
    custom_error_code, pda,
};

use super::use_app_state;
use crate::desk::Desk;
use crate::desk::types::{Quote, QuoteRequest, Side, SwapQuote, TokenInfo};
use crate::flows;
use crate::format;
use crate::state::{AppState, PlayKind, Progress, Submission, SubmitStep};
use crate::wallet::{Compile, Wallet, amm_lookup_table, use_wallet_api};

/// What the drawer's swap fee line shows: the amount paid, and the desk's swap quote.
#[derive(Debug, Clone, PartialEq)]
pub struct SwapLine {
    pub token: TokenInfo,
    pub amount_in: u64,
    pub quote: SwapQuote,
}

/// Test tokens per faucet tap, in whole tokens (the program caps each claim).
fn faucet_tokens(symbol: &str) -> u64 {
    if symbol.contains("SOL") { 1 } else { 50_000 }
}

/// Under the stake row while paying in `token`: the wallet's balance of it and its faucet
/// (live desk only; demo mode has no wallet).
#[component]
pub fn TokenLine(token: TokenInfo, demo: bool) -> Element {
    let state = use_app_state();
    let wallet = use_wallet_api();
    let mut note = use_signal(|| None::<String>);
    let mint = token.mint.clone();
    let mut balance = use_resource(move || {
        let mint = mint.clone();
        let user = wallet.public_key();
        async move {
            let user = user?;
            let mint: Pubkey = mint.parse().ok()?;
            wallet
                .token_balance(&pda::ata(&user, &mint))
                .await
                .ok()
                .map(|b| b.unwrap_or(0))
        }
    });
    let busy = state.read().submission.as_ref().is_some_and(|s| s.busy());
    let t = token.clone();
    if demo {
        return rsx! {
            p { class: "slip-small", id: "token-line",
                "Demo: the desk's example swap, whatever amount of {t.symbol} you enter."
            }
        };
    }
    rsx! {
        p { class: "slip-small slip-token-line", id: "token-line",
            match &*balance.read() {
                Some(Some(b)) => rsx! { span { "You hold {format::token(*b, t.decimals)} {t.symbol}." } },
                _ => rsx! {},
            }
            button {
                class: "k-link accent slip-inline-link",
                id: "token-faucet",
                disabled: busy,
                onclick: move |_| {
                    let t = t.clone();
                    spawn(async move {
                        note.set(Some(format!("Asking the faucet for {}\u{2026}", t.symbol)));
                        let r = faucet(wallet, &t).await;
                        note.set(Some(match r {
                            Ok(_) => format!("Test {} received.", t.symbol),
                            Err(e) => e,
                        }));
                        balance.restart();
                    });
                },
                "Get test {token.symbol}"
            }
        }
        if let Some(n) = note() {
            p { class: "slip-small", id: "token-note", "{n}" }
        }
    }
}

async fn faucet(wallet: Wallet, t: &TokenInfo) -> Result<(), String> {
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
        .map_err(|e| explain(&e, 1))?;
    wallet.confirm(&sig).await.map_err(|e| explain(&e, 1))
}

/// Program errors of the AMM instruction at `amm_ix` in its own words, else the app's.
fn explain(e: &crate::wallet::WalletError, amm_ix: usize) -> String {
    let text = e.to_string();
    if text.contains(&format!("Instruction {amm_ix}:"))
        && let Some(words) = custom_error_code(&text).and_then(swap_error_words)
    {
        return format!("Not placed: {words}.");
    }
    flows::explain(e)
}

fn update_own(mut state: Signal<AppState>, id: u64, f: impl FnOnce(&mut Submission)) {
    if let Some(s) = state.write().submission.as_mut().filter(|s| s.id == id) {
        f(s);
    }
}

/// "Bet" while paying in another token (live desk): `shown_swap` and `shown` are what the
/// drawer showed; `req` is the slip's picks at `stake_u = shown_swap.min_out_u`, or a copy's legs
/// (`kind` [`PlayKind::Copy`], `req.copied_from` set) at [`copy_stake_for`] of it, so the stake and
/// the copy fee both come out of the swap (L9).
#[allow(clippy::too_many_arguments)]
pub fn start_swap_play(
    desk: Desk,
    wallet: Wallet,
    rpc: RpcContext,
    mut state: Signal<AppState>,
    swap: SwapLine,
    req: QuoteRequest,
    shown: Quote,
    kind: PlayKind,
) {
    let mut sub = Submission::new(false);
    // A copy never touches the Tray (as `flows::start_play`): only a played Tray is cleared.
    sub.request = (!matches!(kind, PlayKind::Copy { .. })).then(|| req.clone());
    sub.kind = kind.clone();
    let id = sub.id;
    {
        let mut s = state.write();
        s.submission = Some(sub);
        s.slip.run.clear();
    }
    dioxus::core::spawn_forever(async move {
        let outcome = swap_play(desk, wallet, rpc, state, id, swap, req, shown, kind).await;
        if let Err(words) = outcome {
            update_own(state, id, |s| {
                if s.busy() {
                    s.step = SubmitStep::Failed(words);
                }
            });
        }
    });
}

/// Fresh swap quote, fresh desk quote at `stake_u = min_out_u`, `[swap, verify, open_pending]`
/// as one v0 transaction, wallet signature, submit, confirm.
#[allow(clippy::too_many_arguments)]
async fn swap_play(
    desk: Desk,
    wallet: Wallet,
    rpc: RpcContext,
    mut state: Signal<AppState>,
    id: u64,
    shown_swap: SwapLine,
    req: QuoteRequest,
    shown: Quote,
    kind: PlayKind,
) -> Result<(), String> {
    let (opts, again) = match &kind {
        PlayKind::Copy { original, .. } => (
            OpenOptions {
                is_tip: false,
                copied_from: Some(
                    original
                        .parse::<Pubkey>()
                        .map_err(|_| "The original bet has an invalid address.".to_string())?,
                ),
            },
            "Copy Schedina",
        ),
        _ => (OpenOptions::default(), "Bet"),
    };
    let copy = opts.copied_from.is_some();
    if desk.is_demo() {
        return Err("Demo data: connect a desk to pay in another token.".into());
    }
    let SwapLine {
        token: tok,
        amount_in,
        quote: shown_sq,
    } = shown_swap;
    let user = match wallet.public_key() {
        Some(pk) => pk,
        None => wallet.connect().await.map_err(|e| flows::explain(&e))?,
    };
    let mint: Pubkey = tok
        .mint
        .parse()
        .map_err(|_| "bad mint address".to_string())?;
    let table = amm_lookup_table(rpc).await?;
    let needs_usdc_account = wallet
        .token_balance(&pda::user_usdc(&user))
        .await
        .map_err(|e| flows::explain(&e))?
        .is_none();
    let cluster_now = wallet
        .cluster_now_s()
        .await
        .map_err(|e| flows::explain(&e))?;

    // The pool may have moved since the drawer showed it: never stake less than shown.
    let sq = desk
        .swap_quote(&tok.mint, amount_in)
        .await
        .map_err(|e| e.plain(None))?;
    if sq.min_out_u < shown_sq.min_out_u {
        return Err(format!(
            "The swap price moved: {} now buys a stake of {} (was {}). Check it and tap {again} again.",
            tok.symbol,
            format::eur(sq.min_out_u),
            format::eur(shown_sq.min_out_u)
        ));
    }
    let stake_u = if copy {
        copy_stake_for(sq.min_out_u)
    } else {
        sq.min_out_u
    };
    let req = QuoteRequest { stake_u, ..req };
    let fresh = desk.quote(&req).await.map_err(|e| e.plain(None))?;
    let same = fresh.stake_u == req.stake_u
        && fresh.legs.len() == req.legs.len()
        && fresh
            .legs
            .iter()
            .zip(&req.legs)
            .all(|(q, r)| q.symbol == r.symbol && q.side == r.side);
    if !same {
        return Err(format!(
            "The desk priced a different bet. Tap {again} again."
        ));
    }
    if fresh.odds_offered_u < shown.odds_offered_u {
        return Err(format!(
            "The price moved: the total is now {} (was {}). Check it and tap {again} again.",
            format::odds(fresh.odds_offered_u),
            format::odds(shown.odds_offered_u)
        ));
    }
    if let PlayKind::Copy { legs, .. } = &kind {
        // `open_pending` escrows the stake and the copy fee from what the swap pays: a fee the
        // user did not see, or one the swap does not cover, stops here.
        if fresh.copy_fee_u == 0
            || fresh.copy_fee_u > shown.copy_fee_u.max(1)
            || fresh.stake_u + fresh.copy_fee_u > sq.min_out_u
        {
            return Err(format!(
                "The copy fee is now {} (was {}). Check it and tap {again} again.",
                format::eur(fresh.copy_fee_u),
                format::eur(shown.copy_fee_u)
            ));
        }
        let quoted: Vec<ShownLeg> = fresh
            .legs
            .iter()
            .map(|l| ShownLeg {
                symbol: l.symbol.clone(),
                game_id: l.game_id.clone(),
                side: match l.side {
                    Side::Long => 0,
                    Side::Short => 1,
                },
            })
            .collect();
        if !copy_matches(legs, &quoted) {
            return Err("The desk priced other picks than the original's. Not copied.".into());
        }
    }
    update_own(state, id, |s| {
        s.quote = Some(fresh.clone());
        s.request = Some(req.clone());
    });

    let signed = SignedMsg::from_wire(&fresh.quote_signer, &fresh.quote_msg, &fresh.quote_sig)
        .map_err(|e| format!("The desk's quote cannot be signed: {e}."))?;
    let shown_msg = ShownQuote {
        legs: fresh
            .legs
            .iter()
            .map(|l| ShownLeg {
                symbol: l.symbol.clone(),
                game_id: l.game_id.clone(),
                side: match l.side {
                    Side::Long => 0,
                    Side::Short => 1,
                },
            })
            .collect(),
        stake_u: fresh.stake_u,
        odds_offered_u: fresh.odds_offered_u,
        quote_expires_at: fresh.quote_expires_at,
        min_payout_per_unit_u: fresh.min_payout_per_unit_u,
    };
    let pool: Pubkey = sq
        .pool
        .parse()
        .map_err(|_| "bad pool address".to_string())?;
    let swap = ShownSwap {
        mint,
        amount_in,
        min_out_u: sq.min_out_u,
        pool,
    };
    // 53 bits, like every other `open_pending` (`schedina_tx::NONCE_MAX`): a 64-bit draw was
    // refused by `open_pending_ixs` ("nonce above 53 bits") almost every time.
    let nonce = flows::random_nonce();
    // The swap pays into the user's mock-USDC account: create it first if absent (rent paid by
    // the payer: L8-2's sponsor, or the user).
    let amm_ix = usize::from(needs_usdc_account);
    let build = move |payer: &Pubkey| {
        let three = swap_open_pending_ixs_with(
            &user,
            payer,
            &swap,
            &signed,
            &shown_msg,
            nonce,
            cluster_now,
            opts,
        )
        .map_err(|e| format!("This payment cannot be signed: {e}."))?;
        let mut ixs = Vec::with_capacity(4);
        if needs_usdc_account {
            ixs.push(create_ata_idempotent_ix(payer, &user, &pda::usdc_mint()));
        }
        ixs.extend(three);
        Ok(ixs)
    };
    // Refused before the wallet step, in the user's words.
    build(&user)?;
    let pending = pda::pending(&user, nonce).to_string();
    update_own(state, id, |s| {
        s.pending = Some(pending.clone());
        s.step = SubmitStep::Signing;
    });
    let sig = wallet
        .send(build, Compile::V0(table))
        .await
        .map_err(|e| explain(&e, amm_ix))?;
    update_own(state, id, |s| {
        s.sig = Some(sig.to_string());
        if s.step == SubmitStep::Signing {
            s.step = SubmitStep::Confirming;
        }
    });
    wallet.confirm(&sig).await.map_err(|e| {
        format!(
            "{} The transaction may still land: check Your bets on Account.",
            explain(&e, amm_ix)
        )
    })?;
    let mut s = state.write();
    s.progress.entry(pending.clone()).or_insert(Progress {
        pending,
        stage: crate::desk::types::Stage::Escrowed,
        fills: Vec::new(),
        ticket: None,
    });
    s.landed(id);
    Ok(())
}
