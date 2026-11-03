//! `crank`: operator commands for the schedina program (plan 2.C): `init-config`, `fund-vault`,
//! `faucet`, `status`, (4.A, D-121) `pause` / `unpause`, (5.0a) `amm-setup` for the mock AMM
//! beside it, and (7.0c) `amm-peg`. Localnet and devnet only: a URL containing "mainnet" or an endpoint with
//! mainnet-beta's genesis hash is refused before anything is read or sent (D-016). RPC URLs are
//! printed without their query string. Keys are files outside the repo (D-015).
//!
//! ```text
//! crank [--rpc URL | --url URL|devnet|localnet] [--program-id ID] [--authority PATH] <command> [options]
//!   init-config [--crank KEY] [--quote-signer KEY] [--max-legs 5] [--max-payout-usdc 10000]
//!               [--min-stake-usdc 1] [--reserve-usdc 100] [--copy-fee-bps 200]
//!               [--tipster-share-bps 8000]           signed by the upgrade authority
//!   faucet --to KEY [--usdc 1000] [--payer PATH]     mock USDC into KEY's ATA (devnet build)
//!   fund-vault --usdc N [--from PATH]                from PATH's mock-USDC ATA (default authority)
//!   status                                           config, vault, live pendings and tickets
//!   pause | unpause                                  Config.paused, signed by the authority;
//!                                                    the desk's indexer mirrors it (D-121)
//!   amm-setup [--amm-program-id ID] [--pool-usdc 2000]
//!                                                    the mock AMM (L4-3): AmmConfig, mockBONK
//!                                                    and mockSOL, a pool each against the
//!                                                    mock USDC, liquidity, the lookup table;
//!                                                    signed by the authority; idempotent
//!   amm-peg (--target-eur-per-sol X [--eur-per-usd Y | --desk URL] | --from-fx [--desk URL])
//!           [--amm-program-id ID] [--yes]
//!                                                    swaps the mockSOL pool toward the target
//!                                                    USD price per mockSOL (EUR/SOL ÷ EUR/USD);
//!                                                    admin only; dry run without --yes
//! ```
//! `KEY` is a base58 public key or a keypair file. Defaults come from the desk's variables:
//! `SOLANA_RPC_URL` (else localnet), `PROGRAM_ID`, `AUTHORITY_KEYPAIR_PATH`,
//! `CRANK_KEYPAIR_PATH`, `QUOTE_SIGNER_KEYPAIR_PATH`, `SCHEDINA_KEYS_DIR`, `MOCK_AMM_PROGRAM_ID`.
//!
//! `amm-setup` prints one `tx <step> <signature>` line per transaction it sends and ends with
//! `amm-setup: <n> transactions sent` or `amm-setup: nothing sent`; a re-run on a set-up cluster
//! reads only. The pools' mock USDC comes from schedina's `faucet_usdc` (claimed to the
//! authority's ATA, only what its balance lacks); the schedina vault is never touched.
//!
//! `amm-peg` (V-3, D-436; l7-contracts L7-1) moves the mockSOL pool's price (USDC reserve ÷
//! mockSOL reserve) to the real SOL price, so paying in mockSOL matches the app's ≈ SOL figure.
//! The target in USD per mockSOL is EUR per SOL ÷ EUR per USD: `--from-fx` reads both from the
//! desk's `GET /fx` (`--desk`, default `DESK_URL` or `http://127.0.0.1:8787`); with
//! `--target-eur-per-sol` the EUR/USD rate is `--eur-per-usd`, else the desk's, else the ECB
//! fallback. It refuses unless the authority is `AmmConfig.admin`. Within 1% of the target it
//! sends nothing (`amm-peg: nothing sent`), so it is idempotent. Otherwise it plans one swap on
//! the exact pool arithmetic ([`desk::swap::math::swap_out`]) that lands on the target, bounded
//! to 10% of the input side's reserve per run (a far target takes several runs), and prints it;
//! only `--yes` sends it, in one transaction with the admin's faucet for the side it pays in
//! (the mock-token faucet for mockSOL, schedina's `faucet_usdc` for USDC, each only what
//! the admin's balance lacks). Output is redacted like `amm-setup`'s: URLs without query strings,
//! no key material.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use desk::config::DEFAULT_PROGRAM_ID;
use desk::program::accounts::disc;
use desk::program::ix::{self, Params};
use desk::program::{
    BPF_UPGRADEABLE_LOADER_ID, ConfigAccount, Pdas, PendingAccount, TOKEN_PROGRAM_ID,
    TicketAccount, TicketState, VaultAccount, associated_token_address, consts,
};
use desk::solana::rpc::{RpcClient, redact};
use desk::solana::signer::{Keypair, send};
use desk::swap::accounts::{AmmConfigAccount, MockTokenAccount, PoolAccount};
use desk::swap::{self, AmmPdas};
use solana_pubkey::Pubkey;

const USAGE: &str = "usage: crank [--rpc URL | --url URL|devnet|localnet] [--program-id ID] [--authority PATH] \
<init-config|faucet|fund-vault|status|pause|unpause|amm-setup|amm-peg> [options] (see the source header)";

/// Parsed `--flag value` pairs and the command.
struct Args {
    command: String,
    flags: HashMap<String, String>,
}

/// Every flag any command reads. Anything else is an error rather than silently ignored (a
/// mistyped `--url` used to fall back to `SOLANA_RPC_URL` or localnet without a word).
const FLAGS: &[&str] = &[
    "rpc",
    "url",
    "program-id",
    "authority",
    "crank",
    "quote-signer",
    "max-legs",
    "max-payout-usdc",
    "min-stake-usdc",
    "reserve-usdc",
    "copy-fee-bps",
    "tipster-share-bps",
    "to",
    "usdc",
    "payer",
    "from",
    "amm-program-id",
    "pool-usdc",
    "target-eur-per-sol",
    "eur-per-usd",
    "desk",
    "from-fx",
    "yes",
];

/// Flags that take no value.
const SWITCHES: &[&str] = &["from-fx", "yes"];

/// `--url`/`--rpc` value: the Solana CLI's `devnet`/`localnet` monikers (`d`, `l`), else a URL.
/// Mainnet monikers are left as they are, so the "mainnet" check below refuses them.
fn resolve_url(v: &str) -> String {
    match v {
        "devnet" | "d" => "https://api.devnet.solana.com".to_owned(),
        "localnet" | "localhost" | "l" => "http://127.0.0.1:8899".to_owned(),
        "m" => "mainnet-beta".to_owned(),
        _ => v.to_owned(),
    }
}

fn parse(argv: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut command = None;
    let mut flags = HashMap::new();
    let mut it = argv.peekable();
    while let Some(a) = it.next() {
        if let Some(name) = a.strip_prefix("--") {
            if !FLAGS.contains(&name) {
                return Err(format!("unknown flag --{name}\n{USAGE}"));
            }
            let v = if SWITCHES.contains(&name) {
                "1".to_owned()
            } else {
                it.next().ok_or_else(|| format!("--{name} needs a value"))?
            };
            if flags.insert(name.to_owned(), v).is_some() {
                return Err(format!("--{name} given twice"));
            }
        } else if command.is_none() {
            command = Some(a);
        } else {
            return Err(format!("unexpected argument {a}"));
        }
    }
    Ok(Args {
        command: command.ok_or_else(|| USAGE.to_owned())?,
        flags,
    })
}

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

