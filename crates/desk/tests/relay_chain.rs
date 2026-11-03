//! Step 8.B localnet proof, `#[ignore]`d; run by `just relay-chain-test`, which builds the
//! `--features devnet` SBPF v0 `.so` first. A private `solana-test-validator` loads schedina as an
//! upgradeable program owned by a fresh test key (SIMD-0500 deactivated, D-006/D-081); every key
//! is generated in a temp dir.
//!
//! A user with 0 SOL, through the desk's `POST /relay` (the sponsor key pays fees and rent):
//! claims test USDC (`[CreateIdempotent, faucet_usdc, memo]`), claims again at once (no cooldown,
//! D-661), bets (`[Ed25519 verify, open_pending(payer = sponsor)]`,
//! the Pending's rent payer is the sponsor), and still has no SOL account at the end. A System
//! transfer out of the sponsor is refused before anything is sent.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine;
use common::{TempDb, pascal_stub, repo_root};
use desk::Config;
use desk::program::ix::{self, Params, QuoteMsg};
use desk::program::{
    Pdas, PendingAccount, QuoteLeg, SYSTEM_PROGRAM_ID, associated_token_address, sha256,
};
use desk::relay::rules::{FAUCET_MEMO, MEMO_PROGRAM_ID};
use desk::solana::rpc::RpcClient;
use desk::solana::signer::{Keypair, send};
use serde_json::{Value, json};
use solana_instruction::{AccountMeta, Instruction};
use solana_message::{Message, VersionedMessage};
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_transaction::versioned::VersionedTransaction;

const SCHEDINA_ID: &str = "HXdZFi7tR669G1hXogh3uGGqLYsoZPCSuiutAviLNToW";
const SIMD_0500: &str = "B8JJXCy5amZyWG9r7EnUYLwzXSXTxG7GZ1qZ1qggo83g";
const USDC: u64 = 1_000_000;

