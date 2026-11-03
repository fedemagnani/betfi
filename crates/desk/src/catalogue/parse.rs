//! From a Pascal [`Market`] to the catalogue's NBA market record: full-game winners (moneylines),
//! spreads and totals (6.0b).
//!
//! Every one is a binary contract on one symbol. The long side is the market's own proposition,
//! the short side its complement (proposed D-321):
//!
//! | `market_type.market` | long side | short side |
//! | --- | --- | --- |
//! | `moneyline {outcome: T}` | T wins | the other team wins |
//! | `spread {team: T, line: L}` | T's margin + L > 0 ("T L") | the other team covers −L ("O −L") |
//! | `total_points {line: L}` | total points > L ("Over L") | total points < L ("Under L") |
//!
//! Lines must be half points (`-3.5`, `221.5`), so neither side can push.
//!
//! Soccer (7.0b, D-435): the full-time result of an EPL, Bundesliga or LaLiga match is listed as
//! three yes-markets, one per outcome (`EPL_26OCT31_AVLFUL_ML.AVL`, `….DRAW`, `….FUL`; the
//! symbol's team pair is home then away, unlike the NBA's). Each is a winner market whose long
//! side is its outcome; its short side is never offered ([`soccer_market`]). The draw's
//! `long_team` is a placeholder (home, proposed D-461): read [`CatalogueMarket::draw`].

use pascal_rs::{
    DisplayAttributes, Market, MarketType, MicroUsd, Outcome, Reference, Sport, SportPeriod,
    SportsMarket,
};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::api::types::{League, MarketKind, Outcome as Outcome1x2, PickTeam, Side, TeamSide};

/// 1.0 in micro-units.
const ONE_U: i64 = 1_000_000;

/// One market as the catalogue keeps it: an NBA winner, spread or total, or one outcome of a
/// soccer 1-X-2. Prices are as listed by `/markets` (indicative; the quoter prices from
/// `/books`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogueMarket {
    /// Symbol, e.g. `NBA_26OCT28_PHIIND_ML.PHI`.
    pub symbol: String,
    /// `game_page.game_id`, e.g. `nba-phi-ind-2026-10-28`.
    pub game_id: String,
    /// `event_description`.
    pub event_desc: String,
    /// `market_description`.
    pub market_desc: String,
    /// Away team.
    pub away: TeamInfo,
    /// Home team.
    pub home: TeamInfo,
    /// Team the symbol is long on: the winner, or the spread's team. A total has no team; its
    /// long side (Over) is recorded as the away team (proposed D-321).
    pub long_team: TeamSide,
    /// Winner, spread or total.
    pub kind: MarketKind,
    /// The line in tenths of a point (`-35` for a −3.5 spread, `2215` for a 221.5 total);
    /// `None` for a winner. Always a half point.
    pub line_x10: Option<i32>,
    /// `expected_event_start_time_ms`.
    pub start_ms: u64,
    /// `expected_resolution_time_ms`.
    pub expected_resolution_ms: u64,
    /// Minimum tick, micro-USD (D-009).
    pub tick_size_min_u: MicroUsd,
    /// Tick significant figures.
    pub tick_sig_figs: u8,
    /// Taker fee rate, ppm (0.02 → 20_000).
    pub taker_fee_rate_ppm: u32,
    /// Maker rebate share, ppm.
    pub maker_rebate_share_ppm: u32,
    /// Polymarket slug of the reference market, when there is one.
    pub polymarket_slug: Option<String>,
    /// `mark_price`.
    pub mark_price_u: Option<MicroUsd>,
    /// `best_bid`.
    pub best_bid_u: Option<MicroUsd>,
    /// `best_ask`.
    pub best_ask_u: Option<MicroUsd>,
    /// `open_interest` (contracts).
    pub open_interest: u64,
    /// The league, from the symbol's prefix (`NBA_`, `EPL_`, `BUND_`, `LALIGA_`).
    pub league: League,
    /// Whether this is a soccer 1-X-2's draw symbol (`….DRAW`); its `long_team` is then a
    /// placeholder (home, proposed D-461).
    pub draw: bool,
}