fn keys_dir() -> PathBuf {
    env("SCHEDINA_KEYS_DIR").map_or_else(
        || {
            env("HOME")
                .map_or_else(|| PathBuf::from("."), PathBuf::from)
                .join(".config/schedina")
        },
        PathBuf::from,
    )
}

impl Args {
    fn get(&self, k: &str) -> Option<&str> {
        self.flags.get(k).map(String::as_str)
    }

    fn path(&self, flag: &str, var: &str, default: &str) -> PathBuf {
        self.get(flag)
            .map(PathBuf::from)
            .or_else(|| env(var).map(PathBuf::from))
            .unwrap_or_else(|| keys_dir().join(default))
    }

    fn num<T: std::str::FromStr>(&self, k: &str, default: T) -> Result<T, String> {
        self.get(k).map_or(Ok(default), |v| {
            v.parse().map_err(|_| format!("--{k}: not a number: {v}"))
        })
    }

    /// Micro-USDC from a `--<k>` amount in USDC (up to 6 decimals).
    fn usdc(&self, k: &str, default: Option<u64>) -> Result<u64, String> {
        match self.get(k) {
            None => default.ok_or_else(|| format!("--{k} is required")),
            Some(v) => usdc_to_micro(v).ok_or_else(|| format!("--{k}: not an amount: {v}")),
        }
    }
}

fn usdc_to_micro(v: &str) -> Option<u64> {
    let (int, frac) = v.split_once('.').unwrap_or((v, ""));
    if frac.len() > 6 || (int.is_empty() && frac.is_empty()) {
        return None;
    }
    let int: u64 = if int.is_empty() { 0 } else { int.parse().ok()? };
    let frac: u64 = if frac.is_empty() {
        0
    } else {
        format!("{frac:0<6}").parse().ok()?
    };
    int.checked_mul(1_000_000)?.checked_add(frac)
}

fn micro_to_usdc(u: u64) -> String {
    format!("{}.{:06}", u / 1_000_000, u % 1_000_000)
}

