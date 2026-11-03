//! Tipsters (L9, user request 1 Nov): the list of tipsters (`/tipsters`, [`board`]: 30D ROI and
//! win rate), a tipster's page (`/tipsters/:wallet`, [`profile`]: socials and open parlays), a
//! parlay (`/copy/:id`, [`parlay`]: its games locked, "Copied by", Copy Schedina through the bet
//! drawer's copy mode) with its locked games (`/copy/:id/event/:game_id`) and its copiers sheet
//! ([`copiers`]: each copy's stake and its transaction on Solscan). Every number is the desk's
//! (`GET /tipsters`, `/tipsters/{wallet}`, `/tipsters/{wallet}/copiers`, `/ticket/{id}`,
//! `/profile/{wallet}`); the app only words them. Open tips never count in the score (spec 6).
//!
//! Writes are wallet-signed `PUT /profile` messages ([`profile_msg`]), never transactions:
//! - a handle ([`HandleSetup`], on the tipster's own page) and an own open tip's title
//!   ([`TitleSetup`]) sign `betfi/profile/v1`;
//! - Follow / Following ([`follow`]) signs `betfi/profile/v2` with `follow` (V-5, D-471).

pub mod board;
pub mod copiers;
pub mod follow;
pub mod parlay;
pub mod profile;
pub mod profile_msg;

pub use board::Tipsters;
pub use parlay::{CopyEvent, CopyTip};
pub use profile::TipsterPage;

use dioxus::prelude::*;

use super::kit::{self, Chevron, TextLink};
use super::use_desk;
use crate::desk::types::{MarketKind, Pick, ProfileUpdate, Side, Ticket, TicketLeg, TicketState};
use crate::desk::{Desk, ProfileWriteError};
use crate::format::{self, MICRO};
use crate::wallet::{Wallet, use_wallet_api};

// ---------------------------------------------------------------- shared words

/// How a tipster is shown: `@handle` (or a short wallet), a first name for sentences
/// ("Lorenzo bet €500…"), avatar initials and colour (fixed per wallet).
#[derive(Debug, Clone, PartialEq)]
pub struct Who {
    pub name: String,
    pub first: String,
    pub initials: String,
    pub green: bool,
}

impl Who {
    pub fn new(handle: Option<String>, wallet: &str) -> Self {
        // Purple or green, the same for a wallet on every screen.
        let green = wallet.bytes().map(u32::from).sum::<u32>() % 2 == 0;
        match handle.filter(|h| !h.is_empty()) {
            Some(h) => {
                let parts: Vec<&str> = h.split('_').filter(|p| !p.is_empty()).collect();
                let initials: String = match parts.as_slice() {
                    [a, b, ..] => a.chars().take(1).chain(b.chars().take(1)).collect(),
                    _ => h.chars().filter(|c| *c != '_').take(2).collect(),
                };
                let head = parts.first().copied().unwrap_or(&h);
                let first = if head.len() >= 2 && head.chars().any(|c| c.is_ascii_alphabetic()) {
                    let mut c = head.chars();
                    c.next()
                        .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                        .unwrap_or_default()
                } else {
                    format!("@{h}")
                };
                Who {
                    name: format!("@{h}"),
                    first,
                    initials: initials.to_ascii_uppercase(),
                    green,
                }
            }
            None => Who {
                name: format::short(wallet),
                first: format::short(wallet),
                initials: wallet
                    .chars()
                    .take(2)
                    .collect::<String>()
                    .to_ascii_uppercase(),
                green,
            },
        }
    }
}

/// `1,284` for a count.
pub fn count(n: u32) -> String {
    let text = format::eur(u64::from(n) * MICRO);
    text.trim_start_matches('€')
        .trim_end_matches(".00")
        .to_string()
}

/// A pick's two lines: what was picked ("Olympiacos +4.5", "Monaco to win", "Partizan–Virtus
/// over 158.5") and where and when ("vs Real Madrid · 20:45", "at Baskonia · 20:30", a total's
/// "20:30").
pub fn leg_line(leg: &TicketLeg) -> (String, String) {
    if leg.is_unnamed() && leg.label.as_deref().is_none_or(|l| l.trim().is_empty()) {
        return (
            format!("Pick {}", u32::from(leg.idx) + 1),
            "Game not named yet".into(),
        );
    }
    let total = leg.kind == Some(MarketKind::Total);
    let name = match (&leg.label, total) {
        (Some(l), _) if !l.trim().is_empty() => l.clone(),
        (_, true) => format!(
            "{}\u{2013}{} {} {}",
            leg.away,
            leg.home,
            if leg.side == Side::Long {
                "over"
            } else {
                "under"
            },
            leg.line.clone().unwrap_or_default()
        ),
        _ => format!("{} to win", leg.picked_team()),
    };
    let at = format::clock(leg.start_ms);
    let sub = if total {
        at
    } else {
        match leg.pick {
            Pick::Away => format!("at {} · {at}", leg.home),
            Pick::Home => format!("vs {} · {at}", leg.away),
        }
    };
    (name, sub)
}

