//! A tipster's page (`/tipsters/:wallet`, L9, user request 1 Nov):
//! - the header card: initials avatar with the verified rosette, the handle with PRO, Follow /
//!   Following ([`super::follow`]), 30D ROI / WIN RATE (all time) / COPIERS, the social links that
//!   exist and "Solscan ↗" for the wallet;
//! - "ACTIVE PARLAYS (n)": the tipster's open tips only, each with its title (the tipster's,
//!   V-6), how many copied it, the overall stake on it (the tipster's and every copy's) and at
//!   most three team logos; a tap opens the parlay (`/copy/:id`, [`super::parlay`]), where it is
//!   copied.
//!
//! On their own page a tipster also sets a handle and names an open tip.

use dioxus::prelude::*;

use super::follow::{self, followers_words};
use super::{
    HandleSetup, ParlayLogos, TitleSetup, count, is_titleable, me, parlay_teams, tip_title, up_down,
};
use crate::Route;
use crate::desk::DeskError;
use crate::desk::types::{PublicProfile, Ticket, TipsterProfile, TipsterRow};
use crate::format;
use crate::state::Load;
use crate::ui::kit::{
    Avatar, AvatarTone, Badge, BadgeKind, FollowButton, LiveDot, SectionTitle, Skeleton,
    SkeletonKind, Social, SocialLink, SolscanLink, StatGrid, StatTile, StatTone, money,
};
use crate::ui::{use_app_state, use_desk};
use crate::wallet::use_wallet_api;

#[component]
pub fn TipsterPage(wallet: String) -> Element {
    let desk = use_desk();
    let state = use_app_state();
    let api = use_wallet_api();
    let own = me(&api).as_deref() == Some(wallet.as_str());
    let (w1, w2) = (wallet.clone(), wallet.clone());
    let d2 = desk.clone();
    let mut tipster = use_resource(use_reactive!(|(w1,)| {
        let desk = desk.clone();
        async move { desk.tipster(&w1).await }
    }));
    let mut public = use_resource(use_reactive!(|(w2,)| {
        let desk = d2.clone();
        async move { desk.public_profile(&w2).await.ok() }
    }));
    let card = match &state.read().card {
        Load::Ready(c) => Some(c.clone()),
        _ => None,
    };
    let public_now = public.read().clone().flatten();
    rsx! {
        section { class: "screen-tipster", id: "tipster",
            match &*tipster.read() {
                None => rsx! {
                    Skeleton { kind: SkeletonKind::Profile, count: 1, label: "Loading the tipster…" }
                    Skeleton { kind: SkeletonKind::Row, count: 3, label: "Loading parlays…" }
                },
                Some(Err(DeskError::Http(404))) => rsx! {
                    ProfileCard { wallet: wallet.clone(), row: None, public: public_now.clone(), own, onfollowed: move |_| public.restart() }
                    p { class: "tp-status", "This wallet has no tips on record yet." }
                    if own {
                        HandleSetup { wallet: wallet.clone() }
                    }
                },
                Some(Err(e)) => rsx! { p { class: "tp-status error", role: "alert", "{e.plain(card.as_ref())}" } },
                Some(Ok(p)) => rsx! {
                    ProfileCard { wallet: wallet.clone(), row: Some(p.tipster.clone()), public: public_now.clone(), own, onfollowed: move |_| public.restart() }
                    Parlays { p: p.clone(), own, handle: p.tipster.handle.clone(), onsaved: move |_| tipster.restart() }
                    if own {
                        HandleSetup { wallet: wallet.clone() }
                    }
                },
            }
        }
    }
}

// ---------------------------------------------------------------- the header card