fn so() -> PathBuf {
    std::env::var_os("SCHEDINA_SO").map_or_else(
        || repo_root().join("programs/target/deploy-devnet/schedina.so"),
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
            .arg(so())
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

async fn token_balance(rpc: &RpcClient, account: &Pubkey) -> u64 {
    rpc.get_account(account).await.unwrap().map_or(0, |a| {
        u64::from_le_bytes(a.data[64..72].try_into().unwrap())
    })
}

async fn lamports(rpc: &RpcClient, account: &Pubkey) -> u64 {
    rpc.get_account(account)
        .await
        .unwrap()
        .map_or(0, |a| a.lamports)
}

fn memo(text: &[u8], signer: &Pubkey) -> Instruction {
    Instruction {
        program_id: MEMO_PROGRAM_ID,
        accounts: vec![AccountMeta::new_readonly(*signer, true)],
        data: text.to_vec(),
    }
}

/// The user's half of a sponsored legacy transaction: fee payer `sponsor`, slot 0 zeroed.
async fn user_signed(
    rpc: &RpcClient,
    ixs: &[Instruction],
    sponsor: &Pubkey,
    user: &Keypair,
) -> String {
    let (blockhash, _) = rpc.latest_blockhash().await.unwrap();
    let message =
        VersionedMessage::Legacy(Message::new_with_blockhash(ixs, Some(sponsor), &blockhash));
    let mut signatures = vec![Signature::default(); 2];
    assert_eq!(message.static_account_keys()[1], user.pubkey());
    signatures[1] = user.sign(&message.serialize());
    let tx = VersionedTransaction {
        signatures,
        message,
    };
    base64::engine::general_purpose::STANDARD.encode(wincode::serialize(&tx).unwrap())
}

async fn relay(http: &reqwest::Client, base: &str, tx: &str) -> (u16, Value) {
    let r = http
        .post(format!("{base}/relay"))
        .json(&json!({"tx": tx}))
        .send()
        .await
        .unwrap();
    (r.status().as_u16(), r.json().await.unwrap())
}

async fn confirm(rpc: &RpcClient, sig: &str) {
    let t0 = Instant::now();
    loop {
        if let Some((status, err)) = rpc.signature_status(sig).await.unwrap() {
            assert!(err.is_none(), "{sig} failed: {err:?}");
            if status == "confirmed" || status == "finalized" {
                return;
            }
        }
        assert!(
            t0.elapsed() < Duration::from_secs(30),
            "{sig} not confirmed"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs solana-test-validator and the devnet-feature .so: just relay-chain-test"]
async fn a_user_with_no_sol_claims_and_bets_through_the_relay() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_test_writer()
        .try_init();
    assert!(
        so().exists(),
        "{} missing: run `just relay-chain-test`",
        so().display()
    );
    let dir = std::env::temp_dir().join(format!(
        "schedina-relay-chain-{}-{}",
        std::process::id(),
        desk::now_ms()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let authority_path = dir.join("authority.json");
    let authority = keygen(&authority_path);
    let quote_signer = keygen(&dir.join("quote-signer.json"));
    let crank = keygen(&dir.join("crank.json"));
    let sponsor = keygen(&dir.join("sponsor.json"));
    let user = keygen(&dir.join("user.json"));
    let validator = Validator::start(dir.clone(), &authority.pubkey()).await;
    let url = validator.url();
    for k in [&authority, &sponsor] {
        airdrop(&url, &authority_path, &k.pubkey());
    }
    let rpc = RpcClient::new(&url);
    let s = Pdas::new(SCHEDINA_ID.parse().unwrap());
    let (a, sp, u) = (authority.pubkey(), sponsor.pubkey(), user.pubkey());
    let usdc = s.usdc_mint();
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
    assert_eq!(lamports(&rpc, &u).await, 0, "the user starts with no SOL");

    // The desk, with the sponsor key and the other desk keys apart from it.
    let db = TempDb::new("relay-chain");
    let mut cfg = Config::local(pascal_stub().await, db.path());
    cfg.solana_rpc_url = url.clone();
    cfg.program_id = s.program;
    cfg.sponsor_keypair_path = dir.join("sponsor.json");
    cfg.crank_keypair_path = dir.join("crank.json");
    cfg.quote_signer_keypair_path = dir.join("quote-signer.json");
    cfg.authority_keypair_path = authority_path.clone();
    cfg.shutdown_grace = Duration::from_secs(5);
    let running = desk::start(cfg).await.expect("desk starts");
    let base = format!("http://{}", running.addr);
    let http = reqwest::Client::new();
    let info: Value = http
        .get(format!("{base}/relay"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    println!("GET /relay -> {info}");
    assert_eq!(info["sponsor"], sp.to_string());
    assert_eq!(info["max_lamports_per_tx"], 10_000_000);
    let sponsor_start = lamports(&rpc, &sp).await;

    // 1. The faucet claim: the sponsor pays the account, the claim's rent and the fee.
    let faucet = [
        ix::create_ata_idempotent(&sp, &u, &usdc),
        ix::faucet_usdc(&s, &sp, &u, 1_000 * USDC),
        memo(FAUCET_MEMO, &u),
    ];
    let tx = user_signed(&rpc, &faucet, &sp, &user).await;
    let (code, body) = relay(&http, &base, &tx).await;
    println!("POST /relay (faucet) -> {code} {body}");
    assert_eq!(code, 200, "{body}");
    confirm(&rpc, body["signature"].as_str().unwrap()).await;
    let user_usdc = associated_token_address(&u, &usdc);
    assert_eq!(token_balance(&rpc, &user_usdc).await, 1_000 * USDC);
    let faucet_cost = sponsor_start - lamports(&rpc, &sp).await;
    println!("faucet claim cost the sponsor {faucet_cost} lamports");
    assert!(faucet_cost > 0 && faucet_cost <= 10_000_000);

    // A second claim right away lands too (no cooldown, D-661).
    let again = [
        ix::create_ata_idempotent(&sp, &u, &usdc),
        ix::faucet_usdc(&s, &sp, &u, 999 * USDC),
        memo(FAUCET_MEMO, &u),
    ];
    let tx = user_signed(&rpc, &again, &sp, &user).await;
    let (code, body) = relay(&http, &base, &tx).await;
    println!("POST /relay (second claim) -> {code} {body}");
    assert_eq!(code, 200, "{body}");
    confirm(&rpc, body["signature"].as_str().unwrap()).await;
    assert_eq!(token_balance(&rpc, &user_usdc).await, 1_999 * USDC);

    // 2. The bet: [Ed25519 verify, open_pending(payer = sponsor)].
    let now = rpc.cluster_time().await.unwrap();
    let legs: Vec<QuoteLeg> = (0..2)
        .map(|g: u8| QuoteLeg {
            symbol_hash: sha256(format!("NBA_SYM_{g}").as_bytes()),
            game_id_hash: sha256(format!("nba-game-{g}").as_bytes()),
            start_ts: now + 7_200,
            side: g % 2,
            price_u: 500_000,
            m1_ppm: 50_000,
        })
        .collect();
    let stake = 10 * USDC;
    let payout = pricing::payout(
        stake,
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
    let msg = QuoteMsg {
        program_id: s.program,
        legs,
        stake_u: stake,
        odds_offered_u: payout.odds_u,
        quote_expires_at: now + 20,
    }
    .encode();
    let sig: [u8; 64] = quote_signer.sign(&msg).into();
    let nonce = 7;
    let bet = [
        ix::ed25519_verify(&msg, &sig, &quote_signer.pubkey()),
        ix::open_pending(&s, &u, &sp, nonce, payout.odds_u, now + 45, false, None),
    ];
    let tx = user_signed(&rpc, &bet, &sp, &user).await;
    let (code, body) = relay(&http, &base, &tx).await;
    println!("POST /relay (bet) -> {code} {body}");
    assert_eq!(code, 200, "{body}");
    confirm(&rpc, body["signature"].as_str().unwrap()).await;
    let acc = rpc
        .get_account(&s.pending(&u, nonce))
        .await
        .unwrap()
        .expect("pending");
    let pending = PendingAccount::decode(&acc.data, &acc.owner, &s.program).unwrap();
    assert_eq!((pending.user, pending.stake_u), (u, stake));
    assert_eq!(pending.rent_payer, sp);
    assert_eq!(pending.rent_receiver(), sp);
    assert_eq!(token_balance(&rpc, &s.escrow_usdc()).await, stake);
    assert_eq!(token_balance(&rpc, &user_usdc).await, 990 * USDC);

    // 3. A System transfer out of the sponsor: refused, nothing sent.
    let before = lamports(&rpc, &sp).await;
    let mut data = 2u32.to_le_bytes().to_vec();
    data.extend_from_slice(&1_000_000_000u64.to_le_bytes());
    let steal = [
        Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![AccountMeta::new(sp, true), AccountMeta::new(u, false)],
            data,
        },
        memo(b"thanks", &u),
    ];
    let tx = user_signed(&rpc, &steal, &sp, &user).await;
    let (code, body) = relay(&http, &base, &tx).await;
    println!("POST /relay (transfer from the sponsor) -> {code} {body}");
    assert_eq!(code, 422, "{body}");
    assert_eq!(body["status"], "rejected");
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(lamports(&rpc, &sp).await, before);

    assert_eq!(lamports(&rpc, &u).await, 0, "the user never held SOL");
    println!(
        "sponsor spent {} lamports in all",
        sponsor_start - lamports(&rpc, &sp).await
    );
    running.join().await;
    drop(validator);
}
