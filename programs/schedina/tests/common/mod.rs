//! Shared LiteSVM harness. Loads the `anchor build` output (`target/deploy/schedina.so`,
//! built without the `devnet` feature) and refuses a `.so` older than any source file,
//! so tests never run silently against a stale build. Every send is followed by
//! `assert_invariants` (success or failure): `vault.locked == Σ payout(Open tickets)`,
//! `vault.free + vault.locked ==` vault token balance, and escrow token balance
//! `== Σ (stake + copy fee)` over live `Pending` accounts, and (2.F) every
//! `TipsterProfile` equals the sum of its wallet's own tips, copies never counted.
// Not every test binary uses every helper; the error type is litesvm's.
#![allow(dead_code, clippy::result_large_err)]

use std::path::{Path, PathBuf};

use anchor_lang::{
    prelude::{Clock, Pubkey},
    solana_program::{
        bpf_loader_upgradeable,
        instruction::{AccountMeta, Instruction},
    },
    AccountDeserialize, AnchorDeserialize, AnchorSerialize, Discriminator, InstructionData,
    ToAccountMetas,
};
use anchor_spl::token::{self, TokenAccount};
use litesvm::{
    types::{FailedTransactionMetadata, TransactionMetadata},
    LiteSVM,
};
use schedina::{
    math, CashOutMsg, Config, Params, Pending, QuoteLeg, QuoteMsg, Ticket, TicketState,
    TipsterProfile, Vault, CASHOUT_TAG, QUOTE_TAG,
};
use solana_account::Account;
use solana_ed25519_program::new_ed25519_instruction_with_signature;
use solana_instruction_error::InstructionError;
use solana_keypair::Keypair;
use solana_message::{Message, VersionedMessage};
use solana_signer::Signer;
use solana_transaction::versioned::VersionedTransaction;
use solana_transaction_error::TransactionError;

pub type TxResult = Result<TransactionMetadata, FailedTransactionMetadata>;

/// 1 mock USDC in micro-units.
pub const USDC: u64 = 1_000_000;
/// Unix time the harness starts at.
pub const T0: i64 = 1_793_232_000; // Thu 29 Oct 2026 00:00 UTC
/// Legs start two hours after `T0` unless a test says otherwise.
pub const START: i64 = T0 + 7_200;
/// Largest serialized transaction a validator accepts.
pub const PACKET_DATA_SIZE: usize = 1_232;

pub fn program_so() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/deploy/schedina.so")
}

/// Newest modification time under `src/`. `Cargo.toml` is left out on purpose: a
/// manifest edit that does not change the SBF build (dev-deps, `idl-build`) does not
/// relink the `.so`, so its mtime would not move and the check would misfire.
fn newest_source() -> (std::time::SystemTime, PathBuf) {
    fn walk(dir: &Path, best: &mut (std::time::SystemTime, PathBuf)) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, best);
            } else {
                let t = e.metadata().unwrap().modified().unwrap();
                if t > best.0 {
                    *best = (t, p);
                }
            }
        }
    }
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut best = (std::time::UNIX_EPOCH, src.clone());
    walk(&src, &mut best);
    best
}

/// Panics if `so` is older than any program source file.
pub fn assert_fresh(so: &Path) {
    let built = std::fs::metadata(so).unwrap().modified().unwrap();
    let (newest, path) = newest_source();
    assert!(
        built >= newest,
        "{} is older than {}: rebuild (`anchor build`, or `just program-test`)",
        so.display(),
        path.display()
    );
}

// --- addresses ------------------------------------------------------------

pub fn pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &schedina::ID).0
}
pub fn config_pda() -> Pubkey {
    pda(&[schedina::CONFIG_SEED])
}
pub fn vault_pda() -> Pubkey {
    pda(&[schedina::VAULT_SEED])
}
pub fn mint_pda() -> Pubkey {
    pda(&[schedina::MOCK_SEED, schedina::USDC_SYMBOL])
}
pub fn vault_usdc() -> Pubkey {
    pda(&[schedina::VAULT_USDC_SEED])
}
pub fn escrow_usdc() -> Pubkey {
    pda(&[schedina::ESCROW_USDC_SEED])
}
pub fn pending_pda(user: &Pubkey, nonce: u64) -> Pubkey {
    pda(&[schedina::PENDING_SEED, user.as_ref(), &nonce.to_le_bytes()])
}
pub fn ticket_pda(user: &Pubkey, nonce: u64) -> Pubkey {
    pda(&[schedina::TICKET_SEED, user.as_ref(), &nonce.to_le_bytes()])
}
/// `TipsterProfile` PDA `["tipster", wallet]` (2.F).
pub fn tipster_pda(wallet: &Pubkey) -> Pubkey {
    pda(&[schedina::TIPSTER_SEED, wallet.as_ref()])
}
/// `#[event_cpi]` signer PDA.
pub fn event_authority() -> Pubkey {
    pda(&[b"__event_authority"])
}
pub fn program_data() -> Pubkey {
    Pubkey::find_program_address(&[schedina::ID.as_ref()], &bpf_loader_upgradeable::ID).0
}
pub fn sysvar_instructions() -> Pubkey {
    schedina::INSTRUCTIONS_SYSVAR_ID
}

// --- quotes ---------------------------------------------------------------