/// The header card. `row` is the tipster's record (none for a wallet without own tips);
/// `public` its profile (handle, links, followers, verified).
#[component]
fn ProfileCard(
    wallet: String,
    row: Option<TipsterRow>,
    public: Option<PublicProfile>,
    own: bool,
    onfollowed: EventHandler<()>,
) -> Element {
    let desk = use_desk();
    let api = use_wallet_api();
    let viewer = me(&api);
    let handle = row
        .as_ref()
        .and_then(|r| r.handle.clone())
        .or_else(|| public.as_ref().and_then(|p| p.handle.clone()))
        .filter(|h| !h.is_empty());
    let verified =
        row.as_ref().is_some_and(|r| r.verified) || public.as_ref().is_some_and(|p| p.verified);
    let who = super::Who::new(handle.clone(), &wallet);
    // The handle is the name (the desk has no display name, D-511); a wallet without one shows
    // its short address.
    let name = handle.clone().unwrap_or_else(|| format::short(&wallet));

    // Follow state: this session's last answer, else what this browser remembers.
    let mut following = use_signal(|| None::<bool>);
    let mut busy = use_signal(|| false);
    let mut note = use_signal(|| None::<String>);
    let remembered = viewer
        .as_deref()
        .is_some_and(|m| follow::is_following(m, &wallet));
    let is_following = following().unwrap_or(remembered);
    let followers = public.as_ref().and_then(|p| p.followers);
    let target = wallet.clone();
    let toggle = move |_| {
        let (desk, api, target) = (desk.clone(), api, target.clone());
        let on = !is_following;
        async move {
            busy.set(true);
            note.set(None);
            match follow::set_follow(desk, api, target, on).await {
                Ok(_) => {
                    following.set(Some(on));
                    onfollowed.call(());
                }
                Err(e) => note.set(Some(e)),
            }
            busy.set(false);
        }
    };

    let (roi, roi_tone, roi_down) = match &row {
        Some(r) => (
            format::pct_signed(r.roi_30d_ppm),
            if up_down(r.roi_30d_ppm) == "k-up" {
                StatTone::Green
            } else {
                StatTone::Plain
            },
            up_down(r.roi_30d_ppm) == "k-down",
        ),
        None => ("–".to_string(), StatTone::Plain, false),
    };
    let win = row
        .as_ref()
        .map(|r| format::pct_ppm(r.hit_rate_ppm))
        .unwrap_or_else(|| "–".into());
    let copiers_n = row
        .as_ref()
        .map(|r| count(r.copiers))
        .unwrap_or_else(|| "0".into());
    let links = public
        .as_ref()
        .and_then(|p| p.links.clone())
        .unwrap_or_default();
    let socials: Vec<(Social, String)> = [
        (Social::X, links.x),
        (Social::YouTube, links.youtube),
        (Social::Telegram, links.telegram),
        (Social::Instagram, links.instagram),
    ]
    .into_iter()
    .filter_map(|(k, h)| {
        h.filter(|h| {
            h.trim_start_matches('@')
                .bytes()
                .any(|b| b.is_ascii_alphanumeric())
        })
        .map(|h| (k, h))
    })
    .collect();
    let class = if roi_down {
        "k-profile tp-profile roi-down"
    } else {
        "k-profile tp-profile"
    };
    rsx! {
        section { class, id: "tipster-head", aria_label: "{name}",
            span { class: "k-profile-glow a", "aria-hidden": "true" }
            span { class: "k-profile-glow b", "aria-hidden": "true" }
            div { class: "k-profile-top",
                div { class: "k-profile-who",
                    Avatar { initials: who.initials.clone(), size: 64, tone: AvatarTone::Gradient, verified }
                    div { class: "k-profile-names",
                        div { class: "k-profile-name-row",
                            h1 { class: "k-profile-name", id: "tipster-name", "{name}" }
                            if verified {
                                Badge { kind: BadgeKind::Pro }
                            }
                        }
                        if let Some(h) = handle.as_ref() {
                            span { class: "k-profile-handle", "@{h}" }
                        }
                    }
                }
                div { class: "tp-follow-col",
                    if !own {
                        FollowButton { following: is_following, disabled: busy(), onclick: toggle }
                    }
                    if let Some(n) = followers {
                        span { class: "tp-followers", id: "tipster-followers", "{followers_words(n)}" }
                    }
                }
            }
            StatGrid {
                StatTile { label: "30D ROI", value: roi, tone: roi_tone, id: "tp-roi" }
                StatTile { label: "Win rate", value: win, id: "tp-win" }
                StatTile { label: "Copiers", value: copiers_n, tone: StatTone::Cyan, id: "tp-copiers" }
            }
            div { class: "k-profile-links",
                span { class: "tp-socials", id: "tipster-links",
                    for (k, h) in socials {
                        SocialLink { key: "{k.name()}", kind: k, handle: h }
                    }
                }
                SolscanLink { account: wallet.clone(), id: "tipster-solscan" }
            }
            if let Some(n) = note() {
                p { class: "tp-follow-note", role: "alert", id: "follow-note", "{n}" }
            }
        }
    }
}

// ---------------------------------------------------------------- active parlays

#[component]
fn Parlays(
    p: TipsterProfile,
    own: bool,
    handle: Option<String>,
    onsaved: EventHandler<()>,
) -> Element {
    rsx! {
        SectionTitle {
            title: format!("Active parlays ({})", p.open.len()),
            small: true,
            id: "parlays-title",
            end: rsx! { LiveDot { label: "Live on Solana" } },
        }
        if p.open.is_empty() {
            p { class: "tp-status", id: "open-tips-none", "No open parlays right now." }
        } else {
            ol { class: "tp-parlays", id: "open-tips",
                for t in p.open.iter() {
                    li { key: "{t.ticket}", class: "tp-parlay-item", "data-ticket": "{t.ticket}",
                        ParlayRow { t: t.clone() }
                        if own && t.title.is_none() && is_titleable(t) {
                            TitleSetup { t: t.clone(), handle: handle.clone(), onsaved: move |_| onsaved.call(()) }
                        }
                    }
                }
            }
        }
    }
}

/// One open parlay: its title, the legs and copies, the overall stake (the tipster's and the
/// copies'), at most three team logos and the total it was placed at; the row opens it.
#[component]
fn ParlayRow(t: Ticket) -> Element {
    let rate = money::use_fx();
    let copies = t.copies.unwrap_or(0);
    let staked_u = t.stake_u.saturating_add(t.pool_u.unwrap_or(0));
    let staked = match rate {
        Some(r) => money::eur_compact(money::eur_u(staked_u, r)),
        None => money::usdc_text(staked_u),
    };
    let title = tip_title(&t);
    let copies_words = match copies {
        1 => "1 copy".to_string(),
        n => format!("{} copies", count(n)),
    };
    rsx! {
        Link { to: Route::CopyTip { id: t.ticket.clone() }, class: "tp-parlay", id: "tip-{t.ticket}",
            aria_label: "{title}: {format::picks(t.n)}, {copies_words}, {staked} staked, total {format::odds(t.odds_offered_u)}",
            span { class: "tp-parlay-title", "{title}" }
            span { class: "tp-parlay-total", "{format::odds(t.odds_offered_u)}" }
            span { class: "tp-parlay-sub",
                ParlayLogos { teams: parlay_teams(&t) }
                span { class: "tp-parlay-meta",
                    span { class: "tp-parlay-copies", "{copies_words}" }
                    span { class: "sep", "aria-hidden": "true", "\u{2022}" }
                    span { class: "tp-parlay-staked", "{staked} staked" }
                }
            }
        }
    }
}
