//! `/dev/kit` (dev tools only): the v2 kit. First the mockup's pieces rebuilt from kit parts with
//! the mockup's own content (`#spec-*`, each one 390 px wide, shot by `scripts/visual/shoot.mjs`
//! next to the matching crop of the mockup PNG); then every component in every state; then the
//! tokens, the type scale and the icons. Specimens are not screens: they use the mockup's text
//! as drawn after the guidelines (chances for scores, guidelines §2 wording), for the visual review only.

use dioxus::prelude::*;

use super::money::{approx_sol, sol_text, sol_u};
use super::*;
use crate::Route;
use crate::config;

/// SOL at €135.80 (L7-2), for the specimens' "≈ SOL" lines.
const SOL_PPM: u64 = 135_800_000;

/// The v2 tokens (`assets/main.css` `:root`), as swatches.
const SWATCHES: [(&str, &str); 24] = [
    ("--surface", "#121318"),
    ("--surface-container-lowest", "#0d0e13"),
    ("--surface-container-low", "#1a1b21"),
    ("--surface-container", "#1e1f25"),
    ("--surface-container-high", "#292a2f"),
    ("--surface-container-highest", "#34343a"),
    ("--surface-bright", "#38393f"),
    ("--on-surface", "#e3e1e9"),
    ("--on-surface-variant", "#cec2d8"),
    ("--outline", "#978da1"),
    ("--outline-variant", "#4c4355"),
    ("--sol-purple", "#9945FF"),
    ("--inverse-primary", "#7f21e5"),
    ("--primary", "#d8b9ff"),
    ("--sol-green", "#14F195"),
    ("--secondary-fixed", "#56ffa8"),
    ("--secondary", "#a0ffc3"),
    ("--secondary-container", "#00ec91"),
    ("--tertiary", "#7bd0ff"),
    ("--error", "#ffb4ab"),
    ("--error-container", "#93000a"),
    ("--canvas-deep", "#0A0B10"),
    ("--text-secondary", "#94A3B8"),
    ("--text-muted", "#64748B"),
];

/// DESIGN.md's type scale (`.t-*` classes in main.css).
const TYPE: [(&str, &str); 11] = [
    ("t-display-hero", "Display hero · 48 / 800"),
    ("t-display-hero-mobile", "€25.00 · 32 / 800"),
    ("t-headline-lg", "€113.75 · 28 / 700"),
    ("t-headline-md", "Match Winner · 22 / 700"),
    ("t-headline-sm", "Stake Amount · 18 / 600"),
    ("t-body-lg", "Brag on · 16 / 400"),
    ("t-body-md", "Total Selected Stake · 14 / 400"),
    ("t-body-sm", "≈ 0.184 SOL (1 SOL = €135.80) · 12 / 400"),
    ("t-label-caps", "LIVE TICKER · 11 / 700 / 0.06em"),
    ("t-odds-display", "2.35 1.62 4.55 · 15 / 700"),
    ("t-data-metric", "+24.8% 1,284 · 20 / 700"),
];

fn eur2(e: u32) -> String {
    format!("€{e}.00")
}

fn approx_line(e: u32) -> Option<String> {
    approx_sol(u64::from(e) * 1_000_000, Some(SOL_PPM)).map(|s| format!("{s} (1 SOL = €135.80)"))
}

