//! Desk configuration from the environment (the names in `.env.example`, plus `DESK_LISTEN`).
//!
//! Nothing here reads key material: key paths are resolved (defaults under
//! `~/.config/schedina/`, D-015) and the services that sign read the files when they need them.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use solana_pubkey::Pubkey;

/// Production Pascal read API.
pub const PASCAL_DATA_URL_LIVE: &str = "https://data.pascal.trade";
/// Production Pascal market-data socket.
pub const PASCAL_WS_URL_LIVE: &str = "wss://data.pascal.trade/ws";
/// Schedina program id (D-014), used when `PROGRAM_ID` is empty.
pub const DEFAULT_PROGRAM_ID: &str = "HXdZFi7tR669G1hXogh3uGGqLYsoZPCSuiutAviLNToW";
/// Default listen address when `DESK_LISTEN` is empty.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8787";
/// Default database when `DESK_DB` is empty (under the gitignored `target/`).
pub const DEFAULT_DB: &str = "target/desk.sqlite";

/// Run profile (spec section 12, "Environments").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Localnet, mock `synthetic`.
    Dev,
    /// Devnet, mock `replay`.
    Demo,
    /// Devnet, real Pascal reads (orders to Pascal only in mode R, D-007).
    Live,
}

impl Profile {
    /// Lower-case name as written in `PROFILE`.
    pub fn as_str(self) -> &'static str {
        match self {
            Profile::Dev => "dev",
            Profile::Demo => "demo",
            Profile::Live => "live",
        }
    }
}

impl FromStr for Profile {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, ConfigError> {
        match s {
            "" | "dev" => Ok(Profile::Dev),
            "demo" => Ok(Profile::Demo),
            "live" => Ok(Profile::Live),
            other => Err(ConfigError::Invalid(
                "PROFILE",
                format!("{other:?} (expected dev, demo or live)"),
            )),
        }
    }
}

/// A configuration error, naming the variable.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A variable is set to something unusable.
    #[error("{0}: {1}")]
    Invalid(&'static str, String),
    /// A variable the profile needs is empty.
    #[error("{0} must be set when PROFILE={1}")]
    Missing(&'static str, &'static str),
}

