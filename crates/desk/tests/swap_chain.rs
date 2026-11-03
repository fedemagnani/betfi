//! Step 4.D localnet proof (L4-3), `#[ignore]`d; run by `just swap-chain-test`, which builds both
//! `--features devnet`-style SBPF v0 `.so` files first. A private `solana-test-validator` loads
//! schedina and mock-amm as upgradeable programs owned by one fresh test key (SIMD-0500
//! deactivated, D-006/D-081); every key is generated in a temp dir.
//!
//! In order: schedina `init_config` and its USDC faucet; the AMM's `init_amm`, mockBONK mint and
//! pool, seeded with 5,000,000 mockBONK : 5,000 USDC; a lookup table with the static accounts;
//! the desk's `GET /tokens` and `POST /swap-quote` over HTTP; then a user holding only mockBONK
//! stakes through one v0 transaction `[swap, Ed25519 verify, open_pending]` (5 legs, the largest
//! case) and the Pending escrows exactly `min_out_u`; a second transaction whose swap asks more
//! than the pool pays fails whole and leaves no Pending.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use common::{TempDb, pascal_stub, repo_root};
use desk::Config;
use desk::program::ix::{self, Params, QuoteMsg};
use desk::program::{Pdas, PendingAccount, QuoteLeg, associated_token_address, sha256};
use desk::solana::rpc::RpcClient;
use desk::solana::signer::{Keypair, send};
use desk::swap::{self, AmmPdas, SwapQuoteResponse, TokensResponse};
use solana_pubkey::Pubkey;

const SCHEDINA_ID: &str = "HXdZFi7tR669G1hXogh3uGGqLYsoZPCSuiutAviLNToW";
const SIMD_0500: &str = "B8JJXCy5amZyWG9r7EnUYLwzXSXTxG7GZ1qZ1qggo83g";
const USDC: u64 = 1_000_000;
const BONK: u64 = 100_000;

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
        let p = 20_020 + u16::try_from(pid % 1_000).unwrap() * 40;
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

async fn balance(rpc: &RpcClient, account: &Pubkey) -> u64 {
    rpc.get_account(account).await.unwrap().map_or(0, |a| {
        u64::from_le_bytes(a.data[64..72].try_into().unwrap())
    })
}

