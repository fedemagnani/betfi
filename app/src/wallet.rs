//! The app's only wallet surface (plan 1.F; 2.E builds on it). Backed by `dioxus-solana` 0.5.1:
//! Wallet Standard wallets in the browser (Phantom, Solflare, Backpack…), Mobile Wallet Adapter
//! on Android Chrome. Screens use [`use_wallet_api`] and nothing else from `dioxus-solana`, so the
//! `wallet.js` fallback (D-010) would replace only this file.
//!
//! - [`Wallet::connect`]: connect (Phantom if present, else the first wallet found).
//! - [`Wallet::public_key`]: the connected account (reactive).
//! - [`Wallet::sign_and_send`]: serialized unsigned legacy or v0 transaction in, signature out.
//!   The wallet signs, and the app submits through its own RPC (devnet or localnet; never
//!   mainnet), so the transaction lands on the cluster whose blockhash it carries whatever
//!   network the wallet extension is set to.
//! - [`Wallet::send_instructions`] / [`Wallet::send`] (L8-2): instructions built for a payer in,
//!   signature out. When the desk sponsors transactions (`GET /relay`, learned at start), the
//!   sponsor is the fee payer and rent payer, the wallet only signs its own slot, and the desk's
//!   `POST /relay` co-signs and submits: the user never needs SOL. Otherwise the wallet pays,
//!   exactly as before.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::rc::Rc;

use dioxus::logger::tracing::warn;
use dioxus::prelude::*;
use dioxus_solana::rpc::{RpcContext, RpcRequest, use_rpc_context};
use dioxus_solana::{
    AccountMeta, ConnectedAccount, Hash, Instruction, Pubkey, SendError, Signature, WalletContext,
    WalletHandle, WalletSigner, WalletState, build_transaction, deserialize_transaction,
    serialize_transaction, use_wallet,
};
pub use schedina_tx::sponsor::Compile;
use schedina_tx::sponsor::{
    self, NotSponsored, Table, check_self_paid, check_sponsored, relay_tx_b64, self_pay_after,
};
use schedina_tx::swap::{
    account_data_b64, amm_config_lookup_table, amm_pda, lookup_table_addresses,
};
use schedina_tx::{CLOCK_SYSVAR_ID, TxError, clock_unix_timestamp_b64};

use crate::config::{MAINNET_GENESIS_HASH, is_mainnet_url};
use crate::desk::{Desk, DeskError};

/// SPL Memo program v2.
pub const MEMO_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");

/// Why a wallet operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalletError {
    /// No Wallet Standard wallet registered in this browser.
    NoWallet,
    /// No wallet connected.
    NotConnected,
    /// The user declined in the wallet.
    Rejected,
    /// The bytes are not a transaction this wallet can sign.
    BadTransaction(String),
    /// The RPC endpoint is mainnet (by URL or genesis hash): nothing is sent.
    MainnetRefused,
    /// The RPC node refused or failed the request (preflight errors included).
    Rpc(String),
    /// Any other wallet failure.
    Wallet(String),
    /// The instructions could not be built; the words are the user's already.
    Build(String),
    /// Over the 1,232-byte packet limit, and the wallet cannot pay for the smaller self-paid
    /// form (or it too is over).
    TooLarge,
    /// The desk sponsors transactions but cannot right now (`503`), and the wallet has too
    /// little SOL to pay itself.
    SponsorUnavailable,
    /// The desk refused to relay the transaction (`422`), with its reason.
    SponsorRefused(String),
}

