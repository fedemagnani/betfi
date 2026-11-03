//! The desk's public JSON, exactly as frozen in `prompts/l1-contracts.md` C4. `fixtures/desk/*.json`
//! round-trip through these types in `tests/fixtures.rs`, so the fixtures the app (1.F, 2.E) codes
//! against cannot drift from the code.
//!
//! Units follow C1: `*_u` micro-units (USDC, prices, odds), `*_ms` Unix ms, `*_at` / `ts` Unix s.
//! A later step may add a field (C4); it never renames or removes one without a decision entry.

use serde::{Deserialize, Serialize};

// --- shared enums -------------------------------------------------------------------------------

/// Which team a pick or a market's long side is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamSide {
    /// The away team (first in an NBA game id, `nba-<away>-<home>-<date>`; second in a soccer
    /// one, `epl-<home>-<away>-<date>`).
    Away,
    /// The home team.
    Home,
}

impl TeamSide {
    /// The other team.
    pub fn other(self) -> Self {
        match self {
            TeamSide::Away => TeamSide::Home,
            TeamSide::Home => TeamSide::Away,
        }
    }
}

/// The team a card pick backs, with the draw of a three-way (1-X-2) market (l7-contracts L7-1).
/// On the wire `away`, `home` or `draw`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickTeam {
    /// The away team.
    Away,
    /// The home team.
    Home,
    /// Neither: the game ends level (soccer only).
    Draw,
}

impl From<TeamSide> for PickTeam {
    fn from(t: TeamSide) -> Self {
        match t {
            TeamSide::Away => PickTeam::Away,
            TeamSide::Home => PickTeam::Home,
        }
    }
}

impl PickTeam {
    /// The pick's 1-X-2 code (L7-1): 1 = home, X = draw, 2 = away.
    pub fn outcome(self) -> Outcome {
        match self {
            PickTeam::Home => Outcome::Home,
            PickTeam::Draw => Outcome::Draw,
            PickTeam::Away => Outcome::Away,
        }
    }
}

/// A winner pick's 1-X-2 code (l7-contracts L7-1), for the NBA too: `"1"` home, `"X"` draw,
/// `"2"` away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Outcome {
    /// `"1"`: the home team wins.
    #[serde(rename = "1")]
    Home,
    /// `"X"`: a draw.
    #[serde(rename = "X")]
    Draw,
    /// `"2"`: the away team wins.
    #[serde(rename = "2")]
    Away,
}

/// The sport of a card game (l7-contracts L7-1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sport {
    /// NBA basketball: two-way winner, spreads and totals.
    #[default]
    Basketball,
    /// Soccer: the three-way full-time result (1-X-2) only (D-435).
    Soccer,
}

impl Sport {
    /// Lowercase name, as on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Sport::Basketball => "basketball",
            Sport::Soccer => "soccer",
        }
    }
}

/// The league of a card game (l7-contracts L7-1), from the Pascal symbol's prefix.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum League {
    /// `NBA_`.
    #[default]
    #[serde(rename = "NBA")]
    Nba,
    /// English Premier League, `EPL_`.
    #[serde(rename = "EPL")]
    Epl,
    /// Bundesliga, `BUND_`.
    Bundesliga,
    /// LaLiga, `LALIGA_`.
    LaLiga,
    /// UEFA Champions League, `UCL_` (3 Nov: Pascal lists it, often the only soccer of a week
    /// night).
    #[serde(rename = "Champions League")]
    Ucl,
}

impl League {
    /// The league's sport.
    pub fn sport(self) -> Sport {
        match self {
            League::Nba => Sport::Basketball,
            League::Epl | League::Bundesliga | League::LaLiga | League::Ucl => Sport::Soccer,
        }
    }

    /// The name on the wire (`"NBA"`, `"EPL"`, `"Bundesliga"`, `"LaLiga"`, `"Champions League"`).
    pub fn as_str(self) -> &'static str {
        match self {
            League::Nba => "NBA",
            League::Epl => "EPL",
            League::Bundesliga => "Bundesliga",
            League::LaLiga => "LaLiga",
            League::Ucl => "Champions League",
        }
    }
}

/// Side of a leg on its Pascal symbol: long buys the symbol's yes, short sells it (D-021).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// Long the symbol (the team in `outcome_labels.long`).
    Long,
    /// Short the symbol, at `1 - p`.
    Short,
}

/// Side of a Pascal order or fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderSide {
    /// Buy.
    Bid,
    /// Sell.
    Ask,
}

/// Where a fill happened (D-007): real Pascal (mode R) or `pascal-mock` (mode M).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Venue {
    /// Real Pascal.
    Pascal,
    /// The mock's matcher ("mock-filled").
    Mock,
}

