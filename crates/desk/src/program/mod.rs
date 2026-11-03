//! The `schedina` program as the desk sees it (plan 2.C, after the sibling crank's
//! `program.rs`): PDA seeds, the account layouts the indexer reads ([`accounts`]), the
//! `emit_cpi!` events ([`events`]), and the instructions the crank and the CLI send ([`ix`]).
//!
//! The program crate is not a dependency (different toolchain and workspace). Everything here is
//! written from the 1.C source (`programs/schedina/src/`) and checked against the committed IDL
//! (`programs/idl/schedina.json`) by this module's tests: discriminators, account order, signer
//! and writable flags, fixed addresses, argument encoding, account and event layouts. Step 2.F
//! only appends `remaining_accounts` (l2-contracts L2-4), so these stay byte-identical.

pub mod accounts;
pub mod events;
pub mod ix;
#[cfg(test)]
mod tests;

use sha2::{Digest, Sha256};
use solana_pubkey::Pubkey;

pub use accounts::{
    ConfigAccount, DecodeError, LegStatus, PendingAccount, QuoteLeg, TicketAccount, TicketLeg,
    TicketState, VaultAccount,
};
pub use events::ProgramEvent;

/// SPL Token (classic).
pub const TOKEN_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
/// Associated Token Account program.
pub const ASSOCIATED_TOKEN_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
/// System program.
pub const SYSTEM_PROGRAM_ID: Pubkey = Pubkey::from_str_const("11111111111111111111111111111111");
/// Upgradeable BPF loader (owner of the program's `ProgramData`).
pub const BPF_UPGRADEABLE_LOADER_ID: Pubkey =
    Pubkey::from_str_const("BPFLoaderUpgradeab1e11111111111111111111111");
/// The ed25519 signature-verify precompile.
pub const ED25519_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("Ed25519SigVerify111111111111111111111111111");
/// The instructions sysvar.
pub const INSTRUCTIONS_SYSVAR_ID: Pubkey =
    Pubkey::from_str_const("Sysvar1nstructions1111111111111111111111111");

/// PDA seeds (`programs/schedina/src/constants.rs`). Nonces are `u64` little-endian.
pub mod seeds {
    /// `["config"]`.
    pub const CONFIG: &[u8] = b"config";
    /// `["vault"]`.
    pub const VAULT: &[u8] = b"vault";
    /// `["vault_usdc"]`: house token account.
    pub const VAULT_USDC: &[u8] = b"vault_usdc";
    /// `["escrow_usdc"]`: pending stakes.
    pub const ESCROW_USDC: &[u8] = b"escrow_usdc";
    /// `["mock", symbol]`.
    pub const MOCK: &[u8] = b"mock";
    /// Mock USDC symbol.
    pub const USDC: &[u8] = b"USDC";
    /// `["pending", user, nonce]`.
    pub const PENDING: &[u8] = b"pending";
    /// `["ticket", user, nonce]`.
    pub const TICKET: &[u8] = b"ticket";
    /// `["tipster", wallet]`.
    pub const TIPSTER: &[u8] = b"tipster";
    /// `["faucet", owner]` (devnet builds).
    pub const FAUCET: &[u8] = b"faucet";
    /// Anchor's `#[event_cpi]` authority.
    pub const EVENT_AUTHORITY: &[u8] = b"__event_authority";
}

/// Program constants the desk needs (`constants.rs`).
pub mod consts {
    /// Legs a ticket has room for.
    pub const MAX_LEGS: usize = 5;
    /// `refund_pending` opens `expires_at + REFUND_GRACE`.
    pub const REFUND_GRACE: i64 = 15;
    /// Signed quote message tag.
    pub const QUOTE_TAG: [u8; 17] = *b"schedina/quote/v1";
    /// Signed cash-out message tag.
    pub const CASHOUT_TAG: [u8; 19] = *b"schedina/cashout/v1";
    /// `faucet_usdc` per-claim cap (10,000 mock USDC).
    pub const FAUCET_MAX: u64 = 10_000_000_000;
    /// Mock USDC decimals.
    pub const USDC_DECIMALS: u8 = 6;
}

/// Every PDA of one deployment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pdas {
    /// Program id.
    pub program: Pubkey,
}

impl Pdas {
    /// PDAs of `program`.
    pub fn new(program: Pubkey) -> Self {
        Pdas { program }
    }

    fn find(&self, seeds: &[&[u8]]) -> Pubkey {
        Pubkey::find_program_address(seeds, &self.program).0
    }

    /// `Config`.
    pub fn config(&self) -> Pubkey {
        self.find(&[seeds::CONFIG])
    }
    /// `Vault`.
    pub fn vault(&self) -> Pubkey {
        self.find(&[seeds::VAULT])
    }
    /// House token account.
    pub fn vault_usdc(&self) -> Pubkey {
        self.find(&[seeds::VAULT_USDC])
    }
    /// Escrow token account.
    pub fn escrow_usdc(&self) -> Pubkey {
        self.find(&[seeds::ESCROW_USDC])
    }
    /// Mock USDC mint (`Config.usdc_mint`, D-052).
    pub fn usdc_mint(&self) -> Pubkey {
        self.find(&[seeds::MOCK, seeds::USDC])
    }
    /// `Pending` of `user` / `nonce`.
    pub fn pending(&self, user: &Pubkey, nonce: u64) -> Pubkey {
        self.find(&[seeds::PENDING, user.as_ref(), &nonce.to_le_bytes()])
    }
    /// `Ticket` of `user` / `nonce` (same seeds as its pending, D-038).
    pub fn ticket(&self, user: &Pubkey, nonce: u64) -> Pubkey {
        self.find(&[seeds::TICKET, user.as_ref(), &nonce.to_le_bytes()])
    }
    /// `TipsterProfile` of `wallet` (2.F).
    pub fn tipster(&self, wallet: &Pubkey) -> Pubkey {
        self.find(&[seeds::TIPSTER, wallet.as_ref()])
    }
    /// `FaucetClaim` of a recipient token account's owner.
    pub fn faucet_claim(&self, owner: &Pubkey) -> Pubkey {
        self.find(&[seeds::FAUCET, owner.as_ref()])
    }
    /// `#[event_cpi]` authority.
    pub fn event_authority(&self) -> Pubkey {
        self.find(&[seeds::EVENT_AUTHORITY])
    }
    /// The program's `ProgramData` (upgradeable loader).
    pub fn program_data(&self) -> Pubkey {
        Pubkey::find_program_address(&[self.program.as_ref()], &BPF_UPGRADEABLE_LOADER_ID).0
    }
}

/// The classic-SPL-Token associated token account of `owner` for `mint`.
pub fn associated_token_address(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[owner.as_ref(), TOKEN_PROGRAM_ID.as_ref(), mint.as_ref()],
        &ASSOCIATED_TOKEN_PROGRAM_ID,
    )
    .0
}

/// sha256 of a string (symbols and game ids on-chain, D-008).
pub fn sha256(s: &[u8]) -> [u8; 32] {
    Sha256::digest(s).into()
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}
