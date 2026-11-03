//! Who may copy what (spec 6, L2-4): the rules the parlay page (`tipsters::parlay`), its locked
//! games and the receipt share. The copy itself is the bet drawer's copy mode (L9).

use crate::desk::types::{LegStatus, Ticket, TicketState};

/// Own tip: the program's `is_own_tip` (L2-4).
pub fn is_own_tip(t: &Ticket) -> bool {
    t.tipster.as_deref() == Some(t.user.as_str()) && t.copied_from.is_none()
}

/// Why this ticket cannot be copied now, or `None` if it can: the program's `check_copy` (an
/// open own tip, every leg still open; in play is fine, D-660), the same rule as the desk's copy
/// pre-check. The receipt uses it too.
pub fn copy_blocker(t: &Ticket) -> Option<String> {
    if t.copied_from.is_some() {
        return Some("this bet is itself a copy: copy the original tip instead".into());
    }
    if !is_own_tip(t) {
        return Some("only a bet shared as a tip can be copied".into());
    }
    match t.state {
        TicketState::Open => {}
        TicketState::Pending => return Some("this tip is still being placed".into()),
        _ => return Some("this tip is closed".into()),
    }
    t.legs
        .iter()
        .any(|l| l.status != LegStatus::Open)
        .then(|| "a game in it has already been decided".into())
}
