//! Write-API request bodies (`POST /api/v1/orders`, `/cancels`, `/mass-cancels`): parsing,
//! field validation, and the permit-verification seam.
//!
//! The JSON follows docs.pascal.trade ("Place And Replace Orders", "RequestAuth") and
//! the reference `pascal-sign.js`: every number is a string, `side` is `"BID"`/`"ASK"`, optional
//! flags may be omitted. Each batch element is parsed on its own, so one malformed order fails
//! only its own slot. Signatures are **parsed, not verified** in step 1.D: [`PermitVerifier`]
//! is the hook where step 2.D plugs `pascal-rs`'s permit encoder and Ed25519 check (1.A).

use pascal_rs::num::parse_micros;
use pascal_rs::{ApiErrorCode, MicroUsd, OrderType, Side, TimeInForce};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::Value;

use crate::math::rate_u;

/// Largest `recv_window_ms` Pascal accepts.
pub const MAX_RECV_WINDOW_MS: u32 = 60_000;
/// Builder fee rate cap (10%), micro-units.
pub const BUILDER_FEE_RATE_CAP_U: i64 = 100_000;

/// A rejected request or batch item: Pascal's error code and a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reject {
    /// Error code.
    pub code: ApiErrorCode,
    /// Human-readable message.
    pub message: String,
}

impl Reject {
    /// A rejection.
    pub fn new(code: ApiErrorCode, message: impl Into<String>) -> Self {
        Reject {
            code,
            message: message.into(),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Reject::new(ApiErrorCode::InvalidRequest, message)
    }
}

/// `RequestAuth` as sent.
#[derive(Debug, Clone, Deserialize)]
pub struct RawAuth {
    client_ts_ms: String,
    recv_window_ms: String,
    owner: String,
    signer: String,
    signature: String,
}

/// `RequestAuth`, decoded. The signature is decoded but not checked (see [`PermitVerifier`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Auth {
    /// Client timestamp, Unix ms.
    pub client_ts_ms: u64,
    /// Receive window, ms (≤ 60,000).
    pub recv_window_ms: u32,
    /// Account owner (canonical base58).
    pub owner: String,
    /// Owner key bytes.
    pub owner_bytes: [u8; 32],
    /// Signing key (canonical base58).
    pub signer: String,
    /// Signer key bytes.
    pub signer_bytes: [u8; 32],
    /// Ed25519 signature bytes.
    pub signature: [u8; 64],
}

impl Auth {
    fn parse(raw: &RawAuth) -> Result<Auth, Reject> {
        let client_ts_ms = parse_u64(&raw.client_ts_ms, "auth.client_ts_ms")?;
        let recv_window_ms: u32 = raw
            .recv_window_ms
            .parse()
            .map_err(|_| Reject::invalid("auth.recv_window_ms must be a u32 string"))?;
        if recv_window_ms > MAX_RECV_WINDOW_MS {
            return Err(Reject::new(
                ApiErrorCode::RecvWindowTooLarge,
                format!("recv_window_ms={recv_window_ms}, max={MAX_RECV_WINDOW_MS}"),
            ));
        }
        let owner_bytes: [u8; 32] = decode_b58(&raw.owner, "auth.owner")?;
        let signer_bytes: [u8; 32] = decode_b58(&raw.signer, "auth.signer")?;
        let signature: [u8; 64] = decode_b58(&raw.signature, "auth.signature")
            .map_err(|r| Reject::new(ApiErrorCode::Unauthorized, r.message))?;
        Ok(Auth {
            client_ts_ms,
            recv_window_ms,
            owner: bs58::encode(owner_bytes).into_string(),
            owner_bytes,
            signer: bs58::encode(signer_bytes).into_string(),
            signer_bytes,
            signature,
        })
    }
}

/// `BuilderInfo`, decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Builder {
    /// Builder owner (canonical base58).
    pub pubkey: String,
    /// Fee rate as signed, e.g. `0.001000`.
    pub fee_rate: Decimal,
    /// Fee rate in micro-units (`0.001` = 1,000).
    pub fee_rate_u: i64,
}

#[derive(Deserialize)]
struct RawBuilder {
    builder_pubkey: String,
    builder_fee_rate: String,
}

