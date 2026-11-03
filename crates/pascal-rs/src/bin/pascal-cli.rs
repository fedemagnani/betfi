//! `pascal-cli`: manual checks against Pascal's live read API, and order signing.
//!
//! - `pascal-cli top-wallets --days 7`: walks the public tape back `days` days and counts the
//!   distinct maker / taker wallets, with per-wallet trade counts and notional.
//! - `pascal-cli history <owner>`: prints the four timeframes of `/accounts/{owner}/history`.
//! - `pascal-cli time`: the read API's clock and current `seq`.
//! - `pascal-cli books <SYMBOL>...`: best bid / ask and the depth within 5 ticks of each.
//! - `pascal-cli sign-order ...`: builds and signs a place-order and prints the request, the
//!   permit bytes and the signature. It never sends anything.
//! - `pascal-cli verify-order <FILE|-> --signer <TRADING KEY>`: rebuilds the 232-byte permit of
//!   each signed place-order request, verifies its Ed25519 signature against the expected
//!   trading key, and prints every permit field with its byte offsets (builder bytes included).
//!   Exits non-zero on any mismatch.
//! - `pascal-cli pubkey <KEY FILE>`: the base58 public key of a key file.
//! - `pascal-cli place-order ... --i-am-authorized`: signs and SENDS one order to Pascal's
//!   production write API. Refuses to run without the flag (decisions D-001, D-007: no order
//!   is sent from Italy without an approved path). This is the only code path in the
//!   workspace that knows the production write URL.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use pascal_rs::signing::{self, PlaceOrderRequest};
use pascal_rs::{
    AuthParams, BuilderInfo, BuilderPolicy, Client, Error, Level, Lookback, MarketSearch, MicroUsd,
    PlaceOrder, ResolvedOrderBy, ResolvedQuery, Result, Side, Signed, TickRule, TimeInForce,
    Timeframe, TradeClient, TradingSigner, format_micros, parse_micros,
    rest::{DEFAULT_BASE_URL, MAX_TRADES_SYMBOLS},
};

/// Pascal's production write API. Used only by `place-order --i-am-authorized`.
const PROD_TRADE_URL: &str = "https://trade.pascal.trade";

