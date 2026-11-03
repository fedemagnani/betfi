//! Tipster reputation, spec section 6, as pure functions of a tipster's tickets (no I/O), so every
//! number can be reproduced by hand (`tests/tipsters.rs`).
//!
//! Only own tips count (`tipster == user`, `copied_from == null`), and only once they left
//! `Open` by `settle` or `cash_out` ("scored"). An open ticket shows as `open_count` /
//! `open_staked_u` and never moves a score. All money is in micro-USDC; a ticket's stake is its
//! minted `rS` (D-084, D-113), its return what `settle` or `cash_out` paid.
//!
//! | Field | Definition (integer arithmetic, `i128` inside) |
//! | --- | --- |
//! | `settled` | scored own tips |
//! | `won` | scored own tips with `paid > rS` (the program's `TipsterProfile.won`, D-113) |
//! | `hit_rate_ppm` | `floor(won × 1e6 / settled)` |
//! | `roi_ppm` | `(Σ paid − Σ rS) × 1e6 / Σ rS`, truncated toward zero |
//! | `brier_ppm` | `floor(Σ (r̃ − p_fill)² / (legs × 1e6))` over the resolved legs of scored own tips (a cashed-out ticket's unresolved legs have no `r̃` and are left out) |
//! | `edge_ppm` | `Σ (r̃ − p_fill) / legs`, truncated toward zero, same legs |
//! | `avg_odds_u` | `floor(Σ odds_u / settled)`, the minted odds `P / rS` |
//! | `skin_u` | `Σ rS` of scored own tips minted at or after `now − 30 d` (D-170) |
//! | `followers_roi_ppm` | over scored copies of this tipster's tips: `(Σ paid − Σ (rS + copy fee)) × 1e6 / Σ (rS + copy fee)`; `null` with none |
//! | `streak` | consecutive `won` tickets from the latest scored one backwards |
//! | `open_count`, `open_staked_u` | own tips in state `Open`, their `Σ rS` |
//! | `copies` | minted copies (open, settled or cashed out) of this tipster's tips |
//! | `fees_earned_u` | `Σ TipsterPaid.amount_u` |
//! | `roi_30d_ppm` (6.0c) | `roi_ppm` over the scored own tips minted at or after `now − 30 d` (the skin window): `(Σ paid − skin) × 1e6 / skin`, truncated toward zero; 0 with no skin |
//! | `copiers` (6.0c) | distinct wallets with a minted copy of this tipster's tips, the tipster's own wallet (self-copies, D-114) left out |
//! | `verified` (6.0c) | on the default board, [`on_board`] (U-5, D-304) |
//! | `handle` (6.0c) | `null` here; filled from `profiles` by the routes |
//!
//! `r̃ = r` on a long leg and `1 − r` on a short one (spec section 10); `p_fill` is the price the
//! ticket paid for its side (the fill, or the quote price of a leg left unhedged). Scored tickets
//! are ordered by `(settled_at, ticket)`: the history is cumulative in that order, and the streak
//! counts back from its end.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::str::FromStr;

use crate::api::types::{Side, TicketStatus, TipsterHistoryPoint, TipsterRow};

/// Leaderboard filter: at least this many scored own tips (spec section 6).
pub const MIN_SETTLED: u32 = 5;
/// Leaderboard filter: at least this much skin, micro-USDC (20 USDC, spec section 6).
pub const MIN_SKIN_U: u64 = 20_000_000;
/// The skin window, s (30 days).
pub const SKIN_WINDOW_S: i64 = 30 * 86_400;

const UNIT: u64 = 1_000_000;
const PPM: i128 = 1_000_000;

/// One leg of an own tip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TipLeg {
    /// Side on the symbol.
    pub side: Side,
    /// Price paid for the ticket's side.
    pub p_fill_u: u64,
    /// Pascal's `r` (long side) once resolved.
    pub resolution_u: Option<u64>,
}

impl TipLeg {
    /// `r̃`, the leg's resolved value on the ticket's side.
    pub fn rtilde_u(&self) -> Option<u64> {
        self.resolution_u.map(|r| {
            let r = r.min(UNIT);
            match self.side {
                Side::Long => r,
                Side::Short => UNIT - r,
            }
        })
    }
}

/// An own tip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnTip {
    /// Ticket PDA.
    pub ticket: String,
    /// Lifecycle state.
    pub state: TicketStatus,
    /// `rS`.
    pub stake_u: u64,
    /// What settle or cash-out paid.
    pub paid_u: Option<u64>,
    /// Minted odds `P / rS`.
    pub odds_u: u64,
    /// Unix s.
    pub minted_at: Option<i64>,
    /// Unix s.
    pub settled_at: Option<i64>,
    /// Legs.
    pub legs: Vec<TipLeg>,
}

