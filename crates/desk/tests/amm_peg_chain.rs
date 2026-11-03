//! Plan 7.0c review gate, `crank amm-peg` on localnet (V-3, l7-contracts L7-1), `#[ignore]`d; run
//! by `just amm-peg-test`, which builds both SBPF v0 `.so` files first (the recipe of
//! `just swap-chain-test`). A private `solana-test-validator` loads schedina (devnet features)
//! and mock-amm, owned by one fresh test key; every key is generated in a temp dir; the validator
//! is killed at the end. The `crank` binary itself is driven, as an operator would.
//!
//! In order: `init-config`, `faucet` and `amm-setup` (mockSOL at $200); a dry run sends nothing;
//! `--yes` moves the price within 1% of a lower target (a sell, mockSOL from the admin's faucet)
//! and a re-run sends nothing; a higher target is reached by a buy with USDC; a wallet that is not
//! `AmmConfig.admin` is refused; `--from-fx` against an in-process desk (SOL fallback, `sol::FALLBACK_USD_U`)
//! walks a 28% move in bounded steps of at most 10% of the reserves until it is within 1%. The
//! RPC URL carries a query string that must never be printed.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use common::{TempDb, pascal_stub, repo_root, start_desk};
use desk::program::Pdas;
use desk::solana::rpc::RpcClient;
use desk::solana::signer::Keypair;
use desk::swap::{self, AmmPdas};
use solana_pubkey::Pubkey;

const SCHEDINA_ID: &str = "HXdZFi7tR669G1hXogh3uGGqLYsoZPCSuiutAviLNToW";
const SIMD_0500: &str = "B8JJXCy5amZyWG9r7EnUYLwzXSXTxG7GZ1qZ1qggo83g";

fn so(name: &str, env: &str) -> PathBuf {
    std::env::var_os(env).map_or_else(
        || repo_root().join(format!("programs/target/deploy-devnet/{name}.so")),
        PathBuf::from,
    )
}

/// A private `solana-test-validator`; killed on drop.
struct Validator {
    dir: PathBuf,
    rpc_port: u16,
    child: Option<Child>,
}

impl Validator {
    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.rpc_port)
    }

    async fn start(dir: PathBuf, authority: &Pubkey) -> Self {
        let pid = std::process::id();
        let p = 21_020 + u16::try_from(pid % 1_000).unwrap() * 40;
        let log = std::fs::File::create(dir.join("validator.log")).unwrap();
        let child = Command::new("solana-test-validator")
            .arg("--ledger")
            .arg(dir.join("ledger"))
            .args(["--quiet", "--bind-address", "127.0.0.1"])
            .args(["--rpc-port", &p.to_string()])
            .args(["--faucet-port", &(p + 2).to_string()])
            .args(["--gossip-port", &(p + 3).to_string()])
            .args(["--dynamic-port-range", &format!("{}-{}", p + 4, p + 38)])
            .arg("--upgradeable-program")
            .arg(SCHEDINA_ID)
            .arg(so("schedina", "SCHEDINA_SO"))
            .arg(authority.to_string())
            .arg("--upgradeable-program")
            .arg(swap::DEFAULT_PROGRAM_ID)
            .arg(so("mock_amm", "MOCK_AMM_SO"))
            .arg(authority.to_string())
            .args(["--deactivate-feature", SIMD_0500])
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .expect("solana-test-validator on PATH");
        let v = Validator {
            dir,
            rpc_port: p,
            child: Some(child),
        };
        let rpc = RpcClient::new(v.url());
        let t0 = Instant::now();
        while !rpc.block_height().await.is_ok_and(|h| h > 1) {
            assert!(
                t0.elapsed() < Duration::from_secs(60),
                "validator not up; see {}",
                v.dir.join("validator.log").display()
            );
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        v
    }
}