impl Venue {
    /// The `pascal_fills.venue` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Venue::Pascal => "pascal",
            Venue::Mock => "mock",
        }
    }
}

// --- errors -------------------------------------------------------------------------------------

/// `status` of an error body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorStatus {
    /// HTTP 422: the request is understood and refused (correlation rule, bad params, ...).
    Rejected,
    /// HTTP 503: the desk cannot answer now (stale books, Pascal down, paused).
    Unavailable,
    /// HTTP 404: no such ticket. Not in C4; added by 1.E (proposed D-1E-c).
    NotFound,
}

/// Error body: `{"status":"rejected","reason":"..."}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// Kind of error.
    pub status: ErrorStatus,
    /// Human-readable reason, shown in the app.
    pub reason: String,
}

// --- GET /card ----------------------------------------------------------------------------------

/// `GET /card?sport=nba&date=YYYY-MM-DD`; `sport` may also be `basketball` or `soccer`, or left
/// out for both (l7-contracts L7-1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardResponse {
    /// The `sport` asked for (`nba`, `basketball` or `soccer`), `all` when left out.
    pub sport: String,
    /// The requested US date, matched against the game id's date suffix; empty when none was
    /// asked for (every game not over, whatever its day).
    pub date: String,
    /// `state_time_ms` of the `/markets` response the card was built from.
    pub as_of_ms: u64,
    /// Games of that date (or of every date), by start time.
    pub games: Vec<CardGame>,
}

/// One game on the card. On an NBA game both `picks` come from the game's one moneyline symbol
/// (D-021) and `markets` adds its spreads and totals (6.0b). A soccer game (7.0b) has a single
/// market, the 1-X-2 winner, whose three picks are each a long on their own symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardGame {
    /// `display_attributes.game_page.game_id`.
    pub game_id: String,
    /// The moneyline symbol (a soccer game's home-win symbol).
    pub symbol: String,
    /// `expected_event_start_time_ms`.
    pub start_ms: u64,
    /// `expected_resolution_time_ms`.
    pub expected_resolution_ms: u64,
    /// Away team.
    pub away: Team,
    /// Home team.
    pub home: Team,
    /// The team the symbol's long side is on (`home` on a soccer game).
    pub long_team: TeamSide,
    /// Away pick first, then home. Empty on a soccer game, whose picks are on three symbols:
    /// read `markets[0]`.
    pub picks: Vec<CardPick>,
    /// Markets the game offers now: every listed winner, spread and total market before the
    /// start, the winner alone once in play (l6-contracts L6-2, 6.0b); 1 on a soccer game.
    #[serde(default)]
    pub markets_count: u32,
    /// Whether the game has started (venue clock): only its winner market is offered (U-2).
    #[serde(default)]
    pub in_play: bool,
    /// `start_ms` once the game has started, `null` before.
    #[serde(default)]
    pub started_ms: Option<u64>,
    /// The winner market, then a few spread and total lines (proposed D-320).
    #[serde(default)]
    pub markets: Vec<CardMarket>,
    /// `basketball` or `soccer` (l7-contracts L7-1).
    #[serde(default)]
    pub sport: Sport,
    /// `NBA`, `EPL`, `Bundesliga` or `LaLiga` (l7-contracts L7-1).
    #[serde(default)]
    pub league: League,
    /// A started game's score from the live feed ([`crate::scores`], 31 Oct); omitted before the
    /// start and while the feed has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<GameScore>,
}

/// A live game's score ([`crate::scores`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameScore {
    /// Home team's goals or points.
    pub home: u32,
    /// Away team's goals or points.
    pub away: u32,
    /// The feed's period: `1H`, `HT`, `2H` (soccer), `Q1`…`Q4`, `OT` (basketball), …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period: Option<String>,
    /// Minutes played (soccer) or the game clock (basketball), as the feed gives it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed: Option<String>,
}

/// Kind of market behind a card market or a quote leg (l6-contracts L6-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarketKind {
    /// Full-game moneyline: who wins (two-way on the NBA, 1-X-2 on soccer).
    Winner,
    /// Full-game point spread.
    Spread,
    /// Full-game total points.
    Total,
}

impl MarketKind {
    /// Lowercase name, as on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            MarketKind::Winner => "winner",
            MarketKind::Spread => "spread",
            MarketKind::Total => "total",
        }
    }
}