impl fmt::Display for WalletError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WalletError::NoWallet => write!(f, "No Solana wallet found: install Phantom"),
            WalletError::NotConnected => write!(f, "Connect a wallet first"),
            WalletError::Rejected => write!(f, "Declined in the wallet"),
            WalletError::BadTransaction(e) => write!(f, "Cannot sign this transaction: {e}"),
            WalletError::MainnetRefused => {
                write!(
                    f,
                    "Refused: the RPC endpoint is mainnet, this app is devnet only"
                )
            }
            WalletError::Rpc(e) => write!(f, "Network error: {e}"),
            WalletError::Wallet(e) => write!(f, "Wallet error: {e}"),
            WalletError::Build(e) => write!(f, "{e}"),
            WalletError::TooLarge => write!(
                f,
                "Too many picks for one transaction right now: remove one and try again"
            ),
            WalletError::SponsorUnavailable => {
                write!(f, "Sponsored transactions are unavailable right now")
            }
            WalletError::SponsorRefused(r) if r.is_empty() => {
                write!(f, "The desk refused to send this transaction")
            }
            WalletError::SponsorRefused(r) => write!(f, "{r}"),
        }
    }
}

impl From<dioxus_solana::WalletError> for WalletError {
    fn from(e: dioxus_solana::WalletError) -> Self {
        use dioxus_solana::WalletError as E;
        match e {
            E::NotInstalled => WalletError::NoWallet,
            E::UserRejected => WalletError::Rejected,
            E::Disconnected => WalletError::NotConnected,
            E::Codec(m) => WalletError::BadTransaction(m),
            other => WalletError::Wallet(other.to_string()),
        }
    }
}

impl From<SendError> for WalletError {
    fn from(e: SendError) -> Self {
        match e {
            SendError::Wallet(w) => w.into(),
            other => WalletError::Rpc(other.to_string()),
        }
    }
}

/// Copyable handle over the wallet and RPC contexts. Get it with [`use_wallet_api`] in any
/// component under the root providers; it can then be moved into event handlers and tasks.
#[derive(Clone, Copy)]
pub struct Wallet {
    handle: WalletHandle,
    ctx: WalletContext,
    rpc: RpcContext,
}

pub fn use_wallet_api() -> Wallet {
    // The first component to ask (the root) starts learning the desk's sponsor (L8-2).
    use_hook(|| {
        if !RELAY_ASKED.with(|a| a.replace(true)) {
            dioxus::core::spawn_forever(async {
                learn_relay().await;
            });
        }
    });
    Wallet {
        handle: use_wallet(),
        ctx: use_context::<WalletContext>(),
        rpc: use_rpc_context(),
    }
}

thread_local! {
    /// RPC URLs whose genesis hash was checked and found not to be mainnet's.
    static CHECKED_RPC: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// `GET /relay` was asked once at start.
    static RELAY_ASKED: Cell<bool> = const { Cell::new(false) };
}

/// The desk's transaction sponsoring (L8-2), from `GET /relay`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relay {
    /// Not known yet (the desk has not answered; a send asks again).
    Unknown,
    /// No sponsoring (`503`, a desk without the route, demo mode): every flow is self-paid.
    Off,
    /// The desk's sponsor key pays fees and rent.
    On(Pubkey),
}

/// What the app knows of the desk's sponsoring: shared by every [`Wallet`], like a context
/// provided by the root.
static RELAY: GlobalSignal<Relay> = Signal::global(|| Relay::Unknown);

/// Ask the desk whether it sponsors transactions, and remember the answer. A desk that cannot
/// be reached leaves it [`Relay::Unknown`] (the next send asks again).
async fn learn_relay() -> Relay {
    let relay = match Desk::from_config().relay_info().await {
        Ok(Some(info)) => match info.sponsor.parse::<Pubkey>() {
            Ok(pk) => Relay::On(pk),
            Err(_) => {
                warn!("the desk's relay names a bad sponsor key {}", info.sponsor);
                Relay::Off
            }
        },
        Ok(None) => Relay::Off,
        Err(e) => {
            warn!("GET /relay: {e}");
            Relay::Unknown
        }
    };
    if *RELAY.peek() != relay {
        *RELAY.write() = relay;
    }
    relay
}

/// True when the desk sponsors transactions (not reactive; for wording outside components).
pub fn sponsoring() -> bool {
    matches!(*RELAY.peek(), Relay::On(_))
}