/// Desk configuration. Fields are public so tests can build one directly.
#[derive(Debug, Clone)]
pub struct Config {
    /// `PROFILE`.
    pub profile: Profile,
    /// `DESK_LISTEN`: HTTP/WebSocket listen address.
    pub listen: SocketAddr,
    /// `DESK_DB`: SQLite file path (or `sqlite::memory:`).
    pub db_path: String,
    /// `PASCAL_DATA_URL`: Pascal read API, or the mock on `dev`/`demo`.
    pub pascal_data_url: String,
    /// `PASCAL_TRADE_URL`: signed order placement (the mock on `dev`/`demo`); used from 2.B.
    pub pascal_trade_url: Option<String>,
    /// `PASCAL_WS_URL`: market-data socket.
    pub pascal_ws_url: String,
    /// `PASCAL_HEDGER_OWNER`: the hedger account's owner pubkey (base58).
    pub hedger_owner: Option<String>,
    /// `PASCAL_HEDGER_TRADING_KEY_PATH`.
    pub hedger_trading_key_path: Option<PathBuf>,
    /// `PASCAL_BUILDER_PUBKEY`: builder account written into every permit (D-003).
    pub builder_pubkey: Option<String>,
    /// `SOLANA_RPC_URL` (never mainnet, D-016). May carry an API key: log it with
    /// [`crate::solana::rpc::redact`].
    pub solana_rpc_url: String,
    /// `PROGRAM_ID`.
    pub program_id: Pubkey,
    /// `CRANK_KEYPAIR_PATH` (default `<keys>/crank.json`).
    pub crank_keypair_path: PathBuf,
    /// `QUOTE_SIGNER_KEYPAIR_PATH` (default `<keys>/quote-signer.json`).
    pub quote_signer_keypair_path: PathBuf,
    /// `AUTHORITY_KEYPAIR_PATH` (default `<keys>/upgrade-authority.json`); crank CLI only.
    pub authority_keypair_path: PathBuf,
    /// Catalogue refresh period (60 s; never under 30 s, D-004).
    pub catalogue_every: Duration,
    /// `/stream` heartbeat period (at most 10 s, C4).
    pub heartbeat_every: Duration,
    /// `/card` answers 503 when the last good catalogue is older than this.
    pub card_max_age: Duration,
    /// Capacity of the [`crate::state::DeskEvent`] broadcast bus.
    pub bus_capacity: usize,
    /// How long shutdown waits for the services before giving up on them.
    pub shutdown_grace: Duration,
    /// `DESK_CHAIN` (2.C): run the indexer's listener and the crank's sends (default on; off
    /// in [`Config::local`], so desk tests without a validator stay offline).
    pub chain_enabled: bool,
    /// `SOLANA_WS_URL` (2.C): PubSub endpoint; default derived from `SOLANA_RPC_URL`.
    pub solana_ws_url: Option<String>,
    /// `CRANK_RESOLVE_POLL_S` (2.C): `POST /markets/search` period per open leg (60 s, D-012).
    pub resolve_poll_every: Duration,
    /// `CRANK_RESOLVED_SWEEP_S` (2.C): `/markets/resolved` backstop sweep (5 min, D-012).
    pub resolved_sweep_every: Duration,
    /// `CRANK_RESOLVE_AFTER_S` (2.C): polling starts at `start + this` (2.5 h, D-012).
    pub resolve_after: Duration,
    /// `CRANK_TICK_MS` (2.C): refund timer and retry tick (1 s).
    pub crank_tick: Duration,
    /// `INDEXER_RESYNC_S` (2.C): full chain resync period of the listener (60 s).
    pub chain_resync_every: Duration,
    /// `DESK_COPY_QUOTE` (`1`/`true`): serve `POST /ticket/:id/copy-quote` and `/quote` with
    /// `copied_from` (v1, plan 2.A / 2.F). Unset: on for `dev` and `demo`, off for `live`
    /// (l4-contracts L4-2); `0`/`false` turns it off.
    pub copy_quote: bool,
    /// `DESK_SENTIMENT_S` (5.0c): how often the sentiment task follows `/api/v1/trades` (15 s,
    /// floored at [`MIN_SENTIMENT_EVERY`]); `0` turns it off.
    pub sentiment_every: Duration,
    /// `DESK_FX_URL` (6.0c): where the daily EUR/USD rate is read. Unset: [`FX_URL_DEFAULT`]
    /// (frankfurter's ECB rate) on `demo` and `live`, empty on `dev` (localnet and the mock only,
    /// L3-1) and in [`Config::local`]; empty = never fetched, `/fx` serves the fallback.
    pub fx_url: String,
    /// `DESK_FX_S` (6.0c): how often the rate is fetched (86 400 s); `0` turns fetching off.
    pub fx_every: Duration,
    /// `DESK_FX_FALLBACK_PPM` (6.0c): EUR per USD × 1e6 served when no fetched rate is fresh.
    pub fx_fallback_ppm: u64,
    /// `DESK_FX_FALLBACK_AS_OF` (6.0c): the fallback rate's date, `YYYY-MM-DD`.
    pub fx_fallback_as_of: String,
    /// `DESK_SOL_URL` (7.0c): where the SOL/USD price is read (Coinbase spot, or Hermes). Unset:
    /// [`crate::fx::sol::COINBASE_URL_DEFAULT`] on `demo` and `live`, nothing on `dev` and in
    /// [`Config::local`]; empty = never fetched, `/fx` serves the SOL fallback.
    pub sol_url: String,
    /// `DESK_SOL_S` (7.0c): how often the SOL price is read (60 s); `0` turns it off.
    pub sol_every: Duration,
    /// `DESK_SOL_FALLBACK_USD_U` (7.0c): micro-USD per SOL served when no fresh price is cached.
    pub sol_fallback_usd_u: u64,
    /// `DESK_SOL_API_KEY` (7.0c): optional bearer token (Hermes needs one); never printed.
    pub sol_api_key: Option<crate::fx::sol::Secret>,
    /// `DESK_SCORES_URL` (31 Oct): where live scores are read (Polymarket's Gamma API). Unset:
    /// [`crate::scores::GAMMA_URL_DEFAULT`] on `demo` and `live`, nothing on `dev` and in
    /// [`Config::local`]; empty = never fetched.
    pub scores_url: String,
    /// `DESK_SCORES_S` (31 Oct): how often live scores are read (20 s); `0` turns it off.
    pub scores_every: Duration,
    /// `SPONSOR_KEYPAIR_PATH` (L8, default `<keys>/sponsor.json`): the key that pays fees and rent
    /// of the transactions `POST /relay` co-signs. The relay is off when the file is missing.
    pub sponsor_keypair_path: PathBuf,
    /// `DESK_RELAY` (L8): `0`/`false` turns the sponsored-transaction relay off; on by default
    /// (it still needs the sponsor key).
    pub relay_enabled: bool,
    /// `DESK_RELAY_DAILY_LAMPORTS` (L8): the most simulated sponsor spend the relay allows per
    /// UTC day (default [`RELAY_DAILY_LAMPORTS`], 2 SOL).
    pub relay_daily_lamports: u64,
}