/// A base58 public key, or the public key of a keypair file.
fn key_of(v: &str) -> Result<Pubkey, String> {
    if let Ok(k) = v.parse::<Pubkey>() {
        return Ok(k);
    }
    Ok(Keypair::read(Path::new(v))?.pubkey())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run(std::env::args().skip(1)).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("crank: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(argv: impl Iterator<Item = String>) -> Result<(), String> {
    let args = parse(argv)?;
    if args.get("rpc").is_some() && args.get("url").is_some() {
        return Err("give --rpc or --url, not both".to_owned());
    }
    let url = args
        .get("rpc")
        .or_else(|| args.get("url"))
        .map(resolve_url)
        .or_else(|| env("SOLANA_RPC_URL"))
        .unwrap_or_else(|| "http://127.0.0.1:8899".to_owned());
    let shown = redact(&url);
    if url.to_ascii_lowercase().contains("mainnet") {
        return Err(format!("refusing a mainnet RPC URL: {shown} (D-016)"));
    }
    let rpc = RpcClient::new(&url);
    rpc.refuse_mainnet()
        .await
        .map_err(|e| format!("{shown}: {e}"))?;
    let program: Pubkey = args
        .get("program-id")
        .map(str::to_owned)
        .or_else(|| env("PROGRAM_ID"))
        .unwrap_or_else(|| DEFAULT_PROGRAM_ID.to_owned())
        .parse()
        .map_err(|e| format!("program id: {e}"))?;
    let pdas = Pdas::new(program);
    let authority_path = args.path(
        "authority",
        "AUTHORITY_KEYPAIR_PATH",
        "upgrade-authority.json",
    );
    eprintln!("rpc {shown}; program {program}");

    match args.command.as_str() {
        "init-config" => {
            let authority = Keypair::read(&authority_path)?;
            let crank = key_of(
                &args
                    .path("crank", "CRANK_KEYPAIR_PATH", "crank.json")
                    .to_string_lossy(),
            )?;
            let quote_signer = key_of(
                &args
                    .path(
                        "quote-signer",
                        "QUOTE_SIGNER_KEYPAIR_PATH",
                        "quote-signer.json",
                    )
                    .to_string_lossy(),
            )?;
            let params = Params {
                crank,
                quote_signer,
                max_legs: args.num("max-legs", 5u8)?,
                max_payout: args.usdc("max-payout-usdc", Some(10_000_000_000))?,
                min_stake: args.usdc("min-stake-usdc", Some(1_000_000))?,
                reserve: args.usdc("reserve-usdc", Some(100_000_000))?,
                copy_fee_bps: args.num("copy-fee-bps", 200u16)?,
                tipster_share_bps: args.num("tipster-share-bps", 8_000u16)?,
            };
            if rpc
                .get_account(&pdas.config())
                .await
                .map_err(|e| e.to_string())?
                .is_some()
            {
                println!("config {} already exists; nothing sent", pdas.config());
                return Ok(());
            }
            let me = authority.pubkey();
            let sig = send(
                &rpc,
                &[ix::init_config(&pdas, &me, &me, &params)],
                &[&authority],
            )
            .await
            .map_err(|e| e.to_string())?;
            println!(
                "init_config {sig}\n  config {}\n  authority {me}\n  crank {crank}\n  quote_signer {quote_signer}\n  usdc_mint {}",
                pdas.config(),
                pdas.usdc_mint()
            );
        }
        "faucet" => {
            let to = key_of(args.get("to").ok_or("--to is required")?)?;
            let amount = args.usdc("usdc", Some(1_000_000_000))?;
            if amount > consts::FAUCET_MAX {
                return Err(format!(
                    "--usdc above the faucet cap of {}",
                    micro_to_usdc(consts::FAUCET_MAX)
                ));
            }
            let payer = Keypair::read(&args.get("payer").map_or(authority_path, PathBuf::from))?;
            let mint = pdas.usdc_mint();
            let sig = send(
                &rpc,
                &[
                    ix::create_ata_idempotent(&payer.pubkey(), &to, &mint),
                    ix::faucet_usdc(&pdas, &payer.pubkey(), &to, amount),
                ],
                &[&payer],
            )
            .await
            .map_err(|e| e.to_string())?;
            println!(
                "faucet {sig}: {} mock USDC to {}",
                micro_to_usdc(amount),
                associated_token_address(&to, &mint)
            );
        }
        "fund-vault" => {
            let amount = args.usdc("usdc", None)?;
            let from = Keypair::read(&args.get("from").map_or(authority_path, PathBuf::from))?;
            let ata = associated_token_address(&from.pubkey(), &pdas.usdc_mint());
            let sig = send(
                &rpc,
                &[ix::fund_vault(&pdas, &from.pubkey(), &ata, amount)],
                &[&from],
            )
            .await
            .map_err(|e| e.to_string())?;
            println!("fund_vault {sig}: {} mock USDC", micro_to_usdc(amount));
        }
        "status" => status(&rpc, &pdas).await?,
        cmd @ ("pause" | "unpause") => {
            set_paused(&rpc, &pdas, &authority_path, cmd == "pause").await?;
        }
        "amm-setup" => {
            let amm: Pubkey = args
                .get("amm-program-id")
                .map(str::to_owned)
                .or_else(|| env("MOCK_AMM_PROGRAM_ID"))
                .unwrap_or_else(|| swap::DEFAULT_PROGRAM_ID.to_owned())
                .parse()
                .map_err(|e| format!("mock-amm program id: {e}"))?;
            let pool_usdc = args.usdc("pool-usdc", Some(2_000_000_000))?;
            let authority = Keypair::read(&authority_path)?;
            amm_setup(&rpc, &pdas, amm, &authority, pool_usdc).await?;
        }
        "amm-peg" => {
            let amm: Pubkey = args
                .get("amm-program-id")
                .map(str::to_owned)
                .or_else(|| env("MOCK_AMM_PROGRAM_ID"))
                .unwrap_or_else(|| swap::DEFAULT_PROGRAM_ID.to_owned())
                .parse()
                .map_err(|e| format!("mock-amm program id: {e}"))?;
            let target = peg_target(&args).await?;
            let authority = Keypair::read(&authority_path)?;
            amm_peg(
                &rpc,
                &pdas,
                amm,
                &authority,
                target,
                args.get("yes").is_some(),
            )
            .await?;
        }
        other => return Err(format!("unknown command {other}\n{USAGE}")),
    }
    Ok(())
}

/// `pause(paused)` signed by `Config.authority` (runbook "Pascal down"): blocks `open_pending`
/// and `finalize`; every exit stays open. Nothing is sent when the config already says so.
async fn set_paused(
    rpc: &RpcClient,
    pdas: &Pdas,
    authority_path: &Path,
    paused: bool,
) -> Result<(), String> {
    let acc = rpc
        .get_account(&pdas.config())
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no config at {} (run init-config)", pdas.config()))?;
    let cfg =
        ConfigAccount::decode(&acc.data, &acc.owner, &pdas.program).map_err(|e| e.to_string())?;
    if cfg.paused == paused {
        println!("paused {paused} already; nothing sent");
        return Ok(());
    }
    let authority = Keypair::read(authority_path)?;
    if authority.pubkey() != cfg.authority {
        return Err(format!(
            "{} is not the config authority {}",
            authority.pubkey(),
            cfg.authority
        ));
    }
    let sig = send(
        rpc,
        &[ix::pause(pdas, &authority.pubkey(), paused)],
        &[&authority],
    )
    .await
    .map_err(|e| e.to_string())?;
    println!("pause {sig}: paused {paused}");
    Ok(())
}

/// A mock mint `amm-setup` creates and pools: its seed symbol (`["mock", symbol]`), decimals,
/// faucet cap in whole tokens, and the seeding price as base units per micro-USDC.
struct MockMintSpec {
    symbol: &'static str,
    decimals: u8,
    faucet_max_tokens: u64,
    base_per_micro_usdc: u64,
}

/// In `swap::MOCK_SYMBOLS` order. mockBONK at 1,000 per USDC (the app's faucet tap of 50,000
/// is worth ~50 USDC); mockSOL at 200 USDC (its tap of 1 is ~190 USDC after impact).
const AMM_MINTS: [MockMintSpec; 2] = [
    MockMintSpec {
        symbol: "BONK",
        decimals: 5,
        faucet_max_tokens: 100_000,
        base_per_micro_usdc: 100,
    },
    MockMintSpec {
        symbol: "SOL",
        decimals: 9,
        faucet_max_tokens: 2,
        base_per_micro_usdc: 5,
    },
];

/// `v` base units of a `dec`-decimals token, as a decimal string.
fn units(v: u64, dec: u8) -> String {
    let d = 10u64.pow(u32::from(dec));
    if dec == 0 {
        return v.to_string();
    }
    format!("{}.{:0w$}", v / d, v % d, w = usize::from(dec))
}

/// Balance of SPL token account `key`; 0 when it does not exist.
async fn token_balance(rpc: &RpcClient, key: &Pubkey) -> Result<u64, String> {
    match rpc.get_account(key).await.map_err(|e| e.to_string())? {
        None => Ok(0),
        Some(a) if a.owner == TOKEN_PROGRAM_ID => swap::accounts::token_account(&a.data)
            .map(|(_, amount)| amount)
            .map_err(|e| format!("{key}: {}", e.0)),
        Some(_) => Err(format!("{key} is not an SPL token account")),
    }
}

/// `(deactivated, authority)` of an address lookup table (`LookupTableMeta`: type u32,
/// deactivation_slot u64, last_extended_slot u64, start index u8, `Option<Pubkey>` authority).
fn lookup_table_meta(d: &[u8]) -> Result<(bool, Option<Pubkey>), String> {
    if d.len() < 56 || d[..4] != 1u32.to_le_bytes() {
        return Err("not a lookup table".into());
    }
    let deactivated = d[4..12] != u64::MAX.to_le_bytes();
    let authority = (d[21] == 1).then(|| Pubkey::new_from_array(d[22..54].try_into().expect("32")));
    Ok((deactivated, authority))
}

/// `amm-setup`: everything the app's "pay in mockBONK" needs on a cluster where schedina runs
/// (the recipe of `tests/swap_chain.rs`). Each step reads first and sends only what is missing:
/// `init_amm`; per mock mint `init_mock_mint`, `init_pool` and, while the pool is empty, the
/// admin's mock-token faucet and the first `add_liquidity` (`pool_usdc` micro-USDC at the
/// spec's price); a schedina `faucet_usdc` claim to the authority for the USDC those deposits
/// lack; the lookup table of `[swap, verify, open_pending]` (created, extended and recorded
/// with `set_lookup_table` in one transaction, or extended with addresses it lacks).
async fn amm_setup(
    rpc: &RpcClient,
    s: &Pdas,
    amm: Pubkey,
    authority: &Keypair,
    pool_usdc: u64,
) -> Result<(), String> {
    let rpc_err = |e: desk::solana::rpc::RpcError| e.to_string();
    let a = authority.pubkey();
    let p = AmmPdas::new(amm, s);
    let usdc = s.usdc_mint();
    if pool_usdc == 0 {
        return Err("--pool-usdc must be positive".into());
    }
    // Schedina first: its mock USDC is every pool's quote mint.
    let cfg = rpc
        .get_account(&s.config())
        .await
        .map_err(rpc_err)?
        .ok_or_else(|| format!("no schedina config at {} (run init-config)", s.config()))?;
    let cfg =
        ConfigAccount::decode(&cfg.data, &cfg.owner, &s.program).map_err(|e| e.to_string())?;
    if cfg.usdc_mint != usdc {
        return Err(format!(
            "Config.usdc_mint is {}, expected {usdc}",
            cfg.usdc_mint
        ));
    }
    let prog = rpc
        .get_account(&amm)
        .await
        .map_err(rpc_err)?
        .ok_or_else(|| {
            format!("mock-amm {amm} is not deployed here (scripts/deploy.sh --program mock-amm)")
        })?;
    if prog.owner != BPF_UPGRADEABLE_LOADER_ID {
        return Err(format!("{amm} is not an upgradeable program"));
    }
    println!("amm-setup: program {amm}; admin {a}; usdc_mint {usdc}");
    let mut sent = 0usize;

    // Everything is read (and every refusal made) before the first transaction.
    // 1. AmmConfig: ours, or missing (init_amm checks on chain that the signer is the upgrade
    // authority).
    let amm_config = rpc.get_account(&p.amm_config()).await.map_err(rpc_err)?;
    let need_init = amm_config.is_none();
    match amm_config {
        None => {}
        Some(acc) => {
            let c = (acc.owner == amm)
                .then(|| AmmConfigAccount::decode(&acc.data).ok())
                .flatten()
                .ok_or_else(|| format!("{} is not this program's AmmConfig", p.amm_config()))?;
            if c.admin != a {
                return Err(format!("AmmConfig.admin is {}, not {a}: refusing", c.admin));
            }
            if c.usdc_mint != usdc {
                return Err(format!(
                    "AmmConfig.usdc_mint is {}, not schedina's {usdc}",
                    c.usdc_mint
                ));
            }
        }
    }

    // 2. What each mock mint lacks, read before anything is sent.
    struct Plan {
        spec: &'static MockMintSpec,
        mint: Pubkey,
        need_mint: bool,
        need_pool: bool,
        seed: bool,
        mint_amount: u64,
    }
    let mut plans = Vec::new();
    for spec in &AMM_MINTS {
        let mint = p.mock_mint(spec.symbol);
        let pool = p.pool(&mint);
        let (_, accs) = rpc
            .get_multiple_accounts(&[p.mock_token(&mint), pool])
            .await
            .map_err(rpc_err)?;
        if let Some(t) = &accs[0] {
            let t = (t.owner == amm)
                .then(|| MockTokenAccount::decode(&t.data).ok())
                .flatten()
                .ok_or_else(|| format!("{}: not a MockToken", p.mock_token(&mint)))?;
            if t.decimals != spec.decimals {
                return Err(format!(
                    "mock{} has {} decimals on chain, {} here",
                    spec.symbol, t.decimals, spec.decimals
                ));
            }
        }
        let r_mint = token_balance(rpc, &p.vault(&pool, &mint)).await?;
        let r_usdc = token_balance(rpc, &p.vault(&pool, &usdc)).await?;
        plans.push(Plan {
            spec,
            mint,
            need_mint: accs[0].is_none(),
            need_pool: accs[1].is_none(),
            seed: r_mint == 0 || r_usdc == 0,
            mint_amount: pool_usdc
                .checked_mul(spec.base_per_micro_usdc)
                .ok_or("--pool-usdc too large")?,
        });
    }

    // 3. Mock USDC for the empty pools: schedina's faucet, only what the authority's ATA lacks.
    let seeds = plans.iter().filter(|x| x.seed).count() as u64;
    let need_usdc = pool_usdc
        .checked_mul(seeds)
        .ok_or("--pool-usdc too large")?;
    let have_usdc = token_balance(rpc, &associated_token_address(&a, &usdc)).await?;
    let claim = need_usdc.saturating_sub(have_usdc);
    if claim > consts::FAUCET_MAX {
        return Err(format!(
            "the pools need {} mock USDC, over one faucet claim ({})",
            micro_to_usdc(claim),
            micro_to_usdc(consts::FAUCET_MAX)
        ));
    }

    if need_init {
        let sig = send(rpc, &[swap::ix::init_amm(&p, &a)], &[authority])
            .await
            .map_err(|e| format!("init_amm: {e}"))?;
        println!("tx init_amm {sig}");
        sent += 1;
    }
    if claim > 0 {
        let sig = send(
            rpc,
            &[
                ix::create_ata_idempotent(&a, &a, &usdc),
                ix::faucet_usdc(s, &a, &a, claim),
            ],
            &[authority],
        )
        .await
        .map_err(|e| format!("faucet_usdc: {e}"))?;
        println!("tx faucet_usdc {sig} ({} mock USDC)", micro_to_usdc(claim));
        sent += 1;
    }

    // 4. One transaction per mint: mint, pool, and the first deposit while the pool is empty.
    for x in &plans {
        let sym = x.spec.symbol;
        let mut ixs = Vec::new();
        if x.need_mint {
            let cap = x
                .spec
                .faucet_max_tokens
                .saturating_mul(10u64.pow(u32::from(x.spec.decimals)));
            ixs.push(swap::ix::init_mock_mint(&p, &a, sym, x.spec.decimals, cap));
        }
        if x.need_pool {
            ixs.push(swap::ix::init_pool(&p, &a, &x.mint));
        }
        if x.seed {
            // The admin's mock-token faucet is uncapped and has no cooldown.
            let have = token_balance(rpc, &associated_token_address(&a, &x.mint)).await?;
            ixs.push(ix::create_ata_idempotent(&a, &a, &x.mint));
            if have < x.mint_amount {
                ixs.push(swap::ix::faucet(
                    &p,
                    &a,
                    &a,
                    &x.mint,
                    &a,
                    x.mint_amount - have,
                ));
            }
            ixs.push(swap::ix::add_liquidity(
                &p,
                &a,
                &x.mint,
                x.mint_amount,
                pool_usdc,
            ));
        }
        if ixs.is_empty() {
            continue;
        }
        let sig = send(rpc, &ixs, &[authority])
            .await
            .map_err(|e| format!("mock{sym} pool: {e}"))?;
        println!("tx pool_mock{sym} {sig}");
        sent += 1;
    }

    // 5. The lookup table of [swap, verify, open_pending], recorded in AmmConfig.
    let mints: Vec<Pubkey> = plans.iter().map(|x| x.mint).collect();
    let want = swap::ix::lookup_table_addresses(s, &p, &mints);
    let read_cfg = || async {
        let acc = rpc
            .get_account(&p.amm_config())
            .await
            .map_err(rpc_err)?
            .ok_or("AmmConfig vanished")?;
        AmmConfigAccount::decode(&acc.data).map_err(|e| e.0.to_string())
    };
    let cfg = read_cfg().await?;
    if cfg.lookup_table == Pubkey::default() {
        let slot = swap::tx::finalized_slot(rpc).await.map_err(rpc_err)?;
        let table = swap::ix::lookup_table_address(&a, slot);
        let sig = send(
            rpc,
            &[
                swap::ix::create_lookup_table(&a, &a, slot),
                swap::ix::extend_lookup_table(&table, &a, &a, &want),
                swap::ix::set_lookup_table(&p, &a, &table),
            ],
            &[authority],
        )
        .await
        .map_err(|e| format!("lookup table: {e}"))?;
        println!("tx lookup_table {sig}");
        sent += 1;
    } else {
        let acc = rpc
            .get_account(&cfg.lookup_table)
            .await
            .map_err(rpc_err)?
            .filter(|t| t.owner == swap::ix::ALT_PROGRAM_ID)
            .ok_or_else(|| format!("AmmConfig.lookup_table {} is not a table", cfg.lookup_table))?;
        let (deactivated, table_auth) =
            lookup_table_meta(&acc.data).map_err(|e| format!("{}: {e}", cfg.lookup_table))?;
        let have =
            swap::accounts::lookup_table_addresses(&acc.data).map_err(|e| e.0.to_string())?;
        let missing: Vec<Pubkey> = want.iter().filter(|k| !have.contains(k)).copied().collect();
        if !missing.is_empty() {
            if deactivated || table_auth != Some(a) {
                return Err(format!(
                    "lookup table {} lacks {} addresses and cannot be extended by {a}",
                    cfg.lookup_table,
                    missing.len()
                ));
            }
            let sig = send(
                rpc,
                &[swap::ix::extend_lookup_table(
                    &cfg.lookup_table,
                    &a,
                    &a,
                    &missing,
                )],
                &[authority],
            )
            .await
            .map_err(|e| format!("extend lookup table: {e}"))?;
            println!("tx extend_lookup_table {sig}");
            sent += 1;
        }
    }

    // Where things stand.
    let cfg = read_cfg().await?;
    println!("  amm_config {}", p.amm_config());
    println!(
        "  lookup_table {} ({} addresses)",
        cfg.lookup_table,
        want.len()
    );
    for x in &plans {
        let pool = p.pool(&x.mint);
        let (vm, vu) = (p.vault(&pool, &x.mint), p.vault(&pool, &usdc));
        let (r_mint, r_usdc) = (
            token_balance(rpc, &vm).await?,
            token_balance(rpc, &vu).await?,
        );
        let price = (r_mint > 0).then(|| {
            u64::try_from(
                u128::from(r_usdc) * 10u128.pow(u32::from(x.spec.decimals)) / u128::from(r_mint),
            )
            .unwrap_or(u64::MAX)
        });
        println!(
            "  mock{} mint {} pool {} vaults {} / {}\n    reserves {} mock{} : {} USDC; {} USDC per mock{}",
            x.spec.symbol,
            x.mint,
            pool,
            vm,
            vu,
            units(r_mint, x.spec.decimals),
            x.spec.symbol,
            micro_to_usdc(r_usdc),
            price.map_or_else(|| "-".to_owned(), micro_to_usdc),
            x.spec.symbol,
        );
    }
    if sent == 0 {
        println!("amm-setup: nothing sent");
    } else {
        println!("amm-setup: {sent} transactions sent");
    }
    Ok(())
}

/// Within this many ppm of the target, `amm-peg` sends nothing (1%).
const PEG_TOLERANCE_PPM: u64 = 10_000;
/// At most this share of the input side's reserve per `amm-peg` run (10%).
const PEG_MAX_SHARE_PPM: u64 = 100_000;

/// The `amm-peg` target: micro-USD per whole mockSOL, and how it was found.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PegTarget {
    usd_u: u64,
    eur_per_sol_ppm: u64,
    eur_per_usd_ppm: u64,
    note: String,
}

