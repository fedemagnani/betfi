//! Plan 2.C review gate on localnet (l2-contracts L2-6), `#[ignore]`d; run by
//! `just desk-chain-test`, which builds the `--features devnet` `.so` first (the faucet is
//! devnet-only). A `solana-test-validator` on private ports loads it as an upgradeable program
//! owned by a fresh test authority, with SIMD-0500 deactivated (D-006); the desk runs in-process
//! against it and against the in-process Pascal mock (sample tape, frozen clock). Every key is
//! generated in a temp dir.
//!
//! One test, in order: CLI `init-config` / `faucet` / `fund-vault` / `status`; a pending opened
//! with a test-signed quote becomes a ticket after a synthetic `HedgeFilled`; a scripted Pascal
//! resolution reaches `resolve_leg` within one poll period, and `settle` follows once both legs
//! are in; a floor breach becomes `FinalizeRejected`; pendings with no fills are refunded by the
//! crank's timer; the listener and the crank survive a validator restart.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{TempDb, repo_root};
use desk::api::types::{LegStatus, QuoteLeg as ApiQuoteLeg, Side, TeamSide, TicketStatus};
use desk::program::ix::{self, QuoteMsg};
use desk::program::{Pdas, QuoteLeg, associated_token_address, hex, sha256};
use desk::solana::rpc::RpcClient;
use desk::solana::signer::{Keypair, send};
use desk::state::{HedgeFilled, Recv, recv};
use desk::{Config, DeskEvent, Running};
use pascal_mock::{At, Mock, MockConfig, ScriptSource};
use solana_pubkey::Pubkey;
use tokio::sync::broadcast;

const PROGRAM_ID: &str = "HXdZFi7tR669G1hXogh3uGGqLYsoZPCSuiutAviLNToW";
/// SIMD-0500: no new SBPF v0–v2 deployments (D-006).
const SIMD_0500: &str = "B8JJXCy5amZyWG9r7EnUYLwzXSXTxG7GZ1qZ1qggo83g";

const PHI: (&str, &str, i64) = (
    "NBA_26OCT28_PHIIND_ML.PHI",
    "nba-phi-ind-2026-10-28",
    1_793_228_400,
);
const ORL: (&str, &str, i64) = (
    "NBA_26OCT28_ORLTOR_ML.ORL",
    "nba-orl-tor-2026-10-28",
    1_793_230_200,
);

fn so_path() -> PathBuf {
    std::env::var_os("SCHEDINA_SO").map_or_else(
        || repo_root().join("programs/target/deploy-devnet/schedina.so"),
        PathBuf::from,
    )
}

/// A private `solana-test-validator`; killed on drop.
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

    /// Starts (or, with an existing ledger, resumes) the validator and waits until it answers.
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

/// Runs the `crank` CLI built with this test.
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

fn now_s() -> i64 {
    i64::try_from(desk::now_ms() / 1000).unwrap()
}

fn leg((symbol, game, start): (&str, &str, i64), side: u8, price_u: u64) -> QuoteLeg {
    QuoteLeg {
        symbol_hash: sha256(symbol.as_bytes()),
        game_id_hash: sha256(game.as_bytes()),
        start_ts: start,
        side,
        price_u,
        m1_ppm: 50_000,
    }
}

/// What the test needs to open pendings: the program, the user and the quote signer.
struct Opener {
    rpc: RpcClient,
    pdas: Pdas,
    user: Keypair,
    quote_signer: Keypair,
    db: sqlx::SqlitePool,
}

impl Opener {
    /// Signs a quote for `legs` (C3 layout, D-050), stores it as 2.A would (L2-1, unless
    /// `store` is false: the indexer then names the legs from the catalogue), and sends
    /// `[ed25519 verify, open_pending]` with the floor 3% under the offered odds.
    async fn open(
        &self,
        nonce: u64,
        legs: &[(QuoteLeg, &str, &str, TeamSide)],
        ttl: i64,
        store: bool,
    ) -> Pubkey {
        self.open_as(nonce, legs, ttl, store, false).await
    }

