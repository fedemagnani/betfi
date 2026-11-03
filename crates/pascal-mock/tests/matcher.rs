//! Matcher and accounting tests on a hand-made book (engine API, no HTTP).
//!
//! The market is the sample tape's `NBA_26OCT28_PHIIND_ML.PHI` (taker fee 2%, maker rebate 25%
//! of it, tick 0.001 / 3 sig figs); its books are replaced with the levels below.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use pascal_mock::engine::{Engine, EngineConfig, required_collateral};
use pascal_mock::requests::{CancelOrder, MassCancel, PlaceOrder};
use pascal_mock::tape::{BookSnapshot, Tape};
use pascal_mock::{MockEvent, Script, tape_owner};
use pascal_rs::{ApiErrorCode, Level, Liquidity, OrderBook, OrderUpdateReason, Side};
use serde_json::{Value, json};

const PHI: &str = "NBA_26OCT28_PHIIND_ML.PHI";
const T0: u64 = 1_793_213_000_000;
const T1: u64 = T0 + 5_000;
const USD: i64 = 1_000_000;

fn key(n: u8) -> String {
    bs58::encode([n; 32]).into_string()
}

fn book(asks: &[(i64, u64)], bids: &[(i64, u64)]) -> OrderBook {
    let lv = |v: &[(i64, u64)]| {
        v.iter()
            .map(|&(price, size)| Level { price, size })
            .collect()
    };
    OrderBook {
        asks: lv(asks),
        bids: lv(bids),
        spec: None,
    }
}

/// The sample tape with PHI's books replaced by `snaps` (one per time).
fn engine_with(snaps: Vec<(u64, OrderBook)>, collateral_u: i64) -> Engine {
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tapes/nba-2026-10-28/sample");
    let mut tape = Tape::load(sample).unwrap();
    tape.books = snaps
        .into_iter()
        .enumerate()
        .map(|(i, (t_ms, b))| BookSnapshot {
            t_ms,
            seq: Some(100 + i as u64),
            books: BTreeMap::from([(PHI.to_owned(), b)]),
        })
        .collect();
    tape.trades.clear();
    let mut e = Engine::new(
        Arc::new(tape),
        EngineConfig {
            start_ms: T0,
            speed: 0.0,
            initial_collateral_u: collateral_u,
            script: Script::default(),
        },
    );
    e.advance();
    e
}

fn default_book() -> OrderBook {
    book(
        &[(700_000, 100), (710_000, 50)],
        &[(690_000, 80), (680_000, 200)],
    )
}

fn engine() -> Engine {
    engine_with(vec![(T0, default_book())], 10_000 * USD)
}

fn req(owner: &str, cid: u64, side: &str, price: &str, size: u64, tif: &str) -> Value {
    json!({
        "client_order_id": cid.to_string(), "symbol": PHI, "side": side, "price": price,
        "size": size.to_string(), "tif": tif,
        "auth": {"client_ts_ms": "1793213000000", "recv_window_ms": "5000", "owner": owner,
                 "signer": key(200), "signature": bs58::encode([0u8; 64]).into_string()},
    })
}

fn place(e: &mut Engine, v: Value) -> Result<pascal_mock::PlaceResult, ApiErrorCode> {
    e.place(&PlaceOrder::parse(&v).unwrap()).map_err(|r| r.code)
}

fn collateral(e: &mut Engine, owner: &str) -> i64 {
    e.account(owner, None, false).collateral_usd.unwrap()
}