/// A team as the catalogue names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamInfo {
    /// Full name.
    pub name: String,
    /// Abbreviation (`outcome_labels`, else from the symbol; spreads and totals take their
    /// game's moneyline's).
    pub abbr: String,
    /// Short name for labels, e.g. "76ers" (`teams.*_short_name`, else the moneyline's
    /// `outcome_labels.*.extended`, else the abbreviation).
    pub short: String,
    /// Pascal subject slug (same-day team rule, D-008).
    pub subject_id: String,
}

impl CatalogueMarket {
    /// The symbol's event part.
    pub fn event(&self) -> &str {
        self.symbol.split_once('.').map_or(&self.symbol, |(e, _)| e)
    }

    /// The side to take on the symbol to back `pick` (D-021). Meaningless on a total.
    pub fn side_for(&self, pick: TeamSide) -> Side {
        if pick == self.long_team {
            Side::Long
        } else {
            Side::Short
        }
    }

    /// The team a side backs (a placeholder on a total: Over = away, Under = home).
    pub fn pick_for(&self, side: Side) -> TeamSide {
        match side {
            Side::Long => self.long_team,
            Side::Short => self.long_team.other(),
        }
    }

    /// The team a side backs, `None` on a total.
    pub fn team_for(&self, side: Side) -> Option<TeamSide> {
        (self.kind != MarketKind::Total).then(|| self.pick_for(side))
    }

    /// Whether this is one outcome of a soccer 1-X-2 (long only).
    pub fn is_three_way(&self) -> bool {
        self.kind == MarketKind::Winner && self.league != League::Nba
    }

    /// The pick a side backs as the card names it (L7-1): the team, or `draw` for a soccer
    /// draw; `None` on a total, and on the short side of a soccer outcome (never offered: it
    /// backs two outcomes).
    pub fn pick_team(&self, side: Side) -> Option<PickTeam> {
        if self.is_three_way() {
            return match side {
                Side::Long if self.draw => Some(PickTeam::Draw),
                Side::Long => Some(self.long_team.into()),
                Side::Short => None,
            };
        }
        self.team_for(side).map(PickTeam::from)
    }

    /// A winner side's 1-X-2 code (`1` home, `X` draw, `2` away; L7-1), `None` on spreads,
    /// totals and a soccer outcome's short side.
    pub fn outcome(&self, side: Side) -> Option<Outcome1x2> {
        if self.kind != MarketKind::Winner {
            return None;
        }
        self.pick_team(side).map(PickTeam::outcome)
    }

    /// Team names for a side.
    pub fn team(&self, pick: TeamSide) -> &TeamInfo {
        match pick {
            TeamSide::Away => &self.away,
            TeamSide::Home => &self.home,
        }
    }

    /// The market's line as shown: the long team's handicap with its sign (`"-3.5"`,
    /// `"+1.5"`) for a spread, the total (`"221.5"`), `None` for a winner.
    pub fn line(&self) -> Option<String> {
        let x10 = self.line_x10?;
        Some(match self.kind {
            MarketKind::Spread => fmt_line(x10, true),
            _ => fmt_line(x10, false),
        })
    }

    /// The label of `side`: "76ers" / "Pacers" (winner), "76ers -3.5" / "Pacers +3.5"
    /// (spread), "Over 221.5" / "Under 221.5" (total). A soccer outcome's long side is its team
    /// ("Aston Villa") or "Draw"; its short side, never offered, is "Not Aston Villa" /
    /// "Not Draw".
    pub fn label(&self, side: Side) -> String {
        let x10 = self.line_x10.unwrap_or(0);
        if self.is_three_way() {
            let own = if self.draw {
                "Draw".to_owned()
            } else {
                self.team(self.long_team).short.clone()
            };
            return match side {
                Side::Long => own,
                Side::Short => format!("Not {own}"),
            };
        }
        match self.kind {
            MarketKind::Winner => self.team(self.pick_for(side)).short.clone(),
            MarketKind::Spread => {
                let (team, line) = match side {
                    Side::Long => (self.long_team, x10),
                    Side::Short => (self.long_team.other(), -x10),
                };
                format!("{} {}", self.team(team).short, fmt_line(line, true))
            }
            MarketKind::Total => {
                let word = match side {
                    Side::Long => "Over",
                    Side::Short => "Under",
                };
                format!("{word} {}", fmt_line(x10, false))
            }
        }
    }

