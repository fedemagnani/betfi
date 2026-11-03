//! Schedina on-chain program (spec section 4, plan step 1.C): a house vault, users'
//! escrowed pending requests, parlay tickets minted by the crank at the achieved
//! hedge prices, resolution, settlement and desk-signed cash-out. Desk quotes and
//! cash-out quotes are verified on-chain with the ed25519 precompile (`sigverify`).
//! Tipster flag, copies, copy-fee split and `TipsterProfile` (step 2.F) take their
//! extra accounts through `remaining_accounts` only (`tipster`, contract L2-4).

use anchor_lang::prelude::*;

pub mod constants;
pub mod error;
pub mod events;
pub mod instructions;
pub mod math;
pub mod messages;
pub mod sigverify;
pub mod state;
pub mod tipster;
pub mod vault;

pub use constants::*;
pub use instructions::*;
pub use messages::*;
pub use state::*;

declare_id!("HXdZFi7tR669G1hXogh3uGGqLYsoZPCSuiutAviLNToW");

#[program]
pub mod schedina {
    use super::*;

    /// Creates `Config`, `Vault`, the mock USDC mint and the vault and escrow token
    /// accounts. Upgrade authority only.
    pub fn init_config(ctx: Context<InitConfig>, authority: Pubkey, params: Params) -> Result<()> {
        init_config::handler(ctx, authority, params)
    }

    /// Replaces the tunable parameters and backend keys. Authority only.
    pub fn set_params(ctx: Context<SetParams>, params: Params) -> Result<()> {
        set_params::handler(ctx, params)
    }

    /// Pauses (or resumes) `open_pending` and `finalize`. Authority only.
    pub fn pause(ctx: Context<SetParams>, paused: bool) -> Result<()> {
        set_params::pause_handler(ctx, paused)
    }

    /// Deposits `amount` micro-USDC into the house vault. Anyone.
    pub fn fund_vault(ctx: Context<FundVault>, amount: u64) -> Result<()> {
        fund_vault::handler(ctx, amount)
    }

    /// Withdraws `amount` from `Vault.free`, never below `Config.reserve`. Authority only.
    pub fn withdraw_house(ctx: Context<WithdrawHouse>, amount: u64) -> Result<()> {
        withdraw_house::handler(ctx, amount)
    }

    /// Escrows the stake of a desk-signed quote with the user's floor. The quote is the
    /// message of the `Ed25519Program` verify instruction immediately before this one
    /// (borsh `QuoteMsg`, signed by `Config.quote_signer`). User signs.
    pub fn open_pending(
        ctx: Context<OpenPending>,
        nonce: u64,
        min_payout_per_unit_u: u64,
        expires_at: i64,
        is_tip: bool,
        copied_from: Option<Pubkey>,
    ) -> Result<()> {
        open_pending::handler(
            ctx,
            nonce,
            min_payout_per_unit_u,
            expires_at,
            is_tip,
            copied_from,
        )
    }

    /// Mints the ticket at fill prices `fills` (0 = unhedged leg) and fill ratio
    /// `r_ppm`, refunding `S - rS` (plus the unspent copy fee). Crank only.
    /// `remaining_accounts`: own tip `[tipster_profile (w)]` (created if absent, crank
    /// pays); copy `[orig_ticket, tipster_usdc (w)]`; otherwise none.
    pub fn finalize<'info>(
        ctx: Context<'info, Finalize<'info>>,
        fills: Vec<u64>,
        r_ppm: u32,
    ) -> Result<()> {
        finalize::handler(ctx, fills, r_ppm)
    }

    /// Returns an unfinalized pending's escrow from `expires_at + 15 s`. Anyone.
    pub fn refund_pending(ctx: Context<RefundPending>) -> Result<()> {
        refund_pending::handler(ctx)
    }

    /// Records Pascal's resolution `r_ppm` of leg `leg_index`. Crank (or authority).
    pub fn resolve_leg(ctx: Context<ResolveLeg>, leg_index: u8, r_ppm: u32) -> Result<()> {
        resolve_leg::handler(ctx, leg_index, r_ppm)
    }

    /// Pays `P × Π r̃` once every leg is resolved. Anyone. `remaining_accounts`: own
    /// tip `[tipster_profile (w)]` (required), otherwise none.
    pub fn settle(ctx: Context<Settle>) -> Result<()> {
        settle::handler(ctx)
    }

    /// Pays the desk-signed cash-out value. Needs the `Ed25519Program` verify
    /// instruction immediately before this one, over exactly `quote`'s borsh bytes.
    /// Ticket user signs. `remaining_accounts`: own tip `[tipster_profile (w)]`
    /// (required), otherwise none.
    pub fn cash_out(ctx: Context<CashOut>, quote: CashOutMsg) -> Result<()> {
        cash_out::handler(ctx, quote)
    }

    /// Creates the `TipsterProfile` of `wallet` if absent (2.F; `finalize` also creates
    /// it for an own tip). Anyone; the payer pays the rent.
    pub fn init_tipster_profile(ctx: Context<InitTipsterProfile>) -> Result<()> {
        init_tipster_profile::handler(ctx)
    }

    /// Mints mock USDC to `recipient`, as often as asked (D-661). `devnet` feature only.
    #[cfg(feature = "devnet")]
    pub fn faucet_usdc(ctx: Context<FaucetUsdc>, amount: u64) -> Result<()> {
        faucet_usdc::handler(ctx, amount)
    }
}
