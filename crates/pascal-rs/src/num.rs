//! Pascal's numeric string encodings.
//!
//! Pascal serialises every number as a JSON string: USD amounts and prices as decimals with six
//! fractional digits (`"0.550000"`, `"-760.488584"`), and sizes, sequence numbers, ids and
//! timestamps as integers (`"12115562204"`). This module parses both:
//!
//! - USD amounts and prices become **micro-USD `i64`** ([`MicroUsd`]): the decimal is parsed
//!   with [`rust_decimal`] and scaled by 10^6. A value carrying a non-zero digit beyond the sixth
//!   decimal place is rejected rather than rounded, so no precision is ever lost silently.
//! - Integer strings become `u64` / `i64` / `u128`, and non-USD decimals (fee rates, rebate
//!   shares, weighted volume) stay [`Decimal`].
//!
//! The `serde` helper modules ([`micros`], [`opt_micros`], [`str_num`], [`opt_str_num`]) are used
//! with `#[serde(with = "…")]` on the typed models and serialise back to Pascal's encoding.

use std::fmt::Display;
use std::str::FromStr;

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Deserializer, Serializer, de};

/// A USD amount or price in millionths of a dollar (`"1.250000"` is `1_250_000`).
pub type MicroUsd = i64;

/// Micro-units per whole unit (10^6).
pub const MICROS_PER_UNIT: i64 = 1_000_000;

/// Error returned when a decimal string cannot be represented as micro-USD.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecimalError {
    /// The string is not a decimal number.
    #[error("invalid decimal string {0:?}")]
    Invalid(String),
    /// The value has a non-zero digit beyond the sixth decimal place.
    #[error("decimal {0:?} has more than 6 significant fractional digits")]
    TooPrecise(String),
    /// The value does not fit in an `i64` count of micro-units.
    #[error("decimal {0:?} overflows i64 micro-units")]
    Overflow(String),
}

/// Parses a decimal string (`"-12.345600"`) into micro-units (`-12_345_600`).
///
/// Any number of fractional digits is accepted as long as the value is exact in micro-units,
/// so `"0.0010"` and `"0.001000"` both give `1_000`, while `"0.0000001"` is rejected.
pub fn parse_micros(s: &str) -> Result<MicroUsd, DecimalError> {
    let d = Decimal::from_str_exact(s).map_err(|_| DecimalError::Invalid(s.to_owned()))?;
    decimal_to_micros(d).map_err(|e| match e {
        DecimalError::TooPrecise(_) => DecimalError::TooPrecise(s.to_owned()),
        DecimalError::Overflow(_) => DecimalError::Overflow(s.to_owned()),
        other => other,
    })
}

/// Converts a [`Decimal`] into micro-units, rejecting values that are not exact or overflow.
pub fn decimal_to_micros(d: Decimal) -> Result<MicroUsd, DecimalError> {
    let scaled = d
        .checked_mul(Decimal::from(MICROS_PER_UNIT))
        .ok_or_else(|| DecimalError::Overflow(d.to_string()))?;
    if !scaled.fract().is_zero() {
        return Err(DecimalError::TooPrecise(d.to_string()));
    }
    scaled
        .to_i64()
        .ok_or_else(|| DecimalError::Overflow(d.to_string()))
}

/// Converts micro-units back into an exact [`Decimal`] with scale 6.
pub fn micros_to_decimal(micros: MicroUsd) -> Decimal {
    Decimal::new(micros, 6)
}

/// Formats micro-units the way Pascal does: always six fractional digits (`-0.500000`).
pub fn format_micros(micros: MicroUsd) -> String {
    let sign = if micros < 0 { "-" } else { "" };
    let abs = i128::from(micros).unsigned_abs();
    let unit = MICROS_PER_UNIT.unsigned_abs() as u128;
    format!("{sign}{}.{:06}", abs / unit, abs % unit)
}

/// Serde helper for a required micro-USD field encoded as a 6-d.p. decimal string.
pub mod micros {
    use super::*;

    /// Serialises micro-units as a 6-d.p. decimal string.
    pub fn serialize<S: Serializer>(v: &MicroUsd, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format_micros(*v))
    }

    /// Deserialises a decimal string into micro-units.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<MicroUsd, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        parse_micros(&s).map_err(de::Error::custom)
    }
}

/// Serde helper for an optional micro-USD field (absent or `null` is `None`).
///
/// Use with `#[serde(default, with = "opt_micros")]`.
pub mod opt_micros {
    use super::*;

    /// Serialises `Some` as a 6-d.p. decimal string and `None` as `null`.
    pub fn serialize<S: Serializer>(v: &Option<MicroUsd>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(v) => s.serialize_str(&format_micros(*v)),
            None => s.serialize_none(),
        }
    }

    /// Deserialises an optional decimal string into micro-units.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<MicroUsd>, D::Error> {
        let s = Option::<std::borrow::Cow<'de, str>>::deserialize(d)?;
        s.map(|s| parse_micros(&s).map_err(de::Error::custom))
            .transpose()
    }
}

