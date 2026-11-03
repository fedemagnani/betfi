//! Pay in any token (spec section 8, plan step 4.D, contract L4-3): the devnet/localnet mock
//! AMM (`programs/mock-amm`) as the desk serves it.
//!
//! - `GET /tokens` → `{"tokens":[{"mint","symbol","decimals","usdc_mint"}]}`: mock USDC first
//!   (`usdc_mint: true`), then every mock mint whose pool exists against that USDC mint.
//! - `POST /swap-quote {"mint","amount_in":"<u64>"}` →
//!   `{"mint","amount_in","usdc_out_u","fee_u","min_out_u","slippage_bps":50,"route":"mock-amm","pool"}`:
//!   selling `amount_in` of `mint` for USDC at the pool's current reserves. `usdc_out_u` is
//!   what the pool pays now, `fee_u` the 0.5% fee counted in USDC (the "Swap fee" line of
//!   spec 8's fee stack: no-fee output minus `usdc_out_u`), `min_out_u` the 50 bps slippage
//!   floor the app signs as `min_out` and quotes the desk with (`stake_u = min_out_u`).
//!
//! Both are computed from the pool accounts and their vault balances, read over RPC
//! (`SOLANA_RPC_URL`) in one `getMultipleAccounts` and cached [`CACHE_TTL`]. The app's
//! transaction is `[swap, Ed25519 verify, open_pending]` (v0, lookup table in
//! `AmmConfig.lookup_table`); a failed swap fails it all. The program ID is
//! `MOCK_AMM_PROGRAM_ID` (default [`DEFAULT_PROGRAM_ID`]).

pub mod accounts;
pub mod ix;
pub mod math;
#[cfg(test)]
mod tests;
pub mod tx;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

use crate::api::ApiError;
use crate::program::Pdas;
use crate::solana::rpc::RpcClient;
use crate::state::AppState;
use accounts::{AmmConfigAccount, MockTokenAccount, PoolAccount};

/// Program ID of `programs/mock-amm` (`declare_id!`, `programs/idl/mock_amm.json`).
pub const DEFAULT_PROGRAM_ID: &str = "5WhsDY2XjzJoWDbe1AgJnnNgJccQwte9TgsvG4yRtHbf";
/// Slippage the quote's `min_out_u` allows.
pub const SLIPPAGE_BPS: u16 = 50;
/// How long pool reads are reused.
pub const CACHE_TTL: Duration = Duration::from_secs(3);
/// The mock mints the desk looks for (`["mock", seed]` of the AMM), in `/tokens` order.
pub const MOCK_SYMBOLS: [&str; 2] = ["BONK", "SOL"];
/// `route` of every quote.
pub const ROUTE: &str = "mock-amm";

/// PDA seeds (`programs/mock-amm/src/constants.rs`).
pub mod seeds {
    /// `["amm_config"]`.
    pub const AMM_CONFIG: &[u8] = b"amm_config";
    /// `["mock", symbol]`: mock mints.
    pub const MOCK: &[u8] = b"mock";
    /// `["token", mint]`: `MockToken`.
    pub const TOKEN: &[u8] = b"token";
    /// `["pool", mint]`.
    pub const POOL: &[u8] = b"pool";
    /// `["vault", pool, mint]`.
    pub const VAULT: &[u8] = b"vault";
    /// `["faucet", mint, caller]`.
    pub const FAUCET: &[u8] = b"faucet";
}

/// Addresses of one mock-AMM deployment quoting against `usdc_mint`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmmPdas {
    /// Program id.
    pub program: Pubkey,
    /// Schedina's mock USDC (`Pdas::usdc_mint`).
    pub usdc_mint: Pubkey,
}

impl AmmPdas {
    /// The deployment at `program`, pools against schedina `sched`'s mock USDC.
    pub fn new(program: Pubkey, sched: &Pdas) -> Self {
        AmmPdas {
            program,
            usdc_mint: sched.usdc_mint(),
        }
    }

    fn find(&self, seeds: &[&[u8]]) -> Pubkey {
        Pubkey::find_program_address(seeds, &self.program).0
    }

