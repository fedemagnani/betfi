//! Every recorded response in `fixtures/pascal/` decodes into the typed models.
//!
//! Beyond decoding, each typed value is serialised back and compared with the raw JSON
//! (ignoring `null` vs absent). That proves no field present in a fixture is silently dropped:
//! an undocumented field Pascal starts sending fails this test until it is modelled.

use std::collections::BTreeSet;
use std::path::PathBuf;

use pascal_rs::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pascal")
}

fn load(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// Removes `null` members so `None` (serialised as null) equals an absent key.
fn strip_nulls(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k.clone(), strip_nulls(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(strip_nulls).collect()),
        other => other.clone(),
    }
}

fn assert_round_trips<T: Serialize>(name: &str, typed: &T, raw: &Value) {
    let back = serde_json::to_value(typed).unwrap();
    let (back, raw) = (strip_nulls(&back), strip_nulls(raw));
    if back != raw {
        // Find the first differing path for a readable failure.
        fn diff(a: &Value, b: &Value, path: String) -> Option<String> {
            match (a, b) {
                (Value::Object(x), Value::Object(y)) => {
                    for k in x.keys().chain(y.keys()) {
                        match (x.get(k), y.get(k)) {
                            (Some(p), Some(q)) => {
                                if let Some(d) = diff(p, q, format!("{path}.{k}")) {
                                    return Some(d);
                                }
                            }
                            (p, q) => return Some(format!("{path}.{k}: typed={p:?} raw={q:?}")),
                        }
                    }
                    None
                }
                (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x
                    .iter()
                    .zip(y)
                    .enumerate()
                    .find_map(|(i, (p, q))| diff(p, q, format!("{path}[{i}]"))),
                _ if a == b => None,
                _ => Some(format!("{path}: typed={a} raw={b}")),
            }
        }
        panic!(
            "{name}: typed value does not round-trip: {}",
            diff(&back, &raw, String::new()).unwrap()
        );
    }
}

/// Decodes a REST fixture and checks the round trip of `data`.
fn rest<T: DeserializeOwned + Serialize>(name: &str) -> Response<T> {
    let resp =
        decode_response::<T>(200, &load(name), name).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert!(resp.seq.is_some(), "{name}: success envelope has seq");
    assert!(
        resp.state_time_ms.is_some(),
        "{name}: success envelope has state_time_ms"
    );
    assert!(
        resp.server_time_ms.is_some(),
        "{name}: success envelope has server_time_ms"
    );
    assert_round_trips(name, &resp.data, resp.raw_data());
    resp
}

fn ws(name: &str) -> WsFrame {
    let text = String::from_utf8(load(name)).unwrap();
    WsFrame::parse(&text).unwrap_or_else(|e| panic!("{name}: {e}"))
}

const REST_FILES: &[&str] = &[
    "rest_accounts_owner_bio.json",
    "rest_accounts_owner_bio_null.json",
    "rest_accounts_owner_builder_stats.json",
    "rest_accounts_owner_deposit_address.json",
    "rest_accounts_owner_fills.json",
    "rest_accounts_owner_history.json",
    "rest_accounts_owner_history_small.json",
    "rest_accounts_owner_mm.json",
    "rest_accounts_owner_small.json",
    "rest_accounts_owner_trading_keys.json",
    "rest_accounts_owner_transfers.json",
    "rest_books.json",
    "rest_error_invalid_request.json",
    "rest_markets.json",
    "rest_markets_resolved.json",
    "rest_markets_search.json",
    "rest_trades_page1.json",
    "rest_trades_page2.json",
    "rest_trading_keys_pubkey_accounts.json",
    "rest_usernames.json",
    "rest_v2_accounts_owner_position_instances.json",
];

const WS_FILES: &[&str] = &[
    "ws_account_snapshot.json",
    "ws_account_snapshot_with_orders.TRIMMED.json",
    "ws_account_update.json",
    "ws_account_update_with_orders.json",
    "ws_all_trades_snapshot.json",
    "ws_all_trades_update.json",
    "ws_market_update_snapshot.json",
];

/// JSON files in the fixture directory that are not recorded responses.
const NOT_RESPONSES: &[&str] = &["record_config.json"];

#[test]
fn every_fixture_file_is_covered() {
    let on_disk: BTreeSet<String> = std::fs::read_dir(dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".json") && !NOT_RESPONSES.contains(&n.as_str()))
        .collect();
    let covered: BTreeSet<String> = REST_FILES
        .iter()
        .chain(WS_FILES)
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        on_disk, covered,
        "a fixture was added or removed: cover it here"
    );
}