/// `eur_per_sol_ppm / eur_per_usd_ppm` in micro-USD, rounded to the nearest.
fn usd_target(eur_per_sol_ppm: u64, eur_per_usd_ppm: u64) -> Option<u64> {
    if eur_per_usd_ppm == 0 {
        return None;
    }
    let n = u128::from(eur_per_sol_ppm) * 1_000_000 + u128::from(eur_per_usd_ppm) / 2;
    u64::try_from(n / u128::from(eur_per_usd_ppm)).ok()
}

/// The desk's `GET /fx`.
async fn desk_fx(desk: &str) -> Result<desk::api::types::FxResponse, String> {
    let shown = redact(desk);
    let url = format!("{}/fx", desk.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("desk {shown}: {}", e.without_url()))?;
    if !resp.status().is_success() {
        return Err(format!("desk {shown}/fx: HTTP {}", resp.status()));
    }
    resp.json()
        .await
        .map_err(|e| format!("desk {shown}/fx: {}", e.without_url()))
}

async fn peg_target(args: &Args) -> Result<PegTarget, String> {
    let desk_url = || {
        args.get("desk")
            .map(str::to_owned)
            .or_else(|| env("DESK_URL"))
            .unwrap_or_else(|| format!("http://{}", desk::config::DEFAULT_LISTEN))
    };
    let (eur_per_sol_ppm, eur_per_usd_ppm, note) =
        match (args.get("target-eur-per-sol"), args.get("from-fx")) {
            (Some(_), Some(_)) | (None, None) => {
                return Err("give --target-eur-per-sol X or --from-fx".to_owned());
            }
            (None, Some(_)) => {
                if args.get("eur-per-usd").is_some() {
                    return Err("--eur-per-usd goes with --target-eur-per-sol".to_owned());
                }
                let d = desk_url();
                let fx = desk_fx(&d).await?;
                if fx.eur_per_sol_ppm == 0 {
                    return Err(format!(
                        "desk {} serves no eur_per_sol_ppm (older desk?)",
                        redact(&d)
                    ));
                }
                if fx.sol_fallback {
                    eprintln!(
                        "amm-peg: warning: the desk serves its fixed SOL fallback, not a market \
                         price (no fresh Pyth price; see DESK_SOL_URL / DESK_SOL_API_KEY)"
                    );
                }
                let note = format!(
                    "from desk {} /fx: SOL {}, EUR/USD {} {}",
                    redact(&d),
                    fx.sol_source,
                    fx.source,
                    fx.as_of
                );
                (fx.eur_per_sol_ppm, fx.eur_per_usd_ppm, note)
            }
            (Some(x), None) => {
                let eur_per_sol = usdc_to_micro(x)
                    .ok_or_else(|| format!("--target-eur-per-sol: not an amount: {x}"))?;
                let (rate, note) = match (args.get("eur-per-usd"), args.get("desk")) {
                    (Some(y), _) => (
                        usdc_to_micro(y)
                            .ok_or_else(|| format!("--eur-per-usd: not a rate: {y}"))?,
                        "EUR/USD from --eur-per-usd".to_owned(),
                    ),
                    (None, Some(_)) => {
                        let d = desk_url();
                        let fx = desk_fx(&d).await?;
                        (
                            fx.eur_per_usd_ppm,
                            format!(
                                "EUR/USD from desk {} /fx ({} {})",
                                redact(&d),
                                fx.source,
                                fx.as_of
                            ),
                        )
                    }
                    (None, None) => (
                        desk::config::FX_FALLBACK_PPM,
                        format!(
                            "EUR/USD = the ECB fallback of {}",
                            desk::config::FX_FALLBACK_AS_OF
                        ),
                    ),
                };
                (eur_per_sol, rate, note)
            }
        };
    if !(desk::fx::MIN_PPM..=desk::fx::MAX_PPM).contains(&eur_per_usd_ppm) {
        return Err(format!("EUR/USD {eur_per_usd_ppm} ppm is not a sane rate"));
    }
    let usd_u = usd_target(eur_per_sol_ppm, eur_per_usd_ppm)
        .filter(|u| (desk::fx::sol::MIN_USD_U..=desk::fx::sol::MAX_USD_U).contains(u))
        .ok_or_else(|| format!("a target of {eur_per_sol_ppm} ppm EUR per SOL is not sane"))?;
    Ok(PegTarget {
        usd_u,
        eur_per_sol_ppm,
        eur_per_usd_ppm,
        note,
    })
}