/// Default `DESK_RELAY_DAILY_LAMPORTS`: 2 SOL of sponsor spend per UTC day.
pub const RELAY_DAILY_LAMPORTS: u64 = 2_000_000_000;

/// Default `DESK_FX_URL`: frankfurter's v2 rates, ECB reference rate, USD → EUR. Its answer is
/// `[{"date","base","quote","rate"}]`; the v1 shape (`/v1/latest?from=USD&to=EUR`, deprecated
/// upstream, where `api.frankfurter.app` now redirects) is read too.
pub const FX_URL_DEFAULT: &str =
    "https://api.frankfurter.dev/v2/rates?base=USD&quotes=EUR&providers=ECB";
/// Default fallback: the ECB rate of 29 Oct 2026 (0.89847 EUR per USD).
pub const FX_FALLBACK_PPM: u64 = 898_470;
/// Date of [`FX_FALLBACK_PPM`].
pub const FX_FALLBACK_AS_OF: &str = "2026-10-29";

/// Floor of the catalogue period (D-004: `/markets` at most once per 30 s).
pub const MIN_CATALOGUE_EVERY: Duration = Duration::from_secs(30);
/// Floor of the sentiment period (D-250).
pub const MIN_SENTIMENT_EVERY: Duration = Duration::from_secs(5);

