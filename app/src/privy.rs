//! "Continue with Google" through Privy (8.D, contract L8-3): Privy logs the user in with Google
//! and keeps an embedded Solana wallet for them; [`PrivySigner`] makes that wallet the app's
//! signer through [`Wallet::connect_signer`], so no wallet extension is needed.
//!
//! The Privy SDK lives in `assets/privy.js` (an ES module bundled from `app/privy/` by `just
//! privy-bundle`), which sets `window.SchedinaPrivy`. It is imported lazily: on the first Google
//! tap, or at boot when the URL carries Privy's OAuth return (`privy_oauth_code`,
//! `privy_oauth_state`) or a Privy session is stored in this browser ([`resume`]), so a visitor
//! who never taps the button never downloads it. Only the public app ID
//! ([`config::privy_app_id`]) is used; there is no client secret.
//!
//! Signing: the wallet signs the message bytes as they are; from what Privy returns only the
//! user's own signature slot is kept ([`wire::merge_signer_signature`]), checked against the
//! user's key, so a sponsor's slot (L8: the desk pays, slot 0) is never touched.

mod wire;

use std::cell::RefCell;
use std::rc::Rc;
use std::str::FromStr;

use dioxus::logger::tracing::warn;
use dioxus::prelude::*;
use dioxus_solana::{
    Pubkey, Signature, VersionedTransaction, WalletError, WalletSigner, deserialize_transaction,
    serialize_transaction,
};
use js_sys::wasm_bindgen::{JsCast, JsValue};
use js_sys::{Array, Function, Promise, Reflect, Uint8Array};

use crate::config;
use crate::wallet::Wallet;

/// The wallet name the app shows while the Privy wallet is connected.
pub const PRIVY_WALLET_NAME: &str = "Google (Privy)";

/// The bridge: an ES module, already minified by esbuild (dx's minify pass keeps module syntax).
const PRIVY_JS: Asset = asset!("/assets/privy.js", AssetOptions::js().with_module(true));

/// What the Google button shows while a login is under way, or why the last one failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivyStatus {
    Idle,
    Busy(&'static str),
    Failed(String),
}

/// Shared by every Google button and the boot hook.
pub static PRIVY_STATUS: GlobalSignal<PrivyStatus> = Signal::global(|| PrivyStatus::Idle);

fn window_bridge() -> Option<JsValue> {
    let window = web_sys::window()?;
    let bridge = Reflect::get(&window, &JsValue::from_str("SchedinaPrivy")).ok()?;
    (!bridge.is_undefined() && !bridge.is_null()).then_some(bridge)
}

/// `window.SchedinaPrivy`, importing `assets/privy.js` first if needed (the browser imports a
/// module once, so concurrent callers share it).
async fn bridge() -> Result<JsValue, String> {
    if let Some(b) = window_bridge() {
        return Ok(b);
    }
    let url = serde_json::to_string(&PRIVY_JS.to_string()).map_err(|e| e.to_string())?;
    document::eval(&format!("await import({url}); return true;"))
        .join::<bool>()
        .await
        .map_err(|e| format!("Google sign-in did not load ({e})"))?;
    window_bridge().ok_or_else(|| "Google sign-in did not load".to_string())
}

/// A JS rejection's message.
fn js_message(e: JsValue) -> String {
    Reflect::get(&e, &JsValue::from_str("message"))
        .ok()
        .and_then(|m| m.as_string())
        .or_else(|| e.as_string())
        .unwrap_or_else(|| "unknown error".to_string())
}

/// `SchedinaPrivy[method](...args)`, awaited.
async fn call_raw(method: &str, args: &[JsValue]) -> Result<JsValue, String> {
    let b = bridge().await?;
    let f: Function = Reflect::get(&b, &JsValue::from_str(method))
        .map_err(js_message)?
        .dyn_into()
        .map_err(|_| format!("SchedinaPrivy.{method} is missing"))?;
    let out = f
        .apply(&b, &args.iter().collect::<Array>())
        .map_err(js_message)?;
    Promise::resolve(&out).await.map_err(js_message)
}

/// The same after `init(appId)` (idempotent on the JS side: one Privy client per page).
async fn call(method: &str, args: &[JsValue]) -> Result<JsValue, String> {
    call_raw("init", &[JsValue::from_str(config::privy_app_id())]).await?;
    call_raw(method, args).await
}

fn bytes_arg(b: &[u8]) -> JsValue {
    Uint8Array::from(b).into()
}

fn bytes_of(v: &JsValue) -> Result<Vec<u8>, String> {
    v.dyn_ref::<Uint8Array>()
        .map(Uint8Array::to_vec)
        .ok_or_else(|| "Privy returned no bytes".to_string())
}

/// Where Google sends the user back: this page, without its query string.
fn redirect_uri() -> Result<String, String> {
    let loc = web_sys::window().ok_or("no window")?.location();
    let origin = loc.origin().map_err(js_message)?;
    let path = loc.pathname().map_err(js_message)?;
    Ok(format!("{origin}{path}"))
}

thread_local! {
    /// The query string Privy's OAuth return arrived with, kept by [`capture_oauth_return`].
    static OAUTH_RETURN: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Called first in `main`: keep Privy's OAuth return (`privy_oauth_code`, `privy_oauth_state`)
/// from the URL before the router rewrites it to the bare route.
pub fn capture_oauth_return() {
    let search = web_sys::window()
        .and_then(|w| w.location().search().ok())
        .unwrap_or_default();
    if search.contains("privy_oauth_code=") && search.contains("privy_oauth_state=") {
        OAUTH_RETURN.with(|r| *r.borrow_mut() = Some(search));
    }
}

/// A Privy session is stored in this browser (the SDK's refresh token, single- or multi-user).
fn session_stored() -> bool {
    let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) else {
        return false;
    };
    let n = storage.length().unwrap_or(0);
    (0..n).any(|i| {
        storage
            .key(i)
            .ok()
            .flatten()
            .is_some_and(|k| k.starts_with("privy:") && k.ends_with("refresh_token"))
    })
}