/// One market of a game on the card: one Pascal symbol and its two outcomes as picks, or a
/// soccer winner's three outcome symbols.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardMarket {
    /// Winner, spread or total.
    pub kind: MarketKind,
    /// The long side's line: the spread team's handicap (`"-3.5"`) or the total (`"221.5"`);
    /// `null` for a winner.
    pub line: Option<String>,
    /// Both outcomes: away then home (winner, spread), Over then Under (total); a soccer
    /// winner's three: home, draw, away (L7-1).
    pub picks: Vec<CardMarketPick>,
    /// A winner's title: `"Moneyline"` (NBA) or `"Full Time Result (1X2)"` (soccer), L7-1;
    /// omitted on spreads and totals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub market_title: Option<String>,
}

/// One outcome of a card market.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardMarketPick {
    /// Short label, e.g. "76ers", "Pacers +3.5", "Over 221.5", "Draw".
    pub label: String,
    /// The team it backs (`draw` for a soccer draw, L7-1); `null` on a total.
    pub team: Option<PickTeam>,
    /// Pascal symbol (both picks of a two-way market share it; each soccer outcome has its own).
    pub symbol: String,
    /// The side taken on the symbol (always `long` on soccer).
    pub side: Side,
    /// Indicative price, as in [`CardPick`].
    pub price_u: u64,
    /// `floor(1e12 / price_u)`, no margin.
    pub odds_u: u64,
    /// A winner pick's 1-X-2 code (`"1"` home, `"X"` draw, `"2"` away, L7-1); omitted on
    /// spreads and totals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
}

/// Team names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Team {
    /// Full name, e.g. "Philadelphia 76ers".
    pub name: String,
    /// Abbreviation, e.g. "PHI".
    pub abbr: String,
}

/// One button of the card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardPick {
    /// The team picked.
    pub pick: TeamSide,
    /// The side taken on the symbol.
    pub side: Side,
    /// Indicative price: ask for long, `1e6 - bid` for short, `mark_price` without a book.
    pub price_u: u64,
    /// `floor(1e12 / price_u)`, no margin.
    pub odds_u: u64,
    /// The pick's 1-X-2 code: `"2"` away, `"1"` home (L7-1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
}

// --- POST /quote --------------------------------------------------------------------------------

/// `POST /quote` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuoteRequest {
    /// Legs in ticket order.
    pub legs: Vec<QuoteLegRequest>,
    /// Stake, micro-USDC.
    pub stake_u: u64,
    /// Swap-adapter input mint (v1); `null` = USDC.
    #[serde(default)]
    pub token_in: Option<String>,
    /// Ticket being copied (v1 copy-quote); `null` otherwise.
    #[serde(default)]
    pub copied_from: Option<String>,
    /// The tip's name (L9): applied to the own tip `wallet` opens with this quote. Needs
    /// `wallet`; refused with `copied_from`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The wallet that will sign (base58); sent with `title`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wallet: Option<String>,
}

/// One requested leg.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuoteLegRequest {
    /// Pascal symbol.
    pub symbol: String,
    /// Long or short on it.
    pub side: Side,
}

/// `POST /quote` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuoteResponse {
    /// Number of legs.
    pub n: u8,
    /// Stake, micro-USDC.
    pub stake_u: u64,
    /// Priced legs, in ticket order.
    pub legs: Vec<QuoteLeg>,
    /// Fair odds (m1 = 0, mid prices).
    pub odds_fair_u: u64,
    /// Offered odds, `floor(payout_u * 1e6 / stake_u)`.
    pub odds_offered_u: u64,
    /// Offered payout at r = 1 (C2).
    pub payout_u: u64,
    /// Fair payout minus offered payout (the margin in money).
    pub fee_u: u64,
    /// Estimated Pascal taker fees of the hedge.
    pub pascal_fees_est_u: u64,
    /// Copy fee (v1), 0 otherwise.
    pub copy_fee_u: u64,
    /// Slippage allowance between quote and floor.
    pub slippage_bps: u32,
    /// The floor the user signs into `open_pending`.
    pub min_payout_per_unit_u: u64,
    /// Quote expiry, Unix s (10 s TTL).
    pub quote_expires_at: i64,
    /// Quote-signer pubkey, base58.
    pub quote_signer: String,
    /// The C3 quote message, base64.
    pub quote_msg: String,
    /// Ed25519 signature over `quote_msg`, base64 (64 bytes).
    pub quote_sig: String,
}