/// Micro-USDC per whole token at reserves `(r_mint, r_usdc)` of a `dec`-decimals mint.
fn pool_price(r_mint: u64, r_usdc: u64, dec: u8) -> Option<u64> {
    if r_mint == 0 {
        return None;
    }
    u64::try_from(u128::from(r_usdc) * 10u128.pow(u32::from(dec)) / u128::from(r_mint)).ok()
}

/// `|price − target| / target`, ppm.
fn deviation_ppm(price: u64, target: u64) -> u64 {
    if target == 0 {
        return u64::MAX;
    }
    u64::try_from(u128::from(price.abs_diff(target)) * 1_000_000 / u128::from(target))
        .unwrap_or(u64::MAX)
}

/// One planned `amm-peg` swap.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PegPlan {
    /// Sell mockSOL for USDC (price too high), else buy mockSOL with USDC.
    sell: bool,
    /// What goes in: mockSOL base units on a sell, micro-USDC on a buy.
    amount_in: u64,
    /// What the pool pays out.
    out: u64,
    /// `min_out` signed into the swap (the output less 0.5%).
    min_out: u64,
    /// Pool price after the swap, micro-USDC per mockSOL.
    price_after: u64,
    /// The 10% bound stopped it short of the target (run again).
    capped: bool,
}

