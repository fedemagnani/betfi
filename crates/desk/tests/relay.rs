//! Step 8.B: the sponsored-transaction relay. Every refusal rule over transactions built with the
//! desk's own builders (pure [`rules::check`]), the pipeline with a mock chain (simulation cost,
//! rate limits, co-signing), and the HTTP routes against a fake JSON-RPC endpoint (on, off, the
//! sponsor key reused as another desk key).

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::Json;
use axum::routing::post;
use base64::Engine;
use common::{TempDb, pascal_stub};
use desk::Config;
use desk::program::ix::{self, disc};
use desk::program::{
    ASSOCIATED_TOKEN_PROGRAM_ID, Pdas, SYSTEM_PROGRAM_ID, TOKEN_PROGRAM_ID,
    associated_token_address,
};
use desk::relay::chain::{Chain, ChainError, Simulation};
use desk::relay::limits::{ATTEMPTS_PER_IP_PER_HOUR, Limits, PER_IP_PER_HOUR, PER_USER_PER_HOUR};
use desk::relay::rules::{
    self, COMPUTE_BUDGET_PROGRAM_ID, Checked, FAUCET_MEMO, MEMO_PROGRAM_ID, Policy, Refusal,
};
use desk::relay::{MAX_LAMPORTS_PER_TX, RATE_LIMITED, Relay};
use desk::solana::signer::Keypair;
use desk::swap::{self, AmmPdas};
use ed25519_dalek::SigningKey;
use serde_json::{Value, json};
use solana_hash::Hash;
use solana_instruction::{AccountMeta, Instruction};
use solana_message::v0::MessageAddressTableLookup;
use solana_message::{AddressLookupTableAccount, Message, VersionedMessage, v0};
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use solana_transaction::versioned::VersionedTransaction;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

fn key_bytes(seed: u8) -> Vec<u8> {
    let sk = SigningKey::from_bytes(&[seed; 32]);
    let mut b = [seed; 32].to_vec();
    b.extend_from_slice(&sk.verifying_key().to_bytes());
    b
}

fn kp(seed: u8) -> Keypair {
    Keypair::from_bytes(&key_bytes(seed)).unwrap()
}

fn sched() -> Pdas {
    Pdas::new(desk::config::DEFAULT_PROGRAM_ID.parse().unwrap())
}

fn amm() -> AmmPdas {
    AmmPdas::new(swap::DEFAULT_PROGRAM_ID.parse().unwrap(), &sched())
}

fn policy(sponsor: &Pubkey) -> Policy {
    Policy {
        sponsor: *sponsor,
        schedina: sched().program,
        amm: amm().program,
        usdc_mint: sched().usdc_mint(),
    }
}

/// Signs every slot but 0 (the sponsor's, left zero) with the matching key of `signers`.
fn sign(message: VersionedMessage, signers: &[&Keypair]) -> VersionedTransaction {
    let n = usize::from(message.header().num_required_signatures);
    let bytes = message.serialize();
    let mut signatures = vec![Signature::default(); n];
    for (i, key) in message.static_account_keys()[..n]
        .iter()
        .enumerate()
        .skip(1)
    {
        if let Some(k) = signers.iter().find(|k| k.pubkey() == *key) {
            signatures[i] = k.sign(&bytes);
        }
    }
    VersionedTransaction {
        signatures,
        message,
    }
}

fn legacy_msg(ixs: &[Instruction], payer: &Pubkey) -> VersionedMessage {
    VersionedMessage::Legacy(Message::new_with_blockhash(
        ixs,
        Some(payer),
        &Hash::new_from_array([7; 32]),
    ))
}

fn b64(tx: &VersionedTransaction) -> String {
    B64.encode(wincode::serialize(tx).unwrap())
}

/// A legacy transaction paid by `payer`, signed by `signers` (slot 0 zero), as base64.
fn legacy(ixs: &[Instruction], payer: &Pubkey, signers: &[&Keypair]) -> String {
    b64(&sign(legacy_msg(ixs, payer), signers))
}

fn v0_msg(
    ixs: &[Instruction],
    payer: &Pubkey,
    tables: &[AddressLookupTableAccount],
) -> v0::Message {
    v0::Message::try_compile(payer, ixs, tables, Hash::new_from_array([7; 32])).unwrap()
}

fn check(b64: &str, policy: &Policy) -> Result<Checked, Refusal> {
    check_with(b64, policy, &HashMap::new())
}

fn check_with(
    b64: &str,
    policy: &Policy,
    tables: &HashMap<Pubkey, Vec<Pubkey>>,
) -> Result<Checked, Refusal> {
    let tx = rules::decode(b64)?;
    rules::check(&tx, policy, tables)
}

#[track_caller]
fn refused(r: Result<Checked, Refusal>, words: &str) {
    match r {
        Ok(_) => panic!("accepted; expected a refusal with {words:?}"),
        Err(Refusal(reason)) => assert!(reason.contains(words), "{reason:?} lacks {words:?}"),
    }
}

fn memo(text: &[u8], signers: &[Pubkey]) -> Instruction {
    Instruction {
        program_id: MEMO_PROGRAM_ID,
        accounts: signers
            .iter()
            .map(|k| AccountMeta::new_readonly(*k, true))
            .collect(),
        data: text.to_vec(),
    }
}

