// Every instruction module exports a `handler`; the program calls them by path.
#![allow(ambiguous_glob_reexports)]

pub mod cash_out;
#[cfg(feature = "devnet")]
pub mod faucet_usdc;
pub mod finalize;
pub mod fund_vault;
pub mod init_config;
pub mod init_tipster_profile;
pub mod open_pending;
pub mod refund_pending;
pub mod resolve_leg;
pub mod set_params;
pub mod settle;
pub mod withdraw_house;

pub use cash_out::*;
#[cfg(feature = "devnet")]
pub use faucet_usdc::*;
pub use finalize::*;
pub use fund_vault::*;
pub use init_config::*;
pub use init_tipster_profile::*;
pub use open_pending::*;
pub use refund_pending::*;
pub use resolve_leg::*;
pub use set_params::*;
pub use settle::*;
pub use withdraw_house::*;
