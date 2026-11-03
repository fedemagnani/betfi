//! `desk` binary: configuration from the environment, JSON logs, the desk until SIGINT/SIGTERM.

use std::process::ExitCode;

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    // One JSON object per line (spec section 12); `RUST_LOG` filters (default `info`).
    tracing_subscriber::fmt()
        .json()
        .with_current_span(false)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let config = match desk::Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "bad configuration");
            return ExitCode::from(2);
        }
    };
    let running = match desk::start(config).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "desk failed to start");
            return ExitCode::FAILURE;
        }
    };
    tokio::select! {
        () = signal() => tracing::info!("shutdown signal received"),
        () = running.stopped() => {}
    }
    let failed = running.failed();
    running.join().await;
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// SIGINT (Ctrl-C) or, on Unix, SIGTERM.
async fn signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %e, "cannot listen for Ctrl-C");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(e) => {
                tracing::error!(error = %e, "cannot listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = term => {}
    }
}
