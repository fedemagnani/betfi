//! Build-time configuration. A static web app has no process environment at run time, so the
//! two endpoints are read with `option_env!` when `dx` compiles the crate:
//!
//! - `SCHEDINA_DESK_URL` (e.g. `http://127.0.0.1:8080`): the desk's HTTP base. Unset or empty,
//!   the app runs in **demo mode** on the C4 example responses in `fixtures/desk/`.
//! - `SCHEDINA_RPC_URL`: Solana JSON-RPC endpoint. Unset, it is devnet's public endpoint. Any URL
//!   naming mainnet is refused (devnet is used instead and the app says so), and
//!   [`crate::wallet`] also refuses to send through an endpoint whose genesis hash is
//!   mainnet-beta's, so no configuration can make this app send a mainnet transaction.

use dioxus_solana::Cluster;

/// Mainnet-beta's genesis hash: an RPC answering with it is refused before any send.
pub const MAINNET_GENESIS_HASH: &str = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d";

/// The desk base URL without a trailing slash, or `None` for demo mode.
pub fn desk_url() -> Option<String> {
    let raw = option_env!("SCHEDINA_DESK_URL")?
        .trim()
        .trim_end_matches('/');
    if raw.starts_with("http://") || raw.starts_with("https://") {
        Some(raw.to_string())
    } else {
        None
    }
}

/// True when a URL names mainnet. Providers whose URLs do not say so are caught by the genesis
/// hash check in [`crate::wallet`].
pub fn is_mainnet_url(url: &str) -> bool {
    url.to_ascii_lowercase().contains("mainnet")
}

/// The cluster the app reads and signs against, and a warning to show when the configured
/// endpoint was refused.
pub fn cluster() -> (Cluster, Option<String>) {
    let Some(url) = option_env!("SCHEDINA_RPC_URL")
        .map(str::trim)
        .filter(|u| !u.is_empty())
    else {
        return (Cluster::Devnet, None);
    };
    if is_mainnet_url(url) {
        return (
            Cluster::Devnet,
            Some(format!(
                "SCHEDINA_RPC_URL looks like mainnet and was refused; using devnet ({})",
                Cluster::Devnet.rpc_url()
            )),
        );
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return (
            Cluster::Devnet,
            Some("SCHEDINA_RPC_URL is not an http(s) URL; using devnet".to_string()),
        );
    }
    let local = url.contains("localhost") || url.contains("127.0.0.1");
    let ws = if local {
        url.replacen("http", "ws", 1).replace(":8899", ":8900")
    } else {
        url.replacen("http", "ws", 1)
    };
    let chain = if local {
        Cluster::Localnet
    } else {
        Cluster::Devnet
    };
    (Cluster::custom(chain, url, ws), None)
}

/// `SCHEDINA_SLATE_DATE` (`YYYY-MM-DD`): the night the Card asks for, when set. A desk that
/// replays a recorded tape (the `demo` profile on the mock) serves that tape's night, not
/// today's; unset, the Card asks for every game not over ([`crate::desk::slate_date`]).
pub fn slate_date_override() -> Option<String> {
    let raw = option_env!("SCHEDINA_SLATE_DATE")?.trim();
    is_date(raw).then(|| raw.to_string())
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// `SCHEDINA_DESIGN_FIXTURES` (6.0a): set (and not `0`) at build time, the app runs in demo mode
/// on `fixtures/design/`, a fictional data set that reproduces the BetFi mockup's content for
/// the side-by-side visual review. Never the default: unset, demo mode keeps `fixtures/desk/`,
/// and the design files are not compiled into the bundle at all.
pub const DESIGN_FIXTURES: bool = flag_on(option_env!("SCHEDINA_DESIGN_FIXTURES"));

const fn flag_on(v: Option<&str>) -> bool {
    match v {
        None => false,
        Some(s) => {
            let b = s.as_bytes();
            !(b.is_empty() || (b.len() == 1 && b[0] == b'0'))
        }
    }
}

/// Dev-only screens (the wallet spike's test-signature page): debug builds, or any build made
/// with `SCHEDINA_DEV_TOOLS` set.
pub fn dev_tools() -> bool {
    cfg!(debug_assertions) || option_env!("SCHEDINA_DEV_TOOLS").is_some()
}

/// V-9 (7.0a): Solscan for a transaction, on devnet, wherever the mockup says Solscan. Only
/// base58 characters of `sig` are kept, so nothing else reaches the URL.
pub fn solscan_tx(sig: &str) -> String {
    format!("https://solscan.io/tx/{}?cluster=devnet", base58_only(sig))
}

/// V-9 (7.0a): Solscan for an account (a tipster's wallet), on devnet.
pub fn solscan_account(address: &str) -> String {
    format!(
        "https://solscan.io/account/{}?cluster=devnet",
        base58_only(address)
    )
}

fn base58_only(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() && !matches!(c, '0' | 'O' | 'I' | 'l'))
        .collect()
}

/// 8.D (L8-3): the Privy app for "Continue with Google" (its embedded Solana wallet signs). An
/// app ID is public (it ships in every page that uses Privy); there is no client secret.
pub const PRIVY_APP_ID: &str = "cmvwcynao00xn0hlbq5t6gtnz";

/// The Privy app ID this build uses: `SCHEDINA_PRIVY_APP_ID` at build time when set and not
/// blank, else [`PRIVY_APP_ID`].
pub fn privy_app_id() -> &'static str {
    match option_env!("SCHEDINA_PRIVY_APP_ID").map(str::trim) {
        Some(id) if !id.is_empty() => id,
        _ => PRIVY_APP_ID,
    }
}
