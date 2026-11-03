//! `e2e-user`: the scripted user of the localnet lifecycle (plan 3.A, `scripts/e2e_lifecycle.sh`).
//!
//! ```text
//! e2e-user run --desk URL --mock URL --rpc URL --user PATH --hedger OWNER [--program-id ID]
//!              [--date 2026-10-28] [--stake-usdc 20] [--lines 0|1] [--soccer 0|1]
//!              [--phase all|open|close --state FILE]
//! e2e-user timeline --db PATH
//! e2e-user synth-tape --from TAPE_DIR --lines MARKETS_JSON [--soccer MARKETS_JSON] --out DIR
//! ```
//!
//! `run` drives two plain tickets through the desk, the program and the Pascal mock, exactly as
//! the app would: `POST /quote` → `[ed25519 verify, open_pending]` signed by the user → the desk
//! hedges on the mock and the crank finalizes → the mock resolves leg 1 (`/mock/resolve`) → the
//! hedge rolls → `GET /cashout-quote` → `[ed25519 verify, cash_out]` (ticket A); ticket B runs
//! to `settle`. When the card lists spreads and totals (a tape from `synth-tape`, 6.0b), ticket
//! C takes a spread on one game and a total (Under) on another, and runs to `settle` like B;
//! `--lines 1` makes C mandatory. When `/card?sport=soccer` lists a game (7.0b), ticket D takes
//! an NBA winner and that game's draw (a long on its `.DRAW` symbol) and runs to `settle`, the
//! draw resolving at 1 and both team symbols at 0; `--soccer 1` makes D mandatory. With
//! `--phase open` / `--phase close` the run stops after the
//! tickets are
//! minted and resumes from `--state FILE`, so the script can crash and restart the desk in
//! between (runbook "Desk crash"). At every checkpoint it asserts, on chain, `vault.locked == Σ payout` of the open
//! tickets and `vault.free + vault.locked ==` the vault's token balance, and that the desk's
//! `held` (`GET /debug/hedge`) equals the hedger's positions on the mock
//! (`GET /api/v1/accounts/{hedger}`). Any failed check exits non-zero.
//!
//! `timeline` prints each ticket's history from the desk database (read-only): `feed_events`,
//! the hedge fills with their Pascal-style `trade_id`s, and every transaction signature.
//!
//! `synth-tape` copies a tape with the spread and total markets of a `/markets` body added,
//! books included (`desk::catalogue::synthetic`), and with `--soccer` one EPL match moved onto
//! the tape's night.
//!
//! Localnet only: the RPC URL must be on 127.0.0.1/localhost, and the mock too (L3-1).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use base64::Engine;
use desk::api::types::{
    CardGame, CardResponse, CashoutQuoteResponse, LegStatus, MarketKind, Outcome, QuoteLegRequest,
    QuoteRequest, QuoteResponse, Side, Sport, Ticket, TicketStatus,
};
use desk::config::DEFAULT_PROGRAM_ID;
use desk::program::accounts::disc;
use desk::program::ix;
use desk::program::{
    Pdas, TicketAccount, TicketState, VaultAccount, associated_token_address, consts,
};
use desk::solana::rpc::RpcClient;
use desk::solana::signer::{Keypair, SendError, send};
use pascal_rs::{AccountQuery, Client};
use serde_json::{Value, json};
use solana_pubkey::Pubkey;

type Res<T> = Result<T, String>;

const USAGE: &str = "usage: e2e-user run --desk URL --mock URL --rpc URL --user PATH \
--hedger OWNER [--program-id ID] [--date YYYY-MM-DD] [--stake-usdc N] [--lines 0|1] \
[--soccer 0|1] [--phase all|open|close --state FILE] | e2e-user timeline --db PATH \
| e2e-user synth-tape --from DIR --lines FILE [--soccer FILE] --out DIR";

#[tokio::main]
async fn main() -> ExitCode {
    let mut argv = std::env::args().skip(1);
    let Some(cmd) = argv.next() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let flags = match parse_flags(argv) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("e2e-user: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let r = match cmd.as_str() {
        "run" => match Run::new(&flags) {
            Ok(run) => {
                run.run(
                    flags.get("phase").map_or("all", String::as_str),
                    flags.get("state").map(String::as_str),
                )
                .await
            }
            Err(e) => Err(e),
        },
        "timeline" => match flags.get("db") {
            Some(db) => timeline(db).await,
            None => Err("--db is required".into()),
        },
        "synth-tape" => match (flags.get("from"), flags.get("lines"), flags.get("out")) {
            (Some(from), Some(lines), Some(out)) => desk::catalogue::synthetic::write_tape_with(
                from.as_ref(),
                lines.as_ref(),
                flags.get("soccer").map(AsRef::as_ref),
                out.as_ref(),
            )
            .map(|added| {
                let soccer = added.iter().filter(|s| s.starts_with("EPL_")).count();
                say(&format!(
                    "synthetic tape in {out}: {} spread and total markets and {soccer} soccer \
                     outcome markets added",
                    added.len() - soccer
                ))
            }),
            _ => Err("synth-tape needs --from DIR --lines FILE --out DIR [--soccer FILE]".into()),
        },
        _ => Err(USAGE.into()),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("E2E FAIL: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse_flags(argv: impl Iterator<Item = String>) -> Res<HashMap<String, String>> {
    let mut flags = HashMap::new();
    let mut it = argv;
    while let Some(a) = it.next() {
        let name = a
            .strip_prefix("--")
            .ok_or_else(|| format!("unexpected argument {a}"))?;
        let v = it.next().ok_or_else(|| format!("--{name} needs a value"))?;
        flags.insert(name.to_owned(), v);
    }
    Ok(flags)
}

fn usdc(u: u64) -> String {
    format!("{}.{:06}", u / 1_000_000, u % 1_000_000)
}

fn short(s: &str) -> &str {
    s.get(..8).unwrap_or(s)
}

fn is_local(url: &str) -> bool {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or("");
    rest.starts_with("127.0.0.1:") || rest.starts_with("localhost:")
}

/// One ticket the user holds.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Held {
    name: String,
    #[serde(with = "pubkey_str")]
    ticket: Pubkey,
    payout_u: u64,
    symbols: Vec<String>,
}

/// What `run --phase open` hands to `run --phase close` across a desk restart.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Saved {
    a: Held,
    b: Held,
    /// Ticket C (6.0b): a spread leg and a total leg, when the card lists them.
    #[serde(default)]
    c: Option<Held>,
    /// Ticket D (7.0b): an NBA winner leg and a soccer draw, when the card lists a soccer game.
    #[serde(default)]
    d: Option<Held>,
    bal0: u64,
    checkpoints: u32,
    nonce: u64,
    started_ms: u64,
}