#[derive(Parser)]
#[command(
    name = "pascal-cli",
    about = "Manual checks against Pascal's read API, and order signing"
)]
struct Cli {
    /// Read API base URL.
    #[arg(long, default_value = DEFAULT_BASE_URL, global = true)]
    base_url: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Distinct maker / taker wallets on the public tape over the last N days.
    TopWallets {
        /// Lookback in days.
        #[arg(long, default_value_t = 7)]
        days: u64,
        /// How many wallets to list.
        #[arg(long, default_value_t = 25)]
        top: usize,
        /// Safety cap on pages fetched per 120-symbol chunk (500 trades per page).
        #[arg(long, default_value_t = 400)]
        max_pages: u32,
    },
    /// The four timeframes of an account's PnL / value history.
    History {
        /// Account owner (base58 wallet).
        owner: String,
    },
    /// Server time and sequence.
    Time,
    /// Best bid / ask, and contracts and notional within 5 ticks of each best price.
    Books {
        /// Market symbols (1 to 50).
        #[arg(required = true)]
        symbols: Vec<String>,
    },
    /// Build and sign a place-order and print it. Never sends anything.
    SignOrder(OrderArgs),
    /// Verify a signed place-order request (one JSON object) or body (a JSON array) against the
    /// expected trading key, and print every permit field with its byte offsets. Exits non-zero
    /// on any mismatch. Never sends anything.
    VerifyOrder {
        /// File with the JSON, or `-` for stdin.
        file: String,
        /// The trading key (base58) every request must be signed by.
        #[arg(long)]
        signer: String,
        /// Also require this builder account (base58) on every request.
        #[arg(long)]
        builder: Option<String>,
        /// Also require this builder fee rate (decimal, `0.001` = 0.1%); needs --builder.
        #[arg(long, requires = "builder")]
        builder_fee_rate: Option<String>,
        /// Deployment the permit was signed for.
        #[arg(long, default_value_t = signing::DEPLOYMENT_PROD)]
        deployment_id: u8,
        /// Print only the one-line summary per request, not the permit table.
        #[arg(long)]
        brief: bool,
    },
    /// Print the base58 public key of a key file (JSON byte array, hex seed or base58).
    Pubkey {
        /// Key file.
        key_file: String,
    },
    /// Sign and SEND one order to Pascal's write API. Refuses to run without --i-am-authorized.
    PlaceOrder {
        #[command(flatten)]
        order: OrderArgs,
        /// Write API base URL.
        #[arg(long, default_value = PROD_TRADE_URL)]
        trade_url: String,
        /// Confirms an approved path to trade from where you are (decisions D-001, D-007).
        #[arg(long)]
        i_am_authorized: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum SideArg {
    Bid,
    Ask,
}

#[derive(Clone, Copy, ValueEnum)]
enum TifArg {
    Gtc,
    Gtt,
    Ioc,
}

/// A place-order as command-line flags.
#[derive(Args)]
struct OrderArgs {
    /// Market symbol.
    #[arg(long)]
    symbol: String,
    /// Side.
    #[arg(long, value_enum)]
    side: SideArg,
    /// Limit price, decimal (`0.715`).
    #[arg(long)]
    price: String,
    /// Contracts.
    #[arg(long)]
    size: u64,
    /// Client order id (the desk-wide counter).
    #[arg(long)]
    client_order_id: u64,
    /// Time in force.
    #[arg(long, value_enum, default_value = "ioc")]
    tif: TifArg,
    /// GTT expiry, Unix ms.
    #[arg(long)]
    expires_ts_ms: Option<u64>,
    /// Maker only.
    #[arg(long)]
    post_only: bool,
    /// Reduce only.
    #[arg(long)]
    reduce_only: bool,
    /// Builder account (base58).
    #[arg(long)]
    builder: Option<String>,
    /// Builder fee rate, decimal (`0.001` = 0.1%).
    #[arg(long, default_value = "0.001000")]
    builder_fee_rate: String,
    /// Account owner (base58). Defaults to the docs' test owner with --test-key.
    #[arg(long)]
    owner: Option<String>,
    /// Trading key file (JSON byte array, hex seed or base58).
    #[arg(long)]
    key_file: Option<String>,
    /// Sign with the public docs test key (seed [8; 32]; owner seed [7; 32]).
    #[arg(long, conflicts_with = "key_file")]
    test_key: bool,
    /// Client timestamp, Unix ms (default: now).
    #[arg(long)]
    client_ts_ms: Option<u64>,
    /// Receive window, ms.
    #[arg(long, default_value_t = signing::DEFAULT_RECV_WINDOW_MS)]
    recv_window_ms: u32,
    /// Deployment (3 = prod).
    #[arg(long, default_value_t = signing::DEPLOYMENT_PROD)]
    deployment_id: u8,
    /// Check the price against this tick_size_min (decimal); needs --tick-sig-figs.
    #[arg(long, requires = "tick_sig_figs")]
    tick_size_min: Option<String>,
    /// tick_sig_figs of the market; needs --tick-size-min.
    #[arg(long, requires = "tick_size_min")]
    tick_sig_figs: Option<u8>,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let client = match Client::builder().base_url(&cli.base_url).build() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = match cli.command {
        Command::TopWallets {
            days,
            top,
            max_pages,
        } => top_wallets(&client, days, top, max_pages).await,
        Command::History { owner } => history(&client, &owner).await,
        Command::Time => time(&client).await,
        Command::Books { symbols } => books(&client, &symbols).await,
        Command::SignOrder(args) => sign_order(&args),
        Command::VerifyOrder {
            file,
            signer,
            builder,
            builder_fee_rate,
            deployment_id,
            brief,
        } => verify_order(
            &file,
            &signer,
            builder.as_deref(),
            builder_fee_rate.as_deref(),
            deployment_id,
            brief,
        ),
        Command::Pubkey { key_file } => TradingSigner::from_file(&key_file)
            .map(|k| println!("{}", k.pubkey_base58()))
            .map_err(Error::from),
        Command::PlaceOrder {
            order,
            trade_url,
            i_am_authorized,
        } => place_order(&order, &trade_url, i_am_authorized).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn time(client: &Client) -> Result<()> {
    let t = client.time().await?;
    println!(
        "server_time_ms {} ({})  seq {:?}  state_time_ms {:?}",
        t.data.server_time_ms,
        utc(t.data.server_time_ms),
        t.seq,
        t.state_time_ms
    );
    Ok(())
}

#[derive(Default)]
struct WalletStats {
    maker: u64,
    taker: u64,
    notional: i128,
}

async fn top_wallets(client: &Client, days: u64, top: usize, max_pages: u32) -> Result<()> {
    let started = std::time::Instant::now();
    let now_ms = client.time().await?.data.server_time_ms;
    let cutoff = now_ms.saturating_sub(days * 86_400_000);

    // Markets that have ever traded: the live list plus the last 3 days of resolutions (the
    // longest lookback /markets/resolved offers). Busiest first.
    let live = client.markets_lenient().await?;
    let resolved = client
        .resolved_markets_lenient(ResolvedQuery {
            lookback: Lookback::Days3,
            order_by: ResolvedOrderBy::ResolutionTime,
            limit: 200,
        })
        .await?;
    for (what, r) in [("markets", &live), ("resolved markets", &resolved)] {
        for f in &r.data.failures {
            eprintln!("warning: {what}[{}] did not decode: {}", f.index, f.error);
        }
    }
    let mut seen = HashSet::new();
    let mut symbols: Vec<(u64, String)> = live
        .data
        .values()
        .chain(resolved.data.values())
        .filter_map(|m| {
            let trades = m.stats.as_ref().map_or(0, |s| s.all_time.trade_count());
            (trades > 0 && seen.insert(m.symbol.clone())).then(|| (trades, m.symbol.clone()))
        })
        .collect();
    symbols.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let symbols: Vec<String> = symbols.into_iter().map(|(_, s)| s).collect();
    eprintln!(
        "walking {} traded markets ({} live, {} resolved in 3 days) back to {} ({:.1}s)",
        symbols.len(),
        live.data.items.len(),
        resolved.data.items.len(),
        utc(cutoff),
        started.elapsed().as_secs_f64()
    );

    let mut wallets: HashMap<String, WalletStats> = HashMap::new();
    let mut trade_ids = HashSet::new();
    let mut markets_hit = BTreeSet::new();
    let mut pages = 0;
    for (i, chunk) in symbols.chunks(MAX_TRADES_SYMBOLS).enumerate() {
        let mut pager = client.trades_pager(chunk, Some(500))?;
        let trades = pager
            .collect_while(|t| t.trade_ts_ms >= cutoff, max_pages)
            .await?;
        pages += pager.pages_fetched();
        eprintln!(
            "  chunk {}: {} symbols, {} pages, {} trades ({:.1}s)",
            i + 1,
            chunk.len(),
            pager.pages_fetched(),
            trades.len(),
            started.elapsed().as_secs_f64()
        );
        for t in trades {
            if !trade_ids.insert(t.trade_id) {
                continue;
            }
            markets_hit.insert(t.symbol.clone());
            let notional = i128::from(t.price) * i128::from(t.size);
            let m = wallets.entry(t.maker).or_default();
            m.maker += 1;
            m.notional += notional;
            let k = wallets.entry(t.taker).or_default();
            k.taker += 1;
            k.notional += notional;
        }
    }

    let makers = wallets.values().filter(|w| w.maker > 0).count();
    let takers = wallets.values().filter(|w| w.taker > 0).count();
    println!(
        "last {days} days ({} .. {}): {} trades in {} markets, {} pages",
        utc(cutoff),
        utc(now_ms),
        trade_ids.len(),
        markets_hit.len(),
        pages
    );
    println!(
        "distinct wallets: {} ({makers} as maker, {takers} as taker)",
        wallets.len()
    );
    let five_plus = wallets.values().filter(|w| w.maker + w.taker >= 5).count();
    println!("wallets with >= 5 trades: {five_plus}");
    if days > 3 {
        println!(
            "note: markets resolved more than 3 days ago are not walked (/markets/resolved looks back at most 3d)"
        );
    }
    let mut rows: Vec<_> = wallets.into_iter().collect();
    rows.sort_by(|a, b| {
        (b.1.maker + b.1.taker)
            .cmp(&(a.1.maker + a.1.taker))
            .then_with(|| a.0.cmp(&b.0))
    });
    println!(
        "\n{:>4}  {:<44}  {:>7}  {:>7}  {:>7}  {:>14}",
        "#", "owner", "trades", "maker", "taker", "notional USD"
    );
    for (i, (owner, w)) in rows.iter().take(top).enumerate() {
        println!(
            "{:>4}  {:<44}  {:>7}  {:>7}  {:>7}  {:>14.2}",
            i + 1,
            owner,
            w.maker + w.taker,
            w.maker,
            w.taker,
            w.notional as f64 / 1e6
        );
    }
    Ok(())
}

async fn history(client: &Client, owner: &str) -> Result<()> {
    let r = client.account_history(owner).await?;
    println!(
        "{owner}  seq {:?}  state_time {}",
        r.seq,
        r.state_time_ms.map_or("-".into(), utc)
    );
    println!(
        "\n{:<9} {:>6}  {:<17}  {:<17}  {:>14}  {:>14}  {:>14}  {:>14}",
        "timeframe",
        "points",
        "first (UTC)",
        "last (UTC)",
        "account value",
        "total PnL",
        "trading PnL",
        "Δ trading PnL"
    );
    for tf in Timeframe::ALL {
        let Some(s) = r.data.timeframe(tf) else {
            println!("{:<9} missing", tf.as_str());
            continue;
        };
        let pnl = &s.trading_pnl_history;
        let (first, last) = (pnl.first(), pnl.last());
        let last_of = |v: &[pascal_rs::HistoryPoint]| v.last().map_or(0, |p| p.value);
        println!(
            "{:<9} {:>6}  {:<17}  {:<17}  {:>14}  {:>14}  {:>14}  {:>14}",
            tf.as_str(),
            pnl.len(),
            first.map_or("-".into(), |p| utc(p.ts_ms)),
            last.map_or("-".into(), |p| utc(p.ts_ms)),
            usd(last_of(&s.account_value_history)),
            usd(last_of(&s.total_pnl_history)),
            usd(last_of(pnl)),
            usd(last.map_or(0, |p| p.value) - first.map_or(0, |p| p.value)),
        );
    }
    Ok(())
}

/// Depth window of `books`, in ticks from the best price (inclusive).
const DEPTH_TICKS: i64 = 5;
/// Pascal's smallest tick (0.001), used when a market's tick parameters are unknown.
const MIN_TICK: MicroUsd = 1_000;

async fn books(client: &Client, symbols: &[String]) -> Result<()> {
    let r = client.books(symbols).await?;
    // Tick parameters: the book's `spec` when present, else the market from
    // `/markets/search` (one small POST, resolved markets included), else the 0.001 minimum.
    let mut ticks: HashMap<String, TickRule> = r
        .data
        .books
        .iter()
        .filter_map(|(s, b)| Some((s.clone(), TickRule::from_spec(b.spec.as_ref()?).ok()?)))
        .collect();
    let missing: Vec<String> = symbols
        .iter()
        .filter(|s| !ticks.contains_key(*s))
        .cloned()
        .collect();
    if !missing.is_empty() {
        let search = MarketSearch {
            symbols: missing,
            ..MarketSearch::default()
        };
        match client.search_markets_lenient(&search).await {
            Ok(m) => {
                for f in &m.data.failures {
                    eprintln!("warning: market[{}] did not decode: {}", f.index, f.error);
                }
                for m in m.data.values() {
                    match TickRule::from_market(m) {
                        Ok(rule) => {
                            ticks.insert(m.symbol.clone(), rule);
                        }
                        Err(e) => eprintln!("warning: {}: {e}", m.symbol),
                    }
                }
            }
            Err(e) => eprintln!("warning: market search failed, using the 0.001 tick: {e}"),
        }
    }

    println!(
        "seq {:?}  state_time {}  depth = best price and {DEPTH_TICKS} ticks beyond it",
        r.seq,
        r.state_time_ms.map_or("-".into(), utc)
    );
    println!(
        "\n{:<34}  {:>8}  {:>8}  {:>8}  {:>9}  {:>11}  {:>9}  {:>11}",
        "symbol", "tick", "bid", "ask", "bid ct", "bid USD", "ask ct", "ask USD"
    );
    for sym in symbols {
        let Some(book) = r.data.books.get(sym) else {
            println!("{sym:<34}  no book");
            continue;
        };
        let params = ticks.get(sym).copied();
        let tick_of = |p: MicroUsd| params.map_or(MIN_TICK, |rule| rule.tick_at(p));
        let bid = book.bids.first().map(|l| l.price);
        let ask = book.asks.first().map(|l| l.price);
        // Bids: price >= best - 5 ticks; asks: price <= best + 5 ticks.
        let (bid_ct, bid_n) = bid.map_or((0, 0), |b| {
            depth(&book.bids, |p| p >= b - DEPTH_TICKS * tick_of(b))
        });
        let (ask_ct, ask_n) = ask.map_or((0, 0), |a| {
            depth(&book.asks, |p| p <= a + DEPTH_TICKS * tick_of(a))
        });
        let tick = bid.or(ask).map_or(MIN_TICK, tick_of);
        let price = |p: Option<MicroUsd>| p.map_or("-".into(), format_micros);
        println!(
            "{:<34}  {:>8}  {:>8}  {:>8}  {:>9}  {:>11}  {:>9}  {:>11}",
            sym,
            format!(
                "{}{}",
                format_micros(tick),
                if params.is_none() { "*" } else { "" }
            ),
            price(bid),
            price(ask),
            bid_ct,
            usd_wide(bid_n),
            ask_ct,
            usd_wide(ask_n),
        );
    }
    if symbols.iter().any(|s| !ticks.contains_key(s)) {
        println!("* market tick unknown, 0.001 assumed");
    }
    Ok(())
}

/// Total contracts and notional (Σ price × size, micro-USD) of the levels whose price passes `keep`.
fn depth(levels: &[Level], keep: impl Fn(MicroUsd) -> bool) -> (u64, i128) {
    levels
        .iter()
        .filter(|l| keep(l.price))
        .fold((0, 0), |(ct, n), l| {
            (ct + l.size, n + i128::from(l.price) * i128::from(l.size))
        })
}

/// Builds and signs the order described by `args`.
fn build_signed(args: &OrderArgs) -> Result<Signed<PlaceOrderRequest>> {
    let bad =
        |what: &str, e: &dyn std::fmt::Display| Error::InvalidArgument(format!("{what}: {e}"));
    let (signer, default_owner) = match (&args.key_file, args.test_key) {
        (Some(path), false) => (TradingSigner::from_file(path)?, None),
        (None, true) => (
            TradingSigner::from_seed(&[8; 32]),
            Some(TradingSigner::from_seed(&[7; 32]).pubkey()),
        ),
        _ => {
            return Err(Error::InvalidArgument(
                "give --key-file <path> or --test-key".into(),
            ));
        }
    };
    let owner = match (&args.owner, default_owner) {
        (Some(o), _) => signing::decode_pubkey(o, "owner")?,
        (None, Some(o)) => o,
        (None, None) => return Err(Error::InvalidArgument("--owner is required".into())),
    };
    let price = parse_micros(&args.price).map_err(|e| bad("--price", &e))?;
    if let (Some(min), Some(sf)) = (&args.tick_size_min, args.tick_sig_figs) {
        let min = parse_micros(min).map_err(|e| bad("--tick-size-min", &e))?;
        let rule = TickRule::new(min, sf).map_err(|e| bad("tick rule", &e))?;
        rule.validate(price).map_err(|e| bad("--price", &e))?;
    }
    let side = match args.side {
        SideArg::Bid => Side::Bid,
        SideArg::Ask => Side::Ask,
    };
    let mut order = PlaceOrder::limit(&args.symbol, side, price, args.size, args.client_order_id);
    order.tif = match args.tif {
        TifArg::Gtc => TimeInForce::Gtc,
        TifArg::Gtt => TimeInForce::Gtt,
        TifArg::Ioc => TimeInForce::Ioc,
    };
    order.expires_ts_ms = args.expires_ts_ms;
    order.post_only = args.post_only;
    order.reduce_only = args.reduce_only;
    if let Some(b) = &args.builder {
        let rate =
            parse_micros(&args.builder_fee_rate).map_err(|e| bad("--builder-fee-rate", &e))?;
        order.builder = Some(BuilderInfo {
            pubkey: signing::decode_pubkey(b, "builder")?,
            fee_rate_u: u32::try_from(rate).map_err(|e| bad("--builder-fee-rate", &e))?,
        });
    }
    let mut auth = AuthParams::new(owner, args.client_ts_ms.unwrap_or_else(signing::now_ms))
        .recv_window_ms(args.recv_window_ms);
    auth.deployment_id = args.deployment_id;
    Ok(signer.sign_place_order(&auth, &order)?)
}

/// The `POST /api/v1/orders` body, in Pascal's field order.
fn signed_body(signed: &Signed<PlaceOrderRequest>) -> String {
    serde_json::to_string_pretty(&[&signed.request]).expect("request serialises")
}

/// Permit bytes, signature and how to verify them, for stderr.
fn signed_notes(signed: &Signed<PlaceOrderRequest>, sent: bool) -> String {
    format!(
        "permit_hex {}\nsignature  {}\nsigner     {}\nverify     pascal-cli verify-order <body file> --signer {}\nsent       {sent}",
        signed.permit_hex(),
        signed.signature_base58(),
        signed.request.auth.signer,
        signed.request.auth.signer,
    )
}

/// Prints the body on stdout (pipe it to `verify-order`) and the permit notes on stderr.
fn sign_order(args: &OrderArgs) -> Result<()> {
    let signed = build_signed(args)?;
    println!("{}", signed_body(&signed));
    eprintln!("{}", signed_notes(&signed, false));
    Ok(())
}

fn verify_order(
    file: &str,
    signer: &str,
    builder: Option<&str>,
    builder_fee_rate: Option<&str>,
    deployment_id: u8,
    brief: bool,
) -> Result<()> {
    let text = if file == "-" {
        std::io::read_to_string(std::io::stdin())
    } else {
        std::fs::read_to_string(file)
    }
    .map_err(|e| Error::InvalidArgument(format!("cannot read {file}: {e}")))?;
    let expect = Expected::parse(signer, builder, builder_fee_rate)?;
    let (report, failed) = verify_text(&text, &expect, deployment_id, brief)?;
    print!("{report}");
    if failed > 0 {
        return Err(Error::InvalidArgument(format!(
            "{failed} request(s) failed verification"
        )));
    }
    Ok(())
}

/// What `verify-order` requires of every request.
struct Expected {
    signer: signing::Pubkey,
    builder: Option<signing::Pubkey>,
    builder_fee_rate_u: Option<u32>,
}

impl Expected {
    fn parse(signer: &str, builder: Option<&str>, fee_rate: Option<&str>) -> Result<Self> {
        let builder_fee_rate_u = fee_rate
            .map(|r| {
                parse_micros(r)
                    .ok()
                    .and_then(|u| u32::try_from(u).ok())
                    .ok_or_else(|| Error::InvalidArgument(format!("--builder-fee-rate {r:?}")))
            })
            .transpose()?;
        Ok(Expected {
            signer: signing::decode_pubkey(signer, "signer")?,
            builder: builder
                .map(|b| signing::decode_pubkey(b, "builder"))
                .transpose()?,
            builder_fee_rate_u,
        })
    }

    /// Checks a verified request against the expectations; the error names the mismatch.
    fn check(&self, v: &signing::Verified<PlaceOrder>) -> std::result::Result<(), String> {
        if v.header.signer != self.signer {
            return Err(format!(
                "signed by {}, expected trading key {}",
                signing::base58(&v.header.signer),
                signing::base58(&self.signer)
            ));
        }
        // The permit must decode back to the same order and header (byte-level round trip).
        match signing::decode_place_order_permit(&v.permit) {
            Ok((h, o)) if h == v.header && o == v.body => {}
            Ok(_) => return Err("permit bytes do not decode back to the request".into()),
            Err(e) => return Err(format!("permit bytes do not decode: {e}")),
        }
        let got = v.body.builder;
        if let Some(want) = self.builder
            && got.map(|b| b.pubkey) != Some(want)
        {
            return Err(format!(
                "builder {}, expected {}",
                got.map_or("none".into(), |b| signing::base58(&b.pubkey)),
                signing::base58(&want)
            ));
        }
        if let Some(want) = self.builder_fee_rate_u
            && got.map(|b| b.fee_rate_u) != Some(want)
        {
            return Err(format!(
                "builder fee rate {}, expected {}",
                got.map_or("none".into(), |b| format_micros(i64::from(b.fee_rate_u))),
                format_micros(i64::from(want))
            ));
        }
        Ok(())
    }
}

/// Verifies every request in `text` (a body array or one request object). Returns the report
/// and the number of requests that failed.
fn verify_text(
    text: &str,
    expect: &Expected,
    deployment_id: u8,
    brief: bool,
) -> Result<(String, usize)> {
    use std::fmt::Write as _;
    // A body array (what sign-order prints) or a single request.
    let value: serde_json::Value = serde_json::from_str(text.trim_start())
        .map_err(|e| Error::InvalidArgument(format!("not JSON: {e}")))?;
    let body = match value {
        serde_json::Value::Array(_) => value,
        other => serde_json::Value::Array(vec![other]),
    };
    let bytes = serde_json::to_vec(&body).expect("JSON value serialises");
    let results = signing::verify_place_order_body(&bytes, deployment_id)?;
    let mut out = String::new();
    let mut failed = 0;
    for (i, r) in results.into_iter().enumerate() {
        let r = r
            .map_err(|e| format!("{e} ({})", e.api_code().as_str()))
            .and_then(|v| expect.check(&v).map(|()| v));
        match r {
            Ok(v) => {
                let _ = writeln!(
                    out,
                    "[{i}] OK  signature verifies for trading key {} (expected)\n    owner {}  {} {:?} {} @ {} {:?}  client_order_id {}  builder {}",
                    signing::base58(&v.header.signer),
                    signing::base58(&v.header.owner),
                    v.body.symbol,
                    v.body.side,
                    v.body.size,
                    format_micros(v.body.price),
                    v.body.tif,
                    v.body.client_order_id,
                    v.body.builder.map_or("-".into(), |b| format!(
                        "{} @ {}",
                        signing::base58(&b.pubkey),
                        format_micros(i64::from(b.fee_rate_u))
                    )),
                );
                if !brief {
                    out.push_str(&permit_table(&v.permit, deployment_id));
                }
            }
            Err(e) => {
                failed += 1;
                let _ = writeln!(out, "[{i}] FAIL {e}");
            }
        }
    }
    let _ = writeln!(
        out,
        "{} request(s), {} verified, {failed} failed",
        body.as_array().map_or(0, Vec::len),
        body.as_array().map_or(0, Vec::len) - failed
    );
    Ok((out, failed))
}

/// How a permit field is shown.
#[derive(Clone, Copy)]
enum Field {
    U8,
    U32,
    U64,
    /// u64 micro-USD.
    Price,
    /// u64, `u64::MAX` = omitted.
    Replace,
    Key,
    Symbol,
    /// i32 LE, 6 decimals.
    FeeRate,
    Side,
    OrderType,
    Tif,
    Flags,
    Raw,
}

/// The place-order permit, field by field: (name, offset, length, kind, note). Offsets are
/// `signing::layout`; the notes say where the docs and the on-chain IDL disagree (D-003).
fn permit_fields() -> [(&'static str, usize, usize, Field, &'static str); 23] {
    use signing::layout as l;
    [
        ("version", l::VERSION, 1, Field::U8, ""),
        ("cmd_type", l::CMD_TYPE, 1, Field::U8, "1 = place order"),
        (
            "deployment_id",
            l::DEPLOYMENT_ID,
            1,
            Field::U8,
            "3 = production",
        ),
        ("padding0", l::PADDING0, 1, Field::Raw, ""),
        ("recv_window_ms", l::RECV_WINDOW_MS, 4, Field::U32, ""),
        ("client_ts_ms", l::CLIENT_TS_MS, 8, Field::U64, ""),
        ("owner", l::OWNER, 32, Field::Key, ""),
        ("signer", l::SIGNER, 32, Field::Key, "trading key"),
        ("size", l::SIZE, 8, Field::U64, "contracts"),
        ("price", l::PRICE, 8, Field::Price, ""),
        ("client_order_id", l::CLIENT_ORDER_ID, 8, Field::U64, ""),
        (
            "replace_client_order_id",
            l::REPLACE_CLIENT_ORDER_ID,
            8,
            Field::Replace,
            "",
        ),
        ("expire_ts_ms", l::EXPIRE_TS_MS, 8, Field::U64, "0 = none"),
        (
            "builder_pubkey",
            l::BUILDER_PUBKEY,
            32,
            Field::Key,
            "docs: reserved (zeros); on-chain IDL: builderPubkey",
        ),
        ("symbol", l::SYMBOL, 32, Field::Symbol, "NUL-padded ASCII"),
        (
            "builder_fee_rate",
            l::BUILDER_FEE_RATE,
            4,
            Field::FeeRate,
            "docs: reserved (zeros); on-chain IDL: builderFeeRate, i32 LE, 6 d.p.",
        ),
        (
            "offchain_fields_digest",
            l::OFFCHAIN_FIELDS_DIGEST,
            32,
            Field::Raw,
            "fixed constant",
        ),
        (
            "metadata",
            l::METADATA,
            1,
            Field::U8,
            "docs: reserved (zero); on-chain IDL: metadata",
        ),
        ("side", l::SIDE, 1, Field::Side, ""),
        ("order_type", l::ORDER_TYPE, 1, Field::OrderType, ""),
        ("tif", l::TIF, 1, Field::Tif, ""),
        (
            "flags",
            l::FLAGS,
            1,
            Field::Flags,
            "1 post_only, 2 reduce_only, 4, 8",
        ),
        ("padding1", l::PADDING1, 7, Field::Raw, ""),
    ]
}

/// Every field of a verified 232-byte place-order permit: offsets, raw bytes, decoded value.
fn permit_table(permit: &[u8], deployment_id: u8) -> String {
    use std::fmt::Write as _;
    let mut out = format!(
        "    permit: {} bytes, deployment {deployment_id}, the exact message the Ed25519 signature covers\n    {:<9} {:<23} {:<64}  value\n",
        permit.len(),
        "offset",
        "field",
        "bytes (hex)"
    );
    for (name, at, len, kind, note) in permit_fields() {
        let b = &permit[at..at + len];
        let u = |n: usize| {
            let mut x = [0u8; 8];
            x[..n].copy_from_slice(&b[..n]);
            u64::from_le_bytes(x)
        };
        let value = match kind {
            Field::U8 | Field::U32 | Field::U64 => u(len).to_string(),
            Field::Price => format!("{} ({} micro-USD)", format_micros(u(8) as i64), u(8)),
            Field::Replace if u(8) == u64::MAX => "omitted (u64::MAX)".into(),
            Field::Replace => u(8).to_string(),
            Field::Key if b.iter().all(|&x| x == 0) => "zero (none)".into(),
            Field::Key => signing::base58(b),
            Field::Symbol => String::from_utf8_lossy(b).trim_end_matches('\0').to_owned(),
            Field::FeeRate => {
                let r = i32::from_le_bytes(b.try_into().expect("4 bytes"));
                format!("{r} = {}", format_micros(i64::from(r)))
            }
            Field::Side => ["bid", "ask"]
                .get(usize::from(b[0]))
                .unwrap_or(&"?")
                .to_string(),
            Field::OrderType => ["limit", "market"]
                .get(usize::from(b[0]))
                .unwrap_or(&"?")
                .to_string(),
            Field::Tif => ["gtc", "gtt", "ioc"]
                .get(usize::from(b[0]))
                .unwrap_or(&"?")
                .to_string(),
            Field::Flags => format!("{:#04x}", b[0]),
            Field::Raw => String::new(),
        };
        let range = if len == 1 {
            at.to_string()
        } else {
            format!("{at}..{}", at + len)
        };
        let note = if note.is_empty() {
            String::new()
        } else {
            format!("  [{note}]")
        };
        let line = format!(
            "    {range:<9} {name:<23} {:<64}  {value}{note}",
            signing::to_hex(b)
        );
        let _ = writeln!(out, "{}", line.trim_end());
    }
    out
}

async fn place_order(args: &OrderArgs, trade_url: &str, i_am_authorized: bool) -> Result<()> {
    if !i_am_authorized {
        return Err(Error::InvalidArgument(
            "place-order sends a real order. Refusing without --i-am-authorized: Pascal's Terms \
             restrict trading from Italy (decisions D-001, D-007). Use sign-order to sign without \
             sending."
                .into(),
        ));
    }
    if args.test_key {
        return Err(Error::InvalidArgument(
            "place-order does not send orders signed with the public test key".into(),
        ));
    }
    let signed = build_signed(args)?;
    let policy = match &signed.request.builder_info {
        Some(_) => BuilderPolicy::Required,
        None => BuilderPolicy::Any,
    };
    let trade = TradeClient::builder(trade_url)
        .builder_policy(policy)
        .build()?;
    eprintln!(
        "sending to {trade_url}:\n{}\n{}",
        signed_body(&signed),
        signed_notes(&signed, true)
    );
    let results = trade.place_orders(&[signed.request]).await?;
    for r in results {
        match r {
            Ok(resp) => println!("{:#}", resp.raw),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn usd_wide(micros: i128) -> String {
    format!("{:.2}", micros as f64 / 1e6)
}

fn usd(micros: MicroUsd) -> String {
    format!("{:.2}", micros as f64 / 1e6)
}

/// `YYYY-MM-DD HH:MM` in UTC from Unix ms.
fn utc(ms: u64) -> String {
    let secs = ms / 1000;
    let days = (secs / 86_400) as i64;
    let (h, m) = ((secs % 86_400) / 3600, (secs % 3600) / 60);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(mo <= 2);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn utc_formats() {
        assert_eq!(super::utc(0), "1970-01-01 00:00");
        assert_eq!(super::utc(1_793_145_600_000), "2026-10-28 00:00");
        assert_eq!(super::utc(951_782_400_000), "2000-02-29 00:00");
    }

    fn test_args() -> super::OrderArgs {
        super::OrderArgs {
            symbol: "NBA_26OCT28_PHIIND_ML.PHI".into(),
            side: super::SideArg::Bid,
            price: "0.715".into(),
            size: 14,
            client_order_id: 7,
            tif: super::TifArg::Ioc,
            expires_ts_ms: None,
            post_only: false,
            reduce_only: false,
            builder: Some("J2xccRtuG43drESLYznHhLhQkLTdfepcKYbiQ9BsJVaf".into()),
            builder_fee_rate: "0.001".into(),
            owner: None,
            key_file: None,
            test_key: true,
            client_ts_ms: Some(1_793_213_000_123),
            recv_window_ms: 3_000,
            deployment_id: 3,
            tick_size_min: Some("0.001".into()),
            tick_sig_figs: Some(3),
        }
    }

    #[test]
    fn sign_order_reproduces_the_js_reference_vector() {
        // fixtures/pascal/signing/vectors.json, case ioc_bid_builder_0_1pct.
        let signed = super::build_signed(&test_args()).unwrap();
        assert_eq!(
            signed.signature_base58(),
            "267asXzKMvTijvi62h1YEZUPmqzhjFaV7XAjpKQkCB58MAXVyi3hNgU5CwtEVTrcfp5D5gM5JxQSRN4RoUtE1cMd"
        );
        // The printed body verifies and keeps Pascal's field order.
        let body = super::signed_body(&signed);
        assert!(
            body.trim_start()
                .starts_with("[\n  {\n    \"client_order_id\": \"7\"")
        );
        let results = pascal_rs::signing::verify_place_order_body(body.as_bytes(), 3).unwrap();
        let ok = results.into_iter().next().unwrap().unwrap();
        assert_eq!(ok.body.builder.unwrap().fee_rate_u, 1_000);
        assert!(super::signed_notes(&signed, false).ends_with("sent       false"));
        // With 2 significant figures 1 - 0.715 = 0.285 has a 0.01 tick, so 0.715 is off grid;
        // 0.7155 is off grid with 3 as well.
        let mut off = test_args();
        off.tick_sig_figs = Some(2);
        assert!(super::build_signed(&off).is_err());
        off.tick_sig_figs = Some(3);
        off.price = "0.7155".into();
        assert!(super::build_signed(&off).is_err());
    }

    const BUILDER: &str = "J2xccRtuG43drESLYznHhLhQkLTdfepcKYbiQ9BsJVaf";

    fn expected(signer: &str) -> super::Expected {
        super::Expected::parse(signer, Some(BUILDER), Some("0.001")).unwrap()
    }

    fn trading_key() -> String {
        pascal_rs::TradingSigner::from_seed(&[8; 32]).pubkey_base58()
    }

    /// Runs `verify-order` on `body` and returns (report, failed).
    fn verify(body: &str, signer: &str) -> (String, usize) {
        super::verify_text(body, &expected(signer), 3, false).unwrap()
    }

    /// The body with one value of its single request replaced.
    fn tweak(body: &str, f: impl FnOnce(&mut serde_json::Value)) -> String {
        let mut v: serde_json::Value = serde_json::from_str(body).unwrap();
        f(&mut v[0]);
        v.to_string()
    }

    fn flip_b58(s: &str, at: usize) -> String {
        let mut raw = bs58::decode(s).into_vec().unwrap();
        raw[at] ^= 0x01;
        bs58::encode(raw).into_string()
    }

    #[test]
    fn verify_order_passes_a_valid_request_and_prints_the_builder_bytes() {
        let signed = super::build_signed(&test_args()).unwrap();
        let body = super::signed_body(&signed);
        let (report, failed) = verify(&body, &trading_key());
        assert_eq!(failed, 0, "{report}");
        assert!(report.contains("[0] OK  signature verifies"), "{report}");
        for want in [
            "120..152  builder_pubkey".to_owned(),
            BUILDER.to_owned(),
            "184..188  builder_fee_rate        e8030000".to_owned(),
            "1000 = 0.001000".to_owned(),
            "220       metadata                00".to_owned(),
            "1 request(s), 1 verified, 0 failed".to_owned(),
        ] {
            assert!(report.contains(&want), "missing {want:?} in\n{report}");
        }
        // The table's bytes are the signed permit's.
        let builder_hex = pascal_rs::signing::to_hex(&signed.permit[120..152]);
        assert!(report.contains(&builder_hex));
        // A single request object (not an array) is accepted too.
        let one = serde_json::to_string(&signed.request).unwrap();
        assert_eq!(verify(&one, &trading_key()).1, 0);
    }

    #[test]
    fn verify_order_fails_on_any_flipped_byte_or_mismatch() {
        let signed = super::build_signed(&test_args()).unwrap();
        let body = super::signed_body(&signed);
        let key = trading_key();
        let sig = signed.request.auth.signature.clone();
        let bad: Vec<(&str, String)> = vec![
            (
                "signature byte",
                tweak(&body, |r| {
                    r["auth"]["signature"] = flip_b58(&sig, 10).into()
                }),
            ),
            (
                "builder byte",
                tweak(&body, |r| {
                    r["builder_info"]["builder_pubkey"] = flip_b58(BUILDER, 0).into()
                }),
            ),
            (
                "fee rate",
                tweak(&body, |r| {
                    r["builder_info"]["builder_fee_rate"] = "0.002000".into()
                }),
            ),
            ("price", tweak(&body, |r| r["price"] = "0.716000".into())),
            ("size", tweak(&body, |r| r["size"] = "15".into())),
            (
                "builder removed",
                tweak(&body, |r| {
                    r.as_object_mut().unwrap().remove("builder_info");
                }),
            ),
        ];
        for (what, text) in &bad {
            let (report, failed) = verify(text, &key);
            assert_eq!(failed, 1, "{what}: {report}");
            assert!(report.contains("[0] FAIL"), "{what}: {report}");
        }
        // The wrong expected trading key.
        let other = pascal_rs::TradingSigner::from_seed(&[10; 32]).pubkey_base58();
        assert_eq!(verify(&body, &other).1, 1);
        // A correctly signed order naming another builder fails the --builder check.
        let mut args = test_args();
        args.builder = Some(other.clone());
        let other_body = super::signed_body(&super::build_signed(&args).unwrap());
        let (report, failed) = verify(&other_body, &key);
        assert_eq!(failed, 1);
        assert!(report.contains("expected J2xcc"), "{report}");
        // In a batch, each request stands alone.
        let mut both: Vec<serde_json::Value> = serde_json::from_str(&body).unwrap();
        both.extend(serde_json::from_str::<Vec<serde_json::Value>>(&bad[3].1).unwrap());
        let (report, failed) = verify(&serde_json::to_string(&both).unwrap(), &key);
        assert_eq!(failed, 1);
        assert!(report.contains("[0] OK") && report.contains("[1] FAIL"));
        // Every single-bit flip anywhere in the 232 permit bytes breaks the signature.
        let signer = pascal_rs::signing::decode_pubkey(&key, "signer").unwrap();
        for i in 0..signed.permit.len() {
            let mut p = signed.permit.clone();
            p[i] ^= 0x01;
            assert!(
                pascal_rs::signing::verify_signature(&p, &signed.signature, &signer).is_err(),
                "byte {i}"
            );
        }
        // Not JSON at all is an error, not a pass.
        assert!(super::verify_text("[{", &expected(&key), 3, true).is_err());
    }

    #[test]
    fn place_order_refuses_without_the_flag() {
        // An unroutable URL: the refusal must come before any connection attempt.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let e = rt
            .block_on(super::place_order(
                &test_args(),
                "http://127.0.0.1:9",
                false,
            ))
            .unwrap_err();
        assert!(e.to_string().contains("--i-am-authorized"), "{e}");
    }

    #[test]
    fn depth_sums_kept_levels() {
        use pascal_rs::Level;
        let l = |price, size| Level { price, size };
        let levels = [l(540_000, 10), l(536_000, 5), l(534_000, 100)];
        assert_eq!(
            super::depth(&levels, |p| p >= 535_000),
            (15, 540_000 * 10 + 536_000 * 5)
        );
    }
}
