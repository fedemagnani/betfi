//! Typed C4 shapes (`prompts/l1-contracts.md`, "C4. Desk HTTP API"). Amounts are C1 micro-units
//! (`_u`), times are Unix ms (`_ms`) or, for on-chain values, Unix seconds.
//!
//! Decoding is deliberately lenient where C4 allows growth: unknown fields are ignored (a step
//! may add fields), and fields C4 shows as nullable or that a later step may omit are `Option`
//! or `#[serde(default)]`. Enums the UI branches on are strict, with an `Other` fallback where
//! a new value must not break a whole response.

use serde::{Deserialize, Deserializer, Serialize};

/// Which team a leg backs. The UI only ever shows the team's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pick {
    Away,
    Home,
}

/// Pascal side of a leg (D-021: one symbol per game, a home pick is a short). Carried to the
/// desk and the program; never shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Long,
    Short,
}

// ---------------------------------------------------------------- GET /health

/// `GET /health` (the fields the app reads). `profile` is `dev`, `demo` or `live`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Health {
    pub profile: String,
    #[serde(default)]
    pub paused: bool,
}

impl Health {
    /// Hedges really reach Pascal only on the `live` profile; anything else is mock-filled.
    pub fn is_live(&self) -> bool {
        self.profile == "live"
    }
}

// ---------------------------------------------------------------- GET /card

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Card {
    pub sport: String,
    pub date: String,
    pub as_of_ms: i64,
    #[serde(default)]
    pub games: Vec<Game>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Game {
    pub game_id: String,
    pub symbol: String,
    pub start_ms: i64,
    #[serde(default)]
    pub expected_resolution_ms: Option<i64>,
    pub away: Team,
    pub home: Team,
    pub long_team: Pick,
    #[serde(default)]
    pub picks: Vec<CardPick>,
    /// L6-2 (6.0b): how many markets the game has (winner, spreads, totals).
    #[serde(default)]
    pub markets_count: u32,
    /// L6-2 (U-2): the game has started; only its winner market is still offered.
    #[serde(default)]
    pub in_play: bool,
    /// L6-2: when the game started (Unix ms), `null` before.
    #[serde(default)]
    pub started_ms: Option<i64>,
    /// L6-2 (6.0b): every market of the game; empty from a desk before L6.
    #[serde(default)]
    pub markets: Vec<Market>,
    /// L7-1 (7.0b): `basketball` or `soccer`.
    #[serde(default)]
    pub sport: Option<String>,
    /// L7-1 (7.0b): `NBA`, `EPL`, `Bundesliga` or `LaLiga`.
    #[serde(default)]
    pub league: Option<String>,
    /// 31 Oct: a started game's score from the desk's live feed; `null` while it has none.
    #[serde(default)]
    pub score: Option<Score>,
}

/// A live game's score (31 Oct): home and away, the feed's period and clock.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Score {
    pub home: u32,
    pub away: u32,
    /// `1H`, `HT`, `2H` (soccer), `Q1`…`Q4`, `OT` (basketball), …
    #[serde(default)]
    pub period: Option<String>,
    /// Minutes played (soccer) or the game clock (basketball).
    #[serde(default)]
    pub elapsed: Option<String>,
}

impl Game {
    pub fn team(&self, pick: Pick) -> &Team {
        match pick {
            Pick::Away => &self.away,
            Pick::Home => &self.home,
        }
    }

    pub fn pick(&self, pick: Pick) -> Option<&CardPick> {
        self.picks.iter().find(|p| p.pick == pick)
    }

    /// The two teams in the order the screens list them: a EuroLeague game home team first (the
    /// European way, "Real Madrid – Olympiacos" is Olympiacos at Real Madrid), any other away
    /// team first (the NBA's "away @ home").
    pub fn listed(&self) -> [Pick; 2] {
        if self.symbol.starts_with("EL_") || self.game_id.starts_with("el-") {
            [Pick::Home, Pick::Away]
        } else {
            [Pick::Away, Pick::Home]
        }
    }

