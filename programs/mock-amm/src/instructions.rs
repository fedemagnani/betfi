// Every instruction module's accounts struct is re-exported for the `#[program]` macro.
#![allow(ambiguous_glob_reexports)]

pub mod admin;
pub mod faucet;
pub mod liquidity;
pub mod swap;

pub use admin::*;
pub use faucet::*;
pub use liquidity::*;
pub use swap::*;