    async fn open_as(
        &self,
        nonce: u64,
        legs: &[(QuoteLeg, &str, &str, TeamSide)],
        ttl: i64,
        store: bool,
        is_tip: bool,
    ) -> Pubkey {
        // A restarted test validator's clock settles over its first slots; re-sign and retry.
        let mut last = String::new();
        for _ in 0..5 {
            match self.try_open(nonce, legs, ttl, store, is_tip).await {
                Ok(k) => return k,
                Err(e) => last = e,
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        panic!("open_pending {nonce}: {last}");
    }

    async fn try_open(
        &self,
        nonce: u64,
        legs: &[(QuoteLeg, &str, &str, TeamSide)],
        ttl: i64,
        store: bool,
        is_tip: bool,
    ) -> Result<Pubkey, String> {
        let stake_u = 5_000_000;
        let payout = pricing::payout(
            stake_u,
            1_000_000,
            &legs
                .iter()
                .map(|(l, ..)| pricing::PayoutLeg {
                    m1_ppm: l.m1_ppm,
                    f_u: l.price_u,
                })
                .collect::<Vec<_>>(),
        )
        .unwrap();
        // The program judges expiries by the cluster clock, which may trail the wall clock
        // (notably right after a validator restart).
        let now = self.rpc.cluster_time().await.unwrap_or_else(|_| now_s());
        let msg = QuoteMsg {
            program_id: self.pdas.program,
            legs: legs.iter().map(|(l, ..)| *l).collect(),
            stake_u,
            odds_offered_u: payout.odds_u,
            quote_expires_at: now + 10,
        }
        .encode();
        let sig: [u8; 64] = self.quote_signer.sign(&msg).into();
        if store {
            let api_legs: Vec<ApiQuoteLeg> = legs
                .iter()
                .map(|(l, symbol, game, pick)| ApiQuoteLeg {
                    symbol: (*symbol).into(),
                    game_id: (*game).into(),
                    side: if l.side == 1 { Side::Short } else { Side::Long },
                    pick: *pick,
                    start_ms: u64::try_from(l.start_ts).unwrap() * 1000,
                    price_u: l.price_u,
                    p_max_u: l.price_u + 10_000,
                    m1_ppm: l.m1_ppm,
                    overlaps: true,
                    in_play: false,
                    kind: None,
                    line: None,
                    label: None,
                })
                .collect();
            sqlx::query(
                "INSERT INTO quotes (quote_hash, msg, sig, quote_signer, n, stake_u,
                    odds_offered_u, odds_fair_u, payout_u, fee_u, min_payout_per_unit_u,
                    quote_expires_at, legs_json, created_ms)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(hex(&sha256(&msg)))
            .bind(&msg)
            .bind(sig.to_vec())
            .bind(self.quote_signer.pubkey().to_string())
            .bind(legs.len() as i64)
            .bind(stake_u as i64)
            .bind(payout.odds_u as i64)
            .bind(payout.odds_u as i64 + 100_000)
            .bind(payout.payout_u as i64)
            .bind(250_000_i64)
            .bind((payout.odds_u * 97 / 100) as i64)
            .bind(now + 10)
            .bind(serde_json::to_string(&api_legs).unwrap())
            .bind(desk::now_ms() as i64)
            .execute(&self.db)
            .await
            .unwrap();
        }
        let user = self.user.pubkey();
        let ixs = [
            ix::ed25519_verify(&msg, &sig, &self.quote_signer.pubkey()),
            ix::open_pending(
                &self.pdas,
                &user,
                &user,
                nonce,
                payout.odds_u * 97 / 100,
                now + ttl,
                is_tip,
                None,
            ),
        ];
        send(&self.rpc, &ixs, &[&self.user])
            .await
            .map_err(|e| e.to_string())?;
        Ok(self.pdas.pending(&user, nonce))
    }
}

/// The first bus event matching `pred` within `secs`.
async fn wait_for(
    rx: &mut broadcast::Receiver<DeskEvent>,
    secs: u64,
    what: &str,
    mut pred: impl FnMut(&DeskEvent) -> bool,
) -> DeskEvent {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let r = tokio::time::timeout_at(deadline, recv(rx, "chain-test"))
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
    // SPL token account: mint 32, owner 32, amount u64 LE.
    u64::from_le_bytes(acc.data[64..72].try_into().unwrap())
}

async fn db_state(running: &Running, ticket: &str) -> Option<String> {
    sqlx::query_scalar("SELECT state FROM tickets WHERE ticket = ?")
        .bind(ticket)
        .fetch_optional(&running.state.db)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs solana-test-validator and the devnet-feature .so: just desk-chain-test"]
async fn crank_and_indexer_on_localnet() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_test_writer()
        .try_init();
    let so = so_path();
    assert!(
        so.exists(),
        "{} missing: run `just desk-chain-test` (builds it)",
        so.display()
    );
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("schedina-chain-{pid}-{}", desk::now_ms()));
    std::fs::create_dir_all(&dir).unwrap();
    let authority_path = dir.join("authority.json");
    let crank_path = dir.join("crank.json");
    let user_path = dir.join("user.json");
    let authority = keygen(&authority_path);
    let crank = keygen(&crank_path);
    let user = keygen(&user_path);
    let quote_signer = keygen(&dir.join("quote-signer.json"));
    let mut validator = Validator {
        dir: dir.clone(),
        rpc_port: 20_000 + u16::try_from(pid % 1_000).unwrap() * 40,
        authority: authority.pubkey(),
        child: None,
    };
    validator.start().await;
    let url = validator.url();
    for k in [&authority, &crank, &user] {
        airdrop(&url, &authority_path, &k.pubkey());
    }

    // CLI: config, mock USDC, house money.
    let qs = quote_signer.pubkey().to_string();
    let crank_s = crank.pubkey().to_string();
    let out = cli(
        &url,
        &authority_path,
        &["init-config", "--crank", &crank_s, "--quote-signer", &qs],
    );
    assert!(out.contains("init_config"), "{out}");
    let auth_s = authority.pubkey().to_string();
    let user_s = user.pubkey().to_string();
    cli(
        &url,
        &authority_path,
        &["faucet", "--to", &auth_s, "--usdc", "10000"],
    );
    cli(&url, &authority_path, &["fund-vault", "--usdc", "9000"]);
    cli(
        &url,
        &authority_path,
        &["faucet", "--to", &user_s, "--usdc", "100"],
    );
    let status = cli(&url, &authority_path, &["status"]);
    assert!(status.contains("free 9000.000000"), "{status}");
    assert!(status.contains(&format!("crank {crank_s}")), "{status}");

    // Pascal mock on the sample tape, frozen after both games' start + 2.5 h, so the poller
    // (on Pascal's clock, D-033) treats both legs as due.
    let mut mcfg = MockConfig::new(repo_root().join("tapes/nba-2026-10-28/sample"));
    mcfg.speed = 0.0;
    // Also at least 2 h 40 min past the wall clock, so a tip leg starting in 5 min is due too.
    let tape_due = u64::try_from(ORL.2 + 9_000 + 60).unwrap() * 1000;
    mcfg.start_ms = Some(tape_due.max(desk::now_ms() + 9_600_000));
    mcfg.script = ScriptSource::None;
    let mock = Mock::new(mcfg).unwrap().spawn("127.0.0.1:0").await.unwrap();

    // The desk, chain on, fast crank periods.
    let db = TempDb::new("chain");
    let mut cfg = Config::local(mock.base_url(), db.path());
    cfg.chain_enabled = true;
    cfg.solana_rpc_url = url.clone();
    cfg.crank_keypair_path = crank_path.clone();
    cfg.resolve_poll_every = Duration::from_secs(1);
    cfg.resolved_sweep_every = Duration::from_secs(5);
    cfg.crank_tick = Duration::from_millis(500);
    cfg.chain_resync_every = Duration::from_secs(5);
    cfg.shutdown_grace = Duration::from_secs(5);
    let running = desk::start(cfg).await.expect("desk starts");
    let state = running.state.clone();
    let mut rx = state.subscribe();
    let pdas = Pdas::new(PROGRAM_ID.parse().unwrap());
    let mint = pdas.usdc_mint();
    for _ in 0..100 {
        if state.catalogue().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(state.catalogue().is_some(), "catalogue from the mock");
    let rpc = RpcClient::new(&url);
    let opener = Opener {
        rpc: rpc.clone(),
        pdas,
        user: Keypair::read(&user_path).unwrap(),
        quote_signer,
        db: state.db.clone(),
    };
    let base = desk::now_ms();
    let user_pk = user.pubkey();

    // 1. Pending A (PHI long, ORL short), quote stored; B (PHI) with no fills, named from the
    // catalogue; D (ORL) whose hedge breaches the floor.
    let legs_a = [
        (leg(PHI, 0, 695_000), PHI.0, PHI.1, TeamSide::Away),
        (leg(ORL, 1, 629_000), ORL.0, ORL.1, TeamSide::Home),
    ];
    let pa = opener.open(base, &legs_a, 45, true).await;
    let pb = opener
        .open(
            base + 1,
            &[(leg(PHI, 0, 695_000), PHI.0, PHI.1, TeamSide::Away)],
            31,
            false,
        )
        .await;
    let pd = opener
        .open(
            base + 2,
            &[(leg(ORL, 1, 629_000), ORL.0, ORL.1, TeamSide::Home)],
            31,
            true,
        )
        .await;
    let ta = pdas.ticket(&user_pk, base).to_string();
    let mut opened = 0;
    while opened < 3 {
        let e = wait_for(&mut rx, 30, "PendingOpened", |e| {
            matches!(e, DeskEvent::PendingOpened(_))
        })
        .await;
        let DeskEvent::PendingOpened(p) = e else {
            unreachable!()
        };
        if p.pending == pa.to_string() {
            assert_eq!(p.legs[0].symbol, PHI.0);
            assert_eq!(p.legs[1].symbol, ORL.0);
            assert_eq!(p.legs[1].side, Side::Short);
            assert!(!p.tx.is_empty());
        }
        if p.pending == pb.to_string() {
            assert_eq!(p.legs[0].symbol, PHI.0, "named from the catalogue");
        }
        opened += 1;
    }
    assert_eq!(db_state(&running, &ta).await.as_deref(), Some("pending"));

    // 2. HedgeFilled → finalize → TicketMinted (leg 1 left unhedged: f = 0).
    let before = usdc_balance(&rpc, &user_pk, &mint).await;
    state.emit(DeskEvent::HedgeFilled(Arc::new(HedgeFilled {
        pending: pa.to_string(),
        ticket: ta.clone(),
        user: user_pk.to_string(),
        f_u: vec![700_000, 0],
        r_ppm: 1_000_000,
        fills: vec![],
    })));
    let e = wait_for(
        &mut rx,
        30,
        "TicketMinted",
        |e| matches!(e, DeskEvent::TicketMinted { ticket, .. } if *ticket == ta),
    )
    .await;
    let DeskEvent::TicketMinted { tx, pending, .. } = e else {
        unreachable!()
    };
    assert_eq!(pending, pa.to_string());
    assert!(!tx.is_empty(), "minted with the finalize signature");
    let t = desk::crank::tickets::load_ticket(&state, &ta)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.state, TicketStatus::Open);
    assert_eq!(t.legs[0].p_fill_u, Some(700_000));
    assert!(t.legs[0].hedged && !t.legs[1].hedged);
    assert_eq!(t.tx.finalize.as_deref(), Some(tx.as_str()));
    let payout = t.payout_u;

    // 3. A floor breach is refused by the program → FinalizeRejected.
    state.emit(DeskEvent::HedgeFilled(Arc::new(HedgeFilled {
        pending: pd.to_string(),
        ticket: pdas.ticket(&user_pk, base + 2).to_string(),
        user: user_pk.to_string(),
        f_u: vec![990_000],
        r_ppm: 1_000_000,
        fills: vec![],
    })));
    let e = wait_for(
        &mut rx,
        30,
        "FinalizeRejected",
        |e| matches!(e, DeskEvent::FinalizeRejected { pending, .. } if *pending == pd.to_string()),
    )
    .await;
    let DeskEvent::FinalizeRejected { error, .. } = e else {
        unreachable!()
    };
    assert!(error.contains("BelowFloor"), "{error}");

    // 4. Pascal resolves PHI (away won): resolve_leg within one poll period of the mock clock.
    let t0 = Instant::now();
    mock.mock.with_engine(|e| {
        e.schedule_resolution(PHI.0, 1_000_000, At::Now).unwrap();
    });
    wait_for(&mut rx, 30, "MarketResolved PHI", |e| {
        matches!(e, DeskEvent::MarketResolved { symbol, resolution_u: 1_000_000, .. } if symbol == PHI.0)
    })
    .await;
    let e = wait_for(
        &mut rx,
        60,
        "LegResolved PHI",
        |e| matches!(e, DeskEvent::LegResolved { ticket, leg_idx: 0, .. } if *ticket == ta),
    )
    .await;
    let latency = t0.elapsed();
    let DeskEvent::LegResolved { status, tx, .. } = e else {
        unreachable!()
    };
    assert_eq!(status, LegStatus::Won);
    assert!(!tx.is_empty());
    assert!(latency < Duration::from_secs(60), "{latency:?}");
    eprintln!("resolve_leg landed {latency:?} after the scripted resolution");
    assert_eq!(db_state(&running, &ta).await.as_deref(), Some("open"));

    // ORL resolves at 0: the short leg wins, both legs are in → settle pays P.
    mock.mock.with_engine(|e| {
        e.schedule_resolution(ORL.0, 0, At::Now).unwrap();
    });
    let e = wait_for(
        &mut rx,
        60,
        "TicketSettled",
        |e| matches!(e, DeskEvent::TicketSettled { ticket, .. } if *ticket == ta),
    )
    .await;
    let DeskEvent::TicketSettled { paid_u, tx, .. } = e else {
        unreachable!()
    };
    assert_eq!(paid_u, payout);
    assert!(!tx.is_empty());
    let t = desk::crank::tickets::load_ticket(&state, &ta)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.state, TicketStatus::Settled);
    assert_eq!(t.legs[1].status, LegStatus::Won);
    assert_eq!(t.paid_u, Some(payout));
    assert_eq!(t.tx.settle.as_deref(), Some(tx.as_str()));

