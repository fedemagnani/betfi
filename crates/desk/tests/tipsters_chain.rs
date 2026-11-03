//! Plan 4.A review gate on localnet (l4-contracts L4-5), `#[ignore]`d; run by
//! `just desk-chain-test` after the 2.C chain test. A private `solana-test-validator` loads the
//! `--features devnet` `.so` (as tests/chain.rs does); the desk runs in-process with its real
//! quoter, hedge desk, crank and indexer against the in-process Pascal mock on a **synthetic
//! tape dated in the future**: the sample tape with every Unix-ms field moved forward so its
//! clock starts now and its games tip off hours after the chain clock (tips and copies need
//! unstarted legs; the validator clock is never touched). Every key is generated in a temp dir.
//!
//! In order: wallet A quotes and opens an own tip (`is_tip`), the desk hedges it on the mock and
//! the crank finalizes it with the tipster profile (L2-4); wallet B asks
//! `POST /ticket/{id}/copy-quote` (on by default in `dev`), opens the copy with `copied_from`,
//! and the crank finalizes it with the original ticket and A's USDC account (idempotent ATA
//! create): 2% copy fee, 80% of it to A (`TipsterPaid`). Both legs resolve on the mock, both
//! tickets settle, and `/tipsters/{A}` shows the numbers, which agree with the on-chain
//! `TipsterProfile`; `/house` agrees with the vault. Last, `crank pause` makes `/quote` answer
//! 503 through the indexer's mirror (D-121), and `crank unpause` lifts it.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine;
use common::{TempDb, repo_root};
use desk::api::types::{
    ErrorBody, HouseResponse, QuoteLegRequest, QuoteRequest, QuoteResponse, Side, Ticket,
    TicketStatus, TipsterResponse, TipstersResponse,
};
use desk::program::ix;
use desk::program::{Pdas, TicketAccount, VaultAccount, associated_token_address};
use desk::solana::rpc::RpcClient;
use desk::solana::signer::{Keypair, send};
use desk::state::{Recv, recv};
use desk::tipsters::chain::TipsterProfileAccount;
use desk::{Config, DeskEvent, Running};
use pascal_mock::{At, Mock, MockConfig, ScriptSource};
use serde_json::Value;
use solana_pubkey::Pubkey;
use tokio::sync::broadcast;

const PROGRAM_ID: &str = "HXdZFi7tR669G1hXogh3uGGqLYsoZPCSuiutAviLNToW";
/// SIMD-0500: no new SBPF v0–v2 deployments (D-006).
const SIMD_0500: &str = "B8JJXCy5amZyWG9r7EnUYLwzXSXTxG7GZ1qZ1qggo83g";
const PHI: &str = "NBA_26OCT28_PHIIND_ML.PHI";
const ORL: &str = "NBA_26OCT28_ORLTOR_ML.ORL";
const HOU: &str = "NBA_26OCT28_HOUMIL_ML.HOU";
const SAS: &str = "NBA_26OCT28_SASUTA_ML.SAS";
const U: u64 = 1_000_000;

fn so_path() -> PathBuf {
    std::env::var_os("SCHEDINA_SO").map_or_else(
        || repo_root().join("programs/target/deploy-devnet/schedina.so"),
        PathBuf::from,
    )
}

/// A private `solana-test-validator`; killed on drop (as in tests/chain.rs).
struct Validator {
    dir: PathBuf,
    rpc_port: u16,
    authority: Pubkey,
    child: Option<Child>,
}

