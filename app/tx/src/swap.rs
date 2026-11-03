//! Pay in any token (spec section 8, plan 4.D, contract L4-3): the mock AMM's instructions and
//! the one transaction `[swap, Ed25519 verify, open_pending]`, built from
//! `programs/idl/mock_amm.json` and host-tested against it and `fixtures/swap/swap_ix.json`.
//!
//! Flow: the app asks the desk `POST /swap-quote` for `amount_in` of a mock mint, then
//! `POST /quote` with `stake_u = min_out_u` (USDC, `token_in: null`), and signs a transaction in
//! which the swap pays at least `min_out_u` into the user's mock-USDC account and `open_pending`
//! escrows exactly `stake_u` from it. A swap paying less fails the whole transaction, so nothing
//! is pending. The verify instruction stays immediately before `open_pending` (D-049).
//!
//! With 4–5 legs the transaction no longer fits a legacy message (1,365 bytes for 5 legs), so
//! it is a v0 message with the address lookup table named in `AmmConfig.lookup_table`
//! ([`compile_v0`]); 5 legs plus two wallet compute-budget instructions take 1,122 bytes.

use super::{
    AccountMeta, Instruction, OpenOptions, Pubkey, SYSTEM_PROGRAM_ID, ShownQuote, SignedMsg,
    TOKEN_PROGRAM_ID, TxError, create_ata_idempotent_ix, open_pending_ixs, pda,
};

/// The mock AMM (`declare_id!` of `programs/mock-amm`, `programs/idl/mock_amm.json` `address`).
pub const AMM_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("5WhsDY2XjzJoWDbe1AgJnnNgJccQwte9TgsvG4yRtHbf");
/// The address lookup table program.
pub const ALT_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("AddressLookupTab1e1111111111111111111111111");

/// Anchor discriminators, from the IDL.
pub const SWAP_DISC: [u8; 8] = [248, 198, 158, 145, 225, 117, 135, 200];
pub const FAUCET_DISC: [u8; 8] = [0, 98, 59, 30, 144, 142, 113, 12];
pub const AMM_CONFIG_DISC: [u8; 8] = [218, 244, 33, 104, 203, 203, 43, 111];

/// Largest serialized transaction a validator accepts.
pub const PACKET_DATA_SIZE: usize = 1_232;

/// Program-derived addresses of the mock AMM (`programs/mock-amm/src/constants.rs`).
pub mod amm_pda {
    use super::{AMM_PROGRAM_ID, Pubkey};

    fn find(seeds: &[&[u8]]) -> Pubkey {
        Pubkey::find_program_address(seeds, &AMM_PROGRAM_ID).0
    }

    /// `["amm_config"]`: admin, USDC mint, lookup table.
    pub fn amm_config() -> Pubkey {
        find(&[b"amm_config"])
    }
    /// `["mock", symbol]`: a mock mint (`"BONK"`, `"SOL"`).
    pub fn mock_mint(symbol: &str) -> Pubkey {
        find(&[b"mock", symbol.as_bytes()])
    }
    /// `["token", mint]`: the mint's `MockToken`.
    pub fn mock_token(mint: &Pubkey) -> Pubkey {
        find(&[b"token", mint.as_ref()])
    }
    /// `["pool", mint]`.
    pub fn pool(mint: &Pubkey) -> Pubkey {
        find(&[b"pool", mint.as_ref()])
    }
    /// `["vault", pool, mint]`: the pool's token account of `mint` (mock mint or USDC).
    pub fn vault(pool: &Pubkey, mint: &Pubkey) -> Pubkey {
        find(&[b"vault", pool.as_ref(), mint.as_ref()])
    }
    /// `["faucet", mint, caller]`: the signer's hourly claim.
    pub fn faucet_claim(mint: &Pubkey, caller: &Pubkey) -> Pubkey {
        find(&[b"faucet", mint.as_ref(), caller.as_ref()])
    }
}

/// `swap(amount_in, min_out)` selling `amount_in` of `mint` from the user's ATA of it into the
/// user's mock-USDC ATA, with the IDL's account list in order.
pub fn swap_ix(user: &Pubkey, mint: &Pubkey, amount_in: u64, min_out: u64) -> Instruction {
    let usdc = pda::usdc_mint();
    let pool = amm_pda::pool(mint);
    let mut data = SWAP_DISC.to_vec();
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&min_out.to_le_bytes());
    Instruction {
        program_id: AMM_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(*user, true),
            AccountMeta::new(pool, false),
            AccountMeta::new(pda::ata(user, mint), false),
            AccountMeta::new(pda::ata(user, &usdc), false),
            AccountMeta::new(amm_pda::vault(&pool, mint), false),
            AccountMeta::new(amm_pda::vault(&pool, &usdc), false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
        ],
        data,
    }
}

