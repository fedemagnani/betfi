//! The three things the app signs (plan 2.E): placing a schedina (`open_pending`), cashing one
//! out (`cash_out`), and the devnet faucet (`faucet_usdc`). Instructions come from
//! `schedina_tx` (host-tested); here they are only fed with the desk's answers and the wallet.
//!
//! Every flow runs as a root-level task (`spawn_forever`) so leaving the screen does not cancel
//! a transaction between signature and confirmation. Mainnet is refused inside
//! [`Wallet::send`] before any blockhash fetch or wallet prompt (D-046). Each flow hands the
//! wallet a builder taking the payer (L8-2): the desk's sponsor while it sponsors, so a user
//! never needs SOL, else the user.

use dioxus::prelude::*;
use schedina_tx::{
    CashOutTicket, FAUCET_AMOUNT_U, OpenOptions, Pubkey, ShownLeg, ShownQuote, SignedMsg,
    cash_out_ixs, copy_matches, custom_error_code, faucet_ixs, is_own_tip, nonce_from_entropy,
    open_pending_ixs, pda, program_error_words,
};

use crate::desk::types::{CashoutQuote, Quote, QuoteRequest, Side, Stage, Ticket};
use crate::desk::{self, Desk};
use crate::format;
use crate::state::slip::{SingleRun, SingleStatus};
use crate::state::{AppState, PlayKind, Progress, Submission, SubmitStep};
use crate::stream::StreamCmd;
use crate::wallet::{Compile, Wallet, WalletError};

/// A fresh `open_pending` nonce (L2-2, D-054), at most 53 bits so JavaScript readers of the
/// desk's JSON see it exactly (`schedina_tx::NONCE_MAX`). Uniqueness is what matters, not
/// secrecy: `crypto.getRandomValues`, or `Math.random` (52 bits of it) if that is missing.
pub fn random_nonce() -> u64 {
    let mut bytes = [0u8; 8];
    let crypto = web_sys::window().and_then(|w| w.crypto().ok());
    if crypto.is_none_or(|c| c.get_random_values_with_u8_array(&mut bytes).is_err()) {
        let draw = (js_sys::Math::random() * 4_503_599_627_370_496.0) as u64; // 2^52
        bytes = draw.to_le_bytes();
    }
    nonce_from_entropy(bytes)
}

fn shown_side(side: Side) -> u8 {
    match side {
        Side::Long => 0,
        Side::Short => 1,
    }
}

/// The legs of a ticket as the program compares a copy with its original.
pub fn ticket_shown_legs(t: &Ticket) -> Vec<ShownLeg> {
    t.legs
        .iter()
        .map(|l| ShownLeg {
            symbol: l.symbol.clone(),
            game_id: l.game_id.clone(),
            side: shown_side(l.side),
        })
        .collect()
}

/// A wallet or network failure in the user's words: program errors by code, the usual
/// funding problems by their node message, anything else as is. While the desk sponsors
/// transactions (L8-2) nothing here sends the user for SOL.
pub fn explain(e: &WalletError) -> String {
    match e {
        // Already the user's words (a builder's refusal, the sponsoring's own errors).
        WalletError::Build(_) | WalletError::TooLarge | WalletError::SponsorUnavailable => {
            return e.to_string();
        }
        // The desk's reason is shown as it is, unless it carries a program error we can word
        // (a sponsored transaction's preflight runs at the desk).
        WalletError::SponsorRefused(reason) => {
            return match custom_error_code(reason).and_then(program_error_words) {
                Some(words) => format!("Not placed: {words}."),
                None => e.to_string(),
            };
        }
        _ => {}
    }
    let text = e.to_string();
    let code = custom_error_code(&text);
    if let Some(words) = code.and_then(program_error_words) {
        return format!("Not placed: {words}.");
    }
    let sponsored = crate::wallet::sponsoring();
    // Code 1 below the program's 6000+ range comes from a CPI: the System program's
    // insufficient lamports (account rent) or the Token program's insufficient funds (3.C saw
    // the first on devnet with a wallet holding too little SOL for the Pending's rent).
    if code == Some(1) {
        if sponsored {
            return "Not placed: not enough test USDC for this stake: tap \u{201c}Get test USDC\u{201d}."
                .into();
        }
        return "Not placed: this wallet lacks SOL for the network fee and deposit, or test USDC \
                for the stake."
            .into();
    }
    if text.contains("insufficient funds") {
        return "Not enough test USDC for this stake: tap \u{201c}Get test USDC\u{201d}.".into();
    }
    if text.contains("no record of a prior credit") || text.contains("insufficient lamports") {
        if sponsored {
            return WalletError::SponsorUnavailable.to_string();
        }
        return "This wallet has no SOL for the network fee (devnet: faucet.solana.com).".into();
    }
    text
}