/// A copy of one of the tipster's tips (someone else's ticket, or a same-wallet self-copy).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyTicket {
    /// The copying wallet (6.0c, for `copiers`).
    pub user: String,
    /// Lifecycle state.
    pub state: TicketStatus,
    /// `rS`.
    pub stake_u: u64,
    /// Copy fee charged at finalize.
    pub copy_fee_u: u64,
    /// What settle or cash-out paid.
    pub paid_u: Option<u64>,
}

/// Everything a tipster's row is computed from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TipsterBook {
    /// Wallet, base58.
    pub wallet: String,
    /// Own tips, any state.
    pub own: Vec<OwnTip>,
    /// Copies of their tips, any state.
    pub copies: Vec<CopyTicket>,
    /// Σ `TipsterPaid`.
    pub fees_earned_u: u64,
}

/// A computed tipster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scored {
    /// The row.
    pub row: TipsterRow,
    /// Cumulative ROI and Brier per scored own tip, oldest first.
    pub history: Vec<TipsterHistoryPoint>,
    /// Resolved legs behind `brier_ppm` and `edge_ppm` (0: both undefined, shown as 0).
    pub legs: u64,
}

/// Settled or cashed out.
pub fn is_scored(s: TicketStatus) -> bool {
    matches!(s, TicketStatus::Settled | TicketStatus::CashedOut)
}

fn is_minted(s: TicketStatus) -> bool {
    matches!(
        s,
        TicketStatus::Open | TicketStatus::Settled | TicketStatus::CashedOut
    )
}

fn to_i64(v: i128) -> i64 {
    i64::try_from(v).unwrap_or(if v < 0 { i64::MIN } else { i64::MAX })
}

fn to_u64(v: i128) -> u64 {
    u64::try_from(v.max(0)).unwrap_or(u64::MAX)
}

fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// `(returned − staked) × 1e6 / staked`, truncated toward zero; 0 when nothing was staked.
pub fn roi_ppm(staked: i128, returned: i128) -> i64 {
    if staked <= 0 {
        return 0;
    }
    to_i64((returned - staked) * PPM / staked)
}

/// Running sums over scored tickets and their resolved legs.
#[derive(Debug, Clone, Copy, Default)]
struct Acc {
    staked: i128,
    returned: i128,
    /// Σ (r̃ − p)², micro-units squared.
    sq: i128,
    /// Σ (r̃ − p), micro-units.
    diff: i128,
    legs: u64,
}

impl Acc {
    fn add(&mut self, t: &OwnTip) {
        self.staked += i128::from(t.stake_u);
        self.returned += i128::from(t.paid_u.unwrap_or(0));
        for l in &t.legs {
            if let Some(rt) = l.rtilde_u() {
                let d = i128::from(rt) - i128::from(l.p_fill_u);
                self.sq += d * d;
                self.diff += d;
                self.legs += 1;
            }
        }
    }

    fn roi(&self) -> i64 {
        roi_ppm(self.staked, self.returned)
    }

    fn brier(&self) -> u64 {
        if self.legs == 0 {
            return 0;
        }
        to_u64(self.sq / (i128::from(self.legs) * PPM))
    }

    fn edge(&self) -> i64 {
        if self.legs == 0 {
            return 0;
        }
        to_i64(self.diff / i128::from(self.legs))
    }
}

/// Whether a scored ticket returned more than its stake.
fn won(t: &OwnTip) -> bool {
    t.paid_u.unwrap_or(0) > t.stake_u
}

