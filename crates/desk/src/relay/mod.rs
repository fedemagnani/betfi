//! Sponsored transactions (L8): the desk's sponsor key pays the fee and the rent of transactions
//! the user signs, so a wallet needs no SOL to claim test USDC, swap or bet.
//!
//! | Route | What |
//! | --- | --- |
//! | `GET /relay` | `{"sponsor","max_lamports_per_tx"}` while sponsoring is on; 503 `unavailable` when off |
//! | `POST /relay` | `{"tx"}` → `{"signature"}` once the RPC accepted it; 422 `rejected` with the reason; 503 when off or the RPC is down |
//!
//! `tx` is base64 of a wire `VersionedTransaction`, legacy or v0: signature slot 0 is the
//! sponsor's (zeroed; the desk fills it), slot 1 the user's, already signed.
//!
//! # Rules, all checked before the desk signs
//!
//! 1. It decodes; exactly two required signatures; `static_account_keys[0]` is the sponsor (fee
//!    payer) and `[1]` the user, whose signature verifies over the message. The blockhash is
//!    left to the RPC.
//! 2. Top-level instructions come from an allowlist ([`rules`]): ComputeBudget unit limit, and
//!    unit price up to [`rules::MAX_CU_PRICE`]; the Ed25519 precompile; SPL Memo; ATA
//!    `CreateIdempotent` for the user's own account; schedina `open_pending`, `cash_out`,
//!    `faucet_usdc`; mock-amm `swap` and `faucet`. Anything else is refused (every System
//!    program instruction, the crank's schedina instructions, unknown programs).
//! 3. The sponsor key stands only where it pays: fee payer, `open_pending.payer`,
//!    `faucet_usdc.caller` (then the recipient must be the user's mock-USDC account and the
//!    transaction must carry the memo `schedina:faucet` signed by the user), mock-amm
//!    `faucet.payer`, and the funder of an ATA `CreateIdempotent`. Never a token authority or a
//!    transfer source. Lookup tables are read first; one holding the sponsor is refused.
//! 4. `open_pending`, `cash_out`, `swap` and mock-amm `faucet` are the user's own.
//! 5. A simulation (signatures not verified) succeeds and costs the sponsor at most
//!    [`MAX_LAMPORTS_PER_TX`]; a failure is refused with the program's error in plain words.
//! 6. Rate limits in memory ([`limits`]): per user, per client IP, and the simulated spend per UTC
//!    day (`DESK_RELAY_DAILY_LAMPORTS`). Over a limit: 422 `rate limited`.
//! 7. Then the desk signs slot 0 and sends (preflight on). The sponsor key must not be the crank
//!    key, the quote signer, the authority or the hedger's trading key: if it is, the relay stays
//!    off (logged) and serves 503.
//!
//! Configuration: `SPONSOR_KEYPAIR_PATH` (default `<keys>/sponsor.json`; off when the file is
//! missing), `DESK_RELAY` (`0` off), `DESK_RELAY_DAILY_LAMPORTS`. Neither key material nor the
//! RPC URL is ever logged.

pub mod chain;
pub mod limits;
pub mod rules;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use axum::extract::{ConnectInfo, State};
use axum::http::Extensions;
use axum::routing::get;
use axum::{Json, Router};
use solana_pubkey::Pubkey;

use crate::api::ApiError;
use crate::api::types::{RelayInfo, RelayRequest, RelayResponse};
use crate::config::Config;
use crate::now_ms;
use crate::program::Pdas;
use crate::solana::rpc::RpcClient;
use crate::solana::signer::Keypair;
use crate::state::AppState;
use chain::{Chain, ChainError, RpcChain};
use limits::Limits;
use rules::{Policy, Refusal};

/// The most one relayed transaction may cost the sponsor (fee plus rent), lamports.
pub const MAX_LAMPORTS_PER_TX: u64 = 10_000_000;
/// The 422 reason when a limit is reached.
pub const RATE_LIMITED: &str = "rate limited";

/// Whether the relay serves, held in [`crate::state::Shared::relay`].
pub enum Status {
    /// Sponsoring.
    On(Arc<Relay>),
    /// Not sponsoring, and why (served as the 503 reason).
    Off(String),
}

/// The relay: the sponsor key, the rules' policy, the chain and the limits.
pub struct Relay {
    sponsor: Keypair,
    policy: Policy,
    chain: Arc<dyn Chain>,
    limits: Limits,
}

impl std::fmt::Debug for Relay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Relay({})", self.policy.sponsor)
    }
}

impl Relay {
    /// A relay signing with `sponsor` for schedina program `schedina` and mock AMM `amm`, with a
    /// daily budget of `daily_lamports` of simulated sponsor spend.
    pub fn new(
        sponsor: Keypair,
        schedina: Pubkey,
        amm: Pubkey,
        chain: Arc<dyn Chain>,
        daily_lamports: u64,
    ) -> Self {
        let policy = Policy {
            sponsor: sponsor.pubkey(),
            schedina,
            amm,
            usdc_mint: Pdas::new(schedina).usdc_mint(),
        };
        Relay {
            sponsor,
            policy,
            chain,
            limits: Limits::new(daily_lamports),
        }
    }