/// The first Google tap: load the SDK, ask Privy for Google's URL and go there. Resolves (with
/// that URL) just before the page navigates away.
pub async fn start_google_login() -> Result<String, String> {
    let uri = redirect_uri()?;
    let url = call("startGoogleLogin", &[JsValue::from_str(&uri)]).await?;
    url.as_string()
        .ok_or_else(|| "Privy returned no Google URL".to_string())
}

/// The user's embedded Solana wallet as the app's signer (created on first use). `Ok(false)`
/// when there is no Privy session.
async fn connect(wallet: Wallet) -> Result<bool, String> {
    let address = call("solanaAddress", &[]).await?;
    let Some(address) = address.as_string() else {
        return Ok(false);
    };
    let key = Pubkey::from_str(&address).map_err(|e| format!("bad Privy address: {e}"))?;
    wallet.connect_signer(PRIVY_WALLET_NAME, Rc::new(PrivySigner { key }));
    Ok(true)
}

/// Boot hook: back from Google, finish the login; with a stored session, reconnect the Privy
/// wallet. Does nothing (and loads nothing) otherwise, or when another wallet is connected.
pub async fn resume(wallet: Wallet) {
    let returning = OAUTH_RETURN.with(|r| r.borrow_mut().take());
    if returning.is_none() && !session_stored() {
        return;
    }
    *PRIVY_STATUS.write() = PrivyStatus::Busy("Signing in with Google…");
    let result = async {
        if let Some(search) = returning {
            call("completeLoginIfRedirected", &[JsValue::from_str(&search)]).await?;
        }
        // A wallet extension that autoconnected meanwhile keeps the seat.
        if wallet.public_key().is_some() {
            return Ok(true);
        }
        connect(wallet).await
    }
    .await;
    *PRIVY_STATUS.write() = match result {
        Ok(_) => PrivyStatus::Idle,
        Err(e) => {
            warn!("Privy: {e}");
            PrivyStatus::Failed(format!("Google sign-in failed: {e}"))
        }
    };
}

/// The Privy embedded Solana wallet. Signs only; the app sends.
pub struct PrivySigner {
    key: Pubkey,
}

impl PrivySigner {
    fn verify(&self, msg: &[u8], sig: &[u8; 64]) -> Result<(), WalletError> {
        let vk = ed25519_dalek::VerifyingKey::from_bytes(&self.key.to_bytes())
            .map_err(|e| WalletError::Codec(e.to_string()))?;
        vk.verify_strict(msg, &ed25519_dalek::Signature::from_bytes(sig))
            .map_err(|_| {
                WalletError::Codec("Privy returned a signature that does not verify".into())
            })
    }
}

fn js_err(e: String) -> WalletError {
    WalletError::from_js(None, &e)
}

#[async_trait::async_trait(?Send)]
impl WalletSigner for PrivySigner {
    fn pubkey(&self) -> Pubkey {
        self.key
    }

    async fn sign_message(&self, msg: &[u8]) -> Result<Signature, WalletError> {
        let out = call("signMessage", &[bytes_arg(msg)])
            .await
            .map_err(js_err)?;
        let sig: [u8; 64] = bytes_of(&out)
            .map_err(WalletError::Js)?
            .try_into()
            .map_err(|_| WalletError::Codec("signature is not 64 bytes".into()))?;
        self.verify(msg, &sig)?;
        Ok(Signature::from(sig))
    }

    /// Signs the message bytes as they are into this key's slot; every other slot is returned
    /// as it came in. Refuses a transaction this key does not sign, or one Privy changed.
    async fn sign_transaction(
        &self,
        tx: VersionedTransaction,
    ) -> Result<VersionedTransaction, WalletError> {
        let original = serialize_transaction(&tx)?;
        let me = self.key.to_bytes();
        let layout = wire::parse(&original).map_err(WalletError::Codec)?;
        wire::signer_slot(&layout, &me).map_err(WalletError::Codec)?;
        let out = call("signTransaction", &[bytes_arg(&original)])
            .await
            .map_err(js_err)?;
        let signed = bytes_of(&out).map_err(WalletError::Js)?;
        let (merged, slot) =
            wire::merge_signer_signature(&original, &signed, &me).map_err(WalletError::Codec)?;
        let at = layout.sigs_start + slot * 64;
        let mut sig = [0u8; 64];
        sig.copy_from_slice(&merged[at..at + 64]);
        self.verify(wire::message(&merged, &layout), &sig)?;
        deserialize_transaction(&merged)
    }

    /// The app submits through its own RPC ([`crate::wallet::Wallet::sign_and_send`]).
    async fn sign_and_send_transaction(
        &self,
        _tx: VersionedTransaction,
    ) -> Result<Signature, WalletError> {
        Err(WalletError::Feature(
            "the Privy wallet only signs; the app sends".into(),
        ))
    }

    /// Ends the Privy session on this device.
    async fn disconnect(&self) -> Result<(), WalletError> {
        call("logout", &[]).await.map(|_| ()).map_err(js_err)
    }
}

/// After the Google round trip the page reloads, so [`resume`] does the connecting; this is
/// for a tap while a Privy session already exists (e.g. after disconnecting another wallet):
/// reconnect without leaving the page, else start the Google login.
pub async fn login_or_reconnect(wallet: Wallet) -> Result<(), String> {
    if session_stored() && connect(wallet).await? {
        return Ok(());
    }
    start_google_login().await.map(|_| ())
}
