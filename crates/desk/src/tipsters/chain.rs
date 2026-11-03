//! The 2.F program pieces the desk's 2.C decoders do not cover (`program/` predates them):
//! the `TipsterPaid` and `ConfigUpdated` event discriminators, `TipsterPaid`'s body, and the
//! `TipsterProfile` account (`programs/schedina/src/state.rs`). Checked against the committed
//! IDL by the tests below.

use solana_pubkey::Pubkey;

use crate::program::DecodeError;
use crate::program::accounts::Reader;

/// Event discriminators (IDL `events[].discriminator`).
pub mod disc {
    /// `TipsterPaid` (finalize of a copy).
    pub const TIPSTER_PAID: [u8; 8] = [243, 204, 40, 94, 239, 21, 115, 112];
    /// `ConfigUpdated` (`set_params`, `pause`).
    pub const CONFIG_UPDATED: [u8; 8] = [40, 241, 230, 122, 11, 19, 198, 194];
    /// `TipsterProfile` account.
    pub const TIPSTER_PROFILE: [u8; 8] = [99, 197, 89, 94, 194, 135, 204, 246];
}

/// `TipsterPaid`: the tipster's share of a copy fee, paid at `finalize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TipsterPaid {
    /// Paid wallet (the original ticket's user).
    pub tipster: Pubkey,
    /// The copy's Ticket PDA.
    pub ticket: Pubkey,
    /// Amount, micro-USDC.
    pub amount_u: u64,
}

impl TipsterPaid {
    /// Decodes one `cpi_events` item (discriminator ‖ borsh).
    pub fn decode(data: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::with_disc(data, &disc::TIPSTER_PAID)?;
        Ok(TipsterPaid {
            tipster: r.pubkey()?,
            ticket: r.pubkey()?,
            amount_u: r.u64()?,
        })
    }
}

/// The program's own tipster record, PDA `["tipster", wallet]` (2.F): own tips only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TipsterProfileAccount {
    /// Wallet.
    pub wallet: Pubkey,
    /// Own tips open.
    pub open_count: u32,
    /// Their Σ rS.
    pub open_staked: u64,
    /// Own tips settled or cashed out.
    pub settled_count: u32,
    /// Of those, returned more than staked.
    pub won: u32,
    /// Their Σ rS.
    pub settled_staked: u64,
    /// Σ paid or C.
    pub returned: u64,
    /// Always 0 (D-113).
    pub fees_earned: u64,
}

impl TipsterProfileAccount {
    /// Decodes a `TipsterProfile` owned by `program`.
    pub fn decode(data: &[u8], owner: &Pubkey, program: &Pubkey) -> Result<Self, DecodeError> {
        if owner != program {
            return Err(DecodeError::WrongOwner);
        }
        let mut r = Reader::with_disc(data, &disc::TIPSTER_PROFILE)?;
        Ok(TipsterProfileAccount {
            wallet: r.pubkey()?,
            open_count: r.u32()?,
            open_staked: r.u64()?,
            settled_count: r.u32()?,
            won: r.u32()?,
            settled_staked: r.u64()?,
            returned: r.u64()?,
            fees_earned: r.u64()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    fn idl() -> Value {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../programs/idl/schedina.json"
        );
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn disc_of(list: &Value, name: &str) -> [u8; 8] {
        let item = list
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == name)
            .unwrap_or_else(|| panic!("{name} not in the IDL"));
        let v: Vec<u8> = serde_json::from_value(item["discriminator"].clone()).unwrap();
        v.try_into().unwrap()
    }

    #[test]
    fn discriminators_match_the_idl() {
        let idl = idl();
        assert_eq!(disc_of(&idl["events"], "TipsterPaid"), disc::TIPSTER_PAID);
        assert_eq!(
            disc_of(&idl["events"], "ConfigUpdated"),
            disc::CONFIG_UPDATED
        );
        assert_eq!(
            disc_of(&idl["accounts"], "TipsterProfile"),
            disc::TIPSTER_PROFILE
        );
        // Field order as in the IDL types.
        let types = idl["types"].as_array().unwrap();
        let fields = |name: &str| -> Vec<String> {
            types.iter().find(|t| t["name"] == name).unwrap()["type"]["fields"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| f["name"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(fields("TipsterPaid"), ["tipster", "ticket", "amount_u"]);
        assert_eq!(
            fields("TipsterProfile")[..8],
            [
                "wallet",
                "open_count",
                "open_staked",
                "settled_count",
                "won",
                "settled_staked",
                "returned",
                "fees_earned"
            ]
        );
    }

    #[test]
    fn decodes_tipster_paid_and_profiles() {
        let (a, b) = (Pubkey::new_unique(), Pubkey::new_unique());
        let mut raw = disc::TIPSTER_PAID.to_vec();
        raw.extend_from_slice(a.as_ref());
        raw.extend_from_slice(b.as_ref());
        raw.extend_from_slice(&160_000u64.to_le_bytes());
        assert_eq!(
            TipsterPaid::decode(&raw).unwrap(),
            TipsterPaid {
                tipster: a,
                ticket: b,
                amount_u: 160_000
            }
        );
        assert!(TipsterPaid::decode(&raw[..40]).is_err());
        let program = Pubkey::new_unique();
        let mut p = disc::TIPSTER_PROFILE.to_vec();
        p.extend_from_slice(a.as_ref());
        p.extend_from_slice(&1u32.to_le_bytes());
        p.extend_from_slice(&5_000_000u64.to_le_bytes());
        p.extend_from_slice(&2u32.to_le_bytes());
        p.extend_from_slice(&1u32.to_le_bytes());
        p.extend_from_slice(&10_000_000u64.to_le_bytes());
        p.extend_from_slice(&14_000_000u64.to_le_bytes());
        p.extend_from_slice(&0u64.to_le_bytes());
        p.push(255);
        p.extend_from_slice(&[0; 32]);
        let d = TipsterProfileAccount::decode(&p, &program, &program).unwrap();
        assert_eq!(
            (d.wallet, d.open_count, d.settled_count, d.won, d.returned),
            (a, 1, 2, 1, 14_000_000)
        );
        assert_eq!(
            TipsterProfileAccount::decode(&p, &a, &program),
            Err(DecodeError::WrongOwner)
        );
    }
}
