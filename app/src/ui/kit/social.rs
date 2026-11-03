//! People and proof (7.0a): avatar, badges and pills, stat tiles, the profile header, the team
//! chips, the sentiment bar, the copier row and the Solscan link (V-9).

use dioxus::prelude::*;

use super::icons::{Sym, SymIcon, VerifiedBadge};
use crate::config;

/// An avatar's colour. Every avatar is initials (V-5: no photos).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AvatarTone {
    /// Solid Solana purple, white letters (the copiers sheet's profile "LH", L6's default).
    #[default]
    Purple,
    /// Purple to green, white letters (a tipster's own profile).
    Gradient,
    /// Solid Solana green, dark letters (L6's `green`).
    Green,
    /// The copiers list's tinted discs: letters in the tint over 20% of it.
    PurpleTint,
    GreenTint,
    CyanTint,
    Neutral,
}

impl AvatarTone {
    fn class(self) -> &'static str {
        match self {
            AvatarTone::Purple => "",
            AvatarTone::Gradient => "gradient",
            AvatarTone::Green => "green",
            AvatarTone::PurpleTint => "tint-purple",
            AvatarTone::GreenTint => "tint-green",
            AvatarTone::CyanTint => "tint-cyan",
            AvatarTone::Neutral => "neutral",
        }
    }

    /// A stable tint for a list row, from any key (a wallet), as the copiers list cycles them.
    pub fn tint_for(key: &str) -> AvatarTone {
        const TINTS: [AvatarTone; 4] = [
            AvatarTone::PurpleTint,
            AvatarTone::GreenTint,
            AvatarTone::CyanTint,
            AvatarTone::Neutral,
        ];
        let sum = key
            .bytes()
            .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(u32::from(b)));
        TINTS[(sum % 4) as usize]
    }
}

/// Initials in a circle. `size` in px (default 32; 36 copier rows, 48 the copy page, 64 the
/// profile); `tone` its colour; `verified` pins the green rosette bottom-right (PRO = verified,
/// D-304); `mono` sets the letters as an address's ("7x"). Decorative but for the rosette: the
/// name next to it is read. L6's `large` (52 px) and `green` still work.
#[component]
pub fn Avatar(
    initials: String,
    #[props(default)] size: u32,
    #[props(default)] tone: AvatarTone,
    #[props(default)] verified: bool,
    #[props(default)] mono: bool,
    #[props(default)] large: bool,
    #[props(default)] green: bool,
) -> Element {
    let px = match (size, large) {
        (0, true) => 52,
        (0, false) => 32,
        (s, _) => s,
    };
    let tone = if green { AvatarTone::Green } else { tone };
    let font = (px * 9 / 25).max(10);
    let m = if mono { "addr" } else { "" };
    let badge = (px / 3).clamp(14, 20);
    rsx! {
        span { class: "k-avatar-wrap", style: "width: {px}px; height: {px}px",
            span {
                class: "k-avatar {tone.class()} {m}",
                style: "width: {px}px; height: {px}px; font-size: {font}px",
                "aria-hidden": "true",
                "{initials}"
            }
            if verified {
                span { class: "k-avatar-badge", VerifiedBadge { size: badge } }
            }
        }
    }
}

/// A small pill or tag. Each kind has the mockup's colours and default words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeKind {
    /// Red "● LIVE" (the sportsbook's live card).
    Live,
    /// Green "● LIVE IN-PLAY" (event, copy legs).
    LiveInPlay,
    /// Grey "PREMATCH".
    Prematch,
    /// Purple "3 LEGS"; `Legs(n)`.
    Legs(u32),
    /// Purple "PRO" (verified, D-304).
    Pro,
    /// Green "TOP 1%"; `Top(pct)` (the leaderboard rank).
    Top(u32),
    /// Green on grey "3 PICKS" (the slip sheet's header).
    Picks(u32),
    /// Green "● Live Pool" (the copiers sheet).
    LivePool,
}