#[derive(Deserialize)]
struct RawPlace {
    client_order_id: String,
    #[serde(default)]
    replace_client_order_id: Option<String>,
    symbol: String,
    side: Side,
    price: String,
    size: String,
    #[serde(default)]
    tif: Option<TimeInForce>,
    #[serde(default, rename = "type")]
    order_type: Option<OrderType>,
    #[serde(default)]
    post_only: bool,
    #[serde(default)]
    reduce_only: bool,
    #[serde(default)]
    allow_missing_replace: bool,
    #[serde(default)]
    replace_with_open_size: bool,
    #[serde(default)]
    expires_ts_ms: Option<String>,
    #[serde(default)]
    metadata: Option<Value>,
    #[serde(default)]
    builder_info: Option<RawBuilder>,
    auth: RawAuth,
}

/// A place-order request, validated field by field (market rules are the engine's job).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceOrder {
    /// Client order id (< 2^64 − 1).
    pub client_order_id: u64,
    /// Order to replace atomically.
    pub replace_client_order_id: Option<u64>,
    /// Market.
    pub symbol: String,
    /// `BID` buys (goes long), `ASK` sells (goes short).
    pub side: Side,
    /// Limit price, micro-USD, strictly inside (0, 1).
    pub price_u: MicroUsd,
    /// Contracts, ≥ 1.
    pub size: u64,
    /// Time in force (default GTC).
    pub tif: TimeInForce,
    /// Display type (default LIMIT; MARKET requires IOC).
    pub order_type: OrderType,
    /// Maker only.
    pub post_only: bool,
    /// Reduce only.
    pub reduce_only: bool,
    /// Place even if the replaced order is missing.
    pub allow_missing_replace: bool,
    /// Take the replaced order's open size.
    pub replace_with_open_size: bool,
    /// GTT expiry, Unix ms.
    pub expires_ts_ms: Option<u64>,
    /// Permit metadata byte (offset 220), 0 when absent.
    pub metadata: u8,
    /// Builder attribution.
    pub builder: Option<Builder>,
    /// Decoded auth.
    pub auth: Auth,
}

impl PlaceOrder {
    /// Parses and validates one element of a `POST /api/v1/orders` batch.
    pub fn parse(v: &Value) -> Result<PlaceOrder, Reject> {
        let raw: RawPlace = serde_json::from_value(v.clone())
            .map_err(|e| Reject::invalid(format!("deserialize_failed: {e}")))?;
        let auth = Auth::parse(&raw.auth)?;
        let client_order_id = parse_u64(&raw.client_order_id, "client_order_id")?;
        if client_order_id == u64::MAX {
            return Err(Reject::invalid("client_order_id must be < 2^64 - 1"));
        }
        let replace_client_order_id = raw
            .replace_client_order_id
            .as_deref()
            .map(|s| parse_u64(s, "replace_client_order_id"))
            .transpose()?;
        if replace_client_order_id == Some(u64::MAX) {
            return Err(Reject::invalid(
                "replace_client_order_id must be < 2^64 - 1",
            ));
        }
        if !(1..=32).contains(&raw.symbol.len())
            || !raw.symbol.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(Reject::invalid("symbol must be 1-32 printable ASCII bytes"));
        }
        let price_u = parse_micros(&raw.price)
            .map_err(|e| Reject::new(ApiErrorCode::InvalidPrice, format!("price: {e}")))?;
        if price_u <= 0 || price_u >= 1_000_000 {
            return Err(Reject::new(
                ApiErrorCode::InvalidPrice,
                format!("price {} must be strictly between 0 and 1", raw.price),
            ));
        }
        let size: u64 = raw
            .size
            .parse()
            .map_err(|_| Reject::new(ApiErrorCode::InvalidSize, "size must be a u64 string"))?;
        if size == 0 {
            return Err(Reject::new(
                ApiErrorCode::InvalidSize,
                "size must be at least 1",
            ));
        }
        let tif = raw.tif.unwrap_or(TimeInForce::Gtc);
        let order_type = raw.order_type.unwrap_or(OrderType::Limit);
        let expires_ts_ms = raw
            .expires_ts_ms
            .as_deref()
            .map(|s| parse_u64(s, "expires_ts_ms"))
            .transpose()?;
        match (tif, expires_ts_ms) {
            (TimeInForce::Gtt, None) | (TimeInForce::Gtt, Some(0)) => {
                return Err(Reject::new(
                    ApiErrorCode::InvalidExpiration,
                    "expires_ts_ms is required (non-zero) for GTT orders",
                ));
            }
            (TimeInForce::Gtc | TimeInForce::Ioc, Some(_)) => {
                return Err(Reject::new(
                    ApiErrorCode::InvalidExpiration,
                    "expires_ts_ms is only valid for GTT orders",
                ));
            }
            _ => {}
        }
        if order_type == OrderType::Market && tif != TimeInForce::Ioc {
            return Err(Reject::new(
                ApiErrorCode::UnsupportedOrderType,
                "MARKET orders require tif IOC",
            ));
        }
        if raw.post_only && tif == TimeInForce::Ioc {
            return Err(Reject::invalid("post_only is incompatible with IOC"));
        }
        if (raw.allow_missing_replace || raw.replace_with_open_size)
            && replace_client_order_id.is_none()
        {
            return Err(Reject::invalid(
                "allow_missing_replace and replace_with_open_size require replace_client_order_id",
            ));
        }
        if raw.allow_missing_replace && raw.replace_with_open_size {
            return Err(Reject::invalid(
                "allow_missing_replace is incompatible with replace_with_open_size",
            ));
        }
        // Undocumented in the API reference; pascal-sign.js sends it as a string. Accept both.
        let metadata = match &raw.metadata {
            None | Some(Value::Null) => Some(0),
            Some(Value::String(s)) => s.parse().ok(),
            Some(Value::Number(n)) => n.as_u64().and_then(|n| u8::try_from(n).ok()),
            Some(_) => None,
        }
        .ok_or_else(|| Reject::invalid("metadata must be a u8"))?;
        let builder = raw
            .builder_info
            .as_ref()
            .map(|b| parse_builder(b, &auth))
            .transpose()?;
        Ok(PlaceOrder {
            client_order_id,
            replace_client_order_id,
            symbol: raw.symbol,
            side: raw.side,
            price_u,
            size,
            tif,
            order_type,
            post_only: raw.post_only,
            reduce_only: raw.reduce_only,
            allow_missing_replace: raw.allow_missing_replace,
            replace_with_open_size: raw.replace_with_open_size,
            expires_ts_ms,
            metadata,
            builder,
            auth,
        })
    }
}