/// Serde helper for a required number encoded as a string (`u64`, `i64`, `u128`, [`Decimal`], …).
pub mod str_num {
    use super::*;

    /// Serialises the value with its `Display` impl.
    pub fn serialize<T: Display, S: Serializer>(v: &T, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(v)
    }

    /// Deserialises a JSON string with the target's `FromStr` impl. JSON numbers are rejected:
    /// Pascal always quotes these values.
    pub fn deserialize<'de, T, D>(d: D) -> Result<T, D::Error>
    where
        T: FromStr,
        T::Err: Display,
        D: Deserializer<'de>,
    {
        let s = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        s.parse::<T>()
            .map_err(|e| de::Error::custom(format_args!("invalid numeric string {s:?}: {e}")))
    }
}

/// Serde helper for an optional number encoded as a string.
///
/// Use with `#[serde(default, with = "opt_str_num")]`.
pub mod opt_str_num {
    use super::*;

    /// Serialises `Some` with `Display` and `None` as `null`.
    pub fn serialize<T: Display, S: Serializer>(v: &Option<T>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(v) => s.collect_str(v),
            None => s.serialize_none(),
        }
    }

    /// Deserialises an optional JSON string with the target's `FromStr` impl.
    pub fn deserialize<'de, T, D>(d: D) -> Result<Option<T>, D::Error>
    where
        T: FromStr,
        T::Err: Display,
        D: Deserializer<'de>,
    {
        let s = Option::<std::borrow::Cow<'de, str>>::deserialize(d)?;
        s.map(|s| {
            s.parse::<T>()
                .map_err(|e| de::Error::custom(format_args!("invalid numeric string {s:?}: {e}")))
        })
        .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn parses_known_values() {
        assert_eq!(parse_micros("0.550000"), Ok(550_000));
        assert_eq!(parse_micros("-760.488584"), Ok(-760_488_584));
        assert_eq!(parse_micros("0.0010"), Ok(1_000));
        assert_eq!(parse_micros("1"), Ok(1_000_000));
        assert_eq!(parse_micros("-0.000000"), Ok(0));
        assert_eq!(parse_micros("0.000001"), Ok(1));
        assert!(matches!(
            parse_micros("0.0000001"),
            Err(DecimalError::TooPrecise(_))
        ));
        assert_eq!(parse_micros("0.10000000"), Ok(100_000));
        assert!(matches!(parse_micros("abc"), Err(DecimalError::Invalid(_))));
        assert!(matches!(parse_micros(""), Err(DecimalError::Invalid(_))));
        assert!(matches!(
            parse_micros("9223372036854.775808"),
            Err(DecimalError::Overflow(_))
        ));
    }

    #[test]
    fn extremes_round_trip() {
        for v in [i64::MIN, i64::MIN + 1, -1, 0, 1, i64::MAX] {
            let s = format_micros(v);
            assert_eq!(parse_micros(&s), Ok(v), "{s}");
        }
        assert_eq!(format_micros(i64::MIN), "-9223372036854.775808");
        assert_eq!(format_micros(i64::MAX), "9223372036854.775807");
        assert_eq!(format_micros(-1), "-0.000001");
        assert_eq!(format_micros(-500_000), "-0.500000");
    }

    proptest! {
        /// micro-USD -> 6-d.p. string -> micro-USD is the identity on all of `i64`.
        #[test]
        fn micros_string_micros(v in any::<i64>()) {
            let s = format_micros(v);
            prop_assert_eq!(parse_micros(&s), Ok(v));
            prop_assert_eq!(micros_to_decimal(v).to_string(), s);
        }

        /// A canonical 6-d.p. string -> micro-USD -> string is the identity, including
        /// negatives and the `i64` extremes.
        #[test]
        fn string_micros_string(
            neg in any::<bool>(),
            int in 0u64..=9_223_372_036_853u64,
            frac in 0u32..1_000_000u32,
        ) {
            let s = format!("{}{int}.{frac:06}", if neg && (int, frac) != (0, 0) { "-" } else { "" });
            let v = parse_micros(&s).unwrap();
            prop_assert_eq!(format_micros(v), s);
        }

        /// Extra trailing zeros beyond six digits never change the value.
        #[test]
        fn trailing_zeros_are_exact(v in any::<i64>(), zeros in 1usize..6) {
            let s = format!("{}{}", format_micros(v), "0".repeat(zeros));
            prop_assert_eq!(parse_micros(&s), Ok(v));
        }

        /// Any non-zero seventh digit is rejected, never rounded.
        #[test]
        fn seventh_digit_rejected(v in -1_000_000_000i64..1_000_000_000, d in 1u8..10) {
            let s = format!("{}{d}", format_micros(v));
            prop_assert!(matches!(parse_micros(&s), Err(DecimalError::TooPrecise(_))));
        }
    }
}
