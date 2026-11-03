//! Pascal mock: replays a recorded tape and matches orders behind Pascal's REST surface
//! (spec section 9, plan steps 1.D and 2.D).
//!
//! - [`tape`]: loads `tapes/<night>/` (`books`, `trades`, `search`, `markets` `.jsonl` files).
//! - [`clock`]: tape time = start + wall elapsed × speed.
//! - [`engine`]: books at the clock, the matcher, accounts, resolutions.
//! - [`requests`]: write-API bodies and the [`PermitVerifier`] seam.
//! - [`verify`]: [`SignatureVerifier`], `pascal-rs`'s permit check plus the trading-key
//!   registry (2.D, L2-5).
//! - [`events`]: the internal bus the `/ws` channels read.
//! - [`api`]: the axum routes, Pascal's envelopes and codes, error injection, `/mock/*`.
//! - [`ws`]: `GET /ws` with the `account`, `market_update` and `book` channels.
//! - [`script`]: `script.toml` scripted resolutions.
//! - [`snapshot`]: `--account-snapshot`, booting an account from a saved Pascal state.
//! - [`live`]: live mode (L4-1, [`Mock::new_live`]): Pascal's real books and results from the
//!   public read API and socket, the mock's own matcher for every order; [`feed`] holds the
//!   upstream data in the engine, [`record`] writes a live session as a tape (`--record`).
//!
//! ```no_run
//! # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
//! let mut cfg = pascal_mock::MockConfig::new("tapes/nba-2026-10-28/sample");
//! cfg.speed = 0.0; // frozen clock
//! let running = pascal_mock::Mock::new(cfg)?.spawn("127.0.0.1:0").await?;
//! let client = pascal_rs::Client::builder().base_url(running.base_url()).build()?;
//! let books = client.books(&["NBA_26OCT28_PHIIND_ML.PHI"]).await?;
//! # Ok(()) }
//! ```

pub mod api;
pub mod clock;
pub mod engine;
pub mod events;
pub mod feed;
pub mod live;
pub mod math;
pub mod record;
pub mod requests;
pub mod script;
pub mod snapshot;
pub mod tape;
mod util;
pub mod verify;
pub mod ws;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

pub use api::{EngineGuard, Shared};
pub use engine::{Engine, EngineConfig, PlaceResult, tape_owner};
pub use events::MockEvent;
pub use live::{LiveConfig, LiveLink};
pub use requests::{AcceptAll, PermitVerifier};
pub use script::{At, Script};
pub use snapshot::AccountSnapshot;
pub use tape::Tape;
pub use verify::{SignatureVerifier, TradingKeys};

/// Default free collateral of a new mock account: 10,000 USD.
pub const DEFAULT_COLLATERAL_U: i64 = 10_000 * 1_000_000;

/// Where the scripted resolutions come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptSource {
    /// `script.toml` in the tape directory or the directory above it, if present.
    Auto,
    /// This file.
    File(PathBuf),
    /// No script.
    None,
}

/// Mock settings.
#[derive(Debug, Clone)]
pub struct MockConfig {
    /// Tape directory (`tapes/<night>`, or its `raw/` / `sample/`).
    pub tape_dir: PathBuf,
    /// Tape ms per wall ms; 0 freezes the clock. Default 1.
    pub speed: f64,
    /// Clock start, Unix ms; default the tape's first books snapshot.
    pub start_ms: Option<u64>,
    /// Scripted resolutions. Default [`ScriptSource::Auto`].
    pub script: ScriptSource,
    /// Free collateral of each new account, micro-USD. Default 10,000 USD.
    pub initial_collateral_u: i64,
    /// Verify every write's permit with `pascal-rs` ([`SignatureVerifier`]). Default `false`
    /// here so in-process tests may send unsigned requests; the binary defaults to `true`.
    /// Implied by a non-empty [`MockConfig::trading_keys`].
    pub verify_permits: bool,
    /// Registered trading keys (`--trading-key <owner>=<pubkey>`). Empty = any key whose
    /// signature verifies (L2-5).
    pub trading_keys: TradingKeys,
    /// Accounts to boot from saved Pascal states (`--account-snapshot`).
    pub account_snapshots: Vec<AccountSnapshot>,
    /// `/ws` closes a socket with no client message for this long. Default 60 s (Pascal).
    pub ws_idle_timeout: Duration,
}