/// Update submission `id` if it is still the current one (a later Bet replaces it; this
/// flow's late confirmation or failure must not touch the new one).
fn update_own(mut state: Signal<AppState>, id: u64, f: impl FnOnce(&mut Submission)) {
    if let Some(s) = state.write().submission.as_mut().filter(|s| s.id == id) {
        f(s);
    }
}

/// "Bet" (and "Copy Schedina"): fresh signed quote, `[Ed25519 verify, open_pending]`, wallet signature, submit,
/// confirm. The quote shown in the slip is `shown`; a fresh quote paying less stops here so
/// the user sees the new numbers first. `kind` sets `is_tip` / `copied_from`; a copy is
/// re-quoted through `copy-quote` and its legs are the original's (`req.legs`).
pub fn start_play(
    desk: Desk,
    wallet: Wallet,
    stream: Coroutine<StreamCmd>,
    mut state: Signal<AppState>,
    req: QuoteRequest,
    shown: Quote,
    kind: PlayKind,
) {
    let copy = matches!(kind, PlayKind::Copy { .. });
    // A parlay or a copy is not part of a Singles run.
    state.write().slip.run.clear();
    if desk.is_demo() {
        // No program to send to: follow the desk's example pending through the demo stream.
        start_demo(state, stream, false);
        let mut s = state.write();
        if let Some(sub) = s.submission.as_mut() {
            sub.quote = Some(shown);
            sub.kind = kind;
        }
        if !copy {
            s.tray.legs.clear();
            s.tray.tip = false;
            s.tray.tip_name.clear();
        }
        return;
    }
    let mut sub = Submission::new(false);
    // A copy never touches the Tray: only a played Tray is cleared when it lands.
    sub.request = (!copy).then(|| req.clone());
    sub.kind = kind.clone();
    let id = sub.id;
    state.write().submission = Some(sub);
    dioxus::core::spawn_forever(async move {
        let outcome = play(desk, wallet, state, id, req, shown, kind).await;
        if let Err(words) = outcome {
            // A confirmation that timed out after the stream saw the Pending land is not a
            // failure: the Confirm screen keeps following the stream.
            update_own(state, id, |s| {
                if s.busy() {
                    s.step = SubmitStep::Failed(words);
                }
            });
        }
    });
}