impl Config {
    /// Reads the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Reads configuration through `get` (tests pass a map). Empty values count as unset.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let var = |k: &str| {
            get(k)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let profile: Profile = var("PROFILE").unwrap_or_default().parse()?;
        let listen = var("DESK_LISTEN")
            .unwrap_or_else(|| DEFAULT_LISTEN.to_owned())
            .parse()
            .map_err(|e| ConfigError::Invalid("DESK_LISTEN", format!("{e}")))?;
        let pascal_data_url = match (var("PASCAL_DATA_URL"), profile) {
            (Some(u), _) => u,
            (None, Profile::Live) => PASCAL_DATA_URL_LIVE.to_owned(),
            // dev/demo read the mock; defaulting to production would hide a missing mock.
            (None, p) => return Err(ConfigError::Missing("PASCAL_DATA_URL", p.as_str())),
        };
        check_http_url("PASCAL_DATA_URL", &pascal_data_url)?;
        let pascal_ws_url = match (var("PASCAL_WS_URL"), profile) {
            (Some(u), _) => u,
            (None, Profile::Live) => PASCAL_WS_URL_LIVE.to_owned(),
            // The mock serves its socket next to its REST API.
            (None, _) => format!(
                "{}/ws",
                pascal_data_url
                    .trim_end_matches('/')
                    .replacen("http", "ws", 1)
            ),
        };
        let solana_rpc_url = var("SOLANA_RPC_URL").unwrap_or_else(|| match profile {
            Profile::Dev => "http://127.0.0.1:8899".to_owned(),
            Profile::Demo | Profile::Live => "https://api.devnet.solana.com".to_owned(),
        });
        check_http_url("SOLANA_RPC_URL", &solana_rpc_url)?;
        if solana_rpc_url.to_ascii_lowercase().contains("mainnet") {
            return Err(ConfigError::Invalid(
                "SOLANA_RPC_URL",
                "mainnet endpoints are refused (D-016)".into(),
            ));
        }
        let program_id = var("PROGRAM_ID")
            .unwrap_or_else(|| DEFAULT_PROGRAM_ID.to_owned())
            .parse()
            .map_err(|e| ConfigError::Invalid("PROGRAM_ID", format!("{e}")))?;
        let keys_dir = var("SCHEDINA_KEYS_DIR").map_or_else(
            || {
                var("HOME")
                    .map_or_else(|| PathBuf::from("."), PathBuf::from)
                    .join(".config/schedina")
            },
            PathBuf::from,
        );
        let key = |name: &'static str, default: &str| {
            var(name).map_or_else(|| keys_dir.join(default), PathBuf::from)
        };
        let secs = |name: &'static str, default: u64| -> Result<Duration, ConfigError> {
            var(name)
                .map(|v| v.parse::<u64>())
                .transpose()
                .map_err(|e| ConfigError::Invalid(name, format!("{e}")))
                .map(|v| Duration::from_secs(v.unwrap_or(default)))
        };
        // `dev` stays on localnet and the mock (L3-1): no outbound read unless asked for.
        let fx_url = match (var("DESK_FX_URL"), profile) {
            (Some(u), _) => {
                check_http_url("DESK_FX_URL", &u)?;
                u
            }
            (None, Profile::Dev) => String::new(),
            (None, _) => FX_URL_DEFAULT.to_owned(),
        };
        let sol_url = match (var("DESK_SOL_URL"), profile) {
            (Some(u), _) => {
                check_http_url("DESK_SOL_URL", &u)?;
                u
            }
            (None, Profile::Dev) => String::new(),
            (None, _) => crate::fx::sol::COINBASE_URL_DEFAULT.to_owned(),
        };
        let scores_url = match (var("DESK_SCORES_URL"), profile) {
            (Some(u), _) => {
                check_http_url("DESK_SCORES_URL", &u)?;
                u
            }
            (None, Profile::Dev) => String::new(),
            (None, _) => crate::scores::GAMMA_URL_DEFAULT.to_owned(),
        };
        Ok(Config {
            profile,
            listen,
            db_path: var("DESK_DB").unwrap_or_else(|| DEFAULT_DB.to_owned()),
            pascal_data_url,
            pascal_trade_url: var("PASCAL_TRADE_URL"),
            pascal_ws_url,
            hedger_owner: var("PASCAL_HEDGER_OWNER"),
            hedger_trading_key_path: var("PASCAL_HEDGER_TRADING_KEY_PATH").map(PathBuf::from),
            builder_pubkey: var("PASCAL_BUILDER_PUBKEY"),
            solana_rpc_url,
            program_id,
            crank_keypair_path: key("CRANK_KEYPAIR_PATH", "crank.json"),
            quote_signer_keypair_path: key("QUOTE_SIGNER_KEYPAIR_PATH", "quote-signer.json"),
            authority_keypair_path: key("AUTHORITY_KEYPAIR_PATH", "upgrade-authority.json"),
            chain_enabled: var("DESK_CHAIN").is_none_or(|v| v != "0" && v != "false"),
            solana_ws_url: var("SOLANA_WS_URL"),
            resolve_poll_every: secs("CRANK_RESOLVE_POLL_S", 60)?,
            resolved_sweep_every: secs("CRANK_RESOLVED_SWEEP_S", 300)?,
            resolve_after: secs("CRANK_RESOLVE_AFTER_S", 9_000)?,
            crank_tick: Duration::from_millis(
                var("CRANK_TICK_MS")
                    .map(|v| v.parse::<u64>())
                    .transpose()
                    .map_err(|e| ConfigError::Invalid("CRANK_TICK_MS", format!("{e}")))?
                    .unwrap_or(1_000),
            ),
            chain_resync_every: secs("INDEXER_RESYNC_S", 60)?,
            copy_quote: var("DESK_COPY_QUOTE")
                .map_or(profile != Profile::Live, |v| v == "1" || v == "true"),
            sentiment_every: secs("DESK_SENTIMENT_S", 15)?,
            fx_url,
            fx_every: secs("DESK_FX_S", 86_400)?,
            fx_fallback_ppm: var("DESK_FX_FALLBACK_PPM")
                .map(|v| v.parse::<u64>())
                .transpose()
                .map_err(|e| ConfigError::Invalid("DESK_FX_FALLBACK_PPM", format!("{e}")))?
                .unwrap_or(FX_FALLBACK_PPM),
            fx_fallback_as_of: var("DESK_FX_FALLBACK_AS_OF")
                .unwrap_or_else(|| FX_FALLBACK_AS_OF.to_owned()),
            sol_url,
            sol_every: secs("DESK_SOL_S", 60)?,
            sol_fallback_usd_u: var("DESK_SOL_FALLBACK_USD_U")
                .map(|v| v.parse::<u64>())
                .transpose()
                .map_err(|e| ConfigError::Invalid("DESK_SOL_FALLBACK_USD_U", format!("{e}")))?
                .unwrap_or(crate::fx::sol::FALLBACK_USD_U),
            sol_api_key: var("DESK_SOL_API_KEY").map(crate::fx::sol::Secret),
            scores_url,
            scores_every: secs("DESK_SCORES_S", 20)?,
            sponsor_keypair_path: key("SPONSOR_KEYPAIR_PATH", "sponsor.json"),
            relay_enabled: var("DESK_RELAY").is_none_or(|v| v != "0" && v != "false"),
            relay_daily_lamports: var("DESK_RELAY_DAILY_LAMPORTS")
                .map(|v| v.parse::<u64>())
                .transpose()
                .map_err(|e| ConfigError::Invalid("DESK_RELAY_DAILY_LAMPORTS", format!("{e}")))?
                .unwrap_or(RELAY_DAILY_LAMPORTS),
            ..Config::defaults(profile, listen)
        })
    }

    /// A configuration with every URL on localhost and the default periods; tests start from it.
    pub fn local(pascal_data_url: impl Into<String>, db_path: impl Into<String>) -> Self {
        let pascal_data_url = pascal_data_url.into();
        Config {
            pascal_ws_url: format!("{}/ws", pascal_data_url.replacen("http", "ws", 1)),
            pascal_data_url,
            db_path: db_path.into(),
            ..Config::defaults(Profile::Dev, SocketAddr::from(([127, 0, 0, 1], 0)))
        }
    }

    fn defaults(profile: Profile, listen: SocketAddr) -> Self {
        let keys = PathBuf::from(".config/schedina");
        Config {
            profile,
            listen,
            db_path: DEFAULT_DB.to_owned(),
            pascal_data_url: String::new(),
            pascal_trade_url: None,
            pascal_ws_url: String::new(),
            hedger_owner: None,
            hedger_trading_key_path: None,
            builder_pubkey: None,
            solana_rpc_url: "http://127.0.0.1:8899".to_owned(),
            program_id: DEFAULT_PROGRAM_ID.parse().unwrap_or_default(),
            crank_keypair_path: keys.join("crank.json"),
            quote_signer_keypair_path: keys.join("quote-signer.json"),
            authority_keypair_path: keys.join("upgrade-authority.json"),
            catalogue_every: Duration::from_secs(60),
            heartbeat_every: Duration::from_secs(5),
            card_max_age: Duration::from_secs(10 * 60),
            bus_capacity: 4096,
            shutdown_grace: Duration::from_secs(10),
            chain_enabled: false,
            solana_ws_url: None,
            resolve_poll_every: Duration::from_secs(60),
            resolved_sweep_every: Duration::from_secs(300),
            resolve_after: Duration::from_secs(9_000),
            crank_tick: Duration::from_secs(1),
            chain_resync_every: Duration::from_secs(60),
            copy_quote: false,
            sentiment_every: Duration::from_secs(15),
            fx_url: String::new(),
            fx_every: Duration::from_secs(86_400),
            fx_fallback_ppm: FX_FALLBACK_PPM,
            fx_fallback_as_of: FX_FALLBACK_AS_OF.to_owned(),
            sol_url: String::new(),
            sol_every: Duration::from_secs(60),
            sol_fallback_usd_u: crate::fx::sol::FALLBACK_USD_U,
            sol_api_key: None,
            scores_url: String::new(),
            scores_every: Duration::from_secs(20),
            sponsor_keypair_path: keys.join("sponsor.json"),
            relay_enabled: true,
            relay_daily_lamports: RELAY_DAILY_LAMPORTS,
        }
    }

    /// The catalogue period, floored at [`MIN_CATALOGUE_EVERY`].
    pub fn catalogue_period(&self) -> Duration {
        self.catalogue_every.max(MIN_CATALOGUE_EVERY)
    }

    /// The sentiment period, floored at [`MIN_SENTIMENT_EVERY`] (`Duration::ZERO` = off, checked
    /// by the task before it asks).
    pub fn sentiment_period(&self) -> Duration {
        self.sentiment_every.max(MIN_SENTIMENT_EVERY)
    }

    /// The heartbeat period, capped at 10 s (C4).
    pub fn heartbeat_period(&self) -> Duration {
        self.heartbeat_every
            .clamp(Duration::from_millis(10), Duration::from_secs(10))
    }
}