fn transfer(from: &Pubkey, to: &Pubkey, lamports: u64) -> Instruction {
    let mut data = 2u32.to_le_bytes().to_vec();
    data.extend_from_slice(&lamports.to_le_bytes());
    Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![AccountMeta::new(*from, true), AccountMeta::new(*to, false)],
        data,
    }
}

fn cu_limit(units: u32) -> Instruction {
    let mut data = vec![2];
    data.extend_from_slice(&units.to_le_bytes());
    Instruction {
        program_id: COMPUTE_BUDGET_PROGRAM_ID,
        accounts: vec![],
        data,
    }
}

fn cu_price(micro_lamports: u64) -> Instruction {
    let mut data = vec![3];
    data.extend_from_slice(&micro_lamports.to_le_bytes());
    Instruction {
        program_id: COMPUTE_BUDGET_PROGRAM_ID,
        accounts: vec![],
        data,
    }
}

fn sponsored_faucet(sponsor: &Pubkey, user: &Pubkey) -> Vec<Instruction> {
    let s = sched();
    vec![
        ix::create_ata_idempotent(sponsor, user, &s.usdc_mint()),
        ix::faucet_usdc(&s, sponsor, user, 1_000_000_000),
        memo(FAUCET_MEMO, &[*user]),
    ]
}

fn bet(sponsor: &Pubkey, user: &Pubkey, nonce: u64) -> Vec<Instruction> {
    let qs = kp(9).pubkey();
    vec![
        ix::ed25519_verify(&[1, 2, 3], &[0; 64], &qs),
        ix::open_pending(
            &sched(),
            user,
            sponsor,
            nonce,
            2_000_000,
            1_800_000_000,
            false,
            None,
        ),
    ]
}