/// The tipster's row and history at `now_s` (Unix s, for the skin window).
pub fn score(book: &TipsterBook, now_s: i64) -> Scored {
    let mut scored: Vec<&OwnTip> = book.own.iter().filter(|t| is_scored(t.state)).collect();
    scored.sort_by(|a, b| {
        (a.settled_at.unwrap_or(0), &a.ticket).cmp(&(b.settled_at.unwrap_or(0), &b.ticket))
    });

    let mut acc = Acc::default();
    let mut history = Vec::with_capacity(scored.len());
    for t in &scored {
        acc.add(t);
        history.push(TipsterHistoryPoint {
            ticket: t.ticket.clone(),
            settled_at: t.settled_at.unwrap_or(0),
            roi_cum_ppm: acc.roi(),
            brier_cum_ppm: acc.brier(),
        });
    }
    let settled = scored.len();
    let won_n = scored.iter().filter(|t| won(t)).count();
    let streak = scored.iter().rev().take_while(|t| won(t)).count();
    let odds_sum: i128 = scored.iter().map(|t| i128::from(t.odds_u)).sum();
    let since = now_s.saturating_sub(SKIN_WINDOW_S);
    let recent: Vec<&&OwnTip> = scored
        .iter()
        .filter(|t| t.minted_at.is_some_and(|m| m >= since))
        .collect();
    let skin: i128 = recent.iter().map(|t| i128::from(t.stake_u)).sum();
    let returned_30d: i128 = recent
        .iter()
        .map(|t| i128::from(t.paid_u.unwrap_or(0)))
        .sum();
    let copiers: BTreeSet<&str> = book
        .copies
        .iter()
        .filter(|c| is_minted(c.state) && c.user != book.wallet)
        .map(|c| c.user.as_str())
        .collect();
    let open: Vec<&OwnTip> = book
        .own
        .iter()
        .filter(|t| t.state == TicketStatus::Open)
        .collect();

    let scored_copies: Vec<&CopyTicket> =
        book.copies.iter().filter(|c| is_scored(c.state)).collect();
    let followers_roi_ppm = (!scored_copies.is_empty()).then(|| {
        let staked: i128 = scored_copies
            .iter()
            .map(|c| i128::from(c.stake_u) + i128::from(c.copy_fee_u))
            .sum();
        let returned: i128 = scored_copies
            .iter()
            .map(|c| i128::from(c.paid_u.unwrap_or(0)))
            .sum();
        roi_ppm(staked, returned)
    });

    let n = i128::try_from(settled).unwrap_or(i128::MAX);
    let per = |num: i128| if n == 0 { 0 } else { to_u64(num / n) };
    let mut row = TipsterRow {
        wallet: book.wallet.clone(),
        settled: to_u32(settled),
        won: to_u32(won_n),
        hit_rate_ppm: per(i128::try_from(won_n).unwrap_or(0) * PPM),
        roi_ppm: acc.roi(),
        brier_ppm: acc.brier(),
        edge_ppm: acc.edge(),
        avg_odds_u: per(odds_sum),
        skin_u: to_u64(skin),
        followers_roi_ppm,
        streak: to_u32(streak),
        open_count: to_u32(open.len()),
        open_staked_u: to_u64(open.iter().map(|t| i128::from(t.stake_u)).sum()),
        copies: to_u32(book.copies.iter().filter(|c| is_minted(c.state)).count()),
        fees_earned_u: book.fees_earned_u,
        handle: None,
        verified: false,
        roi_30d_ppm: roi_ppm(skin, returned_30d),
        copiers: to_u32(copiers.len()),
    };
    row.verified = on_board(&row);
    Scored {
        row,
        history,
        legs: acc.legs,
    }
}

/// `sort` of `GET /tipsters`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Sort {
    /// ROI, highest first (the default).
    #[default]
    Roi,
    /// Brier, lowest (best calibrated) first.
    Brier,
    /// Skin, highest first.
    Skin,
    /// Followers' ROI, highest first; tipsters without settled copies last.
    FollowersRoi,
}

impl FromStr for Sort {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "" | "roi" => Ok(Sort::Roi),
            "brier" => Ok(Sort::Brier),
            "skin" => Ok(Sort::Skin),
            "followers_roi" => Ok(Sort::FollowersRoi),
            other => Err(format!(
                "sort={other}: expected roi, brier, skin or followers_roi"
            )),
        }
    }
}

/// Whether a tipster makes the default board: `settled ≥ 5` and `skin ≥ 20 USDC`.
pub fn on_board(row: &TipsterRow) -> bool {
    row.settled >= MIN_SETTLED && row.skin_u >= MIN_SKIN_U
}

/// Orders the board by `sort`. A tipster with no score for the key (nothing settled, no resolved
/// leg for Brier, no settled copy for followers' ROI) goes after every scored one; ties go to more
/// settled tickets, then to the wallet.
pub fn sort_board(rows: &mut [Scored], sort: Sort) {
    rows.sort_by(|a, b| {
        let (ra, rb) = (&a.row, &b.row);
        let key = match sort {
            Sort::Roi => (rb.settled > 0)
                .cmp(&(ra.settled > 0))
                .then(rb.roi_ppm.cmp(&ra.roi_ppm)),
            Sort::Brier => (b.legs > 0)
                .cmp(&(a.legs > 0))
                .then(ra.brier_ppm.cmp(&rb.brier_ppm)),
            Sort::Skin => rb.skin_u.cmp(&ra.skin_u),
            Sort::FollowersRoi => match (ra.followers_roi_ppm, rb.followers_roi_ppm) {
                (Some(x), Some(y)) => y.cmp(&x),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
        };
        key.then(rb.settled.cmp(&ra.settled))
            .then(ra.wallet.cmp(&rb.wallet))
    });
}