#[test]
fn markets() {
    let r = rest::<Vec<Market>>("rest_markets.json");
    assert_eq!(r.data.len(), 50);
    assert_eq!(r.seq, Some(12_110_239_487));
    assert_eq!(r.raw_items().len(), 50);
    let tx = r
        .data
        .iter()
        .find(|m| m.symbol == "TX_GOV_2026.REP")
        .unwrap();
    assert_eq!(tx.event(), "TX_GOV_2026");
    assert!(matches!(
        tx.display_attributes.reference,
        Some(Reference::Polymarket { .. })
    ));
    let no_topic: Vec<_> = r
        .data
        .iter()
        .filter(|m| m.topic_id().is_none())
        .map(|m| m.symbol.as_str())
        .collect();
    assert_eq!(
        no_topic,
        [
            "AI_BEST_MODEL_26DEC31.GOOGLE",
            "AI_BEST_MODEL_26OCT31.ANTHROPIC"
        ]
    );
    let cs = r
        .data
        .iter()
        .find(|m| m.symbol == "CS2_26OCT28_MGLZNEM_M1ML.MGLZ")
        .unwrap();
    match &cs.display_attributes.market_type {
        Some(MarketType::Sports(s)) => {
            assert_eq!(s.sport, Sport::CounterStrike);
            assert_eq!(s.series_format, Some(BestOfSeriesFormat::BestOfThree));
            assert_eq!(
                s.market,
                SportsMarket::MapWinner {
                    map_number: 1,
                    outcome: Outcome::Home
                }
            );
        }
        other => panic!("{other:?}"),
    }
    let jobs = r
        .data
        .iter()
        .find(|m| m.symbol == "US_JOBS_26OCT_SCL.M25_100K")
        .unwrap();
    let scalar = jobs.display_attributes.scalar.as_ref().unwrap();
    assert_eq!((scalar.lower, scalar.upper), (-25, 100));
    assert_eq!(jobs.mark_price, Some(659_500));
    let dota = r
        .data
        .iter()
        .find(|m| m.symbol == "DOTA_26OCT28_TYAI_M2ML.TY")
        .unwrap();
    assert_eq!(dota.mark_price, None);
    // The lenient decoder agrees with the strict one on a clean list.
    let lenient = decode_lenient_response::<Market>(200, &load("rest_markets.json"), "m").unwrap();
    assert!(
        lenient
            .data
            .items
            .iter()
            .enumerate()
            .all(|(i, (idx, _))| i == *idx)
    );
    assert!(lenient.data.failures.is_empty());
    assert_eq!(lenient.data.into_values(), r.data);
}

#[test]
fn lenient_reports_bad_items() {
    let mut raw: Value = serde_json::from_slice(&load("rest_markets.json")).unwrap();
    raw["data"][3]["display_attributes"]["market_type"] =
        serde_json::json!({"kind": "sports", "sport": "curling"});
    let body = serde_json::to_vec(&raw).unwrap();
    assert!(matches!(
        decode_response::<Vec<Market>>(200, &body, "m"),
        Err(Error::Decode { .. })
    ));
    let lenient = decode_lenient_response::<Market>(200, &body, "m").unwrap();
    assert_eq!(lenient.data.items.len(), 49);
    assert_eq!(lenient.data.failures.len(), 1);
    assert_eq!(lenient.data.failures[0].index, 3);
    assert!(lenient.data.failures[0].error.contains("curling"));
    // Every decoded item still points at its own raw JSON, also after the failed one.
    let raw_items = lenient.raw_items();
    assert_eq!(raw_items.len(), 50);
    assert_eq!(lenient.data.items[3].0, 4);
    for (index, market) in &lenient.data.items {
        assert_eq!(raw_items[*index]["symbol"], market.symbol.as_str());
    }
}

#[test]
fn resolved_and_search() {
    let r = rest::<Vec<Market>>("rest_markets_resolved.json");
    assert_eq!(r.data.len(), 50);
    assert!(r.data.iter().all(|m| m.resolution.is_some()));
    let first = &r.data[0];
    assert_eq!(first.symbol, "US_IRAN_CFR_THRU.26OCT24");
    let res = first.resolution.as_ref().unwrap();
    assert_eq!(res.price, 1_000_000);
    assert_eq!(res.seq, 12_109_453_024);
    assert_eq!(
        first
            .stats
            .as_ref()
            .unwrap()
            .all_time
            .taker_buy_volume_notional,
        5_615_635_000
    );
    let s = rest::<Vec<Market>>("rest_markets_search.json");
    assert_eq!(s.data.len(), 3);
    assert_eq!(s.data.iter().filter(|m| m.resolution.is_some()).count(), 2);
}