    /// The game has a winner market to list on Home. Every desk game does; only a design
    /// fixture holds a game with its total alone.
    pub fn offers_winner(&self) -> bool {
        !self.picks.is_empty()
            || self
                .markets
                .iter()
                .any(|m| m.kind == MarketKind::Winner && !m.picks.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Team {
    pub name: String,
    pub abbr: String,
}

/// L6-2: a market's kind. A desk may add kinds; the app shows only the ones it knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarketKind {
    Winner,
    Spread,
    Total,
    #[serde(other)]
    Other,
}

/// L6-2: one market of a game (`/card` `games[].markets[]`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Market {
    pub kind: MarketKind,
    /// The spread or total line as the desk writes it (`"-3.5"`, `"221.5"`); `null` for winner.
    #[serde(default)]
    pub line: Option<String>,
    #[serde(default)]
    pub picks: Vec<MarketPick>,
    /// L7-1 (7.0b): the winner market's title, `"Moneyline"` (NBA) or `"Full Time Result (1X2)"`
    /// (soccer); `null` on spreads and totals and from a desk before L7.
    #[serde(default)]
    pub market_title: Option<String>,
}

/// `team` of a card market pick: `away`/`home` as [`Pick`]; anything else (L7-1's soccer
/// `draw`, or a total's `null`) as `None`, so a new value never breaks the whole card. A draw is
/// the winner pick whose `team` is `None` (its `outcome` is `"X"`).
fn lenient_pick<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Pick>, D::Error> {
    let v: Option<String> = Option::deserialize(d)?;
    Ok(match v.as_deref() {
        Some("away") => Some(Pick::Away),
        Some("home") => Some(Pick::Home),
        _ => None,
    })
}

/// L6-2: one side of a market. `team` is `null` for a total's Over and Under.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MarketPick {
    pub label: String,
    /// `null` for a total's Over/Under and (L7-1, 7.0b) for a soccer draw.
    #[serde(default, deserialize_with = "lenient_pick")]
    pub team: Option<Pick>,
    pub symbol: String,
    pub side: Side,
    pub price_u: u64,
    pub odds_u: u64,
    /// L7-1 (7.0b): a winner pick's outcome, `"1"` (home), `"X"` (draw) or `"2"` (away); `null`
    /// on spreads and totals and from a desk before L7.
    #[serde(default)]
    pub outcome: Option<String>,
}

/// Indicative price and odds of one pick (no margin; the Tray's quote is binding).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CardPick {
    pub pick: Pick,
    pub side: Side,
    pub price_u: u64,
    pub odds_u: u64,
    /// L7-1 (7.0b): `"1"` (home) or `"2"` (away).
    #[serde(default)]
    pub outcome: Option<String>,
}

// ---------------------------------------------------------------- POST /quote

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuoteRequest {
    pub legs: Vec<QuoteRequestLeg>,
    pub stake_u: u64,
    pub token_in: Option<String>,
    pub copied_from: Option<String>,
    /// L9: the tip's name, sent only on the last quote before signing, with `wallet`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wallet: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuoteRequestLeg {
    pub symbol: String,
    pub side: Side,
}

/// A signed desk quote. `quote_msg` / `quote_sig` are carried into the transaction untouched.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Quote {
    pub n: u8,
    pub stake_u: u64,
    pub legs: Vec<QuoteLeg>,
    pub odds_fair_u: u64,
    pub odds_offered_u: u64,
    pub payout_u: u64,
    pub fee_u: u64,
    #[serde(default)]
    pub pascal_fees_est_u: u64,
    #[serde(default)]
    pub copy_fee_u: u64,
    pub slippage_bps: u32,
    pub min_payout_per_unit_u: u64,
    /// Unix seconds (on-chain clock), checked as `now <= quote_expires_at`.
    pub quote_expires_at: i64,
    pub quote_signer: String,
    pub quote_msg: String,
    pub quote_sig: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct QuoteLeg {
    pub symbol: String,
    pub game_id: String,
    pub side: Side,
    pub pick: Pick,
    pub start_ms: i64,
    pub price_u: u64,
    pub p_max_u: u64,
    /// L6-2 (U-2): a winner leg on a game already in play.
    #[serde(default)]
    pub in_play: bool,
    pub m1_ppm: u32,
    pub overlaps: bool,
}

