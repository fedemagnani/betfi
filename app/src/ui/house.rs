//! Account (`/account`, step 7.1f; 31 Oct, user request: the wallet, the faucet and the bets)
//! and the two pages that left it:
//! - **Account**: "Your wallet" (address, Solscan, the network, its worth in €: test USDC plus
//!   the mock tokens at their swap value) and "Faucet" (a coin, its balance, the airdrop, then a
//!   celebration with the transaction on the explorer), both in `faucet.rs`; then "Your bets"
//!   (`#bets`: the Open / Settled switch, the cards, the cash-out and its celebration; the My
//!   Bets screen until L8-4), in `my_schedine.rs`.
//! - **House** (`/house`): the vault (free, locked, reserve), the bets by state, the hedger's
//!   positions by team, the fee totals with the builder revenue (`GET /house`, spec 7). In mode
//!   M the hedges are mock-filled, and the section says so. The desk's market symbols never
//!   reach the screen.
//! - **Profile** (`/profile`): handle and the four social links, saved by a wallet-signed
//!   `PUT /profile` (7.1d's v2 message `profile_msg::message_v2`, D-471…D-473; link rules:
//!   [`link_rules`]).

pub mod link_rules;

use dioxus::prelude::*;

use super::faucet::{FaucetCard, WalletCard};
use super::kit::{self, PurpleCTA, SectionTitle, StatGrid, StatTile, SummaryRow, SummaryRows};
use super::my_schedine::YourBets;
use super::tipsters::{profile_msg, profile_words};
use super::{use_app_state, use_desk};
use crate::Route;
use crate::desk::types::{
    Card, House as HouseData, Pick, Position, ProfileLinks, ProfileUpdateV2, PublicProfile, Venue,
};
use crate::format;
use crate::state::Load;
use crate::wallet::use_wallet_api;
use link_rules::Net;

#[component]
pub fn Account() -> Element {
    let desk = use_desk();
    let tokens = use_resource(move || {
        let desk = desk.clone();
        async move { desk.tokens().await.unwrap_or_default() }
    });
    let tokens = tokens.read().clone().unwrap_or_default();
    rsx! {
        div { class: "app-account acct", id: "account",
            h1 { class: "sr-only", "Account" }
            WalletCard {}
            FaucetCard { tokens }
            YourBets {}
        }
    }
}

/// The house's numbers on their own page (31 Oct: off Account).
#[component]
pub fn House() -> Element {
    rsx! {
        div { class: "app-account acct", id: "house",
            h1 { class: "sr-only", "House" }
            HouseSection {}
        }
    }
}

/// The profile editor on its own page (31 Oct: off Account).
#[component]
pub fn ProfileEdit() -> Element {
    let desk = use_desk();
    let state = use_app_state();
    let mut profile = use_resource(move || {
        let user = state.read().user.clone();
        let desk = desk.clone();
        async move {
            let u = user?;
            Some(desk.public_profile(&u).await)
        }
    });
    let loaded = match &*profile.read() {
        Some(Some(Ok(p))) => Some(p.clone()),
        _ => None,
    };
    let failed = matches!(&*profile.read(), Some(Some(Err(_))));
    rsx! {
        div { class: "app-account acct", id: "profile",
            h1 { class: "sr-only", "Profile" }
            ProfileCard { loaded, failed, onsaved: move |_| profile.restart() }
        }
    }
}

// ---------------------------------------------------------------- profile (D-471…D-473)