/// Why a sponsored send stopped: for good (a build error, the user declined), or for one of
/// the relay's reasons, after which the wallet's own SOL may still pay ([`self_pay_after`]).
enum Sponsored {
    Final(WalletError),
    Not(NotSponsored, WalletError),
}

fn tx_error(e: TxError) -> WalletError {
    match e {
        TxError::TooLarge(_) => WalletError::TooLarge,
        e => WalletError::BadTransaction(e.to_string()),
    }
}

impl Wallet {
    /// The connected account, if any. Reactive: a component calling it re-renders on change.
    pub fn public_key(&self) -> Option<Pubkey> {
        self.handle.pubkey()
    }

    /// The raw connection state (for status lines).
    pub fn state(&self) -> WalletState {
        self.handle.state()
    }

    /// The cluster the app signs and sends on.
    pub fn cluster(&self) -> dioxus_solana::Cluster {
        self.handle.cluster()
    }

    /// The app's RPC (account reads the wallet has no method for).
    pub fn rpc(&self) -> RpcContext {
        self.rpc
    }

    /// The desk's sponsor key while it sponsors transactions (L8-2). Reactive.
    pub fn sponsor(&self) -> Option<Pubkey> {
        match *RELAY.read() {
            Relay::On(pk) => Some(pk),
            _ => None,
        }
    }

    /// True while the desk sponsors transactions: no screen asks the user for SOL. Reactive.
    pub fn sponsored(&self) -> bool {
        self.sponsor().is_some()
    }

    /// The sponsor for a send now: asks the desk if nothing is known yet.
    async fn sponsor_now(&self) -> Option<Pubkey> {
        let known = *RELAY.peek();
        match known {
            Relay::On(pk) => Some(pk),
            Relay::Off => None,
            Relay::Unknown => match learn_relay().await {
                Relay::On(pk) => Some(pk),
                _ => None,
            },
        }
    }

    /// True when the desk sponsors this send (asks the desk if nothing is known yet).
    pub async fn will_sponsor(&self) -> bool {
        self.sponsor_now().await.is_some()
    }

    /// Connect a wallet: Phantom if it registered, else the first Wallet Standard wallet found.
    /// Resolves once the user approved (or declined) in the wallet.
    pub async fn connect(&self) -> Result<Pubkey, WalletError> {
        if let Some(pk) = self.public_key() {
            return Ok(pk);
        }
        let wallets = self.ctx.wallets.peek().clone();
        let chosen = wallets
            .iter()
            .find(|w| w.info().name.to_ascii_lowercase().contains("phantom"))
            .or_else(|| wallets.first())
            .cloned()
            .ok_or(WalletError::NoWallet)?;
        let mut state = self.ctx.state;
        state.set(WalletState::Connecting);
        // The wallet round trip runs in a root-level task, so the shared state always leaves
        // `Connecting` even if the component that asked unmounts (cancelling this future).
        let (tx, rx) = futures_channel::oneshot::channel();
        dioxus::core::spawn_forever(async move {
            let result = match chosen.connect().await {
                Ok(acc) => {
                    // Same key dioxus-solana's own connect writes, so autoconnect restores it.
                    dioxus_solana::storage::set_last_wallet(&acc.wallet_name);
                    let pk = acc.pubkey();
                    state.set(WalletState::Connected(acc));
                    Ok(pk)
                }
                Err(e) => {
                    state.set(WalletState::Error(e.clone()));
                    Err(e.into())
                }
            };
            let _ = tx.send(result);
        });
        rx.await
            .unwrap_or_else(|_| Err(WalletError::Wallet("connection task dropped".into())))
    }

    pub fn disconnect(&self) {
        self.handle.disconnect();
    }