impl BadgeKind {
    fn class(self) -> &'static str {
        match self {
            BadgeKind::Live => "live",
            BadgeKind::LiveInPlay => "inplay",
            BadgeKind::Prematch => "prematch",
            BadgeKind::Legs(_) => "legs",
            BadgeKind::Pro => "pro",
            BadgeKind::Top(_) => "top",
            BadgeKind::Picks(_) => "picks",
            BadgeKind::LivePool => "pool",
        }
    }

    fn words(self) -> String {
        match self {
            BadgeKind::Live => "LIVE".into(),
            BadgeKind::LiveInPlay => "LIVE IN-PLAY".into(),
            BadgeKind::Prematch => "PREMATCH".into(),
            BadgeKind::Legs(n) => format!("{n} legs"),
            BadgeKind::Pro => "PRO".into(),
            BadgeKind::Top(p) => format!("TOP {p}%"),
            BadgeKind::Picks(1) => "1 PICK".into(),
            BadgeKind::Picks(n) => format!("{n} picks"),
            BadgeKind::LivePool => "Live Pool".into(),
        }
    }

    fn dot(self) -> bool {
        matches!(
            self,
            BadgeKind::Live | BadgeKind::LiveInPlay | BadgeKind::LivePool
        )
    }
}

/// A badge: `kind` sets colours and words; `label` overrides the words.
#[component]
pub fn Badge(kind: BadgeKind, label: Option<String>) -> Element {
    let text = label.unwrap_or_else(|| kind.words());
    rsx! {
        span { class: "k-badge {kind.class()}",
            if kind.dot() {
                span { class: "k-badge-dot", "aria-hidden": "true" }
            }
            "{text}"
        }
    }
}

/// A stat value's colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatTone {
    #[default]
    Plain,
    /// "+24.8%" (a return, a win).
    Green,
    /// "1,284" (copiers).
    Cyan,
}

/// A stat: caps label over a bold value ("30D ROI" "+24.8%"). Default: the profile's centred
/// tile (put three in a [`StatGrid`]). `large` is the receipt's left-aligned tile with the
/// 20 px value and an optional `sub` line ("STAKE PLACED €25.00 0.184 SOL").
#[component]
pub fn StatTile(
    label: String,
    value: String,
    #[props(default)] tone: StatTone,
    sub: Option<String>,
    #[props(default)] large: bool,
    id: Option<String>,
) -> Element {
    let t = match tone {
        StatTone::Plain => "",
        StatTone::Green => "green",
        StatTone::Cyan => "cyan",
    };
    let l = if large { "lg" } else { "" };
    rsx! {
        div { class: "k-stat {l}", id,
            span { class: "k-stat-label", "{label}" }
            span { class: "k-stat-value {t}", "{value}" }
            if let Some(s) = sub {
                span { class: "k-stat-sub", "{s}" }
            }
        }
    }
}

/// The profile's dark tray of three [`StatTile`]s.
#[component]
pub fn StatGrid(children: Element) -> Element {
    rsx! {
        div { class: "k-stat-grid", {children} }
    }
}

/// Short team codes as overlapping circles ("RM PAO OLY"), coloured in turn green, cyan, red,
/// purple as drawn; a code of three letters or more is set smaller (`long`) so it clears the next
/// circle. Read as "Teams: RM, PAO, OLY".
#[component]
pub fn TeamChips(codes: Vec<String>) -> Element {
    let name = format!("Teams: {}", codes.join(", "));
    rsx! {
        span { class: "k-teamchips", role: "img", aria_label: "{name}",
            for (i, c) in codes.iter().map(|c| chip_code(c)).enumerate() {
                span {
                    key: "{i}",
                    class: if c.chars().count() >= 3 { "k-teamchip long c{i % 4}" } else { "k-teamchip c{i % 4}" },
                    "{c}"
                }
            }
        }
    }
}

