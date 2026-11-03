//! Typed models of Pascal's read API, field-for-field with the API reference.
//!
//! Field names are Pascal's, so a model serialises back to the same JSON keys. USD amounts and
//! prices are micro-USD `i64` ([`MicroUsd`](crate::num::MicroUsd)), integer strings are `u64` /
//! `i64` / `u128`, and rates stay [`Decimal`](rust_decimal::Decimal). Unknown extra fields are
//! ignored; an unknown enum value or a missing required field is a decode error.

mod account;
mod market;
mod misc;

pub use account::*;
pub use market::*;
pub use misc::*;