/// A leg on game `g` (distinct symbol and game hashes per `g`).
pub fn leg(g: u8, side: u8, price_u: u64, m1_ppm: u32) -> QuoteLeg {
    QuoteLeg {
        symbol_hash: [g; 32],
        game_id_hash: [g.wrapping_add(100); 32],
        start_ts: START,
        side,
        price_u,
        m1_ppm,
    }
}

/// `n` long legs at 0.50 on distinct games, overlapping tier.
pub fn legs_at_half(n: u8) -> Vec<QuoteLeg> {
    (0..n).map(|g| leg(g, 0, 500_000, 50_000)).collect()
}

/// Quote with offered odds computed per C2 at the quote prices (`r = 1`), expiring
/// 10 s after `now`.
pub fn quote(legs: Vec<QuoteLeg>, stake_u: u64, now: i64) -> QuoteMsg {
    let payout = math::payout(stake_u, legs.iter().map(|l| (l.m1_ppm, l.price_u))).unwrap();
    QuoteMsg {
        tag: QUOTE_TAG,
        program_id: schedina::ID,
        legs,
        stake_u,
        odds_offered_u: math::odds(payout, stake_u).unwrap(),
        quote_expires_at: now + 10,
    }
}

pub fn borsh<T: AnchorSerialize>(v: &T) -> Vec<u8> {
    let mut out = Vec::new();
    v.serialize(&mut out).unwrap();
    out
}

pub fn sign(kp: &Keypair, msg: &[u8]) -> [u8; 64] {
    kp.sign_message(msg).as_ref().try_into().unwrap()
}

/// The `Ed25519Program` verify instruction the program accepts: one signature, public
/// key, signature and message all inside it (`instruction_index == u16::MAX`).
pub fn ed25519_ix(msg: &[u8], sig: &[u8; 64], pubkey: &Pubkey) -> Instruction {
    new_ed25519_instruction_with_signature(msg, sig, &pubkey.to_bytes())
}

/// A verify instruction whose message offsets point at bytes `8..8 + len` of
/// instruction `msg_ix` (the "compact" layout the program refuses: absolute indices
/// break when a wallet prepends instructions).
pub fn ed25519_ix_external_msg(
    msg_len: usize,
    msg_ix: u16,
    sig: &[u8; 64],
    pubkey: &Pubkey,
) -> Instruction {
    let mut d = vec![1u8, 0];
    for v in [48u16, u16::MAX, 16, u16::MAX, 8, msg_len as u16, msg_ix] {
        d.extend_from_slice(&v.to_le_bytes());
    }
    d.extend_from_slice(pubkey.as_ref());
    d.extend_from_slice(sig);
    Instruction {
        program_id: schedina::ED25519_PROGRAM_ID,
        accounts: vec![],
        data: d,
    }
}

/// What a wallet may prepend: `SetComputeUnitLimit(limit)` and
/// `SetComputeUnitPrice(micro_lamports)`.
pub fn compute_budget_ixs(limit: u32, micro_lamports: u64) -> Vec<Instruction> {
    let id = anchor_lang::prelude::pubkey!("ComputeBudget111111111111111111111111111111");
    let mut lim = vec![2u8];
    lim.extend_from_slice(&limit.to_le_bytes());
    let mut price = vec![3u8];
    price.extend_from_slice(&micro_lamports.to_le_bytes());
    vec![
        Instruction {
            program_id: id,
            accounts: vec![],
            data: lim,
        },
        Instruction {
            program_id: id,
            accounts: vec![],
            data: price,
        },
    ]
}

/// `ix` with `metas` appended as `remaining_accounts` (2.F, contract L2-4).
pub fn with_remaining(mut ix: Instruction, metas: &[AccountMeta]) -> Instruction {
    ix.accounts.extend_from_slice(metas);
    ix
}

/// Own tip: `[tipster_profile (w)]` for `finalize`, `settle` and `cash_out`.
pub fn tip_accounts(wallet: &Pubkey) -> Vec<AccountMeta> {
    vec![AccountMeta::new(tipster_pda(wallet), false)]
}

/// Copy: `[orig_ticket (r), tipster_usdc (w)]` for `finalize`.
pub fn copy_accounts(orig_ticket: &Pubkey, tipster_usdc: &Pubkey) -> Vec<AccountMeta> {
    vec![
        AccountMeta::new_readonly(*orig_ticket, false),
        AccountMeta::new(*tipster_usdc, false),
    ]
}

pub fn cashout_msg(ticket: &Pubkey, c_u: u64, ts: i64) -> CashOutMsg {
    CashOutMsg {
        tag: CASHOUT_TAG,
        program_id: schedina::ID,
        ticket: *ticket,
        c_u,
        ts,
    }
}

/// Serialized size of a legacy transaction with `n_signers` signatures.
pub fn tx_size(ixs: &[Instruction], payer: &Pubkey, n_signers: usize) -> usize {
    let msg = Message::new(ixs, Some(payer));
    1 + 64 * n_signers + msg.serialize().len()
}

// --- harness --------------------------------------------------------------

/// A wallet with SOL and a mock-USDC token account.
pub struct User {
    pub kp: Keypair,
    pub usdc: Pubkey,
}

impl User {
    pub fn key(&self) -> Pubkey {
        self.kp.pubkey()
    }
}

pub struct Harness {
    pub svm: LiteSVM,
    /// Upgrade authority of the loaded program; funded with 100 SOL.
    pub deployer: Keypair,
    pub authority: Keypair,
    pub crank: Keypair,
    pub quote_signer: Keypair,
}

