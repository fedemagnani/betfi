//! Dev-only burner wallet (plan 3.C): a fresh ed25519 keypair generated in the page, so a
//! headless browser (or a tester without Phantom) can drive every signed flow on devnet.
//! Reached only from `/dev/wallet`, which exists in debug builds or with `SCHEDINA_DEV_TOOLS`.
//!
//! The secret is 32 bytes from `crypto.getRandomValues`, kept in wasm memory only: never
//! stored, logged, exported or shown, and gone when the tab closes (fund such a wallet with
//! a few cents of devnet SOL, nothing more). The bundle carries no key material.

use std::rc::Rc;

use dioxus_solana::{Pubkey, Signature, VersionedTransaction, WalletError, WalletSigner};
use ed25519_dalek::{Signer, SigningKey};

/// The wallet name shown in the header while the burner is connected.
pub const BURNER_NAME: &str = "Dev burner (in-page)";

pub struct Burner {
    key: SigningKey,
}

impl Burner {
    /// A new random keypair. Fails only if the browser has no `crypto.getRandomValues`.
    pub fn generate() -> Result<Rc<Self>, String> {
        let crypto = web_sys::window()
            .ok_or("no window")?
            .crypto()
            .map_err(|_| "this browser has no crypto.getRandomValues")?;
        let mut seed = [0u8; 32];
        crypto
            .get_random_values_with_u8_array(&mut seed)
            .map_err(|_| "crypto.getRandomValues failed")?;
        let key = SigningKey::from_bytes(&seed);
        seed.fill(0);
        Ok(Rc::new(Burner { key }))
    }

    pub fn address(&self) -> Pubkey {
        Pubkey::new_from_array(self.key.verifying_key().to_bytes())
    }
}

#[async_trait::async_trait(?Send)]
impl WalletSigner for Burner {
    fn pubkey(&self) -> Pubkey {
        self.address()
    }

    async fn sign_message(&self, msg: &[u8]) -> Result<Signature, WalletError> {
        Ok(Signature::from(self.key.sign(msg).to_bytes()))
    }

    /// Signs the message bytes as they are (never changes the message) into this key's
    /// signature slot; refuses a transaction this key is not a required signer of.
    async fn sign_transaction(
        &self,
        mut tx: VersionedTransaction,
    ) -> Result<VersionedTransaction, WalletError> {
        let me = self.address();
        let required = usize::from(tx.message.header().num_required_signatures);
        let slot = tx
            .message
            .static_account_keys()
            .iter()
            .take(required)
            .position(|k| *k == me)
            .ok_or_else(|| WalletError::Codec(format!("{me} is not a signer")))?;
        if tx.signatures.len() != required {
            return Err(WalletError::Codec(
                "signature slots do not match signers".into(),
            ));
        }
        let sig = self.key.sign(&tx.message.serialize());
        tx.signatures[slot] = Signature::from(sig.to_bytes());
        Ok(tx)
    }

    /// The app submits through its own RPC ([`crate::wallet::Wallet::sign_and_send`]).
    async fn sign_and_send_transaction(
        &self,
        _tx: VersionedTransaction,
    ) -> Result<Signature, WalletError> {
        Err(WalletError::Feature(
            "the burner only signs; the app sends".into(),
        ))
    }

    async fn disconnect(&self) -> Result<(), WalletError> {
        Ok(())
    }
}