/// A chip's code: at most 4 characters ("PHI", "OVR"); a leg the desk has not named yet (its
/// `sha256:` id, D-573) shows "?".
fn chip_code(c: &str) -> String {
    if c.starts_with("sha256:") || c.trim().is_empty() {
        "?".into()
    } else {
        c.chars().take(4).collect()
    }
}

/// The two-sided split (DESIGN.md "market sentiment indicator"): purple for the first side,
/// green for the second, `first_pct` of 100 (clamped). Default is the event card's 12 px track;
/// `thin` the 4 px micro-bar. `aria_label` reads the split ("Fenerbahçe 41%, Panathinaikos 59%").
#[component]
pub fn SentimentBar(first_pct: u32, aria_label: String, #[props(default)] thin: bool) -> Element {
    let a = first_pct.min(100);
    let t = if thin { "thin" } else { "" };
    rsx! {
        div { class: "k-sentiment {t}", role: "img", aria_label: "{aria_label}",
            span { class: "a", style: "width: {a}%" }
            span { class: "b", style: "width: {100 - a}%" }
        }
    }
}

/// How a [`SolscanLink`] looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SolscanStyle {
    /// The profile's chip: verified icon, "Solscan", ↗.
    #[default]
    Chip,
    /// A green text link with ↗ ("Explore on Solscan").
    Inline,
    /// A lone ↗ (a list row's end).
    Icon,
}

/// A link to Solscan on devnet (V-9) for a transaction `sig` or, without it, an `account`. Opens
/// in a new tab; its accessible name says so.
#[component]
pub fn SolscanLink(
    sig: Option<String>,
    account: Option<String>,
    #[props(default)] style: SolscanStyle,
    #[props(default = "Solscan".to_string())] label: String,
    id: Option<String>,
) -> Element {
    let href = match (&sig, &account) {
        (Some(s), _) => config::solscan_tx(s),
        (None, Some(a)) => config::solscan_account(a),
        (None, None) => return rsx! {},
    };
    let what = if sig.is_some() {
        "transaction"
    } else {
        "account"
    };
    let name = format!("View the {what} on Solscan (opens a new tab)");
    let class = match style {
        SolscanStyle::Chip => "k-solscan chip",
        SolscanStyle::Inline => "k-solscan inline",
        SolscanStyle::Icon => "k-solscan icon",
    };
    rsx! {
        a { class, id, href, target: "_blank", rel: "noopener noreferrer", aria_label: "{name}", title: "{name}",
            if style == SolscanStyle::Chip {
                SymIcon { sym: Sym::Verified, size: 13 }
            }
            if style != SolscanStyle::Icon {
                span { "{label}" }
            }
            SymIcon { sym: Sym::OpenInNew, size: if style == SolscanStyle::Icon { 16 } else { 12 } }
        }
    }
}

/// One copier (guidelines §5): avatar, name or short address, euros staked, ↗. The whole row
/// opens the copy's transaction on Solscan. `mono` marks a short address ("7xK9…94Lp"): its
/// avatar letters and name keep their case and get a little tracking.
#[component]
pub fn CopierRow(
    initials: String,
    name: String,
    stake: String,
    sig: String,
    #[props(default)] tone: AvatarTone,
    #[props(default)] mono: bool,
) -> Element {
    let href = config::solscan_tx(&sig);
    let m = if mono { "addr" } else { "" };
    let label = format!("{name}, {stake}: view the copy on Solscan (opens a new tab)");
    rsx! {
        a { class: "k-copier", href, target: "_blank", rel: "noopener noreferrer", aria_label: "{label}",
            span { class: "k-copier-who",
                Avatar { initials, size: 36, tone, mono }
                span { class: "k-copier-name {m}", "{name}" }
            }
            span { class: "k-copier-end",
                span { class: "k-copier-stake", "{stake}" }
                SymIcon { sym: Sym::OpenInNew, size: 16 }
            }
        }
    }
}