/// The swap that moves reserves `(r_sol, r_usdc)` (`dec` decimals, `fee_bps`) closest to
/// `target` micro-USDC per mockSOL, at most [`PEG_MAX_SHARE_PPM`] of the input reserve; `None`
/// within [`PEG_TOLERANCE_PPM`] (or when no swap moves it).
fn peg_plan(r_sol: u64, r_usdc: u64, dec: u8, fee_bps: u16, target: u64) -> Option<PegPlan> {
    let price = pool_price(r_sol, r_usdc, dec)?;
    if deviation_ppm(price, target) <= PEG_TOLERANCE_PPM {
        return None;
    }
    let sell = price > target;
    let r_in = if sell { r_sol } else { r_usdc };
    let cap = u64::try_from(u128::from(r_in) * u128::from(PEG_MAX_SHARE_PPM) / 1_000_000).ok()?;
    // The price after swapping `a` in: down on a sell, up on a buy (monotone in `a`).
    let after = |a: u64| -> Option<(u64, u64)> {
        if sell {
            let o = swap::math::swap_out(a, r_sol, r_usdc, fee_bps)?;
            Some((
                o.amount_out,
                pool_price(r_sol.checked_add(a)?, r_usdc - o.amount_out, dec)?,
            ))
        } else {
            let o = swap::math::swap_out(a, r_usdc, r_sol, fee_bps)?;
            Some((
                o.amount_out,
                pool_price(r_sol - o.amount_out, r_usdc.checked_add(a)?, dec)?,
            ))
        }
    };
    let reached =
        |a: u64| after(a).is_some_and(|(_, p)| if sell { p <= target } else { p >= target });
    let a = if reached(cap) {
        // The smallest amount that reaches the target, then whichever of it and one less lands
        // closer.
        let (mut lo, mut hi) = (0u64, cap);
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if reached(mid) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        match (after(lo), after(hi)) {
            (Some((_, pl)), Some((_, ph)))
                if deviation_ppm(pl, target) < deviation_ppm(ph, target) =>
            {
                lo
            }
            _ => hi,
        }
    } else {
        cap
    };
    let (out, price_after) = after(a)?;
    Some(PegPlan {
        sell,
        amount_in: a,
        out,
        min_out: swap::math::min_out(out, swap::SLIPPAGE_BPS),
        price_after,
        capped: deviation_ppm(price_after, target) > PEG_TOLERANCE_PPM,
    })
}

/// `amm-peg`: one bounded swap of the mockSOL pool toward `target`, sent only with `yes`.
async fn amm_peg(
    rpc: &RpcClient,
    s: &Pdas,
    amm: Pubkey,
    authority: &Keypair,
    target: PegTarget,
    yes: bool,
) -> Result<(), String> {
    let rpc_err = |e: desk::solana::rpc::RpcError| e.to_string();
    let a = authority.pubkey();
    let p = AmmPdas::new(amm, s);
    let usdc = s.usdc_mint();
    // Everything is read (and every refusal made) before anything is sent.
    let cfg = rpc
        .get_account(&p.amm_config())
        .await
        .map_err(rpc_err)?
        .filter(|acc| acc.owner == amm)
        .ok_or_else(|| format!("no AmmConfig at {} (run amm-setup)", p.amm_config()))?;
    let cfg = AmmConfigAccount::decode(&cfg.data).map_err(|e| e.0.to_string())?;
    if cfg.admin != a {
        return Err(format!(
            "{a} is not AmmConfig.admin {}: amm-peg is admin only",
            cfg.admin
        ));
    }
    if cfg.usdc_mint != usdc {
        return Err(format!(
            "AmmConfig.usdc_mint is {}, not schedina's {usdc}",
            cfg.usdc_mint
        ));
    }
    let mint = p.mock_mint("SOL");
    let pool_key = p.pool(&mint);
    let (_, accs) = rpc
        .get_multiple_accounts(&[p.mock_token(&mint), pool_key])
        .await
        .map_err(rpc_err)?;
    let dec = accs[0]
        .as_ref()
        .filter(|t| t.owner == amm)
        .and_then(|t| MockTokenAccount::decode(&t.data).ok())
        .ok_or("no mockSOL mint (run amm-setup)")?
        .decimals;
    let pool = accs[1]
        .as_ref()
        .filter(|t| t.owner == amm)
        .and_then(|t| PoolAccount::decode(&t.data).ok())
        .ok_or("no mockSOL pool (run amm-setup)")?;
    let r_sol = token_balance(rpc, &p.vault(&pool_key, &mint)).await?;
    let r_usdc = token_balance(rpc, &p.vault(&pool_key, &usdc)).await?;
    let price =
        pool_price(r_sol, r_usdc, dec).ok_or("the mockSOL pool is empty (run amm-setup)")?;
    println!(
        "amm-peg: program {amm}; admin {a}; pool {pool_key}\n  target {} USDC per mockSOL = €{} per SOL ÷ {} EUR per USD ({})\n  reserves {} mockSOL : {} USDC; {} USDC per mockSOL ({:+.2}%)",
        micro_to_usdc(target.usd_u),
        micro_to_usdc(target.eur_per_sol_ppm),
        micro_to_usdc(target.eur_per_usd_ppm),
        target.note,
        units(r_sol, dec),
        micro_to_usdc(r_usdc),
        micro_to_usdc(price),
        (price as f64 / target.usd_u as f64 - 1.0) * 100.0,
    );
    let Some(plan) = peg_plan(r_sol, r_usdc, dec, pool.fee_bps, target.usd_u) else {
        println!("amm-peg: within 1% of the target; nothing sent");
        return Ok(());
    };
    let (what_in, what_out) = if plan.sell {
        (
            format!("{} mockSOL", units(plan.amount_in, dec)),
            format!("{} USDC", micro_to_usdc(plan.out)),
        )
    } else {
        (
            format!("{} USDC", micro_to_usdc(plan.amount_in)),
            format!("{} mockSOL", units(plan.out, dec)),
        )
    };
    println!(
        "  plan: swap {what_in} for {what_out} (min {}); price after {} USDC per mockSOL{}",
        if plan.sell {
            micro_to_usdc(plan.min_out)
        } else {
            units(plan.min_out, dec)
        },
        micro_to_usdc(plan.price_after),
        if plan.capped {
            " (10% bound: run again)"
        } else {
            ""
        }
    );
    if !yes {
        println!("amm-peg: dry run; nothing sent (pass --yes to send)");
        return Ok(());
    }

    let (sol_ata, usdc_ata) = (
        associated_token_address(&a, &mint),
        associated_token_address(&a, &usdc),
    );
    let mut ixs = vec![
        ix::create_ata_idempotent(&a, &a, &mint),
        ix::create_ata_idempotent(&a, &a, &usdc),
    ];
    if plan.sell {
        let have = token_balance(rpc, &sol_ata).await?;
        if have < plan.amount_in {
            // The admin's mock-token faucet is uncapped and has no cooldown.
            ixs.push(swap::ix::faucet(
                &p,
                &a,
                &a,
                &mint,
                &a,
                plan.amount_in - have,
            ));
        }
    } else {
        let have = token_balance(rpc, &usdc_ata).await?;
        let claim = plan.amount_in.saturating_sub(have);
        if claim > 0 {
            if claim > consts::FAUCET_MAX {
                return Err(format!(
                    "the swap needs {} more mock USDC, over one faucet claim",
                    micro_to_usdc(claim)
                ));
            }
            ixs.push(ix::faucet_usdc(s, &a, &a, claim));
        }
    }
    let (user_in, user_out) = if plan.sell {
        (sol_ata, usdc_ata)
    } else {
        (usdc_ata, sol_ata)
    };
    ixs.push(swap::ix::swap(
        &p,
        &a,
        &mint,
        &user_in,
        &user_out,
        plan.sell,
        plan.amount_in,
        plan.min_out,
    ));
    let sig = send(rpc, &ixs, &[authority])
        .await
        .map_err(|e| format!("amm-peg swap: {e}"))?;
    println!("tx amm_peg {sig}");
    let r_sol = token_balance(rpc, &p.vault(&pool_key, &mint)).await?;
    let r_usdc = token_balance(rpc, &p.vault(&pool_key, &usdc)).await?;
    let price = pool_price(r_sol, r_usdc, dec).unwrap_or(0);
    println!(
        "  reserves {} mockSOL : {} USDC; {} USDC per mockSOL ({:+.2}%)",
        units(r_sol, dec),
        micro_to_usdc(r_usdc),
        micro_to_usdc(price),
        (price as f64 / target.usd_u as f64 - 1.0) * 100.0,
    );
    println!("amm-peg: 1 transaction sent");
    Ok(())
}