// ---------------------------------------------------------------- tickets

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TicketsResponse {
    pub user: String,
    #[serde(default)]
    pub tickets: Vec<Ticket>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketState {
    Pending,
    Open,
    Settled,
    CashedOut,
    Refunded,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Ticket {
    /// Ticket PDA, base58.
    pub ticket: String,
    pub user: String,
    pub nonce: u64,
    pub state: TicketState,
    pub n: u8,
    pub stake_u: u64,
    pub payout_u: u64,
    pub odds_offered_u: u64,
    #[serde(default)]
    pub odds_fair_u: Option<u64>,
    #[serde(default)]
    pub fee_u: Option<u64>,
    /// Unix seconds.
    #[serde(default)]
    pub minted_at: Option<i64>,
    #[serde(default)]
    pub settled_at: Option<i64>,
    #[serde(default)]
    pub paid_u: Option<u64>,
    #[serde(default)]
    pub cash_out: Option<CashOut>,
    #[serde(default)]
    pub legs: Vec<TicketLeg>,
    #[serde(default)]
    pub fills: Vec<Fill>,
    #[serde(default)]
    pub tx: TicketTx,
    /// The tipster of an own tip, or of the ticket a copy was taken from, base58; `null` for a
    /// plain ticket (C4 amendment, L2-4).
    #[serde(default)]
    pub tipster: Option<String>,
    /// The original Ticket PDA when this ticket is a copy, base58 (C4 amendment, L2-4).
    #[serde(default)]
    pub copied_from: Option<String>,
    /// L6-2 (6.0c): the tipster's title for an own tip ("Thursday treble"), `null` otherwise.
    #[serde(default)]
    pub title: Option<String>,
    /// L6-2 (6.0c): the odds at mint, for "were 6.98 when posted".
    #[serde(default)]
    pub posted_odds_u: Option<u64>,
    /// L7-1 (7.0c): Σ stake of the minted copies of this tip ("Pool"); only on the tips of
    /// `GET /tipsters/{wallet}`.
    #[serde(default)]
    pub pool_u: Option<u64>,
    /// L7-1: minted copies of this tip.
    #[serde(default)]
    pub copies: Option<u32>,
    /// L7-1: each leg's short team code in leg order (`["PHI","TOR"]`); empty when not sent.
    #[serde(default)]
    pub teams: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CashOut {
    pub available: bool,
    #[serde(default)]
    pub c_u: Option<u64>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegStatus {
    Open,
    Won,
    Lost,
    Half,
    Void,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TicketLeg {
    pub idx: u8,
    pub symbol: String,
    pub game_id: String,
    /// Team names.
    pub away: String,
    pub home: String,
    pub pick: Pick,
    pub side: Side,
    pub start_ms: i64,
    pub p_quote_u: u64,
    #[serde(default)]
    pub p_fill_u: Option<u64>,
    #[serde(default)]
    pub hedged: bool,
    pub status: LegStatus,
    #[serde(default)]
    pub resolution_u: Option<u64>,
    /// Proposed (6.0a, D-312): the leg's market kind, line and label ("Real Madrid -3.5",
    /// "Over 221.5"), so a receipt can name a spread or total leg after its game left the
    /// card. Absent from desks before L6: a winner leg is named by [`TicketLeg::picked_team`].
    #[serde(default)]
    pub kind: Option<MarketKind>,
    #[serde(default)]
    pub line: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

impl TicketLeg {
    /// A leg the desk could not name yet: it holds only the chain's hashes (a bet met on chain
    /// by a desk without its quote; the desk names it once Pascal lists the market, D-573).
    pub fn is_unnamed(&self) -> bool {
        self.symbol.starts_with("sha256:") || (self.home.is_empty() && self.away.is_empty())
    }

    pub fn picked_team(&self) -> &str {
        match self.pick {
            Pick::Away => &self.away,
            Pick::Home => &self.home,
        }
    }

    pub fn opponent(&self) -> &str {
        match self.pick {
            Pick::Away => &self.home,
            Pick::Home => &self.away,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Venue {
    Mock,
    Pascal,
    #[serde(other)]
    Other,
}

/// One hedge fill. `side` is Pascal's book side and is never shown.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Fill {
    pub trade_id: String,
    pub symbol: String,
    #[serde(default)]
    pub side: String,
    pub price_u: u64,
    pub size: u64,
    /// Taker fee; negative is a maker rebate.
    #[serde(default)]
    pub fee_u: i64,
    #[serde(default)]
    pub builder_fee_u: u64,
    pub venue: Venue,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct TicketTx {
    #[serde(default)]
    pub open_pending: Option<String>,
    #[serde(default)]
    pub finalize: Option<String>,
    #[serde(default)]
    pub settle: Option<String>,
    /// C4 amendment (29 Oct).
    #[serde(default)]
    pub refund_pending: Option<String>,
    /// C4 amendment (29 Oct).
    #[serde(default)]
    pub cash_out: Option<String>,
    /// L4-2: every `resolve_leg` signature, in leg-resolution order (not leg order); `[]` when
    /// none.
    #[serde(default)]
    pub resolve_leg: Vec<String>,
    /// L7-1 (7.0c): the slot of each transaction as the desk's indexer saw it.
    #[serde(default)]
    pub slot: Option<TxSlots>,
    /// L7-1: block time to the indexer's observation, ms (approximate; `null` when unknown).
    #[serde(default)]
    pub confirm_ms: Option<TxConfirm>,
}

/// L7-1: `Ticket.tx.slot`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct TxSlots {
    #[serde(default)]
    pub open_pending: Option<u64>,
    #[serde(default)]
    pub cash_out: Option<u64>,
    #[serde(default)]
    pub settle: Option<u64>,
}

/// L7-1: `Ticket.tx.confirm_ms`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct TxConfirm {
    #[serde(default)]
    pub open_pending: Option<u64>,
    #[serde(default)]
    pub cash_out: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CashoutQuote {
    pub ticket: String,
    pub c_u: u64,
    /// Unix seconds.
    pub ts: i64,
    pub quote_signer: String,
    pub quote_msg: String,
    pub quote_sig: String,
}

/// `POST /ticket/{id}/copy-quote` body: the copier's stake (the legs are the original's).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CopyQuoteRequest {
    pub stake_u: u64,
}

// ---------------------------------------------------------------- tipsters (L4-2)

/// `GET /tipsters?sort=&all=`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TipstersResponse {
    #[serde(default)]
    pub as_of_ms: i64,
    #[serde(default)]
    pub tipsters: Vec<TipsterRow>,
}

/// One tipster's record, own tips only (spec 6, "Tipster reputation"). Rates that can be
/// negative are signed.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TipsterRow {
    pub wallet: String,
    #[serde(default)]
    pub settled: u32,
    #[serde(default)]
    pub won: u32,
    #[serde(default)]
    pub hit_rate_ppm: u32,
    #[serde(default)]
    pub roi_ppm: i64,
    #[serde(default)]
    pub brier_ppm: u32,
    #[serde(default)]
    pub edge_ppm: i64,
    #[serde(default)]
    pub avg_odds_u: u64,
    /// Own money staked over the last 30 days.
    #[serde(default)]
    pub skin_u: u64,
    /// `null` until a copy of this tipster's tips settles.
    #[serde(default)]
    pub followers_roi_ppm: Option<i64>,
    #[serde(default)]
    pub streak: u32,
    #[serde(default)]
    pub open_count: u32,
    #[serde(default)]
    pub open_staked_u: u64,
    #[serde(default)]
    pub copies: u32,
    #[serde(default)]
    pub fees_earned_u: u64,
    /// L6-2 (6.0c): the wallet's chosen handle (without `@`), `null` when it has none.
    #[serde(default)]
    pub handle: Option<String>,
    /// L6-2 (U-5): on the leaderboard (5 settled own tips and €20 of own money).
    #[serde(default)]
    pub verified: bool,
    /// L6-2 (6.0c): return on own tips settled in the last 30 days.
    #[serde(default)]
    pub roi_30d_ppm: i64,
    /// L6-2 (6.0c): distinct wallets that copied one of this tipster's tips.
    #[serde(default)]
    pub copiers: u32,
}

/// `GET /tipsters/{wallet}`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TipsterProfile {
    pub tipster: TipsterRow,
    #[serde(default)]
    pub history: Vec<HistoryPoint>,
    #[serde(default)]
    pub settled: Vec<Ticket>,
    #[serde(default)]
    pub open: Vec<Ticket>,
}

/// The record after each settled own tip, oldest first.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HistoryPoint {
    pub ticket: String,
    /// Unix seconds.
    #[serde(default)]
    pub settled_at: i64,
    pub roi_cum_ppm: i64,
    pub brier_cum_ppm: u32,
}

/// Leaderboard order (`sort=`); the desk sorts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TipsterSort {
    #[default]
    Roi,
    Brier,
    Skin,
    FollowersRoi,
}

impl TipsterSort {
    pub const ALL: [TipsterSort; 4] = [
        TipsterSort::Roi,
        TipsterSort::Brier,
        TipsterSort::Skin,
        TipsterSort::FollowersRoi,
    ];

    pub fn param(self) -> &'static str {
        match self {
            TipsterSort::Roi => "roi",
            TipsterSort::Brier => "brier",
            TipsterSort::Skin => "skin",
            TipsterSort::FollowersRoi => "followers_roi",
        }
    }
}

// ---------------------------------------------------------------- GET /house (L4-2)

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct House {
    #[serde(default)]
    pub as_of_ms: i64,
    #[serde(default)]
    pub profile: String,
    #[serde(default)]
    pub vault: HouseVault,
    #[serde(default)]
    pub tickets: HouseTickets,
    pub hedger: Hedger,
    #[serde(default)]
    pub fees: HouseFees,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct HouseVault {
    #[serde(default)]
    pub free_u: u64,
    #[serde(default)]
    pub locked_u: u64,
    #[serde(default)]
    pub reserve_u: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct HouseTickets {
    #[serde(default)]
    pub pending: u64,
    #[serde(default)]
    pub open: u64,
    #[serde(default)]
    pub settled: u64,
    #[serde(default)]
    pub cashed_out: u64,
    #[serde(default)]
    pub refunded: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Hedger {
    pub venue: Venue,
    #[serde(default)]
    pub free_collateral_u: u64,
    #[serde(default)]
    pub positions: Vec<Position>,
}

/// The hedger's net position on one market, signed contracts (positive backs the team the
/// market's "yes" side is on, `Game::long_team`, D-021). Shown with team names only.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Position {
    pub symbol: String,
    #[serde(default)]
    pub held: i64,
    #[serde(default)]
    pub target: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct HouseFees {
    #[serde(default)]
    pub margin_u: u64,
    #[serde(default)]
    pub pascal_fees_u: u64,
    #[serde(default)]
    pub builder_fees_u: u64,
    #[serde(default)]
    pub copy_fees_u: u64,
}

/// The body of a 422 / 503.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ErrorBody {
    pub status: String,
    #[serde(default)]
    pub reason: String,
}

// ---------------------------------------------------------------- WS /stream

/// Client -> server.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Subscribe {
    pub subscribe: SubscribeUser,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SubscribeUser {
    pub user: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Escrowed,
    Hedging,
    Minted,
    Refunded,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedKind {
    Fill,
    Mint,
    Resolve,
    Settle,
    Cashout,
    #[serde(other)]
    Other,
}

/// Server -> client, one JSON object per text frame, tagged by `type`. A frame of an unknown
/// `type` decodes to [`Frame::Unknown`] and is ignored.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frame {
    Heartbeat {
        ts_ms: i64,
    },
    Progress {
        pending: String,
        stage: Stage,
        /// C4 leaves the element shape open; decoded per element by [`fills_of`].
        #[serde(default)]
        fills: Vec<serde_json::Value>,
        #[serde(default)]
        ticket: Option<String>,
    },
    Ticket {
        ticket: Box<Ticket>,
    },
    Feed {
        kind: FeedKind,
        text: String,
        #[serde(default)]
        ts_ms: i64,
    },
    #[serde(other)]
    Unknown,
}

/// L7-1 (7.0c): a `feed` frame with the ticker fields, decoded from the same JSON text as
/// [`Frame`] (whose `Feed` variant predates them): "<who> placed <amount> on <team> @ <odds>".
/// `who`, `amount_u`, `team` and `odds_u` are set on kinds `mint` and `copy`.
#[allow(dead_code)] // L7-1 (7.0c): decoded by the v2 screens, not yet by this build
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FeedLine {
    pub kind: FeedLineKind,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub ts_ms: i64,
    /// The handle, else a short address (`7xK9…94Lp`).
    #[serde(default)]
    pub who: Option<String>,
    /// Stake, micro-USDC.
    #[serde(default)]
    pub amount_u: Option<u64>,
    /// The first leg's pick label.
    #[serde(default)]
    pub team: Option<String>,
    #[serde(default)]
    pub odds_u: Option<u64>,
}

/// L7-1: [`FeedKind`] plus `copy` (a minted copy of a tip).
#[allow(dead_code)] // L7-1 (7.0c): decoded by the v2 screens, not yet by this build
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedLineKind {
    Fill,
    Mint,
    Copy,
    Resolve,
    Settle,
    Cashout,
    #[serde(other)]
    Other,
}

/// The progress frame's fills that decode as C4 [`Fill`]s; anything else is skipped.
pub fn fills_of(raw: &[serde_json::Value]) -> Vec<Fill> {
    raw.iter()
        .filter_map(|v| Fill::deserialize(v).ok())
        .collect()
}

// ---------------------------------------------------------------- swap (L4-3)

/// One `GET /tokens` row: mock USDC first (`usdc_mint: true`), then every mock mint with a pool.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TokenInfo {
    pub mint: String,
    pub symbol: String,
    pub decimals: u8,
    pub usdc_mint: bool,
}

/// `GET /tokens`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TokensResponse {
    #[serde(default)]
    pub tokens: Vec<TokenInfo>,
}

/// `POST /swap-quote` body: base units of `mint` to sell, as a decimal string.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SwapQuoteRequest {
    pub mint: String,
    pub amount_in: String,
}