/// A tipster's social link (V-5), a 32 px round button. `kind` sets the icon and colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Social {
    X,
    YouTube,
    Telegram,
    Instagram,
}

impl Social {
    pub fn name(self) -> &'static str {
        match self {
            Social::X => "X",
            Social::YouTube => "YouTube",
            Social::Telegram => "Telegram",
            Social::Instagram => "Instagram",
        }
    }

    /// The profile URL for a stored handle (no scheme or host accepted from the user).
    pub fn url(self, handle: &str) -> String {
        let h: String = handle
            .trim_start_matches('@')
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
            .collect();
        match self {
            Social::X => format!("https://x.com/{h}"),
            Social::YouTube => format!("https://www.youtube.com/@{h}"),
            Social::Telegram => format!("https://t.me/{h}"),
            Social::Instagram => format!("https://www.instagram.com/{h}"),
        }
    }
}

/// One social link on the profile header.
#[component]
pub fn SocialLink(kind: Social, handle: String) -> Element {
    let href = kind.url(&handle);
    let name = format!("{} (opens a new tab)", kind.name());
    let class = match kind {
        Social::X => "k-social x",
        Social::YouTube => "k-social yt",
        Social::Telegram => "k-social tg",
        Social::Instagram => "k-social ig",
    };
    rsx! {
        a { class, href, target: "_blank", rel: "noopener noreferrer", aria_label: "{name}", title: "{name}",
            match kind {
                Social::X => rsx! { span { class: "k-social-x", "aria-hidden": "true", "𝕏" } },
                Social::YouTube => rsx! { SymIcon { sym: Sym::LiveTv, size: 16 } },
                Social::Telegram => rsx! { SymIcon { sym: Sym::Send, size: 16 } },
                Social::Instagram => rsx! { SymIcon { sym: Sym::PhotoCamera, size: 16 } },
            }
        }
    }
}

/// "＋ Follow" / "Following": the profile's toggle (V-5).
#[component]
pub fn FollowButton(
    following: bool,
    #[props(default)] disabled: bool,
    onclick: EventHandler<MouseEvent>,
) -> Element {
    rsx! {
        button {
            r#type: "button",
            class: "k-follow",
            id: "follow",
            aria_pressed: "{following}",
            disabled,
            onclick: move |e| onclick.call(e),
            SymIcon { sym: if following { Sym::Check } else { Sym::PersonAdd }, size: 15 }
            span { if following { "Following" } else { "Follow" } }
        }
    }
}

/// A tipster's header card (mockup `betfi_tipster_profile_one_tap_copy`): the avatar (64 px,
/// verified rosette), the name with PRO, the @handle, `follow` on the right, then `stats` (a
/// [`StatGrid`]) and `links` (social links and the Solscan chip). Purple and green glows in the
/// corners, as drawn.
#[component]
pub fn ProfileHeader(
    name: String,
    handle: String,
    initials: String,
    #[props(default)] verified: bool,
    follow: Option<Element>,
    stats: Element,
    links: Option<Element>,
    id: Option<String>,
) -> Element {
    rsx! {
        section { class: "k-profile", id, aria_label: "{name}",
            span { class: "k-profile-glow a", "aria-hidden": "true" }
            span { class: "k-profile-glow b", "aria-hidden": "true" }
            div { class: "k-profile-top",
                div { class: "k-profile-who",
                    Avatar { initials, size: 64, tone: AvatarTone::Gradient, verified }
                    div { class: "k-profile-names",
                        div { class: "k-profile-name-row",
                            h1 { class: "k-profile-name", "{name}" }
                            if verified {
                                Badge { kind: BadgeKind::Pro }
                            }
                        }
                        span { class: "k-profile-handle", "@{handle}" }
                    }
                }
                {follow}
            }
            {stats}
            if let Some(l) = links {
                div { class: "k-profile-links", {l} }
            }
        }
    }
}
