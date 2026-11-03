//! Program error codes.

use anchor_lang::prelude::*;

#[error_code]
pub enum SchedinaError {
    #[msg("signer is not the program's upgrade authority")]
    NotUpgradeAuthority,
    #[msg("signer is not Config.authority")]
    NotAuthority,
    #[msg("signer is not Config.crank")]
    NotCrank,
    #[msg("signer is neither Config.crank nor Config.authority")]
    NotResolver,
    #[msg("invalid config parameter")]
    InvalidParams,
    #[msg("the program is paused")]
    Paused,
    #[msg("amount must be positive")]
    ZeroAmount,
    #[msg("arithmetic overflow")]
    MathOverflow,

    // --- signed messages ---
    #[msg("the instruction right before this one is not an Ed25519Program verify")]
    SignatureMissing,
    #[msg("Ed25519Program verify must carry exactly one signature, all offsets in itself")]
    SignatureMalformed,
    #[msg("the verified message is not signed by Config.quote_signer")]
    WrongQuoteSigner,
    #[msg("the verified message differs from this instruction's arguments")]
    QuoteMismatch,
    #[msg("the verified message is not a borsh-encoded QuoteMsg")]
    QuoteMalformed,
    #[msg("wrong message tag")]
    BadTag,
    #[msg("message signed for another program id")]
    WrongProgramId,

    // --- open_pending ---
    #[msg("the desk quote has expired")]
    QuoteExpired,
    #[msg("leg count is 0 or above Config.max_legs")]
    TooManyLegs,
    #[msg("two legs on the same game id")]
    DuplicateGame,
    #[msg("invalid leg: side must be 0 or 1, price in (0, 1e6], m1 below 1e6")]
    InvalidLeg,
    #[msg("stake below Config.min_stake or different from the quoted stake")]
    StakeTooSmall,
    #[msg("floor above the offered odds, or zero")]
    FloorAboveOffered,
    #[msg("quoted payout above Config.max_payout")]
    PayoutTooLarge,
    #[msg("expires_at must be within [now + 30 s, now + 60 s]")]
    BadExpiry,
    // Since D-660 only a tip that copies; the 1.C message is frozen by the IDL check.
    #[msg("a tip needs every leg not yet started and no copied_from")]
    TipNotAllowed,
    // Unused since 2.F (copies are accepted); kept so later codes keep their numbers.
    #[msg("copies are not enabled yet (step 2.F)")]
    CopiesNotEnabled,

    // --- finalize / refund ---
    #[msg("the pending has expired")]
    PendingExpired,
    #[msg("refund opens 15 s after expires_at")]
    RefundTooEarly,
    #[msg("one fill per leg expected")]
    FillCount,
    #[msg("fill ratio r must be in (0, 1e6]")]
    BadFillRatio,
    #[msg("fill price must be in (0, 1e6]")]
    BadFillPrice,
    #[msg("payout per unit below the signed floor")]
    BelowFloor,
    #[msg("vault free balance cannot cover the payout")]
    VaultShort,

    // --- tickets ---
    #[msg("ticket is not open")]
    TicketNotOpen,
    #[msg("leg index out of range")]
    BadLegIndex,
    #[msg("leg already resolved")]
    LegAlreadyResolved,
    #[msg("resolution must be in [0, 1e6]")]
    BadResolution,
    #[msg("not every leg is resolved")]
    LegsUnresolved,
    #[msg("cash-out quote is for another ticket")]
    WrongTicket,
    #[msg("cash-out quote is stale")]
    CashOutStale,
    #[msg("cash-out above the ticket payout")]
    CashOutAbovePayout,
    #[msg("a leg is lost: no cash-out")]
    LegLost,

    // --- house ---
    #[msg("withdrawal would take vault.free below Config.reserve")]
    BelowReserve,

    // --- faucet (devnet) ---
    #[msg("faucet amount must be in (0, FAUCET_MAX]")]
    FaucetAmount,
    // Unused since D-661 (no faucet cooldown); kept so later codes keep their numbers.
    #[msg("faucet cooldown: one claim per hour")]
    FaucetCooldown,

    // --- tipsters and copies (2.F; appended so earlier codes keep their numbers) ---
    #[msg("own tip: remaining account 0 must be the user's TipsterProfile PDA, writable")]
    BadTipsterProfile,
    #[msg("copy: remaining accounts must be the original ticket and the tipster's USDC account")]
    BadCopyAccounts,
    #[msg("copy: legs (symbol, game id, side) differ from the original ticket")]
    CopyMismatch,
    #[msg("copy: the original is not an open own tip")]
    OriginalNotCopyable,
    #[msg("copy: a leg of the original has resolved")]
    CopyTooLate,
}