#[test]
fn ioc_bid_at_best_ask_fills_at_the_ask() {
    let mut e = engine();
    let a = key(1);
    let r = place(&mut e, req(&a, 1, "BID", "0.700000", 10, "IOC")).unwrap();
    assert_eq!(r.fills.len(), 1);
    let f = &r.fills[0];
    assert_eq!(
        (f.side, f.fill_price, f.fill_size),
        (Side::Bid, 700_000, 10)
    );
    assert_eq!(f.liquidity, Liquidity::Taker);
    assert_eq!(f.counterparty, tape_owner());
    // 10 × 0.7 × 0.3 × 0.02 = 0.042
    assert_eq!(f.fee_usd, 42_000);
    assert_eq!(f.collateral_change_usd, -7_042_000);
    assert_eq!(f.position_size_prev, 0);
    assert_eq!(f.builder_fee_paid_usd, None);
    assert_eq!(r.order.size_filled, 10);
    assert_eq!(r.order.size_remaining, 0);
    assert_eq!(r.order.notional_filled, 7_000_000);
    assert_eq!(r.update_reason, None);

    let s = e.account(&a, None, false);
    assert_eq!(s.collateral_usd, Some(10_000 * USD - 7_042_000));
    assert_eq!(s.positions.len(), 1);
    let p = &s.positions[0];
    assert_eq!(p.size, 10);
    assert_eq!(p.average_entry_price, Some(700_000));
    assert_eq!(p.cumulative_fees_usd, 42_000);
    // mark = median(bid 0.690, ask 0.700, tape mark 0.695)
    assert_eq!(p.mark_price, 695_000);
    assert_eq!(p.unrealized_pnl_usd, -50_000);
    assert!(s.orders.is_empty());
    let life = s.lifetime_stats.unwrap();
    assert_eq!(life.taker.volume_contracts, 10);
    assert_eq!(life.taker.fees_paid_usd, 42_000);

    // The level shrank by what was taken.
    assert_eq!(
        e.book(PHI).asks[0],
        Level {
            price: 700_000,
            size: 90
        }
    );
}

#[test]
fn walks_levels_partial_fill_and_liquidity_is_consumed_until_the_next_snapshot() {
    let mut e = engine_with(
        vec![(T0, default_book()), (T1, default_book())],
        10_000 * USD,
    );
    let a = key(1);
    let r = place(&mut e, req(&a, 1, "BID", "0.710000", 120, "IOC")).unwrap();
    let got: Vec<(i64, u64)> = r
        .fills
        .iter()
        .map(|f| (f.fill_price, f.fill_size))
        .collect();
    assert_eq!(got, vec![(700_000, 100), (710_000, 20)]);
    assert!(r.fills[0].seq < r.fills[1].seq);
    assert_ne!(r.fills[0].trade_id, r.fills[1].trade_id);

    // Only 30 left at 0.710: partial fill, IOC remainder cancelled.
    let r = place(&mut e, req(&a, 2, "BID", "0.710000", 50, "IOC")).unwrap();
    assert_eq!(r.order.size_filled, 30);
    assert_eq!(r.order.size_remaining, 0);
    assert_eq!(
        r.update_reason,
        Some(OrderUpdateReason::IocRemainderCanceled)
    );

    // Nothing left: the same IOC cannot fill the same level twice.
    let r = place(&mut e, req(&a, 3, "BID", "0.710000", 50, "IOC")).unwrap();
    assert!(r.fills.is_empty());
    assert!(e.book(PHI).asks.is_empty());

    // A new snapshot brings fresh liquidity.
    e.set_clock(Some(T1), None, None).unwrap();
    assert_eq!(
        e.book(PHI).asks[0],
        Level {
            price: 700_000,
            size: 100
        }
    );
    let r = place(&mut e, req(&a, 4, "BID", "0.700000", 150, "IOC")).unwrap();
    assert_eq!(r.order.size_filled, 100);
    let s = e.account(&a, None, false);
    assert_eq!(s.positions[0].size, 250);
}