fn check_http_url(name: &'static str, url: &str) -> Result<(), ConfigError> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(())
    } else {
        Err(ConfigError::Invalid(
            name,
            "expected an http:// or https:// URL".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn cfg(pairs: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Config::from_lookup(|k| map.get(k).cloned())
    }

    #[test]
    fn dev_needs_the_mock_url() {
        assert!(matches!(
            cfg(&[]),
            Err(ConfigError::Missing("PASCAL_DATA_URL", "dev"))
        ));
        let c = cfg(&[("PASCAL_DATA_URL", "http://127.0.0.1:9000")]).unwrap();
        assert_eq!(c.profile, Profile::Dev);
        assert_eq!(c.pascal_ws_url, "ws://127.0.0.1:9000/ws");
        assert_eq!(c.solana_rpc_url, "http://127.0.0.1:8899");
        assert_eq!(c.program_id.to_string(), DEFAULT_PROGRAM_ID);
        assert_eq!(c.listen.to_string(), DEFAULT_LISTEN);
    }

    #[test]
    fn live_defaults_to_pascal_and_devnet() {
        let c = cfg(&[("PROFILE", "live"), ("HOME", "/h")]).unwrap();
        assert_eq!(c.pascal_data_url, PASCAL_DATA_URL_LIVE);
        assert_eq!(c.pascal_ws_url, PASCAL_WS_URL_LIVE);
        assert_eq!(c.solana_rpc_url, "https://api.devnet.solana.com");
        assert_eq!(
            c.crank_keypair_path,
            PathBuf::from("/h/.config/schedina/crank.json")
        );
    }

    #[test]
    fn refuses_mainnet_and_bad_values() {
        let base = ("PASCAL_DATA_URL", "http://127.0.0.1:9000");
        assert!(
            cfg(&[
                base,
                ("SOLANA_RPC_URL", "https://api.mainnet-beta.solana.com")
            ])
            .is_err()
        );
        assert!(cfg(&[base, ("PROFILE", "prod")]).is_err());
        assert!(cfg(&[base, ("PROGRAM_ID", "nope")]).is_err());
        assert!(cfg(&[("PASCAL_DATA_URL", "127.0.0.1:9000")]).is_err());
    }

    #[test]
    fn copy_quote_is_on_by_default_off_live() {
        let base = ("PASCAL_DATA_URL", "http://127.0.0.1:9000");
        assert!(cfg(&[base]).unwrap().copy_quote);
        assert!(cfg(&[base, ("PROFILE", "demo")]).unwrap().copy_quote);
        assert!(!cfg(&[("PROFILE", "live")]).unwrap().copy_quote);
        assert!(!cfg(&[base, ("DESK_COPY_QUOTE", "0")]).unwrap().copy_quote);
        assert!(
            cfg(&[("PROFILE", "live"), ("DESK_COPY_QUOTE", "1")])
                .unwrap()
                .copy_quote
        );
    }

    #[test]
    fn sol_defaults_and_overrides() {
        let base = ("PASCAL_DATA_URL", "http://127.0.0.1:9000");
        let c = cfg(&[base]).unwrap();
        assert!(c.sol_url.is_empty());
        assert_eq!(c.sol_every, Duration::from_secs(60));
        assert_eq!(c.sol_fallback_usd_u, crate::fx::sol::FALLBACK_USD_U);
        assert!(c.sol_api_key.is_none());
        assert_eq!(
            cfg(&[("PROFILE", "live")]).unwrap().sol_url,
            crate::fx::sol::COINBASE_URL_DEFAULT
        );
        let c = cfg(&[
            base,
            ("DESK_SOL_URL", "http://127.0.0.1:5/hermes"),
            ("DESK_SOL_S", "0"),
            ("DESK_SOL_FALLBACK_USD_U", "150000000"),
            ("DESK_SOL_API_KEY", "sekrit"),
        ])
        .unwrap();
        assert_eq!(c.sol_url, "http://127.0.0.1:5/hermes");
        assert!(c.sol_every.is_zero());
        assert_eq!(c.sol_fallback_usd_u, 150_000_000);
        assert!(!format!("{c:?}").contains("sekrit"));
        assert!(cfg(&[base, ("DESK_SOL_URL", "hermes")]).is_err());
        assert!(cfg(&[base, ("DESK_SOL_FALLBACK_USD_U", "151.2")]).is_err());
    }

    #[test]
    fn fx_defaults_and_overrides() {
        let base = ("PASCAL_DATA_URL", "http://127.0.0.1:9000");
        // dev reads nothing outside localnet and the mock unless asked; demo and live read ECB.
        let c = cfg(&[base]).unwrap();
        assert!(c.fx_url.is_empty());
        assert_eq!(c.fx_every, Duration::from_secs(86_400));
        assert_eq!(
            cfg(&[base, ("PROFILE", "demo")]).unwrap().fx_url,
            FX_URL_DEFAULT
        );
        assert_eq!(cfg(&[("PROFILE", "live")]).unwrap().fx_url, FX_URL_DEFAULT);
        assert_eq!(
            cfg(&[base, ("DESK_FX_URL", "http://127.0.0.1:5/v2/rates")])
                .unwrap()
                .fx_url,
            "http://127.0.0.1:5/v2/rates"
        );
        assert_eq!(
            (c.fx_fallback_ppm, c.fx_fallback_as_of.as_str()),
            (898_470, "2026-10-29")
        );
        let c = cfg(&[base, ("DESK_FX_S", "0"), ("DESK_FX_FALLBACK_PPM", "920000")]).unwrap();
        assert!(c.fx_every.is_zero());
        assert_eq!(c.fx_fallback_ppm, 920_000);
        assert!(cfg(&[base, ("DESK_FX_URL", "ftp://x")]).is_err());
        assert!(cfg(&[base, ("DESK_FX_FALLBACK_PPM", "0.92")]).is_err());
        assert!(
            Config::local("http://127.0.0.1:1", "sqlite::memory:")
                .fx_url
                .is_empty()
        );
    }

    #[test]
    fn relay_defaults_and_overrides() {
        let base = ("PASCAL_DATA_URL", "http://127.0.0.1:9000");
        let c = cfg(&[base, ("SCHEDINA_KEYS_DIR", "/k")]).unwrap();
        assert_eq!(c.sponsor_keypair_path, PathBuf::from("/k/sponsor.json"));
        assert!(c.relay_enabled);
        assert_eq!(c.relay_daily_lamports, RELAY_DAILY_LAMPORTS);
        let c = cfg(&[
            base,
            ("SPONSOR_KEYPAIR_PATH", "/s.json"),
            ("DESK_RELAY", "0"),
            ("DESK_RELAY_DAILY_LAMPORTS", "500000000"),
        ])
        .unwrap();
        assert_eq!(c.sponsor_keypair_path, PathBuf::from("/s.json"));
        assert!(!c.relay_enabled);
        assert_eq!(c.relay_daily_lamports, 500_000_000);
        assert!(cfg(&[base, ("DESK_RELAY_DAILY_LAMPORTS", "2 SOL")]).is_err());
    }

    #[test]
    fn periods_respect_the_limits() {
        let mut c = Config::local("http://127.0.0.1:1", "sqlite::memory:");
        c.catalogue_every = Duration::from_secs(1);
        assert_eq!(c.catalogue_period(), MIN_CATALOGUE_EVERY);
        c.heartbeat_every = Duration::from_secs(60);
        assert_eq!(c.heartbeat_period(), Duration::from_secs(10));
        c.sentiment_every = Duration::from_millis(10);
        assert_eq!(c.sentiment_period(), MIN_SENTIMENT_EVERY);
    }
}