/// `POST /swap-quote` (L4-3, Jupiter-like roles): the pool's USDC output, its 0.5% fee, and
/// `min_out_u`, which is both the swap's threshold and the stake the desk is quoted for.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SwapQuote {
    pub mint: String,
    pub amount_in: String,
    pub usdc_out_u: u64,
    pub fee_u: u64,
    pub min_out_u: u64,
    pub slippage_bps: u32,
    pub route: String,
    pub pool: String,
}

// ---------------------------------------------------------------- GET /sentiment (L5-4)

/// `GET /sentiment?symbols=…`: what others think each game's result will be.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SentimentResponse {
    pub as_of_ms: i64,
    /// Fewer people than this on Pascal: "too few traders yet".
    #[serde(default)]
    pub min_holders: u64,
    #[serde(default)]
    pub games: Vec<GameSentiment>,
}

impl SentimentResponse {
    pub fn game(&self, game_id: &str) -> Option<&GameSentiment> {
        self.games.iter().find(|g| g.game_id == game_id)
    }
}

/// One game: four signals, each split by team (away first).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GameSentiment {
    pub game_id: String,
    pub symbol: String,
    /// Each team's mid price, the reference line.
    pub price: AwayHome,
    pub pascal_holders: PascalHolders,
    /// What people paid to back each team on Pascal in the last 24 h (micro-USD).
    pub flow_24h: AwayHome,
    pub schedina: SchedinaStake,
    /// L6-2 (6.0c): notional traded on Pascal since the market opened, per team (micro-USD).
    #[serde(default)]
    pub traded_all_time: Option<TradedTotals>,
    /// L7-1 (7.0c): non-maker Pascal holders plus Schedina players on the game.
    #[serde(default)]
    pub traders: Option<u64>,
    /// L7-1: the newest stakes on the game, newest first, at most 10.
    #[serde(default)]
    pub recent: Vec<RecentStake>,
}