    /// The sponsor key.
    pub fn sponsor(&self) -> Pubkey {
        self.policy.sponsor
    }

    /// The rules' policy.
    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// The limits (simulated spend so far, for tests and logs).
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// `GET /relay`.
    pub fn info(&self) -> RelayInfo {
        RelayInfo {
            sponsor: self.policy.sponsor.to_string(),
            max_lamports_per_tx: MAX_LAMPORTS_PER_TX,
        }
    }

    /// `POST /relay`: checks `tx_b64` against every rule, co-signs it as the sponsor and sends
    /// it; the signature. `ip` keys the per-IP limit, `now_ms` the limits' clock.
    pub async fn submit(&self, tx_b64: &str, ip: &str, now_ms: u64) -> Result<String, ApiError> {
        let r = self.submit_inner(tx_b64, ip, now_ms).await;
        let outcome = match &r {
            Ok(_) => "sent",
            Err(e) if e.0.reason == RATE_LIMITED => "rate_limited",
            Err(e) if e.status_code().as_u16() == 422 => "refused",
            Err(_) => "unavailable",
        };
        metrics::counter!("desk_relay_total", "outcome" => outcome).increment(1);
        r
    }

    async fn submit_inner(&self, tx_b64: &str, ip: &str, now_ms: u64) -> Result<String, ApiError> {
        let refused = |r: Refusal| {
            tracing::info!(reason = %r, "relay: refused");
            ApiError::rejected(r.0)
        };
        if !self.limits.attempt(ip, now_ms) {
            tracing::info!("relay: too many attempts from one address");
            return Err(ApiError::rejected(RATE_LIMITED));
        }
        let mut tx = rules::decode(tx_b64).map_err(refused)?;
        let user = rules::check_signers(&tx, &self.policy.sponsor).map_err(refused)?;
        let user_s = user.to_string();
        if !self.limits.allows(&user_s, ip, 0, now_ms) {
            tracing::info!(user = %user_s, "relay: rate limited");
            return Err(ApiError::rejected(RATE_LIMITED));
        }
        let lookups = rules::lookup_tables(&tx);
        let tables = if lookups.is_empty() {
            Default::default()
        } else {
            self.chain
                .lookup_tables(&lookups)
                .await
                .map_err(|e| chain_error(&user_s, e))?
        };
        let checked = rules::check(&tx, &self.policy, &tables).map_err(|r| {
            tracing::info!(user = %user_s, reason = %r, "relay: refused");
            ApiError::rejected(r.0)
        })?;
        // A token account the sponsor already paid for is missing again only if its owner closed
        // it and kept the rent: the sponsor does not pay for it twice.
        let new_atas = if checked.sponsored_atas.is_empty() {
            Vec::new()
        } else {
            self.chain
                .missing(&checked.sponsored_atas)
                .await
                .map_err(|e| chain_error(&user_s, e))?
        };
        if new_atas.iter().any(|a| self.limits.ata_paid(a)) {
            let reason = "BetFi already paid for this token account once and it was closed since";
            tracing::info!(user = %user_s, reason, "relay: refused");
            return Err(ApiError::rejected(reason));
        }

        // Rule 5: what it costs the sponsor, on the unsigned transaction (slot 0 still zero).
        let wire = wincode::serialize(&tx)
            .map_err(|_| ApiError::rejected("the transaction cannot be re-encoded"))?;
        let sim = self
            .chain
            .simulate(&wire, &self.policy.sponsor)
            .await
            .map_err(|e| chain_error(&user_s, e))?;
        if let Some(err) = sim.err {
            tracing::info!(user = %user_s, reason = %err, "relay: simulation failed");
            return Err(ApiError::rejected(err));
        }
        let spend = sim.spend();
        if spend > MAX_LAMPORTS_PER_TX {
            let reason = format!(
                "it would cost the sponsor {spend} lamports, over the {MAX_LAMPORTS_PER_TX} limit"
            );
            tracing::info!(user = %user_s, %reason, "relay: refused");
            return Err(ApiError::rejected(reason));
        }
        // Rule 6, recorded now that it will be sent.
        if !self.limits.take(&user_s, ip, spend, now_ms) {
            tracing::info!(user = %user_s, spend, "relay: rate limited");
            return Err(ApiError::rejected(RATE_LIMITED));
        }

        self.limits.record_atas(&new_atas);

        // Rule 7: co-sign and send.
        tx.signatures[0] = self.sponsor.sign(&tx.message.serialize());
        let wire = wincode::serialize(&tx)
            .map_err(|_| ApiError::rejected("the transaction cannot be re-encoded"))?;
        let signature = self
            .chain
            .send(&wire)
            .await
            .map_err(|e| chain_error(&user_s, e))?;
        tracing::info!(
            user = %user_s,
            %signature,
            spend_lamports = spend,
            spent_today = self.limits.spent_today(now_ms),
            "relay: sent"
        );
        Ok(signature)
    }
}