/// `cash_out`'s shape (the rules look at the program, the discriminator and account 0).
fn cash_out(user: &Pubkey, user_signs: bool) -> Instruction {
    let s = sched();
    let mut data = disc::CASH_OUT.to_vec();
    data.extend_from_slice(&[0; 99]);
    Instruction {
        program_id: s.program,
        accounts: vec![
            AccountMeta::new_readonly(*user, user_signs),
            AccountMeta::new_readonly(s.config(), false),
            AccountMeta::new(s.vault(), false),
            AccountMeta::new(associated_token_address(user, &s.usdc_mint()), false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
        ],
        data,
    }
}

/// `ix` with account `i` no longer a signer (to name another wallet without its signature).
fn unsigned_at(mut ix: Instruction, i: usize) -> Instruction {
    ix.accounts[i].is_signer = false;
    ix
}

// --- accepted ------------------------------------------------------------------------------------

#[test]
fn accepts_what_the_app_sends() {
    let (sponsor, user) = (kp(1), kp(2));
    let (sp, u) = (sponsor.pubkey(), user.pubkey());
    let pol = policy(&sp);
    let ok = |ixs: &[Instruction]| {
        let c = check(&legacy(ixs, &sp, &[&user]), &pol).expect("accepted");
        assert_eq!(c.user, u);
    };
    ok(&sponsored_faucet(&sp, &u));
    ok(&bet(&sp, &u, 1));
    let mut with_budget = vec![cu_limit(400_000), cu_price(10_000)];
    with_budget.extend(bet(&sp, &u, 2));
    ok(&with_budget);
    ok(&[
        ix::ed25519_verify(&[4; 99], &[0; 64], &kp(9).pubkey()),
        cash_out(&u, true),
    ]);
    // The token faucet: the claim is the user's, the rent the sponsor's.
    let p = amm();
    let bonk = p.mock_mint("BONK");
    ok(&[
        ix::create_ata_idempotent(&sp, &u, &bonk),
        swap::ix::faucet(&p, &u, &sp, &bonk, &u, 5_000_000),
    ]);
    // The user pays their own faucet claim: no memo needed.
    ok(&[
        ix::create_ata_idempotent(&u, &u, &sched().usdc_mint()),
        ix::faucet_usdc(&sched(), &u, &u, 1_000_000_000),
        memo(b"hello", &[u]),
    ]);
}

#[test]
fn accepts_a_v0_bet_with_the_lookup_table() {
    let (sponsor, user) = (kp(1), kp(2));
    let (sp, u) = (sponsor.pubkey(), user.pubkey());
    let (s, p) = (sched(), amm());
    let bonk = p.mock_mint("BONK");
    let table = Pubkey::new_from_array([42; 32]);
    let addresses = swap::ix::lookup_table_addresses(&s, &p, &[bonk]);
    let lut = AddressLookupTableAccount {
        key: table,
        addresses: addresses.clone(),
    };
    let mut ixs = vec![swap::ix::sell_to_usdc(&p, &u, &bonk, 5_000_000, 4_000_000)];
    ixs.extend(bet(&sp, &u, 3));
    let msg = v0_msg(&ixs, &sp, std::slice::from_ref(&lut));
    assert!(!msg.address_table_lookups.is_empty());
    let tx = b64(&sign(VersionedMessage::V0(msg), &[&user]));
    let tables = HashMap::from([(table, addresses)]);
    assert_eq!(check_with(&tx, &policy(&sp), &tables).unwrap().user, u);
    // Without the table's addresses nothing can be checked.
    refused(check(&tx, &policy(&sp)), "could not be read");
}

// --- rule 1: decoding and signatures -------------------------------------------------------------

#[test]
fn rule1_refuses_what_does_not_decode() {
    let pol = policy(&kp(1).pubkey());
    refused(check("not base64!", &pol), "not base64");
    refused(
        check(&B64.encode([1u8; 200]), &pol),
        "not a Solana transaction",
    );
    refused(check(&B64.encode([0u8; 1_300]), &pol), "over 1232");
    refused(check(&"A".repeat(5_000), &pol), "too large");
    // Trailing bytes after a valid transaction.
    let good = legacy(
        &bet(&kp(1).pubkey(), &kp(2).pubkey(), 1),
        &kp(1).pubkey(),
        &[&kp(2)],
    );
    let mut wire = B64.decode(good).unwrap();
    wire.push(0);
    refused(check(&B64.encode(&wire), &pol), "not a Solana transaction");
}

#[test]
fn rule1_needs_two_signatures_sponsor_first_and_the_users_valid() {
    let (sponsor, user, other) = (kp(1), kp(2), kp(3));
    let (sp, u, o) = (sponsor.pubkey(), user.pubkey(), other.pubkey());
    let pol = policy(&sp);
    // One signature: the user pays for themselves.
    refused(
        check(&legacy(&[memo(b"x", &[u])], &u, &[&user]), &pol),
        "exactly two signatures",
    );
    // Three signatures.
    refused(
        check(&legacy(&[memo(b"x", &[u, o])], &sp, &[&user, &other]), &pol),
        "exactly two signatures",
    );
    // Two signatures but another fee payer.
    refused(
        check(&legacy(&[memo(b"x", &[u])], &o, &[&user]), &pol),
        "fee payer must be the sponsor",
    );
    // The user's slot left empty, or signed by another key, or over other bytes.
    refused(
        check(&legacy(&bet(&sp, &u, 1), &sp, &[]), &pol),
        "signature does not match",
    );
    let mut tx = sign(legacy_msg(&bet(&sp, &u, 1), &sp), &[&user]);
    tx.signatures[1] = other.sign(&tx.message.serialize());
    refused(check(&b64(&tx), &pol), "signature does not match");
    let mut tx = sign(legacy_msg(&bet(&sp, &u, 1), &sp), &[&user]);
    tx.signatures[1] = user.sign(b"something else");
    refused(check(&b64(&tx), &pol), "signature does not match");
}

// --- rule 2: the program allowlist ---------------------------------------------------------------

#[test]
fn rule2_refuses_programs_and_instructions_off_the_list() {
    let (sponsor, user) = (kp(1), kp(2));
    let (sp, u) = (sponsor.pubkey(), user.pubkey());
    let pol = policy(&sp);
    let t = |ixs: &[Instruction]| check(&legacy(ixs, &sp, &[&user]), &pol);
    // Any System program instruction, even the user's own transfer.
    refused(
        t(&[transfer(&u, &kp(5).pubkey(), 1), memo(b"x", &[u])]),
        "system program",
    );
    // The crank's schedina instructions.
    for (d, name) in [
        (disc::FINALIZE, "finalize"),
        (disc::SETTLE, "settle"),
        (disc::REFUND_PENDING, "refund_pending"),
        (disc::RESOLVE_LEG, "resolve_leg"),
    ] {
        let ix = Instruction {
            program_id: sched().program,
            accounts: vec![AccountMeta::new_readonly(u, true)],
            data: d.to_vec(),
        };
        refused(t(&[ix]), &format!("{name} is not relayed"));
    }
    refused(
        t(&[Instruction {
            program_id: sched().program,
            accounts: vec![AccountMeta::new_readonly(u, true)],
            data: vec![1, 2],
        }]),
        "is not relayed",
    );
    // Other mock-AMM instructions.
    refused(
        t(&[swap::ix::add_liquidity(
            &amm(),
            &u,
            &amm().mock_mint("BONK"),
            1,
            1,
        )]),
        "only swap and the token faucet",
    );
    // Unknown programs (SPL Token itself included).
    let unknown = Pubkey::new_from_array([77; 32]);
    refused(
        t(&[Instruction {
            program_id: unknown,
            accounts: vec![AccountMeta::new_readonly(u, true)],
            data: vec![],
        }]),
        &format!("program {unknown} is not relayed"),
    );
    refused(
        t(&[Instruction {
            program_id: TOKEN_PROGRAM_ID,
            accounts: vec![AccountMeta::new_readonly(u, true)],
            data: vec![3],
        }]),
        "is not relayed",
    );
    // ATA: only CreateIdempotent.
    let mut create = ix::create_ata_idempotent(&sp, &u, &sched().usdc_mint());
    create.data = vec![];
    refused(
        t(&[create, memo(b"x", &[u])]),
        "only creating a token account",
    );
    // Compute budget: price capped, nothing but limit and price.
    let mut ixs = vec![cu_price(10_001)];
    ixs.extend(bet(&sp, &u, 1));
    refused(t(&ixs), "over 10000");
    let mut ixs = vec![Instruction {
        program_id: COMPUTE_BUDGET_PROGRAM_ID,
        accounts: vec![],
        data: [1u8, 0, 0, 1, 0].to_vec(), // RequestHeapFrame
    }];
    ixs.extend(bet(&sp, &u, 1));
    refused(t(&ixs), "only compute unit limit and price");
}

// --- rule 3: where the sponsor may stand ---------------------------------------------------------

#[test]
fn rule3_the_sponsor_only_pays() {
    let (sponsor, user) = (kp(1), kp(2));
    let (sp, u) = (sponsor.pubkey(), user.pubkey());
    let pol = policy(&sp);
    let t = |ixs: &[Instruction]| check(&legacy(ixs, &sp, &[&user]), &pol);
    let (s, p) = (sched(), amm());
    let bonk = p.mock_mint("BONK");
    // A System transfer from the sponsor.
    refused(
        t(&[transfer(&sp, &u, 1_000_000_000), memo(b"x", &[u])]),
        "system program",
    );
    // The sponsor as a swap's token source, or as open_pending's user.
    refused(
        t(&[swap::ix::swap(
            &p,
            &u,
            &bonk,
            &sp,
            &associated_token_address(&u, &s.usdc_mint()),
            true,
            1,
            1,
        )]),
        "the sponsor may only pay",
    );
    refused(
        t(&[
            ix::open_pending(&s, &sp, &sp, 1, 1, 1, false, None),
            memo(b"x", &[u]),
        ]),
        "the sponsor may only pay",
    );
    // As the mock-AMM faucet's caller (its claim), not its payer.
    refused(
        t(&[
            swap::ix::faucet(&p, &sp, &sp, &bonk, &u, 1),
            memo(b"x", &[u]),
        ]),
        "the sponsor may only pay",
    );
    // In a memo, or in the Ed25519 instruction's accounts.
    let mut ixs = sponsored_faucet(&sp, &u);
    ixs[2] = memo(FAUCET_MEMO, &[u, sp]);
    refused(t(&ixs), "the sponsor may only pay");
    let mut verify = ix::ed25519_verify(&[1], &[0; 64], &kp(9).pubkey());
    verify.accounts.push(AccountMeta::new(sp, false));
    refused(t(&[verify, memo(b"x", &[u])]), "the sponsor may only pay");
    // As the owner of the account an ATA creation makes.
    refused(
        t(&[ix::create_ata_idempotent(&sp, &sp, &bonk), memo(b"x", &[u])]),
        "the sponsor may only pay",
    );
}

#[test]
fn rule3_a_sponsored_faucet_pays_only_the_user_and_carries_the_memo() {
    let (sponsor, user) = (kp(1), kp(2));
    let (sp, u) = (sponsor.pubkey(), user.pubkey());
    let pol = policy(&sp);
    let t = |ixs: &[Instruction]| check(&legacy(ixs, &sp, &[&user]), &pol);
    let s = sched();
    let other = kp(3).pubkey();
    // Paying another wallet's USDC account.
    refused(
        t(&[
            ix::create_ata_idempotent(&sp, &u, &s.usdc_mint()),
            ix::faucet_usdc(&s, &sp, &other, 1_000_000_000),
            memo(FAUCET_MEMO, &[u]),
        ]),
        "only your own USDC account",
    );
    // No memo, another memo, or the memo not signed by the user.
    let mut ixs = sponsored_faucet(&sp, &u);
    ixs.pop();
    refused(t(&ixs), "exactly two signatures");
    ixs.push(cash_out(&u, true));
    refused(t(&ixs), "must carry the memo");
    let mut ixs = sponsored_faucet(&sp, &u);
    ixs[2] = memo(b"schedina:faucet!", &[u]);
    refused(t(&ixs), "must carry the memo");
    let mut ixs = sponsored_faucet(&sp, &u);
    ixs[2] = memo(FAUCET_MEMO, &[]);
    ixs.push(memo(b"x", &[u]));
    refused(t(&ixs), "must carry the memo");
}

#[test]
fn rule3_lookup_tables_never_hold_the_sponsor() {
    let (sponsor, user) = (kp(1), kp(2));
    let (sp, u) = (sponsor.pubkey(), user.pubkey());
    let pol = policy(&sp);
    let table = Pubkey::new_from_array([42; 32]);
    let s = sched();
    let mut addresses = swap::ix::lookup_table_addresses(&s, &amm(), &[]);
    addresses.push(sp);
    let lut = AddressLookupTableAccount {
        key: table,
        addresses: addresses.clone(),
    };
    let tables = HashMap::from([(table, addresses.clone())]);
    let build = |extra: MessageAddressTableLookup| {
        let mut msg = v0_msg(&bet(&sp, &u, 1), &sp, std::slice::from_ref(&lut));
        msg.address_table_lookups.push(extra);
        b64(&sign(VersionedMessage::V0(msg), &[&user]))
    };
    let sponsor_index = u8::try_from(addresses.len() - 1).unwrap();
    // A second lookup of the same table loading the sponsor (unused, still loaded).
    refused(
        check_with(
            &build(MessageAddressTableLookup {
                account_key: table,
                writable_indexes: vec![],
                readonly_indexes: vec![sponsor_index],
            }),
            &pol,
            &tables,
        ),
        "lookup table holds the sponsor",
    );
    // An index past the table's end.
    refused(
        check_with(
            &build(MessageAddressTableLookup {
                account_key: table,
                writable_indexes: vec![200],
                readonly_indexes: vec![],
            }),
            &pol,
            &tables,
        ),
        "has no address 200",
    );
    // An address loaded twice (the config is already loaded by the first lookup).
    refused(
        check_with(
            &build(MessageAddressTableLookup {
                account_key: table,
                writable_indexes: vec![],
                readonly_indexes: vec![0],
            }),
            &pol,
            &tables,
        ),
        "listed twice",
    );
}

// --- rule 4: the user's own instructions ---------------------------------------------------------

#[test]
fn rule4_instructions_are_for_the_signing_wallet() {
    let (sponsor, user) = (kp(1), kp(2));
    let (sp, u) = (sponsor.pubkey(), user.pubkey());
    let pol = policy(&sp);
    let t = |ixs: &[Instruction]| check(&legacy(ixs, &sp, &[&user]), &pol);
    let (s, p) = (sched(), amm());
    let bonk = p.mock_mint("BONK");
    let victim = kp(4).pubkey();
    let sig = memo(b"x", &[u]);
    refused(
        t(&[
            sig.clone(),
            unsigned_at(ix::open_pending(&s, &victim, &sp, 1, 1, 1, false, None), 0),
        ]),
        "open_pending must be for the signing wallet",
    );
    refused(
        t(&[sig.clone(), cash_out(&victim, false)]),
        "cash_out must be for the signing wallet",
    );
    refused(
        t(&[
            sig.clone(),
            unsigned_at(swap::ix::sell_to_usdc(&p, &victim, &bonk, 1, 1), 0),
        ]),
        "swap must be for the signing wallet",
    );
    refused(
        t(&[
            sig.clone(),
            unsigned_at(swap::ix::faucet(&p, &victim, &sp, &bonk, &victim, 1), 0),
        ]),
        "the token faucet must be for the signing wallet",
    );
    refused(
        t(&[sig.clone(), ix::create_ata_idempotent(&sp, &victim, &bonk)]),
        "the token account creation must be for the signing wallet",
    );
    // Too short to tell.
    refused(
        t(&[
            Instruction {
                program_id: p.program,
                accounts: vec![],
                data: swap::ix::disc::SWAP.to_vec(),
            },
            sig,
        ]),
        "too few accounts",
    );
}

// --- the pipeline: rules 5–7 with a mock chain ---------------------------------------------------

/// A chain that answers one simulation and records what it was asked to send.
struct MockChain {
    /// Accounts that do not exist (default: every account exists).
    missing: Mutex<Vec<Pubkey>>,
    sim: Mutex<Result<Simulation, ChainError>>,
    sent: Mutex<Vec<Vec<u8>>>,
    send_err: Mutex<Option<ChainError>>,
}

impl MockChain {
    fn costing(lamports: u64) -> Arc<Self> {
        Arc::new(MockChain {
            missing: Mutex::default(),
            sim: Mutex::new(Ok(Simulation {
                err: None,
                sponsor_before: 1_000_000_000,
                sponsor_after: 1_000_000_000 - lamports,
            })),
            sent: Mutex::default(),
            send_err: Mutex::default(),
        })
    }
}

#[async_trait]
impl Chain for MockChain {
    async fn lookup_tables(
        &self,
        keys: &[Pubkey],
    ) -> Result<HashMap<Pubkey, Vec<Pubkey>>, ChainError> {
        Err(ChainError::Refused(format!(
            "lookup table {} does not exist",
            keys[0]
        )))
    }
    async fn missing(&self, keys: &[Pubkey]) -> Result<Vec<Pubkey>, ChainError> {
        let missing = self.missing.lock().unwrap();
        Ok(keys
            .iter()
            .filter(|k| missing.contains(k))
            .copied()
            .collect())
    }
    async fn simulate(&self, _wire: &[u8], _sponsor: &Pubkey) -> Result<Simulation, ChainError> {
        self.sim.lock().unwrap().clone()
    }
    async fn send(&self, wire: &[u8]) -> Result<String, ChainError> {
        if let Some(e) = self.send_err.lock().unwrap().clone() {
            return Err(e);
        }
        self.sent.lock().unwrap().push(wire.to_vec());
        let tx: VersionedTransaction = wincode::deserialize(wire).unwrap();
        Ok(tx.signatures[0].to_string())
    }
}

fn relay_with(chain: Arc<MockChain>, daily: u64) -> Relay {
    Relay::new(kp(1), sched().program, amm().program, chain, daily)
}

#[tokio::test]
async fn pipeline_cosigns_and_sends() {
    let chain = MockChain::costing(2_100_000);
    let relay = relay_with(chain.clone(), 2_000_000_000);
    let (sp, user) = (kp(1).pubkey(), kp(2));
    let tx = legacy(&sponsored_faucet(&sp, &user.pubkey()), &sp, &[&user]);
    let sig = relay.submit(&tx, "10.0.0.1", 1_000).await.expect("relayed");
    let sent = chain.sent.lock().unwrap().pop().unwrap();
    let sent: VersionedTransaction = wincode::deserialize(&sent).unwrap();
    assert_eq!(sent.signatures[0].to_string(), sig);
    // Both slots now verify over the message the user signed.
    let msg = sent.message.serialize();
    for (i, k) in [sp, user.pubkey()].iter().enumerate() {
        let vk = ed25519_dalek::VerifyingKey::from_bytes(&k.to_bytes()).unwrap();
        let s = ed25519_dalek::Signature::from_bytes(sent.signatures[i].as_array());
        vk.verify_strict(&msg, &s).expect("signature verifies");
    }
    assert_eq!(relay.limits().spent_today(1_000), 2_100_000);
    assert_eq!(relay.info().sponsor, sp.to_string());
    assert_eq!(relay.info().max_lamports_per_tx, MAX_LAMPORTS_PER_TX);
}

#[tokio::test]
async fn rule5_refuses_failed_or_costly_simulations() {
    let (sp, user) = (kp(1).pubkey(), kp(2));
    let tx = legacy(&sponsored_faucet(&sp, &user.pubkey()), &sp, &[&user]);
    let chain = MockChain::costing(MAX_LAMPORTS_PER_TX + 1);
    let relay = relay_with(chain.clone(), u64::MAX);
    let e = relay.submit(&tx, "ip", 0).await.unwrap_err();
    assert_eq!(e.status_code().as_u16(), 422);
    assert!(
        e.0.reason.contains("over the 10000000 limit"),
        "{}",
        e.0.reason
    );
    // Exactly the cap is fine.
    *chain.sim.lock().unwrap() = Ok(Simulation {
        err: None,
        sponsor_before: MAX_LAMPORTS_PER_TX,
        sponsor_after: 0,
    });
    relay.submit(&tx, "ip", 0).await.expect("at the cap");
    *chain.sim.lock().unwrap() = Ok(Simulation {
        err: Some("the program refused it: faucet cooldown: one claim per hour".into()),
        sponsor_before: 0,
        sponsor_after: 0,
    });
    let e = relay.submit(&tx, "ip", 0).await.unwrap_err();
    assert_eq!(
        (e.status_code().as_u16(), e.0.reason.as_str()),
        (
            422,
            "the program refused it: faucet cooldown: one claim per hour"
        )
    );
    // The chain down: 503; the network refusing it at send: 422.
    *chain.sim.lock().unwrap() = Err(ChainError::Unavailable("Solana RPC: down".into()));
    assert_eq!(
        relay
            .submit(&tx, "ip", 0)
            .await
            .unwrap_err()
            .status_code()
            .as_u16(),
        503
    );
    *chain.sim.lock().unwrap() = Ok(Simulation {
        err: None,
        sponsor_before: 10,
        sponsor_after: 5,
    });
    *chain.send_err.lock().unwrap() = Some(ChainError::Refused("blockhash expired".into()));
    assert_eq!(
        relay
            .submit(&tx, "ip", 0)
            .await
            .unwrap_err()
            .status_code()
            .as_u16(),
        422
    );
    // Nothing reaches the chain for a transaction the rules refuse.
    *chain.send_err.lock().unwrap() = None;
    let before = chain.sent.lock().unwrap().len();
    let bad = legacy(
        &[
            transfer(&sp, &user.pubkey(), 1),
            memo(b"x", &[user.pubkey()]),
        ],
        &sp,
        &[&user],
    );
    assert_eq!(
        relay
            .submit(&bad, "ip", 0)
            .await
            .unwrap_err()
            .status_code()
            .as_u16(),
        422
    );
    // A v0 transaction whose table cannot be read is refused.
    let lut = AddressLookupTableAccount {
        key: Pubkey::new_from_array([42; 32]),
        addresses: swap::ix::lookup_table_addresses(&sched(), &amm(), &[]),
    };
    let v0 = b64(&sign(
        VersionedMessage::V0(v0_msg(&bet(&sp, &user.pubkey(), 1), &sp, &[lut])),
        &[&user],
    ));
    let e = relay.submit(&v0, "ip", 0).await.unwrap_err();
    assert!(e.0.reason.contains("does not exist"), "{}", e.0.reason);
    assert_eq!(chain.sent.lock().unwrap().len(), before);
}

#[tokio::test]
async fn rule6_rate_limits_per_user_and_per_day() {
    let (sp, user) = (kp(1).pubkey(), kp(2));
    let tx = legacy(&bet(&sp, &user.pubkey(), 1), &sp, &[&user]);
    let relay = relay_with(MockChain::costing(1_000), u64::MAX);
    let t0 = 1_800_000_000_000u64;
    for i in 0..PER_USER_PER_HOUR {
        relay
            .submit(&tx, &format!("ip{i}"), t0 + i as u64)
            .await
            .expect("under the limit");
    }
    let e = relay.submit(&tx, "ip-new", t0 + 100).await.unwrap_err();
    assert_eq!(
        (e.status_code().as_u16(), e.0.reason.as_str()),
        (422, RATE_LIMITED)
    );
    // An hour later the window has moved on.
    relay
        .submit(&tx, "ip-new", t0 + 3_600_001)
        .await
        .expect("next hour");

    // The daily budget of simulated spend.
    let relay = relay_with(MockChain::costing(4_000_000), 10_000_000);
    relay.submit(&tx, "a", t0).await.unwrap();
    relay.submit(&tx, "b", t0).await.unwrap();
    let e = relay.submit(&tx, "c", t0).await.unwrap_err();
    assert_eq!(e.0.reason, RATE_LIMITED);
    assert_eq!(relay.limits().spent_today(t0), 8_000_000);
    relay
        .submit(&tx, "c", t0 + 86_400_000)
        .await
        .expect("the next UTC day");
}

/// A user who closes a token account the sponsor created keeps its rent: the relay never pays
/// to create the same account twice, while re-claims into the still-open account pass.
#[tokio::test]
async fn the_sponsor_pays_for_a_token_account_once() {
    let chain = MockChain::costing(3_300_000);
    let relay = relay_with(chain.clone(), 2_000_000_000);
    let (sp, user) = (kp(1).pubkey(), kp(2));
    let ata = associated_token_address(&user.pubkey(), &sched().usdc_mint());
    chain.missing.lock().unwrap().push(ata);
    let tx = legacy(&sponsored_faucet(&sp, &user.pubkey()), &sp, &[&user]);
    relay
        .submit(&tx, "10.0.0.1", 1_000)
        .await
        .expect("first creation");
    // The account exists now: a later claim into it passes.
    chain.missing.lock().unwrap().clear();
    relay
        .submit(&tx, "10.0.0.1", 2_000)
        .await
        .expect("account still open");
    // Closed by its owner: the sponsor will not create it again.
    chain.missing.lock().unwrap().push(ata);
    let e = relay.submit(&tx, "10.0.0.1", 3_000).await.unwrap_err();
    assert!(
        e.0.reason.contains("already paid for this token account"),
        "{e:?}"
    );
}

#[tokio::test]
async fn attempts_per_ip_are_capped_refused_ones_included() {
    let relay = relay_with(MockChain::costing(5_000), 2_000_000_000);
    for i in 0..ATTEMPTS_PER_IP_PER_HOUR as u64 {
        let e = relay
            .submit("not base64!", "10.0.0.9", 1_000 + i)
            .await
            .unwrap_err();
        assert_ne!(e.0.reason, "rate limited");
    }
    let e = relay
        .submit("not base64!", "10.0.0.9", 5_000)
        .await
        .unwrap_err();
    assert_eq!(e.0.reason, "rate limited");
    // Another address is not affected; the hour slides.
    let e = relay
        .submit("not base64!", "10.0.0.8", 5_000)
        .await
        .unwrap_err();
    assert_ne!(e.0.reason, "rate limited");
    let e = relay
        .submit("not base64!", "10.0.0.9", 1_000 + 3_600_000)
        .await
        .unwrap_err();
    assert_ne!(e.0.reason, "rate limited");
}

#[test]
fn rule6_limits_per_ip_and_day() {
    let l = Limits::new(5_000);
    let t0 = 1_000_000_000u64;
    for i in 0..PER_IP_PER_HOUR {
        assert!(l.take(&format!("user{i}"), "1.2.3.4", 0, t0));
    }
    assert!(!l.allows("fresh", "1.2.3.4", 0, t0));
    assert!(!l.take("fresh", "1.2.3.4", 0, t0));
    assert!(l.take("fresh", "5.6.7.8", 0, t0));
    assert!(l.take("fresh", "1.2.3.4", 0, t0 + 3_600_000));
    // Daily lamports: 5,000 per UTC day.
    assert!(l.take("x", "9", 3_000, t0));
    assert!(!l.allows("x", "9", 2_001, t0));
    assert!(l.take("x", "9", 2_000, t0));
    assert!(!l.take("y", "10", 1, t0));
    let tomorrow = (t0 / 86_400_000 + 1) * 86_400_000;
    assert_eq!(l.spent_today(tomorrow), 0);
    assert!(l.take("y", "10", 5_000, tomorrow));
}

// --- HTTP: GET/POST /relay and rule 7 ------------------------------------------------------------

fn write_key(dir: &std::path::Path, name: &str, seed: u8) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, serde_json::to_string(&key_bytes(seed)).unwrap()).unwrap();
    p
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "schedina-relay-{tag}-{}-{}",
        std::process::id(),
        desk::now_ms()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A fake JSON-RPC endpoint: simulations cost `cost` lamports; sends are recorded.