impl Validator {
    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.rpc_port)
    }

    async fn start(&mut self) {
        let p = self.rpc_port;
        let log = std::fs::File::create(self.dir.join("validator.log")).unwrap();
        let child = Command::new("solana-test-validator")
            .arg("--ledger")
            .arg(self.dir.join("ledger"))
            .args(["--quiet", "--bind-address", "127.0.0.1"])
            .args(["--rpc-port", &p.to_string()])
            .args(["--faucet-port", &(p + 2).to_string()])
            .args(["--gossip-port", &(p + 3).to_string()])
            .args(["--dynamic-port-range", &format!("{}-{}", p + 4, p + 40)])
            .arg("--upgradeable-program")
            .arg(PROGRAM_ID)
            .arg(so_path())
            .arg(self.authority.to_string())
            .args(["--deactivate-feature", SIMD_0500])
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .expect("solana-test-validator on PATH");
        self.child = Some(child);
        let rpc = RpcClient::new(self.url());
        let t0 = Instant::now();
        loop {
            if rpc.block_height().await.is_ok_and(|h| h > 1) {
                return;
            }
            assert!(
                t0.elapsed() < Duration::from_secs(60),
                "validator not up; see {}",
                self.dir.join("validator.log").display()
            );
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for Validator {
    fn drop(&mut self) {
        self.stop();
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

fn cli(url: &str, authority: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_crank"))
        .args(["--rpc", url, "--authority"])
        .arg(authority)
        .args(args)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "crank {args:?}: {}{}",
        text,
        String::from_utf8_lossy(&out.stderr)
    );
    text
}

/// Moves every Unix-ms value (keys `recv_ms`, `*_ms`, `*_ms_*`; numbers or numeric strings at
/// or above 1e12) forward by `delta`, keeping its JSON type.
fn shift(v: &mut Value, delta: u64) {
    let is_time = |k: &str| k == "recv_ms" || k.ends_with("_ms") || k.contains("_ms_");
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if is_time(k) {
                    match x {
                        Value::Number(n) => {
                            if let Some(t) = n.as_u64().filter(|t| *t >= 1_000_000_000_000) {
                                *x = Value::from(t + delta);
                            }
                        }
                        Value::String(s) => {
                            if let Some(t) =
                                s.parse::<u64>().ok().filter(|t| *t >= 1_000_000_000_000)
                            {
                                *s = (t + delta).to_string();
                            }
                        }
                        _ => shift(x, delta),
                    }
                } else {
                    shift(x, delta);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| shift(x, delta)),
        _ => {}
    }
}

/// The sample tape, dated so its first book snapshot is a minute ago (L4-5). Returns the tape
/// directory (under `dir`) and the shift.
fn future_tape(dir: &Path) -> (PathBuf, u64) {
    let src = repo_root().join("tapes/nba-2026-10-28/sample");
    let out = dir.join("tape");
    std::fs::create_dir_all(&out).unwrap();
    let first: Value = serde_json::from_str(
        std::fs::read_to_string(src.join("books.jsonl"))
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let t0: u64 = first["body"]["state_time_ms"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| first["recv_ms"].as_u64().unwrap());
    let delta = (desk::now_ms() - 60_000).saturating_sub(t0);
    for f in [
        "books.jsonl",
        "trades.jsonl",
        "search.jsonl",
        "markets.jsonl",
    ] {
        let text = std::fs::read_to_string(src.join(f)).unwrap();
        let mut body = String::with_capacity(text.len());
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let mut v: Value = serde_json::from_str(line).unwrap();
            shift(&mut v, delta);
            body.push_str(&serde_json::to_string(&v).unwrap());
            body.push('\n');
        }
        std::fs::write(out.join(f), body).unwrap();
    }
    (out, delta)
}

async fn wait_for(
    rx: &mut broadcast::Receiver<DeskEvent>,
    secs: u64,
    what: &str,
    mut pred: impl FnMut(&DeskEvent) -> bool,
) -> DeskEvent {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let r = tokio::time::timeout_at(deadline, recv(rx, "tipsters-chain-test"))
            .await
            .unwrap_or_else(|_| panic!("no {what} within {secs} s"));
        match r {
            Recv::Event(e) if pred(&e) => return e,
            Recv::Event(_) | Recv::Lagged(_) => {}
            Recv::Closed => panic!("bus closed waiting for {what}"),
        }
    }
}

async fn usdc_balance(rpc: &RpcClient, owner: &Pubkey, mint: &Pubkey) -> u64 {
    let acc = rpc
        .get_account(&associated_token_address(owner, mint))
        .await
        .unwrap()
        .unwrap();
    u64::from_le_bytes(acc.data[64..72].try_into().unwrap())
}

async fn ticket_account(rpc: &RpcClient, pdas: &Pdas, key: &Pubkey) -> TicketAccount {
    let a = rpc.get_account(key).await.unwrap().unwrap();
    TicketAccount::decode(&a.data, &a.owner, &pdas.program).unwrap()
}

async fn get<T: serde::de::DeserializeOwned>(desk: &Running, path: &str) -> (u16, T) {
    let r = reqwest::get(format!("http://{}{path}", desk.addr))
        .await
        .unwrap();
    let s = r.status().as_u16();
    (s, r.json().await.unwrap())
}

async fn post(desk: &Running, path: &str, body: &Value) -> (u16, Value) {
    let r = reqwest::Client::new()
        .post(format!("http://{}{path}", desk.addr))
        .json(body)
        .send()
        .await
        .unwrap();
    let s = r.status().as_u16();
    (s, r.json().await.unwrap())
}

/// A wallet that quotes and opens pendings the way the app does (e2e-user's flow).
struct Wallet<'a> {
    desk: &'a Running,
    rpc: &'a RpcClient,
    pdas: Pdas,
    key: Keypair,
}

