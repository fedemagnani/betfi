//! `pascal-mock`: serves a recorded tape (replay) or Pascal's live books (`--live`) behind
//! Pascal's REST and WebSocket API, filling orders with its own matcher.
//!
//! ```text
//! just mock --tape tapes/nba-2026-10-28 --speed 60
//! just mock --tape tapes/nba-2026-10-28 --speed 60 \
//!     --trading-key <hedger owner>=<trading key> --account-snapshot <owner>=hedger.json
//! just mock --live                                  # books and results from data.pascal.trade
//! just mock --live --record tapes/nba-2026-10-29    # ... and write the session as a tape
//! ```
//!
//! Permits are verified with `pascal-rs` by default (`--no-verify` accepts any signature).
//! Live mode reads Pascal's public data host only; no order ever reaches Pascal (L4-1).

use std::path::PathBuf;

use clap::Parser;
use pascal_mock::live::DEFAULT_UPSTREAM;
use pascal_mock::{AccountSnapshot, At, LiveConfig, Mock, MockConfig, ScriptSource, TradingKeys};
use pascal_rs::num::parse_micros;
use tracing_subscriber::EnvFilter;

/// Pascal mock: tape replay (or live Pascal books) and a matcher behind Pascal's REST schema.
#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    /// Tape directory: tapes/<night> (uses raw/ if present, else sample/) or raw/ or sample/.
    #[arg(long, required_unless_present = "live")]
    tape: Option<PathBuf>,
    /// Live mode: books, catalogue and results from Pascal's public read API and socket (wall
    /// clock); orders still fill locally and are never forwarded.
    #[arg(long, conflicts_with_all = ["tape", "start"])]
    live: bool,
    /// Live mode: the read API base URL (only Pascal's data host, or a loopback fake).
    #[arg(long, default_value = DEFAULT_UPSTREAM, requires = "live")]
    upstream: String,
    /// Live mode: also write the session as a tape into this (new) directory.
    #[arg(long, value_name = "DIR", requires = "live")]
    record: Option<PathBuf>,
    /// Listen address.
    #[arg(long, default_value = "127.0.0.1:8790")]
    bind: String,
    /// Tape ms per wall ms: 60 plays one hour of tape per minute; 0 freezes the clock.
    #[arg(long, default_value_t = 1.0)]
    speed: f64,
    /// Clock start: tape Unix ms, or start+<dur> after the first snapshot (e.g. start+2h).
    #[arg(long)]
    start: Option<String>,
    /// script.toml with scripted resolutions (default: the tape's script.toml, if any).
    #[arg(long, conflicts_with = "no_script")]
    script: Option<PathBuf>,
    /// Ignore the tape's script.toml.
    #[arg(long)]
    no_script: bool,
    /// Free collateral of each new account, USD.
    #[arg(long, default_value = "10000")]
    collateral: String,
    /// Register a trading key: <owner>=<pubkey>, both base58 (repeatable). With none, any
    /// permit whose signature is valid for the key it names is accepted.
    #[arg(long = "trading-key", value_name = "OWNER=PUBKEY")]
    trading_keys: Vec<String>,
    /// Accept every permit signature unchecked (the 1.D behaviour).
    #[arg(long, conflicts_with = "trading_keys")]
    no_verify: bool,
    /// Boot an account from a saved GET /api/v1/accounts/{owner} response (or an `account`
    /// WebSocket snapshot frame): [<owner>=]<file> (repeatable).
    #[arg(long = "account-snapshot", value_name = "[OWNER=]FILE")]
    account_snapshots: Vec<String>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    if let Err(e) = run(Args::parse()).await {
        eprintln!("pascal-mock: {e}");
        std::process::exit(1);
    }
}

async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let tape = args.tape.clone().unwrap_or_default();
    let mut cfg = MockConfig::new(&tape);
    cfg.speed = args.speed;
    cfg.initial_collateral_u =
        parse_micros(&args.collateral).map_err(|e| format!("--collateral: {e}"))?;
    cfg.verify_permits = !args.no_verify;
    let mut keys = TradingKeys::new();
    for spec in &args.trading_keys {
        keys.add_spec(spec)
            .map_err(|e| format!("--trading-key: {e}"))?;
    }
    cfg.trading_keys = keys;
    for spec in &args.account_snapshots {
        cfg.account_snapshots.push(
            AccountSnapshot::load_spec(spec).map_err(|e| format!("--account-snapshot: {e}"))?,
        );
    }
    cfg.script = match (args.script.clone(), args.no_script) {
        (Some(p), _) => ScriptSource::File(p),
        (None, true) => ScriptSource::None,
        (None, false) => ScriptSource::Auto,
    };
    if args.live {
        return run_live(args, cfg).await;
    }
    if let Some(s) = &args.start {
        let first = pascal_mock::Tape::load(&tape)?.first_ms();
        cfg.start_ms = Some(match At::parse(s)? {
            At::Tape(ms) => ms,
            At::AfterStart(d) => first + d,
            _ => return Err("--start takes tape ms or start+<dur>".into()),
        });
    }
    let verify = cfg.verifies();
    let keys = cfg.trading_keys.len();
    let mock = Mock::new(cfg)?;
    let listener = tokio::net::TcpListener::bind(&args.bind).await?;
    tracing::info!(
        addr = %listener.local_addr()?,
        speed = args.speed,
        verify_permits = verify,
        trading_keys = keys,
        "pascal-mock serving (REST /api/v1, WebSocket /ws)"
    );
    axum::serve(listener, mock.router())
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
        })
        .await?;
    Ok(())
}

async fn run_live(args: Args, cfg: MockConfig) -> Result<(), Box<dyn std::error::Error>> {
    if args.speed != 1.0 {
        return Err("--speed does not apply to --live (wall clock)".into());
    }
    let mut live = LiveConfig::new(&args.upstream);
    live.record = args.record.clone();
    let verify = cfg.verifies();
    let keys = cfg.trading_keys.len();
    let mock = Mock::new_live(cfg, live)?;
    let link = mock.live().cloned().ok_or("live link missing")?;
    // Serve once the catalogue is in, so the desk's first /markets poll finds it.
    if !link
        .wait_catalogue(std::time::Duration::from_secs(45))
        .await
    {
        tracing::warn!(
            "live: no catalogue after 45 s; serving anyway (catalogue routes answer NOT_READY)"
        );
    }
    let listener = tokio::net::TcpListener::bind(&args.bind).await?;
    tracing::info!(
        addr = %listener.local_addr()?,
        upstream = %link.host(),
        verify_permits = verify,
        trading_keys = keys,
        record = ?args.record,
        "pascal-mock serving in live mode (REST /api/v1, WebSocket /ws, /mock/info)"
    );
    let reason = tokio::select! {
        r = axum::serve(listener, mock.router()) => {
            r?;
            "server stopped"
        }
        reason = shutdown_signal() => reason,
    };
    // Open sockets are not drained: finalise the recording and exit.
    mock.stop_live(reason).await;
    tracing::info!(reason, "pascal-mock stopped");
    Ok(())
}

/// SIGINT or SIGTERM (the demo scripts stop their children with SIGTERM).
async fn shutdown_signal() -> &'static str {
    use tokio::signal::unix::{SignalKind, signal};
    match signal(SignalKind::terminate()) {
        Ok(mut term) => tokio::select! {
            _ = tokio::signal::ctrl_c() => "SIGINT",
            _ = term.recv() => "SIGTERM",
        },
        Err(_) => {
            tokio::signal::ctrl_c().await.ok();
            "SIGINT"
        }
    }
}
