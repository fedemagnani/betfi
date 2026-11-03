//! Sponsored transactions (L8, contract L8-2), compiled exactly as the app compiles them
//! (`schedina_tx::sponsor`): the sponsor is the fee payer at static key 0, the user is key 1,
//! there are exactly two signers, the user's metas are read-only where the IDLs say so, a
//! sponsored 5-leg bet fits as v0 with the AMM's lookup table, and a sponsored faucet claim
//! carries the user-signed memo.

use std::path::PathBuf;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use schedina_tx::sponsor::*;
use schedina_tx::swap::*;
use schedina_tx::*;
use serde_json::Value;
use solana_message::VersionedMessage;

fn load(rel: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn user() -> Pubkey {
    "J2xccRtuG43drESLYznHhLhQkLTdfepcKYbiQ9BsJVaf"
        .parse()
        .unwrap()
}

fn sponsor() -> Pubkey {
    Pubkey::new_from_array([77; 32])
}

/// `(writable, signer)` of account `account` of instruction `ix` in an IDL.
fn idl_flags(idl: &str, ix: &str, account: &str) -> (bool, bool) {
    let idl = load(idl);
    let def = idl["instructions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == ix)
        .unwrap();
    let acc = def["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == account)
        .unwrap();
    let flag = |k: &str| acc[k].as_bool().unwrap_or(false);
    (flag("writable"), flag("signer"))
}

/// A `n`-leg quote message (D-050 layout), as `tests/swap.rs` builds it.
fn quote(n: usize, stake_u: u64) -> (SignedMsg, ShownQuote) {
    let legs: Vec<ShownLeg> = (0..n)
        .map(|i| ShownLeg {
            symbol: format!("NBA_{i}"),
            game_id: format!("game-{i}"),
            side: 0,
        })
        .collect();
    let sha = |s: &str| -> [u8; 32] {
        use sha2::Digest;
        sha2::Sha256::digest(s.as_bytes()).into()
    };
    let mut msg = QUOTE_TAG.to_vec();
    msg.extend_from_slice(PROGRAM_ID.as_ref());
    msg.extend_from_slice(&(n as u32).to_le_bytes());
    for l in &legs {
        msg.extend_from_slice(&sha(&l.symbol));
        msg.extend_from_slice(&sha(&l.game_id));
        msg.extend_from_slice(&1_793_300_000i64.to_le_bytes());
        msg.push(l.side);
        msg.extend_from_slice(&500_000u64.to_le_bytes());
        msg.extend_from_slice(&50_000u32.to_le_bytes());
    }
    msg.extend_from_slice(&stake_u.to_le_bytes());
    msg.extend_from_slice(&30_000_000u64.to_le_bytes());
    msg.extend_from_slice(&1_793_213_010i64.to_le_bytes());
    let signer: Pubkey = "GmaDrppBC7P5ARKV8g3djiwP89vz1jLK23V2GBjuAEGB"
        .parse()
        .unwrap();
    let signed = SignedMsg::from_wire(
        &signer.to_string(),
        &STANDARD.encode(&msg),
        &STANDARD.encode([9u8; 64]),
    )
    .unwrap();
    let shown = ShownQuote {
        legs,
        stake_u,
        odds_offered_u: 30_000_000,
        quote_expires_at: 1_793_213_010,
        min_payout_per_unit_u: 29_000_000,
    };
    (signed, shown)
}

/// The lookup table the AMM's set-up writes (desk `swap::ix::lookup_table_addresses`).
fn table() -> Table {
    let mint = amm_pda::mock_mint("BONK");
    let usdc = pda::usdc_mint();
    let pool = amm_pda::pool(&mint);
    (
        Pubkey::new_from_array([42; 32]),
        vec![
            pda::config(),
            pda::escrow_usdc(),
            INSTRUCTIONS_SYSVAR_ID,
            TOKEN_PROGRAM_ID,
            SYSTEM_PROGRAM_ID,
            pda::event_authority(),
            usdc,
            mint,
            pool,
            amm_pda::vault(&pool, &mint),
            amm_pda::vault(&pool, &usdc),
        ],
    )
}

/// The worst case the program's own size test uses: a copy (`copied_from`) of `n` legs.
fn bet_ixs(n: usize, payer: &Pubkey) -> Vec<Instruction> {
    let (signed, shown) = quote(n, 10_000_000);
    let opts = OpenOptions {
        is_tip: false,
        copied_from: Some(Pubkey::new_from_array([5; 32])),
    };
    open_pending_ixs(
        &user(),
        payer,
        &signed,
        &shown,
        NONCE_MAX,
        1_793_213_000,
        opts,
    )
    .unwrap()
    .to_vec()
}

/// Two signers, sponsor at key 0 and writable (it pays), user at key 1 and read-only, zeroed
/// signature slots, and the wire bytes are the message's.
fn assert_sponsored(msg: &VersionedMessage, wire: &[u8]) {
    assert_eq!(check_sponsored(msg, &sponsor(), &user()), Ok(()));
    let h = msg.header();
    assert_eq!(h.num_required_signatures, 2, "exactly two signers");
    let keys = msg.static_account_keys();
    assert_eq!(keys[0], sponsor(), "fee payer is the sponsor");
    assert_eq!(keys[1], user(), "the user is the second signer");
    assert!(!is_readonly_signer(msg, 0), "the sponsor pays: writable");
    assert!(is_readonly_signer(msg, 1), "the user is read-only");
    assert_eq!(wire[0], 2);
    assert!(wire[1..129].iter().all(|b| *b == 0), "slots zeroed");
    assert_eq!(&wire[129..], msg.serialize().as_slice());
}

#[test]
fn the_user_is_read_only_wherever_the_idls_say_so() {
    let schedina = "programs/idl/schedina.json";
    let amm = "programs/idl/mock_amm.json";
    assert_eq!(idl_flags(schedina, "open_pending", "user"), (false, true));
    assert_eq!(idl_flags(schedina, "open_pending", "payer"), (true, true));
    assert_eq!(idl_flags(schedina, "cash_out", "user"), (false, true));
    assert_eq!(idl_flags(amm, "faucet", "caller"), (false, true));
    assert_eq!(idl_flags(amm, "faucet", "payer"), (true, true));
    assert_eq!(idl_flags(amm, "swap", "user"), (false, true));
    // `faucet_usdc`'s caller pays and writes; sponsored, that is the sponsor.
    assert_eq!(idl_flags(schedina, "faucet_usdc", "caller"), (true, true));
}

#[test]
fn sponsored_bets_two_signers_sponsor_first_user_read_only() {
    for n in 1..=4 {
        let ixs = bet_ixs(n, &sponsor());
        let msg = compile_message(&sponsor(), &ixs, &Format::Legacy, [7; 32]).unwrap();
        let wire = compile(&sponsor(), &ixs, &Format::Legacy, [7; 32]).unwrap();
        assert_sponsored(&msg, &wire);
        println!("sponsored {n}-leg copy, legacy: {} bytes", wire.len());
        // Self-paid, the same bet is refused by the sponsored check and passes its own.
        let own = compile_message(&user(), &bet_ixs(n, &user()), &Format::Legacy, [7; 32]).unwrap();
        assert!(check_sponsored(&own, &sponsor(), &user()).is_err());
        assert_eq!(check_self_paid(&own, &user()), Ok(()));
        assert!(check_self_paid(&msg, &user()).is_err());
    }
}

#[test]
fn sponsored_five_leg_bet_fits_as_v0_with_the_table() {
    let ixs = bet_ixs(5, &sponsor());
    // Legacy is over the packet limit (1,300 B in 8.A's program test, no compute budget here).
    let legacy = compile(&sponsor(), &ixs, &Format::Legacy, [7; 32]);
    assert!(
        matches!(legacy, Err(TxError::TooLarge(n)) if n > PACKET_DATA_SIZE),
        "{legacy:?}"
    );
    // v0 with no table is no better.
    assert!(matches!(
        compile(&sponsor(), &ixs, &Format::V0(None), [7; 32]),
        Err(TxError::TooLarge(_))
    ));
    let format = Format::V0(Some(table()));
    let msg = compile_message(&sponsor(), &ixs, &format, [7; 32]).unwrap();
    let wire = compile(&sponsor(), &ixs, &format, [7; 32]).unwrap();
    println!(
        "sponsored 5-leg copy, v0 + lookup table: {} bytes",
        wire.len()
    );
    assert!(wire.len() <= PACKET_DATA_SIZE);
    assert_eq!(wire[129], 0x80, "v0 prefix");
    assert_sponsored(&msg, &wire);
}

#[test]
fn sponsored_swap_bet_fits_with_the_table() {
    let (signed, shown) = quote(5, 49_013_564);
    let mint = amm_pda::mock_mint("BONK");
    let swap = ShownSwap {
        mint,
        amount_in: 5_000_000_000,
        min_out_u: 49_013_564,
        pool: amm_pda::pool(&mint),
    };
    let three = swap_open_pending_ixs(
        &user(),
        &sponsor(),
        &swap,
        &signed,
        &shown,
        NONCE_MAX,
        1_793_213_000,
    )
    .unwrap();
    let format = Format::V0(Some(table()));
    let msg = compile_message(&sponsor(), &three, &format, [7; 32]).unwrap();
    let wire = compile(&sponsor(), &three, &format, [7; 32]).unwrap();
    println!(
        "sponsored 5-leg swap bet, v0 + lookup table: {} bytes",
        wire.len()
    );
    assert_sponsored(&msg, &wire);
    // A first-time user's swap also creates the mock-USDC account, paid by the sponsor.
    let mut ixs = vec![create_ata_idempotent_ix(
        &sponsor(),
        &user(),
        &pda::usdc_mint(),
    )];
    ixs.extend(three);
    let wire = compile(&sponsor(), &ixs, &format, [7; 32]).unwrap();
    println!("... plus create-ATA: {} bytes", wire.len());
    let msg = compile_message(&sponsor(), &ixs, &format, [7; 32]).unwrap();
    assert_sponsored(&msg, &wire);
}

#[test]
fn sponsored_cash_out_sponsor_pays_the_fee_only() {
    let user = user();
    let nonce = 42;
    let ticket = pda::ticket(&user, nonce);
    let mut msg = CASHOUT_TAG.to_vec();
    msg.extend_from_slice(PROGRAM_ID.as_ref());
    msg.extend_from_slice(ticket.as_ref());
    msg.extend_from_slice(&7_000_000u64.to_le_bytes());
    msg.extend_from_slice(&1_793_213_010i64.to_le_bytes());
    assert_eq!(msg.len(), CASHOUT_LEN);
    let signed = SignedMsg::from_wire(
        "GmaDrppBC7P5ARKV8g3djiwP89vz1jLK23V2GBjuAEGB",
        &STANDARD.encode(&msg),
        &STANDARD.encode([9u8; 64]),
    )
    .unwrap();
    let t = CashOutTicket {
        user,
        nonce,
        ticket,
        own_tip: true,
    };
    let ixs = cash_out_ixs(&t, &signed, 7_000_000).unwrap();
    // The sponsor is in no instruction: it signs as fee payer only.
    assert!(
        ixs.iter()
            .all(|ix| ix.accounts.iter().all(|m| m.pubkey != sponsor()))
    );
    let msg = compile_message(&sponsor(), &ixs, &Format::Legacy, [7; 32]).unwrap();
    let wire = compile(&sponsor(), &ixs, &Format::Legacy, [7; 32]).unwrap();
    assert_sponsored(&msg, &wire);
}

#[test]
fn sponsored_usdc_faucet_carries_the_user_signed_memo() {
    let (user, sponsor) = (user(), sponsor());
    let ixs = faucet_ixs(&user, &sponsor, FAUCET_AMOUNT_U);
    assert_eq!(ixs.len(), 3);
    let memo = &ixs[2];
    assert_eq!(memo.program_id, MEMO_PROGRAM_ID);
    assert_eq!(memo.data, FAUCET_MEMO);
    assert_eq!(memo.accounts.len(), 1);
    assert_eq!(memo.accounts[0].pubkey, user);
    assert!(memo.accounts[0].is_signer && !memo.accounts[0].is_writable);
    // The ATA's and the claim's rent come from the sponsor.
    assert_eq!(ixs[0].accounts[0].pubkey, sponsor);
    assert_eq!(ixs[1].accounts[0].pubkey, sponsor);
    let msg = compile_message(&sponsor, &ixs, &Format::Legacy, [7; 32]).unwrap();
    let wire = compile(&sponsor, &ixs, &Format::Legacy, [7; 32]).unwrap();
    assert_sponsored(&msg, &wire);
    // Self-paid: no memo, one signer.
    let own = faucet_ixs(&user, &user, FAUCET_AMOUNT_U);
    assert_eq!(own.len(), 2);
    let msg = compile_message(&user, &own, &Format::Legacy, [7; 32]).unwrap();
    assert_eq!(check_self_paid(&msg, &user), Ok(()));
}

#[test]
fn sponsored_mock_token_faucet_user_read_only() {
    let (user, sponsor) = (user(), sponsor());
    let mint = amm_pda::mock_mint("BONK");
    let ixs = mock_faucet_ixs(&user, &sponsor, &mint, 50_000_000_000);
    let msg = compile_message(&sponsor, &ixs, &Format::Legacy, [7; 32]).unwrap();
    let wire = compile(&sponsor, &ixs, &Format::Legacy, [7; 32]).unwrap();
    assert_sponsored(&msg, &wire);
}

#[test]
fn a_sponsored_check_refuses_other_shapes() {
    let (user, sponsor) = (user(), sponsor());
    let ixs = bet_ixs(2, &sponsor);
    let msg = compile_message(&sponsor, &ixs, &Format::Legacy, [7; 32]).unwrap();
    // Another sponsor, another user, the user as sponsor.
    assert!(check_sponsored(&msg, &Pubkey::new_from_array([1; 32]), &user).is_err());
    assert!(check_sponsored(&msg, &sponsor, &Pubkey::new_from_array([1; 32])).is_err());
    assert!(check_sponsored(&msg, &user, &user).is_err());
    // A user meta made writable (could pay from the user's SOL) is refused.
    let mut writable = ixs.clone();
    writable[1].accounts[0].is_writable = true;
    let msg = compile_message(&sponsor, &writable, &Format::Legacy, [7; 32]).unwrap();
    assert!(check_sponsored(&msg, &sponsor, &user).is_err());
    // A third signer is refused.
    let mut three = ixs;
    three.push(memo_ix(b"x", &[Pubkey::new_from_array([3; 32])]));
    let msg = compile_message(&sponsor, &three, &Format::Legacy, [7; 32]).unwrap();
    assert!(check_sponsored(&msg, &sponsor, &user).is_err());
}

#[test]
fn relay_body_is_standard_base64_of_the_wire() {
    let wire = compile(
        &sponsor(),
        &bet_ixs(1, &sponsor()),
        &Format::Legacy,
        [7; 32],
    )
    .unwrap();
    let b64 = relay_tx_b64(&wire);
    assert_eq!(STANDARD.decode(&b64).unwrap(), wire);
}

/// The desk's frozen relay rules (L8-2) as a fake `POST /relay`, on the unsigned message:
/// two signers with the sponsor as fee payer at key 0, only allowed programs, the sponsor only
/// in payer slots, an ATA created only for the user, compute-unit price at most 10,000.
fn fake_relay(
    msg: &VersionedMessage,
    wire: &[u8],
    sponsor: &Pubkey,
    user: &Pubkey,
) -> Result<(), String> {
    let compute_budget: Pubkey = "ComputeBudget111111111111111111111111111111"
        .parse()
        .unwrap();
    if wire.len() > PACKET_DATA_SIZE || wire[0] != 2 || wire[1..65].iter().any(|b| *b != 0) {
        return Err("bad signature slots".into());
    }
    let keys = msg.static_account_keys();
    if msg.header().num_required_signatures != 2 || keys[0] != *sponsor || keys[1] != *user {
        return Err("bad signers".into());
    }
    for ix in msg.instructions() {
        let program = keys[usize::from(ix.program_id_index)];
        let sponsor_slots: Vec<usize> = ix
            .accounts
            .iter()
            .enumerate()
            .filter(|(_, a)| **a == 0)
            .map(|(i, _)| i)
            .collect();
        let payer_slots: &[usize] = if program == PROGRAM_ID {
            match &ix.data[..8] {
                d if d == OPEN_PENDING_DISC => &[8],
                d if d == FAUCET_USDC_DISC => &[0],
                d if d == CASH_OUT_DISC => &[],
                _ => return Err("schedina instruction not relayed".into()),
            }
        } else if program == AMM_PROGRAM_ID {
            match &ix.data[..8] {
                d if d == FAUCET_DISC => &[8],
                d if d == SWAP_DISC => &[],
                _ => return Err("mock-amm instruction not relayed".into()),
            }
        } else if program == ATA_PROGRAM_ID {
            if ix.data != [1] || ix.accounts.get(2) != Some(&1) {
                return Err("ATA create for someone else".into());
            }
            &[0]
        } else if program == compute_budget {
            if ix.data[0] == 3 && u64::from_le_bytes(ix.data[1..9].try_into().unwrap()) > 10_000 {
                return Err("compute-unit price too high".into());
            }
            &[]
        } else if program == ED25519_PROGRAM_ID || program == MEMO_PROGRAM_ID {
            &[]
        } else {
            return Err(format!("program {program} not relayed"));
        };
        if sponsor_slots.iter().any(|s| !payer_slots.contains(s)) {
            return Err(format!("sponsor outside a payer slot of {program}"));
        }
    }
    Ok(())
}

#[test]
fn every_sponsored_transaction_passes_the_relay_rules() {
    let (user, sponsor) = (user(), sponsor());
    let mint = amm_pda::mock_mint("BONK");
    let (signed, shown) = quote(5, 49_013_564);
    let swap = ShownSwap {
        mint,
        amount_in: 5_000_000_000,
        min_out_u: 49_013_564,
        pool: amm_pda::pool(&mint),
    };
    let mut swap_ixs = vec![create_ata_idempotent_ix(&sponsor, &user, &pda::usdc_mint())];
    swap_ixs.extend(
        swap_open_pending_ixs(
            &user,
            &sponsor,
            &swap,
            &signed,
            &shown,
            NONCE_MAX,
            1_793_213_000,
        )
        .unwrap(),
    );
    let cases: Vec<(&str, Vec<Instruction>, Compile)> = vec![
        ("3-leg bet", bet_ixs(3, &sponsor), Compile::Bet(None)),
        (
            "5-leg bet",
            bet_ixs(5, &sponsor),
            Compile::Bet(Some(table())),
        ),
        ("swap bet", swap_ixs, Compile::V0(Some(table()))),
        (
            "usdc faucet",
            faucet_ixs(&user, &sponsor, FAUCET_AMOUNT_U),
            Compile::Legacy,
        ),
        (
            "mock faucet",
            mock_faucet_ixs(&user, &sponsor, &mint, 1_000).to_vec(),
            Compile::Legacy,
        ),
    ];
    for (name, ixs, how) in cases {
        // As the wallet module sends it: behind the sponsored compute budget when it fits.
        let format = how.format(true);
        let msg = compile_sponsored(&sponsor, ixs.clone(), &format, [7; 32]).unwrap();
        let wire = wire(&msg).unwrap();
        let budgeted = msg.instructions().len() == ixs.len() + 2;
        println!(
            "{name}, sponsored: {} bytes, compute budget {budgeted}",
            wire.len()
        );
        assert_eq!(budgeted, name != "swap bet", "{name}");
        assert_eq!(fake_relay(&msg, &wire, &sponsor, &user), Ok(()), "{name}");
    }
    // A builder that put the sponsor in a user slot (here: the swap's seller) is refused.
    let mut ixs = bet_ixs(1, &sponsor);
    ixs.push(swap_ix(&sponsor, &mint, 1, 1));
    let msg = compile_message(&sponsor, &ixs, &Format::Legacy, [7; 32]).unwrap();
    let wire = compile(&sponsor, &ixs, &Format::Legacy, [7; 32]).unwrap();
    assert!(fake_relay(&msg, &wire, &sponsor, &user).is_err());
}

#[test]
fn a_sponsored_transaction_sets_its_own_compute_budget() {
    let ixs = bet_ixs(3, &sponsor());
    let n = ixs.len();
    let with = with_compute_budget(ixs.clone());
    assert_eq!(with.len(), n + 2);
    assert!(
        with[..2]
            .iter()
            .all(|ix| ix.program_id == COMPUTE_BUDGET_PROGRAM_ID && ix.accounts.is_empty())
    );
    let limit = u32::from_le_bytes(with[0].data[1..].try_into().unwrap());
    assert_eq!(
        (with[0].data[0], limit),
        (2, 200_000 * n as u32),
        "the runtime's default limit"
    );
    let price = u64::from_le_bytes(with[1].data[1..].try_into().unwrap());
    assert_eq!((with[1].data[0], price), (3, SPONSORED_CU_PRICE));
    assert!(SPONSORED_CU_PRICE <= 10_000, "under the relay's cap");
    assert_eq!(&with[2..], &ixs[..], "the instructions follow in order");
    // A budget already set is kept as it is; the limit never passes the runtime's ceiling.
    assert_eq!(with_compute_budget(with.clone()), with);
    let many = with_compute_budget(vec![ixs[0].clone(); 9]);
    assert_eq!(
        u32::from_le_bytes(many[0].data[1..].try_into().unwrap()),
        1_400_000
    );
}

#[test]
fn compile_format_follows_the_payer() {
    let t = Some(table());
    assert_eq!(Compile::Legacy.format(true), Format::Legacy);
    assert_eq!(Compile::Legacy.format(false), Format::Legacy);
    // A bet is legacy self-paid (as before L8); sponsored, v0 with the table when there is one.
    assert_eq!(Compile::Bet(t.clone()).format(true), Format::V0(t.clone()));
    assert_eq!(Compile::Bet(t.clone()).format(false), Format::Legacy);
    assert_eq!(Compile::Bet(None).format(true), Format::Legacy);
    // The swap bet is always v0.
    assert_eq!(Compile::V0(t.clone()).format(false), Format::V0(t.clone()));
    assert_eq!(Compile::V0(None).format(true), Format::V0(None));
}

#[test]
fn the_wallet_pays_only_when_nothing_was_sent_and_it_has_sol() {
    use NotSponsored::*;
    for why in [Unavailable, TooLarge, CannotCoSign] {
        assert!(self_pay_after(why, SELF_PAY_MIN_LAMPORTS), "{why:?}");
        assert!(!self_pay_after(why, SELF_PAY_MIN_LAMPORTS - 1), "{why:?}");
        assert!(!self_pay_after(why, 0), "{why:?}");
    }
    // A refusal stands; no clean answer may mean it was sent: never sent again.
    for why in [Refused, NoAnswer] {
        assert!(!self_pay_after(why, u64::MAX), "{why:?}");
    }
}

#[test]
fn a_self_paid_five_leg_bet_still_fits_legacy() {
    // What the app falls back to when a sponsored 5-leg bet has no lookup table and the wallet
    // holds SOL.
    let ixs = bet_ixs(5, &user());
    let wire = compile(&user(), &ixs, &Format::Legacy, [7; 32]).unwrap();
    println!("self-paid 5-leg copy, legacy: {} bytes", wire.len());
    let msg = compile_message(&user(), &ixs, &Format::Legacy, [7; 32]).unwrap();
    assert_eq!(check_self_paid(&msg, &user()), Ok(()));
}

#[test]
fn a_sponsored_swap_copy_pays_stake_and_fee_from_the_swap() {
    let (user, sponsor) = (user(), sponsor());
    let min_out = 49_013_564;
    let stake = copy_stake_for(min_out);
    assert!(stake + copy_fee_u(stake) <= min_out, "stake and fee fit");
    assert!(
        stake + 1 + copy_fee_u(stake + 1) > min_out,
        "the largest that fits"
    );
    assert_eq!(copy_fee_u(10_000_000), 200_000, "ceil(2%), as the program");
    let mint = amm_pda::mock_mint("BONK");
    let swap = ShownSwap {
        mint,
        amount_in: 5_000_000_000,
        min_out_u: min_out,
        pool: amm_pda::pool(&mint),
    };
    let copy = OpenOptions {
        is_tip: false,
        copied_from: Some(Pubkey::new_from_array([5; 32])),
    };
    let (signed, shown) = quote(5, stake);
    let three = swap_open_pending_ixs_with(
        &user,
        &sponsor,
        &swap,
        &signed,
        &shown,
        NONCE_MAX,
        1_793_213_000,
        copy,
    )
    .unwrap();
    // A copy staking the whole swap (no room for its fee), or a tip, is refused.
    let (signed_all, shown_all) = quote(5, min_out);
    let build = |signed: &SignedMsg, shown: &ShownQuote, opts| {
        swap_open_pending_ixs_with(
            &user,
            &sponsor,
            &swap,
            signed,
            shown,
            1,
            1_793_213_000,
            opts,
        )
    };
    assert!(build(&signed_all, &shown_all, copy).is_err());
    let tip = OpenOptions {
        is_tip: true,
        copied_from: None,
    };
    assert!(build(&signed_all, &shown_all, tip).is_err());
    assert!(build(&signed_all, &shown_all, OpenOptions::default()).is_ok());

    // Sponsored, as the app compiles it: 1–4 legs carry the compute budget; 5 legs fit only
    // without it (1,199 B; 1,251 B with), and a first-time user's 5-leg copy not at all.
    let format = Format::V0(Some(table()));
    for n in 1..=5 {
        let (signed, shown) = quote(n, stake);
        let ixs = swap_open_pending_ixs_with(
            &user,
            &sponsor,
            &swap,
            &signed,
            &shown,
            NONCE_MAX,
            1_793_213_000,
            copy,
        )
        .unwrap()
        .to_vec();
        let msg = compile_sponsored(&sponsor, ixs.clone(), &format, [7; 32]).unwrap();
        let w = wire(&msg).unwrap();
        let budgeted = msg.instructions().len() == ixs.len() + 2;
        println!(
            "{n}-leg swap copy, sponsored: {} bytes, compute budget {budgeted}",
            w.len()
        );
        assert_eq!(budgeted, n < 5, "{n} legs");
        assert_eq!(fake_relay(&msg, &w, &sponsor, &user), Ok(()), "{n} legs");
    }
    let mut first_time = vec![create_ata_idempotent_ix(&sponsor, &user, &pda::usdc_mint())];
    first_time.extend(three);
    let msg = compile_sponsored(&sponsor, first_time, &format, [7; 32]).unwrap();
    assert!(matches!(wire(&msg), Err(TxError::TooLarge(_))));
}