#[component]
pub fn KitGallery() -> Element {
    if !config::dev_tools() {
        return rsx! {
            h1 { "Not available" }
        };
    }
    let mut sport = use_signal(|| Sport::Basketball);
    let mut picked = use_signal(Vec::<(usize, usize)>::new);
    let mut stake = use_signal(|| 25u32);
    let mut stake_copy = use_signal(|| 25u32);
    let mut currency = use_signal(|| Currency::Eur);
    let mut following = use_signal(|| false);
    let mut tip = use_signal(|| 0usize);
    let mut sheet_open = use_signal(|| false);
    let mut demo_sel = use_signal(|| 2usize);
    let mut card_sel = use_signal(|| Some(1usize));

    let is_picked = move |card: usize, i: usize| picked.read().contains(&(card, i));
    let mut toggle = move |card: usize, i: usize| {
        let mut p = picked.write();
        if let Some(at) = p.iter().position(|x| *x == (card, i)) {
            p.remove(at);
        } else {
            p.retain(|x| x.0 != card);
            p.push((card, i));
        }
    };
    let upcoming: [(&str, &str, &str, &str, Vec<u64>); 3] = [
        (
            "EuroLeague • 20:45",
            "Head to Head",
            "Real Madrid",
            "Olympiacos",
            vec![1_520_000, 2_550_000],
        ),
        (
            "NBA • 01:00",
            "Moneyline",
            "Boston Celtics",
            "NY Knicks",
            vec![1_710_000, 2_180_000],
        ),
        (
            "Champions League • 21:00",
            "Full Time Result (1X2)",
            "Arsenal",
            "Bayern Munich",
            vec![1_950_000, 3_600_000, 3_800_000],
        ),
    ];
    let ticker = vec![TickerLine {
        key: 0,
        logo: None,
        who: "sol_bull99".into(),
        amount: "5.0 SOL".into(),
        team: "Real Madrid".into(),
        odds: "1.88".into(),
    }];
    let hero_tones = outcome_tones(&[2_350_000, 1_620_000]);
    let slip_tones = outcome_tones(&[2_350_000, 1_520_000, 1_710_000]);
    let s = stake();
    let sc = stake_copy();
    let payout = |e: u32| format!("€{:.2}", f64::from(e) * 4.55);
    let payout_sol = |e: u32| approx_sol(u64::from(e) * 4_550_000, Some(SOL_PPM));

    rsx! {
        div { class: "kit-gallery", id: "kit-gallery",
            h1 { "Kit v2" }
            p { class: "kit-note",
                "BetFi C2 rebuilt from the kit with the mockup's content, read after its guidelines \
                 (chances in place of scores). Specimens first, then every component in every \
                 state, then the tokens."
            }

            // ---------------------------------------------------------------- specimens
            Spec { id: "spec-appbar", title: "App bar and ticker",
                div { class: "kit-chrome",
                    AppBar { eur: "€1,240.50", sol: "9.13" }
                    LiveTicker { lines: ticker.clone() }
                }
            }
            Spec { id: "spec-tabs", title: "Sport tabs",
                div { class: "kit-pad",
                    SportTabs { sports: Sport::ALL.to_vec(), active: sport(), onselect: move |x| sport.set(x) }
                }
            }
            Spec { id: "spec-hero", title: "Match card, live (V-1: chances)",
                div { class: "kit-pad",
                    MatchCard {
                        meta: "EuroLeague • Live · 50′",
                        live: true,
                        to: Route::KitGallery {},
                        action: "Market volume",
                        first: TeamLine { name: "Panathinaikos".into(), code: Some("PAO".into()), value: Some("41%".into()), logo: None },
                        second: TeamLine { name: "Fenerbahçe".into(), code: Some("FB".into()), value: Some("59%".into()), logo: None },
                        OutcomeButton { outcome: "1", value: "2.35", tone: hero_tones[0], hero: true, selected: is_picked(9, 0), onclick: move |_| toggle(9, 0) }
                        OutcomeButton { outcome: "2", value: "1.62", tone: hero_tones[1], hero: true, selected: is_picked(9, 1), onclick: move |_| toggle(9, 1) }
                    }
                }
            }
            Spec { id: "spec-upcoming", title: "Upcoming matches (1–2 and 1–X–2)",
                div { class: "kit-pad kit-stack",
                    SectionTitle { title: "Upcoming", end: rsx! { "Today" } }
                    for (ci, (meta, right, a, b, vals)) in upcoming.into_iter().enumerate() {
                        MatchCard { key: "{a}", meta, right, first: TeamLine::named(a), second: TeamLine::named(b),
                            for (i, t) in outcome_tones(&vals).into_iter().enumerate() {
                                OutcomeButton {
                                    key: "{i}",
                                    outcome: if vals.len() == 3 { ["1", "X", "2"][i] } else { ["1", "2"][i] },
                                    value: crate::format::odds(vals[i]),
                                    tone: t,
                                    selected: is_picked(ci, i),
                                    onclick: move |_| toggle(ci, i),
                                }
                            }
                        }
                    }
                }
            }
            Spec { id: "spec-pill", title: "Slip pill",
                div { class: "kit-pad",
                    SlipPill { picks: 3, total: "4.55", onclick: move |_| sheet_open.set(true) }
                }
            }
            Spec { id: "spec-nav", title: "Bottom nav",
                div { class: "kit-static", BottomNav { active: NavTab::Home } }
            }
            Spec { id: "spec-drawer", title: "Bet drawer (bottom sheet over the sportsbook)",
                div { class: "kit-sheet-frame",
                    div { class: "kit-pad kit-stack",
                        SportTabs { sports: Sport::ALL.to_vec(), active: Sport::Basketball, onselect: |_| {} }
                        MatchCard {
                            meta: "EuroLeague • Live · 50′",
                            live: true,
                            action: "Market volume",
                            first: TeamLine { name: "Panathinaikos".into(), code: Some("PAO".into()), value: Some("41%".into()), logo: None },
                            second: TeamLine { name: "Fenerbahçe".into(), code: Some("FB".into()), value: Some("59%".into()), logo: None },
                            OutcomeButton { outcome: "1", value: "2.35", tone: hero_tones[0], hero: true, onclick: |_| {} }
                            OutcomeButton { outcome: "2", value: "1.62", tone: hero_tones[1], hero: true, onclick: |_| {} }
                        }
                    }
                    BottomSheet {
                        open: true,
                        contained: true,
                        id: "kit-drawer",
                        title: "Schedina",
                        icon: Sym::Receipt,
                        badge: rsx! { Badge { kind: BadgeKind::Picks(3) } },
                        onclose: |_| {},
                        onclear: |_| {},
                        footer: rsx! { PurpleCTA { label: format!("Bet • {}", eur2(s)), onclick: |_| {} } },
                        PickRow { team: "Panathinaikos", league: "EUROLEAGUE", detail: "vs Fenerbahçe", value: "2.35", tone: slip_tones[0], onremove: |_| {} }
                        PickRow { team: "Real Madrid", league: "EUROLEAGUE", detail: "vs Olympiacos", value: "1.52", tone: slip_tones[1], onremove: |_| {} }
                        PickRow { team: "Boston Celtics", league: "NBA", detail: "vs NY Knicks", value: "1.71", tone: slip_tones[2], onremove: |_| {} }
                        StakeAmount {
                            id: "kit-stake",
                            compact: true,
                            amount: eur2(s),
                            approx: approx_line(s),
                            value: s,
                            min: 5,
                            max: 200,
                            presets: vec![10, 25, 50, 100],
                            currency: currency(),
                            oncurrency: move |c| currency.set(c),
                            onchange: move |v| stake.set(v),
                        }
                        SummaryRows { compact: true, rows: vec![SummaryRow::total("4.55"), SummaryRow::payout("Payout", payout(s), None)] }
                    }
                }
            }
            Spec { id: "spec-copy", title: "Stake block and summary (copy confirm)",
                div { class: "kit-pad kit-stack",
                    StakeAmount {
                        id: "kit-stake-copy",
                        amount: eur2(sc),
                        approx: approx_line(sc),
                        value: sc,
                        min: 5,
                        max: 250,
                        presets: vec![10, 25, 50, 100],
                        currency: currency(),
                        oncurrency: move |c| currency.set(c),
                        onchange: move |v| stake_copy.set(v),
                    }
                    SummaryRows {
                        rows: vec![
                            SummaryRow::total("4.55"),
                            SummaryRow::plain("Stake", eur2(sc)),
                            SummaryRow::plain("Copy fee", "2% · 80% to @lorenzo_hoops"),
                            SummaryRow::payout("Potential Payout", payout(sc), payout_sol(sc)),
                        ],
                    }
                    PurpleCTA { label: format!("Copy Schedina • {}", eur2(sc)), icon: Sym::ContentCopy, tall: true, onclick: |_| {} }
                }
            }
            Spec { id: "spec-legs", title: "Copied legs",
                div { class: "kit-pad kit-stack",
                    LegCard { sport: Sym::Basketball, meta: "EUROLEAGUE • LIVE · 50′", live: true, team: "Panathinaikos", detail: "vs Fenerbahçe Beko", pick: "PICK (1)", value: "2.35" }
                    LegCard { sport: Sym::Basketball, meta: "EUROLEAGUE • STARTS 20:45", team: "Real Madrid", detail: "vs Olympiacos", pick: "PICK (1)", value: "1.52" }
                    LegCard { sport: Sym::Soccer, meta: "CHAMPIONS LEAGUE • 21:00", team: "Arsenal", detail: "vs Bayern Munich", pick: "PICK (1)", value: "1.95" }
                }
            }
            Spec { id: "spec-celebration", title: "Bet placement celebration",
                div { class: "kit-pad kit-stack kit-center",
                    TxBadge { slot: "284,912,041" }
                    CelebrationHero { title: "Schedina Locked In!", subtitle: "Hedge mock-filled · test money" }
                    PayoutHero { label: "Maximum Potential Payout", eur: "€113.75", sol: "≈ 0.838 SOL", rate: "@ €135.80", green: true }
                    div { class: "kit-receipt",
                        div { class: "kit-grid2",
                            StatTile { label: "Stake Placed", value: "€25.00", sub: "0.184 SOL", large: true }
                            StatTile { label: "Total", value: "4.55", sub: "3 legs", tone: StatTone::Green, large: true }
                        }
                        LegRow { title: "Panathinaikos", detail: "vs Fenerbahçe • EuroLeague", value: "2.35", tone: slip_tones[0] }
                        LegRow { title: "Real Madrid", detail: "vs Olympiacos • EuroLeague", value: "1.52", tone: slip_tones[1] }
                        LegRow { title: "Boston Celtics", detail: "vs NY Knicks • NBA", value: "1.71", tone: slip_tones[2] }
                    }
                    PurpleCTA { label: "View Transaction", trailing: Sym::OpenInNew, href: config::solscan_tx("5wHu1qwD7q5kXy4y5yFoMvKX2r7cK4nUMa6bwtvYJ8zqDyuYB2ifm2JbCX8n6DCr2SkjzCFZ6YB5JN9Gpd5G2eqT") }
                    SecondaryButton { label: "Brag on", x_logo: true, href: "https://x.com/intent/post?text=Schedina%20locked%20in%20on%20BetFi" }
                }
            }
            Spec { id: "spec-cashout", title: "Cash-out celebration",
                div { class: "kit-pad kit-stack kit-center",
                    CelebrationHero { title: "Cash Out Secured!", emoji: "💸", icon: HeroIcon::Check }
                    PayoutHero { label: "Total payout received", eur: "€61.20", sol: "≈ 0.451 SOL" }
                    TxBadge { slot: "892,104,821", label: "Cash-out confirmed" }
                }
            }
            Spec { id: "spec-profile", title: "Tipster profile",
                div { class: "kit-pad kit-stack",
                    ProfileHeader {
                        name: "Lorenzo Rossi",
                        handle: "lorenzo_hoops",
                        initials: "LR",
                        verified: true,
                        follow: rsx! { FollowButton { following: following(), onclick: move |_| following.toggle() } },
                        stats: rsx! {
                            StatGrid {
                                StatTile { label: "30D ROI", value: "+24.8%", tone: StatTone::Green }
                                StatTile { label: "Win rate", value: "76%" }
                                StatTile { label: "Copiers", value: "1,284", tone: StatTone::Cyan }
                            }
                        },
                        links: rsx! {
                            span { class: "kit-line",
                                SocialLink { kind: Social::X, handle: "lorenzo_hoops" }
                                SocialLink { kind: Social::YouTube, handle: "lorenzo_hoops" }
                                SocialLink { kind: Social::Telegram, handle: "lorenzo_hoops" }
                                SocialLink { kind: Social::Instagram, handle: "lorenzo_hoops" }
                            }
                            SolscanLink { account: "BPmJLbhyAugfbhLPFF7AJL7ifa2kDLKvjEkhYJQ14d2d" }
                        },
                    }
                    SectionTitle { title: "Active parlays (3)", small: true, end: rsx! { LiveDot { label: "Live on Solana" } } }
                    for (i, (title, legs, staked, pool, teams, total, detail)) in [
                        ("Master Treble", 3, "€500.00", "€28.4k", vec!["RM", "PAO", "OLY"], "4.55", "Real Madrid • Panathinaikos • Arsenal"),
                        ("EuroLeague Lock", 2, "€350.00", "€14.2k", vec!["FKB", "EFS"], "3.80", "Fenerbahçe • Anadolu Efes"),
                        ("Weekend Hail Mary", 4, "€200.00", "€9.8k", vec!["MIL", "BKN", "BAR"], "6.20", "Milan • Baskonia • Barcelona • Monaco"),
                    ].into_iter().enumerate() {
                        TipCard {
                            key: "{title}",
                            title,
                            legs,
                            staked,
                            pool,
                            teams: teams.into_iter().map(String::from).collect::<Vec<_>>(),
                            total,
                            selected: tip() == i,
                            detail,
                            pool_action: rsx! { ActionLink { label: "View Pool (1,284)", onclick: move |_| sheet_open.set(true) } },
                            onclick: move |_| tip.set(i),
                        }
                    }
                    PurpleCTA { label: "Copy Schedina • Total 4.55", icon: Sym::ContentCopy, arrow: true, onclick: |_| {} }
                }
            }
            Spec { id: "spec-copiers", title: "Copiers drawer",
                div { class: "kit-sheet-frame short",
                    BottomSheet {
                        open: true,
                        contained: true,
                        raised: true,
                        id: "kit-copiers",
                        title: "Copiers (1,284)",
                        badge: rsx! { Badge { kind: BadgeKind::LivePool } },
                        subtitle: rsx! {
                            b { "€28,450 Pool Volume" }
                            span { class: "sep", "•" }
                            span { class: "sol", "209.5 SOL" }
                            span { class: "sep", "•" }
                            span { class: "cyan", "Master Treble" }
                        },
                        onclose: |_| {},
                        for (ini, name, amt, tone, mono) in [
                            ("SB", "sol_bull99", "€1,200.00", AvatarTone::PurpleTint, false),
                            ("7x", "7xK9…94Lp", "€500.00", AvatarTone::GreenTint, true),
                            ("DC", "degencall.sol", "€350.00", AvatarTone::CyanTint, false),
                            ("EP", "elon_parlay", "€250.00", AvatarTone::Neutral, false),
                            ("3b", "3bF8…12qR", "€150.00", AvatarTone::PurpleTint, true),
                            ("SC", "ser_crypto", "€100.00", AvatarTone::GreenTint, false),
                        ] {
                            CopierRow { key: "{name}", initials: ini, name, stake: amt, tone, mono, sig: "4xT7sampleSig99pL" }
                        }
                    }
                }
            }
            Spec { id: "spec-event", title: "Event outcomes and sentiment",
                div { class: "kit-pad kit-stack",
                    div { class: "kit-grid2",
                        OutcomeCard { outcome: "OUTCOME 1", team: "Panathinaikos", value: "2.35", tone: hero_tones[0], selected: card_sel() == Some(0), onclick: move |_| card_sel.set(Some(0)) }
                        OutcomeCard { outcome: "OUTCOME 2", team: "Fenerbahçe", value: "1.62", tone: hero_tones[1], selected: card_sel() == Some(1), onclick: move |_| card_sel.set(Some(1)) }
                    }
                    SentimentBar { first_pct: 41, aria_label: "Panathinaikos 41%, Fenerbahçe 59%" }
                    SentimentBar { first_pct: 41, thin: true, aria_label: "Panathinaikos 41%, Fenerbahçe 59%" }
                }
            }

            // ---------------------------------------------------------------- states
            h1 { "States" }
            Block { title: "OutcomeButton: lowest green, highest purple, else neutral; selected; disabled",
                div { class: "kit-grid3",
                    for (i, (o, v, t)) in [("1", "1.95", OutcomeTone::Lowest), ("X", "3.60", OutcomeTone::Neutral), ("2", "3.80", OutcomeTone::Highest)].into_iter().enumerate() {
                        OutcomeButton { key: "{o}", outcome: o, value: v, tone: t, selected: demo_sel() == i, onclick: move |_| demo_sel.set(i) }
                    }
                }
                div { class: "kit-grid2",
                    OutcomeButton { outcome: "1", value: "1.90", onclick: |_| {} }
                    OutcomeButton { outcome: "2", value: "1.90", onclick: |_| {} }
                }
                div { class: "kit-grid2",
                    OutcomeButton { outcome: "1", value: "–", disabled: true, onclick: |_| {} }
                    OutcomeButton { outcome: "2", value: "2.18", tone: OutcomeTone::Highest, selected: true, disabled: true, onclick: |_| {} }
                }
            }
            Block { title: "Badges and pills",
                div { class: "kit-line",
                    Badge { kind: BadgeKind::Live }
                    Badge { kind: BadgeKind::LiveInPlay }
                    Badge { kind: BadgeKind::Prematch }
                    Badge { kind: BadgeKind::Legs(3) }
                    Badge { kind: BadgeKind::Pro }
                    Badge { kind: BadgeKind::Top(1) }
                    Badge { kind: BadgeKind::Picks(3) }
                    Badge { kind: BadgeKind::LivePool }
                    span { class: "k-test-chip", "TEST" }
                }
            }
            Block { title: "Avatars: tones, sizes, verified, address",
                div { class: "kit-line",
                    Avatar { initials: "LR", size: 64, tone: AvatarTone::Gradient, verified: true }
                    Avatar { initials: "LH", size: 48, verified: true }
                    Avatar { initials: "SB", size: 36, tone: AvatarTone::PurpleTint }
                    Avatar { initials: "7x", size: 36, tone: AvatarTone::GreenTint, mono: true }
                    Avatar { initials: "DC", size: 36, tone: AvatarTone::CyanTint }
                    Avatar { initials: "EP", size: 36, tone: AvatarTone::Neutral }
                    Avatar { initials: "GM", tone: AvatarTone::Green }
                    Avatar { initials: "AN", size: 36, tone: AvatarTone::tint_for("BPmJLbhyAugfbhLPFF7AJL7ifa2kDLKvjEkhYJQ14d2d") }
                    TeamChips { codes: vec!["MIL".into(), "BKN".into(), "BAR".into()] }
                }
            }
            Block { title: "Buttons",
                PurpleCTA { label: "Bet • €25.00", onclick: |_| {} }
                PurpleCTA { label: "Bet • €25.00", busy: "Hedging…", onclick: |_| {} }
                PurpleCTA { label: "Select to bet", icon: Sym::PostAdd, disabled: true, onclick: |_| {} }
                SecondaryButton { label: "Brag on", x_logo: true, onclick: |_| {} }
                div { class: "kit-line",
                    ActionLink { label: "Market volume", to: Route::KitGallery {} }
                    FollowButton { following: true, onclick: |_| {} }
                    SolscanLink { sig: "4xT7sampleSig99pL", style: SolscanStyle::Chip }
                    SolscanLink { sig: "4xT7sampleSig99pL", style: SolscanStyle::Inline, label: "Explore on Solscan" }
                    SolscanLink { sig: "4xT7sampleSig99pL", style: SolscanStyle::Icon }
                }
            }
            Block { title: "Stat tiles",
                StatGrid {
                    StatTile { label: "30D ROI", value: "−3.1%" }
                    StatTile { label: "Win rate", value: "41%" }
                    StatTile { label: "Copiers", value: "12", tone: StatTone::Cyan }
                }
            }
            Block { title: "Ticker: idle; marquee",
                div { class: "kit-chrome",
                    LiveTicker { lines: vec![], idle: "Bets placed on BetFi show here" }
                    LiveTicker { lines: ticker.clone(), marquee: true }
                }
            }
            Block { title: "Summary: the drawer's; money helpers",
                SummaryRows {
                    compact: true,
                    id: "kit-summary",
                    rows: vec![
                        SummaryRow::total("4.55"),
                        SummaryRow { label: "Pool".into(), value: super::money::eur_compact(28_450_000_000), kind: RowKind::Plain, ..SummaryRow::default() },
                        SummaryRow::plain("Pool volume", super::money::eur_whole(28_450_990_000)),
                        SummaryRow::payout("Payout", "€113.75", None).with_id("kit-payout"),
                    ],
                }
            }
            Block { title: "Bottom sheet over the page, above the nav",
                SecondaryButton { label: "Open the sheet", icon: Sym::Receipt, id: "kit-open-sheet", onclick: move |_| sheet_open.set(true) }
            }
            BottomSheet {
                open: sheet_open(),
                above_nav: true,
                id: "kit-sheet",
                title: "Schedina",
                icon: Sym::Receipt,
                badge: rsx! { Badge { kind: BadgeKind::Picks(3) } },
                onclose: move |_| sheet_open.set(false),
                onclear: move |_| sheet_open.set(false),
                footer: rsx! { PurpleCTA { label: "Bet • €25.00", onclick: move |_| sheet_open.set(false) } },
                PickRow { team: "Panathinaikos", league: "EUROLEAGUE", detail: "vs Fenerbahçe", value: "2.35", tone: slip_tones[0], onremove: |_| {} }
                PickRow { team: "Real Madrid", league: "EUROLEAGUE", detail: "vs Olympiacos", value: "1.52", tone: slip_tones[1], onremove: |_| {} }
                PickRow { team: "Boston Celtics", league: "NBA", detail: "vs NY Knicks", value: "1.71", tone: slip_tones[2], onremove: |_| {} }
            }

            // ---------------------------------------------------------------- tokens
            h1 { "Tokens" }
            Block { title: "Colour",
                div { class: "kit-swatches",
                    for (name, hex) in SWATCHES {
                        div { key: "{name}", class: "kit-swatch",
                            span { style: "background: {hex}" }
                            code { "{hex}" }
                            span { "{name}" }
                        }
                    }
                }
            }
            Block { title: "Type: Plus Jakarta Sans, tabular figures",
                div { class: "kit-type",
                    for (class, text) in TYPE {
                        span { key: "{class}", class, "{text}" }
                    }
                }
            }
            Block { title: "Icons (Material Symbols Outlined)",
                div { class: "kit-icons",
                    span { class: "kit-icon", title: "X", XLogo { size: 22 } code { "x (brand)" } }
                    for sym in Sym::ALL {
                        span { key: "{sym.name()}", class: "kit-icon", title: "{sym.name()}",
                            SymIcon { sym, size: 22 }
                            code { "{sym.name()}" }
                        }
                    }
                }
            }
            p { class: "kit-note",
                "SOL figures at €135.80: {sol_text(sol_u(1_240_500_000, SOL_PPM))} for €1,240.50."
            }
        }
    }
}

/// `/dev/shell` (dev tools only): the shell around an empty screen, for the shell's visual
/// review: the sportsbook's chrome (app bar, ticker, slip pill, nav).
#[component]
pub fn KitShell() -> Element {
    if !config::dev_tools() {
        return rsx! {
            h1 { "Not available" }
        };
    }
    rsx! {
        div { id: "kit-shell", style: "min-height: 560px" }
    }
}

/// One specimen: a 390 px frame (`id` for the shooter) under a small caption.
#[component]
fn Spec(id: String, title: String, children: Element) -> Element {
    rsx! {
        section { class: "kit-spec",
            h2 { "{title}" }
            div { class: "kit-spec-frame", id, {children} }
        }
    }
}

#[component]
fn Block(title: String, children: Element) -> Element {
    rsx! {
        section { class: "kit-block",
            h2 { "{title}" }
            {children}
        }
    }
}
