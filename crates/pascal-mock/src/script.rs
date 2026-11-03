//! `script.toml`: scripted market resolutions for a tape.
//!
//! The plan's line `resolve <symbol> <price> at <clock>` is written as one `[[resolve]]` table:
//!
//! ```toml
//! [[resolve]]
//! symbol = "NBA_26OCT28_PHIIND_ML.PHI"
//! price = "1"          # settlement price in [0, 1]: "1" long side won, "0" short side won
//! at = "wall+40s"      # when, on the mock clock (see below)
//! ```
//!
//! `at` ([`At`]) is one of:
//!
//! - an absolute tape time in Unix ms, as an integer or a string: `1793242800000`;
//! - `start+<dur>`: tape time after the clock's start (`--start`, default the first snapshot);
//! - `wall+<dur>`: wall-clock time after the mock booted, whatever the speed: the resolution
//!   fires once `dur` of real time has passed, at the tape time the clock shows then
//!   (`wall+40s` at `--speed 60` lands 40 minutes into the tape; at `--speed 0` it fires 40 s
//!   after boot at the frozen time, never at boot; speed changes do not move it) (D-090);
//! - `now` (control endpoint only): the current tape time.
//!
//! `<dur>` is an integer with a unit: `ms`, `s`, `m` or `h` (`90s`, `40m`, `2h`). `price` may be
//! a string or a TOML number; it must be exact to 6 decimal places.

use std::path::Path;

use pascal_rs::MicroUsd;
use pascal_rs::num::parse_micros;
use serde::Deserialize;

/// Why a script or an `at` expression is invalid.
#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    /// TOML syntax or schema error.
    #[error("script.toml: {0}")]
    Toml(#[from] toml::de::Error),
    /// File read failure.
    #[error("reading {0}: {1}")]
    Io(String, std::io::Error),
    /// Bad `at`.
    #[error("invalid time {0:?}: expected tape ms, start+<dur>, wall+<dur> or now")]
    At(String),
    /// Bad price.
    #[error("invalid resolution price {0:?}: expected a decimal in [0, 1] with at most 6 d.p.")]
    Price(String),
}

/// When an event happens on the mock clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum At {
    /// Absolute tape time, Unix ms.
    Tape(u64),
    /// Tape ms after the clock start.
    AfterStart(u64),
    /// Wall-clock ms after boot, independent of the clock speed (D-090).
    AfterBoot(u64),
    /// The current tape time.
    Now,
}

impl At {
    /// Parses an `at` expression (see the module docs).
    pub fn parse(s: &str) -> Result<At, ScriptError> {
        let t = s.trim();
        let bad = || ScriptError::At(s.to_owned());
        if t == "now" {
            return Ok(At::Now);
        }
        if let Some(d) = t.strip_prefix("start+") {
            return parse_duration_ms(d).map(At::AfterStart).ok_or_else(bad);
        }
        if let Some(d) = t.strip_prefix("wall+") {
            return parse_duration_ms(d).map(At::AfterBoot).ok_or_else(bad);
        }
        t.parse().map(At::Tape).map_err(|_| bad())
    }

    /// The absolute tape time, given the clock start and the current tape time; `None` for
    /// [`At::AfterBoot`], which is a wall-clock deadline rather than a tape time (D-090).
    pub fn tape_ms(self, start_ms: u64, now_ms: u64) -> Option<u64> {
        match self {
            At::Tape(ms) => Some(ms),
            At::AfterStart(d) => Some(start_ms.saturating_add(d)),
            At::AfterBoot(_) => None,
            At::Now => Some(now_ms),
        }
    }
}

fn parse_duration_ms(s: &str) -> Option<u64> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit())?;
    let (num, unit) = s.split_at(split);
    let n: u64 = num.parse().ok()?;
    let mult = match unit {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        _ => return None,
    };
    n.checked_mul(mult)
}