fn parse_builder(b: &RawBuilder, auth: &Auth) -> Result<Builder, Reject> {
    let key: [u8; 32] = decode_b58(&b.builder_pubkey, "builder_info.builder_pubkey")
        .map_err(|r| Reject::new(ApiErrorCode::BuilderInvalid, r.message))?;
    if key == [0; 32] || key == auth.owner_bytes {
        return Err(Reject::new(
            ApiErrorCode::BuilderInvalid,
            "builder must be a registered account other than the order's own",
        ));
    }
    let fee_rate: Decimal = b
        .builder_fee_rate
        .parse()
        .map_err(|_| Reject::invalid("builder_fee_rate must be a decimal string"))?;
    let fee_rate_u = rate_u(fee_rate)
        .filter(|r| (0..=BUILDER_FEE_RATE_CAP_U).contains(r))
        .ok_or_else(|| {
            Reject::new(
                ApiErrorCode::BuilderFeeRateInvalid,
                format!(
                    "builder_fee_rate {} is outside [0, 0.1]",
                    b.builder_fee_rate
                ),
            )
        })?;
    Ok(Builder {
        pubkey: bs58::encode(key).into_string(),
        fee_rate,
        fee_rate_u,
    })
}

#[derive(Deserialize)]
struct RawCancel {
    #[serde(default)]
    client_order_id: Option<String>,
    #[serde(default)]
    order_id: Option<String>,
    auth: RawAuth,
}

/// Which order a cancel targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelTarget {
    /// By client order id.
    ClientOrderId(u64),
    /// By exchange order id.
    OrderId(u64),
}

/// A cancel request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelOrder {
    /// Target.
    pub target: CancelTarget,
    /// Decoded auth.
    pub auth: Auth,
}

impl CancelOrder {
    /// Parses one element of a `POST /api/v1/cancels` batch.
    pub fn parse(v: &Value) -> Result<CancelOrder, Reject> {
        let raw: RawCancel = serde_json::from_value(v.clone())
            .map_err(|e| Reject::invalid(format!("deserialize_failed: {e}")))?;
        let auth = Auth::parse(&raw.auth)?;
        let target = match (raw.client_order_id, raw.order_id) {
            (Some(c), None) => CancelTarget::ClientOrderId(parse_u64(&c, "client_order_id")?),
            (None, Some(o)) => CancelTarget::OrderId(parse_u64(&o, "order_id")?),
            _ => {
                return Err(Reject::invalid(
                    "exactly one of client_order_id and order_id is required",
                ));
            }
        };
        Ok(CancelOrder { target, auth })
    }
}

#[derive(Deserialize)]
struct RawMassCancel {
    symbol: String,
    auth: RawAuth,
}

/// A mass-cancel request (one market).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MassCancel {
    /// Market.
    pub symbol: String,
    /// Decoded auth.
    pub auth: Auth,
}

