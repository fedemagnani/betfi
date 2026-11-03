//! `desk-flatten`: the hedge desk's panic button (spec section 3 "Desk panic", section 12
//! runbook "Hedger under-collateralized").
//!
//! Reads the desk's environment (`PASCAL_*`, `DESK_DB`, ...), then:
//!
//! - without `--yes`: prints the hedger's resting orders and positions (dry run);
//! - with `--yes`: one mass-cancel per market (batches of 10), then a reduce-only IOC closing
//!   every position, each signed with the builder code and recorded in the desk DB.
//!
//! Stop the desk first (or it re-hedges open tickets on its next event). Writes go where the
//! desk's would: `PASCAL_TRADE_URL`, else the mock on dev/demo; on `live` without a trade URL
//! every write fails locally (D-047).

use std::process::ExitCode;

use desk::hedge::{BUILDER_FEE_RATE_U, Trader, flatten};
use pascal_rs::AccountQuery;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let yes = std::env::args().skip(1).any(|a| a == "--yes");
    let config = match desk::Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("bad configuration: {e}");
            return ExitCode::from(2);
        }
    };
    let pascal = match desk::pascal::pascal_client(&config) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("pascal client: {e}");
            return ExitCode::FAILURE;
        }
    };
    let trader = match Trader::from_config(&config, BUILDER_FEE_RATE_U) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("hedger keys: {e}");
            return ExitCode::from(2);
        }
    };
    if !yes {
        let q = AccountQuery {
            symbol: None,
            exclude_orders: false,
        };
        return match pascal.account(trader.owner(), &q).await {
            Ok(r) => {
                println!("hedger {} (dry run; pass --yes to flatten)", trader.owner());
                for o in &r.data.orders {
                    println!(
                        "order  {} {:?} {} @ {} (client_order_id {})",
                        o.symbol, o.side, o.size_remaining, o.price, o.client_order_id
                    );
                }
                for p in r.data.positions.iter().filter(|p| p.size != 0) {
                    println!("position {} {}", p.symbol, p.size);
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("/accounts: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let db = match desk::db::open(&config.db_path).await {
        Ok(db) => Some(db),
        Err(e) => {
            eprintln!(
                "desk db {} unavailable ({e}); flattening without records",
                config.db_path
            );
            None
        }
    };
    match flatten::flatten(&pascal, &trader, db.as_ref()).await {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).unwrap_or_default()
            );
            if report.left.is_empty() && report.errors.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("flatten: {e}");
            ExitCode::FAILURE
        }
    }
}