    /// Log out of the connected provider and wait until it is done (1 Nov, user request: a switch
    /// to another provider de-authenticates the previous one): the wallet's own disconnect
    /// (Phantom's `standard:disconnect`, Privy's logout for the Google wallet), the remembered
    /// wallet cleared so it does not reconnect on the next visit. Waiting matters: the
    /// disconnect lands asynchronously and would otherwise drop the newly connected wallet.
    pub async fn log_out(&self) {
        if !self.handle.state().is_connected() {
            return;
        }
        self.handle.disconnect();
        for _ in 0..100 {
            if !self.handle.state().is_connected() {
                return;
            }
            gloo_timers::future::sleep(std::time::Duration::from_millis(50)).await;
        }
        warn!("the previous wallet took over 5 s to disconnect");
    }

    /// Sign a serialized transaction (bincode wire format of a legacy or v0
    /// `VersionedTransaction`, signature slots zeroed) with the connected wallet and submit it
    /// through the app's RPC. Returns once the node accepted it (preflight passed); call
    /// [`Wallet::confirm`] to wait for `confirmed`.
    ///
    /// Refuses before the wallet opens if: no wallet is connected; the bytes do not decode;
    /// the connected account is not the fee payer and only signer (a transaction another key
    /// must sign, such as a sponsored one, would only be refused by the node: those go through
    /// [`Wallet::send`]); the RPC is mainnet by URL or genesis hash.
    pub async fn sign_and_send(&self, tx_bytes: Vec<u8>) -> Result<Signature, WalletError> {
        let me = self.public_key().ok_or(WalletError::NotConnected)?;
        let tx = deserialize_transaction(&tx_bytes)?;
        let required = usize::from(tx.message.header().num_required_signatures);
        if check_self_paid(&tx.message, &me).is_err() {
            return Err(WalletError::BadTransaction(format!(
                "{me} is not the fee payer and only signer of this transaction"
            )));
        }
        if tx.signatures.len() != required {
            return Err(WalletError::BadTransaction(format!(
                "{} signature slots for {required} signers",
                tx.signatures.len()
            )));
        }
        self.ensure_not_mainnet().await?;
        let rpc = self.rpc.client();
        let sent_message = tx.message.clone();
        // dioxus-solana: sign with the wallet, submit through our RPC; a wallet that can only
        // sign-and-send falls back to that (it is still asked for this app's chain).
        let sig = match self.handle.sign_transaction(tx.clone()).await {
            Ok(signed) => {
                if signed.message != sent_message {
                    // Some wallets append instructions (e.g. guards). The program reads the
                    // ed25519 instruction by index, so 2.E must know if this ever happens.
                    warn!("the wallet changed the transaction message before signing");
                }
                rpc.send_versioned_transaction(&signed).await?
            }
            Err(dioxus_solana::WalletError::Feature(_)) => self.handle.sign_and_send(tx).await?,
            Err(e) => return Err(e.into()),
        };
        Ok(sig)
    }

    /// Sign `msg` as it is with the connected wallet (Wallet Standard `solana:signMessage`, or
    /// the dev burner): the 64-byte ed25519 signature. Nothing is sent anywhere, so no cluster
    /// check; used for the desk's signed profile writes (L6-2 `PUT /profile`).
    pub async fn sign_message(&self, msg: &[u8]) -> Result<[u8; 64], WalletError> {
        if self.public_key().is_none() {
            return Err(WalletError::NotConnected);
        }
        let sig = self.handle.sign_message(msg.to_vec()).await?;
        Ok(sig.into())
    }

    /// Wait until `sig` is confirmed (about 90 s at most), or failed on chain.
    pub async fn confirm(&self, sig: &Signature) -> Result<(), WalletError> {
        Ok(self.rpc.client().confirm_transaction(sig).await?)
    }

    /// A fresh blockhash from the app's RPC, for building a transaction to pass to
    /// [`Wallet::sign_and_send`].
    pub async fn latest_blockhash(&self) -> Result<Hash, WalletError> {
        self.ensure_not_mainnet().await?;
        Ok(self.rpc.client().latest_blockhash().await?)
    }

