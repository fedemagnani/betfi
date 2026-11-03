//! Permit verification with `pascal-rs`'s signing code (step 2.D, contract L2-5).
//!
//! [`SignatureVerifier`] rebuilds the exact permit bytes of every place-order, cancel and
//! mass-cancel request with `pascal_rs::signing::verify_*` (the Rust port of Pascal's signer,
//! so the port is exercised on every mock write), checks the Ed25519 signature against
//! `auth.signer`, then checks that the signer is a trading key of `auth.owner`:
//!
//! - with an empty [`TradingKeys`] registry, any key is accepted as long as the signature is
//!   valid for the key the request names (dev);
//! - with a registry, the signer must be registered for that owner (an owner with no entry
//!   has no valid key).
//!
//! Any failure becomes Pascal's `UNAUTHORIZED` (HTTP 401) for that batch item. Freshness
//! (`client_ts_ms` + `recv_window_ms`) is not checked: mock time is tape time (D-033).

use std::collections::{BTreeMap, BTreeSet};

use pascal_rs::signing::{
    self, CancelOrderRequest, DEPLOYMENT_PROD, MassCancelRequest, PermitHeader, PlaceOrderRequest,
    Pubkey,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::requests::{CancelOrder, MassCancel, PermitVerifier, PlaceOrder};

/// Trading keys per owner (`--trading-key <owner>=<pubkey>`, repeatable).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TradingKeys {
    keys: BTreeMap<Pubkey, BTreeSet<Pubkey>>,
}

impl TradingKeys {
    /// An empty registry: every validly signed permit is accepted.
    pub fn new() -> Self {
        TradingKeys::default()
    }

    /// Registers `key` as a trading key of `owner` (both base58).
    pub fn add(&mut self, owner: &str, key: &str) -> Result<(), String> {
        let o = signing::decode_pubkey(owner.trim(), "owner").map_err(|e| e.to_string())?;
        let k = signing::decode_pubkey(key.trim(), "trading key").map_err(|e| e.to_string())?;
        self.keys.entry(o).or_default().insert(k);
        Ok(())
    }

    /// Parses `<owner>=<pubkey>` and registers it.
    pub fn add_spec(&mut self, spec: &str) -> Result<(), String> {
        let (owner, key) = spec
            .split_once('=')
            .ok_or_else(|| format!("{spec:?}: expected <owner>=<trading key>"))?;
        self.add(owner, key)
    }

    /// Whether no key is registered (dev mode: any valid signature passes).
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Number of (owner, key) pairs.
    pub fn len(&self) -> usize {
        self.keys.values().map(BTreeSet::len).sum()
    }

    /// Whether `signer` may sign for `owner` under this registry.
    pub fn allows(&self, owner: &Pubkey, signer: &Pubkey) -> bool {
        self.is_empty() || self.keys.get(owner).is_some_and(|ks| ks.contains(signer))
    }
}

/// Verifies permits with `pascal-rs` (see the module docs).
#[derive(Debug, Clone)]
pub struct SignatureVerifier {
    keys: TradingKeys,
    deployment_id: u8,
}

impl SignatureVerifier {
    /// A verifier for Pascal's production deployment id ([`DEPLOYMENT_PROD`], what the desk
    /// and `pascal-cli` sign).
    pub fn new(keys: TradingKeys) -> Self {
        SignatureVerifier {
            keys,
            deployment_id: DEPLOYMENT_PROD,
        }
    }

    /// Uses another `deployment_id` in the rebuilt permit header.
    pub fn with_deployment_id(mut self, id: u8) -> Self {
        self.deployment_id = id;
        self
    }

    /// The registry.
    pub fn keys(&self) -> &TradingKeys {
        &self.keys
    }

    fn authorize(&self, header: &PermitHeader) -> Result<(), String> {
        if self.keys.allows(&header.owner, &header.signer) {
            Ok(())
        } else {
            Err(format!(
                "trading key {} is not registered for owner {}",
                signing::base58(&header.signer),
                signing::base58(&header.owner)
            ))
        }
    }
}

/// Deserialises a request element as `pascal-rs` models it. The mock's parser (D-037) also
/// takes `metadata` as a JSON number; `pascal-rs` wants the documented string, so a number is
/// turned into its string first (same byte, same permit).
fn as_request<T: DeserializeOwned>(raw: &Value) -> Result<T, String> {
    let mut v = raw.clone();
    if let Some(m) = v.get_mut("metadata")
        && m.is_number()
    {
        *m = Value::String(m.to_string());
    }
    serde_json::from_value(v).map_err(|e| format!("cannot rebuild the permit: {e}"))
}

