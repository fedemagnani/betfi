//! LiteSVM harness for the mock AMM. Loads both `anchor build` outputs
//! (`target/deploy/mock_amm.so` and `schedina.so`) with one key as upgrade authority of
//! both, initializes schedina (for its mock-USDC mint, D-052) and the AMM with the
//! mockBONK and mockSOL mints and pools, and refuses a `mock_amm.so` older than its sources.
#![allow(dead_code, clippy::result_large_err)]

use std::path::{Path, PathBuf};

use anchor_lang::{
    prelude::{Clock, Pubkey},
    solana_program::{
        bpf_loader_upgradeable,
        instruction::{AccountMeta, Instruction},
    },
    AccountDeserialize, AnchorSerialize, InstructionData, ToAccountMetas,
};
use anchor_spl::token::{self, TokenAccount};
use litesvm::{
    types::{FailedTransactionMetadata, TransactionMetadata},
    LiteSVM,
};
use mock_amm::{MockToken, Pool};
use serde_json::Value;
use solana_account::Account;
use solana_ed25519_program::new_ed25519_instruction_with_signature;
use solana_instruction_error::InstructionError;
use solana_keypair::Keypair;
use solana_message::{v0, AddressLookupTableAccount, Message, VersionedMessage};
use solana_signer::Signer;
use solana_transaction::versioned::VersionedTransaction;
use solana_transaction_error::TransactionError;

pub type TxResult = Result<TransactionMetadata, FailedTransactionMetadata>;

pub const USDC: u64 = 1_000_000;
/// 1 mockBONK (5 decimals) and 1 mockSOL (9 decimals) in base units.
pub const BONK: u64 = 100_000;
pub const SOL: u64 = 1_000_000_000;
pub const T0: i64 = 1_793_232_000;
pub const PACKET_DATA_SIZE: usize = 1_232;
/// Address lookup table program.
pub const ALT_ID: Pubkey = mock_amm::ALT_PROGRAM_ID;

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn vectors() -> Value {
    let p = repo_root().join("fixtures/swap/vectors.json");
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn so(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../target/deploy/{name}.so"))
}

fn newest(dir: &Path, best: &mut (std::time::SystemTime, PathBuf)) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            newest(&p, best);
        } else {
            let t = e.metadata().unwrap().modified().unwrap();
            if t > best.0 {
                *best = (t, p);
            }
        }
    }
}

/// Panics if `so` is older than any file under `src`.
fn assert_fresh(so: &Path, src: &Path) {
    let built = std::fs::metadata(so)
        .unwrap_or_else(|_| panic!("{} missing: run `anchor build` in programs/", so.display()))
        .modified()
        .unwrap();
    let mut best = (std::time::UNIX_EPOCH, src.to_path_buf());
    newest(src, &mut best);
    assert!(
        built >= best.0,
        "{} is older than {}: rebuild (`just program-test`)",
        so.display(),
        best.1.display()
    );
}

// --- addresses --------------------------------------------------------------------

pub fn amm_pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &mock_amm::ID).0
}
pub fn sched_pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &schedina::ID).0
}
pub fn amm_config() -> Pubkey {
    amm_pda(&[mock_amm::AMM_CONFIG_SEED])
}
pub fn mock_mint(symbol: &str) -> Pubkey {
    amm_pda(&[mock_amm::MOCK_SEED, symbol.as_bytes()])
}
pub fn mock_token(mint: &Pubkey) -> Pubkey {
    amm_pda(&[mock_amm::TOKEN_SEED, mint.as_ref()])
}
pub fn pool(mint: &Pubkey) -> Pubkey {
    amm_pda(&[mock_amm::POOL_SEED, mint.as_ref()])
}
pub fn vault(pool: &Pubkey, mint: &Pubkey) -> Pubkey {
    amm_pda(&[mock_amm::VAULT_SEED, pool.as_ref(), mint.as_ref()])
}
pub fn faucet_claim(mint: &Pubkey, owner: &Pubkey) -> Pubkey {
    amm_pda(&[mock_amm::FAUCET_SEED, mint.as_ref(), owner.as_ref()])
}
pub fn program_data(program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[program.as_ref()], &bpf_loader_upgradeable::ID).0
}
pub fn usdc_mint() -> Pubkey {
    sched_pda(&[schedina::MOCK_SEED, schedina::USDC_SYMBOL])
}
pub fn sched_config() -> Pubkey {
    sched_pda(&[schedina::CONFIG_SEED])
}
pub fn escrow_usdc() -> Pubkey {
    sched_pda(&[schedina::ESCROW_USDC_SEED])
}
pub fn sched_event_authority() -> Pubkey {
    sched_pda(&[b"__event_authority"])
}
pub fn pending_pda(user: &Pubkey, nonce: u64) -> Pubkey {
    sched_pda(&[schedina::PENDING_SEED, user.as_ref(), &nonce.to_le_bytes()])
}