/// The profile editor: the handle (3–20 lower-case letters, digits, `_`) and the four social
/// links (bare handles, D-473). Save signs the v2 message with the connected wallet (which must
/// be the viewer's) and sends `PUT /profile` with all four links (`links` replaces them; an
/// empty field clears one). Links need a handle (D-472).
#[component]
fn ProfileCard(loaded: Option<PublicProfile>, failed: bool, onsaved: EventHandler<()>) -> Element {
    let desk = use_desk();
    let state = use_app_state();
    let wallet = use_wallet_api();
    let demo = desk.is_demo();
    let user = state.read().user.clone();
    let mut handle = use_signal(String::new);
    let mut links = use_signal(|| [String::new(), String::new(), String::new(), String::new()]);
    let mut status = use_signal(|| None::<Result<String, String>>);
    let mut busy = use_signal(|| false);
    // Fill the fields from the desk's profile once it loads (and again for another wallet).
    let mut filled_for = use_signal(|| None::<String>);
    {
        let loaded = loaded.clone();
        use_effect(use_reactive!(|loaded| {
            if let Some(p) = loaded
                && filled_for.peek().as_deref() != Some(p.wallet.as_str())
            {
                handle.set(p.handle.clone().unwrap_or_default());
                let l = p.links.clone().unwrap_or_default();
                links.set([
                    l.x.unwrap_or_default(),
                    l.youtube.unwrap_or_default(),
                    l.telegram.unwrap_or_default(),
                    l.instagram.unwrap_or_default(),
                ]);
                filled_for.set(Some(p.wallet.clone()));
            }
        }));
    }
    let typed = handle();
    let handle_err = (!typed.is_empty())
        .then(|| profile_msg::handle_problem(&typed))
        .flatten();
    let link_errs: Vec<Option<String>> = Net::ALL
        .iter()
        .zip(links().iter())
        .map(|(n, v)| link_rules::clean_link(*n, v).err())
        .collect();
    let invalid = handle_err.is_some() || link_errs.iter().any(Option::is_some);
    let no_handle = typed.is_empty();
    let off = demo || user.is_none() || busy() || invalid || no_handle;

    let save = move |_| {
        let desk = desk.clone();
        async move {
            let Some(owner) = state.peek().user.clone() else {
                return;
            };
            busy.set(true);
            status.set(None);
            let r = write_profile(
                desk,
                wallet,
                owner,
                handle.peek().clone(),
                links.peek().clone(),
            )
            .await;
            busy.set(false);
            if r.is_ok() {
                onsaved.call(());
            }
            status.set(Some(
                r.map(|_| "Saved. Your tipster page shows it.".to_string()),
            ));
        }
    };

    rsx! {
        section { id: "account-profile", aria_label: "Profile", class: "acct-card",
            SectionTitle {
                title: "Profile",
                small: true,
                end: rsx! {
                    if let Some(u) = user.clone() {
                        Link { to: Route::TipsterPage { wallet: u }, class: "acct-link", id: "account-handle",
                            "Your tipster page"
                            kit::SymIcon { sym: kit::Sym::ChevronRight, size: 16 }
                        }
                    }
                },
            }
            if user.is_none() {
                p { class: "acct-sub", "Connect a wallet to choose a handle." }
            } else if failed {
                p { class: "acct-sub", "The profile is unavailable right now." }
            }
            div { class: "acct-fields",
                label { class: "acct-field",
                    span { class: "acct-field-label", "Handle" }
                    span { class: "acct-input",
                        span { class: "acct-input-at", "aria-hidden": "true", "@" }
                        input {
                            id: "profile-handle",
                            r#type: "text",
                            value: "{typed}",
                            placeholder: "your_handle",
                            maxlength: "20",
                            autocomplete: "off",
                            spellcheck: "false",
                            aria_invalid: "{handle_err.is_some()}",
                            oninput: move |e| handle.set(e.value().trim().to_ascii_lowercase()),
                        }
                    }
                    if let Some(e) = handle_err.clone() {
                        span { class: "acct-field-error", role: "alert", "{e}." }
                    }
                }
                for (i, net) in Net::ALL.iter().copied().enumerate() {
                    label { key: "{net.key()}", class: "acct-field",
                        span { class: "acct-field-label", "{net.name()}" }
                        span { class: "acct-input",
                            span { class: "acct-input-at", "aria-hidden": "true", "@" }
                            input {
                                id: "profile-{net.key()}",
                                r#type: "text",
                                value: "{links.read()[i]}",
                                placeholder: "handle",
                                maxlength: "33",
                                autocomplete: "off",
                                spellcheck: "false",
                                aria_invalid: "{link_errs[i].is_some()}",
                                oninput: move |e| links.with_mut(|l| l[i] = e.value()),
                            }
                        }
                        if let Some(e) = link_errs[i].clone() {
                            span { class: "acct-field-error", role: "alert", "{e}." }
                        }
                    }
                }
            }
            if no_handle && links.read().iter().any(|l| !l.trim().is_empty()) {
                p { class: "acct-sub", "Links need a handle: choose one first." }
            }
            PurpleCTA {
                id: "profile-save",
                label: "Save profile",
                busy: busy().then(|| "Signing…".to_string()),
                disabled: off,
                onclick: save,
            }
            p { class: "acct-sub",
                if demo {
                    "Demo data: there is no desk to save to."
                } else {
                    "Saving signs a message with your wallet (no transaction, no fee). A handle changes at most once a day."
                }
            }
            match status() {
                Some(Ok(t)) => rsx! { p { class: "acct-status ok", id: "profile-status", role: "status", "{t}" } },
                Some(Err(e)) => rsx! { p { class: "acct-status error", id: "profile-status", role: "alert", "{e}" } },
                None => rsx! {},
            }
        }
    }
}