    /// [`Wallet::send`] as a legacy transaction (faucets, cash-out).
    pub async fn send_instructions<F>(&self, build: F) -> Result<Signature, WalletError>
    where
        F: Fn(&Pubkey) -> Result<Vec<Instruction>, String>,
    {
        self.send(build, Compile::Legacy).await
    }

    /// Build the instructions for a payer with `build` (in order, untouched; its error is
    /// shown as is), compile them as `compile` says on a fresh blockhash, sign in the wallet
    /// and submit. Returns once the transaction was accepted; [`Wallet::confirm`] waits for it.
    ///
    /// - The desk sponsors (L8-2): `build(sponsor)`, fee payer the sponsor at key 0, the user
    ///   signs only their own slot (key 1, read-only), and `POST /relay` co-signs and submits.
    ///   A `422` is [`WalletError::SponsorRefused`] with the desk's reason. A `503`, a
    ///   transaction over the packet limit, or a wallet that cannot co-sign is retried
    ///   self-paid if the wallet holds `SELF_PAY_MIN_LAMPORTS` ([`self_pay_after`]); otherwise
    ///   it stops there ([`WalletError::SponsorUnavailable`], [`WalletError::TooLarge`]).
    /// - Otherwise: `build(user)`, the wallet pays, and the app's RPC submits, as before L8.
    ///
    /// The mainnet refusal runs before anything is built and again before the wallet opens.
    pub async fn send<F>(&self, build: F, compile: Compile) -> Result<Signature, WalletError>
    where
        F: Fn(&Pubkey) -> Result<Vec<Instruction>, String>,
    {
        let user = self.public_key().ok_or(WalletError::NotConnected)?;
        self.ensure_not_mainnet().await?;
        if let Some(sponsor) = self.sponsor_now().await {
            match self.send_sponsored(user, sponsor, &build, &compile).await {
                Ok(sig) => return Ok(sig),
                Err(Sponsored::Final(e)) => return Err(e),
                Err(Sponsored::Not(why, e)) => {
                    // The balance is read only when it can matter.
                    let sol = if self_pay_after(why, u64::MAX) {
                        self.sol_balance().await.unwrap_or(0)
                    } else {
                        0
                    };
                    if !self_pay_after(why, sol) {
                        return Err(e);
                    }
                    warn!("not sponsored ({e}): the wallet pays this one");
                }
            }
        }
        let ixs = build(&user).map_err(WalletError::Build)?;
        let blockhash = self.latest_blockhash().await?;
        let wire = sponsor::compile(&user, &ixs, &compile.format(false), blockhash.to_bytes())
            .map_err(tx_error)?;
        self.sign_and_send(wire).await
    }