    /// The away team's handicap in tenths for a spread (`-35` when the away team gives 3.5),
    /// the line itself for a total; orders a game's lines on the card.
    pub fn ladder_key(&self) -> i32 {
        let x10 = self.line_x10.unwrap_or(0);
        match (self.kind, self.long_team) {
            (MarketKind::Spread, TeamSide::Home) => -x10,
            _ => x10,
        }
    }

    /// The US date of the game, from the game id's `YYYY-MM-DD` suffix.
    pub fn date(&self) -> Option<&str> {
        game_date(&self.game_id)
    }

    /// Indicative price of a side from the list: the ask for long, `1e6 - bid` for short,
    /// `mark_price` (or `1e6 - mark_price`) when that side of the book is empty. `None` without
    /// any price or outside `(0, 1e6)`.
    pub fn indicative_price_u(&self, side: Side) -> Option<u64> {
        let p = match side {
            Side::Long => self.best_ask_u.or(self.mark_price_u)?,
            Side::Short => ONE_U - self.best_bid_u.or(self.mark_price_u)?,
        };
        (p > 0 && p < ONE_U).then(|| p.unsigned_abs())
    }

    /// Indicative chance of the long side: `mark_price`, else the bid/ask mid, else one side.
    pub fn long_mid_u(&self) -> Option<i64> {
        self.mark_price_u
            .or_else(|| Some((self.best_bid_u? + self.best_ask_u?) / 2))
            .or(self.best_bid_u)
            .or(self.best_ask_u)
    }
}

/// `x10` tenths as `"3.5"`; `signed` adds `+` / `-` (a spread handicap).
pub fn fmt_line(x10: i32, signed: bool) -> String {
    let a = x10.unsigned_abs();
    let sign = match (signed, x10.signum()) {
        (_, -1) => "-",
        (true, 1) => "+",
        _ => "",
    };
    format!("{sign}{}.{}", a / 10, a % 10)
}

/// Parses a Pascal line (`"-1.5"`, `"+2.5"`, `"221.5"`) into tenths of a point. Only a sign,
/// up to three digits and one decimal are accepted; `None` otherwise.
pub fn parse_line(s: &str) -> Option<i32> {
    let (neg, rest) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    let (whole, frac) = rest.split_once('.').unwrap_or((rest, "0"));
    let digits = |d: &str, max: usize| {
        !d.is_empty() && d.len() <= max && d.bytes().all(|c| c.is_ascii_digit())
    };
    if !digits(whole, 3) || !digits(frac, 1) {
        return None;
    }
    let v = whole.parse::<i32>().ok()? * 10 + frac.parse::<i32>().ok()?;
    Some(if neg { -v } else { v })
}

/// Whether `x10` tenths is a half point (no push possible).
pub fn is_half_point(x10: i32) -> bool {
    x10.rem_euclid(10) == 5
}

/// The `YYYY-MM-DD` suffix of a game id (`nba-phi-ind-2026-10-28` → `2026-10-28`).
pub fn game_date(game_id: &str) -> Option<&str> {
    let d = game_id.get(game_id.len().checked_sub(10)?..)?;
    is_date(d).then_some(d)
}

/// Whether `s` is shaped `YYYY-MM-DD`.
pub fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

/// Parses `m` if it is an unresolved NBA full-game moneyline with a game id and a start time.
pub fn nba_moneyline(m: &Market) -> Option<CatalogueMarket> {
    nba_market(m).filter(|c| c.kind == MarketKind::Winner)
}