/// A tip's name: its title, else "<n>-leg tip" (V-6).
pub fn tip_title(t: &Ticket) -> String {
    t.title
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("{}-leg tip", t.n))
}

/// The teams a parlay is on, as its preview shows them: each leg's picked team (a total's or a
/// draw's home team) with the desk's short code for it when it has one (`teams`, leg order), in
/// leg order, without repeats.
pub fn parlay_teams(t: &Ticket) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    for (i, l) in t.legs.iter().enumerate() {
        let draw = l
            .label
            .as_deref()
            .is_some_and(|x| x.eq_ignore_ascii_case("draw"));
        let (team, code) = if l.kind == Some(MarketKind::Total) || draw {
            (l.home.clone(), None)
        } else {
            let code = t.teams.get(i).cloned();
            (l.picked_team().to_string(), code)
        };
        if !team.is_empty() && !out.iter().any(|(n, _)| *n == team) {
            out.push((team, code.filter(|c| !c.trim().is_empty())));
        }
    }
    out
}

/// A team's short code when the desk gave none: the first three letters of a one-word name, else
/// the initials of its first three words ("Fenerbahçe Beko" → "FB").
fn team_code(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let code: String = match words.as_slice() {
        [one] => one.chars().take(3).collect(),
        many => many
            .iter()
            .filter_map(|w| w.chars().next())
            .take(3)
            .collect(),
    };
    code.to_uppercase()
}

/// At most three team logos of a parlay (a team without one shows its code), and "+n" for the
/// rest (L9).
#[component]
pub fn ParlayLogos(teams: Vec<(String, Option<String>)>, #[props(default)] large: bool) -> Element {
    let size = if large { "32" } else { "24" };
    let more = teams.len().saturating_sub(3);
    rsx! {
        span { class: if large { "tp-logos large" } else { "tp-logos" }, "aria-hidden": "true",
            for (name, code) in teams.iter().take(3) {
                match kit::team_logo(name, false).or_else(|| kit::team_logo(name, true)) {
                    Some(src) => rsx! {
                        span { key: "{name}", class: "tp-logo",
                            img { src: "{src}", alt: "", decoding: "async", width: size, height: size }
                        }
                    },
                    None => rsx! {
                        span { key: "{name}", class: "tp-logo code",
                            {code.clone().unwrap_or_else(|| team_code(name))}
                        }
                    },
                }
            }
            if more > 0 {
                span { class: "tp-logo more", "+{more}" }
            }
        }
    }
}

/// Green for a gain, red for a loss, plain for what [`format::pct_signed`] shows as "0%".
pub fn up_down(ppm: i64) -> &'static str {
    match (ppm / 1_000).signum() {
        1 => "k-up",
        -1 => "k-down",
        _ => "",
    }
}

/// The connected wallet, base58.
fn me(wallet: &Wallet) -> Option<String> {
    wallet.public_key().map(|k| k.to_string())
}

/// Re-renders every 10 s, so "starts in under a minute" and the copy CTA follow the clock.
fn use_clock_tick() {
    let mut tick = use_signal(|| 0u32);
    use_future(move || async move {
        loop {
            gloo_timers::future::sleep(std::time::Duration::from_secs(10)).await;
            tick.with_mut(|t| *t = t.wrapping_add(1));
        }
    });
    let _ = tick();
}

// ---------------------------------------------------------------- handle and title (L6-2)

