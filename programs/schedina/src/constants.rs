//! PDA seeds, message tags and fixed parameters (spec section 4, L1 contracts C1-C3).

use anchor_lang::prelude::*;

/// `Config` PDA: `["config"]`. Also the token authority of the mock mint and of both
/// program token accounts.
#[constant]
pub const CONFIG_SEED: &[u8] = b"config";
/// `Vault` PDA: `["vault"]` (accounting of the house token account).
#[constant]
pub const VAULT_SEED: &[u8] = b"vault";
/// House USDC token account (`Vault.usdc_ata`), PDA `["vault_usdc"]`, owner `Config`.
#[constant]
pub const VAULT_USDC_SEED: &[u8] = b"vault_usdc";
/// Escrow token account holding every open `Pending`'s stake, PDA `["escrow_usdc"]`,
/// owner `Config`.
#[constant]
pub const ESCROW_USDC_SEED: &[u8] = b"escrow_usdc";
/// Mock mints (`MockMint`): `["mock", symbol]`; mock USDC is `["mock", "USDC"]`.
#[constant]
pub const MOCK_SEED: &[u8] = b"mock";
#[constant]
pub const USDC_SYMBOL: &[u8] = b"USDC";
/// `Pending` PDA: `["pending", user, nonce u64 LE]`.
#[constant]
pub const PENDING_SEED: &[u8] = b"pending";
/// `Ticket` PDA: `["ticket", user, nonce u64 LE]` (same nonce as its `Pending`).
#[constant]
pub const TICKET_SEED: &[u8] = b"ticket";
/// `TipsterProfile` PDA: `["tipster", wallet]` (step 2.F).
#[constant]
pub const TIPSTER_SEED: &[u8] = b"tipster";
/// `FaucetClaim` PDA: `["faucet", owner]` (`devnet` builds only).
#[constant]
pub const FAUCET_SEED: &[u8] = b"faucet";

/// First bytes of every signed quote message (C3 domain separation).
#[constant]
pub const QUOTE_TAG: [u8; 17] = *b"schedina/quote/v1";
/// First bytes of every signed cash-out message.
#[constant]
pub const CASHOUT_TAG: [u8; 19] = *b"schedina/cashout/v1";

/// Mock USDC decimals (micro-USDC units).
#[constant]
pub const USDC_DECIMALS: u8 = 6;

/// `1.0` in micro-units (prices, probabilities, odds, `_ppm` ratios).
#[constant]
pub const ONE: u64 = 1_000_000;
/// Legs per ticket the accounts have room for; `Config.max_legs` is at most this.
pub const MAX_LEGS: usize = 5;
/// Basis-point denominator.
pub const BPS: u16 = 10_000;

/// `Pending.expires_at` must lie in `[now + MIN, now + MAX]` at `open_pending` (C3). MAX was
/// 60 s; 360 s since D-664, so a wallet prompt may take as long as the 5 min quote.
#[constant]
pub const PENDING_TTL_MIN: i64 = 30;
#[constant]
pub const PENDING_TTL_MAX: i64 = 360;
/// `refund_pending` opens this long after `expires_at`, so a `finalize` sent before
/// expiry cannot lose the race.
#[constant]
pub const REFUND_GRACE: i64 = 15;
/// A cash-out quote is valid while `|now - ts| <= CASHOUT_TTL`.
#[constant]
pub const CASHOUT_TTL: i64 = 30;

/// Leg sides (C3 `side u8`).
pub const SIDE_LONG: u8 = 0;
pub const SIDE_SHORT: u8 = 1;

/// Largest `faucet_usdc` mint per claim: 10,000 mock USDC.
#[constant]
pub const FAUCET_MAX: u64 = 10_000_000_000u64;
/// Seconds between two `faucet_usdc` claims by the same recipient owner. Unused since D-661
/// (no cooldown); kept so the 1.C IDL's constants stay as they were.
#[constant]
pub const FAUCET_COOLDOWN: i64 = 3_600;

/// The ed25519 signature-verify precompile.
pub const ED25519_PROGRAM_ID: Pubkey = pubkey!("Ed25519SigVerify111111111111111111111111111");
/// The instructions sysvar.
pub const INSTRUCTIONS_SYSVAR_ID: Pubkey = pubkey!("Sysvar1nstructions1111111111111111111111111");
