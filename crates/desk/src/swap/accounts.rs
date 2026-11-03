//! Account layouts of the mock AMM (`programs/mock-amm/src/state.rs`, checked against
//! `programs/idl/mock_amm.json` by this module's tests) and the two SPL Token fields read.

use solana_pubkey::Pubkey;

/// Anchor account discriminators (IDL `accounts[].discriminator`).
pub mod disc {
    /// `AmmConfig`.
    pub const AMM_CONFIG: [u8; 8] = [218, 244, 33, 104, 203, 203, 43, 111];
    /// `MockToken`.
    pub const MOCK_TOKEN: [u8; 8] = [88, 107, 212, 6, 227, 196, 28, 235];
    /// `Pool`.
    pub const POOL: [u8; 8] = [241, 154, 109, 4, 17, 177, 109, 188];
    /// `FaucetClaim`.
    pub const FAUCET_CLAIM: [u8; 8] = [88, 39, 189, 221, 15, 215, 24, 248];
}

/// Why an account did not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError(pub &'static str);

struct Reader<'a> {
    d: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(d: &'a [u8], disc: [u8; 8], what: &'static str) -> Result<Self, DecodeError> {
        if d.len() < 8 || d[..8] != disc {
            return Err(DecodeError(what));
        }
        Ok(Reader { d, at: 8 })
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let s = self
            .d
            .get(self.at..self.at + n)
            .ok_or(DecodeError("short account"))?;
        self.at += n;
        Ok(s)
    }
    fn key(&mut self) -> Result<Pubkey, DecodeError> {
        Ok(Pubkey::new_from_array(
            self.take(32)?.try_into().expect("32"),
        ))
    }
    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().expect("2")))
    }
    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4")))
    }
    fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("8")))
    }
    fn string(&mut self) -> Result<String, DecodeError> {
        let n = self.u32()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| DecodeError("utf-8"))
    }
}

/// `AmmConfig` (PDA `["amm_config"]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmmConfigAccount {
    /// Admin (the program's upgrade authority at `init_amm`).
    pub admin: Pubkey,
    /// Quote mint of every pool (schedina's mock USDC).
    pub usdc_mint: Pubkey,
    /// Address lookup table of the swap transaction (`Pubkey::default()` until set).
    pub lookup_table: Pubkey,
}

impl AmmConfigAccount {
    /// Decodes the account data.
    pub fn decode(d: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(d, disc::AMM_CONFIG, "not an AmmConfig")?;
        Ok(AmmConfigAccount {
            admin: r.key()?,
            usdc_mint: r.key()?,
            lookup_table: r.key()?,
        })
    }
}

/// `MockToken` (PDA `["token", mint]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockTokenAccount {
    /// The SPL mint.
    pub mint: Pubkey,
    /// Seed symbol (`"BONK"`); shown as `mock` + symbol.
    pub symbol: String,
    /// Mint decimals.
    pub decimals: u8,
    /// Largest faucet claim.
    pub faucet_max: u64,
}

impl MockTokenAccount {
    /// Decodes the account data.
    pub fn decode(d: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(d, disc::MOCK_TOKEN, "not a MockToken")?;
        Ok(MockTokenAccount {
            mint: r.key()?,
            symbol: r.string()?,
            decimals: r.u8()?,
            faucet_max: r.u64()?,
        })
    }
}

/// `Pool` (PDA `["pool", mint]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolAccount {
    /// Mock mint.
    pub mint: Pubkey,
    /// USDC mint.
    pub usdc_mint: Pubkey,
    /// Token account of `mint`.
    pub vault: Pubkey,
    /// Token account of `usdc_mint`.
    pub vault_usdc: Pubkey,
    /// Fee, bps of the amount in.
    pub fee_bps: u16,
    /// Fees kept in `mint`.
    pub fees_mint: u64,
    /// Fees kept in USDC.
    pub fees_usdc: u64,
    /// Swaps so far.
    pub swaps: u64,
}

impl PoolAccount {
    /// Decodes the account data.
    pub fn decode(d: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(d, disc::POOL, "not a Pool")?;
        Ok(PoolAccount {
            mint: r.key()?,
            usdc_mint: r.key()?,
            vault: r.key()?,
            vault_usdc: r.key()?,
            fee_bps: r.u16()?,
            fees_mint: r.u64()?,
            fees_usdc: r.u64()?,
            swaps: r.u64()?,
        })
    }
}

/// SPL token account: `(mint, amount)` (mint 0..32, amount 64..72 of 165 bytes).
pub fn token_account(d: &[u8]) -> Result<(Pubkey, u64), DecodeError> {
    if d.len() != 165 {
        return Err(DecodeError("not a token account"));
    }
    Ok((
        Pubkey::new_from_array(d[0..32].try_into().expect("32")),
        u64::from_le_bytes(d[64..72].try_into().expect("8")),
    ))
}

/// SPL mint decimals (byte 44 of 82).
pub fn mint_decimals(d: &[u8]) -> Result<u8, DecodeError> {
    if d.len() != 82 {
        return Err(DecodeError("not a mint"));
    }
    Ok(d[44])
}

/// Address lookup table account: its addresses (after the 56-byte `LookupTableMeta`).
pub fn lookup_table_addresses(d: &[u8]) -> Result<Vec<Pubkey>, DecodeError> {
    if d.len() < 56 || !(d.len() - 56).is_multiple_of(32) || d[..4] != 1u32.to_le_bytes() {
        return Err(DecodeError("not a lookup table"));
    }
    Ok(d[56..]
        .as_chunks::<32>()
        .0
        .iter()
        .map(|c| Pubkey::new_from_array(*c))
        .collect())
}