#[test]
fn trades_pages() {
    let p1 = rest::<Page<PublicTrade>>("rest_trades_page1.json");
    let p2 = rest::<Page<PublicTrade>>("rest_trades_page2.json");
    assert_eq!(p1.data.items.len(), 50);
    assert_eq!(
        p1.data.next_cursor.as_deref(),
        Some("00000000012110785211:0000000006")
    );
    let t = &p1.data.items[0];
    assert_eq!(t.trade_id, 151_785);
    assert_eq!(t.taker_side, Side::Bid);
    assert_eq!(t.price, 627_000);
    assert_eq!(t.maker_position_size_prev, Some(-22));
    // Page 2 is strictly older than page 1.
    let last1 = p1.data.items.last().unwrap();
    let first2 = &p2.data.items[0];
    assert!(first2.seq <= last1.seq && first2.trade_id < last1.trade_id);
    assert!(p1.data.items.windows(2).all(|w| w[0].seq >= w[1].seq));
}

#[test]
fn books() {
    let r = rest::<Books>("rest_books.json");
    assert_eq!(r.data.books.len(), 13);
    let b = &r.data.books["AK_SEN_2026.SULLIVAN"];
    assert_eq!(
        b.asks[0],
        Level {
            price: 260_000,
            size: 13_491
        }
    );
    assert!(r.data.books.values().all(|b| b.spec.is_none()));
    assert_eq!(r.data.books["EPL_2027_PTS_DED.MCI"].asks.len(), 1);
}

#[test]
fn accounts() {
    let mm = rest::<AccountState>("rest_accounts_owner_mm.json");
    assert_eq!(mm.data.orders.len(), 35);
    assert_eq!(mm.data.positions.len(), 19);
    let lt = mm.data.lifetime_stats.as_ref().unwrap();
    assert_eq!(lt.maker.volume_notional_usd, 363_038_535_000);
    assert_eq!(lt.maker.fees_paid_usd, -760_488_584);
    assert_eq!(mm.data.orders[0].tif, TimeInForce::Gtc);
    assert_eq!(mm.data.orders[0].order_type, OrderType::Limit);

    let small = rest::<AccountState>("rest_accounts_owner_small.json");
    assert_eq!(small.data.collateral_usd, Some(4_931_023));
    let tw = small.data.twitter_account_connection.as_ref().unwrap();
    assert_eq!(tw.twitter_user_id, 1_446_064_645_207_384_064);
    let p = &small.data.positions[0];
    assert_eq!((p.size, p.average_entry_price), (3, Some(301_000)));
    assert_eq!(p.remaining_entry_notional_usd, Some(903_000));
    assert!(small.data.fills.is_none());
}

#[test]
fn history() {
    for (name, all_time) in [
        ("rest_accounts_owner_history.json", 57),
        ("rest_accounts_owner_history_small.json", 123),
    ] {
        let r = rest::<AccountHistory>(name);
        let tfs: Vec<_> = r.data.items.iter().map(|i| i.timeframe).collect();
        assert_eq!(tfs, Timeframe::ALL);
        assert_eq!(
            r.data
                .timeframe(Timeframe::AllTime)
                .unwrap()
                .trading_pnl_history
                .len(),
            all_time
        );
        // At every point: total = trading + referral + builder + user rewards.
        for item in &r.data.items {
            let d = &item.data;
            for i in 0..d.total_pnl_history.len() {
                let sum = d.trading_pnl_history[i].value
                    + d.referral_rewards_received_history[i].value
                    + d.builder_fees_received_history[i].value
                    + d.user_rewards_received_history[i].value;
                assert_eq!(
                    sum, d.total_pnl_history[i].value,
                    "{name} {:?} #{i}",
                    item.timeframe
                );
            }
        }
    }
    let r = rest::<AccountHistory>("rest_accounts_owner_history.json");
    let day = r.data.timeframe(Timeframe::Day).unwrap();
    assert_eq!(day.trading_pnl_history.len(), 49);
    assert_eq!(
        day.trading_pnl_history[0],
        HistoryPoint {
            ts_ms: 1_793_059_200_000,
            value: -6_381_119_333
        }
    );
    // Last point is the 2026-10-28 00:00 UTC boundary in every timeframe.
    for tf in Timeframe::ALL {
        let s = r.data.timeframe(tf).unwrap();
        assert_eq!(
            s.account_value_history.last().unwrap().ts_ms,
            1_793_145_600_000
        );
    }
}