/// L7-1: one recent stake on a game.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RecentStake {
    /// Micro-USD (Pascal) or micro-USDC (Schedina).
    #[serde(default)]
    pub amount_u: u64,
    /// `away` or `home`.
    #[serde(default)]
    pub team: String,
    #[serde(default)]
    pub ts_ms: u64,
    /// `pascal` or `schedina`.
    #[serde(default)]
    pub source: String,
}

/// L6-2: all-time traded notional on a game's winner market, per team (micro-USD).
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
pub struct TradedTotals {
    #[serde(default)]
    pub away_u: u64,
    #[serde(default)]
    pub home_u: u64,
    #[serde(default)]
    pub total_u: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
pub struct AwayHome {
    #[serde(default)]
    pub away_u: u64,
    #[serde(default)]
    pub home_u: u64,
}

/// People holding each team on Pascal (market makers left out by the desk).
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
pub struct PascalHolders {
    pub away: TeamHolders,
    pub home: TeamHolders,
    #[serde(default)]
    pub excluded_makers: u64,
    #[serde(default)]
    pub trades_seen: u64,
    #[serde(default)]
    pub synced: bool,
    /// Too few people (or still loading): no bar, "too few traders yet".
    #[serde(default)]
    pub thin: bool,
}

impl PascalHolders {
    pub fn people(&self) -> u64 {
        self.away.accounts + self.home.accounts
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
pub struct TeamHolders {
    #[serde(default)]
    pub accounts: u64,
    /// Micro-USD.
    #[serde(default)]
    pub stake_u: u64,
}

/// Schedina players' stake per pick on open and pending schedine (micro-USDC).
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
pub struct SchedinaStake {
    #[serde(default)]
    pub away_u: u64,
    #[serde(default)]
    pub home_u: u64,
    #[serde(default)]
    pub tickets: u64,
}

/// An away/home split as whole percents (rounded, summing to 100), or `None` with nothing on
/// either side. Display only.
pub fn split_pct(away: u64, home: u64) -> Option<(u64, u64)> {
    let total = u128::from(away) + u128::from(home);
    if total == 0 {
        return None;
    }
    let a = ((u128::from(away) * 100 + total / 2) / total) as u64;
    Some((a, 100 - a))
}

// ---------------------------------------------------------------- L6-2: FX and profiles

/// `GET /fx` (U-4): euros per US dollar in ppm; USDC is taken at 1 USD.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Fx {
    pub eur_per_usd_ppm: u64,
    #[serde(default)]
    pub as_of: String,
    #[serde(default)]
    pub source: String,
    /// The desk could not reach the ECB rate and serves its fixed fallback.
    #[serde(default)]
    pub fallback: bool,
    /// L7-1 (7.0c): euros per SOL in ppm (Pyth SOL/USD × the ECB rate), for "≈ SOL".
    #[serde(default)]
    pub eur_per_sol_ppm: Option<u64>,
    /// L7-1: Pyth publish time of the SOL price, Unix ms (0 for the fallback).
    #[serde(default)]
    pub sol_as_of_ms: Option<u64>,
    /// L7-1: `pyth` or `fallback`.
    #[serde(default)]
    pub sol_source: Option<String>,
    /// L7-1: the SOL price is the desk's fixed fallback ("approx.").
    #[serde(default)]
    pub sol_fallback: Option<bool>,
}

/// `GET /profile/{wallet}` (6.0c): the wallet's handle, `null` when it has none.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Profile {
    pub wallet: String,
    #[serde(default)]
    pub handle: Option<String>,
    /// Unix seconds of the last change, 0 when never set.
    #[serde(default)]
    pub since: i64,
}

/// `PUT /profile` body (L6-2, 6.1e): `sig` is base64 ed25519 by `wallet` over
/// `ui::tipsters::profile_msg::message(wallet, handle, ticket, title, ts)`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProfileUpdate {
    pub wallet: String,
    pub handle: String,
    pub title: Option<String>,
    pub ticket: Option<String>,
    pub ts: i64,
    pub sig: String,
}

