//! Start-up and shutdown: open the database, build the state, spawn the services and the HTTP
//! server, and stop them all on request.

use std::future::IntoFuture;
use std::net::SocketAddr;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::FutureExt;
use tokio::task::JoinSet;

use crate::config::Config;
use crate::solana::rpc::redact;
use crate::state::{AppState, DeskEvent, pascal_client};
use crate::{api, catalogue, crank, db, hedge, indexer, now_ms, quoter, sentiment};

/// Boxed error of [`start`].
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// A started desk.
pub struct Running {
    /// Bound address (resolved when `listen` had port 0).
    pub addr: SocketAddr,
    /// Shared state (tests emit events through it).
    pub state: AppState,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
    services: JoinSet<&'static str>,
    failed: Arc<AtomicBool>,
}

/// Opens the database (applying migrations), connects the Pascal handle, spawns the services and
/// serves the API on `config.listen`.
pub async fn start(config: Config) -> Result<Running, BoxError> {
    crate::metrics::install();
    tracing::info!(
        profile = config.profile.as_str(),
        listen = %config.listen,
        db = %config.db_path,
        pascal_data_url = %redact(&config.pascal_data_url),
        pascal_ws_url = %redact(&config.pascal_ws_url),
        solana_rpc_url = %redact(&config.solana_rpc_url),
        program_id = %config.program_id,
        "desk starting"
    );
    let pool = db::open(&config.db_path)
        .await
        .map_err(|e| format!("database {}: {e}", config.db_path))?;
    let pascal = pascal_client(&config)?;
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|e| format!("listen on {}: {e}", config.listen))?;
    let addr = listener.local_addr()?;
    let state = AppState::new(config, pascal, pool);

    let mut services = JoinSet::new();
    let failed = Arc::new(AtomicBool::new(false));
    let s = &mut services;
    spawn(
        s,
        &state,
        &failed,
        "catalogue",
        catalogue::run(state.clone()),
    );
    spawn(s, &state, &failed, "quoter", quoter::run(state.clone()));
    spawn(s, &state, &failed, "hedge", hedge::run(state.clone()));
    spawn(s, &state, &failed, "crank", crank::run(state.clone()));
    spawn(s, &state, &failed, "indexer", indexer::run(state.clone()));
    spawn(
        s,
        &state,
        &failed,
        "sentiment",
        sentiment::run(state.clone()),
    );
    spawn(s, &state, &failed, "fx", crate::fx::run(state.clone()));
    spawn(
        s,
        &state,
        &failed,
        "scores",
        crate::scores::run(state.clone()),
    );
    spawn(s, &state, &failed, "heartbeat", heartbeat(state.clone()));

    let app = api::router(state.clone());
    let stop = state.clone();
    let server = tokio::spawn(
        // The peer address reaches handlers as `ConnectInfo` (`PUT /profile` limits per IP).
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move { stop.shutdown_requested().await })
        .into_future(),
    );
    tracing::info!(%addr, "desk listening");
    Ok(Running {
        addr,
        state,
        server,
        services,
        failed,
    })
}

/// Spawns a service. A service that panics, or returns before shutdown was asked for, stops the
/// whole desk (fail fast: the runbook's answer to a crash is a restart, which replays from the
/// DB and Pascal) and is recorded so `main` exits non-zero.
fn spawn(
    set: &mut JoinSet<&'static str>,
    state: &AppState,
    failed: &Arc<AtomicBool>,
    name: &'static str,
    service: impl Future<Output = ()> + Send + 'static,
) {
    let (state, failed) = (state.clone(), failed.clone());
    set.spawn(async move {
        let outcome = std::panic::AssertUnwindSafe(service).catch_unwind().await;
        if outcome.is_err() || !state.is_shutting_down() {
            let how = if outcome.is_err() {
                "panicked"
            } else {
                "returned early"
            };
            tracing::error!(service = name, how, "service failed; stopping the desk");
            failed.store(true, Ordering::SeqCst);
            state.shutdown();
        }
        name
    });
}

/// Emits [`DeskEvent::Heartbeat`] every `heartbeat_period()` and runs metrics upkeep.
async fn heartbeat(state: AppState) {
    let mut tick = tokio::time::interval(state.config.heartbeat_period());
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = state.shutdown_requested() => break,
            _ = tick.tick() => {
                state.emit(DeskEvent::Heartbeat { ts_ms: now_ms() });
                crate::metrics::upkeep();
            }
        }
    }
}

impl Running {
    /// Asks the server and every service to stop.
    pub fn shutdown(&self) {
        self.state.shutdown();
    }

    /// Resolves when shutdown was asked for (by [`Self::shutdown`] or a service).
    pub async fn stopped(&self) {
        self.state.shutdown_requested().await;
    }

    /// Whether a service failed (panicked or returned before shutdown).
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }

    /// Shuts down and waits for the server and the services, at most `shutdown_grace` each,
    /// then closes the database.
    pub async fn join(mut self) {
        self.shutdown();
        let grace = self.state.config.shutdown_grace;
        match tokio::time::timeout(grace, &mut self.server).await {
            Ok(Ok(Ok(()))) => tracing::info!("http server stopped"),
            Ok(Ok(Err(e))) => tracing::error!(error = %e, "http server failed"),
            Ok(Err(e)) => tracing::error!(error = %e, "http server task panicked"),
            Err(_) => {
                tracing::warn!("http server did not stop in time; aborting");
                self.server.abort();
            }
        }
        let deadline = tokio::time::Instant::now() + grace;
        loop {
            match tokio::time::timeout_at(deadline, self.services.join_next()).await {
                Ok(None) => break,
                Ok(Some(Ok(name))) => tracing::info!(service = name, "service stopped"),
                Ok(Some(Err(e))) => tracing::error!(error = %e, "service task failed"),
                Err(_) => {
                    tracing::warn!(
                        left = self.services.len(),
                        "services did not stop in time; aborting"
                    );
                    self.services.shutdown().await;
                    break;
                }
            }
        }
        self.state.db.close().await;
        tracing::info!("desk stopped");
    }
}