impl MassCancel {
    /// Parses one element of a `POST /api/v1/mass-cancels` batch.
    pub fn parse(v: &Value) -> Result<MassCancel, Reject> {
        let raw: RawMassCancel = serde_json::from_value(v.clone())
            .map_err(|e| Reject::invalid(format!("deserialize_failed: {e}")))?;
        let auth = Auth::parse(&raw.auth)?;
        Ok(MassCancel {
            symbol: raw.symbol,
            auth,
        })
    }
}

/// Checks the Ed25519 permit of each write request. Step 1.D ships [`AcceptAll`]; step 2.D
/// implements this with `pascal-rs`'s permit encoder (1.A) and returns `Err` → `UNAUTHORIZED`.
///
/// Each request carries everything the permit encodes (`auth` header fields, the order body
/// including `metadata` and builder bytes), so an implementation can rebuild the exact signed
/// bytes. `raw` is the JSON element as received.
pub trait PermitVerifier: Send + Sync + 'static {
    /// Verifies a place-order permit (`cmd_type` 1).
    fn verify_place(&self, order: &PlaceOrder, raw: &Value) -> Result<(), String>;
    /// Verifies a cancel permit (`cmd_type` 2).
    fn verify_cancel(&self, cancel: &CancelOrder, raw: &Value) -> Result<(), String>;
    /// Verifies a mass-cancel permit (`cmd_type` 10).
    fn verify_mass_cancel(&self, cancel: &MassCancel, raw: &Value) -> Result<(), String>;
}

/// Accepts every permit (step 1.D: signatures parsed, not verified).
#[derive(Debug, Clone, Copy, Default)]
pub struct AcceptAll;

impl PermitVerifier for AcceptAll {
    fn verify_place(&self, _: &PlaceOrder, _: &Value) -> Result<(), String> {
        Ok(())
    }
    fn verify_cancel(&self, _: &CancelOrder, _: &Value) -> Result<(), String> {
        Ok(())
    }
    fn verify_mass_cancel(&self, _: &MassCancel, _: &Value) -> Result<(), String> {
        Ok(())
    }
}

fn parse_u64(s: &str, field: &str) -> Result<u64, Reject> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Reject::invalid(format!("{field} must be a u64 string")));
    }
    s.parse()
        .map_err(|_| Reject::invalid(format!("{field} must be a u64 string")))
}

fn decode_b58<const N: usize>(s: &str, field: &str) -> Result<[u8; N], Reject> {
    let bytes = bs58::decode(s)
        .into_vec()
        .map_err(|_| Reject::invalid(format!("{field} is not base58")))?;
    bytes
        .try_into()
        .map_err(|_| Reject::invalid(format!("{field} must decode to {N} bytes")))
}