/// One priced leg of a quote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuoteLeg {
    /// Pascal symbol.
    pub symbol: String,
    /// Game id.
    pub game_id: String,
    /// Side on the symbol.
    pub side: Side,
    /// Team picked.
    pub pick: TeamSide,
    /// Expected start, Unix ms.
    pub start_ms: u64,
    /// Ask-side quote price `p_i` (signed into the quote).
    pub price_u: u64,
    /// Limit price of the hedge IOC.
    pub p_max_u: u64,
    /// Leg margin, 30_000 or 50_000 (C2).
    pub m1_ppm: u32,
    /// Whether the leg overlaps another leg in time.
    pub overlaps: bool,
    /// A winner leg on a game already started (U-2, l6-contracts L6-2); omitted when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub in_play: bool,
    /// Winner, spread or total (6.0b); omitted on records written before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<MarketKind>,
    /// The market's line, as on the card (`null` for a winner).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// The pick's card label, e.g. "Pacers +3.5" or "Under 221.5". For a total `pick` is a
    /// placeholder (Over = away, Under = home, proposed D-321): read `label` instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

// --- tickets ------------------------------------------------------------------------------------

/// `GET /tickets/:user`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketsResponse {
    /// The wallet, base58.
    pub user: String,
    /// Open and settled tickets, newest first.
    pub tickets: Vec<Ticket>,
}

/// Ticket state (on-chain lifecycle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketStatus {
    /// `Pending` escrowed, not finalized yet.
    Pending,
    /// Minted.
    Open,
    /// Paid out by `settle`.
    Settled,
    /// Closed by `cash_out`.
    CashedOut,
    /// Pending refunded.
    Refunded,
}

impl TicketStatus {
    /// The `tickets.state` value.
    pub fn as_str(self) -> &'static str {
        match self {
            TicketStatus::Pending => "pending",
            TicketStatus::Open => "open",
            TicketStatus::Settled => "settled",
            TicketStatus::CashedOut => "cashed_out",
            TicketStatus::Refunded => "refunded",
        }
    }
}

/// Status of one leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegStatus {
    /// Unresolved.
    Open,
    /// r̃ = 1.
    Won,
    /// r̃ = 0.
    Lost,
    /// 0 < r̃ < 1.
    Half,
    /// Voided.
    Void,
}

/// `GET /ticket/:id`, an item of `/tickets/:user`, and the `ticket` stream frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    /// Ticket PDA, base58 (derivable from the pending's user and nonce).
    pub ticket: String,
    /// Owner, base58.
    pub user: String,
    /// The user's nonce in the PDA seeds.
    pub nonce: u64,
    /// Lifecycle state.
    pub state: TicketStatus,
    /// Number of legs.
    pub n: u8,
    /// Stake, micro-USDC.
    pub stake_u: u64,
    /// Payout if every leg wins.
    pub payout_u: u64,
    /// Offered odds.
    pub odds_offered_u: u64,
    /// Fair odds at quote time.
    pub odds_fair_u: u64,
    /// Margin in money.
    pub fee_u: u64,
    /// Mint time, Unix s (`null` while pending).
    pub minted_at: Option<i64>,
    /// Settlement time, Unix s.
    pub settled_at: Option<i64>,
    /// Amount paid by settle or cash-out.
    pub paid_u: Option<u64>,
    /// Live cash-out value.
    pub cash_out: CashOut,
    /// Legs in ticket order.
    pub legs: Vec<TicketLeg>,
    /// Hedge fills allocated to this ticket.
    pub fills: Vec<TicketFill>,
    /// Transaction signatures.
    pub tx: TicketTx,
    /// The tipster of an own tip or of the ticket a copy was taken from, base58 (l2-contracts
    /// L2-4; `null` for a plain ticket).
    pub tipster: Option<String>,
    /// The original Ticket PDA when this ticket is a copy, base58 (`null` otherwise).
    pub copied_from: Option<String>,
    /// The odds at mint, `Ticket.odds_u` on chain (l6-contracts L6-2); 0 while pending. The copy
    /// screen shows it as "were … when posted" next to the copy-quote's odds now.
    #[serde(default)]
    pub posted_odds_u: u64,
    /// The tipster's slip title of an own tip (L6-2, `PUT /profile`); `null` otherwise.
    #[serde(default)]
    pub title: Option<String>,
    /// Σ stake (rS) of the minted copies of this tip (l7-contracts L7-1, 7.0c). Set on the tips
    /// of `GET /tipsters/{wallet}`; omitted elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_u: Option<u64>,
    /// Minted copies of this tip, self-copies included (D-171, D-332). Set with `pool_u`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copies: Option<u32>,
    /// Short code of each leg's pick, in leg order (`["PHI","TOR"]`): the catalogue's team
    /// abbreviation on NBA symbols, the symbol's team code otherwise. Set with `pool_u`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teams: Option<Vec<String>>,
}