    /// `AmmConfig`.
    pub fn amm_config(&self) -> Pubkey {
        self.find(&[seeds::AMM_CONFIG])
    }
    /// The mock mint of `symbol` (`"BONK"`).
    pub fn mock_mint(&self, symbol: &str) -> Pubkey {
        self.find(&[seeds::MOCK, symbol.as_bytes()])
    }
    /// `MockToken` of `mint`.
    pub fn mock_token(&self, mint: &Pubkey) -> Pubkey {
        self.find(&[seeds::TOKEN, mint.as_ref()])
    }
    /// `Pool` of `mint`.
    pub fn pool(&self, mint: &Pubkey) -> Pubkey {
        self.find(&[seeds::POOL, mint.as_ref()])
    }
    /// A pool's token account of `mint` (the mock mint or USDC).
    pub fn vault(&self, pool: &Pubkey, mint: &Pubkey) -> Pubkey {
        self.find(&[seeds::VAULT, pool.as_ref(), mint.as_ref()])
    }
    /// `FaucetClaim` of `mint` by signer `caller`.
    pub fn faucet_claim(&self, mint: &Pubkey, caller: &Pubkey) -> Pubkey {
        self.find(&[seeds::FAUCET, mint.as_ref(), caller.as_ref()])
    }
}

// ------------------------------------------------------------------------------- JSON (L4-3)

/// One `/tokens` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenInfo {
    /// Base58 mint.
    pub mint: String,
    /// `mockUSDC`, `mockBONK`, `mockSOL`.
    pub symbol: String,
    /// Mint decimals.
    pub decimals: u8,
    /// True for the stake currency (mock USDC), listed first.
    pub usdc_mint: bool,
}

/// `GET /tokens`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokensResponse {
    /// USDC first, then the swappable mints.
    pub tokens: Vec<TokenInfo>,
}

/// `POST /swap-quote` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapQuoteRequest {
    /// Base58 mint to sell.
    pub mint: String,
    /// Base units of `mint`, as a decimal string (u64).
    pub amount_in: String,
}

/// `POST /swap-quote` answer (Jupiter-like roles: out amount, fee, threshold, route).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapQuoteResponse {
    /// The mint sold.
    pub mint: String,
    /// Echo of the request, string.
    pub amount_in: String,
    /// Micro-USDC the pool pays at the read reserves.
    pub usdc_out_u: u64,
    /// The 0.5% pool fee in micro-USDC (output with no fee minus `usdc_out_u`).
    pub fee_u: u64,
    /// `usdc_out_u` less [`SLIPPAGE_BPS`], rounded down: the swap's `min_out` and the stake.
    pub min_out_u: u64,
    /// [`SLIPPAGE_BPS`].
    pub slippage_bps: u16,
    /// [`ROUTE`].
    pub route: String,
    /// Base58 `Pool` PDA.
    pub pool: String,
}

// ------------------------------------------------------------------------------- snapshot

/// One pool as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolSnap {
    /// `Pool` PDA.
    pub pool: Pubkey,
    /// The mock mint.
    pub mint: Pubkey,
    /// `mock` + seed symbol.
    pub symbol: String,
    /// Mint decimals.
    pub decimals: u8,
    /// Vault balance of the mock mint.
    pub r_mint: u64,
    /// Vault balance of USDC.
    pub r_usdc: u64,
    /// Pool fee.
    pub fee_bps: u16,
}

/// Every pool of the deployment, as of one read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Mock USDC.
    pub usdc_mint: Pubkey,
    /// Its decimals (6).
    pub usdc_decimals: u8,
    /// Usable pools in [`MOCK_SYMBOLS`] order.
    pub pools: Vec<PoolSnap>,
}

impl Snapshot {
    /// `GET /tokens`.
    pub fn tokens(&self) -> TokensResponse {
        let mut tokens = vec![TokenInfo {
            mint: self.usdc_mint.to_string(),
            symbol: "mockUSDC".into(),
            decimals: self.usdc_decimals,
            usdc_mint: true,
        }];
        tokens.extend(self.pools.iter().map(|p| TokenInfo {
            mint: p.mint.to_string(),
            symbol: p.symbol.clone(),
            decimals: p.decimals,
            usdc_mint: false,
        }));
        TokensResponse { tokens }
    }

