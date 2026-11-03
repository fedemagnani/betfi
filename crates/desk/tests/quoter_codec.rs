//! Review gate (2.A): the desk's codec and signer reproduce `fixtures/schedina/quote_vector.json`
//! byte for byte (message hex, its sha256, and the signature hex from the vector's seed), through
//! the same functions `/quote` and `/cashout-quote` use; and `fixtures/desk/quote.json` /
//! `cashout_quote.json` are real signed bytes (D-050) from a deterministic test key.
//!
//! Regenerate the two desk fixtures with `SCHEDINA_WRITE_FIXTURES=1 cargo test -p desk --test
//! quoter_codec`.

mod common;

use std::collections::BTreeMap;
use std::str::FromStr;

use base64::Engine;
use common::repo_root;
use desk::api::types::{CashoutQuoteResponse, QuoteRequest, QuoteResponse};
use desk::catalogue::{Catalogue, nba_moneyline};
use desk::quoter::cashout::sign_cash_out;
use desk::quoter::codec::{self, CashOutMsg, QuoteMsg};
use desk::quoter::quote::{leg_input_of, sign_quote};
use desk::solana::signer::Keypair;
use ed25519_dalek::{Signature, SigningKey, Verifier, VerifyingKey};
use pascal_rs::{Books, Market, OrderBook};
use serde_json::Value;
use solana_pubkey::Pubkey;

fn load(rel: &str) -> Value {
    let p = repo_root().join(rel);
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// The keypair of a 32-byte seed, as a Solana keypair file would hold it.
fn keypair(seed: [u8; 32]) -> Keypair {
    let sk = SigningKey::from_bytes(&seed);
    let mut bytes = seed.to_vec();
    bytes.extend_from_slice(sk.verifying_key().as_bytes());
    Keypair::from_bytes(&bytes).unwrap()
}

fn vector_key(v: &Value) -> Keypair {
    let seed: [u8; 32] = unhex(v["signer_seed_hex"].as_str().unwrap())
        .try_into()
        .unwrap();
    let k = keypair(seed);
    assert_eq!(k.pubkey().to_string(), v["signer_pubkey"].as_str().unwrap());
    k
}

fn verify(signer_b58: &str, msg: &[u8], sig: &[u8]) {
    let pk: [u8; 32] = bs58::decode(signer_b58)
        .into_vec()
        .unwrap()
        .try_into()
        .unwrap();
    let vk = VerifyingKey::from_bytes(&pk).unwrap();
    vk.verify(msg, &Signature::from_slice(sig).unwrap())
        .expect("signature verifies");
}

#[test]
fn reproduces_the_quote_vector() {
    let v = load("fixtures/schedina/quote_vector.json");
    let key = vector_key(&v);
    let program = Pubkey::from_str(v["program_id"].as_str().unwrap()).unwrap();
    let qv = &v["quote"];
    let inputs = &qv["inputs"];

    // The quote as `pricing::quote` would hand it to the signer (only the signed fields matter).
    let legs: Vec<pricing::QuoteLeg> = inputs["legs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            let side = match l["side"].as_u64().unwrap() {
                0 => pricing::Side::Long,
                _ => pricing::Side::Short,
            };
            assert_eq!(
                codec::hex(&codec::sha256(l["symbol"].as_str().unwrap().as_bytes())),
                l["symbol_hash"].as_str().unwrap()
            );
            pricing::QuoteLeg {
                symbol: l["symbol"].as_str().unwrap().to_owned(),
                game_id: l["game_id"].as_str().unwrap().to_owned(),
                side,
                pick: pricing::Pick::Away,
                start_ms: l["start_ts"].as_u64().unwrap() * 1000,
                price_u: l["price_u"].as_u64().unwrap(),
                p_max_u: 0,
                m1_ppm: l["m1_ppm"].as_u64().unwrap() as u32,
                overlaps: true,
                mid_u: 0,
                size: 0,
                hedged: true,
                in_play: false,
            }
        })
        .collect();
    let q = pricing::Quote {
        n: legs.len() as u8,
        stake_u: inputs["stake_u"].as_u64().unwrap(),
        legs,
        odds_fair_u: 0,
        odds_offered_u: inputs["odds_offered_u"].as_u64().unwrap(),
        payout_u: 0,
        fee_u: 0,
        pascal_fees_est_u: 0,
        copy_fee_u: 0,
        slippage_bps: 300,
        min_payout_per_unit_u: 0,
        margin_ppm: 0,
        payout_fair_u: 0,
    };
    let expires = inputs["quote_expires_at"].as_i64().unwrap();
    let s = sign_quote(&q, 0, expires, &program, &key);

    assert_eq!(codec::hex(&s.msg), qv["message_hex"].as_str().unwrap());
    assert_eq!(s.msg.len(), codec::quote_len(2));
    assert_eq!(s.quote_hash, qv["message_sha256_hex"].as_str().unwrap());
    assert_eq!(codec::hex(&s.sig), qv["signature_hex"].as_str().unwrap());
    let b64 = base64::engine::general_purpose::STANDARD;
    assert_eq!(b64.decode(&s.response.quote_msg).unwrap(), s.msg);
    assert_eq!(b64.decode(&s.response.quote_sig).unwrap(), s.sig);
    assert_eq!(
        s.response.quote_signer,
        v["signer_pubkey"].as_str().unwrap()
    );
    verify(&s.response.quote_signer, &s.msg, &s.sig);

    // And back.
    let m = QuoteMsg::decode(&unhex(qv["message_hex"].as_str().unwrap())).unwrap();
    assert_eq!(m.program_id, program.to_bytes());
    assert_eq!(m.legs.len(), 2);
    assert_eq!(m.legs[1].side, codec::SIDE_SHORT);
    assert_eq!(m.legs[1].start_ts, 1_793_230_200);
    assert_eq!(m.quote_expires_at, expires);
    assert_eq!(m.encode(), s.msg);
}