/// Live cash-out value of a ticket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CashOut {
    /// Whether a cash-out quote can be asked for now.
    pub available: bool,
    /// Indicative value, micro-USDC (`null` when unavailable).
    pub c_u: Option<u64>,
    /// Why not, when unavailable.
    pub reason: Option<String>,
}

impl CashOut {
    /// Not available, for `reason`.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        CashOut {
            available: false,
            c_u: None,
            reason: Some(reason.into()),
        }
    }
}

/// One leg of a ticket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketLeg {
    /// Position in the ticket.
    pub idx: u8,
    /// Pascal symbol.
    pub symbol: String,
    /// Game id.
    pub game_id: String,
    /// Away team name.
    pub away: String,
    /// Home team name.
    pub home: String,
    /// Team picked.
    pub pick: TeamSide,
    /// Side on the symbol.
    pub side: Side,
    /// Expected start, Unix ms.
    pub start_ms: u64,
    /// Quote price `p_i`.
    pub p_quote_u: u64,
    /// Achieved fill price (or `p_quote_u` if left unhedged); `null` while pending.
    pub p_fill_u: Option<u64>,
    /// Whether the leg was hedged at inception.
    pub hedged: bool,
    /// Leg status.
    pub status: LegStatus,
    /// Resolution value `r` once resolved.
    pub resolution_u: Option<u64>,
    /// Winner, spread or total, from the ticket's signed quote (D-424); omitted when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<MarketKind>,
    /// The market's line (`null` for a winner).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    /// The pick's card label, e.g. "Over 237.5" or "Pacers +3.5"; read it instead of `pick`
    /// for a total (D-321).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// A hedge fill as shown on a ticket and in `progress` frames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketFill {
    /// Pascal (or mock) trade id, as a string.
    pub trade_id: String,
    /// Symbol.
    pub symbol: String,
    /// Order side.
    pub side: OrderSide,
    /// Fill price.
    pub price_u: u64,
    /// Contracts.
    pub size: u64,
    /// Taker fee (negative = maker rebate).
    pub fee_u: i64,
    /// Builder fee paid.
    pub builder_fee_u: u64,
    /// Pascal or mock.
    pub venue: Venue,
}

/// Transaction signatures of a ticket.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketTx {
    /// `open_pending`.
    pub open_pending: Option<String>,
    /// `finalize`.
    pub finalize: Option<String>,
    /// `settle`.
    pub settle: Option<String>,
    /// `refund_pending` (l1-contracts C4 amendment, 29 Oct).
    pub refund_pending: Option<String>,
    /// `cash_out` (l1-contracts C4 amendment, 29 Oct).
    pub cash_out: Option<String>,
    /// `resolve_leg` signatures in leg-resolution order, `[]` when none (l4-contracts L4-2).
    #[serde(default)]
    pub resolve_leg: Vec<String>,
    /// Slot of the `open_pending`, `cash_out` and `settle` transactions as the indexer applied
    /// them (l7-contracts L7-1); `null` when unknown (a transition found by a resync).
    #[serde(default)]
    pub slot: TxSlots,
    /// The indexer's time from block time to its own observation, ms (approximate: block time
    /// has whole-second resolution); `null` when unknown.
    #[serde(default)]
    pub confirm_ms: TxConfirm,
}

/// `Ticket.tx.slot` (L7-1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxSlots {
    /// Slot of `open_pending`.
    pub open_pending: Option<u64>,
    /// Slot of `cash_out`.
    pub cash_out: Option<u64>,
    /// Slot of `settle`.
    pub settle: Option<u64>,
}

/// `Ticket.tx.confirm_ms` (L7-1): observed − block time, ms.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxConfirm {
    /// Of `open_pending`.
    pub open_pending: Option<u64>,
    /// Of `cash_out`.
    pub cash_out: Option<u64>,
}

// --- GET /cashout-quote/:ticket -----------------------------------------------------------------

/// `GET /cashout-quote/:ticket`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CashoutQuoteResponse {
    /// Ticket PDA.
    pub ticket: String,
    /// Cash-out amount, micro-USDC.
    pub c_u: u64,
    /// Signing time, Unix s (valid 30 s).
    pub ts: i64,
    /// Quote-signer pubkey, base58.
    pub quote_signer: String,
    /// The C3 cash-out message, base64.
    pub quote_msg: String,
    /// Signature, base64.
    pub quote_sig: String,
}

// --- WS /stream ---------------------------------------------------------------------------------

/// Client → server message on `/stream` (optional).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientMessage {
    /// Restrict `progress` and `ticket` frames to one wallet.
    pub subscribe: Subscribe,
}

/// Subscription filter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscribe {
    /// Wallet, base58.
    pub user: String,
}