    /// `POST /swap-quote`: 422 for an unknown mint, USDC itself, a bad amount, or an amount
    /// the pool would refuse (zero output).
    pub fn quote(&self, req: &SwapQuoteRequest) -> Result<SwapQuoteResponse, ApiError> {
        let mint: Pubkey = req
            .mint
            .parse()
            .map_err(|_| ApiError::rejected("mint is not a base58 address"))?;
        let amount_in: u64 = req
            .amount_in
            .parse()
            .map_err(|_| ApiError::rejected("amount_in must be a u64 decimal string"))?;
        if mint == self.usdc_mint {
            return Err(ApiError::rejected(
                "mock USDC is the stake currency: no swap",
            ));
        }
        let p = self
            .pools
            .iter()
            .find(|p| p.mint == mint)
            .ok_or_else(|| ApiError::rejected("no pool for this mint"))?;
        if amount_in == 0 {
            return Err(ApiError::rejected("amount_in must be positive"));
        }
        let s = math::swap_out(amount_in, p.r_mint, p.r_usdc, p.fee_bps)
            .ok_or_else(|| ApiError::rejected("amount too small for this pool"))?;
        Ok(SwapQuoteResponse {
            mint: mint.to_string(),
            amount_in: amount_in.to_string(),
            usdc_out_u: s.amount_out,
            fee_u: s.gross_out - s.amount_out,
            min_out_u: math::min_out(s.amount_out, SLIPPAGE_BPS),
            slippage_bps: SLIPPAGE_BPS,
            route: ROUTE.into(),
            pool: p.pool.to_string(),
        })
    }
}

/// Reads every pool over RPC (one `getMultipleAccounts`) and caches the result.
pub struct SwapService {
    rpc: RpcClient,
    pdas: AmmPdas,
    cache: Mutex<Option<(Instant, Arc<Snapshot>)>>,
}

impl SwapService {
    /// A service reading `pdas`'s accounts through `rpc`.
    pub fn new(rpc: RpcClient, pdas: AmmPdas) -> Self {
        SwapService {
            rpc,
            pdas,
            cache: Mutex::new(None),
        }
    }

    /// The deployment's addresses.
    pub fn pdas(&self) -> &AmmPdas {
        &self.pdas
    }

    /// The cached snapshot if younger than [`CACHE_TTL`], else a fresh read; 503 when the RPC
    /// fails.
    pub async fn snapshot(&self) -> Result<Arc<Snapshot>, ApiError> {
        if let Some((at, s)) = self.cache.lock().expect("swap cache").as_ref()
            && at.elapsed() < CACHE_TTL
        {
            return Ok(s.clone());
        }
        let s = Arc::new(self.read().await?);
        *self.cache.lock().expect("swap cache") = Some((Instant::now(), s.clone()));
        Ok(s)
    }

    async fn read(&self) -> Result<Snapshot, ApiError> {
        let p = &self.pdas;
        let mints: Vec<Pubkey> = MOCK_SYMBOLS.iter().map(|s| p.mock_mint(s)).collect();
        let mut keys = vec![p.amm_config(), p.usdc_mint];
        for m in &mints {
            let pool = p.pool(m);
            keys.extend([
                *m,
                p.mock_token(m),
                pool,
                p.vault(&pool, m),
                p.vault(&pool, &p.usdc_mint),
            ]);
        }
        let (_, accs) = self.rpc.get_multiple_accounts(&keys).await.map_err(|e| {
            tracing::warn!(error = %e, "swap: pool read failed");
            ApiError::unavailable("cannot read the swap pools")
        })?;
        snapshot_from(p, &mints, &accs)
    }
}