#[test]
fn paginated_account_lists() {
    let pi = rest::<Page<PositionHistory>>("rest_v2_accounts_owner_position_instances.json");
    assert_eq!(pi.data.items.len(), 50);
    assert!(pi.data.next_cursor.is_some());
    assert_eq!(
        pi.data
            .items
            .iter()
            .filter(|p| p.held_until_resolution)
            .count(),
        47
    );
    assert_eq!(pi.data.items[0].realized_pnl_usd, -85_120_000);

    let fills = rest::<Page<Fill>>("rest_accounts_owner_fills.json");
    let kinds: BTreeSet<_> = fills
        .data
        .items
        .iter()
        .map(|f| (f.liquidity == Liquidity::Maker, f.side == Side::Bid))
        .collect();
    assert_eq!(kinds.len(), 4, "MAKER/TAKER x BID/ASK all present");
    assert_eq!(fills.data.items[0].client_order_id, 60_281_615_273_667_505);
    assert!(fills.data.items.iter().all(|f| f.builder_pubkey.is_none()));

    let tr = rest::<Page<Transfer>>("rest_accounts_owner_transfers.json");
    let reasons: BTreeSet<_> = tr
        .data
        .items
        .iter()
        .filter_map(|t| t.user_reward_reason.as_deref())
        .collect();
    assert_eq!(
        reasons,
        BTreeSet::from([
            "MIDTERMS_2026_REFERRAL_REWARD",
            "MIDTERMS_2026_TRADING_REWARD"
        ])
    );
    let types: BTreeSet<_> = tr
        .data
        .items
        .iter()
        .map(|t| format!("{:?}", t.transfer_type))
        .collect();
    assert_eq!(types.len(), 3);
    assert!(
        tr.data
            .items
            .iter()
            .filter(|t| t.transfer_type == TransferType::Withdrawal)
            .all(|t| t.net_usd < 0 && t.chain_seq.is_some())
    );
}

#[test]
fn small_endpoints() {
    let bio = rest::<AccountBio>("rest_accounts_owner_bio.json");
    assert_eq!(bio.data.account_bio.as_deref(), Some("PASCAL SUPERCYCLE"));
    let none = rest::<AccountBio>("rest_accounts_owner_bio_null.json");
    assert_eq!(none.data.account_bio, None);
    let bs = rest::<BuilderStats>("rest_accounts_owner_builder_stats.json");
    assert_eq!(bs.data.builder_volume_contracts, 0);
    let da = rest::<DepositAddress>("rest_accounts_owner_deposit_address.json");
    assert!(da.data.registered && da.data.deposit_address.is_some());
    let keys = rest::<Vec<TradingKey>>("rest_accounts_owner_trading_keys.json");
    assert_eq!(keys.data[0].name, "pmx-keeper");
    let owners = rest::<Vec<String>>("rest_trading_keys_pubkey_accounts.json");
    assert_eq!(owners.data.len(), 23);
    let names = rest::<Usernames>("rest_usernames.json");
    assert!(names.data.usernames.is_empty());
}