/// Server → client frame, one JSON object per text frame, tagged by `type`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamFrame {
    /// Liveness, at least every 10 s.
    Heartbeat {
        /// Server time, Unix ms.
        ts_ms: u64,
    },
    /// Mint-flow progress of one pending.
    Progress {
        /// Pending PDA.
        pending: String,
        /// Stage reached.
        stage: ProgressStage,
        /// Fills so far (at `hedging`).
        fills: Vec<TicketFill>,
        /// Ticket PDA once minted.
        ticket: Option<String>,
    },
    /// A ticket changed.
    Ticket {
        /// The ticket as `/ticket/:id` returns it.
        ticket: Box<Ticket>,
    },
    /// Money-flow ticker item.
    Feed {
        /// Kind of item.
        kind: FeedKind,
        /// Display text (kept for older clients).
        text: String,
        /// Time, Unix ms.
        ts_ms: u64,
        /// Who placed it: the handle, else a short address (`7xK9…94Lp`). On `mint` and `copy`
        /// (l7-contracts L7-1).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        who: Option<String>,
        /// Stake (rS), micro-USDC. On `mint` and `copy`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        amount_u: Option<u64>,
        /// The first leg's pick label (e.g. "76ers", "Over 221.5"). On `mint` and `copy`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        team: Option<String>,
        /// The ticket's odds at mint. On `mint` and `copy`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        odds_u: Option<u64>,
    },
}

/// Stage of a `progress` frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressStage {
    /// `open_pending` landed: stake escrowed.
    Escrowed,
    /// Hedge filled, finalize on its way.
    Hedging,
    /// `finalize` landed: ticket minted.
    Minted,
    /// Pending refunded.
    Refunded,
}

/// Kind of a feed item (also `feed_events.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedKind {
    /// A hedge fill.
    Fill,
    /// A ticket minted.
    Mint,
    /// A copy of a tip minted (l7-contracts L7-1).
    Copy,
    /// A leg resolved.
    Resolve,
    /// A ticket settled.
    Settle,
    /// A ticket cashed out.
    Cashout,
}

impl FeedKind {
    /// The `feed_events.kind` value.
    pub fn as_str(self) -> &'static str {
        match self {
            FeedKind::Fill => "fill",
            FeedKind::Mint => "mint",
            FeedKind::Copy => "copy",
            FeedKind::Resolve => "resolve",
            FeedKind::Settle => "settle",
            FeedKind::Cashout => "cashout",
        }
    }
}

/// `floor(1e12 / price_u)`: decimal odds at a price, no margin (C4 card). `0` for a zero price.
pub fn odds_at(price_u: u64) -> u64 {
    1_000_000_000_000_u64.checked_div(price_u).unwrap_or(0)
}

// --- GET /tipsters, GET /tipsters/{wallet} (l4-contracts L4-2) ---------------------------------

/// `GET /tipsters?sort=roi|brier|skin|followers_roi&all=0|1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TipstersResponse {
    /// When the board was computed, Unix ms.
    pub as_of_ms: u64,
    /// The board, sorted.
    pub tipsters: Vec<TipsterRow>,
}

/// One tipster's reputation (spec section 6), from their settled and cashed-out own tips only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TipsterRow {
    /// Wallet, base58.
    pub wallet: String,
    /// Own tips settled or cashed out.
    pub settled: u32,
    /// Of those, the ones that returned more than their stake.
    pub won: u32,
    /// `won / settled`.
    pub hit_rate_ppm: u64,
    /// `(Σ returned − Σ staked) / Σ staked`, signed.
    pub roi_ppm: i64,
    /// Mean over resolved legs of `(r̃ − p_fill)²`.
    pub brier_ppm: u64,
    /// Mean over resolved legs of `r̃ − p_fill`, signed.
    pub edge_ppm: i64,
    /// Mean odds of the settled own tips.
    pub avg_odds_u: u64,
    /// Σ stake of the settled own tips minted in the last 30 days.
    pub skin_u: u64,
    /// ROI of the settled copies of this tipster's tips (copy fee included); `null` with none.
    pub followers_roi_ppm: Option<i64>,
    /// Consecutive wins among the latest settled own tips.
    pub streak: u32,
    /// Own tips open now.
    pub open_count: u32,
    /// Their stake.
    pub open_staked_u: u64,
    /// Copies minted of this tipster's tips.
    pub copies: u32,
    /// Σ `TipsterPaid`.
    pub fees_earned_u: u64,
    /// The wallet's handle (`PUT /profile`), `null` without one (l6-contracts L6-2).
    #[serde(default)]
    pub handle: Option<String>,
    /// On the default board: `settled ≥ 5` and `skin ≥ 20 USDC` (U-5, D-304).
    #[serde(default)]
    pub verified: bool,
    /// `roi_ppm` over the settled own tips minted in the last 30 days (the skin window).
    #[serde(default)]
    pub roi_30d_ppm: i64,
    /// Distinct wallets, other than the tipster's own, with a minted copy of their tips.
    #[serde(default)]
    pub copiers: u32,
}