/// Sign the v2 profile message (handle and the four links) with the connected wallet, which must
/// be `owner`, and send it.
async fn write_profile(
    desk: crate::desk::Desk,
    wallet: crate::wallet::Wallet,
    owner: String,
    handle: String,
    typed: [String; 4],
) -> Result<PublicProfile, String> {
    if desk.is_demo() {
        return Err(profile_words(&crate::desk::ProfileWriteError::Demo));
    }
    if let Some(p) = profile_msg::handle_problem(&handle) {
        return Err(format!("Not saved: {p}."));
    }
    let mut cleaned: [Option<String>; 4] = Default::default();
    for (i, net) in Net::ALL.iter().enumerate() {
        cleaned[i] =
            link_rules::clean_link(*net, &typed[i]).map_err(|e| format!("Not saved: {e}."))?;
    }
    let pk = match wallet.public_key() {
        Some(pk) => pk,
        None => wallet.connect().await.map_err(|e| format!("{e}."))?,
    };
    if pk.to_string() != owner {
        return Err(format!(
            "The connected wallet is {}: connect {} to change its profile.",
            format::short(&pk.to_string()),
            format::short(&owner)
        ));
    }
    let ts = (js_sys::Date::now() / 1000.0) as i64;
    let refs: [Option<&str>; 4] = [
        cleaned[0].as_deref(),
        cleaned[1].as_deref(),
        cleaned[2].as_deref(),
        cleaned[3].as_deref(),
    ];
    let msg = profile_msg::message_v2(&owner, &handle, None, None, Some(refs), None, ts);
    let sig = wallet
        .sign_message(msg.as_bytes())
        .await
        .map_err(|e| format!("Not saved: {e}."))?;
    let [x, youtube, telegram, instagram] = cleaned;
    let body = ProfileUpdateV2 {
        wallet: owner,
        handle,
        title: None,
        ticket: None,
        ts,
        sig: profile_msg::base64(&sig),
        links: Some(ProfileLinks {
            x,
            youtube,
            telegram,
            instagram,
        }),
        follow: None,
    };
    desk.put_profile_v2(&body)
        .await
        .map_err(|e| profile_words(&e))
}

// ---------------------------------------------------------------- the house

/// The house's numbers (`GET /house`), under the profile.
#[component]
fn HouseSection() -> Element {
    let desk = use_desk();
    let state = use_app_state();
    let house = use_resource(move || {
        let desk = desk.clone();
        async move { desk.house().await }
    });
    let card = match &state.read().card {
        Load::Ready(c) => Some(c.clone()),
        _ => None,
    };
    let mock_profile = state.read().mock_fills();
    rsx! {
        section { class: "acct-house", id: "account-house", aria_labelledby: "house-title",
            SectionTitle { title: "The house", small: true, id: "house-title" }
            p { class: "acct-sub", "The money behind every bet, and how it is covered." }
            match &*house.read() {
                None => rsx! { p { class: "acct-sub", role: "status", "Loading…" } },
                Some(Err(e)) => rsx! { p { class: "acct-status error", role: "alert", "{e.plain(card.as_ref())}" } },
                Some(Ok(h)) => rsx! { HouseView { h: h.clone(), card, mock_profile } },
            }
        }
    }
}

/// "Real Madrid at Olympiacos" and "8 contracts on Real Madrid" for a signed position on a Card
/// game (positive = the team the market's yes side is on, D-021).
fn backing(card: Option<&Card>, symbol: &str, contracts: i64) -> (String, String) {
    let game = card.and_then(|c| c.games.iter().find(|g| g.symbol == symbol));
    let Some(g) = game else {
        return (
            "A game off tonight's card".into(),
            format::contracts(contracts.unsigned_abs()),
        );
    };
    let yes = g.long_team;
    let team = if contracts >= 0 {
        yes
    } else {
        match yes {
            Pick::Away => Pick::Home,
            Pick::Home => Pick::Away,
        }
    };
    (
        format!("{} at {}", g.away.name, g.home.name),
        format!(
            "{} on {}",
            format::contracts(contracts.unsigned_abs()),
            g.team(team).name
        ),
    )
}