/// Default parameters: 5 legs, payout ≤ 10,000 USDC, stake ≥ 1 USDC, reserve 100 USDC.
pub fn default_params(crank: Pubkey, quote_signer: Pubkey) -> Params {
    Params {
        crank,
        quote_signer,
        max_legs: 5,
        max_payout: 10_000 * USDC,
        min_stake: USDC,
        reserve: 100 * USDC,
        copy_fee_bps: 200,
        tipster_share_bps: 8_000,
    }
}

/// House money `Harness::ready` funds the vault with.
pub const HOUSE: u64 = 5_000 * USDC;

impl Harness {
    /// Program loaded with `deployer` as its upgrade authority; not initialized.
    pub fn load(so: &Path) -> Self {
        assert!(
            so.exists(),
            "{} not found: run `anchor build` in programs/ first",
            so.display()
        );
        assert_fresh(so);
        let mut svm = LiteSVM::new();
        svm.add_program_from_file(schedina::ID, so)
            .expect("load schedina.so");
        let deployer = Keypair::new();
        svm.airdrop(&deployer.pubkey(), 100_000_000_000).unwrap();

        // ProgramData: u32 tag | u64 slot | Option<Pubkey> upgrade authority (byte 12 = Some).
        let pd = program_data();
        let mut acc = svm.get_account(&pd).unwrap();
        acc.data[12] = 1;
        acc.data[13..45].copy_from_slice(deployer.pubkey().as_ref());
        svm.set_account(pd, acc).unwrap();

        let (authority, crank, quote_signer) = (Keypair::new(), Keypair::new(), Keypair::new());
        for k in [&authority, &crank] {
            svm.airdrop(&k.pubkey(), 100_000_000_000).unwrap();
        }
        let mut h = Harness {
            svm,
            deployer,
            authority,
            crank,
            quote_signer,
        };
        h.set_time(T0);
        h
    }

    pub fn new() -> Self {
        Self::load(&program_so())
    }

    /// Loaded and `init_config`'d with `default_params`.
    pub fn initialized() -> Self {
        let mut h = Self::new();
        let params = default_params(h.crank.pubkey(), h.quote_signer.pubkey());
        let ix = h.ix_init_config(&h.deployer.pubkey(), h.authority.pubkey(), params);
        h.send_deployer(ix).expect("init_config");
        h
    }

    /// Initialized, with `HOUSE` in the vault.
    pub fn ready() -> Self {
        let mut h = Self::initialized();
        let funder = h.user(HOUSE);
        h.fund_vault(&funder, HOUSE).expect("fund_vault");
        h
    }

    // --- transactions ---------------------------------------------------

    /// Sends one instruction (fee payer = first signer), then asserts the invariants.
    pub fn send(&mut self, ix: Instruction, signers: &[&Keypair]) -> TxResult {
        self.send_ixs(&[ix], signers)
    }