/// `GET /tipsters/{wallet}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TipsterResponse {
    /// The tipster's row.
    pub tipster: TipsterRow,
    /// Cumulative ROI and Brier after each settled own tip, oldest first.
    pub history: Vec<TipsterHistoryPoint>,
    /// Settled and cashed-out own tips, newest first.
    pub settled: Vec<Ticket>,
    /// Open own tips, newest first.
    pub open: Vec<Ticket>,
}

/// One point of a tipster's history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TipsterHistoryPoint {
    /// Ticket PDA.
    pub ticket: String,
    /// Settlement (or cash-out) time, Unix s.
    pub settled_at: i64,
    /// ROI over the settled own tips up to this one.
    pub roi_cum_ppm: i64,
    /// Brier over their resolved legs up to this one.
    pub brier_cum_ppm: u64,
}

// --- GET /house (l4-contracts L4-2) ---------------------------------------------------------

/// `GET /house`: the house's books at a glance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HouseResponse {
    /// Unix ms.
    pub as_of_ms: u64,
    /// `dev`, `demo` or `live`.
    pub profile: String,
    /// The on-chain vault.
    pub vault: HouseVault,
    /// Tickets by state.
    pub tickets: HouseTickets,
    /// The hedger account.
    pub hedger: HouseHedger,
    /// Fee totals.
    pub fees: HouseFees,
}

/// The vault (`Vault.free`, `Vault.locked`, `Config.reserve`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HouseVault {
    /// House money.
    pub free_u: u64,
    /// Σ payout of open tickets.
    pub locked_u: u64,
    /// `withdraw_house` floor.
    pub reserve_u: u64,
}

/// Ticket counts by state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HouseTickets {
    /// Escrowed, not finalized.
    pub pending: u64,
    /// Minted.
    pub open: u64,
    /// Settled.
    pub settled: u64,
    /// Cashed out.
    pub cashed_out: u64,
    /// Refunded pendings.
    pub refunded: u64,
}

/// The hedger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HouseHedger {
    /// Where its orders fill (D-007).
    pub venue: Venue,
    /// Free collateral, micro-USD (0 when unknown).
    pub free_collateral_u: i64,
    /// Net position per symbol against the desk's target.
    pub positions: Vec<HousePosition>,
}

/// One symbol of the hedger's book.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HousePosition {
    /// Symbol.
    pub symbol: String,
    /// Signed contracts held: positive = long the symbol's yes side (the away team on an NBA
    /// moneyline), negative = short.
    pub held: i64,
    /// Signed contracts the desk wants held (section 3), same sign convention.
    pub target: i64,
}

/// Fee totals, micro-USDC.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HouseFees {
    /// Σ `Ticket.fee_u` (`rS × m(n)`) of minted tickets.
    pub margin_u: u64,
    /// Σ Pascal taker fees of the hedge fills (negative = net maker rebates).
    pub pascal_fees_u: i64,
    /// Σ builder fees of the hedge fills.
    pub builder_fees_u: u64,
    /// The house's part of the copy fees charged (charged − Σ `TipsterPaid`).
    pub copy_fees_u: u64,
}

// --- profiles and FX (l6-contracts L6-2, step 6.0c) ---------------------------------------------

/// `PUT /profile` body. `sig` is base64 ed25519 by `wallet` over the UTF-8 message
/// [`crate::profiles::message`]: `betfi/profile/v1\n<wallet>\n<handle>\n<ticket or ->\n<title or
/// ->\n<ts>`, or, with `links` or `follow` (l7-contracts L7-1), [`crate::profiles::message_v2`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileRequest {
    /// Signing wallet, base58.
    pub wallet: String,
    /// Handle, `^[a-z0-9_]{3,20}$` (send the current one to set only a title). May be empty or
    /// absent only on a follow-only request (7.0c).
    #[serde(default)]
    pub handle: String,
    /// Slip title for `ticket` (≤ 40 characters), or `null`.
    #[serde(default)]
    pub title: Option<String>,
    /// The own tip the title is for (Ticket PDA), or `null`.
    #[serde(default)]
    pub ticket: Option<String>,
    /// Signing time, Unix s (within ±300 s of the desk's clock).
    pub ts: i64,
    /// Signature, base64 (64 bytes).
    pub sig: String,
    /// Social links to store, replacing all four (L7-1); `null` = unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<ProfileLinks>,
    /// Follow or unfollow a wallet (L7-1); `null` = nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow: Option<FollowRequest>,
}

