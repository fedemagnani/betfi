//! Display formatting for C1 micro-units. The app never computes prices or odds: it only turns the
//! desk's integers into text. Truncation (not rounding) everywhere, so a displayed payout or odd is
//! never above what the desk signed (C1: "rounding always in the house's favour").

/// 1.0 in micro-units (C1).
pub const MICRO: u64 = 1_000_000;

/// Euros from micro-USDC, truncated to the cent: `13_956_834` -> `"€13.95"`.
pub fn eur(amount_u: u64) -> String {
    let whole = amount_u / MICRO;
    let cents = (amount_u % MICRO) / 10_000;
    format!("€{}.{cents:02}", group_thousands(whole))
}

/// Small euro amounts (fees per fill) to the hundredth of a cent, truncated:
/// `5_592` -> `"€0.0055"`.
pub fn eur_fine(amount_u: u64) -> String {
    let whole = amount_u / MICRO;
    let frac = (amount_u % MICRO) / 100;
    format!("€{}.{frac:04}", group_thousands(whole))
}

/// A `_ppm` rate as a percentage, truncated to a tenth: `50_000` -> `"5%"`, `87_000` -> `"8.7%"`.
pub fn pct_ppm(ppm: u32) -> String {
    let tenths = ppm / 1_000;
    if tenths.is_multiple_of(10) {
        format!("{}%", tenths / 10)
    } else {
        format!("{}.{}%", tenths / 10, tenths % 10)
    }
}

/// A signed `_ppm` rate as a percentage with its sign, truncated toward zero to a tenth:
/// `182_000` -> `"+18.2%"`, `-12_000` -> `"−1.2%"`, `-999` -> `"0%"`. The minus is U+2212.
pub fn pct_signed(ppm: i64) -> String {
    let tenths = ppm / 1_000;
    let mag = tenths.unsigned_abs();
    let sign = match tenths.signum() {
        1 => "+",
        -1 => "\u{2212}",
        _ => "",
    };
    if mag.is_multiple_of(10) {
        format!("{sign}{}%", mag / 10)
    } else {
        format!("{sign}{}.{}%", mag / 10, mag % 10)
    }
}

/// A price in micro-units (0 to 1) as the chance it stands for: `699_000` -> `"69.9%"`.
pub fn chance(price_u: u64) -> String {
    pct_ppm(u32::try_from(price_u.min(MICRO)).unwrap_or(0))
}

/// `"1 contract"`, `"8 contracts"`.
pub fn contracts(n: u64) -> String {
    match n {
        1 => "1 contract".to_string(),
        n => format!("{n} contracts"),
    }
}

/// `"1 pick"`, `"3 picks"`.
pub fn picks(n: impl Into<u64>) -> String {
    match n.into() {
        1 => "1 pick".to_string(),
        n => format!("{n} picks"),
    }
}

/// Decimal odds from micro-units, truncated to two places: `1_438_848` -> `"1.43"`.
pub fn odds(odds_u: u64) -> String {
    let whole = odds_u / MICRO;
    let hundredths = (odds_u % MICRO) / 10_000;
    format!("{whole}.{hundredths:02}")
}

/// `1234567` -> `"1,234,567"`.
/// A token amount for display: `schedina_tx::swap::format_units` (exact, no trailing zeros)
/// with the whole part grouped: `1_000_000_000` at 5 decimals -> `"10,000"`.
pub fn token(amount: u64, decimals: u8) -> String {
    let text = schedina_tx::swap::format_units(amount, decimals);
    let (whole, frac) = text.split_once('.').unwrap_or((&text, ""));
    let grouped = whole
        .parse::<u64>()
        .map(group_thousands)
        .unwrap_or_else(|_| whole.to_string());
    if frac.is_empty() {
        grouped
    } else {
        format!("{grouped}.{frac}")
    }
}

