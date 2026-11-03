//! The hedger's signing identity and the order wire (spec section 1, "Order conventions" and
//! "Signing"): build an order, sign it with the hedger's trading key through
//! `pascal_rs::signing`, log it as its signed payload, and send it through the desk's one Pascal
//! handle.
//!
//! - Long the outcome: `BID` at `p`; short the outcome: `ASK` at `1 − p` ([`order_for`]).
//! - Every order carries the builder code (`PASCAL_BUILDER_PUBKEY`, fee rate 0.001, D-003).
//! - `client_order_id` is the desk-wide counter ([`crate::AppState::next_client_order_id`]).
//! - Every signed order is logged as one JSON line (target `schedina::signed_order`) with the
//!   request, the permit hex and the signature: what mode M shows on screen.

use std::time::Instant;

use pascal_rs::signing::{Pubkey, base58, decode_pubkey};
use pascal_rs::{
    AuthParams, BuilderInfo, CancelOrder, CancelOrderRequest, MassCancel, MassCancelRequest,
    MassCancelled, Order, PermitError, PlaceOrder, PlaceOrderRequest, PlacedOrder, Signed,
    TimeInForce, TradingSigner, WriteResult,
};

use crate::api::types::{OrderSide, Side, Venue};
use crate::config::Config;
use crate::state::{Allocation, Pascal};

/// Schedina's builder fee rate on every hedge, micro-units: 0.1% (spec section 1).
pub const BUILDER_FEE_RATE_U: u32 = 1_000;

/// Prices and probabilities: 1.0 in micro-units.
const UNIT: u64 = 1_000_000;

/// The hedger account as the desk trades it: trading key, owner, builder code, venue.
#[derive(Clone)]
pub struct Trader {
    signer: TradingSigner,
    owner: Pubkey,
    owner_b58: String,
    builder: BuilderInfo,
    venue: Venue,
    recv_window_ms: u32,
}

impl std::fmt::Debug for Trader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Trader")
            .field("owner", &self.owner_b58)
            .field("signer", &self.signer.pubkey_base58())
            .field("builder", &base58(&self.builder.pubkey))
            .field("venue", &self.venue)
            .finish()
    }
}

impl Trader {
    /// A trader for `owner` (base58) signing with `signer`, naming `builder` (base58).
    pub fn new(
        signer: TradingSigner,
        owner: &str,
        builder: &str,
        builder_fee_rate_u: u32,
        venue: Venue,
    ) -> Result<Self, PermitError> {
        let owner_key = decode_pubkey(owner, "hedger owner")?;
        let builder = BuilderInfo {
            pubkey: decode_pubkey(builder, "builder pubkey")?,
            fee_rate_u: builder_fee_rate_u,
        };
        Ok(Trader {
            signer,
            owner: owner_key,
            owner_b58: base58(&owner_key),
            builder,
            venue,
            recv_window_ms: pascal_rs::signing::DEFAULT_RECV_WINDOW_MS,
        })
    }

    /// From the desk configuration: `PASCAL_HEDGER_OWNER`, `PASCAL_HEDGER_TRADING_KEY_PATH`,
    /// `PASCAL_BUILDER_PUBKEY`. The venue is [`venue_of`] the configuration.
    pub fn from_config(config: &Config, builder_fee_rate_u: u32) -> Result<Self, String> {
        let owner = config
            .hedger_owner
            .as_deref()
            .ok_or("PASCAL_HEDGER_OWNER is not set")?;
        let path = config
            .hedger_trading_key_path
            .as_ref()
            .ok_or("PASCAL_HEDGER_TRADING_KEY_PATH is not set")?;
        let builder = config
            .builder_pubkey
            .as_deref()
            .ok_or("PASCAL_BUILDER_PUBKEY is not set")?;
        let signer = TradingSigner::from_file(path).map_err(|e| e.to_string())?;
        Trader::new(signer, owner, builder, builder_fee_rate_u, venue_of(config))
            .map_err(|e| e.to_string())
    }

    /// The hedger owner, base58 (the `/accounts/{owner}` path).
    pub fn owner(&self) -> &str {
        &self.owner_b58
    }

    /// The trading key, base58.
    pub fn signer_pubkey(&self) -> String {
        self.signer.pubkey_base58()
    }

    /// The builder code signed into every order.
    pub fn builder(&self) -> BuilderInfo {
        self.builder
    }

    /// Where the orders go (the `venue` of the fills they produce).
    pub fn venue(&self) -> Venue {
        self.venue
    }

    fn auth(&self) -> AuthParams {
        AuthParams::now(self.owner).recv_window_ms(self.recv_window_ms)
    }

    /// Signs one order with the builder code and logs its signed payload.
    pub fn sign_order(
        &self,
        spec: &OrderSpec,
        client_order_id: u64,
    ) -> Result<SignedOrder, PermitError> {
        let mut order = PlaceOrder::limit(
            spec.symbol.clone(),
            spec.side,
            spec.price_u,
            spec.size,
            client_order_id,
        )
        .with_builder(self.builder);
        order.tif = spec.tif;
        order.post_only = spec.post_only;
        order.reduce_only = spec.reduce_only;
        order.validate()?;
        let signed = self.signer.sign_place_order(&self.auth(), &order)?;
        let request_json = serde_json::to_string(&signed.request).unwrap_or_default();
        let out = SignedOrder {
            spec: spec.clone(),
            client_order_id,
            signed,
            request_json,
        };
        out.log(self.venue);
        Ok(out)
    }