    /// The sponsored path of [`Wallet::send`].
    async fn send_sponsored<F>(
        &self,
        user: Pubkey,
        sponsor: Pubkey,
        build: &F,
        compile: &Compile,
    ) -> Result<Signature, Sponsored>
    where
        F: Fn(&Pubkey) -> Result<Vec<Instruction>, String>,
    {
        use Sponsored::{Final, Not};
        let ixs = build(&sponsor).map_err(|e| Final(WalletError::Build(e)))?;
        let blockhash = self.latest_blockhash().await.map_err(Final)?.to_bytes();
        let msg = sponsor::compile_sponsored(&sponsor, ixs, &compile.format(true), blockhash)
            .map_err(|e| Final(tx_error(e)))?;
        check_sponsored(&msg, &sponsor, &user)
            .map_err(|e| Final(WalletError::BadTransaction(e.to_string())))?;
        let wire = sponsor::wire(&msg).map_err(|e| match e {
            TxError::TooLarge(_) => Not(NotSponsored::TooLarge, WalletError::TooLarge),
            e => Final(tx_error(e)),
        })?;
        let tx = deserialize_transaction(&wire).map_err(|e| Final(e.into()))?;
        let sent_message = tx.message.clone();
        self.ensure_not_mainnet().await.map_err(Final)?;
        // Sign only: a wallet's sign-and-send would submit without the sponsor's signature.
        let signed = match self.handle.sign_transaction(tx).await {
            Ok(signed) => signed,
            Err(dioxus_solana::WalletError::Feature(_)) => {
                return Err(Not(
                    NotSponsored::CannotCoSign,
                    WalletError::Wallet(
                        "this wallet cannot sign a transaction another key pays for".into(),
                    ),
                ));
            }
            Err(e) => return Err(Final(e.into())),
        };
        if signed.message != sent_message {
            // The desk relays only what its rules allow; it decides on the changed message.
            warn!("the wallet changed the sponsored transaction before signing");
            check_sponsored(&signed.message, &sponsor, &user)
                .map_err(|e| Final(WalletError::BadTransaction(e.to_string())))?;
        }
        if signed.signatures.len() != 2 || signed.signatures[1] == Signature::default() {
            return Err(Final(WalletError::BadTransaction(
                "the wallet did not sign its slot".into(),
            )));
        }
        let bytes = serialize_transaction(&signed).map_err(|e| Final(e.into()))?;
        match Desk::from_config().relay(relay_tx_b64(&bytes)).await {
            Ok(sig) => sig.parse::<Signature>().map_err(|_| {
                Final(WalletError::Rpc(format!(
                    "the desk answered a bad signature {sig}"
                )))
            }),
            Err(DeskError::Rejected(reason)) => Err(Not(
                NotSponsored::Refused,
                WalletError::SponsorRefused(reason),
            )),
            Err(DeskError::Unavailable(_)) => Err(Not(
                NotSponsored::Unavailable,
                WalletError::SponsorUnavailable,
            )),
            // Not a clean refusal: the desk may have sent it, so nothing is sent again.
            Err(e) => Err(Not(NotSponsored::NoAnswer, WalletError::Rpc(e.to_string()))),
        }
    }

    /// Balance of an SPL token account in base units, `None` when the account does not exist.
    pub async fn token_balance(&self, account: &Pubkey) -> Result<Option<u64>, WalletError> {
        self.ensure_not_mainnet().await?;
        match self
            .rpc
            .client()
            .get_token_account_balance(account, None)
            .await
        {
            Ok(resp) => Ok(resp.value.amount.parse().ok()),
            // The node answers "could not find account" (or "Invalid param") for a missing one.
            Err(e) => {
                let text = e.to_string();
                if text.contains("could not find account") || text.contains("Invalid param") {
                    Ok(None)
                } else {
                    Err(WalletError::Rpc(text))
                }
            }
        }
    }

    /// Ask the cluster's faucet for `lamports` (devnet/localnet only; the public devnet faucet
    /// is rate-limited). Returns the airdrop signature.
    pub async fn airdrop(&self, lamports: u64) -> Result<String, WalletError> {
        let me = self.public_key().ok_or(WalletError::NotConnected)?;
        self.ensure_not_mainnet().await?;
        let rpc = self.rpc.client();
        let sig = rpc
            .request_airdrop(me, lamports, None)
            .await
            .map_err(|e| WalletError::Rpc(e.to_string()))?;
        rpc.confirm_transaction(&sig).await?;
        Ok(sig)
    }

    /// The cluster's clock: `Clock::unix_timestamp` of the latest processed bank, read from the
    /// Clock sysvar through the app's RPC. This is the `now` the program checks the Pending's
    /// `expires_at` against (L3-4), so it is used instead of the browser clock.
    pub async fn cluster_now_s(&self) -> Result<i64, WalletError> {
        self.ensure_not_mainnet().await?;
        let resp: serde_json::Value = self
            .rpc
            .client()
            .send(
                RpcRequest::GetAccountInfo,
                serde_json::json!([
                    CLOCK_SYSVAR_ID.to_string(),
                    {"encoding": "base64", "commitment": "processed"}
                ]),
            )
            .await
            .map_err(|e| WalletError::Rpc(e.to_string()))?;
        let b64 = resp["value"]["data"][0]
            .as_str()
            .ok_or_else(|| WalletError::Rpc("the clock sysvar was not returned".into()))?;
        clock_unix_timestamp_b64(b64)
            .map(|(_, ts)| ts)
            .map_err(|e| WalletError::Rpc(e.to_string()))
    }

