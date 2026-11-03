//! The one global `Signal<AppState>` (spec 7, "State"): fed by the `/stream` socket and by the
//! few HTTP loads, read by every screen. Screens derive from it; there is no per-screen polling
//! of the stream.

pub mod slip;

use std::collections::{BTreeMap, VecDeque};

use crate::desk::types::{
    Card, FeedKind, Fill, Frame, Game, Health, MarketKind, MarketPick, Pick, Quote, QuoteRequest,
    SentimentResponse, Side, Stage, Ticket, TokenInfo, fills_of,
};

/// How many feed lines to keep.
const FEED_CAP: usize = 30;
/// Most legs a schedina may hold: the program's accounts hold 5 (spec 2: `1 ≤ n ≤ 5`).
pub const MAX_LEGS: usize = schedina_tx::MAX_LEGS;
/// Stake the slip starts with, micro-euros (€10; 6.1c converts euros to USDC at the ECB rate).
pub const DEFAULT_STAKE_U: u64 = 10_000_000;
/// Smallest stake the Tray asks a quote for, micro-USDC (€5; the program's `min_stake` is the
/// binding check).
pub const MIN_STAKE_U: u64 = 5_000_000;

/// State of the `/stream` socket, for the status dot in the header.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Link {
    #[default]
    Connecting,
    Live,
    /// Dropped; the next attempt is in `in_ms`.
    Retrying {
        attempt: u32,
        in_ms: u32,
    },
    /// Demo mode: frames come from the bundled fixtures.
    Demo,
}

/// An HTTP-loaded value.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Load<T> {
    #[default]
    Loading,
    Ready(T),
    Failed(String),
}

/// One leg in the Tray, copied from the Card row it was picked on.
#[derive(Debug, Clone, PartialEq)]
pub struct TrayLeg {
    pub game_id: String,
    pub symbol: String,
    pub pick: Pick,
    pub side: Side,
    pub team: String,
    pub opponent: String,
    pub start_ms: i64,
    /// The Card's indicative odds for this pick (display only; the quote is binding).
    pub card_odds_u: u64,
    /// L6 (6.1b): the leg's market. For a spread or total `team` is the pick's label ("76ers -3.5",
    /// "Over 221.5"); a spread's `opponent` is the other team, a total's the matchup ("Celtics vs
    /// Knicks"), and a total's `pick` is the desk's placeholder (Over = away, D-321).
    pub kind: MarketKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tray {
    pub legs: Vec<TrayLeg>,
    /// The stake as chosen, micro-euros (6.1c: a preset or "Other"; quoted in USDC at the rate).
    pub stake_u: u64,
    /// "Share as a tip": `open_pending(is_tip = true)`. Applied only while
    /// [`Tray::tip_blocker`] has nothing against it.
    pub tip: bool,
    /// The tip's name as typed under the switch (L9, optional): sent with the last quote
    /// before signing and shown as the tip's title. Cleared with `tip`.
    pub tip_name: String,
    /// The token the stake is paid in (the stake row's chip): `None` = mock USDC, the stake
    /// currency; another `GET /tokens` mint is swapped into USDC in the same transaction (L4-3).
    pub pay: Option<TokenInfo>,
}

impl Default for Tray {
    fn default() -> Self {
        Tray {
            legs: Vec::new(),
            stake_u: DEFAULT_STAKE_U,
            tip: false,
            tip_name: String::new(),
            pay: None,
        }
    }
}

impl Tray {
    /// Why this schedina cannot be shared as a tip right now, or `None` if it can.
    pub fn tip_blocker(&self) -> Option<String> {
        if self.legs.is_empty() {
            return Some("pick a team first".into());
        }
        // The swap transaction opens a plain schedina (`schedina_tx::swap`). A game in play is
        // fine (D-660): the quote decides what is still on offer.
        self.pay.as_ref().map(|t| {
            format!(
                "a tip is staked in test USDC, not {}. Switch the stake back to \u{20ac} to share it",
                t.symbol
            )
        })
    }

    /// The winner pick on a game, if the Tray holds one (a spread or total leg is not a team pick).
    pub fn pick_on(&self, game_id: &str) -> Option<Pick> {
        self.legs
            .iter()
            .find(|l| l.game_id == game_id && l.kind == MarketKind::Winner)
            .map(|l| l.pick)
    }