/// Parses `m` if it is an unresolved NBA full-game moneyline, spread or total points market
/// with a game id and a start time (and, for a spread or total, a half-point line).
pub fn nba_market(m: &Market) -> Option<CatalogueMarket> {
    let da: &DisplayAttributes = &m.display_attributes;
    let MarketType::Sports(sports) = da.market_type.as_ref()? else {
        return None;
    };
    if sports.sport != Sport::Basketball
        || m.resolution.is_some()
        || sports.period.is_some_and(|p| p != SportPeriod::FullGame)
    {
        return None;
    }
    let (kind, long_team, line) = match &sports.market {
        SportsMarket::Moneyline { outcome } => {
            let t = match outcome {
                Outcome::Away => TeamSide::Away,
                Outcome::Home => TeamSide::Home,
                Outcome::Draw => return None,
            };
            (MarketKind::Winner, t, None)
        }
        SportsMarket::Spread { team, line } => {
            let t = match team {
                pascal_rs::TeamSide::Away => TeamSide::Away,
                pascal_rs::TeamSide::Home => TeamSide::Home,
            };
            (MarketKind::Spread, t, Some(line))
        }
        SportsMarket::TotalPoints { line } => (MarketKind::Total, TeamSide::Away, Some(line)),
        _ => return None,
    };
    let line_x10 = match line {
        None => None,
        Some(s) => {
            let x = parse_line(s).filter(|&x| is_half_point(x));
            let x = x.filter(|&x| kind != MarketKind::Total || x > 0);
            if x.is_none() {
                tracing::debug!(symbol = %m.symbol, line = %s, "line is not a half point; skipped");
                return None;
            }
            x
        }
    };
    let game = da.game_page.as_ref()?;
    if !game.game_id.starts_with("nba-") || game_date(&game.game_id).is_none() {
        return None;
    }
    let Some(start_ms) = da.expected_event_start_time_ms else {
        tracing::warn!(symbol = %m.symbol, "NBA market without a start time; skipped");
        return None;
    };
    let teams = &sports.teams;
    let (away_abbr, home_abbr) = abbreviations(m, kind, long_team);
    let labels = da
        .outcome_labels
        .as_ref()
        .filter(|_| kind == MarketKind::Winner);
    let label_of = |t: TeamSide| {
        labels.map(|l| {
            if t == long_team {
                l.long.extended.clone()
            } else {
                l.short.extended.clone()
            }
        })
    };
    let short = |given: &Option<String>, t: TeamSide, abbr: &str| {
        given
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| label_of(t))
            .unwrap_or_else(|| abbr.to_owned())
    };
    let polymarket_slug = da.reference.as_ref().map(|r| match r {
        Reference::Polymarket { market_slug, .. } => market_slug.clone(),
    });
    Some(CatalogueMarket {
        symbol: m.symbol.clone(),
        game_id: game.game_id.clone(),
        event_desc: da.event_description.clone(),
        market_desc: da.market_description.clone(),
        away: TeamInfo {
            name: teams.away_name.clone(),
            short: short(&teams.away_short_name, TeamSide::Away, &away_abbr),
            abbr: away_abbr,
            subject_id: teams.away_subject_id.clone(),
        },
        home: TeamInfo {
            name: teams.home_name.clone(),
            short: short(&teams.home_short_name, TeamSide::Home, &home_abbr),
            abbr: home_abbr,
            subject_id: teams.home_subject_id.clone(),
        },
        long_team,
        kind,
        line_x10,
        start_ms,
        expected_resolution_ms: da.expected_resolution_time_ms,
        tick_size_min_u: m.tick_size_min,
        tick_sig_figs: m.tick_sig_figs,
        taker_fee_rate_ppm: ppm(m.taker_fee_rate),
        maker_rebate_share_ppm: ppm(m.maker_rebate_share),
        polymarket_slug,
        mark_price_u: m.mark_price,
        best_bid_u: m.best_bid,
        best_ask_u: m.best_ask,
        open_interest: m.open_interest,
        league: League::Nba,
        draw: false,
    })
}

/// Parses `m` if it is any market the catalogue keeps: an NBA winner, spread or total
/// ([`nba_market`]) or a soccer 1-X-2 outcome ([`soccer_market`]).
pub fn catalogue_market(m: &Market) -> Option<CatalogueMarket> {
    nba_market(m).or_else(|| soccer_market(m))
}

/// The soccer league of a symbol prefix (`EPL_`, `BUND_`, `LALIGA_`, `UCL_`); `None` for any other.
pub fn soccer_league(symbol: &str) -> Option<League> {
    match symbol.split_once('_')?.0 {
        "EPL" => Some(League::Epl),
        "BUND" => Some(League::Bundesliga),
        "LALIGA" => Some(League::LaLiga),
        "UCL" => Some(League::Ucl),
        _ => None,
    }
}