    /// Signs a cancel by `client_order_id`.
    pub fn sign_cancel(
        &self,
        client_order_id: u64,
    ) -> Result<Signed<CancelOrderRequest>, PermitError> {
        self.signer.sign_cancel(
            &self.auth(),
            &CancelOrder::by_client_order_id(client_order_id),
        )
    }

    /// Signs a mass cancel of one market.
    pub fn sign_mass_cancel(&self, symbol: &str) -> Result<Signed<MassCancelRequest>, PermitError> {
        self.signer
            .sign_mass_cancel(&self.auth(), &MassCancel::new(symbol))
    }
}

/// `pascal` when the desk writes to a `*.pascal.trade` host, else `mock` (D-007: in mode M the
/// signed orders go to `pascal-mock` and the fills are labelled "mock-filled").
pub fn venue_of(config: &Config) -> Venue {
    let pascal = crate::pascal::trade_url(config)
        .and_then(|u| reqwest::Url::parse(u).ok())
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|h| h == "pascal.trade" || h.ends_with(".pascal.trade"));
    if pascal { Venue::Pascal } else { Venue::Mock }
}

/// What to send, before the `client_order_id` and the signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderSpec {
    /// Symbol.
    pub symbol: String,
    /// `BID` (buys the long side) or `ASK`.
    pub side: pascal_rs::Side,
    /// Limit price on the symbol, micro-USD, on tick.
    pub price_u: i64,
    /// Contracts.
    pub size: u64,
    /// IOC for hedges and unwinds, GTC for post-only rolls.
    pub tif: TimeInForce,
    /// Maker only (rolls).
    pub post_only: bool,
    /// Reduce only (unwinds).
    pub reduce_only: bool,
    /// `inception`, `excess`, `roll_post`, `roll_ioc`, `unwind`, `rebalance`, `flatten`.
    pub kind: &'static str,
    /// Why (`pending_opened`, `lost_leg`, `cash_out`, ...).
    pub reason: String,
    /// The pending it hedges or unwinds, if any.
    pub pending: Option<String>,
    /// Ticket legs served, signed contracts each (`order_map`).
    pub serves: Vec<Allocation>,
}

impl OrderSpec {
    /// Signed contracts of the order: `+size` for a bid, `−size` for an ask.
    pub fn signed_size(&self) -> i64 {
        signed(self.side, self.size)
    }
}

/// A signed order, ready to send.
#[derive(Debug, Clone)]
pub struct SignedOrder {
    /// What it is.
    pub spec: OrderSpec,
    /// Its `client_order_id`.
    pub client_order_id: u64,
    /// Request, permit and signature.
    pub signed: Signed<PlaceOrderRequest>,
    /// The request as sent, JSON.
    pub request_json: String,
}

impl SignedOrder {
    fn log(&self, venue: Venue) {
        let s = &self.spec;
        tracing::info!(
            target: "schedina::signed_order",
            client_order_id = self.client_order_id,
            kind = s.kind,
            reason = %s.reason,
            pending = s.pending.as_deref(),
            symbol = %s.symbol,
            side = ?s.side,
            price_u = s.price_u,
            size = s.size,
            tif = ?s.tif,
            post_only = s.post_only,
            reduce_only = s.reduce_only,
            venue = venue.as_str(),
            permit_hex = %self.signed.permit_hex(),
            signature = %self.signed.signature_base58(),
            request = %self.request_json,
            "signed order"
        );
    }
}

/// `+size` for a bid, `−size` for an ask.
pub fn signed(side: pascal_rs::Side, size: u64) -> i64 {
    let s = i64::try_from(size).unwrap_or(i64::MAX);
    match side {
        pascal_rs::Side::Bid => s,
        pascal_rs::Side::Ask => -s,
    }
}

/// The order that buys the user's side at `p_user_u`: long = `BID` at `p`, short = `ASK` at
/// `1 − p` on the long symbol (spec section 1).
pub fn order_for(side: Side, p_user_u: u64) -> (pascal_rs::Side, i64) {
    let p = p_user_u.min(UNIT);
    match side {
        Side::Long => (pascal_rs::Side::Bid, p as i64),
        Side::Short => (pascal_rs::Side::Ask, (UNIT - p) as i64),
    }
}

/// The API's order side.
pub fn api_side(side: pascal_rs::Side) -> OrderSide {
    match side {
        pascal_rs::Side::Bid => OrderSide::Bid,
        pascal_rs::Side::Ask => OrderSide::Ask,
    }
}

/// `"bid"` / `"ask"`.
pub fn side_str(side: pascal_rs::Side) -> &'static str {
    match side {
        pascal_rs::Side::Bid => "bid",
        pascal_rs::Side::Ask => "ask",
    }
}