    /// Card tap: add the pick, replace the other pick on the same game, or remove it when it is
    /// already there. One leg per game is also the desk's correlation rule (D-008).
    pub fn toggle(&mut self, game: &Game, pick: Pick) {
        if let Some(i) = self.legs.iter().position(|l| l.game_id == game.game_id) {
            let same = self.legs[i].pick == pick && self.legs[i].kind == MarketKind::Winner;
            self.legs.remove(i);
            if same {
                return;
            }
        }
        if self.legs.len() >= MAX_LEGS {
            return;
        }
        let Some(card_pick) = game.pick(pick) else {
            return;
        };
        let opponent = match pick {
            Pick::Away => &game.home,
            Pick::Home => &game.away,
        };
        self.legs.push(TrayLeg {
            game_id: game.game_id.clone(),
            symbol: game.symbol.clone(),
            pick,
            side: card_pick.side,
            team: game.team(pick).name.clone(),
            opponent: opponent.name.clone(),
            start_ms: game.start_ms,
            card_odds_u: card_pick.odds_u,
            kind: MarketKind::Winner,
        });
    }

    /// L7-1 (7.1a) Sportsbook tap on one outcome of a game's winner market (1–2 or 1–X–2): add
    /// it, replace the game's other leg in place, or take it out when it is already there. Each
    /// soccer outcome is a long on its own symbol (7.0b), so the leg is the market pick's symbol
    /// and side. A draw has no team: its `pick` is the desk's placeholder (home, D-461), `team`
    /// its label ("Draw") and `opponent` the matchup ("Arsenal vs Bayern Munich"), as a total's.
    /// `false` when the slip is full (nothing changed).
    pub fn toggle_outcome(&mut self, game: &Game, mp: &MarketPick) -> bool {
        let at = self.legs.iter().position(|l| l.game_id == game.game_id);
        if let Some(i) = at
            && self.legs[i].symbol == mp.symbol
            && self.legs[i].side == mp.side
        {
            self.legs.remove(i);
            return true;
        }
        let (pick, team, opponent) = match mp.team {
            Some(p) => {
                let other = match p {
                    Pick::Away => Pick::Home,
                    Pick::Home => Pick::Away,
                };
                (p, game.team(p).name.clone(), game.team(other).name.clone())
            }
            None => (
                Pick::Home,
                mp.label.clone(),
                format!("{} vs {}", game.home.name, game.away.name),
            ),
        };
        let leg = TrayLeg {
            game_id: game.game_id.clone(),
            symbol: mp.symbol.clone(),
            pick,
            side: mp.side,
            team,
            opponent,
            start_ms: game.start_ms,
            card_odds_u: mp.odds_u,
            kind: MarketKind::Winner,
        };
        match at {
            Some(i) => self.legs[i] = leg,
            None if self.legs.len() >= MAX_LEGS => return false,
            None => self.legs.push(leg),
        }
        true
    }

    pub fn remove(&mut self, game_id: &str) {
        self.legs.retain(|l| l.game_id != game_id);
    }

    /// True when the Tray still holds exactly the picks of `req` (symbol and side, in order).
    /// A placed schedina clears the Tray only then: its confirmation can arrive after the user
    /// has already started the next one (3.C: a slow public RPC confirmed ticket A after ticket
    /// B's picks were in the Tray, and wiped them).
    pub fn holds(&self, req: &QuoteRequest) -> bool {
        self.legs.len() == req.legs.len()
            && self
                .legs
                .iter()
                .zip(&req.legs)
                .all(|(l, r)| l.symbol == r.symbol && l.side == r.side)
    }
}

/// Where one pending schedina is between signature and mint (Confirm screen).
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub pending: String,
    pub stage: Stage,
    pub fills: Vec<Fill>,
    pub ticket: Option<String>,
}

fn stage_rank(s: Stage) -> u8 {
    match s {
        Stage::Other => 0,
        Stage::Escrowed => 1,
        Stage::Hedging => 2,
        Stage::Minted | Stage::Refunded => 3,
    }
}