/// `faucet(amount)` of `mint` into the user's ATA of it (the program caps it per mint and lets
/// each signer claim once an hour). `payer` pays the claim's rent (L8: the user, or the sponsor).
pub fn mock_faucet_ix(user: &Pubkey, payer: &Pubkey, mint: &Pubkey, amount: u64) -> Instruction {
    let mut data = FAUCET_DISC.to_vec();
    data.extend_from_slice(&amount.to_le_bytes());
    Instruction {
        program_id: AMM_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(*user, true),
            AccountMeta::new_readonly(amm_pda::amm_config(), false),
            AccountMeta::new_readonly(amm_pda::mock_token(mint), false),
            AccountMeta::new(*mint, false),
            AccountMeta::new(pda::ata(user, mint), false),
            AccountMeta::new(amm_pda::faucet_claim(mint, user), false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new(*payer, true),
        ],
        data,
    }
}

/// `[create the user's ATA of mint if absent, faucet(amount)]`, rent paid by `payer`.
pub fn mock_faucet_ixs(
    user: &Pubkey,
    payer: &Pubkey,
    mint: &Pubkey,
    amount: u64,
) -> [Instruction; 2] {
    [
        create_ata_idempotent_ix(payer, user, mint),
        mock_faucet_ix(user, payer, mint, amount),
    ]
}

/// What the desk's `POST /swap-quote` answered, as the app holds it (L4-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShownSwap {
    pub mint: Pubkey,
    pub amount_in: u64,
    /// The swap's `min_out`; also the stake the desk quoted (`stake_u = min_out_u`).
    pub min_out_u: u64,
    /// The `pool` field of the quote: must be `["pool", mint]`.
    pub pool: Pubkey,
}

/// The program's copy fee (`Config.copy_fee_bps`, 2% on devnet, D-064).
pub const COPY_FEE_BPS: u64 = 200;

/// The copy fee on `stake_u`, as the program charges it: `ceil(stake_u × 2%)` (`math::copy_fee`).
pub fn copy_fee_u(stake_u: u64) -> u64 {
    let fee = (u128::from(stake_u) * u128::from(COPY_FEE_BPS)).div_ceil(10_000);
    u64::try_from(fee).unwrap_or(u64::MAX)
}

/// The largest copy stake whose stake and copy fee together fit in `min_out_u`: `open_pending`
/// escrows both from the USDC the swap pays (a copy paid in another token, L9).
pub fn copy_stake_for(min_out_u: u64) -> u64 {
    let mut stake =
        u64::try_from(u128::from(min_out_u) * 10_000 / (10_000 + u128::from(COPY_FEE_BPS)))
            .unwrap_or(0);
    while stake > 0 && stake + copy_fee_u(stake) > min_out_u {
        stake -= 1;
    }
    stake
}

/// `[swap, Ed25519 verify, open_pending]` (L4-3), a plain schedina: [`swap_open_pending_ixs_with`]
/// with no tip and no copy.
pub fn swap_open_pending_ixs(
    user: &Pubkey,
    payer: &Pubkey,
    swap: &ShownSwap,
    signed: &SignedMsg,
    shown: &ShownQuote,
    nonce: u64,
    now_s: i64,
) -> Result<[Instruction; 3], TxError> {
    swap_open_pending_ixs_with(
        user,
        payer,
        swap,
        signed,
        shown,
        nonce,
        now_s,
        OpenOptions::default(),
    )
}

/// `[swap, Ed25519 verify, open_pending]` (L4-3). On top of [`open_pending_ixs`]'s checks of the
/// signed quote against `shown`, refuses: mock USDC itself as the input, a zero amount or floor,
/// a pool that is not the mint's PDA, a tip (a schedina paid in another token is never one), and
/// a stake the swap does not pay for: a plain schedina's stake is exactly `min_out_u` (the escrow
/// takes exactly the stake); a copy's is [`copy_stake_for`]`(min_out_u)`, so the stake and the
/// copy fee both come out of what the swap pays (L9).
#[allow(clippy::too_many_arguments)]
pub fn swap_open_pending_ixs_with(
    user: &Pubkey,
    payer: &Pubkey,
    swap: &ShownSwap,
    signed: &SignedMsg,
    shown: &ShownQuote,
    nonce: u64,
    now_s: i64,
    opts: OpenOptions,
) -> Result<[Instruction; 3], TxError> {
    let bad = |m: &str| Err(TxError::Message(m.to_string()));
    if swap.mint == pda::usdc_mint() {
        return bad("mock USDC needs no swap");
    }
    if swap.amount_in == 0 || swap.min_out_u == 0 {
        return bad("swap amount or minimum is zero");
    }
    if swap.pool != amm_pda::pool(&swap.mint) {
        return bad("the swap quote names another pool");
    }
    if opts.is_tip {
        return bad("a schedina paid in another token cannot be shared as a tip");
    }
    if opts.copied_from.is_some() {
        if shown.stake_u != copy_stake_for(swap.min_out_u) {
            return bad("the copy's stake and fee are not what the swap pays");
        }
    } else if shown.stake_u != swap.min_out_u {
        return bad("the stake is not the swap's minimum USDC out");
    }
    let [verify, open] = open_pending_ixs(user, payer, signed, shown, nonce, now_s, opts)?;
    Ok([
        swap_ix(user, &swap.mint, swap.amount_in, swap.min_out_u),
        verify,
        open,
    ])
}