fn leg(g: u8, start_ts: i64) -> QuoteLeg {
    QuoteLeg {
        symbol_hash: sha256(format!("NBA_SYM_{g}").as_bytes()),
        game_id_hash: sha256(format!("nba-game-{g}").as_bytes()),
        start_ts,
        side: g % 2,
        price_u: 500_000,
        m1_ppm: 50_000,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs solana-test-validator and both SBPF v0 .so files: just swap-chain-test"]
async fn stake_in_mockbonk_on_localnet() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_test_writer()
        .try_init();
    for (n, e) in [("schedina", "SCHEDINA_SO"), ("mock_amm", "MOCK_AMM_SO")] {
        assert!(
            so(n, e).exists(),
            "{} missing: run `just swap-chain-test`",
            so(n, e).display()
        );
    }
    let dir = std::env::temp_dir().join(format!(
        "schedina-swap-{}-{}",
        std::process::id(),
        desk::now_ms()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let authority_path = dir.join("authority.json");
    let authority = keygen(&authority_path);
    let user = keygen(&dir.join("user.json"));
    let quote_signer = keygen(&dir.join("quote-signer.json"));
    let crank = keygen(&dir.join("crank.json"));
    let validator = Validator::start(dir.clone(), &authority.pubkey()).await;
    let url = validator.url();
    for k in [&authority, &user] {
        airdrop(&url, &authority_path, &k.pubkey());
    }
    let rpc = RpcClient::new(&url);
    let s = Pdas::new(SCHEDINA_ID.parse().unwrap());
    let p = AmmPdas::new(swap::DEFAULT_PROGRAM_ID.parse().unwrap(), &s);
    let usdc = s.usdc_mint();
    let (a, u) = (authority.pubkey(), user.pubkey());

    // Schedina: config, then the authority's mock USDC from its faucet (D-052).
    let params = Params {
        crank: crank.pubkey(),
        quote_signer: quote_signer.pubkey(),
        max_legs: 5,
        max_payout: 10_000 * USDC,
        min_stake: USDC,
        reserve: 0,
        copy_fee_bps: 200,
        tipster_share_bps: 8_000,
    };
    send(&rpc, &[ix::init_config(&s, &a, &a, &params)], &[&authority])
        .await
        .expect("schedina init_config");
    send(
        &rpc,
        &[
            ix::create_ata_idempotent(&a, &a, &usdc),
            ix::faucet_usdc(&s, &a, &a, 10_000 * USDC),
        ],
        &[&authority],
    )
    .await
    .expect("faucet_usdc");

    // The AMM: config, mockBONK, its pool, 5,000,000 mockBONK : 5,000 USDC.
    let bonk = p.mock_mint("BONK");
    send(&rpc, &[swap::ix::init_amm(&p, &a)], &[&authority])
        .await
        .expect("init_amm");
    send(
        &rpc,
        &[
            swap::ix::init_mock_mint(&p, &a, "BONK", 5, 100_000 * BONK),
            swap::ix::init_pool(&p, &a, &bonk),
            ix::create_ata_idempotent(&a, &a, &bonk),
            swap::ix::faucet(&p, &a, &a, &bonk, &a, 5_000_000 * BONK),
            swap::ix::add_liquidity(&p, &a, &bonk, 5_000_000 * BONK, 5_000 * USDC),
        ],
        &[&authority],
    )
    .await
    .expect("mockBONK pool");

    // The lookup table of [swap, verify, open_pending], recorded in AmmConfig.
    let slot = swap::tx::finalized_slot(&rpc).await.unwrap();
    let table = swap::ix::lookup_table_address(&a, slot);
    let addresses = swap::ix::lookup_table_addresses(&s, &p, &[bonk]);
    send(
        &rpc,
        &[
            swap::ix::create_lookup_table(&a, &a, slot),
            swap::ix::extend_lookup_table(&table, &a, &a, &addresses),
            swap::ix::set_lookup_table(&p, &a, &table),
        ],
        &[&authority],
    )
    .await
    .expect("lookup table");
    // Addresses extended in a slot are usable from the next one.
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    let cfg = swap::accounts::AmmConfigAccount::decode(
        &rpc.get_account(&p.amm_config())
            .await
            .unwrap()
            .unwrap()
            .data,
    )
    .unwrap();
    assert_eq!(
        (cfg.admin, cfg.usdc_mint, cfg.lookup_table),
        (a, usdc, table)
    );
    let lut = swap::tx::lookup_table(&rpc, &table).await.unwrap();
    assert_eq!(lut.addresses, addresses);

    // The desk, offline from Pascal's chain side, reading the pools over this validator.
    let db = TempDb::new("swap-chain");
    let mut dcfg = Config::local(pascal_stub().await, db.path());
    dcfg.solana_rpc_url = url.clone();
    dcfg.shutdown_grace = Duration::from_secs(5);
    let running = desk::start(dcfg).await.expect("desk starts");
    let base = format!("http://{}", running.addr);
    let http = reqwest::Client::new();
    let tokens: TokensResponse = http
        .get(format!("{base}/tokens"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    println!("GET /tokens -> {}", serde_json::to_string(&tokens).unwrap());
    assert_eq!(
        tokens.tokens.len(),
        2,
        "mockUSDC and mockBONK (no mockSOL pool here)"
    );
    assert!(tokens.tokens[0].usdc_mint && tokens.tokens[0].mint == usdc.to_string());
    assert_eq!(
        (tokens.tokens[1].symbol.as_str(), tokens.tokens[1].decimals),
        ("mockBONK", 5)
    );

    // The user holds only mockBONK (faucet) and an empty mock-USDC account.
    let user_bonk = associated_token_address(&u, &bonk);
    let user_usdc = associated_token_address(&u, &usdc);
    send(
        &rpc,
        &[
            ix::create_ata_idempotent(&u, &u, &bonk),
            swap::ix::faucet(&p, &u, &u, &bonk, &u, 50_000 * BONK),
            ix::create_ata_idempotent(&u, &u, &usdc),
        ],
        &[&user],
    )
    .await
    .expect("user faucet");
    assert_eq!(balance(&rpc, &user_bonk).await, 50_000 * BONK);

    let amount_in = 50_000 * BONK;
    let q: SwapQuoteResponse = http
        .post(format!("{base}/swap-quote"))
        .json(&serde_json::json!({"mint": bonk.to_string(), "amount_in": amount_in.to_string()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    println!("POST /swap-quote -> {}", serde_json::to_string(&q).unwrap());
    let want = swap::math::swap_out(amount_in, 5_000_000 * BONK, 5_000 * USDC, 50).unwrap();
    assert_eq!(q.usdc_out_u, want.amount_out);
    assert_eq!(q.min_out_u, swap::math::min_out(want.amount_out, 50));
    assert_eq!(q.pool, p.pool(&bonk).to_string());

    // A desk-signed 5-leg quote for stake_u = min_out_u (the largest transaction).
    let now = rpc.cluster_time().await.unwrap();
    let legs: Vec<QuoteLeg> = (0..5).map(|g| leg(g, now + 7_200)).collect();
    let payout = pricing::payout(
        q.min_out_u,
        1_000_000,
        &legs
            .iter()
            .map(|l| pricing::PayoutLeg {
                m1_ppm: l.m1_ppm,
                f_u: l.price_u,
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
    // The cluster clock (block time trails the Clock sysvar a little): read before each quote.
    let sign_quote = |stake_u: u64, odds_u: u64, now: i64| {
        let msg = QuoteMsg {
            program_id: s.program,
            legs: legs.clone(),
            stake_u,
            odds_offered_u: odds_u,
            quote_expires_at: now + 20,
        }
        .encode();
        let sig: [u8; 64] = quote_signer.sign(&msg).into();
        ix::ed25519_verify(&msg, &sig, &quote_signer.pubkey())
    };

    // 1. The swap asks one micro-USDC more than the pool pays: the whole transaction fails.
    let bad_nonce = 41;
    let now = rpc.cluster_time().await.unwrap();
    let bad = [
        swap::ix::sell_to_usdc(&p, &u, &bonk, amount_in, q.usdc_out_u + 1),
        sign_quote(q.usdc_out_u + 1, payout.odds_u, now),
        ix::open_pending(&s, &u, &u, bad_nonce, payout.odds_u, now + 45, false, None),
    ];
    let err = swap::tx::send_v0(&rpc, &bad, &[&user], std::slice::from_ref(&lut))
        .await
        .expect_err("swap below min_out must fail");
    println!("failed swap -> {err}");
    let code = 6000 + 10; // AmmError::SlippageExceeded
    assert!(
        err.to_string().contains(&format!("{code:#x}"))
            || err.to_string().contains(&code.to_string()),
        "{err}"
    );
    assert!(
        rpc.get_account(&s.pending(&u, bad_nonce))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(balance(&rpc, &user_bonk).await, amount_in);
    assert_eq!(balance(&rpc, &user_usdc).await, 0);
    assert_eq!(balance(&rpc, &s.escrow_usdc()).await, 0);

    // 2. The real one: [swap, verify, open_pending], stake = min_out_u.
    let nonce = 42;
    let now = rpc.cluster_time().await.unwrap();
    let good = [
        swap::ix::sell_to_usdc(&p, &u, &bonk, amount_in, q.min_out_u),
        sign_quote(q.min_out_u, payout.odds_u, now),
        ix::open_pending(&s, &u, &u, nonce, payout.odds_u, now + 45, false, None),
    ];
    let (blockhash, _) = rpc.latest_blockhash().await.unwrap();
    let (_, wire) =
        swap::tx::sign_v0(&good, &[&user], std::slice::from_ref(&lut), blockhash).unwrap();
    println!(
        "5-leg [swap, verify, open_pending], v0 + lookup table: {} bytes (limit {})",
        wire.len(),
        swap::tx::PACKET_DATA_SIZE
    );
    assert!(wire.len() <= swap::tx::PACKET_DATA_SIZE);
    let sig = swap::tx::send_v0(&rpc, &good, &[&user], std::slice::from_ref(&lut))
        .await
        .expect("stake in mockBONK");
    println!("staked in mockBONK: {sig}");
    let pending_key = s.pending(&u, nonce);
    let acc = rpc
        .get_account(&pending_key)
        .await
        .unwrap()
        .expect("pending");
    let pending = PendingAccount::decode(&acc.data, &acc.owner, &s.program).unwrap();
    assert_eq!((pending.user, pending.stake_u), (u, q.min_out_u));
    assert_eq!(pending.legs.len(), 5);
    let escrow = balance(&rpc, &s.escrow_usdc()).await;
    let leftover = balance(&rpc, &user_usdc).await;
    println!(
        "pending {pending_key}: stake {} micro-USDC escrowed {escrow}; user keeps {leftover} \
         micro-USDC (usdc_out {} - min_out {})",
        pending.stake_u, q.usdc_out_u, q.min_out_u
    );
    assert_eq!(escrow, q.min_out_u);
    assert_eq!(leftover, q.usdc_out_u - q.min_out_u);
    assert_eq!(balance(&rpc, &user_bonk).await, 0);
    let pool = p.pool(&bonk);
    assert_eq!(
        balance(&rpc, &p.vault(&pool, &usdc)).await,
        5_000 * USDC - q.usdc_out_u
    );

    running.join().await;
    drop(validator);
}