/// Parses `m` if it is an unresolved regular-time moneyline outcome (home, draw or away) of an
/// EPL, Bundesliga, LaLiga or Champions League match with a game id and a start time (7.0b). Both-teams-to-score,
/// spreads, totals and other periods are left out (D-435). Abbreviations come from the symbol
/// (`EPL_26OCT31_AVLFUL_ML.AVL`: the pair is home then away, the suffix is the outcome);
/// [`super::Catalogue::build`] then gives all three outcomes their team symbols' suffixes.
pub fn soccer_market(m: &Market) -> Option<CatalogueMarket> {
    let league = soccer_league(&m.symbol)?;
    let da: &DisplayAttributes = &m.display_attributes;
    let MarketType::Sports(sports) = da.market_type.as_ref()? else {
        return None;
    };
    if sports.sport != Sport::Soccer
        || m.resolution.is_some()
        || sports.period.is_some_and(|p| p != SportPeriod::RegTime)
    {
        return None;
    }
    let SportsMarket::Moneyline { outcome } = &sports.market else {
        return None;
    };
    let (long_team, draw) = match outcome {
        Outcome::Home => (TeamSide::Home, false),
        Outcome::Away => (TeamSide::Away, false),
        Outcome::Draw => (TeamSide::Home, true),
    };
    let game = da.game_page.as_ref()?;
    game_date(&game.game_id)?;
    let Some(start_ms) = da.expected_event_start_time_ms else {
        tracing::warn!(symbol = %m.symbol, "soccer market without a start time; skipped");
        return None;
    };
    let suffix = m.symbol.rsplit_once('.').map_or("", |(_, t)| t);
    let pair = m.symbol.split('_').nth(2).unwrap_or("");
    let (home_abbr, away_abbr) = match outcome {
        Outcome::Home => (suffix, pair.strip_prefix(suffix).unwrap_or("")),
        Outcome::Away => (pair.strip_suffix(suffix).unwrap_or(""), suffix),
        Outcome::Draw if pair.len().is_multiple_of(2) && pair.is_ascii() => {
            pair.split_at(pair.len() / 2)
        }
        Outcome::Draw => ("", ""),
    };
    let teams = &sports.teams;
    let short = |given: &Option<String>, name: &str| {
        given
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| club_short(name))
    };
    let polymarket_slug = da.reference.as_ref().map(|r| match r {
        Reference::Polymarket { market_slug, .. } => market_slug.clone(),
    });
    Some(CatalogueMarket {
        symbol: m.symbol.clone(),
        game_id: game.game_id.clone(),
        event_desc: da.event_description.clone(),
        market_desc: da.market_description.clone(),
        away: TeamInfo {
            name: teams.away_name.clone(),
            abbr: away_abbr.to_owned(),
            short: short(&teams.away_short_name, &teams.away_name),
            subject_id: teams.away_subject_id.clone(),
        },
        home: TeamInfo {
            name: teams.home_name.clone(),
            abbr: home_abbr.to_owned(),
            short: short(&teams.home_short_name, &teams.home_name),
            subject_id: teams.home_subject_id.clone(),
        },
        long_team,
        kind: MarketKind::Winner,
        line_x10: None,
        start_ms,
        expected_resolution_ms: da.expected_resolution_time_ms,
        tick_size_min_u: m.tick_size_min,
        tick_sig_figs: m.tick_sig_figs,
        taker_fee_rate_ppm: ppm(m.taker_fee_rate),
        maker_rebate_share_ppm: ppm(m.maker_rebate_share),
        polymarket_slug,
        mark_price_u: m.mark_price,
        best_bid_u: m.best_bid,
        best_ask_u: m.best_ask,
        open_interest: m.open_interest,
        league,
        draw,
    })
}

/// A club's name without the "FC" / "AFC" / "CF" around it, for labels: "Aston Villa FC" →
/// "Aston Villa", "AFC Bournemouth" → "Bournemouth", "FC Bayern München" → "Bayern München".
pub fn club_short(name: &str) -> String {
    let mut s = name.trim();
    for p in ["AFC ", "FC "] {
        s = s.strip_prefix(p).unwrap_or(s);
    }
    for x in [" AFC", " FC", " CF"] {
        s = s.strip_suffix(x).unwrap_or(s);
    }
    if s.is_empty() { name.trim() } else { s }.to_owned()
}