pub fn ix<D: InstructionData, A: ToAccountMetas>(
    program: Pubkey,
    data: D,
    accounts: A,
) -> Instruction {
    Instruction {
        program_id: program,
        accounts: accounts.to_account_metas(None),
        data: data.data(),
    }
}

/// The custom error code of a failed instruction.
pub fn custom_error(res: &TxResult) -> Option<(u8, u32)> {
    match res.as_ref().err()?.err {
        TransactionError::InstructionError(i, InstructionError::Custom(c)) => Some((i, c)),
        _ => None,
    }
}

/// `AmmError` code of a variant index (6000 + index).
pub fn amm_err(e: mock_amm::error::AmmError) -> u32 {
    6000 + e as u32
}

// --- harness ----------------------------------------------------------------------

pub struct Amm {
    pub svm: LiteSVM,
    /// Upgrade authority of both programs, schedina's `deployer` and the AMM admin.
    pub admin: Keypair,
    pub quote_signer: Keypair,
    pub bonk: Pubkey,
    pub sol: Pubkey,
}

impl Amm {
    /// Both programs loaded; schedina and the AMM initialized; mockBONK (5 dp, faucet
    /// 100,000) and mockSOL (9 dp, faucet 10) with empty pools.
    pub fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        assert_fresh(&so("mock_amm"), &root.join("src"));
        assert_fresh(&so("schedina"), &root.join("../schedina/src"));
        let mut svm = LiteSVM::new();
        svm.add_program_from_file(mock_amm::ID, so("mock_amm"))
            .unwrap();
        svm.add_program_from_file(schedina::ID, so("schedina"))
            .unwrap();
        let admin = Keypair::new();
        svm.airdrop(&admin.pubkey(), 1_000_000_000_000).unwrap();
        for p in [mock_amm::ID, schedina::ID] {
            // ProgramData: u32 tag | u64 slot | Option<Pubkey> (byte 12 = Some).
            let pd = program_data(&p);
            let mut acc = svm.get_account(&pd).unwrap();
            acc.data[12] = 1;
            acc.data[13..45].copy_from_slice(admin.pubkey().as_ref());
            svm.set_account(pd, acc).unwrap();
        }
        let mut h = Amm {
            svm,
            admin,
            quote_signer: Keypair::new(),
            bonk: mock_mint("BONK"),
            sol: mock_mint("SOL"),
        };
        h.set_time(T0);
        h.init_schedina();
        let a = h.admin.pubkey();
        h.send_admin(h.ix_init_amm(&a, usdc_mint()))
            .expect("init_amm");
        h.send_admin(h.ix_init_mock_mint(&a, "BONK", 5, 100_000 * BONK))
            .expect("init BONK");
        h.send_admin(h.ix_init_mock_mint(&a, "SOL", 9, 10 * SOL))
            .expect("init SOL");
        for m in [h.bonk, h.sol] {
            h.send_admin(h.ix_init_pool(&a, &m)).expect("init_pool");
        }
        h
    }

    /// `new()` plus a 5,000,000 mockBONK : 5,000 USDC pool and a 1,000 mockSOL : 150,000
    /// USDC pool, seeded by the admin.
    pub fn seeded() -> Self {
        let mut h = Self::new();
        let (b, s) = (h.bonk, h.sol);
        h.seed(&b, 5_000_000 * BONK, 5_000 * USDC);
        h.seed(&s, 1_000 * SOL, 150_000 * USDC);
        h
    }

    fn init_schedina(&mut self) {
        let crank = Keypair::new().pubkey();
        let params = schedina::Params {
            crank,
            quote_signer: self.quote_signer.pubkey(),
            max_legs: 5,
            max_payout: 10_000 * USDC,
            min_stake: USDC,
            reserve: 100 * USDC,
            copy_fee_bps: 200,
            tipster_share_bps: 8_000,
        };
        let a = self.admin.pubkey();
        let i = ix(
            schedina::ID,
            schedina::instruction::InitConfig {
                authority: a,
                params,
            },
            schedina::accounts::InitConfig {
                deployer: a,
                config: sched_config(),
                vault: sched_pda(&[schedina::VAULT_SEED]),
                usdc_mint: usdc_mint(),
                vault_usdc: sched_pda(&[schedina::VAULT_USDC_SEED]),
                escrow_usdc: escrow_usdc(),
                program: schedina::ID,
                program_data: program_data(&schedina::ID),
                token_program: token::ID,
                system_program: anchor_lang::system_program::ID,
            },
        );
        self.send_admin(i).expect("schedina init_config");
    }

    /// The admin deposits `mock_amount` of `mint` (from its faucet) and `usdc` (written
    /// directly) as the pool's first liquidity.
    pub fn seed(&mut self, mint: &Pubkey, mock_amount: u64, usdc: u64) {
        let a = self.admin.pubkey();
        let m = self.token_account(mint, &a, 0);
        self.send_admin(self.ix_faucet(&a, mint, &m, &a, mock_amount))
            .expect("admin faucet");
        let u = self.token_account(&usdc_mint(), &a, usdc);
        let i = self.ix_add_liquidity(&a, mint, &m, &u, mock_amount, usdc);
        self.send_admin(i).expect("seed");
    }

    // --- instructions -------------------------------------------------------------

    pub fn ix_init_amm(&self, admin: &Pubkey, usdc: Pubkey) -> Instruction {
        ix(
            mock_amm::ID,
            mock_amm::instruction::InitAmm { usdc_mint: usdc },
            mock_amm::accounts::InitAmm {
                admin: *admin,
                amm_config: amm_config(),
                program: mock_amm::ID,
                program_data: program_data(&mock_amm::ID),
                system_program: anchor_lang::system_program::ID,
            },
        )
    }

    pub fn ix_init_mock_mint(
        &self,
        admin: &Pubkey,
        symbol: &str,
        decimals: u8,
        faucet_max: u64,
    ) -> Instruction {
        let mint = mock_mint(symbol);
        ix(
            mock_amm::ID,
            mock_amm::instruction::InitMockMint {
                symbol: symbol.into(),
                decimals,
                faucet_max,
            },
            mock_amm::accounts::InitMockMint {
                admin: *admin,
                amm_config: amm_config(),
                mint,
                mock_token: mock_token(&mint),
                token_program: token::ID,
                system_program: anchor_lang::system_program::ID,
            },
        )
    }

    pub fn ix_init_pool(&self, admin: &Pubkey, mint: &Pubkey) -> Instruction {
        let p = pool(mint);
        ix(
            mock_amm::ID,
            mock_amm::instruction::InitPool {},
            mock_amm::accounts::InitPool {
                admin: *admin,
                amm_config: amm_config(),
                mock_token: mock_token(mint),
                mint: *mint,
                usdc_mint: usdc_mint(),
                pool: p,
                vault: vault(&p, mint),
                vault_usdc: vault(&p, &usdc_mint()),
                token_program: token::ID,
                system_program: anchor_lang::system_program::ID,
            },
        )
    }

    pub fn ix_faucet(
        &self,
        caller: &Pubkey,
        mint: &Pubkey,
        recipient: &Pubkey,
        _recipient_owner: &Pubkey,
        amount: u64,
    ) -> Instruction {
        self.ix_faucet_paid_by(caller, caller, mint, recipient, amount)
    }

    /// `faucet` whose `FaucetClaim` rent `payer` pays (L8: the desk's sponsor key).
    pub fn ix_faucet_paid_by(
        &self,
        caller: &Pubkey,
        payer: &Pubkey,
        mint: &Pubkey,
        recipient: &Pubkey,
        amount: u64,
    ) -> Instruction {
        ix(
            mock_amm::ID,
            mock_amm::instruction::Faucet { amount },
            mock_amm::accounts::Faucet {
                caller: *caller,
                amm_config: amm_config(),
                mock_token: mock_token(mint),
                mint: *mint,
                recipient: *recipient,
                faucet_claim: faucet_claim(mint, caller),
                token_program: token::ID,
                system_program: anchor_lang::system_program::ID,
                payer: *payer,
            },
        )
    }

    pub fn ix_add_liquidity(
        &self,
        provider: &Pubkey,
        mint: &Pubkey,
        provider_mint: &Pubkey,
        provider_usdc: &Pubkey,
        mint_amount: u64,
        max_usdc: u64,
    ) -> Instruction {
        let p = pool(mint);
        ix(
            mock_amm::ID,
            mock_amm::instruction::AddLiquidity {
                mint_amount,
                max_usdc,
            },
            mock_amm::accounts::AddLiquidity {
                provider: *provider,
                amm_config: amm_config(),
                pool: p,
                provider_mint: *provider_mint,
                provider_usdc: *provider_usdc,
                vault: vault(&p, mint),
                vault_usdc: vault(&p, &usdc_mint()),
                token_program: token::ID,
            },
        )
    }

    /// `swap` on `mint`'s pool: `sell` = mock token in, USDC out; else USDC in.
    #[allow(clippy::too_many_arguments)]
    pub fn ix_swap(
        &self,
        user: &Pubkey,
        mint: &Pubkey,
        user_in: &Pubkey,
        user_out: &Pubkey,
        sell: bool,
        amount_in: u64,
        min_out: u64,
    ) -> Instruction {
        let p = pool(mint);
        let (vm, vu) = (vault(&p, mint), vault(&p, &usdc_mint()));
        let (vault_in, vault_out) = if sell { (vm, vu) } else { (vu, vm) };
        ix(
            mock_amm::ID,
            mock_amm::instruction::Swap { amount_in, min_out },
            mock_amm::accounts::Swap {
                user: *user,
                pool: p,
                user_in: *user_in,
                user_out: *user_out,
                vault_in,
                vault_out,
                token_program: token::ID,
            },
        )
    }

    pub fn ix_set_lookup_table(&self, admin: &Pubkey, lut: &Pubkey) -> Instruction {
        ix(
            mock_amm::ID,
            mock_amm::instruction::SetLookupTable {},
            mock_amm::accounts::SetLookupTable {
                admin: *admin,
                amm_config: amm_config(),
                lookup_table: *lut,
            },
        )
    }

    /// `open_pending` from `user_usdc` (no tip, no copy).
    pub fn ix_open_pending(
        &self,
        user: &Pubkey,
        user_usdc: &Pubkey,
        nonce: u64,
        floor: u64,
        expires_at: i64,
    ) -> Instruction {
        ix(
            schedina::ID,
            schedina::instruction::OpenPending {
                nonce,
                min_payout_per_unit_u: floor,
                expires_at,
                is_tip: false,
                copied_from: None,
            },
            schedina::accounts::OpenPending {
                user: *user,
                config: sched_config(),
                pending: pending_pda(user, nonce),
                user_usdc: *user_usdc,
                escrow_usdc: escrow_usdc(),
                instructions: schedina::INSTRUCTIONS_SYSVAR_ID,
                token_program: token::ID,
                system_program: anchor_lang::system_program::ID,
                payer: *user,
                event_authority: sched_event_authority(),
                program: schedina::ID,
            },
        )
    }

    /// A desk-signed `n`-leg quote for `stake_u` and its verify instruction.
    pub fn signed_quote(&self, n: u8, stake_u: u64) -> (schedina::QuoteMsg, Instruction) {
        let legs: Vec<schedina::QuoteLeg> = (0..n)
            .map(|g| schedina::QuoteLeg {
                symbol_hash: [g; 32],
                game_id_hash: [g.wrapping_add(100); 32],
                start_ts: self.now() + 7_200,
                side: 0,
                price_u: 500_000,
                m1_ppm: 50_000,
            })
            .collect();
        let payout =
            schedina::math::payout(stake_u, legs.iter().map(|l| (l.m1_ppm, l.price_u))).unwrap();
        let q = schedina::QuoteMsg {
            tag: schedina::QUOTE_TAG,
            program_id: schedina::ID,
            legs,
            stake_u,
            odds_offered_u: schedina::math::odds(payout, stake_u).unwrap(),
            quote_expires_at: self.now() + 10,
        };
        let mut msg = Vec::new();
        q.serialize(&mut msg).unwrap();
        let sig: [u8; 64] = self
            .quote_signer
            .sign_message(&msg)
            .as_ref()
            .try_into()
            .unwrap();
        let verify = new_ed25519_instruction_with_signature(
            &msg,
            &sig,
            &self.quote_signer.pubkey().to_bytes(),
        );
        (q, verify)
    }

    // --- transactions -------------------------------------------------------------

    pub fn send(&mut self, ixs: &[Instruction], signers: &[&Keypair]) -> TxResult {
        let payer = signers[0].pubkey();
        let msg = Message::new_with_blockhash(ixs, Some(&payer), &self.svm.latest_blockhash());
        let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), signers).unwrap();
        let res = self.svm.send_transaction(tx);
        self.svm.expire_blockhash();
        res
    }

    pub fn send_admin(&mut self, i: Instruction) -> TxResult {
        let kp = self.admin.insecure_clone();
        self.send(&[i], &[&kp])
    }

    /// The v0 transaction of `ixs` paid by `signers[0]` with lookup table `lut`.
    pub fn v0_tx(
        &self,
        ixs: &[Instruction],
        signers: &[&Keypair],
        lut: &AddressLookupTableAccount,
    ) -> VersionedTransaction {
        let payer = signers[0].pubkey();
        let msg = v0::Message::try_compile(
            &payer,
            ixs,
            std::slice::from_ref(lut),
            self.svm.latest_blockhash(),
        )
        .unwrap();
        VersionedTransaction::try_new(VersionedMessage::V0(msg), signers).unwrap()
    }

    pub fn send_v0(&mut self, tx: VersionedTransaction) -> TxResult {
        let res = self.svm.send_transaction(tx);
        self.svm.expire_blockhash();
        res
    }

    // --- state --------------------------------------------------------------------

    /// A token account of `mint` owned by `owner` holding `amount` (written directly).
    pub fn token_account(&mut self, mint: &Pubkey, owner: &Pubkey, amount: u64) -> Pubkey {
        let addr = Keypair::new().pubkey();
        self.write_token_account(&addr, mint, owner, amount);
        addr
    }

    /// SPL Token account layout: mint 0..32, owner 32..64, amount 64..72, state 108.
    pub fn write_token_account(
        &mut self,
        addr: &Pubkey,
        mint: &Pubkey,
        owner: &Pubkey,
        amount: u64,
    ) {
        let old = self.svm.get_account(addr).map_or(0, |a| {
            if a.owner == token::ID && a.data.len() == TokenAccount::LEN {
                u64::from_le_bytes(a.data[64..72].try_into().unwrap())
            } else {
                0
            }
        });
        let mut data = vec![0u8; TokenAccount::LEN];
        data[0..32].copy_from_slice(mint.as_ref());
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
        // Keep the mint supply consistent (offset 36..44).
        let mut m = self.svm.get_account(mint).unwrap();
        let s = u64::from_le_bytes(m.data[36..44].try_into().unwrap()) - old + amount;
        m.data[36..44].copy_from_slice(&s.to_le_bytes());
        self.svm.set_account(*mint, m).unwrap();
    }

    /// Sets a pool's reserves by writing both vault balances.
    pub fn set_reserves(&mut self, mint: &Pubkey, r_mint: u64, r_usdc: u64) {
        let p = pool(mint);
        self.write_token_account(&vault(&p, mint), mint, &p, r_mint);
        self.write_token_account(&vault(&p, &usdc_mint()), &usdc_mint(), &p, r_usdc);
    }

    /// An active address lookup table at a fresh address holding `addresses` (account data
    /// written directly: 56-byte `LookupTableMeta`, then the addresses).
    pub fn lookup_table(&mut self, addresses: &[Pubkey]) -> AddressLookupTableAccount {
        let key = Keypair::new().pubkey();
        let mut data = Vec::with_capacity(56 + 32 * addresses.len());
        data.extend_from_slice(&1u32.to_le_bytes()); // ProgramState::LookupTable
        data.extend_from_slice(&u64::MAX.to_le_bytes()); // deactivation_slot: active
        data.extend_from_slice(&0u64.to_le_bytes()); // last_extended_slot
        data.push(u8::try_from(addresses.len()).unwrap()); // every address already usable
        data.push(1);
        data.extend_from_slice(self.admin.pubkey().as_ref()); // authority
        data.extend_from_slice(&[0, 0]); // padding
        assert_eq!(data.len(), 56);
        for a in addresses {
            data.extend_from_slice(a.as_ref());
        }
        let lamports = self.svm.minimum_balance_for_rent_exemption(data.len());
        self.svm
            .set_account(
                key,
                Account {
                    lamports,
                    data,
                    owner: ALT_ID,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
        AddressLookupTableAccount {
            key,
            addresses: addresses.to_vec(),
        }
    }

    pub fn wallet(&mut self) -> Keypair {
        let kp = Keypair::new();
        self.svm.airdrop(&kp.pubkey(), 10_000_000_000).unwrap();
        kp
    }

    pub fn set_time(&mut self, t: i64) {
        let mut clock: Clock = self.svm.get_sysvar();
        clock.unix_timestamp = t;
        self.svm.set_sysvar(&clock);
    }

    pub fn warp(&mut self, secs: i64) {
        let t = self.now() + secs;
        self.set_time(t);
    }

    pub fn now(&self) -> i64 {
        self.svm.get_sysvar::<Clock>().unix_timestamp
    }

    pub fn read<T: AccountDeserialize>(&self, addr: &Pubkey) -> T {
        let acc = self
            .svm
            .get_account(addr)
            .unwrap_or_else(|| panic!("{addr} missing"));
        T::try_deserialize(&mut acc.data.as_slice()).unwrap()
    }

    pub fn exists(&self, addr: &Pubkey) -> bool {
        self.svm.get_account(addr).is_some_and(|a| a.lamports > 0)
    }

    pub fn balance(&self, addr: &Pubkey) -> u64 {
        self.read::<TokenAccount>(addr).amount
    }

    pub fn pool(&self, mint: &Pubkey) -> Pool {
        self.read(&pool(mint))
    }

    pub fn mock_token(&self, mint: &Pubkey) -> MockToken {
        self.read(&mock_token(mint))
    }

    /// `(mock reserve, USDC reserve)` of `mint`'s pool.
    pub fn reserves(&self, mint: &Pubkey) -> (u64, u64) {
        let p = pool(mint);
        (
            self.balance(&vault(&p, mint)),
            self.balance(&vault(&p, &usdc_mint())),
        )
    }

    /// Static accounts of `[swap(mint), verify, open_pending]` that a lookup table can hold.
    pub fn lut_addresses(&self, mint: &Pubkey) -> Vec<Pubkey> {
        let p = pool(mint);
        vec![
            sched_config(),
            escrow_usdc(),
            schedina::INSTRUCTIONS_SYSVAR_ID,
            token::ID,
            anchor_lang::system_program::ID,
            sched_event_authority(),
            p,
            vault(&p, mint),
            vault(&p, &usdc_mint()),
        ]
    }
}

/// Serialized size of a signed transaction.
pub fn wire_size(tx: &VersionedTransaction) -> usize {
    1 + 64 * tx.signatures.len() + tx.message.serialize().len()
}

/// `metas` with `ix`'s accounts, for building malformed variants.
pub fn metas(ix: &Instruction) -> Vec<AccountMeta> {
    ix.accounts.clone()
}