fn chain_error(user: &str, e: ChainError) -> ApiError {
    match e {
        ChainError::Refused(reason) => {
            tracing::info!(user, %reason, "relay: refused by the chain");
            ApiError::rejected(reason)
        }
        ChainError::Unavailable(reason) => {
            tracing::warn!(user, %reason, "relay: chain unavailable");
            ApiError::unavailable(reason)
        }
    }
}

/// Reads the sponsor key and decides whether the relay serves (rules 7 and the configuration).
/// Logs why it is off; never logs key material or the RPC URL.
pub fn status_from_config(config: &Config) -> Status {
    let off = |why: String| {
        tracing::info!(reason = %why, "relay off");
        Status::Off(why)
    };
    if !config.relay_enabled {
        return off("sponsored transactions are off (DESK_RELAY=0)".into());
    }
    let path = &config.sponsor_keypair_path;
    if !path.exists() {
        // The path goes to the log only: the 503 reason is public.
        tracing::info!(path = %path.display(), "relay: no sponsor key file");
        return off("sponsored transactions are off (no sponsor key)".into());
    }
    let sponsor = match Keypair::read(path) {
        Ok(k) => k,
        Err(e) => {
            tracing::error!(error = %e, "relay: cannot read the sponsor key; relay off");
            return Status::Off("sponsored transactions are off (sponsor key unreadable)".into());
        }
    };
    let others: [(&str, Option<&Path>); 4] = [
        ("crank", Some(config.crank_keypair_path.as_path())),
        (
            "quote signer",
            Some(config.quote_signer_keypair_path.as_path()),
        ),
        ("authority", Some(config.authority_keypair_path.as_path())),
        ("hedger trading", config.hedger_trading_key_path.as_deref()),
    ];
    for (name, other) in others {
        let Some(other) = other else { continue };
        let same_file = matches!(
            (path.canonicalize(), other.canonicalize()),
            (Ok(a), Ok(b)) if a == b
        );
        let same_key = Keypair::read(other).is_ok_and(|k| k.pubkey() == sponsor.pubkey());
        if same_file || same_key {
            tracing::error!(
                key = name,
                "relay: the sponsor key is also the {name} key; refusing to sponsor with it"
            );
            // Which key goes to the log only: the 503 reason is public.
            return Status::Off(
                "sponsored transactions are off (the sponsor key is also another desk key)".into(),
            );
        }
    }
    let chain = Arc::new(RpcChain::new(RpcClient::new(config.solana_rpc_url.clone())));
    let relay = Relay::new(
        sponsor,
        config.program_id,
        crate::swap::program_id_from_env(),
        chain,
        config.relay_daily_lamports,
    );
    tracing::info!(
        sponsor = %relay.sponsor(),
        max_lamports_per_tx = MAX_LAMPORTS_PER_TX,
        daily_lamports = config.relay_daily_lamports,
        "relay on"
    );
    Status::On(Arc::new(relay))
}

/// Adds `GET /relay` and `POST /relay`. The status is decided here, once (a test may set
/// [`crate::state::Shared::relay`] before the router is built).
pub fn mount(router: Router<AppState>, state: &AppState) -> Router<AppState> {
    state
        .relay
        .get_or_init(|| status_from_config(&state.config));
    router.route("/relay", get(get_relay).post(post_relay))
}

fn relay_of(state: &AppState) -> Result<Arc<Relay>, ApiError> {
    match state.relay.get() {
        Some(Status::On(r)) => Ok(r.clone()),
        Some(Status::Off(why)) => Err(ApiError::unavailable(why.clone())),
        None => Err(ApiError::unavailable("sponsored transactions are off")),
    }
}

/// `GET /relay`.
async fn get_relay(State(state): State<AppState>) -> Result<Json<RelayInfo>, ApiError> {
    Ok(Json(relay_of(&state)?.info()))
}

/// `POST /relay`.
async fn post_relay(
    State(state): State<AppState>,
    ext: Extensions,
    body: axum::body::Bytes,
) -> Result<Json<RelayResponse>, ApiError> {
    let relay = relay_of(&state)?;
    // Parsed here so a malformed body gets the C4 `rejected` body, not axum's.
    let req: RelayRequest = serde_json::from_slice(&body)
        .map_err(|_| ApiError::rejected(r#"body must be {"tx":"<base64 transaction>"}"#))?;
    let ip = ext
        .get::<ConnectInfo<SocketAddr>>()
        .map_or_else(|| "unknown".to_owned(), |c| c.0.ip().to_string());
    let signature = relay.submit(&req.tx, &ip, now_ms()).await?;
    Ok(Json(RelayResponse { signature }))
}