/// `AmmConfig.lookup_table` from the account data (`discriminator ‖ admin ‖ usdc_mint ‖
/// lookup_table ‖ …`). `None` while unset (all zeros).
pub fn amm_config_lookup_table(data: &[u8]) -> Result<Option<Pubkey>, TxError> {
    if data.len() < 8 + 96 || data[..8] != AMM_CONFIG_DISC {
        return Err(TxError::Message("not the mock AMM's config".into()));
    }
    let key = Pubkey::new_from_array(data[72..104].try_into().expect("32 bytes"));
    Ok((key != Pubkey::default()).then_some(key))
}

/// The addresses of an address lookup table account (after its 56-byte `LookupTableMeta`).
pub fn lookup_table_addresses(data: &[u8]) -> Result<Vec<Pubkey>, TxError> {
    let bad = || TxError::Message("not an address lookup table".into());
    if data.len() < 56 || data[..4] != 1u32.to_le_bytes() {
        return Err(bad());
    }
    let (chunks, rest) = data[56..].as_chunks::<32>();
    if !rest.is_empty() {
        return Err(bad());
    }
    Ok(chunks.iter().map(|c| Pubkey::new_from_array(*c)).collect())
}

/// The unsigned v0 transaction of `ixs` paid by `payer` on `blockhash`, with lookup table
/// `table` (its key and addresses as read from chain), in the wire format the wallet signs:
/// `compact(n) ‖ n zeroed signatures ‖ 0x80 ‖ v0 message`. Only addresses the instructions use
/// are taken from the table; signers and invoked programs stay static. Refuses a transaction
/// over [`PACKET_DATA_SIZE`].
pub fn compile_v0(
    payer: &Pubkey,
    ixs: &[Instruction],
    table: Option<(Pubkey, Vec<Pubkey>)>,
    blockhash: [u8; 32],
) -> Result<Vec<u8>, TxError> {
    super::sponsor::compile(payer, ixs, &super::sponsor::Format::V0(table), blockhash)
}

/// Account data from `getAccountInfo`'s base64 `data[0]`.
pub fn account_data_b64(b64: &str) -> Result<Vec<u8>, TxError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|_| TxError::Base64("account data"))
}

/// `amount` base units with `decimals`, as text without trailing zeros (`12.5`, `50000`).
pub fn format_units(amount: u64, decimals: u8) -> String {
    let d = u32::from(decimals.min(19));
    let unit = 10u64.pow(d);
    let (whole, frac) = (amount / unit, amount % unit);
    if frac == 0 {
        return whole.to_string();
    }
    let f = format!("{frac:0width$}", width = d as usize);
    format!("{whole}.{}", f.trim_end_matches('0'))
}

/// Parses a token amount typed by the user (`50000`, `1,000.5`, `0.25`) into base units with
/// `decimals`; `None` for anything else, more fraction digits than `decimals`, zero or overflow.
pub fn parse_units(text: &str, decimals: u8) -> Option<u64> {
    let t: String = text
        .trim()
        .chars()
        .filter(|c| *c != ',' && *c != '_')
        .collect();
    let (whole, frac) = t.split_once('.').unwrap_or((&t, ""));
    if whole.is_empty() && frac.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
        || frac.len() > usize::from(decimals)
    {
        return None;
    }
    let unit = 10u64.checked_pow(u32::from(decimals))?;
    let w: u64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let f: u64 = if frac.is_empty() {
        0
    } else {
        frac.parse::<u64>().ok()? * 10u64.pow((usize::from(decimals) - frac.len()) as u32)
    };
    let v = w.checked_mul(unit)?.checked_add(f)?;
    (v > 0).then_some(v)
}

/// The mock AMM's error for an Anchor error code, in plain words (`None` if not the user's).
pub fn swap_error_words(code: u32) -> Option<&'static str> {
    Some(match code {
        6005 => "the faucet gives at most its cap per claim",
        6006 => "the faucet gives test tokens once an hour",
        6009 => "this amount is too small to swap",
        6010 => "the swap price moved: ask for a new quote",
        _ => return None,
    })
}