// ---------------------------------------------------------------- L7-1: profiles v2, copiers

/// L7-1 (7.0c): `GET /profile/{wallet}` with the v2 fields. The same JSON as [`Profile`] (kept
/// as it is for its callers); decode this one to read links, follows and `verified`.
#[allow(dead_code)] // L7-1 (7.0c): decoded by the v2 screens, not yet by this build
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PublicProfile {
    pub wallet: String,
    #[serde(default)]
    pub handle: Option<String>,
    #[serde(default)]
    pub since: i64,
    /// On the tipster board (PRO badge, D-304).
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub links: Option<ProfileLinks>,
    #[serde(default)]
    pub followers: Option<u64>,
    #[serde(default)]
    pub following: Option<u64>,
}

/// L7-1: social links, bare handles or usernames (never URLs); `null` = none.
#[allow(dead_code)] // L7-1 (7.0c): decoded by the v2 screens, not yet by this build
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileLinks {
    #[serde(default)]
    pub x: Option<String>,
    #[serde(default)]
    pub youtube: Option<String>,
    #[serde(default)]
    pub telegram: Option<String>,
    #[serde(default)]
    pub instagram: Option<String>,
}

/// L7-1: `PUT /profile` `follow`.
#[allow(dead_code)] // L7-1 (7.0c): decoded by the v2 screens, not yet by this build
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FollowRequest {
    pub wallet: String,
    pub on: bool,
}