async fn status(rpc: &RpcClient, pdas: &Pdas) -> Result<(), String> {
    let p = pdas.program;
    let (slot, accs) = rpc
        .get_multiple_accounts(&[pdas.config(), pdas.vault()])
        .await
        .map_err(|e| e.to_string())?;
    let [cfg, vault] = <[_; 2]>::try_from(accs).map_err(|_| "account count")?;
    let Some(cfg) = cfg else {
        println!(
            "slot {slot}: no config at {} (run init-config)",
            pdas.config()
        );
        return Ok(());
    };
    let cfg = ConfigAccount::decode(&cfg.data, &cfg.owner, &p).map_err(|e| e.to_string())?;
    println!(
        "slot {slot}\nconfig {}\n  authority {}\n  crank {}\n  quote_signer {}\n  usdc_mint {}\n  max_legs {}  max_payout {}  min_stake {}  reserve {}\n  copy_fee_bps {}  tipster_share_bps {}  paused {}",
        pdas.config(),
        cfg.authority,
        cfg.crank,
        cfg.quote_signer,
        cfg.usdc_mint,
        cfg.max_legs,
        micro_to_usdc(cfg.max_payout),
        micro_to_usdc(cfg.min_stake),
        micro_to_usdc(cfg.reserve),
        cfg.copy_fee_bps,
        cfg.tipster_share_bps,
        cfg.paused
    );
    if let Some(v) = vault {
        let v = VaultAccount::decode(&v.data, &v.owner, &p).map_err(|e| e.to_string())?;
        println!(
            "vault {}\n  free {}  locked {}",
            pdas.vault(),
            micro_to_usdc(v.free),
            micro_to_usdc(v.locked)
        );
    }
    let (_, pendings) = rpc
        .get_program_accounts(&p, &[(0, disc::PENDING.to_vec())])
        .await
        .map_err(|e| e.to_string())?;
    let (_, tickets) = rpc
        .get_program_accounts(&p, &[(0, disc::TICKET.to_vec())])
        .await
        .map_err(|e| e.to_string())?;
    let escrow: u64 = pendings
        .iter()
        .filter_map(|(_, a)| PendingAccount::decode(&a.data, &a.owner, &p).ok())
        .map(|x| x.stake_u + x.copy_fee_u)
        .sum();
    let decoded: Vec<TicketAccount> = tickets
        .iter()
        .filter_map(|(_, a)| TicketAccount::decode(&a.data, &a.owner, &p).ok())
        .collect();
    let count = |s: TicketState| decoded.iter().filter(|t| t.state == s).count();
    println!(
        "pendings {} (escrow {})\ntickets {}: open {}, settled {}, cashed out {}",
        pendings.len(),
        micro_to_usdc(escrow),
        decoded.len(),
        count(TicketState::Open),
        count(TicketState::Settled),
        count(TicketState::CashedOut)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_amounts_and_flags() {
        assert_eq!(usdc_to_micro("12"), Some(12_000_000));
        assert_eq!(usdc_to_micro("0.5"), Some(500_000));
        assert_eq!(usdc_to_micro("1.000001"), Some(1_000_001));
        assert_eq!(usdc_to_micro("1.0000001"), None);
        assert_eq!(usdc_to_micro("x"), None);
        assert_eq!(micro_to_usdc(1_500_000), "1.500000");
        let a = parse(
            ["status", "--rpc", "http://x"]
                .map(String::from)
                .into_iter(),
        )
        .unwrap();
        assert_eq!(a.command, "status");
        assert_eq!(a.get("rpc"), Some("http://x"));
        assert!(parse(["--rpc"].map(String::from).into_iter()).is_err());
        assert!(parse(["a", "b"].map(String::from).into_iter()).is_err());
        let e = parse(["status", "--uri", "x"].map(String::from).into_iter())
            .err()
            .unwrap();
        assert!(e.contains("unknown flag --uri"), "{e}");
        assert!(
            parse(
                ["status", "--to", "a", "--to", "b"]
                    .map(String::from)
                    .into_iter()
            )
            .is_err()
        );
        assert_eq!(resolve_url("devnet"), "https://api.devnet.solana.com");
        assert_eq!(resolve_url("l"), "http://127.0.0.1:8899");
        assert_eq!(resolve_url("http://h:1"), "http://h:1");
    }

    #[test]
    fn amm_specs_match_the_desk_and_price_sensibly() {
        let syms: Vec<&str> = AMM_MINTS.iter().map(|m| m.symbol).collect();
        assert_eq!(syms, swap::MOCK_SYMBOLS);
        // 2,000 USDC per pool: 2,000,000 mockBONK ($0.001) and 10 mockSOL ($200).
        let usdc = 2_000_000_000u64;
        assert_eq!(
            units(usdc * AMM_MINTS[0].base_per_micro_usdc, 5),
            "2000000.00000"
        );
        assert_eq!(
            units(usdc * AMM_MINTS[1].base_per_micro_usdc, 9),
            "10.000000000"
        );
        // The app's faucet taps (50,000 mockBONK, 1 mockSOL) fit under the caps.
        assert!(AMM_MINTS[0].faucet_max_tokens >= 50_000 && AMM_MINTS[1].faucet_max_tokens >= 1);
        assert_eq!(units(7, 0), "7");
        let a = parse(
            ["amm-setup", "--pool-usdc", "10", "--amm-program-id", "x"]
                .map(String::from)
                .into_iter(),
        )
        .unwrap();
        assert_eq!(a.usdc("pool-usdc", None), Ok(10_000_000));
    }

    #[test]
    fn peg_targets_and_plans() {
        // €135.80 per SOL at 0.89847 EUR per USD: $151.145837 per SOL.
        assert_eq!(usd_target(135_800_000, 898_470), Some(151_145_837));
        assert_eq!(usd_target(1, 0), None);
        // amm-setup's pool: 10 mockSOL : 2,000 USDC ($200), fee 30 bps.
        let (r_sol, r_usdc) = (10_000_000_000u64, 2_000_000_000u64);
        assert_eq!(pool_price(r_sol, r_usdc, 9), Some(200_000_000));
        // Within 1%: nothing.
        assert_eq!(peg_plan(r_sol, r_usdc, 9, 30, 198_100_000), None);
        assert_eq!(peg_plan(r_sol, r_usdc, 9, 30, 201_900_000), None);
        // $185: a sell of ~0.4 mockSOL lands on it, well inside 10%.
        let p = peg_plan(r_sol, r_usdc, 9, 30, 185_000_000).unwrap();
        assert!(p.sell && !p.capped, "{p:?}");
        assert!(p.amount_in < r_sol / 10);
        assert!(deviation_ppm(p.price_after, 185_000_000) < 1_000, "{p:?}");
        // Re-planned on the reserves after that swap: nothing (idempotent).
        let o = swap::math::swap_out(p.amount_in, r_sol, r_usdc, 30).unwrap();
        assert_eq!(o.amount_out, p.out);
        assert_eq!(
            peg_plan(r_sol + p.amount_in, r_usdc - p.out, 9, 30, 185_000_000),
            None
        );
        assert!(p.min_out < p.out);
        // $210: a buy with USDC.
        let b = peg_plan(r_sol, r_usdc, 9, 30, 210_000_000).unwrap();
        assert!(!b.sell && !b.capped && b.amount_in < r_usdc / 10, "{b:?}");
        assert!(deviation_ppm(b.price_after, 210_000_000) < 1_000, "{b:?}");
        // $100: half the price is past the 10% bound; the first run sells exactly 10%.
        let c = peg_plan(r_sol, r_usdc, 9, 30, 100_000_000).unwrap();
        assert!(c.sell && c.capped);
        assert_eq!(c.amount_in, r_sol / 10);
        // A run lands closer each time and never overshoots.
        let (mut rs, mut ru, mut runs) = (r_sol, r_usdc, 0);
        while let Some(x) = peg_plan(rs, ru, 9, 30, 100_000_000) {
            assert!(x.price_after >= 99_000_000, "{x:?}");
            rs += x.amount_in;
            ru -= x.out;
            runs += 1;
            assert!(runs < 20);
        }
        assert!(deviation_ppm(pool_price(rs, ru, 9).unwrap(), 100_000_000) <= 10_000);
        assert_eq!(deviation_ppm(1, 0), u64::MAX);
    }

    #[test]
    fn peg_flags_parse() {
        let a = parse(
            [
                "amm-peg",
                "--from-fx",
                "--desk",
                "http://127.0.0.1:8787",
                "--yes",
            ]
            .map(String::from)
            .into_iter(),
        )
        .unwrap();
        assert_eq!(a.command, "amm-peg");
        assert!(a.get("from-fx").is_some() && a.get("yes").is_some());
        assert_eq!(a.get("desk"), Some("http://127.0.0.1:8787"));
        let a = parse(
            ["amm-peg", "--target-eur-per-sol", "135.80"]
                .map(String::from)
                .into_iter(),
        )
        .unwrap();
        assert!(a.get("yes").is_none());
        assert_eq!(
            usdc_to_micro(a.get("target-eur-per-sol").unwrap()),
            Some(135_800_000)
        );
    }

    #[tokio::test]
    async fn peg_targets_refuse_bad_input_before_any_io() {
        let args = |v: &[&str]| parse(v.iter().map(|s| (*s).to_owned())).unwrap();
        for bad in [
            &["amm-peg"][..],
            &["amm-peg", "--from-fx", "--target-eur-per-sol", "1"],
            &["amm-peg", "--from-fx", "--eur-per-usd", "0.9"],
            &["amm-peg", "--target-eur-per-sol", "x"],
            &[
                "amm-peg",
                "--target-eur-per-sol",
                "135",
                "--eur-per-usd",
                "9",
            ],
            &[
                "amm-peg",
                "--target-eur-per-sol",
                "0.0001",
                "--eur-per-usd",
                "0.9",
            ],
        ] {
            assert!(peg_target(&args(bad)).await.is_err(), "{bad:?}");
        }
        let t = peg_target(&args(&["amm-peg", "--target-eur-per-sol", "135.80"]))
            .await
            .unwrap();
        assert_eq!(t.usd_u, 151_145_837);
        assert!(t.note.contains("fallback"), "{}", t.note);
    }

    #[test]
    fn reads_lookup_table_meta() {
        let k = Pubkey::new_from_array([9; 32]);
        let mut d = vec![0u8; 56 + 64];
        d[..4].copy_from_slice(&1u32.to_le_bytes());
        d[4..12].copy_from_slice(&u64::MAX.to_le_bytes());
        d[21] = 1;
        d[22..54].copy_from_slice(k.as_ref());
        assert_eq!(lookup_table_meta(&d), Ok((false, Some(k))));
        d[4..12].copy_from_slice(&5u64.to_le_bytes());
        d[21] = 0;
        assert_eq!(lookup_table_meta(&d), Ok((true, None)));
        assert!(lookup_table_meta(&d[..40]).is_err());
    }

    #[tokio::test]
    async fn refuses_mainnet_monikers_before_any_io() {
        for m in ["mainnet-beta", "m"] {
            let e = run(["--url", m, "status"].map(String::from).into_iter())
                .await
                .unwrap_err();
            assert!(e.contains("mainnet"), "{m}: {e}");
        }
        let e = run(["--rpc", "http://a", "--url", "http://b", "status"]
            .map(String::from)
            .into_iter())
        .await
        .unwrap_err();
        assert!(e.contains("not both"), "{e}");
    }

    #[tokio::test]
    async fn refuses_mainnet_urls_before_any_io() {
        let e = run([
            "--rpc",
            "https://api.mainnet-beta.solana.com/?api-key=secret",
            "status",
        ]
        .map(String::from)
        .into_iter())
        .await
        .unwrap_err();
        assert!(e.contains("mainnet"), "{e}");
        assert!(!e.contains("secret"), "{e}");
    }
}