impl PermitVerifier for SignatureVerifier {
    fn verify_place(&self, _: &PlaceOrder, raw: &Value) -> Result<(), String> {
        let req: PlaceOrderRequest = as_request(raw)?;
        let v = signing::verify_place_order(&req, self.deployment_id).map_err(|e| e.to_string())?;
        self.authorize(&v.header)
    }

    fn verify_cancel(&self, _: &CancelOrder, raw: &Value) -> Result<(), String> {
        let req: CancelOrderRequest = as_request(raw)?;
        let v = signing::verify_cancel(&req, self.deployment_id).map_err(|e| e.to_string())?;
        self.authorize(&v.header)
    }

    fn verify_mass_cancel(&self, _: &MassCancel, raw: &Value) -> Result<(), String> {
        let req: MassCancelRequest = as_request(raw)?;
        let v = signing::verify_mass_cancel(&req, self.deployment_id).map_err(|e| e.to_string())?;
        self.authorize(&v.header)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pascal_rs::Side;
    use pascal_rs::signing::{AuthParams, TradingSigner};
    use serde_json::json;

    fn signed(signer: &TradingSigner, owner: Pubkey) -> (PlaceOrder, Value) {
        let order = signing::PlaceOrder::ioc("A.B", Side::Bid, 500_000, 3, 7);
        let s = signer
            .sign_place_order(&AuthParams::new(owner, 1_793_213_000_000), &order)
            .unwrap();
        let raw = serde_json::to_value(&s.request).unwrap();
        (PlaceOrder::parse(&raw).unwrap(), raw)
    }

    #[test]
    fn checks_signature_and_registry() {
        let owner = [5u8; 32];
        let k = TradingSigner::from_seed(&[8; 32]);
        let other = TradingSigner::from_seed(&[9; 32]);
        let (o, raw) = signed(&k, owner);
        let open = SignatureVerifier::new(TradingKeys::new());
        open.verify_place(&o, &raw).unwrap();

        // One flipped signature byte.
        let mut bad = raw.clone();
        let mut sig = bs58::decode(bad["auth"]["signature"].as_str().unwrap())
            .into_vec()
            .unwrap();
        sig[0] ^= 1;
        bad["auth"]["signature"] = json!(bs58::encode(sig).into_string());
        assert!(open.verify_place(&o, &bad).is_err());

        // A changed field breaks the signature too.
        let mut moved = raw.clone();
        moved["price"] = json!("0.510000");
        assert!(open.verify_place(&o, &moved).is_err());

        // Registry: only the registered key passes.
        let mut keys = TradingKeys::new();
        keys.add(&signing::base58(&owner), &other.pubkey_base58())
            .unwrap();
        let strict = SignatureVerifier::new(keys.clone());
        let err = strict.verify_place(&o, &raw).unwrap_err();
        assert!(err.contains("not registered"), "{err}");
        keys.add_spec(&format!(
            "{}={}",
            signing::base58(&owner),
            k.pubkey_base58()
        ))
        .unwrap();
        SignatureVerifier::new(keys.clone())
            .verify_place(&o, &raw)
            .unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys.add_spec("nonsense").is_err());

        // Metadata as a JSON number (D-037) verifies like the string.
        let mut order = signing::PlaceOrder::limit("A.B", Side::Bid, 500_000, 3, 8);
        order.metadata = 2;
        let s = k
            .sign_place_order(&AuthParams::new(owner, 1), &order)
            .unwrap();
        let mut raw = serde_json::to_value(&s.request).unwrap();
        raw["metadata"] = json!(2);
        let o = PlaceOrder::parse(&raw).unwrap();
        open.verify_place(&o, &raw).unwrap();
    }

    #[test]
    fn cancels_are_verified() {
        let owner = [5u8; 32];
        let k = TradingSigner::from_seed(&[8; 32]);
        let open = SignatureVerifier::new(TradingKeys::new());
        let c = k
            .sign_cancel(
                &AuthParams::new(owner, 1),
                &signing::CancelOrder::by_client_order_id(7),
            )
            .unwrap();
        let raw = serde_json::to_value(&c.request).unwrap();
        let parsed = CancelOrder::parse(&raw).unwrap();
        open.verify_cancel(&parsed, &raw).unwrap();
        let mut bad = raw.clone();
        bad["client_order_id"] = json!("8");
        assert!(open.verify_cancel(&parsed, &bad).is_err());

        let m = k
            .sign_mass_cancel(&AuthParams::new(owner, 1), &signing::MassCancel::new("A.B"))
            .unwrap();
        let raw = serde_json::to_value(&m.request).unwrap();
        let parsed = MassCancel::parse(&raw).unwrap();
        open.verify_mass_cancel(&parsed, &raw).unwrap();
        let mut bad = raw.clone();
        bad["symbol"] = json!("A.C");
        assert!(open.verify_mass_cancel(&parsed, &bad).is_err());
    }
}