async fn fake_rpc(cost: u64) -> (String, Arc<Mutex<Vec<Vec<u8>>>>) {
    let sent: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
    let rec = sent.clone();
    let app = axum::Router::new().route(
        "/",
        post(move |Json(req): Json<Value>| {
            let rec = rec.clone();
            async move {
                let result = match req["method"].as_str().unwrap_or("") {
                    "simulateTransaction" => json!({
                        "context": {"slot": 1},
                        "value": {
                            "err": null,
                            "logs": ["Program log: ok"],
                            "accounts": [{"lamports": 1_000_000_000 - cost, "owner": "11111111111111111111111111111111", "data": ["", "base64"], "executable": false, "rentEpoch": 0}],
                            "preBalances": [1_000_000_000u64],
                        }
                    }),
                    "getGenesisHash" => json!("LocalGenesis1111111111111111111111111111111"),
                    // No account exists yet (the sponsored ATA is a first creation).
                    "getMultipleAccounts" => {
                        let n = req["params"][0].as_array().map_or(0, Vec::len);
                        json!({"context": {"slot": 1}, "value": vec![Value::Null; n]})
                    }
                    "sendTransaction" => {
                        let wire = B64.decode(req["params"][0].as_str().unwrap()).unwrap();
                        let tx: VersionedTransaction = wincode::deserialize(&wire).unwrap();
                        rec.lock().unwrap().push(wire);
                        json!(tx.signatures[0].to_string())
                    }
                    m => return Json(json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32601, "message": format!("no {m}")}})),
                };
                Json(json!({"jsonrpc": "2.0", "id": 1, "result": result}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    (format!("http://{addr}"), sent)
}

async fn desk_with(cfg: impl FnOnce(&mut Config)) -> (desk::Running, TempDb) {
    let db = TempDb::new("relay");
    let mut c = Config::local(pascal_stub().await, db.path());
    c.shutdown_grace = Duration::from_secs(5);
    cfg(&mut c);
    (desk::start(c).await.expect("desk starts"), db)
}

async fn get_relay(base: &str) -> (u16, Value) {
    let r = reqwest::get(format!("{base}/relay")).await.unwrap();
    (r.status().as_u16(), r.json().await.unwrap())
}

#[tokio::test]
async fn http_relay_on_sends_through_the_rpc() {
    let dir = temp_dir("on");
    let sponsor_path = write_key(&dir, "sponsor.json", 1);
    let (rpc, sent) = fake_rpc(2_000_000).await;
    let (running, _db) = desk_with(|c| {
        c.sponsor_keypair_path = sponsor_path.clone();
        c.solana_rpc_url = rpc.clone();
    })
    .await;
    let base = format!("http://{}", running.addr);
    let sp = kp(1).pubkey();
    let (code, body) = get_relay(&base).await;
    assert_eq!(code, 200);
    assert_eq!(
        body,
        json!({"sponsor": sp.to_string(), "max_lamports_per_tx": 10_000_000})
    );

    let http = reqwest::Client::new();
    let user = kp(2);
    let tx = legacy(&sponsored_faucet(&sp, &user.pubkey()), &sp, &[&user]);
    let r = http
        .post(format!("{base}/relay"))
        .json(&json!({"tx": tx}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let body: Value = r.json().await.unwrap();
    let wire = sent.lock().unwrap().pop().expect("sent");
    let sent_tx: VersionedTransaction = wincode::deserialize(&wire).unwrap();
    assert_eq!(body["signature"], sent_tx.signatures[0].to_string());
    assert_ne!(sent_tx.signatures[0], Signature::default());

    // Refusals: the C4 `rejected` body.
    for bad in [json!({"nope": 1}), json!({"tx": "!!"})] {
        let r = http
            .post(format!("{base}/relay"))
            .json(&bad)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 422);
        let b: Value = r.json().await.unwrap();
        assert_eq!(b["status"], "rejected");
    }
    let forbidden = legacy(
        &[
            transfer(&sp, &user.pubkey(), 1_000_000_000),
            memo(b"x", &[user.pubkey()]),
        ],
        &sp,
        &[&user],
    );
    let r = http
        .post(format!("{base}/relay"))
        .json(&json!({"tx": forbidden}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 422);
    assert!(sent.lock().unwrap().is_empty());
    running.join().await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn http_relay_rpc_down_is_unavailable() {
    let dir = temp_dir("down");
    let sponsor_path = write_key(&dir, "sponsor.json", 1);
    let (running, _db) = desk_with(|c| {
        c.sponsor_keypair_path = sponsor_path.clone();
        c.solana_rpc_url = "http://127.0.0.1:9".into();
    })
    .await;
    let base = format!("http://{}", running.addr);
    let (sp, user) = (kp(1).pubkey(), kp(2));
    let tx = legacy(&bet(&sp, &user.pubkey(), 1), &sp, &[&user]);
    let r = reqwest::Client::new()
        .post(format!("{base}/relay"))
        .json(&json!({"tx": tx}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 503);
    let b: Value = r.json().await.unwrap();
    assert_eq!(b["status"], "unavailable");
    assert!(
        !b["reason"].as_str().unwrap().contains("127.0.0.1:9"),
        "{b}"
    );
    running.join().await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn rule7_relay_off_serves_503() {
    let user = kp(2);
    let post = |base: String, sp: Pubkey| {
        let tx = legacy(&bet(&sp, &user.pubkey(), 1), &sp, &[&user]);
        async move {
            reqwest::Client::new()
                .post(format!("{base}/relay"))
                .json(&json!({"tx": tx}))
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    };
    // No sponsor key (Config::local points at a file that does not exist).
    let (running, _db) = desk_with(|_| {}).await;
    let base = format!("http://{}", running.addr);
    let (code, body) = get_relay(&base).await;
    assert_eq!((code, body["status"].as_str()), (503, Some("unavailable")));
    assert!(body["reason"].as_str().unwrap().contains("no sponsor key"));
    assert_eq!(post(base, kp(1).pubkey()).await, 503);
    running.join().await;

    // DESK_RELAY=0 with a key.
    let dir = temp_dir("off");
    let sponsor_path = write_key(&dir, "sponsor.json", 1);
    let (running, _db) = desk_with(|c| {
        c.sponsor_keypair_path = sponsor_path.clone();
        c.relay_enabled = false;
    })
    .await;
    let (code, body) = get_relay(&format!("http://{}", running.addr)).await;
    assert_eq!(code, 503);
    assert!(body["reason"].as_str().unwrap().contains("DESK_RELAY=0"));
    running.join().await;

    // The sponsor key is also the crank's (another file, same key), the quote signer's (same
    // file) or the authority's.
    let crank = write_key(&dir, "crank.json", 1);
    for (name, set) in [("crank", 0usize), ("quote signer", 1), ("authority", 2)] {
        let (running, _db) = desk_with(|c| {
            c.sponsor_keypair_path = sponsor_path.clone();
            match set {
                0 => c.crank_keypair_path = crank.clone(),
                1 => c.quote_signer_keypair_path = sponsor_path.clone(),
                _ => c.authority_keypair_path = crank.clone(),
            }
        })
        .await;
        let base = format!("http://{}", running.addr);
        let (code, body) = get_relay(&base).await;
        assert_eq!(code, 503, "{name}");
        assert!(
            body["reason"]
                .as_str()
                .unwrap()
                .contains("also another desk key"),
            "{name}: {body}"
        );
        assert_eq!(post(base, kp(1).pubkey()).await, 503);
        running.join().await;
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn other_ata_and_system_ids_are_the_programs() {
    // The rules match the same ids the builders use.
    assert_eq!(
        ix::create_ata_idempotent(&kp(1).pubkey(), &kp(2).pubkey(), &sched().usdc_mint())
            .program_id,
        ASSOCIATED_TOKEN_PROGRAM_ID
    );
    assert_eq!(
        MEMO_PROGRAM_ID.to_string(),
        "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr"
    );
}