/// "€0.0055" (four decimals: fees per fill are fractions of a cent), in euros when the rate is
/// known.
fn eur_fine(usdc_u: u64, fx: Option<u64>) -> String {
    match fx {
        Some(r) => format::eur_fine(kit::money::eur_u(usdc_u, r)),
        None => format!("{} USDC", format::eur_fine(usdc_u).trim_start_matches('€')),
    }
}

#[component]
fn HouseView(h: HouseData, card: Option<Card>, mock_profile: bool) -> Element {
    let fx = kit::money::use_fx();
    let sol_rate = kit::money::use_sol_rate();
    let mock = h.hedger.venue != Venue::Pascal || mock_profile;
    let t = &h.tickets;
    let pair = |u: u64| kit::money::pair(u, fx).0;
    let (free_eur, free_sol) = match fx {
        Some(r) => {
            let e = kit::money::eur_u(h.vault.free_u, r);
            (format::eur(e), kit::money::approx_sol(e, sol_rate))
        }
        None => (kit::money::usdc_text(h.vault.free_u), None),
    };
    let counts = [
        ("Placing", t.pending),
        ("Open", t.open),
        ("Settled", t.settled),
        ("Cashed out", t.cashed_out),
        ("Refunded", t.refunded),
    ];
    let mut hedges = vec![SummaryRow::plain(
        "Cash available to the hedger",
        pair(h.hedger.free_collateral_u),
    )];
    for p in &h.hedger.positions {
        hedges.push(position_row(p, card.as_ref()));
    }
    rsx! {
        if mock {
            p { class: "acct-mock", id: "house-mock", role: "note",
                span { class: "acct-mock-tag", "Mock-filled" }
                "Hedges are signed for Pascal and filled by a mock: test money on devnet, real Pascal prices."
            }
        }
        div { class: "acct-card", id: "house-vault", aria_label: "Vault",
            kit::PayoutHero { label: "Vault · free to back new bets", eur: free_eur, sol: free_sol }
            SummaryRows {
                compact: true,
                rows: vec![
                    SummaryRow::plain("Locked for open bets", pair(h.vault.locked_u)),
                    SummaryRow::plain("Reserve kept aside", pair(h.vault.reserve_u)),
                ],
            }
        }
        div { class: "acct-card", id: "house-bets", aria_label: "Bets",
            span { class: "acct-label", "Bets" }
            StatGrid {
                for (label, n) in counts {
                    StatTile { key: "{label}", label, value: n.to_string() }
                }
            }
        }
        div { class: "acct-card", id: "house-hedger", aria_label: "Hedges",
            span { class: "acct-label",
                if h.hedger.venue == Venue::Pascal { "Hedges · on Pascal" } else { "Hedges · signed for Pascal, mock-filled" }
            }
            SummaryRows { compact: true, rows: hedges }
            if h.hedger.positions.is_empty() {
                p { class: "acct-sub", "No cover held right now." }
            }
        }
        div { class: "acct-card", id: "house-fees", aria_label: "Fees so far",
            span { class: "acct-label", "Fees so far" }
            SummaryRows {
                compact: true,
                rows: vec![
                    SummaryRow::plain("Our margin", pair(h.fees.margin_u)),
                    SummaryRow::plain("Paid to Pascal", pair(h.fees.pascal_fees_u)),
                    SummaryRow::payout("Builder revenue", eur_fine(h.fees.builder_fees_u, fx), None),
                    SummaryRow::plain("Copy fees", pair(h.fees.copy_fees_u)),
                ],
            }
            p { class: "acct-sub",
                if mock {
                    "Builder revenue is what Pascal's builder code would earn on these hedges; here the mock computes it."
                } else {
                    "Builder revenue as Pascal reports it for our builder code."
                }
            }
        }
        if h.as_of_ms > 0 {
            p { class: "acct-asof", "As of {format::when(h.as_of_ms)} · profile {h.profile}" }
        }
    }
}

/// One hedger position as a summary row: the game, then what is held (and the target when it
/// differs).
fn position_row(p: &Position, card: Option<&Card>) -> SummaryRow {
    let (game, held) = backing(card, &p.symbol, p.held);
    let (_, target) = backing(card, &p.symbol, p.target);
    let value = if p.held == p.target {
        format!("{held}, on target")
    } else {
        format!("{held} (target {target})")
    };
    SummaryRow::plain(game, value)
}