#[test]
fn error_envelope() {
    let body = load("rest_error_invalid_request.json");
    let err = decode_response::<Page<PublicTrade>>(422, &body, "trades").unwrap_err();
    match &err {
        Error::Api(api) => {
            assert_eq!(api.http_status, Some(422));
            assert_eq!(api.code, "INVALID_REQUEST");
            assert_eq!(api.kind(), Some(ApiErrorCode::InvalidRequest));
            assert!(api.message.contains("max=500"));
            assert_eq!(api.seq, None);
            assert_eq!(api.server_time_ms, Some(1_793_147_361_138));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(err.api_code(), Some("INVALID_REQUEST"));
    // An unknown future code is kept verbatim.
    let body = br#"{"status":"error","data":{"code":"NEW_CODE","message":"m"}}"#;
    let err = decode_response::<ServerTime>(500, body, "x").unwrap_err();
    assert_eq!(err.api_code(), Some("NEW_CODE"));
    // A non-envelope error body (e.g. a proxy page) keeps the HTTP status.
    let err = decode_response::<ServerTime>(429, b"<html>slow down</html>", "x").unwrap_err();
    assert_eq!(err.http_status(), Some(429));
}

#[test]
fn ws_all_trades() {
    let snap = ws("ws_all_trades_snapshot.json");
    assert_eq!(snap.seq, Some(12_117_113_181));
    match &snap.message {
        WsMessage::AllTrades { kind, trades } => {
            assert_eq!(*kind, FrameKind::Snapshot);
            assert_eq!(trades.len(), 100);
            assert!(
                trades.windows(2).all(|w| w[0].seq <= w[1].seq),
                "oldest first"
            );
            assert_round_trips("ws_all_trades_snapshot.json", trades, &snap.raw["data"]);
        }
        other => panic!("{other:?}"),
    }
    let upd = ws("ws_all_trades_update.json");
    match &upd.message {
        WsMessage::AllTrades { kind, trades } => {
            assert_eq!(*kind, FrameKind::Update);
            assert_eq!(trades.len(), 2);
            assert_round_trips("ws_all_trades_update.json", trades, &upd.raw["data"]);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn ws_account() {
    const BIG_MM: &str = "7ATdPM8uGMu3qiRSCpc6hUrSZ5VxFSUyYLfRipABG5bC";
    let snap = ws("ws_account_snapshot.json");
    match &snap.message {
        WsMessage::Account {
            owner,
            exclude_orders,
            kind,
            state,
        } => {
            assert_eq!(owner, BIG_MM);
            assert!(*exclude_orders, "echoed by the server");
            assert_eq!(*kind, FrameKind::Snapshot);
            assert!(state.orders.is_empty(), "exclude_orders");
            assert_eq!(state.positions.len(), 587);
            assert!(state.rewards.is_some() && state.lifetime_stats.is_some());
            assert_round_trips("ws_account_snapshot.json", state, &snap.raw["data"]);
        }
        other => panic!("{other:?}"),
    }
    let upd = ws("ws_account_update.json");
    match &upd.message {
        WsMessage::Account { kind, state, .. } => {
            assert_eq!(*kind, FrameKind::Update);
            assert_eq!(state.fills.as_ref().map(Vec::len), Some(1));
            assert!(state.collateral_usd.is_some());
            assert!(state.rewards.is_none());
            assert_round_trips("ws_account_update.json", state, &upd.raw["data"]);
        }
        other => panic!("{other:?}"),
    }
    // The channel descriptor echoed in the frame serialises back the same way.
    let ch: Channel = serde_json::from_value(serde_json::json!({
        "channel": snap.raw["channel"], "owner": snap.raw["owner"], "exclude_orders": snap.raw["exclude_orders"]
    }))
    .unwrap();
    assert_eq!(ch, Channel::account(BIG_MM));
}

#[test]
fn ws_account_with_orders() {
    let snap = ws("ws_account_snapshot_with_orders.TRIMMED.json");
    match &snap.message {
        WsMessage::Account {
            kind,
            state,
            exclude_orders,
            ..
        } => {
            assert!(!exclude_orders);
            assert_eq!(*kind, FrameKind::Snapshot);
            assert_eq!((state.orders.len(), state.positions.len()), (50, 50));
            assert_round_trips(
                "ws_account_snapshot_with_orders.TRIMMED.json",
                state,
                &snap.raw["data"],
            );
        }
        other => panic!("{other:?}"),
    }
    let upd = ws("ws_account_update_with_orders.json");
    match &upd.message {
        WsMessage::Account { kind, state, .. } => {
            assert_eq!(*kind, FrameKind::Update);
            let ou = state.order_updates.as_ref().unwrap();
            assert_eq!((ou.len(), state.orders.len()), (90, 90));
            assert!(ou.iter().zip(&state.orders).all(|(u, o)| u.order == *o));
            assert_round_trips(
                "ws_account_update_with_orders.json",
                state,
                &upd.raw["data"],
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn ws_market_update() {
    let snap = ws("ws_market_update_snapshot.json");
    match &snap.message {
        WsMessage::MarketUpdate { kind, updates } => {
            assert_eq!(*kind, FrameKind::Snapshot);
            assert!(updates.items.is_empty() && updates.failures.is_empty());
        }
        other => panic!("{other:?}"),
    }
    // The docs' update example, with a market from the REST fixture as payload.
    let raw: Value = serde_json::from_slice(&load("rest_markets.json")).unwrap();
    let frame = serde_json::json!({
        "channel": "market_update", "type": "update",
        "data": [{"update_type": "resolved", "market": raw["data"][0]}],
        "seq": "1", "state_time_ms": "2", "server_time_ms": "3"
    });
    match WsFrame::from_value(frame).unwrap().message {
        WsMessage::MarketUpdate { kind, updates } => {
            assert_eq!(kind, FrameKind::Update);
            assert_eq!(updates.items[0].0, 0);
            assert_eq!(updates.items[0].1.update_type, MarketUpdateType::Resolved);
        }
        other => panic!("{other:?}"),
    }
}