    // 5. B (no fills) and D (rejected) are refunded by the crank's timer at expires_at + 15 s.
    let mut refunded = vec![];
    while refunded.len() < 2 {
        let e = wait_for(&mut rx, 90, "PendingRefunded", |e| {
            matches!(e, DeskEvent::PendingRefunded { .. })
        })
        .await;
        let DeskEvent::PendingRefunded { pending, tx, .. } = e else {
            unreachable!()
        };
        assert!(!tx.is_empty());
        refunded.push(pending);
    }
    refunded.sort();
    let mut want = vec![pb.to_string(), pd.to_string()];
    want.sort();
    assert_eq!(refunded, want);
    // Stakes back: 3 × 5 USDC escrowed, A's 5 USDC staked and paid out, B and D refunded.
    let after = usdc_balance(&rpc, &user_pk, &mint).await;
    assert_eq!(after, before + payout + 10_000_000);

    // 5b. An own tip (legs not started yet; 2.F): finalize and settle pass the tipster profile
    // (L2-4). PHI is already resolved, so the poller resolves and settles it at once.
    let mut tip_leg = leg(PHI, 0, 695_000);
    tip_leg.start_ts = now_s() + 300;
    let pe = opener
        .open_as(
            base + 4,
            &[(tip_leg, PHI.0, PHI.1, TeamSide::Away)],
            45,
            true,
            true,
        )
        .await;
    let te = pdas.ticket(&user_pk, base + 4).to_string();
    wait_for(
        &mut rx,
        30,
        "PendingOpened tip",
        |e| matches!(e, DeskEvent::PendingOpened(p) if p.pending == pe.to_string()),
    )
    .await;
    state.emit(DeskEvent::HedgeFilled(Arc::new(HedgeFilled {
        pending: pe.to_string(),
        ticket: te.clone(),
        user: user_pk.to_string(),
        f_u: vec![695_000],
        r_ppm: 1_000_000,
        fills: vec![],
    })));
    wait_for(
        &mut rx,
        60,
        "TicketSettled tip",
        |e| matches!(e, DeskEvent::TicketSettled { ticket, .. } if *ticket == te),
    )
    .await;
    let t = desk::crank::tickets::load_ticket(&state, &te)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(t.tipster, Some(user_pk.to_string()));
    let profile = rpc.get_account(&pdas.tipster(&user_pk)).await.unwrap();
    assert!(profile.is_some(), "finalize created the tipster profile");