/// Parses a settlement price in `[0, 1]`.
pub fn parse_price(s: &str) -> Result<MicroUsd, ScriptError> {
    match parse_micros(s.trim()) {
        Ok(p) if (0..=1_000_000).contains(&p) => Ok(p),
        _ => Err(ScriptError::Price(s.to_owned())),
    }
}

/// One scripted resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptedResolution {
    /// Market symbol.
    pub symbol: String,
    /// Settlement price, micro-USD.
    pub price_u: MicroUsd,
    /// When.
    pub at: At,
}

/// A parsed `script.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Script {
    /// Resolutions, in file order.
    pub resolve: Vec<ScriptedResolution>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScript {
    #[serde(default)]
    resolve: Vec<RawResolve>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawResolve {
    symbol: String,
    price: NumOrStr,
    at: NumOrStr,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum NumOrStr {
    Int(i64),
    Float(f64),
    Str(String),
}

impl NumOrStr {
    fn text(&self) -> String {
        match self {
            NumOrStr::Int(i) => i.to_string(),
            NumOrStr::Float(f) => f.to_string(),
            NumOrStr::Str(s) => s.clone(),
        }
    }
}

impl Script {
    /// Parses a script from TOML text.
    pub fn parse(text: &str) -> Result<Script, ScriptError> {
        let raw: RawScript = toml::from_str(text)?;
        let resolve = raw
            .resolve
            .into_iter()
            .map(|r| {
                Ok(ScriptedResolution {
                    price_u: parse_price(&r.price.text())?,
                    at: At::parse(&r.at.text())?,
                    symbol: r.symbol,
                })
            })
            .collect::<Result<_, ScriptError>>()?;
        Ok(Script { resolve })
    }

    /// Reads and parses a script file.
    pub fn load(path: &Path) -> Result<Script, ScriptError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| ScriptError::Io(path.display().to_string(), e))?;
        Script::parse(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_form() {
        let s = Script::parse(
            r#"
            [[resolve]]
            symbol = "A.B"
            price = "1"
            at = "wall+40s"

            [[resolve]]
            symbol = "C.D"
            price = 0
            at = 1793242800000

            [[resolve]]
            symbol = "E.F"
            price = 0.5
            at = "start+2h"
            "#,
        )
        .unwrap();
        assert_eq!(s.resolve.len(), 3);
        assert_eq!(s.resolve[0].price_u, 1_000_000);
        assert_eq!(s.resolve[0].at, At::AfterBoot(40_000));
        assert_eq!(s.resolve[1].price_u, 0);
        assert_eq!(s.resolve[1].at, At::Tape(1_793_242_800_000));
        assert_eq!(s.resolve[2].price_u, 500_000);
        assert_eq!(s.resolve[2].at, At::AfterStart(7_200_000));
        assert_eq!(s.resolve[0].at.tape_ms(1_000, 5), None);
        assert_eq!(s.resolve[1].at.tape_ms(1_000, 5), Some(1_793_242_800_000));
        assert_eq!(s.resolve[2].at.tape_ms(1_000, 5), Some(7_201_000));
        assert_eq!(At::Now.tape_ms(1_000, 5), Some(5));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(At::parse("wall+40").is_err());
        assert!(At::parse("later").is_err());
        assert!(At::parse("start+5d").is_err());
        assert!(parse_price("1.5").is_err());
        assert!(parse_price("-0.1").is_err());
        assert!(parse_price("0.0000001").is_err());
        assert!(Script::parse("[[resolve]]\nsymbol = \"A\"\nprice = 1\n").is_err());
        assert!(Script::parse("[[resolv]]\n").is_err());
        assert_eq!(Script::parse("").unwrap(), Script::default());
    }

    #[test]
    fn the_tape_script_parses() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tapes/nba-2026-10-28/script.toml");
        let s = Script::load(&path).unwrap();
        assert!(!s.resolve.is_empty());
    }
}