/// Social links of a profile: bare handles or usernames, never URLs (L7-1, 7.0c).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileLinks {
    /// X (Twitter) handle, `[A-Za-z0-9_]{1,15}`.
    #[serde(default)]
    pub x: Option<String>,
    /// YouTube handle, `[A-Za-z0-9._-]{3,30}`.
    #[serde(default)]
    pub youtube: Option<String>,
    /// Telegram username, `[A-Za-z][A-Za-z0-9_]{4,31}`.
    #[serde(default)]
    pub telegram: Option<String>,
    /// Instagram username, `[A-Za-z0-9._]{1,30}`.
    #[serde(default)]
    pub instagram: Option<String>,
}

/// `PUT /profile` `follow` (L7-1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowRequest {
    /// The wallet to follow or unfollow, base58.
    pub wallet: String,
    /// `true` follows, `false` unfollows (both idempotent).
    pub on: bool,
}

/// `GET /profile/{wallet}`, and the answer of a successful `PUT /profile`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileResponse {
    /// Wallet, base58.
    pub wallet: String,
    /// Handle, `null` without one.
    pub handle: Option<String>,
    /// When the current handle was set, Unix s (0 without one).
    pub since: i64,
    /// On the tipster board: `settled ≥ 5` and `skin ≥ 20 USDC` (D-304; L7-1).
    #[serde(default)]
    pub verified: bool,
    /// Social links (L7-1); all `null` without any.
    #[serde(default)]
    pub links: ProfileLinks,
    /// Wallets following this one (L7-1).
    #[serde(default)]
    pub followers: u64,
    /// Wallets this one follows (L7-1).
    #[serde(default)]
    pub following: u64,
}

/// `GET /tipsters/{wallet}/copiers?ticket=<pda>` (L7-1): the minted copies of one own tip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopiersResponse {
    /// The tip's Ticket PDA.
    pub ticket: String,
    /// One row per minted copy, newest first.
    pub copiers: Vec<Copier>,
    /// Σ `stake_u` of every minted copy (as the tip's `pool_u`).
    pub pool_u: u64,
}

/// One copy of a tip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Copier {
    /// The copier's wallet, base58.
    pub wallet: String,
    /// Its handle, `null` without one.
    pub handle: Option<String>,
    /// Stake of the copy (rS), micro-USDC.
    pub stake_u: u64,
    /// The copy's `open_pending` signature (`null` if the indexer never saw it).
    pub tx: Option<String>,
}

/// `GET /fx`: euros per US dollar (USDC taken at 1 USD, U-4).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FxResponse {
    /// EUR per USD × 1e6.
    pub eur_per_usd_ppm: u64,
    /// The rate's date, `YYYY-MM-DD`.
    pub as_of: String,
    /// `ecb` (the ECB reference rate, fetched daily) or `fallback` (the configured fixed rate).
    pub source: String,
    /// Whether this is the configured fallback (no fetched rate fresh enough).
    pub fallback: bool,
    /// EUR per SOL × 1e6: SOL/USD (Pyth Hermes, or the fixed fallback) × `eur_per_usd_ppm`
    /// (l7-contracts L7-1, 7.0c).
    #[serde(default)]
    pub eur_per_sol_ppm: u64,
    /// Pyth publish time of the SOL/USD price, Unix ms (0 for the fallback).
    #[serde(default)]
    pub sol_as_of_ms: u64,
    /// `pyth` or `fallback`.
    #[serde(default)]
    pub sol_source: String,
    /// Whether the SOL/USD price is the configured fallback.
    #[serde(default)]
    pub sol_fallback: bool,
}

// --- /relay (L8) --------------------------------------------------------------------------------

/// `GET /relay` while sponsoring is on (503 `unavailable` when it is off).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayInfo {
    /// The sponsor key, base58: the fee payer (signature slot 0) of every relayed transaction.
    pub sponsor: String,
    /// The most a relayed transaction may cost the sponsor (fee plus rent), lamports.
    pub max_lamports_per_tx: u64,
}

/// `POST /relay` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayRequest {
    /// Base64 of the wire `VersionedTransaction` (legacy or v0): slot 0 (the sponsor) zeroed,
    /// slot 1 signed by the user.
    pub tx: String,
}

/// `POST /relay` answer once the RPC accepted the transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayResponse {
    /// The transaction signature, base58 (the sponsor's, slot 0).
    pub signature: String,
}