/// Where the app is in placing one schedina, before the desk's `progress` frames take over.
#[derive(Debug, Clone, PartialEq)]
pub enum SubmitStep {
    /// Asking the desk for a fresh signed quote.
    Quoting,
    /// Waiting for the wallet's signature.
    Signing,
    /// Sent; waiting for the cluster to confirm.
    Confirming,
    /// `open_pending` landed: the stake is escrowed; the stream tells the rest.
    Landed,
    /// Nothing was escrowed (or the outcome is unknown), in words for the user.
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Submission {
    /// Distinguishes this submission from the next one: a flow still awaiting its confirmation
    /// updates only its own submission, never one started after it.
    pub id: u64,
    pub step: SubmitStep,
    /// The Pending PDA, known once the transaction is built.
    pub pending: Option<String>,
    /// The `open_pending` signature, once sent.
    pub sig: Option<String>,
    /// The signed quote the transaction carries (for the summary).
    pub quote: Option<Quote>,
    /// The picks and stake played: the Tray is cleared when they land, if it still holds them.
    pub request: Option<QuoteRequest>,
    /// Demo mode: no wallet, the stream fixtures play the progress.
    pub demo: bool,
    /// A plain schedina, a tip, or a copy of a tip.
    pub kind: PlayKind,
}

/// What an `open_pending` opens: its `is_tip` / `copied_from` arguments, in the app's terms.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum PlayKind {
    #[default]
    Plain,
    /// Shared as a tip (`is_tip = true`), named `title` when the tipster typed one (L9).
    Tip { title: Option<String> },
    /// A copy of the tip `original` (Ticket PDA, base58): `copied_from = Some(original)`.
    /// `legs` are the original's, which the copy's signed legs must equal as a set.
    Copy {
        original: String,
        legs: Vec<schedina_tx::ShownLeg>,
    },
}

impl Submission {
    pub fn new(demo: bool) -> Self {
        thread_local! {
            static NEXT_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
        }
        let id = NEXT_ID.with(|n| {
            let id = n.get();
            n.set(id + 1);
            id
        });
        Submission {
            id,
            step: SubmitStep::Quoting,
            pending: None,
            sig: None,
            quote: None,
            request: None,
            demo,
            kind: PlayKind::Plain,
        }
    }