/// Whether `s` is a base58 string of a 32-byte key.
pub fn is_pubkey(s: &str) -> bool {
    decode_b58::<32>(s, "").is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const OWNER: &str = "GmaDrppBC7P5ARKV8g3djiwP89vz1jLK23V2GBjuAEGB";
    const SIGNER: &str = "2KW2XRd9kwqet15Aha2oK3tYvd3nWbTFH1MBiRAv1BE1";
    const SIG: &str =
        "46gXGMwTxEckctHD3tjVyedhGG95VK2F97F3idkSYFR3WNEX4rG9g6g8KDTQPoxzLieudk8WHDXw7xNV3R2WwKCd";

    fn auth() -> Value {
        json!({"client_ts_ms":"1731536000000","recv_window_ms":"5000","owner":OWNER,"signer":SIGNER,"signature":SIG})
    }

    #[test]
    fn parses_the_docs_request() {
        // docs.pascal.trade "Request Signing" place-order example (fixtures/pascal/signing).
        let v = json!({
            "client_order_id": "42", "replace_client_order_id": "43",
            "symbol": "SIM_EVENT_1.MARKET_1", "side": "BID", "price": "0.550000", "size": "10",
            "tif": "GTT", "post_only": true, "reduce_only": true, "allow_missing_replace": true,
            "expires_ts_ms": "1731539600000", "auth": auth(),
        });
        let o = PlaceOrder::parse(&v).unwrap();
        assert_eq!(o.client_order_id, 42);
        assert_eq!(o.replace_client_order_id, Some(43));
        assert_eq!(o.side, Side::Bid);
        assert_eq!(o.price_u, 550_000);
        assert_eq!(o.size, 10);
        assert_eq!(o.tif, TimeInForce::Gtt);
        assert_eq!(o.expires_ts_ms, Some(1_731_539_600_000));
        assert!(o.post_only && o.reduce_only && o.allow_missing_replace);
        assert_eq!(o.auth.owner, OWNER);
        assert_eq!(o.auth.recv_window_ms, 5000);
        assert_eq!(o.auth.owner_bytes[0], 0xea);
    }

    #[test]
    fn parses_an_ioc_with_builder_as_pascal_sign_js_emits_it() {
        let v = json!({
            "client_order_id": "7", "symbol": "NBA_26OCT28_PHIIND_ML.PHI", "side": "ASK",
            "price": "0.305000", "size": "3", "tif": "IOC", "type": "MARKET", "metadata": "2",
            "builder_info": {"builder_pubkey": SIGNER, "builder_fee_rate": "0.001000"},
            "auth": auth(),
        });
        let o = PlaceOrder::parse(&v).unwrap();
        assert_eq!(o.order_type, OrderType::Market);
        assert_eq!(o.metadata, 2);
        let b = o.builder.unwrap();
        assert_eq!(b.fee_rate_u, 1_000);
        assert_eq!(b.fee_rate.to_string(), "0.001000");
    }

    #[test]
    fn rejects_with_pascal_codes() {
        let base = || json!({"client_order_id":"1","symbol":"A.B","side":"BID","price":"0.500000","size":"1","auth":auth()});
        let code = |patch: Value| {
            let mut v = base();
            for (k, x) in patch.as_object().unwrap() {
                v[k] = x.clone();
            }
            PlaceOrder::parse(&v).unwrap_err().code
        };
        assert_eq!(
            code(json!({"price":"1.000000"})),
            ApiErrorCode::InvalidPrice
        );
        assert_eq!(
            code(json!({"price":"0.5000001"})),
            ApiErrorCode::InvalidPrice
        );
        assert_eq!(code(json!({"size":"0"})), ApiErrorCode::InvalidSize);
        assert_eq!(code(json!({"size":10})), ApiErrorCode::InvalidRequest);
        assert_eq!(code(json!({"side":"BUY"})), ApiErrorCode::InvalidRequest);
        assert_eq!(code(json!({"tif":"GTT"})), ApiErrorCode::InvalidExpiration);
        assert_eq!(
            code(json!({"expires_ts_ms":"5"})),
            ApiErrorCode::InvalidExpiration
        );
        assert_eq!(
            code(json!({"type":"MARKET"})),
            ApiErrorCode::UnsupportedOrderType
        );
        assert_eq!(
            code(json!({"tif":"IOC","post_only":true})),
            ApiErrorCode::InvalidRequest
        );
        assert_eq!(
            code(json!({"client_order_id":"18446744073709551615"})),
            ApiErrorCode::InvalidRequest
        );
        assert_eq!(
            code(json!({"builder_info":{"builder_pubkey":OWNER,"builder_fee_rate":"0.001000"}})),
            ApiErrorCode::BuilderInvalid
        );
        assert_eq!(
            code(json!({"builder_info":{"builder_pubkey":SIGNER,"builder_fee_rate":"0.200000"}})),
            ApiErrorCode::BuilderFeeRateInvalid
        );
        let mut v = base();
        v["auth"]["recv_window_ms"] = json!("60001");
        assert_eq!(
            PlaceOrder::parse(&v).unwrap_err().code,
            ApiErrorCode::RecvWindowTooLarge
        );
        v["auth"]["recv_window_ms"] = json!("5000");
        v["auth"]["signature"] = json!("abc");
        assert_eq!(
            PlaceOrder::parse(&v).unwrap_err().code,
            ApiErrorCode::Unauthorized
        );
    }

    #[test]
    fn parses_cancels() {
        let c = CancelOrder::parse(&json!({"client_order_id":"42","auth":auth()})).unwrap();
        assert_eq!(c.target, CancelTarget::ClientOrderId(42));
        let c = CancelOrder::parse(&json!({"order_id":"9","auth":auth()})).unwrap();
        assert_eq!(c.target, CancelTarget::OrderId(9));
        assert!(CancelOrder::parse(&json!({"auth":auth()})).is_err());
        assert!(
            CancelOrder::parse(&json!({"order_id":"9","client_order_id":"1","auth":auth()}))
                .is_err()
        );
        let m = MassCancel::parse(&json!({"symbol":"A.B","auth":auth()})).unwrap();
        assert_eq!(m.symbol, "A.B");
        assert!(is_pubkey(OWNER));
        assert!(!is_pubkey("abc"));
    }
}
