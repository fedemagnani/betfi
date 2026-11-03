//! What others think: the desk's `/sentiment` as the event page's card.
//!
//! - **Staked Sentiment** (7.1c, the event page's card, `betfi_event_detail_market_volume`):
//!   what Pascal holders have staked on each side, market makers left out (D-251), as a purple
//!   and green bar with each side's share, euros and ≈ SOL; Pool Participation (the desk's
//!   `traders` and their stake); the newest stakes on the game. Side 1 is the home team, side 2
//!   the away team (the 1-X-2 mapping of `/card`'s `outcome`). Below 3 people: "Too few traders
//!   yet".
//! - The drawer's "What others think" lines (L5-4) are gone (31 Oct, user request).
//!
//! Team names only; every number is the desk's (shares are display arithmetic on its integers).

use dioxus::prelude::*;

use super::kit::{self, SentimentBar, Sym, SymIcon};
use crate::desk::types::{Game, GameSentiment, RecentStake, split_pct};
use crate::format;

/// Fewer people than this holding the game on Pascal: no split, "Too few traders yet" (D-251).
const MIN_PEOPLE: u64 = 3;
/// Recent stakes listed on the card (the desk sends up to 10).
const RECENT_SHOWN: usize = 3;

/// The event page's Staked Sentiment card. `sentiment` is `None` while `/sentiment` loads, or
/// when its answer (`loaded`) lists no entry for the game; `fx` is the euro rate, `sol` the SOL
/// rate; `now_ms` dates the recent stakes.
#[component]
pub fn StakedSentiment(
    game: Game,
    #[props(!optional)] sentiment: Option<GameSentiment>,
    loaded: bool,
    #[props(!optional)] fx: Option<u64>,
    #[props(!optional)] sol: Option<u64>,
    now_ms: i64,
) -> Element {
    let Some(s) = sentiment else {
        return rsx! {
            section { class: "ev-card ev-sent", id: "event-sentiment", aria_labelledby: "event-sent-title",
                SentHead { staked: None }
                p { class: "ev-thin", id: "event-thin",
                    if loaded { "Too few traders yet" } else { "Loading stakes…" }
                }
            }
        };
    };
    let h = s.pascal_holders;
    let (first_u, second_u) = (h.home.stake_u, h.away.stake_u);
    let held_u = first_u.saturating_add(second_u);
    let thin = h.thin || h.people() < MIN_PEOPLE;
    let split = (!thin).then(|| split_pct(first_u, second_u)).flatten();
    let staked = split.map(|_| format!("{} staked", whole(held_u, fx)));
    // The pool is everyone `traders` counts: non-maker Pascal holders and Schedina players.
    let pool_u = held_u
        .saturating_add(s.schedina.home_u)
        .saturating_add(s.schedina.away_u);
    let traders = s.traders.unwrap_or_else(|| h.people());
    let (home, away) = (game.home.name.clone(), game.away.name.clone());
    let side = |name: &str, n: u8, u: u64| {
        let sol = fx.and_then(|r| kit::money::approx_sol(kit::money::eur_u(u, r), sol));
        (
            format!("{name} ({n})"),
            whole(u, fx),
            sol.map(|x| format!("({x})")),
        )
    };
    let (t1, e1, s1) = side(&home, 1, first_u);
    let (t2, e2, s2) = side(&away, 2, second_u);
    let recent: Vec<(String, &'static str, String, String)> = s
        .recent
        .iter()
        .filter_map(|r| recent_row(r, &game, fx, now_ms))
        .take(RECENT_SHOWN)
        .collect();
    let people = match traders {
        1 => "1 ACTIVE TRADER".to_string(),
        n => format!("{} active traders", grouped(n)),
    };
    let pool = compact(pool_u, fx);
    rsx! {
        section { class: "ev-card ev-sent", id: "event-sentiment", aria_labelledby: "event-sent-title",
            SentHead { staked }
            if let Some((p1, p2)) = split {
                div { class: "ev-split", id: "event-split",
                    SentimentBar {
                        first_pct: u32::try_from(p1).unwrap_or(100),
                        aria_label: "Staked on Pascal: {home} {p1}%, {away} {p2}%",
                    }
                    div { class: "ev-sides",
                        div { class: "ev-side first",
                            span { class: "ev-side-pct", "{p1}%" }
                            span { class: "ev-side-team", "{t1}" }
                            span { class: "ev-side-money",
                                "{e1}"
                                if let Some(x) = &s1 {
                                    " "
                                    span { class: "ev-side-sol", "{x}" }
                                }
                            }
                        }
                        div { class: "ev-side second",
                            span { class: "ev-side-pct", "{p2}%" }
                            span { class: "ev-side-team", "{t2}" }
                            span { class: "ev-side-money",
                                "{e2}"
                                if let Some(x) = &s2 {
                                    " "
                                    span { class: "ev-side-sol", "{x}" }
                                }
                            }
                        }
                    }
                }
            } else {
                p { class: "ev-thin", id: "event-thin", "Too few traders yet" }
            }
            div { class: "ev-pool", id: "event-pool",
                div { class: "ev-pool-start",
                    span { class: "ev-pool-icon", SymIcon { sym: Sym::Group, size: 18 } }
                    span { class: "ev-pool-words",
                        span { class: "ev-pool-title", "Pool participation" }
                        span { class: "ev-caps", id: "event-traders", "{people}" }
                    }
                }
                span { class: "ev-pool-total", id: "event-pool-total", "{pool}" }
            }
            if !recent.is_empty() {
                div { class: "ev-recent", id: "event-recent",
                    span { class: "ev-caps ev-recent-title", "Recent stakes" }
                    for (i, (amount, dot, team, when)) in recent.into_iter().enumerate() {
                        div { key: "{i}", class: "ev-recent-row",
                            span { class: "ev-recent-start",
                                span { class: "ev-dot {dot}", "aria-hidden": "true" }
                                span { class: "ev-recent-amt", "{amount}" }
                                span { class: "ev-recent-on", "on {team}" }
                            }
                            span { class: "ev-recent-when", "{when}" }
                        }
                    }
                }
            }
        }
    }
}

/// The card's title row: "Staked sentiment" and, when there is a split, the "€x staked" chip.
#[component]
fn SentHead(#[props(!optional)] staked: Option<String>) -> Element {
    rsx! {
        div { class: "ev-sent-head",
            h2 { class: "ev-sent-title", id: "event-sent-title",
                SymIcon { sym: Sym::PieChart, size: 20 }
                "Staked sentiment"
            }
            if let Some(t) = staked {
                span { class: "ev-chip", id: "event-staked",
                    SymIcon { sym: Sym::Lock, size: 14 }
                    span { "{t}" }
                }
            }
        }
    }
}

/// One recent stake as the card lists it: ("€500", dot class, "Panathinaikos", "1 min ago"), or
/// `None` for a side the app does not know.
fn recent_row(
    r: &RecentStake,
    game: &Game,
    fx: Option<u64>,
    now_ms: i64,
) -> Option<(String, &'static str, String, String)> {
    let (dot, team) = match r.team.as_str() {
        "home" => ("first", game.home.name.clone()),
        "away" => ("second", game.away.name.clone()),
        "draw" => ("draw", "Draw".to_string()),
        _ => return None,
    };
    Some((whole(r.amount_u, fx), dot, team, ago(r.ts_ms, now_ms)))
}

/// "1 min ago", "3 mins ago", "2 h ago": how long before `now_ms` a stake was made.
fn ago(ts_ms: u64, now_ms: i64) -> String {
    let secs = (now_ms - i64::try_from(ts_ms).unwrap_or(i64::MAX)).max(0) / 1000;
    match secs {
        0..60 => "just now".to_string(),
        60..120 => "1 min ago".to_string(),
        120..3_600 => format!("{} mins ago", secs / 60),
        3_600..86_400 => format!("{} h ago", secs / 3_600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

/// "€14,633" for `usd_u` micro-USD at the euro rate (USDC at 1 USD, truncated), or "15,887
/// USDC" before the rate is known.
fn whole(usd_u: u64, fx: Option<u64>) -> String {
    match fx {
        Some(r) => kit::money::eur_whole(kit::money::eur_u(usd_u, r)),
        None => format!("{} USDC", grouped(usd_u / format::MICRO)),
    }
}

/// "€23.9k" (the pool's size, as drawn), or "25.9k USDC" before the rate is known.
fn compact(usd_u: u64, fx: Option<u64>) -> String {
    match fx {
        Some(r) => kit::money::eur_compact(kit::money::eur_u(usd_u, r)),
        None => {
            let usd = usd_u / format::MICRO;
            if usd >= 1_000 {
                format!("{}.{}k USDC", usd / 1_000, usd % 1_000 / 100)
            } else {
                format!("{usd} USDC")
            }
        }
    }
}

/// 1842 → "1,842".
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
