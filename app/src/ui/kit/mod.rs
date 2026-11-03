//! The BetFi component kit, v2 (7.0a): the BetFi C2 mockup's controls as Dioxus components.
//! Styles live in `assets/main.css` (classes `k-*`, tokens on `:root`); `/dev/kit` (dev tools)
//! shows each one in each state next to the mockup's own content.
//!
//! **v2 components** (L7-1 builds every screen from these):
//! - chrome: [`AppBar`], [`LiveTicker`] ([`TickerLine`]), [`BottomNav`] ([`NavTab`]),
//!   [`SubHeader`], [`SlipPill`];
//! - sportsbook: [`SportTabs`] ([`Sport`]), [`MatchCard`] ([`TeamLine`]), [`OutcomeButton`]
//!   ([`OutcomeTone`], [`outcome_tones`]), [`ActionLink`], [`SectionTitle`], [`LiveDot`];
//! - picks and tips: [`PickRow`] (drawer), [`LegCard`] (copy), [`LegRow`] (receipt),
//!   [`OutcomeCard`] (event), [`TipCard`] (profile);
//! - drawers: [`BottomSheet`]; stake: [`StakeAmount`] ([`Currency`]);
//! - actions and outcomes: [`PurpleCTA`], [`SecondaryButton`], [`SummaryRows`] ([`SummaryRow`],
//!   [`RowKind`]), [`CelebrationHero`] ([`HeroIcon`]), [`TxBadge`], [`PayoutHero`];
//! - people and proof: [`Avatar`] ([`AvatarTone`]), [`Badge`] ([`BadgeKind`]), [`StatTile`]
//!   ([`StatTone`]), [`StatGrid`], [`ProfileHeader`], [`FollowButton`], [`SocialLink`]
//!   ([`Social`]), [`TeamChips`], [`SentimentBar`], [`CopierRow`], [`SolscanLink`]
//!   ([`SolscanStyle`]);
//! - icons: [`SymIcon`] ([`Sym`], Material Symbols as in the mockup), [`XLogo`]; money:
//!   [`money`] (€, ≈ SOL).
//!
//! The L6 components (`Button`, `OddsButton`, `Panel`, `ListRow`, …) stay for the screens L7-1
//! has not redesigned yet, restyled through the remapped tokens; new screens should not use them.
//!
//! Conventions: text props take ready-formatted strings (the screens own the numbers, through
//! `crate::format` and [`money`]); every figure is set in Plus Jakarta Sans with tabular
//! figures (global `tnum`); icon-only controls take an accessible name; never the word "odds"
//! in visible text (guidelines §2); links out (Solscan, socials) open a new tab with
//! `rel="noopener noreferrer"`.

mod buttons;
mod chrome;
mod cta;
mod gallery;
mod icons;
mod inputs;
mod layout;
mod legs;
mod logos;
mod market;
pub mod money;
pub use logos::team_logo;
mod sequencer;
mod sheet;
mod skeleton;
mod social;
mod stake;
mod status;

pub use buttons::{
    Button, ButtonVariant, Chevron, IconButton, IconSize, LinkButton, TextLink, note_screen_shown,
};
pub use chrome::{Action, AppBar, BottomNav, LiveTicker, NavTab, SlipPill, SubHeader, TickerLine};
pub use cta::{
    CelebrationHero, HeroIcon, PayoutHero, PurpleCTA, RowKind, SecondaryButton, SummaryRow,
    SummaryRows, TxBadge, reduced_motion,
};
pub use gallery::{KitGallery, KitShell};
pub use icons::{Icon, Sym, SymIcon, XLogo};
pub use inputs::Toggle;
pub use layout::Panel;
pub use legs::{LegCard, LegRow, OutcomeCard, PickRow, TipCard};
pub use market::{
    ActionLink, LiveDot, MatchCard, OutcomeButton, OutcomeTone, SectionTitle, Sport, SportTabs,
    TeamLine, outcome_tones,
};
pub use sequencer::{SeqStep, Sequencer, StepState};
pub use sheet::BottomSheet;
pub use skeleton::{Skeleton, SkeletonKind};
pub use social::{
    Avatar, AvatarTone, Badge, BadgeKind, CopierRow, FollowButton, ProfileHeader, SentimentBar,
    Social, SocialLink, SolscanLink, SolscanStyle, StatGrid, StatTile, StatTone, TeamChips,
};
pub use stake::{CoinPill, Currency, StakeAmount};
pub use status::PriceMoved;

use dioxus::prelude::*;

const JAKARTA_LATIN: Asset = asset!("/assets/fonts/plus-jakarta-sans-latin-wght-normal.woff2");
const JAKARTA_LATIN_EXT: Asset =
    asset!("/assets/fonts/plus-jakarta-sans-latin-ext-wght-normal.woff2");
/// Inter SemiBold cut to the five glyphs of "BetFi" (1 KB): the wordmark, as on the slides.
const WORDMARK: Asset = asset!("/assets/fonts/inter-600-betfi.woff2");

/// Unicode range of the latin subset (Fontsource's): "€", "•" and "≈"'s neighbours live here.
const LATIN: &str = "U+0000-00FF,U+0131,U+0152-0153,U+02BB-02BC,U+02C6,U+02DA,U+02DC,U+0304,\
U+0308,U+0329,U+2000-206F,U+20AC,U+2122,U+2191,U+2193,U+2212,U+2215,U+FEFF,U+FFFD";

/// Unicode range of the latin-ext subset (Fontsource's), so "ç" in Fenerbahçe loads it.
const LATIN_EXT: &str = "U+0100-02BA,U+02BD-02C5,U+02C7-02CC,U+02CE-02D7,U+02DD-02FF,U+0304,\
U+0308,U+0329,U+1D00-1DBF,U+1E00-1E9F,U+1EF2-1EFF,U+2020,U+20A0-20AB,U+20AD-20C0,U+2113,\
U+2C60-2C7F,U+A720-A7FF";

/// Plus Jakarta Sans, the only typeface (DESIGN.md), variable 200–800, plus the wordmark's five
/// Inter glyphs, bundled with the app (OFL, `assets/fonts/OFL.txt`); nothing is fetched from a
/// font CDN.
#[component]
pub fn Fonts() -> Element {
    let css = format!(
        "@font-face{{font-family:'Plus Jakarta Sans';font-style:normal;font-weight:200 800;\
         font-display:swap;src:url({JAKARTA_LATIN}) format('woff2');unicode-range:{LATIN};}}\
         @font-face{{font-family:'Plus Jakarta Sans';font-style:normal;font-weight:200 800;\
         font-display:swap;src:url({JAKARTA_LATIN_EXT}) format('woff2');\
         unicode-range:{LATIN_EXT};}}\
         @font-face{{font-family:'BetFi Wordmark';font-style:normal;font-weight:600;\
         font-display:block;src:url({WORDMARK}) format('woff2');}}"
    );
    rsx! {
        document::Link { rel: "preload", href: JAKARTA_LATIN, r#as: "font", r#type: "font/woff2", crossorigin: "anonymous" }
        document::Link { rel: "preload", href: WORDMARK, r#as: "font", r#type: "font/woff2", crossorigin: "anonymous" }
        document::Style { {css} }
    }
}