/// Builds the snapshot from `getMultipleAccounts` over `[amm_config, usdc_mint, then per mint:
/// mint, mock_token, pool, vault, vault_usdc]`. A pool that is missing, owned elsewhere, against
/// another USDC mint or with other vaults is left out; with no (or a foreign) `AmmConfig` only
/// mock USDC is listed.
pub fn snapshot_from(
    p: &AmmPdas,
    mints: &[Pubkey],
    accs: &[Option<crate::solana::rpc::RawAccount>],
) -> Result<Snapshot, ApiError> {
    let owned = |i: usize| {
        accs.get(i)
            .and_then(Option::as_ref)
            .filter(|a| a.owner == p.program)
    };
    let token = |i: usize| {
        accs.get(i)
            .and_then(Option::as_ref)
            .filter(|a| a.owner == crate::program::TOKEN_PROGRAM_ID)
    };
    let usdc_decimals = token(1)
        .and_then(|a| accounts::mint_decimals(&a.data).ok())
        .unwrap_or(crate::program::consts::USDC_DECIMALS);
    let mut pools = Vec::new();
    // No AMM on this cluster (live profile, or not set up yet), or one quoting another USDC
    // mint: only mock USDC is listed and every swap quote is refused.
    let cfg = owned(0).and_then(|a| AmmConfigAccount::decode(&a.data).ok());
    if cfg.is_none_or(|c| c.usdc_mint != p.usdc_mint) {
        return Ok(Snapshot {
            usdc_mint: p.usdc_mint,
            usdc_decimals,
            pools,
        });
    }
    for (j, m) in mints.iter().enumerate() {
        let b = 2 + 5 * j;
        let pool_key = p.pool(m);
        let Some(t) = owned(b + 1).and_then(|a| MockTokenAccount::decode(&a.data).ok()) else {
            continue;
        };
        let Some(pa) = owned(b + 2).and_then(|a| PoolAccount::decode(&a.data).ok()) else {
            continue;
        };
        let sane = pa.mint == *m
            && t.mint == *m
            && pa.usdc_mint == p.usdc_mint
            && pa.vault == p.vault(&pool_key, m)
            && pa.vault_usdc == p.vault(&pool_key, &p.usdc_mint);
        let vm = token(b + 3).and_then(|a| accounts::token_account(&a.data).ok());
        let vu = token(b + 4).and_then(|a| accounts::token_account(&a.data).ok());
        let (Some((vm_mint, r_mint)), Some((vu_mint, r_usdc))) = (vm, vu) else {
            continue;
        };
        if !sane || vm_mint != *m || vu_mint != p.usdc_mint {
            tracing::warn!(pool = %pool_key, "swap: pool accounts inconsistent; skipped");
            continue;
        }
        let decimals = token(b)
            .and_then(|a| accounts::mint_decimals(&a.data).ok())
            .unwrap_or(t.decimals);
        pools.push(PoolSnap {
            pool: pool_key,
            mint: *m,
            symbol: format!("mock{}", t.symbol),
            decimals,
            r_mint,
            r_usdc,
            fee_bps: pa.fee_bps,
        });
    }
    Ok(Snapshot {
        usdc_mint: p.usdc_mint,
        usdc_decimals,
        pools,
    })
}

/// The mock-AMM program id: `MOCK_AMM_PROGRAM_ID` if set and valid, else
/// [`DEFAULT_PROGRAM_ID`].
pub fn program_id_from_env() -> Pubkey {
    let default = Pubkey::from_str_const(DEFAULT_PROGRAM_ID);
    match std::env::var("MOCK_AMM_PROGRAM_ID") {
        Ok(s) if !s.trim().is_empty() => s.trim().parse().unwrap_or_else(|_| {
            tracing::warn!("MOCK_AMM_PROGRAM_ID is not a base58 address; using the default");
            default
        }),
        _ => default,
    }
}

/// Adds `GET /tokens` and `POST /swap-quote`.
pub fn mount(router: Router<AppState>, state: &AppState) -> Router<AppState> {
    let pdas = AmmPdas::new(program_id_from_env(), &Pdas::new(state.config.program_id));
    let svc = Arc::new(SwapService::new(
        RpcClient::new(state.config.solana_rpc_url.clone()),
        pdas,
    ));
    let tokens_svc = svc.clone();
    router
        .route(
            "/tokens",
            get(move || {
                let svc = tokens_svc.clone();
                async move { svc.snapshot().await.map(|s| Json(s.tokens())) }
            }),
        )
        .route(
            "/swap-quote",
            post(move |body: axum::body::Bytes| {
                let svc = svc.clone();
                async move {
                    // Parsed here so a malformed body gets the C4 `rejected` body, not axum's.
                    let req: SwapQuoteRequest = serde_json::from_slice(&body).map_err(|_| {
                        ApiError::rejected(r#"body must be {"mint":"…","amount_in":"<u64>"}"#)
                    })?;
                    svc.snapshot().await?.quote(&req).map(Json)
                }
            }),
        )
}