    // 6. Restart the validator (same ledger): the listener resyncs and keeps indexing, the
    // crank keeps sending.
    validator.stop();
    tokio::time::sleep(Duration::from_secs(2)).await;
    validator.start().await;
    let pc = opener
        .open(
            base + 3,
            &[(leg(PHI, 0, 695_000), PHI.0, PHI.1, TeamSide::Away)],
            45,
            true,
        )
        .await;
    wait_for(
        &mut rx,
        60,
        "PendingOpened after restart",
        |e| matches!(e, DeskEvent::PendingOpened(p) if p.pending == pc.to_string()),
    )
    .await;
    let tc = pdas.ticket(&user_pk, base + 3).to_string();
    state.emit(DeskEvent::HedgeFilled(Arc::new(HedgeFilled {
        pending: pc.to_string(),
        ticket: tc.clone(),
        user: user_pk.to_string(),
        f_u: vec![695_000],
        r_ppm: 1_000_000,
        fills: vec![],
    })));
    wait_for(
        &mut rx,
        30,
        "TicketMinted after restart",
        |e| matches!(e, DeskEvent::TicketMinted { ticket, .. } if *ticket == tc),
    )
    .await;
    // PHI is already resolved: the poller resolves the new leg and settles at once.
    wait_for(
        &mut rx,
        60,
        "TicketSettled after restart",
        |e| matches!(e, DeskEvent::TicketSettled { ticket, .. } if *ticket == tc),
    )
    .await;

    let ops: Vec<(String, String)> = sqlx::query_as("SELECT op, status FROM crank_ops ORDER BY id")
        .fetch_all(&state.db)
        .await
        .unwrap();
    eprintln!("crank ops: {ops:?}");
    assert!(ops.contains(&("finalize".into(), "rejected".into())));
    running.join().await;
    validator.stop();
}
