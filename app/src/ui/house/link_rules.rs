//! The profile links' per-site rules the desk applies (D-473; 7.0c `crates/desk/src/profiles`
//! `check_link`), so Account's editor refuses a link before the wallet signs. The v2 message
//! itself is 7.1d's `tipsters/profile_msg.rs` (`message_v2`).
//!
//! Dependency-free on purpose, like `profile_msg.rs`, so it is tested on the host by itself:
//! `rustc --edition 2024 --test app/src/ui/house/link_rules.rs -o <bin> && <bin>`.

/// A social network of a profile link, in the v2 message's order (x, youtube, telegram,
/// instagram).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Net {
    X,
    Youtube,
    Telegram,
    Instagram,
}

impl Net {
    pub const ALL: [Net; 4] = [Net::X, Net::Youtube, Net::Telegram, Net::Instagram];

    /// The key in the v2 message and in the JSON body.
    pub fn key(self) -> &'static str {
        match self {
            Net::X => "x",
            Net::Youtube => "youtube",
            Net::Telegram => "telegram",
            Net::Instagram => "instagram",
        }
    }

    /// The name shown on a field.
    pub fn name(self) -> &'static str {
        match self {
            Net::X => "X",
            Net::Youtube => "YouTube",
            Net::Telegram => "Telegram",
            Net::Instagram => "Instagram",
        }
    }
}

/// The link to send for what was typed in `net`'s field: `None` for an empty field (the link is
/// cleared), else the bare handle without one leading `@`, or why the desk would refuse it (a
/// URL, a path, spaces, a wrong length; D-473).
pub fn clean_link(net: Net, typed: &str) -> Result<Option<String>, String> {
    let t = typed.trim();
    if t.is_empty() {
        return Ok(None);
    }
    let h = t.strip_prefix('@').unwrap_or(t);
    let (min, max, extra): (usize, usize, &[u8]) = match net {
        Net::X => (1, 15, b"_"),
        Net::Youtube => (3, 30, b"._-"),
        Net::Telegram => (5, 32, b"_"),
        Net::Instagram => (1, 30, b"._"),
    };
    let chars_ok = h
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || extra.contains(&b));
    let shape_ok = match net {
        Net::Telegram => h.bytes().next().is_some_and(|b| b.is_ascii_alphabetic()),
        Net::Instagram => !h.starts_with('.') && !h.ends_with('.') && !h.contains(".."),
        Net::X | Net::Youtube => true,
    };
    if !(min..=max).contains(&h.len()) || !chars_ok || !shape_ok {
        return Err(format!(
            "{}: the bare handle ({min} to {max} characters), not a link",
            net.name()
        ));
    }
    Ok(Some(h.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_follow_the_desk_rules() {
        assert_eq!(clean_link(Net::X, "  "), Ok(None));
        assert_eq!(
            clean_link(Net::X, "@lorenzo_hoops"),
            Ok(Some("lorenzo_hoops".into()))
        );
        assert!(clean_link(Net::X, "https://x.com/lorenzo").is_err());
        assert!(clean_link(Net::X, "a_very_long_handle_x").is_err());
        assert_eq!(
            clean_link(Net::Youtube, "lo.ren-zo"),
            Ok(Some("lo.ren-zo".into()))
        );
        assert!(clean_link(Net::Youtube, "lo").is_err());
        assert!(clean_link(Net::Telegram, "1abcde").is_err());
        assert!(clean_link(Net::Telegram, "abcd").is_err());
        assert_eq!(clean_link(Net::Telegram, "abcde"), Ok(Some("abcde".into())));
        assert!(clean_link(Net::Instagram, ".lorenzo").is_err());
        assert!(clean_link(Net::Instagram, "lo..renzo").is_err());
        assert!(clean_link(Net::Instagram, "lo renzo").is_err());
        assert_eq!(
            clean_link(Net::Instagram, "lo.renzo"),
            Ok(Some("lo.renzo".into()))
        );
    }
}
