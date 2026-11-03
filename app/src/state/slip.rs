//! The slip's quotes (6.1c): what the desk last offered for the slip's picks, as one parlay or one
//! single per pick, and the odds the user last saw, so a re-quote that changes them asks for an
//! Accept before Place bet. Shared by the slip screen, the desktop slip column and the shell's
//! slip bar. The client only compares the desk's integers; it never computes odds.

use crate::desk::types::{Quote, QuoteRequest, SwapQuote, TokenInfo};

/// One desk answer the slip holds: the parlay's, or one single's.
#[derive(Debug, Clone, PartialEq)]
pub struct SlipLine {
    pub req: QuoteRequest,
    /// The signed quote, or why there is none, in plain words.
    pub quote: Result<Quote, String>,
    /// The offered odds the user last saw or accepted for exactly this request (`None` before
    /// its first answer).
    pub seen_odds_u: Option<u64>,
}

impl SlipLine {
    pub fn offered(&self) -> Option<u64> {
        self.quote.as_ref().ok().map(|q| q.odds_offered_u)
    }

    /// The desk now offers other odds than the ones the user accepted.
    pub fn moved(&self) -> bool {
        matches!((self.offered(), self.seen_odds_u), (Some(o), Some(s)) if o != s)
    }
}

/// The swap quote for the amount typed while paying in another token (L4-3).
#[derive(Debug, Clone, PartialEq)]
pub struct SwapAnswer {
    pub mint: String,
    pub amount_in: u64,
    pub quote: Result<SwapQuote, String>,
}

/// Where one single of a Singles run is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SingleStatus {
    Waiting,
    Placing,
    /// `open_pending` landed (or, in demo mode, the example plays).
    Placed,
    Failed(String),
}

/// One single of a Singles run, shown on Confirm.
#[derive(Debug, Clone, PartialEq)]
pub struct SingleRun {
    pub label: String,
    pub pending: Option<String>,
    pub status: SingleStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Slip {
    /// Singles: one ticket per pick. Parlay otherwise.
    pub singles: bool,
    /// "Always accept better odds": a higher offered price is taken without asking. A lower one
    /// always needs Accept.
    pub accept_better: bool,
    /// The "Other" stake field's text while it is in use.
    pub other: Option<String>,
    /// The amount typed while paying in another token (its own text: switching back to euros
    /// keeps the euro stake).
    pub pay_amount: String,
    /// `GET /tokens`: the mints the chip offers (empty until it answers).
    pub tokens: Vec<TokenInfo>,
    pub swap: Option<SwapAnswer>,
    /// The newest desk answers for the slip's current requests, in order.
    pub lines: Vec<SlipLine>,
    /// Design builds only: the odds the mockup's slip showed before its price moved, taken as
    /// "seen" by the first answer.
    pub seed_seen_odds_u: Option<u64>,
    /// Design builds only: the mockup's picks were put in the slip once.
    pub seeded: bool,
    /// The Singles run being placed (empty for a parlay).
    pub run: Vec<SingleRun>,
}

impl Default for Slip {
    fn default() -> Self {
        Slip {
            singles: false,
            accept_better: true,
            other: None,
            pay_amount: String::new(),
            tokens: Vec::new(),
            swap: None,
            lines: Vec::new(),
            seed_seen_odds_u: None,
            seeded: false,
            run: Vec::new(),
        }
    }
}

impl Slip {
    /// Fresh desk answers for the slip's requests, in order. A request the slip already held
    /// keeps the odds the user accepted, unless the new price is higher and better odds are
    /// accepted; a new request (other picks or stake) starts from what the desk offers now.
    pub fn apply(&mut self, answers: Vec<(QuoteRequest, Result<Quote, String>)>) {
        let old = std::mem::take(&mut self.lines);
        for (req, quote) in answers {
            let offered = quote.as_ref().ok().map(|q| q.odds_offered_u);
            let mut seen = old
                .iter()
                .find(|l| l.req == req)
                .and_then(|l| l.seen_odds_u);
            if seen.is_none() && offered.is_some() {
                seen = self.seed_seen_odds_u.take();
            }
            let seen = match (seen, offered) {
                (None, o) => o,
                (Some(s), Some(o)) if o > s && self.accept_better => Some(o),
                (s, _) => s,
            };
            self.lines.push(SlipLine {
                req,
                quote,
                seen_odds_u: seen,
            });
        }
    }

    /// "Accept": the odds on offer become the ones the user saw.
    pub fn accept(&mut self) {
        for l in &mut self.lines {
            if let Some(o) = l.offered() {
                l.seen_odds_u = Some(o);
            }
        }
    }

    /// How many answers moved away from what the user accepted.
    pub fn moved(&self) -> usize {
        self.lines.iter().filter(|l| l.moved()).count()
    }

    /// The parlay's odds as the slip bar shows them: the ones the user last saw, while the slip
    /// still holds exactly the picks they were quoted for. `None` for singles.
    pub fn bar_odds_u(&self, tray: &super::Tray) -> Option<u64> {
        if self.singles {
            return None;
        }
        let line = self.lines.first().filter(|l| tray.holds(&l.req))?;
        line.seen_odds_u.or(line.offered())
    }
}
