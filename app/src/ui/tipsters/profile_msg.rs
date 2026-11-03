//! The `PUT /profile` messages a wallet signs (l6-contracts L6-2; v2 with links and follows,
//! l7-contracts L7-1 and D-471), the field rules the desk applies (6.0c and 7.0c
//! `crates/desk/src/profiles`), and the base64 of the signature.
//!
//! Dependency-free on purpose, so it is tested on the host by itself (the app only builds for
//! wasm32): `rustc --edition 2024 --test app/src/ui/tipsters/profile_msg.rs -o <bin> && <bin>`.

/// The message tag (the desk's `profiles::DOMAIN`).
pub const DOMAIN: &str = "betfi/profile/v1";
/// The v2 message tag (the desk's `profiles::DOMAIN_V2`), for a request with links or a follow.
pub const DOMAIN_V2: &str = "betfi/profile/v2";
/// Longest tip title, characters after trimming.
pub const MAX_TITLE_CHARS: usize = 40;
/// Words no handle may contain (D-331).
pub const RESERVED: [&str; 12] = [
    "betfi",
    "schedina",
    "pascal",
    "admin",
    "official",
    "support",
    "verified",
    "moderator",
    "desk",
    "house",
    "system",
    "null",
];

/// The exact UTF-8 text `wallet` signs:
/// `betfi/profile/v1\n<wallet>\n<handle>\n<ticket or ->\n<title or ->\n<ts>`, no trailing
/// newline, `ts` in decimal Unix seconds, the title exactly as sent.
pub fn message(
    wallet: &str,
    handle: &str,
    ticket: Option<&str>,
    title: Option<&str>,
    ts: i64,
) -> String {
    format!(
        "{DOMAIN}\n{wallet}\n{handle}\n{}\n{}\n{ts}",
        ticket.unwrap_or("-"),
        title.unwrap_or("-")
    )
}

/// A request's four links in the desk's order (X, YouTube, Telegram, Instagram), each a bare
/// handle or `None` (cleared).
pub type Links<'a> = [Option<&'a str>; 4];

/// The exact UTF-8 text `wallet` signs for a request carrying `links` or `follow` (D-471, the
/// desk's `profiles::message_v2`):
/// `betfi/profile/v2\n<wallet>\n<handle or ->\n<ticket or ->\n<title or ->\n<links>\n<follow>\n<ts>`,
/// where `<links>` is `-` (no `links`) or `x=<x or ->;youtube=<…>;telegram=<…>;instagram=<…>`
/// and `<follow>` is `-` or `<wallet>:on` / `<wallet>:off`. `handle` is `""` on a follow-only
/// request (sent as `-`). No trailing newline; values exactly as sent. Callers pass checked
/// values only (a base58 wallet, a desk-valid handle and links), so none carries `\n`, `;`, `=`
/// or `:` and the framing stays unambiguous.
pub fn message_v2(
    wallet: &str,
    handle: &str,
    ticket: Option<&str>,
    title: Option<&str>,
    links: Option<Links<'_>>,
    follow: Option<(&str, bool)>,
    ts: i64,
) -> String {
    let links = match links {
        None => "-".to_owned(),
        Some([x, youtube, telegram, instagram]) => format!(
            "x={};youtube={};telegram={};instagram={}",
            x.unwrap_or("-"),
            youtube.unwrap_or("-"),
            telegram.unwrap_or("-"),
            instagram.unwrap_or("-")
        ),
    };
    let follow = match follow {
        None => "-".to_owned(),
        Some((w, on)) => format!("{w}:{}", if on { "on" } else { "off" }),
    };
    let handle = if handle.is_empty() { "-" } else { handle };
    format!(
        "{DOMAIN_V2}\n{wallet}\n{handle}\n{}\n{}\n{links}\n{follow}\n{ts}",
        ticket.unwrap_or("-"),
        title.unwrap_or("-")
    )
}

/// Why `h` is not a handle the desk takes, in plain words; `None` if it is one
/// (`^[a-z0-9_]{3,20}$`, no reserved word).
pub fn handle_problem(h: &str) -> Option<String> {
    if !(3..=20).contains(&h.len()) {
        return Some("a handle has 3 to 20 characters".into());
    }
    if !h
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        return Some("use lower-case letters, digits and _ only".into());
    }
    RESERVED
        .iter()
        .find(|r| h.contains(*r))
        .map(|r| format!("a handle cannot contain \u{201c}{r}\u{201d}"))
}

/// Invisible or direction-changing characters a title may not carry (the desk's list).
fn is_format_char(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2069}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}')
}

/// The title to send (trimmed) or why it is refused: 1 to 40 characters, no control or
/// invisible characters, not `-` alone.
pub fn clean_title(t: &str) -> Result<String, String> {
    if t.chars().any(|c| c.is_control() || is_format_char(c)) {
        return Err("a name cannot hold control or invisible characters".into());
    }
    let trimmed = t.trim();
    if trimmed.is_empty() || trimmed == "-" {
        return Err("type a name first".into());
    }
    if trimmed.chars().count() > MAX_TITLE_CHARS {
        return Err(format!("a name has at most {MAX_TITLE_CHARS} characters"));
    }
    Ok(trimmed.to_owned())
}