async fn play(
    desk: Desk,
    wallet: Wallet,
    mut state: Signal<AppState>,
    id: u64,
    req: QuoteRequest,
    shown: Quote,
    kind: PlayKind,
) -> Result<(), String> {
    let card = match &state.peek().card {
        crate::state::Load::Ready(c) => Some(c.clone()),
        _ => None,
    };
    let opts = match &kind {
        PlayKind::Plain => OpenOptions::default(),
        PlayKind::Tip { .. } => OpenOptions {
            is_tip: true,
            copied_from: None,
        },
        PlayKind::Copy { original, .. } => OpenOptions {
            is_tip: false,
            copied_from: Some(
                original
                    .parse::<Pubkey>()
                    .map_err(|_| "The original bet has an invalid address.".to_string())?,
            ),
        },
    };
    // Connect first: a first wallet connection can take longer than a prompt should.
    let user = match wallet.public_key() {
        Some(pk) => pk,
        None => wallet.connect().await.map_err(|e| explain(&e))?,
    };
    // The Pending's `expires_at` is checked against the cluster's clock, not the browser's
    // (L3-4): read the Clock sysvar before asking the quote (5 min, D-664). The program accepts
    // `now + 30 ..= now + 360` and the app sets `now + 345`, so the wallet prompt has ~5 min.
    let cluster_now = wallet.cluster_now_s().await.map_err(|e| explain(&e))?;
    // A sponsored bet is v0 with the AMM's lookup table when the cluster has one (5 legs do
    // not fit a legacy message with two signers, L8-2); read before the quote's clock starts.
    let table = if wallet.will_sponsor().await {
        crate::wallet::amm_lookup_table(wallet.rpc())
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    let fresh = match &kind {
        PlayKind::Copy { original, .. } => desk.copy_quote(original, req.stake_u).await,
        // A named tip (L9): the desk keeps the name for the tip this wallet opens with the quote.
        PlayKind::Tip { title: Some(t) } => {
            let named = QuoteRequest {
                title: Some(t.clone()),
                wallet: Some(user.to_string()),
                ..req.clone()
            };
            desk.quote(&named).await
        }
        _ => desk.quote(&req).await,
    }
    .map_err(|e| e.plain(card.as_ref()))?;
    // The desk must have priced exactly the picks and stake the user asked for, in order.
    let same_request = fresh.stake_u == req.stake_u
        && shown.stake_u == req.stake_u
        && fresh.legs.len() == req.legs.len()
        && fresh
            .legs
            .iter()
            .zip(&req.legs)
            .all(|(q, r)| q.symbol == r.symbol && q.side == r.side);
    // The v2 buttons' words: the drawer's "Bet", the copy page's "Copy Schedina".
    let again = match &kind {
        PlayKind::Copy { .. } => "Copy Schedina",
        _ => "Bet",
    };
    if !same_request {
        return Err(format!(
            "The desk priced a different bet. Tap {again} again."
        ));
    }
    if fresh.payout_u < shown.payout_u {
        return Err(format!(
            // USDC as signed: this async flow has no euro rate in scope, and printing USDC with
            // a € sign would misstate the amount (7.2a-p follow-up).
            "The price moved: the payout is now {} (was {}). Check it and tap {again} again.",
            crate::ui::kit::money::usdc_text(fresh.payout_u),
            crate::ui::kit::money::usdc_text(shown.payout_u)
        ));
    }
    if let PlayKind::Copy { legs, .. } = &kind {
        // The program charges the copy fee from the stake (D-064), on top of it; the user
        // must have seen it, and no higher one.
        if fresh.copy_fee_u == 0 || fresh.copy_fee_u > shown.copy_fee_u {
            return Err(format!(
                "The copy fee is now {} (was {}). Check it and tap {again} again.",
                format::eur(fresh.copy_fee_u),
                format::eur(shown.copy_fee_u)
            ));
        }
        // `finalize` refuses a copy whose legs differ from the original's (CopyMismatch),
        // and the stake would only come back as a refund: check before signing.
        let quoted: Vec<ShownLeg> = fresh
            .legs
            .iter()
            .map(|l| ShownLeg {
                symbol: l.symbol.clone(),
                game_id: l.game_id.clone(),
                side: shown_side(l.side),
            })
            .collect();
        if !copy_matches(legs, &quoted) {
            return Err("The desk priced other picks than the original's. Not copied.".into());
        }
    }
    update_own(state, id, |s| s.quote = Some(fresh.clone()));

    let signed = SignedMsg::from_wire(&fresh.quote_signer, &fresh.quote_msg, &fresh.quote_sig)
        .map_err(|e| format!("The desk's quote cannot be signed: {e}."))?;
    // What the user sees is checked against the signed bytes: leg hashes and sides, stake,
    // odds, expiry, and a floor within (0, offered].
    let shown_msg = ShownQuote {
        legs: fresh
            .legs
            .iter()
            .map(|l| ShownLeg {
                symbol: l.symbol.clone(),
                game_id: l.game_id.clone(),
                side: shown_side(l.side),
            })
            .collect(),
        stake_u: fresh.stake_u,
        odds_offered_u: fresh.odds_offered_u,
        quote_expires_at: fresh.quote_expires_at,
        min_payout_per_unit_u: fresh.min_payout_per_unit_u,
    };
    let nonce = random_nonce();
    // The program's `TipNotAllowed` rule (a copy cannot be a tip) is checked before the wallet
    // step. The payer (L8-2: the desk's sponsor, or the user) changes nothing the checks look at.
    let build = move |payer: &Pubkey| {
        open_pending_ixs(&user, payer, &signed, &shown_msg, nonce, cluster_now, opts)
            .map(Vec::from)
            .map_err(|e| match e {
                schedina_tx::TxError::Message(m) if opts.is_tip && m.contains("tip") => {
                    format!(
                        "Not shared as a tip: {m}. Turn off \u{201c}Share as a tip\u{201d} to place it."
                    )
                }
                e => format!("The desk's quote cannot be signed: {e}."),
            })
    };
    // Refused before the wallet step, in the user's words.
    build(&user)?;
    let pending = pda::pending(&user, nonce).to_string();
    update_own(state, id, |s| {
        s.pending = Some(pending.clone());
        s.step = SubmitStep::Signing;
    });

    let sig = wallet
        .send(build, Compile::Bet(table))
        .await
        .map_err(|e| explain(&e))?;
    update_own(state, id, |s| {
        s.sig = Some(sig.to_string());
        // The stream may already have reported the Pending (then the step is Landed).
        if s.step == SubmitStep::Signing {
            s.step = SubmitStep::Confirming;
        }
    });
    wallet.confirm(&sig).await.map_err(|e| {
        format!(
            "{} The transaction may still land: check Your bets on Account.",
            explain(&e)
        )
    })?;

    let mut s = state.write();
    // Landed = escrowed, even if the stream's frame has not arrived yet.
    s.progress.entry(pending.clone()).or_insert(Progress {
        pending,
        stage: Stage::Escrowed,
        fills: Vec::new(),
        ticket: None,
    });
    s.landed(id);
    Ok(())
}

/// One single of a Singles run: its request (one leg), the quote the slip showed for it, and its
/// name for the progress list.
// 7.1b: the bet drawer places a parlay only (proposed D-490); Singles are kept for later.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub struct SingleBet {
    pub req: QuoteRequest,
    pub shown: Quote,
    pub label: String,
}