impl MockConfig {
    /// Defaults for a tape directory.
    pub fn new(tape_dir: impl Into<PathBuf>) -> Self {
        MockConfig {
            tape_dir: tape_dir.into(),
            speed: 1.0,
            start_ms: None,
            script: ScriptSource::Auto,
            initial_collateral_u: DEFAULT_COLLATERAL_U,
            verify_permits: false,
            trading_keys: TradingKeys::new(),
            account_snapshots: Vec::new(),
            ws_idle_timeout: api::DEFAULT_WS_IDLE_TIMEOUT,
        }
    }

    /// Whether [`Mock::new`] verifies permits.
    pub fn verifies(&self) -> bool {
        self.verify_permits || !self.trading_keys.is_empty()
    }
}

/// Why the mock could not start.
#[derive(Debug, thiserror::Error)]
pub enum MockError {
    /// Tape loading failed.
    #[error(transparent)]
    Tape(#[from] tape::TapeError),
    /// Script loading failed.
    #[error(transparent)]
    Script(#[from] script::ScriptError),
    /// Live mode could not start (bad upstream, recording directory, no runtime).
    #[error("live mode: {0}")]
    Live(String),
}

/// A mock exchange. Cheap to clone (shared state).
#[derive(Clone)]
pub struct Mock {
    shared: Arc<Shared>,
}

impl Mock {
    /// Loads the tape and script. Permits are verified with [`SignatureVerifier`] when
    /// [`MockConfig::verifies`], else every signature is accepted ([`AcceptAll`]).
    pub fn new(cfg: MockConfig) -> Result<Mock, MockError> {
        let verifier: Arc<dyn PermitVerifier> = if cfg.verifies() {
            Arc::new(SignatureVerifier::new(cfg.trading_keys.clone()))
        } else {
            Arc::new(AcceptAll)
        };
        Mock::with_verifier(cfg, verifier)
    }

    /// Like [`Mock::new`] with an explicit permit verifier.
    pub fn with_verifier(
        cfg: MockConfig,
        verifier: Arc<dyn PermitVerifier>,
    ) -> Result<Mock, MockError> {
        let tape = Arc::new(Tape::load(&cfg.tape_dir)?);
        let script = match &cfg.script {
            ScriptSource::None => Script::default(),
            ScriptSource::File(p) => Script::load(p)?,
            ScriptSource::Auto => match find_script(&cfg.tape_dir, &tape.dir) {
                Some(p) => {
                    tracing::info!(path = %p.display(), "loading script");
                    Script::load(&p)?
                }
                None => Script::default(),
            },
        };
        let start_ms = cfg.start_ms.unwrap_or_else(|| tape.first_ms());
        tracing::info!(
            dir = %tape.dir.display(),
            books = tape.books.len(),
            trades = tape.trades.len(),
            markets = tape.markets.len(),
            first_ms = tape.first_ms(),
            last_ms = tape.last_ms(),
            stats = ?tape.stats,
            scripted = script.resolve.len(),
            "tape loaded"
        );
        let mut engine = Engine::new(
            tape,
            EngineConfig {
                start_ms,
                speed: cfg.speed,
                initial_collateral_u: cfg.initial_collateral_u,
                script,
            },
        );
        for s in &cfg.account_snapshots {
            let positions = engine.boot_account(&s.owner, &s.state);
            tracing::info!(owner = %s.owner, positions, collateral = ?s.state.collateral_usd, "account booted from snapshot");
        }
        Ok(Mock {
            shared: Arc::new(
                Shared::new(engine, verifier).with_ws_idle_timeout(cfg.ws_idle_timeout),
            ),
        })
    }

    /// A live-mode mock (L4-1): no tape; books and results from `live.upstream`, orders
    /// matched locally and never forwarded. The clock is the wall clock (`cfg.speed`,
    /// `cfg.start_ms` and `cfg.tape_dir` are ignored); `cfg.script` applies only as
    /// [`ScriptSource::File`]. Starts the upstream tasks, so it must be called inside a Tokio
    /// runtime. Permits are verified as by [`Mock::new`].
    pub fn new_live(cfg: MockConfig, live: LiveConfig) -> Result<Mock, MockError> {
        let verifier: Arc<dyn PermitVerifier> = if cfg.verifies() {
            Arc::new(SignatureVerifier::new(cfg.trading_keys.clone()))
        } else {
            Arc::new(AcceptAll)
        };
        Mock::live_with_verifier(cfg, live, verifier)
    }

    /// Like [`Mock::new_live`] with an explicit permit verifier.
    pub fn live_with_verifier(
        cfg: MockConfig,
        mut live: LiveConfig,
        verifier: Arc<dyn PermitVerifier>,
    ) -> Result<Mock, MockError> {
        tokio::runtime::Handle::try_current()
            .map_err(|_| MockError::Live("Mock::new_live needs a Tokio runtime".into()))?;
        let host = live.validate().map_err(MockError::Live)?;
        let script = match &cfg.script {
            ScriptSource::File(p) => Script::load(p)?,
            ScriptSource::Auto | ScriptSource::None => Script::default(),
        };
        let start_ms = util::wall_ms();
        let recorder = match &live.record {
            Some(dir) => {
                Some(record::Recorder::open(dir, &host, start_ms).map_err(MockError::Live)?)
            }
            None => None,
        };
        let mut engine = Engine::new_live(EngineConfig {
            start_ms,
            speed: 1.0,
            initial_collateral_u: cfg.initial_collateral_u,
            script,
        });
        for s in &cfg.account_snapshots {
            let positions = engine.boot_account(&s.owner, &s.state);
            tracing::info!(owner = %s.owner, positions, collateral = ?s.state.collateral_usd, "account booted from snapshot");
        }
        tracing::info!(upstream = %live.upstream, socket = %live.ws_url(),
            record = ?live.record, markets_every = ?live.markets_every, "live mode");
        let (link, tasks) = LiveLink::new(live, host, recorder);
        let shared = Arc::new(
            Shared::new(engine, verifier)
                .with_ws_idle_timeout(cfg.ws_idle_timeout)
                .with_live(link.clone()),
        );
        live::spawn(&shared, &link, tasks);
        Ok(Mock { shared })
    }

    /// The live link (`None` in replay).
    pub fn live(&self) -> Option<&Arc<LiveLink>> {
        self.shared.live()
    }

    /// Live mode: stops the upstream tasks and finalises a recording's manifest with
    /// `stop_reason`. No-op in replay.
    pub async fn stop_live(&self, stop_reason: &str) {
        if let Some(link) = self.shared.live() {
            link.stop(stop_reason).await;
        }
    }

    /// The axum router.
    pub fn router(&self) -> axum::Router {
        api::router(self.shared.clone())
    }

    /// Subscribes to the event bus.
    pub fn subscribe(&self) -> broadcast::Receiver<MockEvent> {
        self.shared.engine().subscribe()
    }

    /// Runs `f` on the engine (brought to the clock first; account deltas are published
    /// when `f` returns).
    pub fn with_engine<R>(&self, f: impl FnOnce(&mut Engine) -> R) -> R {
        f(&mut self.shared.engine())
    }

    /// Serves on `listener` until the task is dropped.
    pub async fn serve(self, listener: TcpListener) -> std::io::Result<()> {
        axum::serve(listener, self.router()).await
    }

    /// Binds `addr` (e.g. `127.0.0.1:0`) and serves in a background task.
    pub async fn spawn(self, addr: &str) -> std::io::Result<RunningMock> {
        let listener = TcpListener::bind(addr).await?;
        let addr = listener.local_addr()?;
        let mock = self.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = self.serve(listener).await {
                tracing::error!(error = %e, "pascal-mock server stopped");
            }
        });
        Ok(RunningMock { addr, mock, task })
    }
}

/// A mock serving in the background; the server stops when this is dropped.
pub struct RunningMock {
    /// Bound address.
    pub addr: SocketAddr,
    /// Handle on the mock's state.
    pub mock: Mock,
    task: JoinHandle<()>,
}

impl RunningMock {
    /// `http://<addr>`, for `pascal_rs::Client::builder().base_url(..)`.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// `ws://<addr>/ws`, for `pascal_rs::WsConfig::url`.
    pub fn ws_url(&self) -> String {
        format!("ws://{}/ws", self.addr)
    }
}

impl Drop for RunningMock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn find_script(given: &Path, files: &Path) -> Option<PathBuf> {
    [Some(given), Some(files), files.parent()]
        .into_iter()
        .flatten()
        .map(|d| d.join("script.toml"))
        .find(|p| p.is_file())
}