/// Sign the v1 profile message with the connected wallet (which must be `owner`) and send it.
async fn write_profile(
    desk: Desk,
    wallet: Wallet,
    owner: String,
    handle: String,
    ticket: Option<String>,
    title: Option<String>,
) -> Result<crate::desk::types::Profile, String> {
    if desk.is_demo() {
        return Err(profile_words(&ProfileWriteError::Demo));
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
    let msg = profile_msg::message(&owner, &handle, ticket.as_deref(), title.as_deref(), ts);
    let sig = wallet
        .sign_message(msg.as_bytes())
        .await
        .map_err(|e| format!("Not saved: {e}."))?;
    let body = ProfileUpdate {
        wallet: owner,
        handle,
        title,
        ticket,
        ts,
        sig: profile_msg::base64(&sig),
    };
    desk.put_profile(&body).await.map_err(|e| profile_words(&e))
}

/// The desk's refusals in plain words (D-336: 409 and 429 with a reason).
pub fn profile_words(e: &ProfileWriteError) -> String {
    match e {
        ProfileWriteError::Demo => {
            "Demo data: there is no desk to save to. Run the app against a desk to set it.".into()
        }
        ProfileWriteError::Refused { status: 429, .. } => {
            "Too many changes in a short time. Wait a minute and try again.".into()
        }
        ProfileWriteError::Refused { reason, .. } => {
            let r = reason.as_str();
            if r.contains("is taken") {
                "That handle is taken. Choose another.".into()
            } else if r.contains("less than 24 h") {
                let next = r
                    .split("next change from ")
                    .nth(1)
                    .and_then(|s| s.split_whitespace().next())
                    .and_then(|s| s.parse::<i64>().ok());
                match next {
                    Some(s) => format!(
                        "A handle can change once a day. Next change from {}.",
                        format::when(s * 1000)
                    ),
                    None => "A handle can change once a day. Try again tomorrow.".into(),
                }
            } else if r.contains("given up") {
                "That handle was given up recently: it stays with its previous owner for 30 days. Choose another.".into()
            } else if r.contains("reserved") {
                "That handle holds a reserved word. Choose another.".into()
            } else if r.contains("already used") {
                "That signature was already used. Try again.".into()
            } else if r.contains("already has a title") {
                "This tip already has a name: a tip is named once.".into()
            } else if r.contains("while the tip is open") {
                "Too late: a tip is named while it is open.".into()
            } else if r.contains("from the desk's clock") {
                "This device's clock is more than 5 minutes off. Set it right and try again.".into()
            } else if r.contains("not yours") || r.contains("not a tip of yours") {
                "Only the tipster who shared this tip can name it.".into()
            } else if r.contains("signature does not match") {
                "The wallet's signature did not match. Try again.".into()
            } else if r.contains("follows at most") {
                "You follow the most tipsters a wallet can. Unfollow one first.".into()
            } else if r.contains("cannot follow itself") {
                "You cannot follow yourself.".into()
            } else if r.is_empty() {
                "Not saved: the desk refused it.".into()
            } else {
                format!("Not saved: {}.", crate::desk::plain_reason(r, None))
            }
        }
        ProfileWriteError::Desk(d) => d.plain(None),
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Saving {
    Idle,
    Busy,
    Done(String),
    Failed(String),
}

/// "Choose a handle" for `wallet` (the connected one): the current handle, a field, and Save,
/// which signs `betfi/profile/v1` and sends `PUT /profile`. On a tipster's own page.
#[component]
pub fn HandleSetup(wallet: String) -> Element {
    let desk = use_desk();
    let api = use_wallet_api();
    let w = wallet.clone();
    let read_desk = desk.clone();
    let mut current = use_resource(use_reactive!(|(w,)| {
        let desk = read_desk.clone();
        async move { desk.profile(&w).await.ok() }
    }));
    let mut text = use_signal(String::new);
    let mut saving = use_signal(|| Saving::Idle);
    let now_s = (js_sys::Date::now() / 1000.0) as i64;
    let profile = current.read().clone().flatten();
    let handle = profile.as_ref().and_then(|p| p.handle.clone());
    let since = profile.as_ref().map(|p| p.since).unwrap_or(0);
    let typed = text().trim().to_string();
    let problem = (!typed.is_empty())
        .then(|| profile_msg::handle_problem(&typed))
        .flatten();
    let too_soon = handle.is_some() && since > 0 && now_s < since + 86_400;
    let can_save = !typed.is_empty()
        && problem.is_none()
        && handle.as_deref() != Some(typed.as_str())
        && saving() != Saving::Busy;
    let save = move |_| {
        let desk = desk.clone();
        let owner = wallet.clone();
        let handle = text().trim().to_string();
        async move {
            saving.set(Saving::Busy);
            match write_profile(desk, api, owner, handle, None, None).await {
                Ok(p) => {
                    saving.set(Saving::Done(format!(
                        "Saved: you are @{}.",
                        p.handle.unwrap_or_default()
                    )));
                    text.set(String::new());
                    current.restart();
                }
                Err(e) => saving.set(Saving::Failed(e)),
            }
        }
    };
    rsx! {
        section { class: "tp-setup", id: "handle-setup", aria_label: "Your handle",
            div { class: "tp-setup-head",
                h2 { if handle.is_some() { "Your handle" } else { "Choose a handle" } }
                if let Some(h) = handle.as_ref() {
                    span { class: "tp-setup-current", id: "handle-current", "@{h}" }
                }
            }
            p { class: "tp-setup-note",
                "3 to 20 lower-case letters, digits or _; once a day. Your wallet signs it: no transaction, nothing to pay."
            }
            if too_soon {
                p { class: "tp-setup-note", "Next change from {format::when((since + 86_400) * 1000)}." }
            }
            div { class: "tp-field",
                label { class: "tp-input",
                    span { class: "tp-input-at", "aria-hidden": "true", "@" }
                    input {
                        r#type: "text",
                        id: "handle-input",
                        aria_label: "Handle",
                        autocomplete: "off",
                        autocapitalize: "none",
                        spellcheck: "false",
                        maxlength: "20",
                        placeholder: "your_handle",
                        aria_invalid: "{problem.is_some()}",
                        value: "{text}",
                        oninput: move |e| {
                            text.set(e.value());
                            if !matches!(saving(), Saving::Busy) {
                                saving.set(Saving::Idle);
                            }
                        },
                    }
                }
                button { r#type: "button", class: "tp-save", id: "handle-save", disabled: !can_save, onclick: save,
                    if saving() == Saving::Busy { "Signing…" } else { "Save" }
                }
            }
            if let Some(p) = problem.as_ref() {
                p { class: "tp-setup-msg error", role: "alert", "Not a handle: {p}." }
            }
            match saving() {
                Saving::Done(m) => rsx! { p { class: "tp-setup-msg ok", role: "status", id: "handle-saved", "{m}" } },
                Saving::Failed(m) => rsx! { p { class: "tp-setup-msg error", role: "alert", id: "handle-error", "{m}" } },
                _ => rsx! {},
            }
        }
    }
}

/// "Name this tip" under an own open tip: a title of up to 40
/// characters, set once, signed with the current handle (the desk needs one).
#[component]
fn TitleSetup(t: Ticket, handle: Option<String>, onsaved: EventHandler<()>) -> Element {
    let desk = use_desk();
    let api = use_wallet_api();
    let mut text = use_signal(String::new);
    let mut saving = use_signal(|| Saving::Idle);
    let mut open = use_signal(|| false);
    let typed = text();
    let cleaned = profile_msg::clean_title(&typed);
    let input_id = format!("title-input-{}", t.ticket);
    let Some(h) = handle else {
        return rsx! {
            p { class: "tp-setup-msg", "Choose a handle below to name this tip." }
        };
    };
    if !open() {
        return rsx! {
            TextLink { label: "Name this tip", accent: true, chevron: Chevron::None, id: "name-tip", onclick: move |_| open.set(true) }
        };
    }
    let can_save = cleaned.is_ok() && saving() != Saving::Busy;
    let owner = t.user.clone();
    let ticket = t.ticket.clone();
    let save = move |_| {
        let desk = desk.clone();
        let owner = owner.clone();
        let ticket = ticket.clone();
        let handle = h.clone();
        async move {
            let Ok(title) = profile_msg::clean_title(&text()) else {
                return;
            };
            saving.set(Saving::Busy);
            match write_profile(desk, api, owner, handle, Some(ticket), Some(title)).await {
                Ok(_) => {
                    saving.set(Saving::Done("Named.".into()));
                    onsaved.call(());
                }
                Err(e) => saving.set(Saving::Failed(e)),
            }
        }
    };
    rsx! {
        div { class: "tp-title-setup",
            label { class: "tp-setup-label", r#for: "{input_id}", "Name this tip (once)" }
            div { class: "tp-field",
                label { class: "tp-input",
                    input {
                        r#type: "text",
                        id: "{input_id}",
                        aria_label: "Name of this tip",
                        autocomplete: "off",
                        maxlength: "40",
                        placeholder: "Thursday treble",
                        value: "{text}",
                        oninput: move |e| text.set(e.value()),
                    }
                }
                button { r#type: "button", class: "tp-save", disabled: !can_save, onclick: save,
                    if saving() == Saving::Busy { "Signing…" } else { "Save" }
                }
            }
            if let (Err(why), false) = (&cleaned, typed.is_empty()) {
                p { class: "tp-setup-msg error", role: "alert", "Not a name: {why}." }
            }
            match saving() {
                Saving::Done(m) => rsx! { p { class: "tp-setup-msg ok", role: "status", "{m}" } },
                Saving::Failed(m) => rsx! { p { class: "tp-setup-msg error", role: "alert", "{m}" } },
                _ => rsx! {},
            }
        }
    }
}

/// An own tip can still be named: pending or open, its games in play included (D-660; was
/// "before its first game", D-337).
fn is_titleable(t: &Ticket) -> bool {
    matches!(t.state, TicketState::Pending | TicketState::Open)
}