/// Singles (6.1c): one ticket per pick, each through [`start_play`]'s checks and its own wallet
/// signature, one after another; the run stops at the first single that is not placed. Each
/// placed single leaves the slip; Confirm lists the run (`state.slip.run`) and follows the
/// single being placed.
// 7.1b: unused while the bet drawer places a parlay only (proposed D-490).
#[allow(dead_code)]
pub fn start_singles(
    desk: Desk,
    wallet: Wallet,
    stream: Coroutine<StreamCmd>,
    mut state: Signal<AppState>,
    bets: Vec<SingleBet>,
    kind: PlayKind,
) {
    let Some(first) = bets.first().cloned() else {
        return;
    };
    state.write().slip.run = bets
        .iter()
        .map(|b| SingleRun {
            label: b.label.clone(),
            pending: None,
            status: SingleStatus::Waiting,
        })
        .collect();
    if desk.is_demo() {
        // No program: the desk's one example pending plays for the run, labelled as a demo.
        start_demo(state, stream, false);
        let mut s = state.write();
        let pending = s.submission.as_ref().and_then(|x| x.pending.clone());
        if let Some(sub) = s.submission.as_mut() {
            sub.quote = Some(first.shown);
            sub.kind = kind;
        }
        for r in &mut s.slip.run {
            r.status = SingleStatus::Placed;
            r.pending = pending.clone();
        }
        s.tray.legs.clear();
        s.tray.tip = false;
        s.tray.tip_name.clear();
        return;
    }
    dioxus::core::spawn_forever(async move {
        for (i, bet) in bets.into_iter().enumerate() {
            let mut sub = Submission::new(false);
            sub.kind = kind.clone();
            let id = sub.id;
            {
                let mut s = state.write();
                s.submission = Some(sub);
                if let Some(r) = s.slip.run.get_mut(i) {
                    r.status = SingleStatus::Placing;
                }
            }
            let leg = bet.req.legs.first().cloned();
            let outcome = play(
                desk.clone(),
                wallet,
                state,
                id,
                bet.req,
                bet.shown,
                kind.clone(),
            )
            .await;
            let mut s = state.write();
            let pending = s
                .submission
                .as_ref()
                .filter(|x| x.id == id)
                .and_then(|x| x.pending.clone());
            match outcome {
                Ok(()) => {
                    if let Some(r) = s.slip.run.get_mut(i) {
                        r.status = SingleStatus::Placed;
                        r.pending = pending;
                    }
                    if let Some(l) = leg {
                        s.tray
                            .legs
                            .retain(|t| !(t.symbol == l.symbol && t.side == l.side));
                    }
                }
                Err(words) => {
                    if let Some(sub) = s.submission.as_mut().filter(|x| x.id == id && x.busy()) {
                        sub.step = SubmitStep::Failed(words.clone());
                    }
                    if let Some(r) = s.slip.run.get_mut(i) {
                        r.status = SingleStatus::Failed(words);
                        r.pending = pending;
                    }
                    return;
                }
            }
        }
        let mut s = state.write();
        s.tray.tip = false;
        s.tray.tip_name.clear();
    });
}