impl Wallet<'_> {
    /// `path` + `body` → quote → `[ed25519 verify, open_pending]`, re-quoted on a failed send.
    async fn open(
        &self,
        nonce: u64,
        path: &str,
        body: &Value,
        is_tip: bool,
        copied_from: Option<&Pubkey>,
    ) -> QuoteResponse {
        let mut last = String::new();
        for _ in 0..3 {
            let (s, v) = post(self.desk, path, body).await;
            assert_eq!(s, 200, "POST {path}: {v}");
            let q: QuoteResponse = serde_json::from_value(v).unwrap();
            let b64 = base64::engine::general_purpose::STANDARD;
            let msg = b64.decode(&q.quote_msg).unwrap();
            let sig: [u8; 64] = b64.decode(&q.quote_sig).unwrap().try_into().unwrap();
            let signer: Pubkey = q.quote_signer.parse().unwrap();
            let now = self.rpc.cluster_time().await.unwrap();
            let user = self.key.pubkey();
            let ixs = [
                ix::ed25519_verify(&msg, &sig, &signer),
                ix::open_pending(
                    &self.pdas,
                    &user,
                    &user,
                    nonce,
                    q.min_payout_per_unit_u,
                    now + 45,
                    is_tip,
                    copied_from,
                ),
            ];
            match send(self.rpc, &ixs, &[&self.key]).await {
                Ok(_) => return q,
                Err(e) => last = e.to_string(),
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        panic!("open_pending {nonce} ({path}): {last}");
    }
}

fn legs_body(stake_u: u64, symbols: [&str; 2]) -> Value {
    serde_json::to_value(QuoteRequest {
        legs: symbols
            .iter()
            .map(|s| QuoteLegRequest {
                symbol: (*s).into(),
                side: Side::Long,
            })
            .collect(),
        stake_u,
        token_in: None,
        copied_from: None,
        title: None,
        wallet: None,
    })
    .unwrap()
}

/// `floor((r̃ − p)²)` summed and the signed sum, over legs that all won (r̃ = 1).
fn brier_edge(t: &TicketAccount) -> (u64, i64) {
    let n = t.legs.len() as i128;
    let d: Vec<i128> = t
        .legs
        .iter()
        .map(|l| 1_000_000 - i128::from(l.fill_u))
        .collect();
    let sq: i128 = d.iter().map(|x| x * x).sum();
    (
        (sq / (n * 1_000_000)) as u64,
        (d.iter().sum::<i128>() / n) as i64,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs solana-test-validator and the devnet-feature .so: just desk-chain-test"]
async fn tip_copy_settle_and_pause_on_localnet() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_test_writer()
        .try_init();
    let so = so_path();
    assert!(
        so.exists(),
        "{} missing: run `just desk-chain-test`",
        so.display()
    );
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("schedina-tipsters-{pid}-{}", desk::now_ms()));
    std::fs::create_dir_all(&dir).unwrap();
    let authority_path = dir.join("authority.json");
    let crank_path = dir.join("crank.json");
    let qs_path = dir.join("quote-signer.json");
    let authority = keygen(&authority_path);
    let crank = keygen(&crank_path);
    let quote_signer = keygen(&qs_path);
    let tipster = keygen(&dir.join("tipster.json"));
    let copier = keygen(&dir.join("copier.json"));
    // The hedger on the mock: owner, trading key (seed file) and builder, as in tests/hedge.rs.
    let trading_path = dir.join("hedger-trading.json");
    std::fs::write(
        &trading_path,
        serde_json::to_string(&[8u8; 32].to_vec()).unwrap(),
    )
    .unwrap();
    let mut validator = Validator {
        dir: dir.clone(),
        rpc_port: 21_000 + u16::try_from(pid % 900).unwrap() * 40,
        authority: authority.pubkey(),
        child: None,
    };
    validator.start().await;
    let url = validator.url();
    for k in [&authority, &crank, &tipster, &copier] {
        airdrop(&url, &authority_path, &k.pubkey());
    }
    let s = |k: &Keypair| k.pubkey().to_string();
    cli(
        &url,
        &authority_path,
        &[
            "init-config",
            "--crank",
            &s(&crank),
            "--quote-signer",
            &s(&quote_signer),
        ],
    );
    cli(
        &url,
        &authority_path,
        &["faucet", "--to", &s(&authority), "--usdc", "10000"],
    );
    cli(&url, &authority_path, &["fund-vault", "--usdc", "9000"]);
    for w in [&tipster, &copier] {
        cli(
            &url,
            &authority_path,
            &["faucet", "--to", &s(w), "--usdc", "100"],
        );
    }

    // The mock on the future-dated tape, clock frozen at now.
    let (tape, delta) = future_tape(&dir);
    let mut mcfg = MockConfig::new(&tape);
    mcfg.speed = 0.0;
    mcfg.start_ms = Some(desk::now_ms());
    mcfg.script = ScriptSource::None;
    let mock = Mock::new(mcfg).unwrap().spawn("127.0.0.1:0").await.unwrap();
    eprintln!("tape shifted by {} h", delta / 3_600_000);

    let db = TempDb::new("tipsters-chain");
    let mut cfg = Config::local(mock.base_url(), db.path());
    cfg.chain_enabled = true;
    cfg.solana_rpc_url = url.clone();
    cfg.crank_keypair_path = crank_path.clone();
    cfg.quote_signer_keypair_path = qs_path.clone();
    cfg.pascal_trade_url = Some(mock.base_url());
    cfg.hedger_owner = Some(Pubkey::new_from_array([7; 32]).to_string());
    cfg.hedger_trading_key_path = Some(trading_path.clone());
    cfg.builder_pubkey = Some(Pubkey::new_from_array([9; 32]).to_string());
    // Unset DESK_COPY_QUOTE: on in dev (L4-2).
    cfg.copy_quote =
        Config::from_lookup(|k| (k == "PASCAL_DATA_URL").then(|| "http://127.0.0.1:1".to_owned()))
            .unwrap()
            .copy_quote;
    assert!(cfg.copy_quote);
    cfg.resolve_poll_every = Duration::from_secs(1);
    cfg.resolved_sweep_every = Duration::from_secs(2);
    cfg.crank_tick = Duration::from_millis(500);
    cfg.chain_resync_every = Duration::from_secs(5);
    cfg.shutdown_grace = Duration::from_secs(5);
    let running = desk::start(cfg).await.expect("desk starts");
    let state = running.state.clone();
    let mut rx = state.subscribe();
    for _ in 0..100 {
        if state.catalogue().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(state.catalogue().is_some(), "catalogue from the mock");
    let pdas = Pdas::new(PROGRAM_ID.parse().unwrap());
    let mint = pdas.usdc_mint();
    let rpc = RpcClient::new(&url);
    let a = Wallet {
        desk: &running,
        rpc: &rpc,
        pdas,
        key: tipster,
    };
    let b = Wallet {
        desk: &running,
        rpc: &rpc,
        pdas,
        key: copier,
    };
    let (a_pk, b_pk) = (a.key.pubkey(), b.key.pubkey());

    // 1. A's own tip, 10 USDC on PHI and ORL (both tip off hours after the chain clock).
    let q = a
        .open(1, "/quote", &legs_body(10 * U, [PHI, ORL]), true, None)
        .await;
    assert_eq!(q.copy_fee_u, 0);
    let tip = pdas.ticket(&a_pk, 1);
    wait_for(
        &mut rx,
        90,
        "tip minted",
        |e| matches!(e, DeskEvent::TicketMinted { ticket, .. } if *ticket == tip.to_string()),
    )
    .await;
    let tip_acc = ticket_account(&rpc, &pdas, &tip).await;
    assert_eq!(tip_acc.tipster, Some(a_pk));
    assert!(tip_acc.is_own_tip());
    let profile_key = pdas.tipster(&a_pk);
    let profile = rpc
        .get_account(&profile_key)
        .await
        .unwrap()
        .expect("profile");
    let p = TipsterProfileAccount::decode(&profile.data, &profile.owner, &pdas.program).unwrap();
    assert_eq!((p.open_count, p.open_staked), (1, tip_acc.stake_u));
    let (_, open): (u16, TipsterResponse) = get(&running, &format!("/tipsters/{a_pk}")).await;
    assert_eq!((open.tipster.settled, open.tipster.open_count), (0, 1));
    assert_eq!(open.open[0].ticket, tip.to_string());

    // 2. B copies it: copy-quote with the 2% fee line, open_pending with copied_from.
    let a_before = usdc_balance(&rpc, &a_pk, &mint).await;
    let b_before = usdc_balance(&rpc, &b_pk, &mint).await;
    let cq = b
        .open(
            1,
            &format!("/ticket/{tip}/copy-quote"),
            &serde_json::json!({"stake_u": 10 * U}),
            false,
            Some(&tip),
        )
        .await;
    assert_eq!(cq.copy_fee_u, 200_000, "ceil(2% of 10 USDC), D-064");
    assert_eq!(
        cq.legs
            .iter()
            .map(|l| l.symbol.as_str())
            .collect::<Vec<_>>(),
        [PHI, ORL]
    );
    let copy = pdas.ticket(&b_pk, 1);
    wait_for(
        &mut rx,
        90,
        "copy minted",
        |e| matches!(e, DeskEvent::TicketMinted { ticket, .. } if *ticket == copy.to_string()),
    )
    .await;
    let copy_acc = ticket_account(&rpc, &pdas, &copy).await;
    assert_eq!(copy_acc.copied_from, Some(tip));
    assert_eq!(copy_acc.tipster, Some(a_pk));
    // D-111: the fee charged is floor(fee × r); the tipster gets floor(80%) of it.
    let used = 200_000 * u64::from(copy_acc.r_ppm) / 1_000_000;
    assert_eq!(copy_acc.copy_fee_u, used);
    let share = used * 8_000 / 10_000;
    let a_after = usdc_balance(&rpc, &a_pk, &mint).await;
    assert_eq!(a_after - a_before, share, "TipsterPaid to A's ATA");
    assert_eq!(
        b_before - usdc_balance(&rpc, &b_pk, &mint).await,
        copy_acc.stake_u + used,
        "B paid rS plus the fee charged"
    );
    let row: (String, i64, String) =
        sqlx::query_as("SELECT tipster, amount_u, source FROM tipster_paid WHERE ticket = ?")
            .bind(copy.to_string())
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(row, (a_pk.to_string(), share as i64, "event".to_owned()));
    let (_, ct): (u16, Ticket) = get(&running, &format!("/ticket/{copy}")).await;
    assert_eq!(ct.copied_from.as_deref(), Some(tip.to_string().as_str()));
    assert_eq!(ct.tipster.as_deref(), Some(a_pk.to_string().as_str()));

    // 3. Both legs win on the mock; the crank resolves and settles both tickets.
    for sym in [PHI, ORL] {
        mock.mock.with_engine(|e| {
            e.schedule_resolution(sym, 1_000_000, At::Now).unwrap();
        });
    }
    let mut settled = Vec::new();
    while settled.len() < 2 {
        let e = wait_for(&mut rx, 120, "TicketSettled", |e| {
            matches!(e, DeskEvent::TicketSettled { .. })
        })
        .await;
        if let DeskEvent::TicketSettled { ticket, paid_u, .. } = e {
            settled.push((ticket, paid_u));
        }
    }
    let tip_acc = ticket_account(&rpc, &pdas, &tip).await;
    let copy_acc = ticket_account(&rpc, &pdas, &copy).await;
    assert_eq!(tip_acc.paid_u, tip_acc.payout_u);
    assert_eq!(copy_acc.paid_u, copy_acc.payout_u);

    // 4. /tipsters/{A}: by hand from the chain accounts, and equal to the program's profile.
    let (s, t): (u16, TipsterResponse) = get(&running, &format!("/tipsters/{a_pk}")).await;
    assert_eq!(s, 200);
    let r = &t.tipster;
    let roi = (i128::from(tip_acc.paid_u) - i128::from(tip_acc.stake_u)) * 1_000_000
        / i128::from(tip_acc.stake_u);
    let (brier, edge) = brier_edge(&tip_acc);
    let f_staked = i128::from(copy_acc.stake_u + copy_acc.copy_fee_u);
    let f_roi = (i128::from(copy_acc.paid_u) - f_staked) * 1_000_000 / f_staked;
    eprintln!("tipster row: {r:?}");
    assert_eq!(
        (r.settled, r.won, r.hit_rate_ppm, r.streak),
        (1, 1, 1_000_000, 1)
    );
    assert_eq!(r.roi_ppm, roi as i64);
    assert_eq!((r.brier_ppm, r.edge_ppm), (brier, edge));
    assert_eq!(r.avg_odds_u, tip_acc.odds_u);
    assert_eq!(r.skin_u, tip_acc.stake_u);
    assert_eq!(r.followers_roi_ppm, Some(f_roi as i64));
    assert_eq!((r.open_count, r.open_staked_u), (0, 0));
    assert_eq!((r.copies, r.fees_earned_u), (1, share));
    assert_eq!(t.history.len(), 1);
    assert_eq!(t.history[0].ticket, tip.to_string());
    assert_eq!(t.history[0].roi_cum_ppm, r.roi_ppm);
    assert_eq!(t.settled.len(), 1);
    assert!(t.open.is_empty());
    assert_eq!(t.settled[0].state, TicketStatus::Settled);
    assert_eq!(
        t.settled[0].tx.resolve_leg.len(),
        2,
        "one resolve_leg per leg"
    );
    let profile = rpc.get_account(&profile_key).await.unwrap().unwrap();
    let p = TipsterProfileAccount::decode(&profile.data, &profile.owner, &pdas.program).unwrap();
    assert_eq!(
        (p.settled_count, p.won, p.open_count),
        (r.settled, r.won, r.open_count)
    );
    assert_eq!(
        (p.settled_staked, p.returned),
        (tip_acc.stake_u, tip_acc.paid_u)
    );
    // B only copied: not a tipster. A is on the all=1 board, not the default one (1 < 5).
    let (s, _): (u16, ErrorBody) = get(&running, &format!("/tipsters/{b_pk}")).await;
    assert_eq!(s, 404);
    let (_, all): (u16, TipstersResponse) = get(&running, "/tipsters?all=1").await;
    assert_eq!(all.tipsters, vec![r.clone()]);
    let (_, board): (u16, TipstersResponse) = get(&running, "/tipsters").await;
    assert!(board.tipsters.is_empty());

    // 5. /house against the vault account.
    let (_, h): (u16, HouseResponse) = get(&running, "/house").await;
    let v = rpc.get_account(&pdas.vault()).await.unwrap().unwrap();
    let v = VaultAccount::decode(&v.data, &v.owner, &pdas.program).unwrap();
    assert_eq!((h.vault.free_u, h.vault.locked_u), (v.free, v.locked));
    assert_eq!(h.vault.reserve_u, 100 * U);
    assert_eq!((h.tickets.settled, h.tickets.open), (2, 0));
    assert_eq!(h.fees.margin_u, tip_acc.fee_u + copy_acc.fee_u);
    assert_eq!(h.fees.copy_fees_u, used - share);
    assert!(h.fees.pascal_fees_u != 0, "hedge fills carry taker fees");
    assert!(h.hedger.free_collateral_u > 0);
    eprintln!("house: {h:?}");

    // 6. Pause (D-121): the CLI pauses on-chain, the indexer mirrors it, /quote is 503.
    let out = cli(&url, &authority_path, &["pause"]);
    assert!(out.contains("paused true"), "{out}");
    let t0 = Instant::now();
    while !state.is_paused() {
        assert!(t0.elapsed() < Duration::from_secs(30), "pause not mirrored");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // Two games still to play (PHI and ORL resolved above).
    let (s, e) = post(&running, "/quote", &legs_body(5 * U, [HOU, SAS])).await;
    assert_eq!(s, 503, "{e}");
    assert!(e["reason"].as_str().unwrap().contains("paused"), "{e}");
    let again = cli(&url, &authority_path, &["pause"]);
    assert!(again.contains("already"), "{again}");
    cli(&url, &authority_path, &["unpause"]);
    let t0 = Instant::now();
    while state.is_paused() {
        assert!(
            t0.elapsed() < Duration::from_secs(30),
            "unpause not mirrored"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let (s, e) = post(&running, "/quote", &legs_body(5 * U, [HOU, SAS])).await;
    assert_eq!(s, 200, "{e}");

    running.join().await;
    validator.stop();
}