#[test]
fn reproduces_the_cash_out_vector() {
    let v = load("fixtures/schedina/quote_vector.json");
    let key = vector_key(&v);
    let program = Pubkey::from_str(v["program_id"].as_str().unwrap()).unwrap();
    let cv = &v["cash_out"];
    let ticket = Pubkey::from_str(cv["inputs"]["ticket"].as_str().unwrap()).unwrap();
    let (resp, msg, sig) = sign_cash_out(
        &ticket,
        cv["inputs"]["c_u"].as_u64().unwrap(),
        cv["inputs"]["ts"].as_i64().unwrap(),
        &program,
        &key,
    );
    assert_eq!(codec::hex(&msg), cv["message_hex"].as_str().unwrap());
    assert_eq!(msg.len(), codec::CASHOUT_LEN);
    assert_eq!(codec::hex(&sig), cv["signature_hex"].as_str().unwrap());
    assert_eq!(resp.ticket, ticket.to_string());
    verify(&resp.quote_signer, &msg, &sig);
    let m = CashOutMsg::decode(&msg).unwrap();
    assert_eq!(m.ticket, ticket.to_bytes());
    assert_eq!(m.c_u, 12_000_000);
}

// --- fixtures/desk ------------------------------------------------------------------------------

/// The deterministic key of the desk fixtures: the vector's seed (so `quote_signer` is the
/// vector's `signer_pubkey`). Test material only.
fn fixture_key() -> Keypair {
    vector_key(&load("fixtures/schedina/quote_vector.json"))
}

/// The tape's catalogue (`markets_selected.json`) as the catalogue service builds it.
pub fn tape_catalogue() -> Catalogue {
    let v = load("tapes/nba-2026-10-28/markets_selected.json");
    let list: Vec<Market> = serde_json::from_value(v["data"].clone()).unwrap();
    let as_of: u64 = v["state_time_ms"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_793_212_985_011);
    Catalogue::build(
        as_of,
        as_of,
        None,
        list.iter().filter_map(nba_moneyline).collect(),
    )
}

/// `(state_time_ms, books)` of the first line of `sample/books.jsonl`.
fn first_books() -> (u64, BTreeMap<String, OrderBook>) {
    let text = std::fs::read_to_string(repo_root().join("tapes/nba-2026-10-28/sample/books.jsonl"))
        .unwrap();
    let v: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    let t: u64 = v["body"]["state_time_ms"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let books: Books = serde_json::from_value(v["body"]["data"].clone()).unwrap();
    (t, books.books)
}

fn check_or_write(rel: &str, value: &Value) {
    let path = repo_root().join(rel);
    let text = serde_json::to_string_pretty(value).unwrap() + "\n";
    if std::env::var("SCHEDINA_WRITE_FIXTURES").is_ok_and(|v| v == "1") {
        std::fs::write(&path, text).unwrap();
        return;
    }
    let on_disk: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        &on_disk, value,
        "{rel} is stale: SCHEDINA_WRITE_FIXTURES=1 cargo test -p desk --test quoter_codec"
    );
}

/// `fixtures/desk/quote.json` is `fixtures/desk/quote_request.json` priced by the desk's own
/// pipeline (catalogue legs, `pricing::quote`, signer) on the sample tape's first books, with a
/// fixed `quote_expires_at`.
#[test]
fn desk_quote_fixture_is_real() {
    let req: QuoteRequest =
        serde_json::from_value(load("fixtures/desk/quote_request.json")).unwrap();
    let cat = tape_catalogue();
    let inputs: Vec<_> = req
        .legs
        .iter()
        .map(|l| leg_input_of(cat.market(&l.symbol).unwrap(), l.side))
        .collect();
    let (now, books) = first_books();
    let q = pricing::quote(
        &inputs,
        &books,
        req.stake_u,
        now,
        &pricing::Params::default(),
    )
    .unwrap();
    let program = Pubkey::from_str(desk::config::DEFAULT_PROGRAM_ID).unwrap();
    let s = sign_quote(&q, 0, 1_793_213_000, &program, &fixture_key());
    verify(&s.response.quote_signer, &s.msg, &s.sig);
    check_or_write(
        "fixtures/desk/quote.json",
        &serde_json::to_value(&s.response).unwrap(),
    );
    let back: QuoteResponse = serde_json::from_value(load("fixtures/desk/quote.json")).unwrap();
    assert_eq!(back, s.response);
}

/// `fixtures/desk/cashout_quote.json` signs the example ticket of `fixtures/desk/ticket.json`
/// (2.C's) for its listed cash-out value.
#[test]
fn desk_cashout_fixture_is_real() {
    let ticket: Value = load("fixtures/desk/ticket.json");
    let pda = Pubkey::from_str(ticket["ticket"].as_str().unwrap()).unwrap();
    let c_u = ticket["cash_out"]["c_u"].as_u64().unwrap();
    let program = Pubkey::from_str(desk::config::DEFAULT_PROGRAM_ID).unwrap();
    let (resp, msg, sig) = sign_cash_out(&pda, c_u, 1_793_214_000, &program, &fixture_key());
    verify(&resp.quote_signer, &msg, &sig);
    check_or_write(
        "fixtures/desk/cashout_quote.json",
        &serde_json::to_value(&resp).unwrap(),
    );
    let back: CashoutQuoteResponse =
        serde_json::from_value(load("fixtures/desk/cashout_quote.json")).unwrap();
    assert_eq!(back, resp);
}