impl Drop for Validator {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn keygen(path: &Path) -> Keypair {
    let ok = Command::new("solana-keygen")
        .args(["new", "--no-bip39-passphrase", "--silent", "--force", "-o"])
        .arg(path)
        .stdout(Stdio::null())
        .status()
        .expect("solana-keygen on PATH")
        .success();
    assert!(ok, "solana-keygen");
    Keypair::read(path).unwrap()
}

fn airdrop(url: &str, payer: &Path, to: &Pubkey) {
    let ok = Command::new("solana")
        .args(["airdrop", "--url", url, "-k"])
        .arg(payer)
        .args(["100", &to.to_string()])
        .stdout(Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(ok, "airdrop to {to}");
}

/// Runs `crank` against `rpc` with `authority`; `(success, stdout + stderr)`.
fn crank(rpc: &str, authority: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_crank"))
        .args(["--rpc", rpc, "--program-id", SCHEDINA_ID, "--authority"])
        .arg(authority)
        .args(args)
        .env_remove("SOLANA_RPC_URL")
        .env_remove("MOCK_AMM_PROGRAM_ID")
        .env_remove("DESK_URL")
        .output()
        .expect("crank runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    println!("$ crank {}\n{text}", args.join(" "));
    (out.status.success(), text)
}

/// `(mockSOL reserve, USDC reserve)` of the mockSOL pool.
async fn reserves(rpc: &RpcClient, p: &AmmPdas, usdc: &Pubkey) -> (u64, u64) {
    let mint = p.mock_mint("SOL");
    let pool = p.pool(&mint);
    let bal = |k: Pubkey| async move {
        rpc.get_account(&k).await.unwrap().map_or(0, |a| {
            u64::from_le_bytes(a.data[64..72].try_into().unwrap())
        })
    };
    (
        bal(p.vault(&pool, &mint)).await,
        bal(p.vault(&pool, usdc)).await,
    )
}

/// Micro-USDC per mockSOL (9 decimals).
fn price((r_sol, r_usdc): (u64, u64)) -> u64 {
    u64::try_from(u128::from(r_usdc) * 1_000_000_000 / u128::from(r_sol)).unwrap()
}

fn within_1pct(p: u64, target: u64) -> bool {
    p.abs_diff(target) * 100 <= target
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs solana-test-validator and both SBPF v0 .so files: just amm-peg-test"]
async fn amm_peg_on_localnet() {
    for (n, e) in [("schedina", "SCHEDINA_SO"), ("mock_amm", "MOCK_AMM_SO")] {
        assert!(
            so(n, e).exists(),
            "{} missing: run `just amm-peg-test`",
            so(n, e).display()
        );
    }
    let dir = std::env::temp_dir().join(format!(
        "schedina-peg-{}-{}",
        std::process::id(),
        desk::now_ms()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let authority_path = dir.join("authority.json");
    let authority = keygen(&authority_path);
    let stranger_path = dir.join("stranger.json");
    let stranger = keygen(&stranger_path);
    let crank_key = keygen(&dir.join("crank.json"));
    let quote_signer = keygen(&dir.join("quote-signer.json"));
    let validator = Validator::start(dir.clone(), &authority.pubkey()).await;
    let url = validator.url();
    for k in [&authority, &stranger] {
        airdrop(&url, &authority_path, &k.pubkey());
    }
    // A query string the output must never show (redaction, as amm-setup).
    let rpc_url = format!("{url}/?api-key=sekrit");
    let rpc = RpcClient::new(&url);
    let s = Pdas::new(SCHEDINA_ID.parse().unwrap());
    let p = AmmPdas::new(swap::DEFAULT_PROGRAM_ID.parse().unwrap(), &s);
    let usdc = s.usdc_mint();
    let a = authority.pubkey().to_string();
    let run = |args: &[&str]| {
        let (ok, out) = crank(&rpc_url, &authority_path, args);
        assert!(!out.contains("sekrit"), "{out}");
        (ok, out)
    };

    // Set up: config, 10,000 mock USDC for the admin (amm-setup then claims nothing), the pools.
    let (ok, out) = run(&[
        "init-config",
        "--crank",
        &crank_key.pubkey().to_string(),
        "--quote-signer",
        &quote_signer.pubkey().to_string(),
        "--reserve-usdc",
        "0",
    ]);
    assert!(ok, "{out}");
    let (ok, out) = run(&["faucet", "--to", &a, "--usdc", "10000"]);
    assert!(ok, "{out}");
    let (ok, out) = run(&["amm-setup"]);
    assert!(ok, "{out}");
    let r0 = reserves(&rpc, &p, &usdc).await;
    assert_eq!(price(r0), 200_000_000, "amm-setup seeds mockSOL at $200");

    // €166.20 per SOL at 0.90 EUR per USD: $184.666667. A dry run sends nothing.
    let lower = [
        "amm-peg",
        "--target-eur-per-sol",
        "166.20",
        "--eur-per-usd",
        "0.9",
    ];
    let (ok, out) = run(&lower);
    assert!(ok && out.contains("dry run; nothing sent"), "{out}");
    assert_eq!(reserves(&rpc, &p, &usdc).await, r0);
    // --yes: one sell, within 1%.
    let (ok, out) = run(&[&lower[..], &["--yes"]].concat());
    assert!(ok && out.contains("amm-peg: 1 transaction sent"), "{out}");
    let r1 = reserves(&rpc, &p, &usdc).await;
    assert!(r1.0 > r0.0 && r1.1 < r0.1, "a sell: {r0:?} → {r1:?}");
    assert!(within_1pct(price(r1), 184_666_667), "{}", price(r1));
    // A re-run sends nothing.
    let (ok, out) = run(&[&lower[..], &["--yes"]].concat());
    assert!(ok && out.contains("nothing sent"), "{out}");
    assert_eq!(reserves(&rpc, &p, &usdc).await, r1);

    // €189 at 0.90: $210, a buy with the admin's USDC.
    let higher = [
        "amm-peg",
        "--target-eur-per-sol",
        "189",
        "--eur-per-usd",
        "0.9",
        "--yes",
    ];
    let (ok, out) = run(&higher);
    assert!(ok && out.contains("1 transaction sent"), "{out}");
    let r2 = reserves(&rpc, &p, &usdc).await;
    assert!(r2.0 < r1.0 && r2.1 > r1.1, "a buy: {r1:?} → {r2:?}");
    assert!(within_1pct(price(r2), 210_000_000), "{}", price(r2));

    // Not the admin: refused before anything is sent.
    let (ok, out) = crank(&rpc_url, &stranger_path, &higher);
    assert!(!ok && out.contains("admin only"), "{out}");
    assert!(!out.contains("sekrit"), "{out}");

    // --from-fx against a desk serving the SOL fallback (fallback USD × EUR/USD ÷ EUR/USD = fallback USD): a
    // 28% move, at most 10% of the input reserve per run, until within 1%.
    let pascal = pascal_stub().await;
    let db = TempDb::new("amm-peg");
    let desk = start_desk(&pascal, &db).await;
    let desk_url = format!("http://{}", desk.addr);
    let mut runs = 0;
    loop {
        let before = reserves(&rpc, &p, &usdc).await;
        let (ok, out) = run(&["amm-peg", "--from-fx", "--desk", &desk_url, "--yes"]);
        assert!(ok, "{out}");
        if out.contains("nothing sent") {
            break;
        }
        let after = reserves(&rpc, &p, &usdc).await;
        assert!(
            after.0 - before.0 <= before.0 / 10,
            "{before:?} → {after:?}"
        );
        assert!(
            price(after) >= desk::fx::sol::FALLBACK_USD_U * 99 / 100,
            "never overshoots"
        );
        runs += 1;
        assert!(runs <= 6, "too many runs");
    }
    assert!(runs >= 2, "the 10% bound split the move ({runs} run)");
    assert!(within_1pct(
        price(reserves(&rpc, &p, &usdc).await),
        desk::fx::sol::FALLBACK_USD_U
    ));
    desk.join().await;
    drop(validator);
}