    /// SOL balance of the connected account, in lamports.
    pub async fn sol_balance(&self) -> Result<u64, WalletError> {
        let me = self.public_key().ok_or(WalletError::NotConnected)?;
        self.ensure_not_mainnet().await?;
        Ok(self
            .rpc
            .client()
            .get_balance(me, None)
            .await
            .map_err(|e| WalletError::Rpc(e.to_string()))?
            .value)
    }

    /// Connect an in-page signer directly (the dev-only burner wallet, [`crate::burner`]),
    /// bypassing Wallet Standard discovery. Replaces any current connection.
    pub fn connect_signer(&self, wallet_name: &str, signer: Rc<dyn WalletSigner>) {
        let mut state = self.ctx.state;
        state.set(WalletState::Connected(ConnectedAccount {
            wallet_name: wallet_name.to_string(),
            signer,
        }));
    }

    /// Refuse a mainnet endpoint: by URL, then by genesis hash (checked once per URL).
    async fn ensure_not_mainnet(&self) -> Result<(), WalletError> {
        let rpc = self.rpc.client();
        let url = rpc.url().to_string();
        if is_mainnet_url(&url) || self.cluster().chain_id() == "solana:mainnet" {
            return Err(WalletError::MainnetRefused);
        }
        if CHECKED_RPC.with(|c| c.borrow().contains(&url)) {
            return Ok(());
        }
        let genesis = rpc
            .get_genesis_hash()
            .await
            .map_err(|e| WalletError::Rpc(e.to_string()))?;
        if genesis == MAINNET_GENESIS_HASH {
            return Err(WalletError::MainnetRefused);
        }
        CHECKED_RPC.with(|c| c.borrow_mut().push(url));
        Ok(())
    }
}

/// Data of one account through the app's RPC (`None` when absent).
async fn account_data(rpc: RpcContext, key: &Pubkey) -> Result<Option<Vec<u8>>, String> {
    let resp: serde_json::Value = rpc
        .client()
        .send(
            RpcRequest::GetAccountInfo,
            serde_json::json!([key.to_string(), {"encoding": "base64", "commitment": "confirmed"}]),
        )
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    match resp["value"]["data"][0].as_str() {
        Some(b64) => account_data_b64(b64).map(Some).map_err(|e| e.to_string()),
        None => Ok(None),
    }
}

/// The mock AMM's address lookup table (`AmmConfig.lookup_table`: key and addresses), `None`
/// while it has none; an error where the AMM is not set up. The swap bet always uses it; a
/// sponsored plain bet too, when there is one (L8-2).
pub async fn amm_lookup_table(rpc: RpcContext) -> Result<Option<Table>, String> {
    let Some(cfg) = account_data(rpc, &amm_pda::amm_config()).await? else {
        return Err("The swap program is not set up on this cluster.".into());
    };
    let Some(key) = amm_config_lookup_table(&cfg).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let data = account_data(rpc, &key)
        .await?
        .ok_or("The swap lookup table is missing.")?;
    Ok(Some((
        key,
        lookup_table_addresses(&data).map_err(|e| e.to_string())?,
    )))
}

/// The wallet spike's test transaction: one SPL Memo instruction signed by `payer`, serialized
/// unsigned, exactly as 2.E will hand its `open_pending` transaction to `sign_and_send`.
pub fn memo_transaction(
    payer: Pubkey,
    blockhash: Hash,
    text: &str,
) -> Result<Vec<u8>, WalletError> {
    let ix = Instruction {
        program_id: MEMO_PROGRAM_ID,
        accounts: vec![AccountMeta::new_readonly(payer, true)],
        data: text.as_bytes().to_vec(),
    };
    let tx = build_transaction(&payer, &[ix], blockhash);
    Ok(serialize_transaction(&tx)?)
}
