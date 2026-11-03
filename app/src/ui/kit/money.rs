//! Euros from USDC (U-4, L6-2): € = USDC × `eur_per_usd_ppm` (USDC taken at 1 USD), truncated
//! to the cent, with the USDC amount beneath. Display only; the desk's integers are not changed.

use dioxus::prelude::*;

use crate::desk::types::Fx;
use crate::format;

/// The shell's `/fx` answer, shared with every screen (`None` until it loads or if it fails).
pub type FxSignal = Signal<Option<Fx>>;

/// The current rate in ppm, if known. Reactive: a component reading it re-renders when it loads.
pub fn use_fx() -> Option<u64> {
    try_use_context::<FxSignal>().and_then(|fx| fx.read().as_ref().map(|f| f.eur_per_usd_ppm))
}

/// Micro-euros for `usdc_u` micro-USDC, truncated.
pub fn eur_u(usdc_u: u64, eur_per_usd_ppm: u64) -> u64 {
    let v = u128::from(usdc_u) * u128::from(eur_per_usd_ppm) / 1_000_000;
    u64::try_from(v).unwrap_or(u64::MAX)
}

/// "€1,240.50" for `usdc_u` at the rate.
pub fn eur_text(usdc_u: u64, eur_per_usd_ppm: u64) -> String {
    format::eur(eur_u(usdc_u, eur_per_usd_ppm))
}

/// "1,346.81 USDC", truncated to the cent.
pub fn usdc_text(usdc_u: u64) -> String {
    format!("{} USDC", format::eur(usdc_u).trim_start_matches('€'))
}

/// The pair a screen shows: the euro figure (or, before the rate is known, the USDC figure) and
/// the USDC line beneath (`None` before the rate is known, so nothing is shown twice).
pub fn pair(usdc_u: u64, eur_per_usd_ppm: Option<u64>) -> (String, Option<String>) {
    match eur_per_usd_ppm {
        Some(r) => (eur_text(usdc_u, r), Some(usdc_text(usdc_u))),
        None => (usdc_text(usdc_u), None),
    }
}

// ---------------------------------------------------------------- SOL (V-3)

/// The shell's SOL price in euros, as `/fx`'s `eur_per_sol_ppm` (€135.80 = 135_800_000), shared
/// with every screen. `None` until known: a screen then shows no "≈ SOL" figure at all.
pub type SolRateSignal = Signal<Option<u64>>;

/// The SOL rate, if known. Reactive, like [`use_fx`].
pub fn use_sol_rate() -> Option<u64> {
    try_use_context::<SolRateSignal>().and_then(|r| *r.read())
}

/// Micro-SOL for `eur_u` micro-euros at `eur_per_sol_ppm`, rounded half up (0 for a 0 rate).
pub fn sol_u(eur_u: u64, eur_per_sol_ppm: u64) -> u64 {
    if eur_per_sol_ppm == 0 {
        return 0;
    }
    let r = u128::from(eur_per_sol_ppm);
    let v = (u128::from(eur_u) * 1_000_000 + r / 2) / r;
    u64::try_from(v).unwrap_or(u64::MAX)
}

/// "0.184 SOL", "9.13 SOL", "208.8 SOL": three decimals under 1 SOL, two under 100, one above
/// (the mockup's precision), rounded half up.
pub fn sol_text(sol_u: u64) -> String {
    let (div, places) = if sol_u < 1_000_000 {
        (1_000, 3)
    } else if sol_u < 100_000_000 {
        (10_000, 2)
    } else {
        (100_000, 1)
    };
    let units = (sol_u + div / 2) / div;
    let scale = 10u64.pow(places);
    format!(
        "{}.{:0w$} SOL",
        grouped(units / scale),
        units % scale,
        w = places as usize
    )
}

/// 1234567 → "1,234,567".
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

/// "≈ 0.184 SOL" for a euro amount, or `None` while the SOL rate is unknown.
pub fn approx_sol(eur_u: u64, eur_per_sol_ppm: Option<u64>) -> Option<String> {
    eur_per_sol_ppm
        .filter(|r| *r > 0)
        .map(|r| format!("≈ {}", sol_text(sol_u(eur_u, r))))
}

/// "€28,450": a whole-euro figure (truncated), for pools and volumes.
pub fn eur_whole(eur_u: u64) -> String {
    let full = format::eur(eur_u);
    full.split('.').next().unwrap_or(&full).to_string()
}

/// "€28.4k", "€9.8k" from €1,000 (one decimal, truncated), else [`eur_whole`]; the mockup's pool size.
pub fn eur_compact(eur_u: u64) -> String {
    let euros = eur_u / 1_000_000;
    if euros >= 1_000 {
        format!("€{}.{}k", euros / 1000, (euros % 1000) / 100)
    } else {
        eur_whole(eur_u)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sol_at_the_mockup_rate() {
        // The mockup's figures at €135.80 per SOL (L7-2).
        let r = 135_800_000;
        assert_eq!(sol_text(sol_u(25_000_000, r)), "0.184 SOL");
        assert_eq!(sol_text(sol_u(113_750_000, r)), "0.838 SOL");
        assert_eq!(sol_text(sol_u(1_240_500_000, r)), "9.13 SOL");
        assert_eq!(sol_text(sol_u(28_350_000_000, r)), "208.8 SOL");
        assert_eq!(sol_text(1_234_567_890), "1,234.6 SOL");
        assert_eq!(approx_sol(25_000_000, None), None);
        assert_eq!(approx_sol(25_000_000, Some(0)), None);
        assert_eq!(eur_whole(28_450_990_000), "€28,450");
        assert_eq!(eur_compact(28_450_000_000), "€28.4k");
        assert_eq!(eur_compact(9_850_000_000), "€9.8k");
        assert_eq!(eur_compact(500_000_000), "€500");
    }

    #[test]
    fn euros_truncate_at_the_rate() {
        // The design fixtures' pairs (fixtures/design/gen_design.py).
        assert_eq!(eur_text(1_346_810_000, 921_068), "€1,240.50");
        assert_eq!(usdc_text(1_346_810_000), "1,346.81 USDC");
        assert_eq!(eur_text(157_972_260, 921_068), "€145.50");
        assert_eq!(eur_text(0, 921_068), "€0.00");
        assert_eq!(eur_u(u64::MAX, 2_000_000), u64::MAX);
        assert_eq!(pair(5_000_000, None), ("5.00 USDC".to_string(), None));
    }
}