/// L7-1: `PUT /profile` body with `links` or `follow`, signed over the desk's v2 message
/// (`desk::profiles::message_v2`: `betfi/profile/v2\n<wallet>\n<handle or ->\n<ticket or ->\n
/// <title or ->\n<links or ->\n<follow or ->\n<ts>`). `handle` may be empty only on a
/// follow-only request; `links` replaces all four.
#[allow(dead_code)] // L7-1 (7.0c): decoded by the v2 screens, not yet by this build
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProfileUpdateV2 {
    pub wallet: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub handle: String,
    pub title: Option<String>,
    pub ticket: Option<String>,
    pub ts: i64,
    pub sig: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<ProfileLinks>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follow: Option<FollowRequest>,
}

/// L7-1: `GET /tipsters/{wallet}/copiers?ticket=<pda>`.
#[allow(dead_code)] // L7-1 (7.0c): decoded by the v2 screens, not yet by this build
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CopiersResponse {
    pub ticket: String,
    /// Newest first.
    #[serde(default)]
    pub copiers: Vec<Copier>,
    #[serde(default)]
    pub pool_u: u64,
}

// ---------------------------------------------------------------- /relay (L8-2)

/// `GET /relay` (L8-2): the desk sponsors transactions. `sponsor` (base58) is the fee payer and
/// rent payer of every sponsored transaction; the desk refuses one costing it more than
/// `max_lamports_per_tx`. Answered `503` when sponsoring is off.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RelayInfo {
    pub sponsor: String,
    #[serde(default)]
    pub max_lamports_per_tx: u64,
}

/// `POST /relay` body: the user-signed transaction, standard base64 of its bincode
/// `VersionedTransaction` (fee payer slot 0 zeroed, the user's slot signed).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RelayRequest {
    pub tx: String,
}

/// `POST /relay` answer once the RPC accepted the transaction.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RelayResponse {
    pub signature: String,
}

/// L7-1: one copy of a tip; the row opens Solscan on `tx`.
#[allow(dead_code)] // L7-1 (7.0c): decoded by the v2 screens, not yet by this build
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Copier {
    pub wallet: String,
    #[serde(default)]
    pub handle: Option<String>,
    #[serde(default)]
    pub stake_u: u64,
    #[serde(default)]
    pub tx: Option<String>,
}