/// Counts a Pascal error by code (`pascal_errors_total{code}`, spec section 12).
pub fn count_error(e: &pascal_rs::Error) {
    let code = e.api_code().map_or_else(
        || match e {
            pascal_rs::Error::InvalidArgument(_) => "INVALID_ARGUMENT".to_owned(),
            _ => "TRANSPORT".to_owned(),
        },
        str::to_owned,
    );
    metrics::counter!("pascal_errors_total", "code" => code).increment(1);
}

fn latency(op: &'static str, since: Instant) {
    metrics::histogram!("pascal_order_latency_ms", "op" => op)
        .record(since.elapsed().as_secs_f64() * 1000.0);
}

/// `POST /orders` with latency and error metrics.
pub async fn place(
    pascal: &Pascal,
    orders: &[SignedOrder],
) -> pascal_rs::Result<Vec<WriteResult<PlacedOrder>>> {
    let reqs: Vec<PlaceOrderRequest> = orders.iter().map(|o| o.signed.request.clone()).collect();
    let t = Instant::now();
    let out = pascal.place_orders(&reqs).await;
    latency("place", t);
    match &out {
        Ok(items) => items
            .iter()
            .filter_map(|r| r.as_ref().err())
            .for_each(count_error),
        Err(e) => count_error(e),
    }
    out
}

/// `POST /cancels` of one order by `client_order_id`.
pub async fn cancel(
    pascal: &Pascal,
    trader: &Trader,
    client_order_id: u64,
) -> pascal_rs::Result<Order> {
    let req = trader.sign_cancel(client_order_id)?.request;
    let t = Instant::now();
    let out = pascal.cancel(std::slice::from_ref(&req)).await;
    latency("cancel", t);
    let one = out.and_then(|mut v| {
        v.pop()
            .ok_or_else(|| pascal_rs::Error::InvalidArgument("empty cancel response".into()))?
            .map(|r| r.data)
    });
    if let Err(e) = &one {
        count_error(e);
    }
    one
}

/// `POST /mass-cancels` for up to 10 markets.
pub async fn mass_cancel(
    pascal: &Pascal,
    trader: &Trader,
    symbols: &[String],
) -> pascal_rs::Result<Vec<WriteResult<MassCancelled>>> {
    let reqs = symbols
        .iter()
        .map(|s| trader.sign_mass_cancel(s).map(|x| x.request))
        .collect::<Result<Vec<_>, _>>()?;
    let t = Instant::now();
    let out = pascal.mass_cancel(&reqs).await;
    latency("mass_cancel", t);
    match &out {
        Ok(items) => items
            .iter()
            .filter_map(|r| r.as_ref().err())
            .for_each(count_error),
        Err(e) => count_error(e),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_bids_at_p_short_asks_at_one_minus_p() {
        assert_eq!(
            order_for(Side::Long, 695_000),
            (pascal_rs::Side::Bid, 695_000)
        );
        assert_eq!(
            order_for(Side::Short, 305_000),
            (pascal_rs::Side::Ask, 695_000)
        );
        assert_eq!(signed(pascal_rs::Side::Ask, 7), -7);
    }

    #[test]
    fn venue_follows_the_trade_host() {
        let mut c = Config::local("http://127.0.0.1:9000", ":memory:");
        assert_eq!(venue_of(&c), Venue::Mock);
        c.pascal_trade_url = Some("https://trade.pascal.trade".into());
        assert_eq!(venue_of(&c), Venue::Pascal);
        c.pascal_trade_url = Some("https://pascal.trade.evil.example".into());
        assert_eq!(venue_of(&c), Venue::Mock);
    }

    #[test]
    fn signed_order_carries_the_builder() {
        let signer = TradingSigner::from_seed(&[8; 32]);
        let owner = TradingSigner::from_seed(&[7; 32]).pubkey_base58();
        let builder = TradingSigner::from_seed(&[9; 32]).pubkey_base58();
        let t = Trader::new(signer, &owner, &builder, BUILDER_FEE_RATE_U, Venue::Mock).unwrap();
        let spec = OrderSpec {
            symbol: "NBA_26OCT28_PHIIND_ML.PHI".into(),
            side: pascal_rs::Side::Ask,
            price_u: 695_000,
            size: 12,
            tif: TimeInForce::Ioc,
            post_only: false,
            reduce_only: true,
            kind: "unwind",
            reason: "test".into(),
            pending: None,
            serves: vec![],
        };
        let s = t.sign_order(&spec, 42).unwrap();
        let b = s.signed.request.builder_info.as_ref().unwrap();
        assert_eq!(b.builder_pubkey, builder);
        assert_eq!(b.builder_fee_rate, 1_000);
        assert!(s.signed.request.reduce_only);
        assert_eq!(s.signed.request.client_order_id, 42);
        let v = pascal_rs::signing::verify_place_order(&s.signed.request, 3).unwrap();
        assert_eq!(v.body.builder.unwrap().fee_rate_u, 1_000);
    }
}