mod pubkey_str {
    use serde::{Deserialize, Deserializer, Serializer};
    use solana_pubkey::Pubkey;

    pub fn serialize<S: Serializer>(k: &Pubkey, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&k.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Pubkey, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

struct Run {
    http: reqwest::Client,
    desk: String,
    mock: String,
    rpc: RpcClient,
    pascal: Client,
    pdas: Pdas,
    user: Keypair,
    hedger: String,
    date: String,
    stake_u: u64,
    /// Ticket C must run (the card must list spreads and totals).
    lines: bool,
    /// Ticket D must run (the soccer card must list a game).
    soccer: bool,
    checkpoints: u32,
    nonce: u64,
}

impl Run {
    fn new(f: &HashMap<String, String>) -> Res<Self> {
        let need = |k: &str| {
            f.get(k)
                .cloned()
                .ok_or_else(|| format!("--{k} is required"))
        };
        let desk = need("desk")?.trim_end_matches('/').to_owned();
        let mock = need("mock")?.trim_end_matches('/').to_owned();
        let rpc_url = need("rpc")?;
        for (name, u) in [("--desk", &desk), ("--mock", &mock), ("--rpc", &rpc_url)] {
            if !is_local(u) {
                return Err(format!("{name} {u}: localnet only (127.0.0.1 / localhost)"));
            }
        }
        let user = Keypair::read(&PathBuf::from(need("user")?))?;
        let program: Pubkey = f
            .get("program-id")
            .map_or(DEFAULT_PROGRAM_ID, String::as_str)
            .parse()
            .map_err(|e| format!("--program-id: {e}"))?;
        let stake_usdc: u64 = f
            .get("stake-usdc")
            .map_or(Ok(20), |v| v.parse())
            .map_err(|e| format!("--stake-usdc: {e}"))?;
        let pascal = Client::builder()
            .base_url(&mock)
            .build()
            .map_err(|e| format!("mock client: {e}"))?;
        Ok(Run {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .map_err(|e| e.to_string())?,
            desk,
            mock,
            rpc: RpcClient::new(rpc_url),
            pascal,
            pdas: Pdas::new(program),
            user,
            hedger: need("hedger")?,
            date: f
                .get("date")
                .cloned()
                .unwrap_or_else(|| "2026-10-28".into()),
            stake_u: stake_usdc
                .checked_mul(1_000_000)
                .ok_or("--stake-usdc: too large")?,
            lines: match f.get("lines").map(String::as_str) {
                None | Some("0") => false,
                Some("1") => true,
                Some(v) => return Err(format!("--lines {v}: expected 0 or 1")),
            },
            soccer: match f.get("soccer").map(String::as_str) {
                None | Some("0") => false,
                Some("1") => true,
                Some(v) => return Err(format!("--soccer {v}: expected 0 or 1")),
            },
            checkpoints: 0,
            nonce: desk::now_ms() * 1_000,
        })
    }

    // --- HTTP ---------------------------------------------------------------------------------

    async fn get_json(&self, url: &str) -> Res<(u16, Value)> {
        let r = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| format!("GET {url}: {e}"))?;
        let status = r.status().as_u16();
        let v = r
            .json::<Value>()
            .await
            .map_err(|e| format!("GET {url}: body: {e}"))?;
        Ok((status, v))
    }

    async fn post_json(&self, url: &str, body: &Value) -> Res<(u16, Value)> {
        let r = self
            .http
            .post(url)
            .json(body)
            .send()
            .await
            .map_err(|e| format!("POST {url}: {e}"))?;
        let status = r.status().as_u16();
        let v = r
            .json::<Value>()
            .await
            .map_err(|e| format!("POST {url}: body: {e}"))?;
        Ok((status, v))
    }

    async fn ticket(&self, t: &Pubkey) -> Res<Option<Ticket>> {
        let (s, v) = self.get_json(&format!("{}/ticket/{t}", self.desk)).await?;
        match s {
            200 => serde_json::from_value(v)
                .map(Some)
                .map_err(|e| format!("ticket {t}: {e}")),
            404 => Ok(None),
            _ => Err(format!("GET /ticket/{t}: {s} {v}")),
        }
    }

    /// Polls `/ticket/{t}` until `pred` holds (or `secs` pass).
    async fn wait_ticket(
        &self,
        t: &Pubkey,
        secs: u64,
        what: &str,
        pred: impl Fn(&Ticket) -> bool,
    ) -> Res<Ticket> {
        let t0 = Instant::now();
        let mut last = None;
        while t0.elapsed() < Duration::from_secs(secs) {
            if let Some(tk) = self.ticket(t).await? {
                if pred(&tk) {
                    say(&format!(
                        "{what}: ticket {} after {:.1} s",
                        short(&t.to_string()),
                        t0.elapsed().as_secs_f64()
                    ));
                    return Ok(tk);
                }
                last = Some(tk);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Err(format!(
            "{what}: not reached within {secs} s; last view {}",
            last.map_or("none".into(), |t| serde_json::to_string(&t)
                .unwrap_or_default())
        ))
    }

    // --- chain --------------------------------------------------------------------------------

    async fn usdc_of(&self, owner: &Pubkey) -> Res<u64> {
        let ata = associated_token_address(owner, &self.pdas.usdc_mint());
        token_amount(&self.rpc, &ata).await
    }

    /// Vault, its token balance and the open tickets, read consistently: the vault account and
    /// its token account in one `getMultipleAccounts`, the tickets in between two vault reads
    /// that must agree (else the crank moved something; read again).
    async fn chain_book(&self) -> Res<(VaultAccount, u64, Vec<(Pubkey, TicketAccount)>)> {
        let program = self.pdas.program;
        for _ in 0..10 {
            let (v1, bal1) = self.vault().await?;
            let (_, accs) = self
                .rpc
                .get_program_accounts(&program, &[(0, disc::TICKET.to_vec())])
                .await
                .map_err(|e| format!("getProgramAccounts: {e}"))?;
            let (v2, bal2) = self.vault().await?;
            if v1 != v2 || bal1 != bal2 {
                tokio::time::sleep(Duration::from_millis(300)).await;
                continue;
            }
            let mut open = Vec::new();
            for (k, a) in accs {
                let t = TicketAccount::decode(&a.data, &a.owner, &program)
                    .map_err(|e| format!("ticket {k}: {e:?}"))?;
                if t.state == TicketState::Open {
                    open.push((k, t));
                }
            }
            return Ok((v2, bal2, open));
        }
        Err("vault kept changing while reading the tickets".into())
    }

    async fn vault(&self) -> Res<(VaultAccount, u64)> {
        let keys = [self.pdas.vault(), self.pdas.vault_usdc()];
        let (_, accs) = self
            .rpc
            .get_multiple_accounts(&keys)
            .await
            .map_err(|e| format!("vault: {e}"))?;
        let v = accs[0].as_ref().ok_or("no vault account")?;
        let vault = VaultAccount::decode(&v.data, &v.owner, &self.pdas.program)
            .map_err(|e| format!("vault: {e:?}"))?;
        let t = accs[1].as_ref().ok_or("no vault token account")?;
        Ok((vault, spl_amount(&t.data)?))
    }

    async fn ticket_account(&self, t: &Pubkey) -> Res<TicketAccount> {
        let a = self
            .rpc
            .get_account(t)
            .await
            .map_err(|e| format!("ticket {t}: {e}"))?
            .ok_or_else(|| format!("ticket {t}: no account"))?;
        TicketAccount::decode(&a.data, &a.owner, &self.pdas.program)
            .map_err(|e| format!("ticket {t}: {e:?}"))
    }

    // --- checkpoints --------------------------------------------------------------------------

    /// `vault.locked == Σ payout` of the open tickets, `free + locked ==` the vault balance,
    /// and the desk's `held` == the hedger's positions on the mock.
    async fn checkpoint(&mut self, label: &str) -> Res<VaultAccount> {
        self.checkpoints += 1;
        let (v, bal, open) = self.chain_book().await?;
        let sum: u64 = open.iter().map(|(_, t)| t.payout_u).sum();
        say(&format!(
            "CHECK [{label}] vault locked {} free {} balance {}; {} open tickets, Σ payout {}",
            usdc(v.locked),
            usdc(v.free),
            usdc(bal),
            open.len(),
            usdc(sum)
        ));
        if v.locked != sum {
            return Err(format!(
                "[{label}] vault.locked {} != Σ payout of open tickets {}",
                v.locked, sum
            ));
        }
        if v.free + v.locked != bal {
            return Err(format!(
                "[{label}] vault.free + vault.locked {} != vault ATA balance {bal}",
                v.free + v.locked
            ));
        }
        self.check_held(label).await?;
        Ok(v)
    }

    /// The desk's `held` against the mock's positions, retried while orders may be in flight.
    async fn check_held(&self, label: &str) -> Res<()> {
        let t0 = Instant::now();
        loop {
            let (desk, mock) = self.positions().await?;
            let symbols: BTreeSet<&String> = desk.keys().chain(mock.keys()).collect();
            let diff: Vec<String> = symbols
                .iter()
                .filter_map(|s| {
                    let (d, m) = (
                        desk.get(*s).copied().unwrap_or(0),
                        mock.get(*s).copied().unwrap_or(0),
                    );
                    (d != m).then(|| format!("{s}: desk {d} mock {m}"))
                })
                .collect();
            if diff.is_empty() {
                let shown: Vec<String> = mock
                    .iter()
                    .filter(|(_, n)| **n != 0)
                    .map(|(s, n)| format!("{s} {n:+}"))
                    .collect();
                say(&format!(
                    "CHECK [{label}] held == mock positions ({})",
                    if shown.is_empty() {
                        "all flat".into()
                    } else {
                        shown.join(", ")
                    }
                ));
                return Ok(());
            }
            if t0.elapsed() > Duration::from_secs(20) {
                return Err(format!(
                    "[{label}] desk held != mock positions: {}",
                    diff.join("; ")
                ));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// (desk held, mock positions) per symbol, signed contracts.
    async fn positions(&self) -> Res<(BTreeMap<String, i64>, BTreeMap<String, i64>)> {
        let (s, v) = self.get_json(&format!("{}/debug/hedge", self.desk)).await?;
        if s != 200 {
            return Err(format!("GET /debug/hedge: {s} {v}"));
        }
        let desk: BTreeMap<String, i64> =
            serde_json::from_value(v["held"].clone()).map_err(|e| format!("/debug/hedge: {e}"))?;
        let acct = self
            .pascal
            .account(
                &self.hedger,
                &AccountQuery {
                    symbol: None,
                    exclude_orders: true,
                },
            )
            .await
            .map_err(|e| format!("mock /accounts/{}: {e}", self.hedger))?;
        let mock = acct
            .data
            .positions
            .iter()
            .map(|p| (p.symbol.clone(), p.size))
            .collect();
        Ok((desk, mock))
    }

    /// The desk's (target, held, resting) per symbol (`/debug/hedge`).
    async fn targets(&self) -> Res<BTreeMap<String, (i64, i64, i64)>> {
        let (s, v) = self.get_json(&format!("{}/debug/hedge", self.desk)).await?;
        if s != 200 {
            return Err(format!("GET /debug/hedge: {s} {v}"));
        }
        let map = |k: &str| -> Res<BTreeMap<String, i64>> {
            serde_json::from_value(v[k].clone()).map_err(|e| format!("/debug/hedge {k}: {e}"))
        };
        let (held, target, resting) = (map("held")?, map("target")?, map("resting")?);
        Ok(target
            .into_iter()
            .map(|(s, t)| {
                let h = held.get(&s).copied().unwrap_or(0);
                let r = resting.get(&s).copied().unwrap_or(0);
                (s, (t, h, r))
            })
            .collect())
    }

    /// Waits until the desk holds its target on every one of `symbols` with nothing resting,
    /// up to `secs`: exactly (`strict`: a roll or an unwind trades to `Tᵢ`), or within the
    /// desk's own rebalance tolerance (10% and 2 contracts: inception buys each ticket's `Δᵢ`
    /// rounded up, `Tᵢ` rounds the net, and the sweep leaves that difference alone).
    async fn wait_on_target(
        &self,
        symbols: &[String],
        secs: u64,
        what: &str,
        strict: bool,
    ) -> Res<()> {
        let p = pricing::Params::default();
        let t0 = Instant::now();
        loop {
            let tg = self.targets().await?;
            let off: Vec<String> = symbols
                .iter()
                .filter_map(|s| {
                    let (t, h, r) = tg.get(s).copied().unwrap_or((0, 0, 0));
                    let off = if strict {
                        t != h
                    } else {
                        pricing::needs_rebalance(
                            h,
                            t,
                            p.rebalance_rel_ppm,
                            p.rebalance_min_contracts,
                        )
                    };
                    (off || r != 0).then(|| format!("{s}: target {t} held {h} resting {r}"))
                })
                .collect();
            if off.is_empty() {
                let on: Vec<String> = symbols
                    .iter()
                    .map(|s| {
                        let (t, h, _) = tg.get(s).copied().unwrap_or((0, 0, 0));
                        format!("{s} held {h:+} target {t:+}")
                    })
                    .collect();
                say(&format!(
                    "{what}: on target after {:.1} s ({})",
                    t0.elapsed().as_secs_f64(),
                    on.join(", ")
                ));
                return Ok(());
            }
            if t0.elapsed() > Duration::from_secs(secs) {
                return Err(format!("{what}: off target: {}", off.join("; ")));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    // --- the user's actions -------------------------------------------------------------------

    /// `POST /quote`, then `[ed25519 verify, open_pending]`; retried with a fresh quote when the
    /// transaction fails (a quote lives 10 s).
    async fn open(&mut self, name: &str, legs: &[QuoteLegRequest]) -> Res<Held> {
        let mut last = String::new();
        for attempt in 1..=3 {
            let req = QuoteRequest {
                legs: legs.to_vec(),
                stake_u: self.stake_u,
                token_in: None,
                copied_from: None,
                title: None,
                wallet: None,
            };
            let (s, v) = self
                .post_json(
                    &format!("{}/quote", self.desk),
                    &serde_json::to_value(&req).map_err(|e| e.to_string())?,
                )
                .await?;
            if s != 200 {
                return Err(format!("POST /quote ({name}): {s} {v}"));
            }
            let q: QuoteResponse =
                serde_json::from_value(v).map_err(|e| format!("quote ({name}): {e}"))?;
            let b64 = base64::engine::general_purpose::STANDARD;
            let msg = b64.decode(&q.quote_msg).map_err(|e| e.to_string())?;
            let sig: [u8; 64] = b64
                .decode(&q.quote_sig)
                .map_err(|e| e.to_string())?
                .try_into()
                .map_err(|_| "quote_sig is not 64 bytes")?;
            let signer: Pubkey = q.quote_signer.parse().map_err(|e| format!("{e}"))?;
            if msg.get(17..49) != Some(self.pdas.program.as_ref()) {
                return Err("quote is for another program id".into());
            }
            say(&format!(
                "{name}: quoted {} legs, stake {} USDC, odds {}.{:06} (fair {}.{:06}), payout {} USDC, floor {}",
                q.n,
                usdc(q.stake_u),
                q.odds_offered_u / 1_000_000,
                q.odds_offered_u % 1_000_000,
                q.odds_fair_u / 1_000_000,
                q.odds_fair_u % 1_000_000,
                usdc(q.payout_u),
                q.min_payout_per_unit_u
            ));
            for l in &q.legs {
                say(&format!(
                    "  leg {} {:?} {:?}: p {} (max {}), m1 {} ppm, overlaps {}",
                    l.symbol, l.side, l.pick, l.price_u, l.p_max_u, l.m1_ppm, l.overlaps
                ));
            }
            self.nonce += 1;
            let nonce = self.nonce;
            // The program judges the Pending's expiry by the cluster clock (L2 merge notes).
            let now = self
                .rpc
                .cluster_time()
                .await
                .map_err(|e| format!("cluster time: {e}"))?;
            let user = self.user.pubkey();
            let ixs = [
                ix::ed25519_verify(&msg, &sig, &signer),
                ix::open_pending(
                    &self.pdas,
                    &user,
                    &user,
                    nonce,
                    q.min_payout_per_unit_u,
                    now + 45,
                    false,
                    None,
                ),
            ];
            match send(&self.rpc, &ixs, &[&self.user]).await {
                Ok(sig) => {
                    let ticket = self.pdas.ticket(&user, nonce);
                    say(&format!(
                        "{name}: open_pending {sig} (nonce {nonce}, ticket {ticket})"
                    ));
                    return Ok(Held {
                        name: name.to_owned(),
                        ticket,
                        payout_u: q.payout_u,
                        symbols: legs.iter().map(|l| l.symbol.clone()).collect(),
                    });
                }
                Err(e @ SendError::Failed { .. }) => {
                    last = e.to_string();
                    say(&format!(
                        "{name}: open_pending attempt {attempt} failed: {last}"
                    ));
                }
                Err(e) => return Err(format!("open_pending ({name}): {e}")),
            }
        }
        Err(format!("open_pending ({name}): {last}"))
    }

    /// Waits for the desk to hedge and the crank to finalize: the ticket is `open`.
    async fn wait_minted(&self, h: &mut Held) -> Res<Ticket> {
        let t = self
            .wait_ticket(&h.ticket, 90, &format!("{} minted", h.name), |t| {
                t.state != TicketStatus::Pending
            })
            .await?;
        if t.state != TicketStatus::Open {
            return Err(format!(
                "{}: expected open after finalize, got {:?} (refunded: the hedge missed the floor?)",
                h.name, t.state
            ));
        }
        let chain = self.ticket_account(&h.ticket).await?;
        if chain.payout_u != t.payout_u {
            return Err(format!(
                "{}: desk payout {} != chain payout {}",
                h.name, t.payout_u, chain.payout_u
            ));
        }
        h.payout_u = chain.payout_u;
        say(&format!(
            "{}: minted, r {} ppm, rS {} USDC, P {} USDC, {} hedge fills, finalize {}",
            h.name,
            chain.r_ppm,
            usdc(chain.stake_u),
            usdc(chain.payout_u),
            t.fills.len(),
            t.tx.finalize.as_deref().unwrap_or("?")
        ));
        for f in &t.fills {
            say(&format!(
                "  fill trade_id {} {} {:?} {} @ {} fee {} builder {} ({:?})",
                f.trade_id, f.symbol, f.side, f.size, f.price_u, f.fee_u, f.builder_fee_u, f.venue
            ));
        }
        if t.fills.is_empty() {
            return Err(format!("{}: minted without a single hedge fill", h.name));
        }
        Ok(t)
    }

    /// Resolves `symbol` on the mock (`price` = "1": the long side won).
    async fn resolve(&self, symbol: &str, price: &str) -> Res<()> {
        let (s, v) = self
            .post_json(
                &format!("{}/mock/resolve", self.mock),
                &json!({"symbol": symbol, "price": price}),
            )
            .await?;
        if s != 200 {
            return Err(format!("/mock/resolve {symbol}: {s} {v}"));
        }
        say(&format!("mock: {symbol} resolved at {price}"));
        Ok(())
    }

    /// `GET /cashout-quote`, then `[ed25519 verify, cash_out]`; retried on a stale quote or an
    /// unavailable one.
    async fn cash_out(&self, h: &Held) -> Res<(u64, String)> {
        let mut last = String::new();
        for attempt in 1..=5 {
            let (s, v) = self
                .get_json(&format!("{}/cashout-quote/{}", self.desk, h.ticket))
                .await?;
            if s != 200 {
                last = format!("{s} {v}");
                say(&format!(
                    "{}: cash-out quote attempt {attempt}: {last}",
                    h.name
                ));
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
            let q: CashoutQuoteResponse = serde_json::from_value(v).map_err(|e| e.to_string())?;
            let b64 = base64::engine::general_purpose::STANDARD;
            let msg = b64.decode(&q.quote_msg).map_err(|e| e.to_string())?;
            let sig: [u8; 64] = b64
                .decode(&q.quote_sig)
                .map_err(|e| e.to_string())?
                .try_into()
                .map_err(|_| "quote_sig is not 64 bytes")?;
            if msg.len() != 99
                || msg[..19] != consts::CASHOUT_TAG
                || msg[51..83] != h.ticket.to_bytes()
                || u64::from_le_bytes(msg[83..91].try_into().unwrap_or_default()) != q.c_u
            {
                return Err(format!(
                    "{}: cash-out message does not match the quote",
                    h.name
                ));
            }
            let signer: Pubkey = q.quote_signer.parse().map_err(|e| format!("{e}"))?;
            let chain = self.ticket_account(&h.ticket).await?;
            say(&format!(
                "{}: cash-out quote C {} USDC (P {} USDC, ts {})",
                h.name,
                usdc(q.c_u),
                usdc(chain.payout_u),
                q.ts
            ));
            let ixs = [
                ix::ed25519_verify(&msg, &sig, &signer),
                ix::cash_out(&self.pdas, &h.ticket, &chain, &msg),
            ];
            match send(&self.rpc, &ixs, &[&self.user]).await {
                Ok(sig) => {
                    say(&format!("{}: cash_out {sig}", h.name));
                    return Ok((q.c_u, sig));
                }
                Err(e @ SendError::Failed { .. }) => {
                    last = e.to_string();
                    say(&format!(
                        "{}: cash_out attempt {attempt} failed: {last}",
                        h.name
                    ));
                }
                Err(e) => return Err(format!("cash_out ({}): {e}", h.name)),
            }
        }
        Err(format!("cash_out ({}): {last}", h.name))
    }

    // --- the scenario -------------------------------------------------------------------------

    /// `--phase all` (default), or `open` then `close` with `--state FILE` between them (the
    /// script restarts the desk there).
    async fn run(mut self, phase: &str, state: Option<&str>) -> Res<()> {
        match phase {
            "all" => {
                let saved = self.phase_open().await?;
                self.phase_close(saved).await
            }
            "open" => {
                let path = state.ok_or("--phase open needs --state FILE")?;
                let saved = self.phase_open().await?;
                let text = serde_json::to_string_pretty(&saved).map_err(|e| e.to_string())?;
                std::fs::write(path, text).map_err(|e| format!("{path}: {e}"))?;
                say(&format!("phase open done; state in {path}"));
                Ok(())
            }
            "close" => {
                let path = state.ok_or("--phase close needs --state FILE")?;
                let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
                let saved: Saved = serde_json::from_str(&text).map_err(|e| e.to_string())?;
                self.checkpoints = saved.checkpoints;
                self.nonce = self.nonce.max(saved.nonce);
                // The desk was restarted: it must have rebuilt the tickets and held from the
                // DB and the mock (runbook "Desk crash").
                for h in [
                    Some(&saved.a),
                    Some(&saved.b),
                    saved.c.as_ref(),
                    saved.d.as_ref(),
                ]
                .into_iter()
                .flatten()
                {
                    self.wait_ticket(&h.ticket, 60, &format!("{} after restart", h.name), |t| {
                        t.state == TicketStatus::Open
                    })
                    .await?;
                }
                let mut all = saved.a.symbols.clone();
                all.extend(saved.b.symbols.iter().cloned());
                all.extend(saved.c.iter().flat_map(|c| c.symbols.iter().cloned()));
                all.extend(saved.d.iter().flat_map(|d| d.symbols.iter().cloned()));
                self.wait_on_target(&all, 30, "after restart", false)
                    .await?;
                self.checkpoint("desk restarted").await?;
                self.phase_close(saved).await
            }
            other => Err(format!("--phase {other}: expected all, open or close")),
        }
    }

    /// Tickets A and B quoted, opened, hedged and minted.
    async fn phase_open(&mut self) -> Res<Saved> {
        let started_ms = desk::now_ms();
        let user = self.user.pubkey();
        let card = self.card().await?;
        let mut games = card.games.clone();
        games.sort_by_key(|g| (g.start_ms, g.symbol.clone()));
        if games.len() < 5 {
            return Err(format!("the card has {} games, need 5", games.len()));
        }
        let leg = |i: usize, side: Side| QuoteLegRequest {
            symbol: games[i].symbol.clone(),
            side,
        };
        say(&format!(
            "card {}: {}",
            card.date,
            games
                .iter()
                .map(|g| g.symbol.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        let bal0 = self.usdc_of(&user).await?;
        self.checkpoint("start").await?;

        // Ticket A: the night's first three games (all overlapping): long leg 1 (resolved
        // first, at 1: won), short leg 2, long leg 3.
        let legs_a = [leg(0, Side::Long), leg(1, Side::Short), leg(2, Side::Long)];
        let mut a = self.open("A", &legs_a).await?;
        self.wait_minted(&mut a).await?;
        self.checkpoint("A minted").await?;

        // Ticket B: games 4 and 5, run to settle.
        let legs_b = [leg(3, Side::Long), leg(4, Side::Short)];
        let mut b = self.open("B", &legs_b).await?;
        self.wait_minted(&mut b).await?;
        self.checkpoint("B minted").await?;

        // Ticket C (6.0b): a spread leg and a total leg on two games, run to settle.
        let c = match Self::lines_legs(&card) {
            Some(legs_c) => {
                let mut c = self.open("C", &legs_c).await?;
                let t = self.wait_minted(&mut c).await?;
                let symbols: BTreeSet<&str> = t.fills.iter().map(|f| f.symbol.as_str()).collect();
                if legs_c.iter().any(|l| !symbols.contains(l.symbol.as_str())) {
                    return Err(format!(
                        "C: both overlapping legs must be hedged at inception, fills on {symbols:?}"
                    ));
                }
                self.checkpoint("C minted").await?;
                Some(c)
            }
            None if self.lines => {
                return Err("--lines 1: the card lists no spread and total on two games".into());
            }
            None => {
                say("C: skipped (the card lists no spreads and totals)");
                None
            }
        };

        // Ticket D (7.0b): game 3's winner (long; never resolved by A, B or C) and the soccer
        // game's draw (a long on its own symbol), overlapping, run to settle.
        let soccer = self.soccer_card().await?;
        let d = match Self::soccer_draw(&soccer) {
            Some(draw) => {
                let legs_d = [leg(2, Side::Long), draw];
                let mut d = self.open("D", &legs_d).await?;
                let t = self.wait_minted(&mut d).await?;
                let symbols: BTreeSet<&str> = t.fills.iter().map(|f| f.symbol.as_str()).collect();
                if legs_d.iter().any(|l| !symbols.contains(l.symbol.as_str())) {
                    return Err(format!(
                        "D: both overlapping legs must be hedged at inception, fills on {symbols:?}"
                    ));
                }
                if t.legs[1].label.as_deref() != Some("Draw") || t.legs[1].side != Side::Long {
                    return Err(format!(
                        "D: leg 2 should be a long labelled Draw, got {:?} {:?}",
                        t.legs[1].side, t.legs[1].label
                    ));
                }
                self.checkpoint("D minted").await?;
                Some(d)
            }
            None if self.soccer => {
                return Err("--soccer 1: the soccer card lists no game".into());
            }
            None => {
                say("D: skipped (the card lists no soccer game)");
                None
            }
        };
        Ok(Saved {
            a,
            b,
            c,
            d,
            bal0,
            checkpoints: self.checkpoints,
            nonce: self.nonce,
            started_ms,
        })
    }

    /// A: leg 1 resolves → roll → cash-out. B: both legs resolve → settle.
    async fn phase_close(&mut self, saved: Saved) -> Res<()> {
        let Saved {
            a,
            b,
            c,
            d,
            bal0,
            started_ms,
            ..
        } = saved;
        let user = self.user.pubkey();

        // Leg 1 of A resolves (the demo's PHI at 1) → resolve_leg → roll.
        self.resolve(&a.symbols[0], "1").await?;
        self.wait_ticket(&a.ticket, 60, "A leg 1 won", |t| {
            t.legs[0].status == LegStatus::Won
        })
        .await?;
        let rest_a = a.symbols[1..].to_vec();
        self.wait_on_target(&rest_a, 45, "A roll", true).await?;
        let before = self.checkpoint("A leg 1 resolved, rolled").await?;

        // Cash out A: the vault unlocks P − C.
        let user_before = self.usdc_of(&user).await?;
        let (c_u, _) = self.cash_out(&a).await?;
        let after = self.vault().await?.0;
        if after.locked != before.locked - a.payout_u
            || after.free != before.free + (a.payout_u - c_u)
        {
            return Err(format!(
                "A cash-out: vault locked {} → {} and free {} → {}; expected −P = −{} and +(P − C) = +{}",
                before.locked,
                after.locked,
                before.free,
                after.free,
                a.payout_u,
                a.payout_u - c_u
            ));
        }
        let user_after = self.usdc_of(&user).await?;
        if user_after != user_before + c_u {
            return Err(format!(
                "A cash-out: user USDC {user_before} → {user_after}, expected + {c_u}"
            ));
        }
        say(&format!(
            "A: vault unlocked P − C = {} USDC (locked −{}, free +{})",
            usdc(a.payout_u - c_u),
            usdc(a.payout_u),
            usdc(a.payout_u - c_u)
        ));
        self.wait_ticket(&a.ticket, 30, "A cashed out (desk)", |t| {
            t.state == TicketStatus::CashedOut && t.paid_u == Some(c_u)
        })
        .await?;
        self.wait_on_target(&rest_a, 30, "A unwind", true).await?;
        self.checkpoint("A cashed out, unwound").await?;

        // B: leg 1 won (long at 1) → roll, leg 2 won (short at 0) → settle pays P.
        let paid = self.settle_two(&b, &[]).await?;
        // C: the spread leg won (long at 1) → roll, the total's Under won (short at 0) → settle.
        let paid_c = match &c {
            Some(c) => Some(self.settle_two(c, &[]).await?),
            None => None,
        };
        // D: the NBA leg won → roll; then the match ends level, as Pascal resolves a 1-X-2: the
        // draw at 1 and both team symbols at 0 → the draw leg won → settle pays P.
        let paid_d = match &d {
            Some(d) => {
                let draw = d.symbols.get(1).cloned().unwrap_or_default();
                let event = draw.rsplit_once('.').map_or("", |(e, _)| e).to_owned();
                let teams: Vec<String> = self
                    .soccer_card()
                    .await?
                    .games
                    .iter()
                    .flat_map(|g| g.markets.iter().flat_map(|m| m.picks.iter()))
                    .map(|p| p.symbol.clone())
                    .filter(|s| *s != draw && s.starts_with(&format!("{event}.")))
                    .collect();
                if teams.len() != 2 {
                    return Err(format!(
                        "D: expected two team symbols beside {draw}, got {teams:?}"
                    ));
                }
                Some(self.settle_two(d, &teams).await?)
            }
            None => None,
        };
        let end = self.checkpoint("end").await?;
        if end.locked != 0 {
            return Err(format!("end: vault still locks {}", end.locked));
        }
        let bal1 = self.usdc_of(&user).await?;
        println!(
            "E2E PASS: {} checkpoints in {} s; A cashed out C {} of P {}; B settled {}; {}{}\
             user USDC {} → {}; vault locked {} free {}",
            self.checkpoints,
            desk::now_ms().saturating_sub(started_ms) / 1000,
            usdc(c_u),
            usdc(a.payout_u),
            usdc(paid),
            paid_c.map_or_else(String::new, |p| format!(
                "C (spread + total) settled {}; ",
                usdc(p)
            )),
            paid_d.map_or_else(String::new, |p| format!(
                "D (NBA + EPL draw) settled {}; ",
                usdc(p)
            )),
            usdc(bal0),
            usdc(bal1),
            usdc(end.locked),
            usdc(end.free)
        );
        Ok(())
    }

    /// A two-leg ticket to `settle`, both legs won: leg 1 resolves for its side (1 for a long
    /// leg, 0 for a short one) → roll; then the `losers` (symbols no leg holds, e.g. a soccer
    /// match's other outcomes) resolve at 0 and leg 2 resolves → settle pays P. Checks the vault
    /// and the user's balance.
    async fn settle_two(&mut self, h: &Held, losers: &[String]) -> Res<u64> {
        let user = self.user.pubkey();
        let name = h.name.clone();
        let t = self
            .ticket(&h.ticket)
            .await?
            .ok_or_else(|| format!("{name}: the desk does not know ticket {}", h.ticket))?;
        let wins: Vec<&str> = t
            .legs
            .iter()
            .map(|l| if l.side == Side::Long { "1" } else { "0" })
            .collect();
        if wins.len() != 2 || t.legs.iter().map(|l| &l.symbol).ne(h.symbols.iter()) {
            return Err(format!("{name}: expected the two legs {:?}", h.symbols));
        }
        self.resolve(&h.symbols[0], wins[0]).await?;
        self.wait_ticket(&h.ticket, 60, &format!("{name} leg 1 won"), |t| {
            t.legs[0].status == LegStatus::Won
        })
        .await?;
        self.wait_on_target(&h.symbols[1..], 45, &format!("{name} roll"), true)
            .await?;
        self.checkpoint(&format!("{name} leg 1 resolved, rolled"))
            .await?;
        let before = self.vault().await?.0;
        let user_before = self.usdc_of(&user).await?;
        for s in losers {
            self.resolve(s, "0").await?;
        }
        self.resolve(&h.symbols[1], wins[1]).await?;
        let t = self
            .wait_ticket(&h.ticket, 60, &format!("{name} settled"), |t| {
                t.state == TicketStatus::Settled
            })
            .await?;
        let paid = t.paid_u.unwrap_or(0);
        if paid != h.payout_u {
            return Err(format!(
                "{name} settled for {paid}, expected the full payout {}",
                h.payout_u
            ));
        }
        let after = self.vault().await?.0;
        if after.locked != before.locked - h.payout_u || after.free != before.free {
            return Err(format!(
                "{name} settle: vault locked {} → {}, free {} → {}; expected −P = −{} and free unchanged",
                before.locked, after.locked, before.free, after.free, h.payout_u
            ));
        }
        let user_after = self.usdc_of(&user).await?;
        if user_after != user_before + paid {
            return Err(format!(
                "{name} settle: user USDC {user_before} → {user_after}, expected + {paid}"
            ));
        }
        say(&format!(
            "{name}: settled, paid {} USDC, settle {}",
            usdc(paid),
            t.tx.settle.as_deref().unwrap_or("?")
        ));
        Ok(paid)
    }

    /// Ticket C's legs (6.0b): the card's first game with a spread line takes its first spread
    /// (away pick), and another game with a total line takes its first total's Under. `None`
    /// when the card lists no such pair.
    fn lines_legs(card: &CardResponse) -> Option<Vec<QuoteLegRequest>> {
        let first = |g: &CardGame, kind: MarketKind| {
            g.markets
                .iter()
                .find(|m| m.kind == kind && m.picks.len() == 2)
                .cloned()
        };
        let mut games: Vec<&CardGame> = card.games.iter().filter(|g| !g.in_play).collect();
        games.sort_by_key(|g| (g.start_ms, g.game_id.clone()));
        let (gs, spread) = games
            .iter()
            .find_map(|g| first(g, MarketKind::Spread).map(|m| (g.game_id.clone(), m)))?;
        let total = games
            .iter()
            .filter(|g| g.game_id != gs)
            .find_map(|g| first(g, MarketKind::Total))?;
        let away = &spread.picks[0];
        let under = total.picks.iter().find(|p| p.side == Side::Short)?;
        say(&format!(
            "C: {} ({:?} {}) and {} ({:?} {})",
            away.label, away.side, away.symbol, under.label, under.side, under.symbol
        ));
        Some(vec![
            QuoteLegRequest {
                symbol: away.symbol.clone(),
                side: away.side,
            },
            QuoteLegRequest {
                symbol: under.symbol.clone(),
                side: under.side,
            },
        ])
    }

    /// Ticket D's soccer leg (7.0b): the draw ("X") of the soccer card's first game, a long on
    /// its own symbol. `None` when the card lists no soccer game.
    fn soccer_draw(card: &CardResponse) -> Option<QuoteLegRequest> {
        let g = card.games.iter().find(|g| g.sport == Sport::Soccer)?;
        let w = g
            .markets
            .iter()
            .find(|m| m.kind == MarketKind::Winner && m.picks.len() == 3)?;
        let x = w.picks.iter().find(|p| p.outcome == Some(Outcome::Draw))?;
        say(&format!(
            "D: {} – {} ({}), {} ({:?} {}, odds {})",
            g.home.name,
            g.away.name,
            g.league.as_str(),
            x.label,
            x.side,
            x.symbol,
            usdc(x.odds_u)
        ));
        Some(QuoteLegRequest {
            symbol: x.symbol.clone(),
            side: x.side,
        })
    }

    /// `GET /card?sport=soccer&date=` once (an empty card is a valid answer).
    async fn soccer_card(&self) -> Res<CardResponse> {
        let url = format!("{}/card?sport=soccer&date={}", self.desk, self.date);
        let (s, v) = self.get_json(&url).await?;
        if s != 200 {
            return Err(format!("GET /card?sport=soccer: {s} {v}"));
        }
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    async fn card(&self) -> Res<CardResponse> {
        let url = format!("{}/card?sport=nba&date={}", self.desk, self.date);
        let t0 = Instant::now();
        loop {
            let (s, v) = self.get_json(&url).await?;
            if s == 200 {
                let c: CardResponse = serde_json::from_value(v).map_err(|e| e.to_string())?;
                if !c.games.is_empty() {
                    return Ok(c);
                }
            }
            if t0.elapsed() > Duration::from_secs(30) {
                return Err(format!("GET /card: {s}"));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

fn say(s: &str) {
    println!("[e2e] {s}");
}

async fn token_amount(rpc: &RpcClient, ata: &Pubkey) -> Res<u64> {
    let a = rpc
        .get_account(ata)
        .await
        .map_err(|e| format!("token account {ata}: {e}"))?
        .ok_or_else(|| format!("token account {ata}: missing"))?;
    spl_amount(&a.data)
}

/// SPL token account: mint 32, owner 32, amount u64 LE.
fn spl_amount(data: &[u8]) -> Res<u64> {
    data.get(64..72)
        .and_then(|b| b.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or_else(|| "not a token account".into())
}

// --- timeline -----------------------------------------------------------------------------------

/// One line of a ticket's history.
struct Line {
    ts_ms: i64,
    what: String,
}

fn hms(ms: i64) -> String {
    let s = ms.div_euclid(1000);
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        s.div_euclid(3600).rem_euclid(24),
        s.div_euclid(60).rem_euclid(60),
        s.rem_euclid(60),
        ms.rem_euclid(1000)
    )
}

async fn timeline(db: &str) -> Res<()> {
    let opts = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(db)
        .read_only(true);
    let pool = sqlx::SqlitePool::connect_with(opts)
        .await
        .map_err(|e| format!("{db}: {e}"))?;
    type TicketRow = (
        String,
        String,
        i64,
        String,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
    );
    let tickets: Vec<TicketRow> = sqlx::query_as(
        "SELECT ticket, user, nonce, state, stake_u, payout_u, paid_u, tx_open_pending,
                tx_finalize, tx_settle, tx_cash_out, tx_refund, COALESCE(opened_at, 0)
         FROM tickets ORDER BY COALESCE(opened_at, 0), nonce",
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| e.to_string())?;
    println!("TIMELINE ({} tickets, desk wall clock UTC)", tickets.len());
    for (
        ticket,
        user,
        nonce,
        state,
        stake_u,
        payout_u,
        paid_u,
        tx_open,
        tx_fin,
        tx_settle,
        tx_cash,
        tx_refund,
        opened_at,
    ) in tickets
    {
        println!(
            "ticket {ticket} (user {}, nonce {nonce}): {state}, stake {} payout {} paid {}",
            short(&user),
            stake_u.map_or("-".into(), |v| usdc(v.max(0).unsigned_abs())),
            payout_u.map_or("-".into(), |v| usdc(v.max(0).unsigned_abs())),
            paid_u.map_or("-".into(), |v| usdc(v.max(0).unsigned_abs())),
        );
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line {
            ts_ms: opened_at * 1000,
            what: format!("open_pending   tx {}", tx_open.as_deref().unwrap_or("-")),
        });
        let feed: Vec<(String, String, Option<String>, i64)> = sqlx::query_as(
            "SELECT kind, text, payload, ts_ms FROM feed_events WHERE ticket = ? ORDER BY id",
        )
        .bind(&ticket)
        .fetch_all(&pool)
        .await
        .map_err(|e| e.to_string())?;
        for (kind, text, payload, ts_ms) in feed {
            let p: Value = payload
                .as_deref()
                .and_then(|p| serde_json::from_str(p).ok())
                .unwrap_or(Value::Null);
            let extra = match kind.as_str() {
                "fill" => format!(
                    "trade_id {}",
                    p["trade_id"]
                        .as_str()
                        .map_or(p["trade_id"].to_string(), str::to_owned)
                ),
                "mint" => format!("tx {}", tx_fin.as_deref().unwrap_or("-")),
                "settle" => format!("tx {}", tx_settle.as_deref().unwrap_or("-")),
                "cashout" => format!("tx {}", tx_cash.as_deref().unwrap_or("-")),
                "resolve" => {
                    let idx = p["leg_idx"].as_i64().unwrap_or(-1);
                    let tx: Option<String> = sqlx::query_scalar(
                        "SELECT tx_resolve FROM ticket_legs WHERE ticket = ? AND idx = ?",
                    )
                    .bind(&ticket)
                    .bind(idx)
                    .fetch_optional(&pool)
                    .await
                    .map_err(|e| e.to_string())?
                    .flatten();
                    format!("tx {}", tx.as_deref().unwrap_or("-"))
                }
                _ => String::new(),
            };
            lines.push(Line {
                ts_ms,
                what: format!("{kind:<14} {text}  [{extra}]"),
            });
        }
        // Hedge fills after inception (rolls, unwinds, excess): the feed only carries the
        // inception fills.
        // trade_id, symbol, side, price_u, size, contracts, kind, reason, ts_ms
        type FillRow = (i64, String, String, i64, i64, i64, String, String, i64);
        let fills: Vec<FillRow> = sqlx::query_as(
            "SELECT f.trade_id, f.symbol, f.side, f.price_u, f.size, a.contracts, o.kind,
                        o.reason, f.ts_ms
                 FROM fill_allocations a
                 JOIN pascal_fills f ON f.venue = a.venue AND f.trade_id = a.trade_id
                 JOIN hedge_orders o ON o.client_order_id = f.client_order_id
                 WHERE a.ticket = ? AND o.kind <> 'inception'
                 ORDER BY f.seq",
        )
        .bind(&ticket)
        .fetch_all(&pool)
        .await
        .map_err(|e| e.to_string())?;
        let hedge_ms: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT f.trade_id, o.updated_ms FROM pascal_fills f
             JOIN hedge_orders o ON o.client_order_id = f.client_order_id",
        )
        .fetch_all(&pool)
        .await
        .map_err(|e| e.to_string())?;
        let desk_ms: HashMap<i64, i64> = hedge_ms.into_iter().collect();
        for (trade_id, symbol, side, price_u, size, contracts, kind, reason, ts_ms) in fills {
            lines.push(Line {
                // Fill times are the mock's tape clock; place them by the desk's order time.
                ts_ms: desk_ms.get(&trade_id).copied().unwrap_or(ts_ms),
                what: format!(
                    "hedge {kind:<8} {reason}: {side} {size} {symbol} @ {}  ({contracts:+} for this ticket) [trade_id {trade_id}]",
                    usdc(price_u.max(0).unsigned_abs())
                ),
            });
        }
        if let Some(tx) = tx_refund {
            lines.push(Line {
                ts_ms: i64::MAX,
                what: format!("refund_pending tx {tx}"),
            });
        }
        lines.sort_by_key(|l| l.ts_ms);
        for l in lines {
            let t = if l.ts_ms == i64::MAX || l.ts_ms == 0 {
                "            ".to_owned()
            } else {
                hms(l.ts_ms)
            };
            println!("  {t}  {}", l.what);
        }
    }
    let ops: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT op, status, COUNT(*) FROM crank_ops GROUP BY op, status ORDER BY op, status",
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| e.to_string())?;
    println!(
        "crank ops: {}",
        ops.iter()
            .map(|(o, s, n)| format!("{o} {s} ×{n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let orders: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT kind, status, COUNT(*) FROM hedge_orders GROUP BY kind, status ORDER BY kind, status",
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| e.to_string())?;
    println!(
        "hedge orders: {}",
        orders
            .iter()
            .map(|(k, s, n)| format!("{k} {s} ×{n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    pool.close().await;
    Ok(())
}