    /// Sends `ixs` in one transaction (fee payer = first signer), then asserts the
    /// invariants. Used for an `Ed25519Program` verify followed by the program call.
    pub fn send_ixs(&mut self, ixs: &[Instruction], signers: &[&Keypair]) -> TxResult {
        let payer = signers[0].pubkey();
        let msg = Message::new_with_blockhash(ixs, Some(&payer), &self.svm.latest_blockhash());
        let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), signers).unwrap();
        let res = self.svm.send_transaction(tx);
        self.svm.expire_blockhash();
        self.assert_invariants();
        res
    }

    /// Sends `ix` signed and paid by the deployer.
    pub fn send_deployer(&mut self, ix: Instruction) -> TxResult {
        let kp = self.deployer.insecure_clone();
        self.send(ix, &[&kp])
    }

    pub fn send_authority(&mut self, ix: Instruction) -> TxResult {
        let kp = self.authority.insecure_clone();
        self.send(ix, &[&kp])
    }

    pub fn send_crank(&mut self, ix: Instruction) -> TxResult {
        let kp = self.crank.insecure_clone();
        self.send(ix, &[&kp])
    }

    pub fn fund_vault(&mut self, funder: &User, amount: u64) -> TxResult {
        let ix = self.ix_fund_vault(&funder.key(), &funder.usdc, amount);
        self.send(ix, &[&funder.kp])
    }

    /// `[ed25519 verify(quote, by quote_signer), open_pending]`, signed by `user`.
    pub fn open(
        &mut self,
        user: &User,
        nonce: u64,
        q: &QuoteMsg,
        floor: u64,
        expires_at: i64,
    ) -> TxResult {
        let signer = self.quote_signer.insecure_clone();
        self.open_with(user, nonce, q, floor, expires_at, false, &signer)
    }

    /// `[ed25519 verify(borsh(q), by signer), open_pending]`, signed by `user`.
    #[allow(clippy::too_many_arguments)]
    pub fn open_with(
        &mut self,
        user: &User,
        nonce: u64,
        q: &QuoteMsg,
        floor: u64,
        expires_at: i64,
        is_tip: bool,
        signer: &Keypair,
    ) -> TxResult {
        let ixs = self.open_ixs(user, nonce, &borsh(q), signer, floor, expires_at, is_tip);
        self.send_ixs(&ixs, &[&user.kp])
    }

    /// `[ed25519 verify(msg, by signer), open_pending]`.
    #[allow(clippy::too_many_arguments)]
    pub fn open_ixs(
        &self,
        user: &User,
        nonce: u64,
        msg: &[u8],
        signer: &Keypair,
        floor: u64,
        expires_at: i64,
        is_tip: bool,
    ) -> Vec<Instruction> {
        let sig = sign(signer, msg);
        vec![
            ed25519_ix(msg, &sig, &signer.pubkey()),
            self.ix_open_pending(user, nonce, floor, expires_at, is_tip, None),
        ]
    }

    /// Opens a pending with the default quote of `legs` and `stake_u`, floor = offered
    /// odds, `expires_at = now + 45`. Returns the quote.
    pub fn open_default(
        &mut self,
        user: &User,
        nonce: u64,
        legs: Vec<QuoteLeg>,
        stake_u: u64,
    ) -> QuoteMsg {
        let q = quote(legs, stake_u, self.now());
        let floor = q.odds_offered_u;
        let exp = self.now() + 45;
        self.open(user, nonce, &q, floor, exp)
            .expect("open_pending");
        q
    }

    pub fn finalize(&mut self, user: &User, nonce: u64, fills: Vec<u64>, r_ppm: u32) -> TxResult {
        let ix = self.ix_finalize(&user.key(), nonce, &user.usdc, fills, r_ppm);
        self.send_crank(ix)
    }

    /// Opens and finalizes at the quote prices with `r = 1`; returns the ticket address.
    pub fn mint(&mut self, user: &User, nonce: u64, legs: Vec<QuoteLeg>, stake_u: u64) -> Pubkey {
        let n = legs.len();
        self.open_default(user, nonce, legs, stake_u);
        self.finalize(user, nonce, vec![0; n], 1_000_000)
            .expect("finalize");
        ticket_pda(&user.key(), nonce)
    }

    pub fn resolve(&mut self, ticket: &Pubkey, leg_index: u8, r_ppm: u32) -> TxResult {
        let ix = self.ix_resolve_leg(&self.crank.pubkey(), ticket, leg_index, r_ppm);
        self.send_crank(ix)
    }

    pub fn settle(&mut self, caller: &Keypair, ticket: &Pubkey, user_usdc: &Pubkey) -> TxResult {
        let ix = self.ix_settle(&caller.pubkey(), ticket, user_usdc);
        self.send(ix, &[caller])
    }

    /// `[ed25519 verify(cash-out msg, by signer), cash_out]`, signed by `user`.
    pub fn cash_out_with(&mut self, user: &User, msg: &CashOutMsg, signer: &Keypair) -> TxResult {
        let ixs = self.cash_out_ixs(user, msg, signer);
        self.send_ixs(&ixs, &[&user.kp])
    }

    pub fn cash_out_ixs(
        &self,
        user: &User,
        msg: &CashOutMsg,
        signer: &Keypair,
    ) -> Vec<Instruction> {
        let bytes = borsh(msg);
        let sig = sign(signer, &bytes);
        vec![
            ed25519_ix(&bytes, &sig, &signer.pubkey()),
            self.ix_cash_out(&user.key(), &msg.ticket, &user.usdc, msg.clone()),
        ]
    }

    pub fn cash_out(&mut self, user: &User, ticket: &Pubkey, c_u: u64) -> TxResult {
        let msg = cashout_msg(ticket, c_u, self.now());
        let signer = self.quote_signer.insecure_clone();
        self.cash_out_with(user, &msg, &signer)
    }

    // --- tipsters and copies (2.F) --------------------------------------

    /// `[ed25519 verify(q), open_pending(is_tip, copied_from)]` with floor = offered
    /// odds and `expires_at = now + 45`, signed by `user`.
    pub fn open_flagged(
        &mut self,
        user: &User,
        nonce: u64,
        q: &QuoteMsg,
        is_tip: bool,
        copied_from: Option<Pubkey>,
    ) -> TxResult {
        let msg = borsh(q);
        let sig = sign(&self.quote_signer, &msg);
        let exp = self.now() + 45;
        let ixs = [
            ed25519_ix(&msg, &sig, &self.quote_signer.pubkey()),
            self.ix_open_pending(user, nonce, q.odds_offered_u, exp, is_tip, copied_from),
        ];
        self.send_ixs(&ixs, &[&user.kp])
    }

    /// `finalize` by the crank with `remaining` appended.
    pub fn finalize_with(
        &mut self,
        user: &User,
        nonce: u64,
        fills: Vec<u64>,
        r_ppm: u32,
        remaining: &[AccountMeta],
    ) -> TxResult {
        let ix = self.ix_finalize(&user.key(), nonce, &user.usdc, fills, r_ppm);
        self.send_crank(with_remaining(ix, remaining))
    }

    /// Opens an own tip on `legs` and finalizes it at the quote prices (`r = 1`) with
    /// its profile; returns the ticket.
    pub fn mint_tip(
        &mut self,
        user: &User,
        nonce: u64,
        legs: Vec<QuoteLeg>,
        stake_u: u64,
    ) -> Pubkey {
        let n = legs.len();
        let q = quote(legs, stake_u, self.now());
        ok(self.open_flagged(user, nonce, &q, true, None));
        ok(self.finalize_with(
            user,
            nonce,
            vec![0; n],
            1_000_000,
            &tip_accounts(&user.key()),
        ));
        ticket_pda(&user.key(), nonce)
    }

    /// `settle` with `remaining` appended.
    pub fn settle_with(
        &mut self,
        caller: &Keypair,
        ticket: &Pubkey,
        user_usdc: &Pubkey,
        remaining: &[AccountMeta],
    ) -> TxResult {
        let ix = with_remaining(
            self.ix_settle(&caller.pubkey(), ticket, user_usdc),
            remaining,
        );
        self.send(ix, &[caller])
    }

    /// `cash_out` of `c_u` (fresh desk quote) with `remaining` appended.
    pub fn cash_out_remaining(
        &mut self,
        user: &User,
        ticket: &Pubkey,
        c_u: u64,
        remaining: &[AccountMeta],
    ) -> TxResult {
        let msg = cashout_msg(ticket, c_u, self.now());
        let signer = self.quote_signer.insecure_clone();
        let mut ixs = self.cash_out_ixs(user, &msg, &signer);
        let last = ixs.pop().unwrap();
        ixs.push(with_remaining(last, remaining));
        self.send_ixs(&ixs, &[&user.kp])
    }

    pub fn ix_init_tipster_profile(&self, payer: &Pubkey, wallet: &Pubkey) -> Instruction {
        Self::ix(
            schedina::instruction::InitTipsterProfile {},
            schedina::accounts::InitTipsterProfile {
                payer: *payer,
                wallet: *wallet,
                tipster_profile: tipster_pda(wallet),
                system_program: anchor_lang::system_program::ID,
            },
        )
    }

    pub fn profile(&self, wallet: &Pubkey) -> Option<TipsterProfile> {
        self.try_read(&tipster_pda(wallet))
    }

    // --- instruction builders -------------------------------------------

    fn ix<D: InstructionData, A: ToAccountMetas>(data: D, accounts: A) -> Instruction {
        Instruction::new_with_bytes(schedina::ID, &data.data(), accounts.to_account_metas(None))
    }

    pub fn ix_init_config(
        &self,
        deployer: &Pubkey,
        authority: Pubkey,
        params: Params,
    ) -> Instruction {
        Self::ix(
            schedina::instruction::InitConfig { authority, params },
            schedina::accounts::InitConfig {
                deployer: *deployer,
                config: config_pda(),
                vault: vault_pda(),
                usdc_mint: mint_pda(),
                vault_usdc: vault_usdc(),
                escrow_usdc: escrow_usdc(),
                program: schedina::ID,
                program_data: program_data(),
                token_program: token::ID,
                system_program: anchor_lang::system_program::ID,
            },
        )
    }

    fn set_params_accounts(authority: &Pubkey) -> schedina::accounts::SetParams {
        schedina::accounts::SetParams {
            authority: *authority,
            config: config_pda(),
            event_authority: event_authority(),
            program: schedina::ID,
        }
    }

    pub fn ix_set_params(&self, authority: &Pubkey, params: Params) -> Instruction {
        Self::ix(
            schedina::instruction::SetParams { params },
            Self::set_params_accounts(authority),
        )
    }

    pub fn ix_pause(&self, authority: &Pubkey, paused: bool) -> Instruction {
        Self::ix(
            schedina::instruction::Pause { paused },
            Self::set_params_accounts(authority),
        )
    }

    pub fn ix_fund_vault(&self, funder: &Pubkey, funder_usdc: &Pubkey, amount: u64) -> Instruction {
        Self::ix(
            schedina::instruction::FundVault { amount },
            schedina::accounts::FundVault {
                funder: *funder,
                config: config_pda(),
                vault: vault_pda(),
                usdc_mint: mint_pda(),
                funder_usdc: *funder_usdc,
                vault_usdc: vault_usdc(),
                token_program: token::ID,
                event_authority: event_authority(),
                program: schedina::ID,
            },
        )
    }

    pub fn ix_withdraw_house(&self, authority: &Pubkey, dest: &Pubkey, amount: u64) -> Instruction {
        Self::ix(
            schedina::instruction::WithdrawHouse { amount },
            schedina::accounts::WithdrawHouse {
                authority: *authority,
                config: config_pda(),
                vault: vault_pda(),
                usdc_mint: mint_pda(),
                vault_usdc: vault_usdc(),
                destination: *dest,
                token_program: token::ID,
                event_authority: event_authority(),
                program: schedina::ID,
            },
        )
    }

    pub fn ix_open_pending(
        &self,
        user: &User,
        nonce: u64,
        min_payout_per_unit_u: u64,
        expires_at: i64,
        is_tip: bool,
        copied_from: Option<Pubkey>,
    ) -> Instruction {
        let payer = user.key();
        self.ix_open_pending_paid_by(
            user,
            &payer,
            nonce,
            min_payout_per_unit_u,
            expires_at,
            is_tip,
            copied_from,
        )
    }

    /// `open_pending` whose Pending rent `payer` pays (L8: the desk's sponsor key).
    #[allow(clippy::too_many_arguments)]
    pub fn ix_open_pending_paid_by(
        &self,
        user: &User,
        payer: &Pubkey,
        nonce: u64,
        min_payout_per_unit_u: u64,
        expires_at: i64,
        is_tip: bool,
        copied_from: Option<Pubkey>,
    ) -> Instruction {
        Self::ix(
            schedina::instruction::OpenPending {
                nonce,
                min_payout_per_unit_u,
                expires_at,
                is_tip,
                copied_from,
            },
            schedina::accounts::OpenPending {
                user: user.key(),
                config: config_pda(),
                pending: pending_pda(&user.key(), nonce),
                user_usdc: user.usdc,
                escrow_usdc: escrow_usdc(),
                instructions: sysvar_instructions(),
                token_program: token::ID,
                system_program: anchor_lang::system_program::ID,
                payer: *payer,
                event_authority: event_authority(),
                program: schedina::ID,
            },
        )
    }

    pub fn ix_finalize(
        &self,
        user: &Pubkey,
        nonce: u64,
        user_usdc: &Pubkey,
        fills: Vec<u64>,
        r_ppm: u32,
    ) -> Instruction {
        self.ix_finalize_by(&self.crank.pubkey(), user, nonce, user_usdc, fills, r_ppm)
    }

    pub fn ix_finalize_by(
        &self,
        crank: &Pubkey,
        user: &Pubkey,
        nonce: u64,
        user_usdc: &Pubkey,
        fills: Vec<u64>,
        r_ppm: u32,
    ) -> Instruction {
        self.ix_finalize_rent_to(crank, user, nonce, user_usdc, fills, r_ppm, user)
    }

    /// `finalize` with the Pending rent going to `rent_payer` (L8: `Pending.rent_receiver()`).
    #[allow(clippy::too_many_arguments)]
    pub fn ix_finalize_rent_to(
        &self,
        crank: &Pubkey,
        user: &Pubkey,
        nonce: u64,
        user_usdc: &Pubkey,
        fills: Vec<u64>,
        r_ppm: u32,
        rent_payer: &Pubkey,
    ) -> Instruction {
        Self::ix(
            schedina::instruction::Finalize { fills, r_ppm },
            schedina::accounts::Finalize {
                crank: *crank,
                config: config_pda(),
                vault: vault_pda(),
                pending: pending_pda(user, nonce),
                ticket: ticket_pda(user, nonce),
                rent_payer: *rent_payer,
                user_usdc: *user_usdc,
                usdc_mint: mint_pda(),
                escrow_usdc: escrow_usdc(),
                vault_usdc: vault_usdc(),
                token_program: token::ID,
                system_program: anchor_lang::system_program::ID,
                event_authority: event_authority(),
                program: schedina::ID,
            },
        )
    }

    pub fn ix_refund_pending(
        &self,
        caller: &Pubkey,
        user: &Pubkey,
        nonce: u64,
        user_usdc: &Pubkey,
    ) -> Instruction {
        self.ix_refund_pending_rent_to(caller, user, nonce, user_usdc, user)
    }

    /// `refund_pending` with the Pending rent going to `rent_payer` (L8).
    pub fn ix_refund_pending_rent_to(
        &self,
        caller: &Pubkey,
        user: &Pubkey,
        nonce: u64,
        user_usdc: &Pubkey,
        rent_payer: &Pubkey,
    ) -> Instruction {
        Self::ix(
            schedina::instruction::RefundPending {},
            schedina::accounts::RefundPending {
                caller: *caller,
                config: config_pda(),
                pending: pending_pda(user, nonce),
                rent_payer: *rent_payer,
                user_usdc: *user_usdc,
                usdc_mint: mint_pda(),
                escrow_usdc: escrow_usdc(),
                token_program: token::ID,
                event_authority: event_authority(),
                program: schedina::ID,
            },
        )
    }

    pub fn ix_resolve_leg(
        &self,
        resolver: &Pubkey,
        ticket: &Pubkey,
        leg_index: u8,
        r_ppm: u32,
    ) -> Instruction {
        Self::ix(
            schedina::instruction::ResolveLeg { leg_index, r_ppm },
            schedina::accounts::ResolveLeg {
                resolver: *resolver,
                config: config_pda(),
                ticket: *ticket,
                event_authority: event_authority(),
                program: schedina::ID,
            },
        )
    }

    pub fn ix_settle(&self, caller: &Pubkey, ticket: &Pubkey, user_usdc: &Pubkey) -> Instruction {
        Self::ix(
            schedina::instruction::Settle {},
            schedina::accounts::Settle {
                caller: *caller,
                config: config_pda(),
                vault: vault_pda(),
                ticket: *ticket,
                user_usdc: *user_usdc,
                usdc_mint: mint_pda(),
                vault_usdc: vault_usdc(),
                token_program: token::ID,
                event_authority: event_authority(),
                program: schedina::ID,
            },
        )
    }

    pub fn ix_cash_out(
        &self,
        user: &Pubkey,
        ticket: &Pubkey,
        user_usdc: &Pubkey,
        quote: CashOutMsg,
    ) -> Instruction {
        Self::ix(
            schedina::instruction::CashOut { quote },
            schedina::accounts::CashOut {
                user: *user,
                config: config_pda(),
                vault: vault_pda(),
                ticket: *ticket,
                user_usdc: *user_usdc,
                usdc_mint: mint_pda(),
                vault_usdc: vault_usdc(),
                instructions: sysvar_instructions(),
                token_program: token::ID,
                event_authority: event_authority(),
                program: schedina::ID,
            },
        )
    }

    // --- state setup (not instructions) ---------------------------------

    /// A wallet with 10 SOL and a mock-USDC token account holding `usdc` micro-USDC.
    /// The token account is written directly (the faucet is not in the default build).
    pub fn user(&mut self, usdc: u64) -> User {
        let kp = Keypair::new();
        self.svm.airdrop(&kp.pubkey(), 10_000_000_000).unwrap();
        let usdc_acc = Keypair::new().pubkey();
        self.write_token_account(&usdc_acc, &kp.pubkey(), usdc);
        User { kp, usdc: usdc_acc }
    }

    /// A mock-USDC token account for `owner` (e.g. the authority's withdrawal target).
    pub fn token_account_for(&mut self, owner: &Pubkey) -> Pubkey {
        let addr = Keypair::new().pubkey();
        self.write_token_account(&addr, owner, 0);
        addr
    }

    /// SPL Token account layout: mint 0..32, owner 32..64, amount 64..72, state 108
    /// (1 = Initialized). Adds `amount` to the mint's supply.
    pub fn write_token_account(&mut self, addr: &Pubkey, owner: &Pubkey, amount: u64) {
        let mut data = vec![0u8; TokenAccount::LEN];
        data[0..32].copy_from_slice(mint_pda().as_ref());
        data[32..64].copy_from_slice(owner.as_ref());
        data[64..72].copy_from_slice(&amount.to_le_bytes());
        data[108] = 1;
        let lamports = self.svm.minimum_balance_for_rent_exemption(data.len());
        self.svm
            .set_account(
                *addr,
                Account {
                    lamports,
                    data,
                    owner: token::ID,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
        // Keep the mint supply consistent (offset 36..44 of the mint).
        if let Some(mut m) = self.svm.get_account(&mint_pda()) {
            let s = u64::from_le_bytes(m.data[36..44].try_into().unwrap()) + amount;
            m.data[36..44].copy_from_slice(&s.to_le_bytes());
            self.svm.set_account(mint_pda(), m).unwrap();
        }
    }

    pub fn set_time(&mut self, unix_timestamp: i64) {
        let mut clock: Clock = self.svm.get_sysvar();
        clock.unix_timestamp = unix_timestamp;
        self.svm.set_sysvar(&clock);
    }

    pub fn warp(&mut self, secs: i64) {
        let t = self.now() + secs;
        self.set_time(t);
    }

    pub fn now(&self) -> i64 {
        self.svm.get_sysvar::<Clock>().unix_timestamp
    }

    /// A funded wallet (10 SOL), no token account.
    pub fn wallet(&mut self) -> Keypair {
        let kp = Keypair::new();
        self.svm.airdrop(&kp.pubkey(), 10_000_000_000).unwrap();
        kp
    }

    // --- reads ----------------------------------------------------------

    pub fn try_read<T: AccountDeserialize>(&self, addr: &Pubkey) -> Option<T> {
        let acc = self.svm.get_account(addr)?;
        T::try_deserialize(&mut acc.data.as_slice()).ok()
    }

    pub fn read<T: AccountDeserialize>(&self, addr: &Pubkey) -> T {
        self.try_read(addr)
            .unwrap_or_else(|| panic!("account {addr} missing or wrong type"))
    }

    pub fn config(&self) -> Config {
        self.read(&config_pda())
    }
    pub fn vault(&self) -> Vault {
        self.read(&vault_pda())
    }
    pub fn pending(&self, user: &Pubkey, nonce: u64) -> Pending {
        self.read(&pending_pda(user, nonce))
    }
    pub fn ticket(&self, addr: &Pubkey) -> Ticket {
        self.read(addr)
    }
    pub fn balance(&self, addr: &Pubkey) -> u64 {
        self.read::<TokenAccount>(addr).amount
    }
    pub fn exists(&self, addr: &Pubkey) -> bool {
        self.svm.get_account(addr).is_some_and(|a| a.lamports > 0)
    }

    fn program_accounts<T: AccountDeserialize + Discriminator>(&self) -> Vec<(Pubkey, T)> {
        self.svm
            .get_program_accounts(&schedina::ID)
            .into_iter()
            .filter(|(_, a)| a.lamports > 0 && a.data.starts_with(T::DISCRIMINATOR))
            .map(|(k, a)| (k, T::try_deserialize(&mut a.data.as_slice()).unwrap()))
            .collect()
    }

    // --- invariants (spec section 4) --------------------------------------

    pub fn assert_invariants(&self) {
        let tickets = self.program_accounts::<Ticket>();
        let pendings = self.program_accounts::<Pending>();
        let Some(vault) = self.try_read::<Vault>(&vault_pda()) else {
            assert!(
                tickets.is_empty() && pendings.is_empty(),
                "state without a Vault"
            );
            return;
        };
        let open: u128 = tickets
            .iter()
            .filter(|(_, t)| t.state == TicketState::Open)
            .map(|(_, t)| u128::from(t.payout_u))
            .sum();
        assert_eq!(
            u128::from(vault.locked),
            open,
            "vault.locked != Σ payout(Open)"
        );
        let bal = self.balance(&vault.usdc_ata);
        assert_eq!(
            u128::from(vault.free) + u128::from(vault.locked),
            u128::from(bal),
            "vault.free + vault.locked != vault token balance"
        );
        let escrowed: u128 = pendings
            .iter()
            .map(|(_, p)| u128::from(p.stake_u) + u128::from(p.copy_fee_u))
            .sum();
        assert_eq!(
            u128::from(self.balance(&vault.escrow_ata)),
            escrowed,
            "escrow balance != Σ pending stake + copy fee"
        );
        for (addr, t) in &tickets {
            assert!(t.paid_u <= t.payout_u, "ticket {addr}: paid > payout");
            assert!(
                math::floor_ok(t.payout_u, t.stake_u, t.min_payout_per_unit_u),
                "ticket {addr}: payout below the signed floor"
            );
            // 2.F: a copy names its tipster; only copies carry a copy fee.
            if t.copied_from.is_some() {
                assert!(t.tipster.is_some(), "copy {addr} without a tipster");
            } else {
                assert_eq!(t.copy_fee_u, 0, "ticket {addr}: copy fee without a copy");
            }
        }
        for (addr, p) in &pendings {
            if p.copied_from.is_none() {
                assert_eq!(p.copy_fee_u, 0, "pending {addr}: copy fee without a copy");
            }
            assert!(
                !(p.is_tip && p.copied_from.is_some()),
                "pending {addr}: tip and copy"
            );
        }
        self.assert_profiles(&tickets);
    }

    /// Every `TipsterProfile` equals the sum of its wallet's own tips (2.F): open
    /// fields over `Open` ones, settled fields over `Settled`/`CashedOut` ones; every own
    /// tip has a profile; copies are never counted.
    fn assert_profiles(&self, tickets: &[(Pubkey, Ticket)]) {
        let profiles = self.program_accounts::<TipsterProfile>();
        let own = |t: &Ticket| t.copied_from.is_none() && t.tipster == Some(t.user);
        for (_, t) in tickets.iter().filter(|(_, t)| own(t)) {
            assert!(
                profiles.iter().any(|(_, p)| p.wallet == t.user),
                "own tip of {} without a profile",
                t.user
            );
        }
        for (addr, p) in &profiles {
            assert_eq!(*addr, tipster_pda(&p.wallet), "profile at a wrong address");
            let mine: Vec<&Ticket> = tickets
                .iter()
                .map(|(_, t)| t)
                .filter(|t| own(t) && t.user == p.wallet)
                .collect();
            let open: Vec<&&Ticket> = mine
                .iter()
                .filter(|t| t.state == TicketState::Open)
                .collect();
            let done: Vec<&&Ticket> = mine
                .iter()
                .filter(|t| t.state != TicketState::Open)
                .collect();
            let want = (
                open.len() as u32,
                open.iter().map(|t| t.stake_u).sum::<u64>(),
                done.len() as u32,
                done.iter().filter(|t| t.paid_u > t.stake_u).count() as u32,
                done.iter().map(|t| t.stake_u).sum::<u64>(),
                done.iter().map(|t| t.paid_u).sum::<u64>(),
            );
            let got = (
                p.open_count,
                p.open_staked,
                p.settled_count,
                p.won,
                p.settled_staked,
                p.returned,
            );
            assert_eq!(got, want, "profile {addr} of {} != its own tips", p.wallet);
            assert_eq!(
                p.fees_earned, 0,
                "profile {addr}: copies never touch a profile"
            );
        }
    }
}

// --- error assertions ---------------------------------------------------

pub fn custom_code(res: &TxResult) -> u32 {
    match res {
        Ok(_) => panic!("expected the transaction to fail"),
        Err(f) => match &f.err {
            TransactionError::InstructionError(_, InstructionError::Custom(c)) => *c,
            other => panic!("expected a custom error, got {other:?}\n{:#?}", f.meta.logs),
        },
    }
}

pub fn assert_err(res: &TxResult, e: schedina::error::SchedinaError) {
    let want: u32 = e.into();
    assert_eq!(
        custom_code(res),
        want,
        "want {e:?}: {:#?}",
        res.as_ref().err().map(|f| &f.meta.logs)
    );
}

pub fn assert_anchor_err(res: &TxResult, e: anchor_lang::error::ErrorCode) {
    let want: u32 = e.into();
    assert_eq!(
        custom_code(res),
        want,
        "want {e:?}: {:#?}",
        res.as_ref().err().map(|f| &f.meta.logs)
    );
}

/// Fails at the precompile (instruction 0), before the program runs.
pub fn assert_precompile_err(res: &TxResult) {
    match res {
        Ok(_) => panic!("expected the precompile to reject the transaction"),
        Err(f) => match &f.err {
            TransactionError::InstructionError(0, InstructionError::Custom(_)) => {}
            other => panic!("expected a precompile error at instruction 0, got {other:?}"),
        },
    }
}

pub fn ok(res: TxResult) -> TransactionMetadata {
    match res {
        Ok(m) => m,
        Err(f) => panic!("{:?}\n{:#?}", f.err, f.meta.logs),
    }
}

/// Events of type `E` emitted with `emit_cpi!`, decoded from the transaction's inner
/// instructions (`EVENT_IX_TAG_LE ‖ discriminator ‖ borsh`), in emission order.
pub fn cpi_events<E: AnchorDeserialize + Discriminator>(meta: &TransactionMetadata) -> Vec<E> {
    let prefix = [anchor_lang::event::EVENT_IX_TAG_LE, E::DISCRIMINATOR].concat();
    meta.inner_instructions
        .iter()
        .flatten()
        .filter_map(|ii| ii.instruction.data.strip_prefix(prefix.as_slice()))
        .map(|mut body| E::deserialize(&mut body).unwrap())
        .collect()
}
