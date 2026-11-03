//! Solana plumbing shared by the crank and the indexer (2.C), adapted from the sibling crank and
//! made program-agnostic: the program id comes from [`crate::Config::program_id`].
//!
//! - [`signer`]: keypair files, ed25519 signing, send-and-confirm.
//! - [`rpc`]: a small JSON-RPC client; URLs only ever logged through [`rpc::redact`].
//! - [`listen`]: `logsSubscribe` listener with resync, and `emit_cpi!` event extraction.
//!
//! Nothing in 1.E sends a transaction.

pub mod listen;
pub mod rpc;
pub mod signer;