/// Demo mode: follow one of the example pendings (`minted` or `refunded`) from a replay.
pub fn start_demo(mut state: Signal<AppState>, stream: Coroutine<StreamCmd>, refunded: bool) {
    let frame = if refunded {
        desk::fixtures::PROGRESS_REFUNDED
    } else {
        desk::fixtures::PROGRESS_ESCROWED
    };
    let mut sub = Submission::new(true);
    sub.pending = desk::demo_pending(frame);
    sub.step = SubmitStep::Landed;
    state.write().submission = Some(sub);
    stream.send(StreamCmd::ReplayDemo);
}

/// The ticket fields a cash-out needs, checked against the connected wallet.
fn cash_out_ticket(t: &Ticket, wallet_user: &Pubkey) -> Result<CashOutTicket, String> {
    let user: Pubkey = t
        .user
        .parse()
        .map_err(|_| "This bet has an invalid owner address.".to_string())?;
    if user != *wallet_user {
        return Err("This bet belongs to another wallet.".into());
    }
    let ticket: Pubkey = t
        .ticket
        .parse()
        .map_err(|_| "This bet has an invalid address.".to_string())?;
    let parse = |s: &Option<String>| s.as_deref().and_then(|k| k.parse::<Pubkey>().ok());
    let tipster = parse(&t.tipster);
    let copied_from = parse(&t.copied_from);
    if t.copied_from.is_some() && copied_from.is_none() {
        return Err("This bet has an invalid original address.".into());
    }
    Ok(CashOutTicket {
        user,
        nonce: t.nonce,
        ticket,
        own_tip: is_own_tip(&user, tipster.as_ref(), copied_from.as_ref()),
    })
}

/// "Incassa", second tap: `[Ed25519 verify, cash_out]` for the signed offer the user saw,
/// then the user's tickets are reloaded. Returns the signature.
pub async fn cash_out(
    desk: Desk,
    wallet: Wallet,
    state: Signal<AppState>,
    ticket: Ticket,
    offer: CashoutQuote,
) -> Result<String, String> {
    if desk.is_demo() {
        return Err("Demo data: connect a desk to cash out for real.".into());
    }
    let user = match wallet.public_key() {
        Some(pk) => pk,
        None => wallet.connect().await.map_err(|e| explain(&e))?,
    };
    let t = cash_out_ticket(&ticket, &user)?;
    if offer.ticket != ticket.ticket {
        return Err("The cash-out offer is for another bet.".into());
    }
    let signed = SignedMsg::from_wire(&offer.quote_signer, &offer.quote_msg, &offer.quote_sig)
        .map_err(|e| format!("The cash-out offer cannot be signed: {e}."))?;
    // The sponsor (L8-2) pays the fee only: `cash_out` has no payer account.
    let build = move |_payer: &Pubkey| {
        cash_out_ixs(&t, &signed, offer.c_u)
            .map(Vec::from)
            .map_err(|e| format!("The cash-out offer cannot be signed: {e}."))
    };
    build(&user)?;
    let sig = wallet
        .send_instructions(build)
        .await
        .map_err(|e| explain(&e))?;
    wallet.confirm(&sig).await.map_err(|e| explain(&e))?;
    crate::load_tickets(desk, state, ticket.user.clone()).await;
    Ok(sig.to_string())
}

/// Mock USDC from the program's faucet into the connected wallet's account (created if
/// absent). Devnet and localnet only: the RPC is never mainnet (D-046).
pub async fn faucet(wallet: Wallet) -> Result<String, String> {
    let user = match wallet.public_key() {
        Some(pk) => pk,
        None => wallet.connect().await.map_err(|e| explain(&e))?,
    };
    // Sponsored (L8-2), the sponsor pays the account's and the claim's rent and the user signs
    // the claim's memo.
    let sig = wallet
        .send_instructions(move |payer| Ok(faucet_ixs(&user, payer, FAUCET_AMOUNT_U)))
        .await
        .map_err(|e| explain(&e))?;
    wallet.confirm(&sig).await.map_err(|e| explain(&e))?;
    Ok(sig.to_string())
}

/// The connected wallet's mock-USDC balance, `None` before its account exists.
pub async fn usdc_balance(wallet: Wallet) -> Result<Option<u64>, String> {
    let user = wallet.public_key().ok_or("Connect a wallet first")?;
    wallet
        .token_balance(&pda::user_usdc(&user))
        .await
        .map_err(|e| explain(&e))
}