fn group_thousands(n: u64) -> String {
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

/// A base58 address or signature shortened for display: `"455U…uHhC"`.
pub fn short(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() <= 10 {
        return id.to_string();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

/// Local wall-clock time of a Unix-ms timestamp, `"HH:MM"` (browser time zone).
#[cfg(target_arch = "wasm32")]
pub fn clock(ms: i64) -> String {
    let d = js_sys::Date::new(&(ms as f64).into());
    format!("{:02}:{:02}", d.get_hours(), d.get_minutes())
}

/// Local calendar day of a Unix-ms timestamp, `"Wed 28 Oct"` (browser time zone).
#[cfg(target_arch = "wasm32")]
pub fn day(ms: i64) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let d = js_sys::Date::new(&(ms as f64).into());
    let wd = DAYS.get(d.get_day() as usize).copied().unwrap_or("");
    let mo = MONTHS.get(d.get_month() as usize).copied().unwrap_or("");
    format!("{wd} {} {mo}", d.get_date())
}

/// The day of a game to come, against `now_ms`: `"Today"`, `"Tomorrow"`, else [`day`] (browser
/// time zone; calendar days, so a DST change does not shift them).
#[cfg(target_arch = "wasm32")]
pub fn day_label(ms: i64, now_ms: i64) -> String {
    let ymd = |d: &js_sys::Date| (d.get_full_year(), d.get_month(), d.get_date());
    let at = ymd(&js_sys::Date::new(&(ms as f64).into()));
    let now = js_sys::Date::new(&(now_ms as f64).into());
    if at == ymd(&now) {
        return "Today".to_string();
    }
    now.set_date(now.get_date() + 1);
    if at == ymd(&now) {
        return "Tomorrow".to_string();
    }
    day(ms)
}

/// `"Wed 28 Oct, 21:00"` (browser time zone).
#[cfg(target_arch = "wasm32")]
pub fn when(ms: i64) -> String {
    format!("{}, {}", day(ms), clock(ms))
}

/// A live game's clock: the feed's minute (`"67′"`), half-time, the quarter and its clock
/// (`"Q3 4:12"`), or with no feed the minutes since the start (`"50′"`).
pub fn match_clock(period: Option<&str>, elapsed: Option<&str>, minutes: i64) -> String {
    let elapsed = elapsed.map(str::trim).filter(|e| !e.is_empty());
    match (period.map(str::trim).filter(|p| !p.is_empty()), elapsed) {
        (Some("HT"), _) => "Half-time".to_string(),
        (Some(p), Some(e)) if p.starts_with('Q') || p.starts_with("OT") => format!("{p} {e}"),
        (Some(p), None) if p.starts_with('Q') || p.starts_with("OT") => p.to_string(),
        (_, Some(e)) if e.chars().all(|c| c.is_ascii_digit() || c == '+') => format!("{e}\u{2032}"),
        (Some(p), _) => p.to_string(),
        (None, _) => format!("{minutes}\u{2032}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_clocks() {
        assert_eq!(match_clock(Some("2H"), Some("67"), 80), "67′");
        assert_eq!(match_clock(Some("2H"), Some("90+3"), 80), "90+3′");
        assert_eq!(match_clock(Some("HT"), Some(""), 50), "Half-time");
        assert_eq!(match_clock(Some("Q3"), Some("4:12"), 90), "Q3 4:12");
        assert_eq!(match_clock(Some("OT"), None, 150), "OT");
        assert_eq!(match_clock(Some("BT"), None, 150), "BT");
        assert_eq!(match_clock(None, None, 50), "50′");
    }

    #[test]
    fn euros_truncate_to_the_cent() {
        assert_eq!(eur(13_956_834), "€13.95");
        assert_eq!(eur(0), "€0.00");
        assert_eq!(eur(9_999), "€0.00");
        assert_eq!(eur(10_000), "€0.01");
        assert_eq!(eur(431_655), "€0.43");
        assert_eq!(eur(1_234_567_890_000), "€1,234,567.89");
        assert_eq!(eur(u64::MAX), "€18,446,744,073,709.55");
    }

    #[test]
    fn token_amounts_are_exact_and_grouped() {
        assert_eq!(token(1_000_000_000, 5), "10,000");
        assert_eq!(token(1_234_567_850, 5), "12,345.6785");
        assert_eq!(token(5, 5), "0.00005");
        assert_eq!(token(0, 6), "0");
    }

    #[test]
    fn fine_euros_and_percentages() {
        assert_eq!(eur_fine(5_592), "€0.0055");
        assert_eq!(eur_fine(1_000_000), "€1.0000");
        assert_eq!(pct_ppm(50_000), "5%");
        assert_eq!(pct_ppm(30_000), "3%");
        assert_eq!(pct_ppm(87_000), "8.7%");
    }

    #[test]
    fn signed_rates() {
        assert_eq!(pct_signed(182_000), "+18.2%");
        assert_eq!(pct_signed(-12_000), "\u{2212}1.2%");
        assert_eq!(pct_signed(-310_000), "\u{2212}31%");
        assert_eq!(pct_signed(-999), "0%");
        assert_eq!(pct_signed(0), "0%");
        assert_eq!(pct_signed(1_000_000), "+100%");
        assert_eq!(pct_signed(i64::MIN), "\u{2212}922337203685477.5%");
        assert_eq!(chance(699_000), "69.9%");
        assert_eq!(chance(2_000_000), "100%");
    }

    #[test]
    fn odds_truncate_to_two_places() {
        assert_eq!(odds(1_438_848), "1.43");
        assert_eq!(odds(1_395_683), "1.39");
        assert_eq!(odds(3_278_688), "3.27");
        assert_eq!(odds(1_000_000), "1.00");
        assert_eq!(odds(1_009_999), "1.00");
    }

    #[test]
    fn shortens_ids() {
        assert_eq!(
            short("455UxaLA8AQkpSkYPMPwyhTtH4yuDbSGBmjv9S2yuHhC"),
            "455U…uHhC"
        );
        assert_eq!(short("abc"), "abc");
    }
}