/// Standard base64 with padding (RFC 4648 section 4), as the desk decodes `sig`.
pub fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ABC[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const WALLET: &str = "AfEyyLojZei24gnkvwMeELENHo5MnxRG22jDUuvhpXHu";
    const TICKET: &str = "AdT3mBFikZe7V6rpN51eCGw8N8wqdezxG2X4s5mNjSge";

    #[test]
    fn the_message_is_byte_for_byte_l6_2() {
        let m = message(WALLET, "lorenzo_hoops", None, None, 1_793_301_600);
        assert_eq!(
            m.as_bytes(),
            b"betfi/profile/v1\nAfEyyLojZei24gnkvwMeELENHo5MnxRG22jDUuvhpXHu\nlorenzo_hoops\n-\n-\n1793301600"
        );
        let t = message(
            WALLET,
            "lorenzo_hoops",
            Some(TICKET),
            Some("Thursday treble"),
            1_793_301_600,
        );
        assert_eq!(
            t,
            format!(
                "betfi/profile/v1\n{WALLET}\nlorenzo_hoops\n{TICKET}\nThursday treble\n1793301600"
            )
        );
        assert!(!t.ends_with('\n'));
        // UTF-8 as typed (the title is not escaped or normalised).
        let u = message(WALLET, "abc", Some(TICKET), Some("Fenerbahçe ✓"), 1);
        assert!(u.as_bytes().ends_with("Fenerbahçe ✓\n1".as_bytes()));
        assert_eq!(u.lines().count(), 6);
    }

    #[test]
    fn the_v2_message_is_byte_for_byte_the_desks() {
        // The desk's own vectors (crates/desk/src/profiles: the_v2_message_frames_links_and_follows).
        assert_eq!(
            message_v2(
                "W",
                "lorenzo_hoops",
                None,
                None,
                Some([Some("@lorenzo_hoops"), None, Some("lorenzo_tg"), None]),
                None,
                7
            ),
            "betfi/profile/v2\nW\nlorenzo_hoops\n-\n-\nx=@lorenzo_hoops;youtube=-;telegram=lorenzo_tg;instagram=-\n-\n7"
        );
        assert_eq!(
            message_v2("W", "", None, None, None, Some(("T1pst3r", true)), 7),
            "betfi/profile/v2\nW\n-\n-\n-\n-\nT1pst3r:on\n7"
        );
        // The app's follow: a wallet without a handle unfollowing a tipster.
        let m = message_v2(
            WALLET,
            "",
            None,
            None,
            None,
            Some((TICKET, false)),
            1_793_301_600,
        );
        assert_eq!(
            m.as_bytes(),
            format!("betfi/profile/v2\n{WALLET}\n-\n-\n-\n-\n{TICKET}:off\n1793301600").as_bytes()
        );
        assert!(!m.ends_with('\n'));
        assert_eq!(m.lines().count(), 8);
        // No links and all-cleared links are different messages; v2 never reads as v1.
        assert_ne!(
            message_v2("W", "h", None, None, None, None, 7),
            message_v2("W", "h", None, None, Some([None; 4]), None, 7)
        );
        assert_ne!(
            message_v2("W", "h", None, None, None, None, 7),
            message("W", "h", None, None, 7)
        );
    }

    #[test]
    fn handles_follow_the_desk_rule() {
        assert_eq!(handle_problem("lorenzo_hoops"), None);
        assert_eq!(handle_problem("abc"), None);
        assert_eq!(handle_problem("a2345678901234567890"), None);
        assert!(handle_problem("ab").is_some());
        assert!(handle_problem("a23456789012345678901").is_some());
        assert!(handle_problem("Lorenzo").is_some());
        assert!(handle_problem("lo-renzo").is_some());
        assert!(handle_problem("l\u{f6}renzo").is_some());
        assert!(handle_problem("the_betfi_guy").is_some());
        assert!(handle_problem("nullable").is_some());
    }

    #[test]
    fn titles_are_trimmed_and_bounded() {
        assert_eq!(
            clean_title("  Thursday treble ").as_deref(),
            Ok("Thursday treble")
        );
        assert!(clean_title("   ").is_err());
        assert!(clean_title("-").is_err());
        assert!(clean_title("a\nb").is_err());
        assert!(clean_title("a\u{202E}b").is_err());
        assert!(clean_title(&"é".repeat(40)).is_ok());
        assert!(clean_title(&"é".repeat(41)).is_err());
    }

    #[test]
    fn base64_is_rfc_4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xfb, 0xff, 0xbf]), "+/+/");
        // A 64-byte signature: 88 characters, two of them padding.
        let sig = base64(&[7u8; 64]);
        assert_eq!(sig.len(), 88);
        assert!(sig.ends_with("=="));
    }
}