/// A fraction as ppm, rounded to nearest (Pascal sends 6 d.p., so this is exact).
fn ppm(rate: Decimal) -> u32 {
    (rate * Decimal::from(1_000_000))
        .round()
        .to_u32()
        .unwrap_or(0)
}

/// (away, home) abbreviations. A moneyline's come from `outcome_labels`, else from the symbol
/// (`NBA_26OCT28_PHIIND_ML.PHI`: suffix = long team, the other three letters of `PHIIND`). A
/// spread's suffix is `TEAM_N` (`SP.PHI_3`); a total's has no team, so its pair is split in
/// half. [`super::Catalogue::build`] then gives spreads and totals their moneyline's.
fn abbreviations(m: &Market, kind: MarketKind, long_team: TeamSide) -> (String, String) {
    let order = |long: String, short: String| match long_team {
        TeamSide::Away => (long, short),
        TeamSide::Home => (short, long),
    };
    if kind == MarketKind::Winner
        && let Some(l) = &m.display_attributes.outcome_labels
    {
        return order(l.long.abbreviated.clone(), l.short.abbreviated.clone());
    }
    let suffix = m.symbol.rsplit_once('.').map_or("", |(_, t)| t);
    let pair = m.symbol.split('_').nth(2).unwrap_or("");
    if kind == MarketKind::Total {
        let half = pair.len() / 2;
        return match (pair.get(..half), pair.get(half..)) {
            (Some(a), Some(h)) if pair.len().is_multiple_of(2) => (a.to_owned(), h.to_owned()),
            _ => (String::new(), String::new()),
        };
    }
    let long = suffix.split('_').next().unwrap_or("").to_owned();
    let short = match long_team {
        TeamSide::Away => pair.strip_prefix(long.as_str()),
        TeamSide::Home => pair.strip_suffix(long.as_str()),
    }
    .unwrap_or("")
    .to_owned();
    order(long, short)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tape() -> Vec<Market> {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tapes/nba-2026-10-28/markets_selected.json"
        ))
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        serde_json::from_value(v["data"].clone()).unwrap()
    }

    #[test]
    fn parses_the_tape() {
        let list: Vec<_> = tape().iter().filter_map(nba_moneyline).collect();
        assert_eq!(list.len(), 5);
        let phi = list
            .iter()
            .find(|m| m.symbol == "NBA_26OCT28_PHIIND_ML.PHI")
            .unwrap();
        assert_eq!(phi.game_id, "nba-phi-ind-2026-10-28");
        assert_eq!(phi.date(), Some("2026-10-28"));
        assert_eq!(phi.long_team, TeamSide::Away);
        assert_eq!(phi.kind, MarketKind::Winner);
        assert_eq!(phi.line_x10, None);
        assert_eq!(
            (phi.away.abbr.as_str(), phi.home.abbr.as_str()),
            ("PHI", "IND")
        );
        assert_eq!(
            (phi.away.short.as_str(), phi.home.short.as_str()),
            ("76ers", "Pacers")
        );
        assert_eq!(phi.away.name, "Philadelphia 76ers");
        assert_eq!(phi.start_ms, 1_793_228_400_000);
        assert_eq!(phi.taker_fee_rate_ppm, 20_000);
        assert_eq!(phi.maker_rebate_share_ppm, 250_000);
        assert_eq!(phi.tick_size_min_u, 1_000);
        assert_eq!(phi.indicative_price_u(Side::Long), Some(699_000));
        assert_eq!(phi.indicative_price_u(Side::Short), Some(309_000));
        assert_eq!(phi.side_for(TeamSide::Home), Side::Short);
        assert_eq!(phi.pick_for(Side::Short), TeamSide::Home);
        assert_eq!(phi.label(Side::Long), "76ers");
        assert_eq!(phi.label(Side::Short), "Pacers");
        assert_eq!(
            phi.polymarket_slug.as_deref(),
            Some("nba-phi-ind-2026-10-28")
        );
    }

    #[test]
    fn falls_back_to_mark_and_symbol() {
        let mut m = tape()
            .into_iter()
            .find(|m| m.symbol == "NBA_26OCT28_ORLTOR_ML.ORL")
            .unwrap();
        m.best_ask = None;
        m.best_bid = None;
        m.display_attributes.outcome_labels = None;
        let c = nba_moneyline(&m).unwrap();
        assert_eq!(c.indicative_price_u(Side::Long), Some(375_000));
        assert_eq!(c.indicative_price_u(Side::Short), Some(625_000));
        assert_eq!((c.away.abbr.as_str(), c.home.abbr.as_str()), ("ORL", "TOR"));
        m.mark_price = None;
        assert_eq!(
            nba_moneyline(&m).unwrap().indicative_price_u(Side::Long),
            None
        );
    }

    #[test]
    fn rejects_other_markets() {
        let base = tape().remove(0);
        let mut resolved = base.clone();
        resolved.resolution = Some(pascal_rs::MarketResolution {
            price: 1_000_000,
            seq: 1,
            market_resolution_ts_ms: 1,
        });
        assert!(nba_market(&resolved).is_none());
        let set = |m: &mut Market, market: SportsMarket| {
            if let Some(MarketType::Sports(s)) = &mut m.display_attributes.market_type {
                s.market = market;
            }
        };
        // A total is a market now, but not a moneyline.
        let mut total = base.clone();
        set(
            &mut total,
            SportsMarket::TotalPoints {
                line: "219.5".into(),
            },
        );
        assert!(nba_moneyline(&total).is_none());
        assert_eq!(nba_market(&total).unwrap().kind, MarketKind::Total);
        // Whole-point and malformed lines are left out (a push is possible on them).
        for line in [
            "219", "219.0", "2.25", "abc", "", "-", "1e2", "-219.5", "0.5x",
        ] {
            set(&mut total, SportsMarket::TotalPoints { line: line.into() });
            assert!(nba_market(&total).is_none(), "{line}");
        }
        let mut btts = base.clone();
        set(&mut btts, SportsMarket::BothTeamsToScore {});
        assert!(nba_market(&btts).is_none());
        let mut half = base.clone();
        if let Some(MarketType::Sports(s)) = &mut half.display_attributes.market_type {
            s.period = Some(SportPeriod::FirstHalf);
        }
        assert!(nba_market(&half).is_none());
        let mut no_game = base;
        no_game.display_attributes.game_page = None;
        assert!(nba_market(&no_game).is_none());
    }

    #[test]
    fn lines() {
        assert_eq!(parse_line("-1.5"), Some(-15));
        assert_eq!(parse_line("+2.5"), Some(25));
        assert_eq!(parse_line("221.5"), Some(2215));
        assert_eq!(parse_line("-10.5"), Some(-105));
        assert_eq!(parse_line("0.5"), Some(5));
        assert_eq!(parse_line("-0.5"), Some(-5));
        assert_eq!(parse_line("3"), Some(30));
        for bad in [
            "", "-", "+", ".5", "1.", "1.25", "1234.5", "--1.5", " 1.5", "1,5",
        ] {
            assert_eq!(parse_line(bad), None, "{bad:?}");
        }
        assert!(is_half_point(-15) && is_half_point(2215) && is_half_point(-5));
        assert!(!is_half_point(30) && !is_half_point(-30) && !is_half_point(-12));
        assert_eq!(fmt_line(-35, true), "-3.5");
        assert_eq!(fmt_line(35, true), "+3.5");
        assert_eq!(fmt_line(-5, true), "-0.5");
        assert_eq!(fmt_line(5, true), "+0.5");
        assert_eq!(fmt_line(-105, true), "-10.5");
        assert_eq!(fmt_line(2215, false), "221.5");
        for s in ["-1.5", "+2.5", "-10.5", "-0.5", "+0.5"] {
            assert_eq!(fmt_line(parse_line(s).unwrap(), true), s);
        }
    }

    #[test]
    fn dates() {
        assert_eq!(game_date("nba-phi-ind-2026-10-28"), Some("2026-10-28"));
        assert_eq!(game_date("short"), None);
        assert_eq!(game_date("nba-phi-ind-2026-1-028"), None);
        assert!(is_date("2026-10-28"));
        assert!(!is_date("2026/10/28"));
    }
}