#[test]
fn resting_orders_match_by_price_then_time_with_tape_first_at_equal_price() {
    let mut e = engine();
    let (a, b, c) = (key(1), key(2), key(3));
    let r = place(&mut e, req(&a, 1, "BID", "0.695000", 5, "GTC")).unwrap();
    assert!(r.fills.is_empty());
    assert_eq!(r.order.size_remaining, 5);
    place(&mut e, req(&c, 1, "BID", "0.695000", 5, "GTC")).unwrap();
    assert_eq!(
        e.book(PHI).bids[0],
        Level {
            price: 695_000,
            size: 10
        }
    );
    assert_eq!(e.account(&a, None, false).orders.len(), 1);

    // B sells 7: A (first at 0.695) gets 5, C gets 2.
    let r = place(&mut e, req(&b, 1, "ASK", "0.690000", 7, "IOC")).unwrap();
    let got: Vec<(i64, u64, &str)> = r
        .fills
        .iter()
        .map(|f| (f.fill_price, f.fill_size, f.counterparty.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![(695_000, 5, a.as_str()), (695_000, 2, c.as_str())]
    );
    // Taker fee on 5 @ 0.695: 5 × 0.695 × 0.305 × 0.02 = 0.0211975 → 0.021197.
    assert_eq!(r.fills[0].fee_usd, 21_197);
    // Short 5 locks 5 × 0.305.
    assert_eq!(r.fills[0].collateral_change_usd, -1_525_000 - 21_197);

    let af = &e.account_fills(&a)[0];
    assert_eq!(af.liquidity, Liquidity::Maker);
    // Rebate: 5 × 0.695 × 0.305 × 0.02 × 0.25 = 0.005299375 → −0.005299.
    assert_eq!(af.fee_usd, -5_299);
    assert_eq!(af.collateral_change_usd, -3_475_000 + 5_299);
    assert_eq!(af.counterparty, b);
    assert_eq!(af.trade_id, r.fills[0].trade_id);
    assert_eq!(af.seq, r.fills[0].seq);
    assert_eq!(e.account(&a, None, false).orders.len(), 0);
    let c_orders = e.account(&c, None, false).orders;
    assert_eq!(c_orders[0].size_remaining, 3);
    assert_eq!(c_orders[0].size_filled, 2);

    // At 0.690 the tape level (80) was there first; C's order at 0.690 comes after it.
    place(&mut e, req(&c, 2, "BID", "0.690000", 5, "GTC")).unwrap();
    let r = place(&mut e, req(&b, 2, "ASK", "0.690000", 88, "IOC")).unwrap();
    let got: Vec<(i64, u64, String)> = r
        .fills
        .iter()
        .map(|f| (f.fill_price, f.fill_size, f.counterparty.clone()))
        .collect();
    assert_eq!(
        got,
        vec![
            (695_000, 3, c.clone()),
            (690_000, 80, tape_owner()),
            (690_000, 5, c.clone())
        ]
    );
}

#[test]
fn post_only_tick_and_collateral_rejections() {
    let mut e = engine();
    let a = key(1);
    let mut po = req(&a, 1, "BID", "0.700000", 5, "GTC");
    po["post_only"] = json!(true);
    assert_eq!(place(&mut e, po.clone()), Err(ApiErrorCode::WouldCross));
    po["price"] = json!("0.695000");
    let r = place(&mut e, po).unwrap();
    assert_eq!(r.order.size_remaining, 5);
    assert!(r.order.post_only);

    assert_eq!(
        place(&mut e, req(&a, 2, "BID", "0.695500", 5, "GTC")),
        Err(ApiErrorCode::InvalidPrice)
    );
    assert_eq!(
        place(&mut e, req(&a, 1, "BID", "0.690000", 5, "GTC")),
        Err(ApiErrorCode::DuplicateClientOrderId)
    );
    assert_eq!(
        place(
            &mut e,
            req(&a, 3, "ASK", "0.690000", 5, "GTC").tap_reduce_only()
        ),
        Err(ApiErrorCode::ReduceOnlyRejected)
    );
    let mut unknown = req(&a, 4, "BID", "0.690000", 5, "GTC");
    unknown["symbol"] = json!("NOPE.X");
    assert_eq!(place(&mut e, unknown), Err(ApiErrorCode::SymbolNotFound));

    let mut poor = engine_with(vec![(T0, default_book())], 7 * USD);
    let b = key(2);
    // 10 @ 0.70 needs 7 + fees.
    assert_eq!(
        place(&mut poor, req(&b, 1, "BID", "0.700000", 10, "IOC")),
        Err(ApiErrorCode::InsufficientCollateral)
    );
    assert_eq!(collateral(&mut poor, &b), 7 * USD);
    assert!(place(&mut poor, req(&b, 2, "BID", "0.700000", 9, "IOC")).is_ok());
    assert!(required_collateral(0, Side::Ask, 10, 690_000, 20_000) > 3_100_000);
    // Closing a short frees collateral rather than needing it.
    assert_eq!(required_collateral(-10, Side::Bid, 10, 300_000, 0), 0);
}

trait ReduceOnly {
    fn tap_reduce_only(self) -> Self;
}
impl ReduceOnly for Value {
    fn tap_reduce_only(mut self) -> Self {
        self["reduce_only"] = json!(true);
        self
    }
}

#[test]
fn short_then_close_realises_pnl_and_releases_collateral() {
    let mut e = engine();
    let a = key(1);
    let r = place(&mut e, req(&a, 1, "ASK", "0.690000", 10, "IOC")).unwrap();
    // Short 10 @ 0.69 locks 10 × 0.31; fee 10 × 0.69 × 0.31 × 0.02 = 0.04278.
    assert_eq!(r.fills[0].collateral_change_usd, -3_100_000 - 42_780);
    let s = e.account(&a, None, false);
    assert_eq!(s.positions[0].size, -10);
    assert_eq!(s.collateral_usd, Some(10_000 * USD - 3_142_780));

    let r = place(
        &mut e,
        req(&a, 2, "BID", "0.700000", 10, "IOC").tap_reduce_only(),
    )
    .unwrap();
    let f = &r.fills[0];
    assert_eq!(f.position_size_prev, -10);
    assert_eq!(f.realized_pnl_usd, -100_000);
    assert_eq!(f.collateral_change_usd, 3_000_000 - 42_000);
    let s = e.account(&a, None, false);
    assert!(s.positions.is_empty());
    assert_eq!(
        s.collateral_usd,
        Some(10_000 * USD - 100_000 - 42_780 - 42_000)
    );

    // Flip: long 4 then sell 10 → short 6.
    place(&mut e, req(&a, 3, "BID", "0.700000", 4, "IOC")).unwrap();
    let r = place(&mut e, req(&a, 4, "ASK", "0.680000", 10, "IOC")).unwrap();
    let total: u64 = r.fills.iter().map(|f| f.fill_size).sum();
    assert_eq!(total, 10);
    assert_eq!(e.account(&a, None, false).positions[0].size, -6);
}

#[test]
fn builder_fee_is_charged_and_credited() {
    let mut e = engine();
    let (a, builder) = (key(1), key(9));
    let mut v = req(&a, 1, "BID", "0.700000", 10, "IOC");
    v["builder_info"] = json!({"builder_pubkey": builder, "builder_fee_rate": "0.001000"});
    let r = place(&mut e, v).unwrap();
    let f = &r.fills[0];
    // 10 × 0.7 × 0.3 × 0.001 = 0.0021
    assert_eq!(f.builder_fee_paid_usd, Some(2_100));
    assert_eq!(f.builder_pubkey.as_deref(), Some(builder.as_str()));
    assert_eq!(f.collateral_change_usd, -7_000_000 - 42_000 - 2_100);
    assert_eq!(r.order.builder_pubkey.as_deref(), Some(builder.as_str()));
    assert_eq!(r.order.builder_fee_rate.unwrap().to_string(), "0.001000");
    let s = e.account(&builder, None, false);
    assert_eq!(s.collateral_usd, Some(10_000 * USD + 2_100));
    assert_eq!(s.rewards.unwrap().builder_fees_received_usd, 2_100);
}

#[test]
fn a_new_snapshot_fills_resting_orders_it_crosses() {
    let next = book(
        &[(694_000, 4), (695_000, 3), (700_000, 100)],
        &[(690_000, 80)],
    );
    let mut e = engine_with(vec![(T0, default_book()), (T1, next)], 10_000 * USD);
    let a = key(1);
    place(&mut e, req(&a, 1, "BID", "0.695000", 10, "GTC")).unwrap();
    let mut events = e.subscribe();
    e.set_clock(Some(T1), None, None).unwrap();
    let fills = e.account_fills(&a).to_vec();
    let got: Vec<(i64, u64, Liquidity)> = fills
        .iter()
        .map(|f| (f.fill_price, f.fill_size, f.liquidity))
        .collect();
    assert_eq!(
        got,
        vec![
            (695_000, 4, Liquidity::Maker),
            (695_000, 3, Liquidity::Maker)
        ]
    );
    assert!(fills.iter().all(|f| f.fee_usd < 0));
    let s = e.account(&a, None, false);
    assert_eq!(s.orders[0].size_remaining, 3);
    assert_eq!(s.positions[0].size, 7);
    let b = e.book(PHI);
    assert_eq!(
        b.asks[0],
        Level {
            price: 700_000,
            size: 100
        }
    );
    assert_eq!(
        b.bids[0],
        Level {
            price: 695_000,
            size: 3
        }
    );
    let mut n_fills = 0;
    while let Ok(ev) = events.try_recv() {
        if matches!(ev, MockEvent::Fill { .. }) {
            n_fills += 1;
        }
    }
    assert_eq!(n_fills, 2);
}

#[test]
fn self_trade_cancels_the_resting_order() {
    let mut e = engine();
    let a = key(1);
    place(&mut e, req(&a, 1, "ASK", "0.699000", 5, "GTC")).unwrap();
    let mut events = e.subscribe();
    let r = place(&mut e, req(&a, 2, "BID", "0.700000", 5, "IOC")).unwrap();
    assert_eq!(r.fills.len(), 1);
    assert_eq!(r.fills[0].fill_price, 700_000);
    assert!(e.account(&a, None, false).orders.is_empty());
    let reasons: Vec<Option<OrderUpdateReason>> = std::iter::from_fn(|| events.try_recv().ok())
        .filter_map(|ev| match ev {
            MockEvent::Order { update, .. } => Some(update.update_reason),
            _ => None,
        })
        .collect();
    assert!(reasons.contains(&Some(OrderUpdateReason::SelfTrade)));
}

#[test]
fn cancel_replace_and_mass_cancel() {
    let mut e = engine();
    let a = key(1);
    place(&mut e, req(&a, 77, "BID", "0.680000", 5, "GTC")).unwrap();
    let c = CancelOrder::parse(
        &json!({"client_order_id": "77", "auth": req(&a, 0, "BID", "0.5", 1, "GTC")["auth"]}),
    )
    .unwrap();
    let o = e.cancel(&c).unwrap();
    assert_eq!((o.client_order_id, o.size_remaining), (77, 0));
    assert_eq!(e.cancel(&c).unwrap_err().code, ApiErrorCode::OrderNotFound);

    place(&mut e, req(&a, 1, "BID", "0.680000", 5, "GTC")).unwrap();
    let mut rep = req(&a, 2, "BID", "0.685000", 5, "GTC");
    rep["replace_client_order_id"] = json!("1");
    place(&mut e, rep).unwrap();
    let orders = e.account(&a, None, false).orders;
    assert_eq!(orders.len(), 1);
    assert_eq!((orders[0].client_order_id, orders[0].price), (2, 685_000));
    let mut missing = req(&a, 3, "BID", "0.685000", 5, "GTC");
    missing["replace_client_order_id"] = json!("99");
    assert_eq!(
        place(&mut e, missing),
        Err(ApiErrorCode::ReplacementOrderNotFound)
    );

    place(
        &mut e,
        req(&a, 4, "ASK", "0.720000", 5, "GTT").tap_expiry(T0 + 10_000),
    )
    .unwrap();
    let m = MassCancel::parse(
        &json!({"symbol": PHI, "auth": req(&a, 0, "BID", "0.5", 1, "GTC")["auth"]}),
    )
    .unwrap();
    let closed = e.mass_cancel(&m).unwrap();
    assert_eq!(closed.len(), 2);
    assert!(e.account(&a, None, false).orders.is_empty());
}

trait Expiry {
    fn tap_expiry(self, ms: u64) -> Self;
}
impl Expiry for Value {
    fn tap_expiry(mut self, ms: u64) -> Self {
        self["expires_ts_ms"] = json!(ms.to_string());
        self
    }
}

#[test]
fn gtt_orders_expire_on_the_mock_clock() {
    let mut e = engine();
    let a = key(1);
    place(
        &mut e,
        req(&a, 1, "ASK", "0.720000", 5, "GTT").tap_expiry(T0 + 10_000),
    )
    .unwrap();
    e.set_clock(None, Some(9_999), None).unwrap();
    assert_eq!(e.account(&a, None, false).orders.len(), 1);
    e.set_clock(None, Some(1), None).unwrap();
    assert!(e.account(&a, None, false).orders.is_empty());
    assert!(
        e.set_clock(Some(T0), None, None).is_err(),
        "the clock never goes back"
    );
}

#[test]
fn resolution_settles_positions_cancels_orders_and_closes_the_market() {
    let mut e = engine();
    let (a, b) = (key(1), key(2));
    place(&mut e, req(&a, 1, "BID", "0.700000", 10, "IOC")).unwrap();
    place(&mut e, req(&b, 1, "ASK", "0.690000", 3, "IOC")).unwrap();
    place(&mut e, req(&a, 2, "BID", "0.680000", 5, "GTC")).unwrap();
    let before_b = collateral(&mut e, &b);

    let r = e
        .schedule_resolution(PHI, 1_000_000, pascal_mock::At::Now)
        .unwrap()
        .unwrap();
    assert_eq!(r.price, 1_000_000);
    assert_eq!(r.market_resolution_ts_ms, T0);
    let s = e.account(&a, None, false);
    assert!(s.positions.is_empty());
    assert!(s.orders.is_empty());
    // Paid 7.042 for 10 longs, received 10.
    assert_eq!(s.collateral_usd, Some(10_000 * USD - 7_042_000 + 10 * USD));
    // The short receives 1 − 1 = 0.
    assert_eq!(collateral(&mut e, &b), before_b);
    assert!(e.book(PHI).asks.is_empty() && e.book(PHI).bids.is_empty());
    assert_eq!(
        place(&mut e, req(&a, 3, "BID", "0.700000", 1, "IOC")),
        Err(ApiErrorCode::MarketResolved)
    );
    assert_eq!(e.resolved_markets(24 * 3_600_000, 50).len(), 1);
    assert!(e.live_markets().iter().all(|m| m["symbol"] != PHI));
    let found = e.search(&[PHI.to_owned()], &[], &[]);
    assert_eq!(found[0]["resolution"]["price"], "1.000000");
    assert_eq!(found[0]["mark_price"], "1.000000");
    // Immutable: same price is a no-op, another price a conflict.
    assert!(
        e.schedule_resolution(PHI, 1_000_000, pascal_mock::At::Now)
            .is_ok()
    );
    assert_eq!(
        e.schedule_resolution(PHI, 0, pascal_mock::At::Now)
            .unwrap_err()
            .code,
        ApiErrorCode::Conflict
    );
}

#[test]
fn scheduled_resolution_fires_when_the_clock_passes_it() {
    let mut e = engine();
    let r = e
        .schedule_resolution(PHI, 0, pascal_mock::At::AfterStart(60_000))
        .unwrap();
    assert!(r.is_none());
    e.set_clock(None, Some(59_999), None).unwrap();
    assert!(e.resolved_markets(3_600_000, 50).is_empty());
    e.set_clock(None, Some(1), None).unwrap();
    let res = e.resolved_markets(3_600_000, 50);
    assert_eq!(res[0]["resolution"]["price"], "0.000000");
    assert_eq!(
        res[0]["resolution"]["market_resolution_ts_ms"],
        (T0 + 60_000).to_string()
    );
}

#[test]
fn seqs_increase_and_trade_ids_are_unique() {
    let mut e = engine();
    let mut seqs = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for (i, n) in [1u8, 2, 3].into_iter().enumerate() {
        let r = place(&mut e, req(&key(n), i as u64, "BID", "0.710000", 40, "IOC")).unwrap();
        assert!(r.order.seq < r.order.update_seq);
        for f in r.fills {
            seqs.push(f.seq);
            assert!(ids.insert(f.trade_id));
        }
    }
    assert!(seqs.windows(2).all(|w| w[0] < w[1]));
    assert!(e.envelope().seq >= *seqs.last().unwrap());
}