    /// Still between the tap and the landed transaction.
    pub fn busy(&self) -> bool {
        matches!(
            self.step,
            SubmitStep::Quoting | SubmitStep::Signing | SubmitStep::Confirming
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FeedItem {
    pub kind: FeedKind,
    pub text: String,
    pub ts_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AppState {
    pub link: Link,
    /// The desk's `GET /health` (profile); `Loading` until it answers, `Failed` if it did not.
    pub health: Load<Health>,
    pub card: Load<Card>,
    /// `GET /sentiment` for the card's games (L5-4); `None` until it answers. A failed refresh
    /// keeps the last answer.
    pub sentiment: Option<SentimentResponse>,
    pub tray: Tray,
    /// 6.1c: the slip's latest quotes and the odds the user accepted (slip, slip bar).
    pub slip: slip::Slip,
    /// The connected wallet (base58), or the example tickets' owner in demo mode.
    pub user: Option<String>,
    /// Pending schedine by pending PDA, from `progress` frames.
    pub progress: BTreeMap<String, Progress>,
    /// The schedina this app is placing (or last placed); the Confirm screen follows it.
    pub submission: Option<Submission>,
    /// Tickets by ticket PDA, from `GET /tickets/:user` and `ticket` frames.
    pub tickets: BTreeMap<String, Ticket>,
    pub tickets_load: Load<()>,
    /// Newest first.
    pub feed: VecDeque<FeedItem>,
    /// Frames that did not decode (malformed JSON or a known type with a bad shape). Ignored.
    pub bad_frames: u32,
    /// Frames of a `type` this build does not know. Ignored.
    pub unknown_frames: u32,
}

impl AppState {
    /// True unless the desk said its profile is `live`: hedges are then filled by the Pascal
    /// mock, and the app says so on every screen (spec 7, 3.D′). Unknown counts as mock.
    pub fn mock_fills(&self) -> bool {
        !matches!(&self.health, Load::Ready(h) if h.is_live())
    }

    /// "Now" for the tip and copy rules, Unix ms: the browser clock, or in demo mode the
    /// example card's `as_of_ms`, so the bundled examples (recorded on one night) stay
    /// self-consistent whatever the date. The program's own clock is the binding check.
    pub fn now_ms(&self, demo: bool) -> i64 {
        match &self.card {
            Load::Ready(c) if demo => c.as_of_ms,
            _ => js_sys::Date::now() as i64,
        }
    }

    /// Submission `id` landed on chain (its confirmation, or the desk's first `progress` frame
    /// for its Pending, whichever comes first): no longer busy, so the next Gioca is enabled,
    /// and its picks leave the Tray, which then holds only what the user picked since.
    pub fn landed(&mut self, id: u64) {
        let Some(sub) = self.submission.as_mut().filter(|s| s.id == id) else {
            return;
        };
        if matches!(sub.step, SubmitStep::Signing | SubmitStep::Confirming) {
            sub.step = SubmitStep::Landed;
        }
        if let Some(req) = sub.request.take()
            && self.tray.holds(&req)
        {
            self.tray.legs.clear();
            // Sharing is chosen per schedina: the next one starts private.
            self.tray.tip = false;
            self.tray.tip_name.clear();
        }
    }

    /// Apply one raw `/stream` text frame. Never panics: a frame that does not decode is
    /// counted and dropped, and the state is left as it was.
    pub fn apply_text(&mut self, text: &str) -> Result<(), String> {
        match serde_json::from_str::<Frame>(text) {
            Ok(frame) => {
                self.apply(frame);
                Ok(())
            }
            Err(e) => {
                self.bad_frames = self.bad_frames.saturating_add(1);
                Err(e.to_string())
            }
        }
    }

    pub fn apply(&mut self, frame: Frame) {
        match frame {
            // Liveness only; the socket task tracks it, the state does not need to change.
            Frame::Heartbeat { .. } => {}
            Frame::Progress {
                pending,
                stage,
                fills,
                ticket,
            } => {
                let fills = fills_of(&fills);
                // The desk saw the Pending on chain: the transaction landed, even if the
                // wallet's confirmation poll (slow on a throttled public RPC) has not returned.
                // Without this the submission stays busy and the next Gioca is disabled.
                let ours = self.submission.as_ref().filter(|s| {
                    s.pending.as_deref() == Some(pending.as_str()) && stage != Stage::Other
                });
                if let Some(id) = ours.map(|s| s.id) {
                    self.landed(id);
                }
                match self.progress.get_mut(&pending) {
                    // A late or replayed frame never moves a pending backwards.
                    Some(p) if stage_rank(stage) < stage_rank(p.stage) => {}
                    Some(p) => {
                        p.stage = stage;
                        if !fills.is_empty() {
                            p.fills = fills;
                        }
                        if ticket.is_some() {
                            p.ticket = ticket;
                        }
                    }
                    None => {
                        self.progress.insert(
                            pending.clone(),
                            Progress {
                                pending,
                                stage,
                                fills,
                                ticket,
                            },
                        );
                    }
                }
            }
            Frame::Ticket { ticket } => {
                self.tickets.insert(ticket.ticket.clone(), *ticket);
            }
            Frame::Feed { kind, text, ts_ms } => {
                self.feed.push_front(FeedItem { kind, text, ts_ms });
                self.feed.truncate(FEED_CAP);
            }
            Frame::Unknown => {
                self.unknown_frames = self.unknown_frames.saturating_add(1);
            }
        }
    }

    /// Replace the tickets of `user` with a fresh `GET /tickets/:user` answer, keeping tickets
    /// of other users untouched (a stale answer for a previous wallet is dropped by the caller).
    pub fn set_tickets(&mut self, user: &str, tickets: Vec<Ticket>) {
        self.tickets.retain(|_, t| t.user != user);
        for t in tickets {
            self.tickets.insert(t.ticket.clone(), t);
        }
        self.tickets_load = Load::Ready(());
    }

    /// The viewer's tickets, newest first (by nonce).
    pub fn my_tickets(&self) -> Vec<&Ticket> {
        let Some(user) = &self.user else {
            return Vec::new();
        };
        let mut v: Vec<&Ticket> = self.tickets.values().filter(|t| &t.user == user).collect();
        v.sort_by_key(|t| std::cmp::Reverse(t.nonce));
        v
    }
}
