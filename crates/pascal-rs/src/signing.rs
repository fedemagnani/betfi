//! Pascal request signing: binary permits, Ed25519 signatures and the signed JSON requests.
//!
//! A Rust port of the reference `pascal-sign.js`, extended with the cancel and mass-cancel
//! permits. Every write request to Pascal carries an `auth` object whose `signature` is an
//! Ed25519 signature, by the account's trading key, over a compact little-endian binary
//! **permit**: an 80-byte header followed by a command-specific body.
//!
//! | Permit | `cmd_type` | Length | Body |
//! | --- | --- | --- | --- |
//! | place order | 1 | 232 | size, price, ids, expiry, builder, symbol, flags ([`layout`]) |
//! | cancel | 2 | 96 | `cancel_by` (80), metadata (81), id (88) |
//! | mass cancel | 10 | 120 | metadata (80), symbol (88..120) |
//!
//! Sources: docs.pascal.trade "Request Signing" for the header, the bodies and the test
//! vectors; Pascal's on-chain IDL (`fixtures/pascal/signing/pascal-idl-permit.json`) for the
//! bytes the docs call "reserved" in the place-order body: `builder_pubkey` at 120..152,
//! `builder_fee_rate` (i32 LE, 6 d.p.) at 184..188 and `metadata` at 220 (decision D-003). With
//! no builder and metadata 0 those bytes are zero, which is exactly the published vector.
//!
//! The permit bytes and the JSON request are both built from one validated value
//! ([`PlaceOrder`], [`CancelOrder`], [`MassCancel`]), so they cannot disagree. The reverse
//! direction, [`verify_place_order`] and friends, rebuilds the permit from a JSON request,
//! checks the signature against `auth.signer`, and returns the decoded fields: this is what
//! `pascal-mock` uses to accept only correctly signed orders.
//!
//! Nothing in this module does I/O.

use std::fmt;
use std::path::Path;

use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::error::ApiErrorCode;
use crate::num::{MICROS_PER_UNIT, MicroUsd, micros, opt_str_num, str_num};
use crate::types::{OrderType, Side, TimeInForce};

/// A 32-byte Ed25519 / Solana public key.
pub type Pubkey = [u8; 32];

/// Permit envelope version (byte 0 of every permit).
pub const PERMIT_VERSION: u8 = 1;
/// `cmd_type` of a place-order permit.
pub const CMD_PLACE_ORDER: u8 = 1;
/// `cmd_type` of a cancel permit.
pub const CMD_CANCEL_ORDER: u8 = 2;
/// `cmd_type` of a mass-cancel permit.
pub const CMD_MASS_CANCEL: u8 = 10;
/// Length of the permit header.
pub const HEADER_LEN: usize = 80;
/// Length of a place-order permit (header + 152-byte body).
pub const PLACE_ORDER_PERMIT_LEN: usize = 232;
/// Length of a cancel permit (header + 16-byte body).
pub const CANCEL_PERMIT_LEN: usize = 96;
/// Length of a mass-cancel permit (header + 40-byte body).
pub const MASS_CANCEL_PERMIT_LEN: usize = 120;
/// `deployment_id` of Pascal's production deployment.
pub const DEPLOYMENT_PROD: u8 = 3;
/// Default `recv_window_ms`.
pub const DEFAULT_RECV_WINDOW_MS: u32 = 5_000;
/// Largest `recv_window_ms` Pascal accepts (one minute).
pub const MAX_RECV_WINDOW_MS: u32 = 60_000;
/// `replace_client_order_id` in the permit when the JSON omits it (all bits set).
pub const OMITTED_REPLACE_CLIENT_ORDER_ID: u64 = u64::MAX;
/// Highest builder fee rate Pascal accepts, micro-units (0.100000 = 10%).
pub const BUILDER_FEE_RATE_CAP_U: u32 = 100_000;
/// Longest market symbol (the permit's fixed 32-byte field).
pub const MAX_SYMBOL_LEN: usize = 32;
/// SHA-256 of `pascal.place_order.offchain_fields.v1`; the docs fix it as the current value.
pub const OFFCHAIN_FIELDS_DIGEST: [u8; 32] = [
    0xdc, 0xaf, 0x53, 0x73, 0x8f, 0x1d, 0x5f, 0x06, 0x16, 0x46, 0x60, 0x49, 0x8a, 0x75, 0x88, 0x8e,
    0x7f, 0x0c, 0x78, 0xed, 0x5f, 0x1b, 0xc1, 0x7b, 0xe1, 0xdd, 0x1c, 0xb3, 0xcc, 0xf5, 0x3c, 0xe3,
];

/// Byte offsets of the place-order permit (header + body), as in the on-chain IDL.
pub mod layout {
    /// `version` (u8).
    pub const VERSION: usize = 0;
    /// `cmd_type` (u8).
    pub const CMD_TYPE: usize = 1;
    /// `deployment_id` (u8).
    pub const DEPLOYMENT_ID: usize = 2;
    /// `padding0` (1 byte, zero).
    pub const PADDING0: usize = 3;
    /// `recv_window_ms` (u32 LE).
    pub const RECV_WINDOW_MS: usize = 4;
    /// `client_ts_ms` (u64 LE).
    pub const CLIENT_TS_MS: usize = 8;
    /// `owner` (32 bytes).
    pub const OWNER: usize = 16;
    /// `signer` (32 bytes).
    pub const SIGNER: usize = 48;
    /// `size` (u64 LE, contracts).
    pub const SIZE: usize = 80;
    /// `price` (u64 LE, micro-USD).
    pub const PRICE: usize = 88;
    /// `client_order_id` (u64 LE).
    pub const CLIENT_ORDER_ID: usize = 96;
    /// `replace_client_order_id` (u64 LE, all bits set when omitted).
    pub const REPLACE_CLIENT_ORDER_ID: usize = 104;
    /// `expire_ts_ms` (u64 LE, 0 when omitted).
    pub const EXPIRE_TS_MS: usize = 112;
    /// `builder_pubkey` (32 bytes, zero without a builder).
    pub const BUILDER_PUBKEY: usize = 120;
    /// `symbol` (32 bytes, zero-padded).
    pub const SYMBOL: usize = 152;
    /// `builder_fee_rate` (i32 LE, 6 d.p.).
    pub const BUILDER_FEE_RATE: usize = 184;
    /// `offchain_fields_digest` (32 bytes).
    pub const OFFCHAIN_FIELDS_DIGEST: usize = 188;
    /// `metadata` (u8).
    pub const METADATA: usize = 220;
    /// `side` (u8: 0 bid, 1 ask).
    pub const SIDE: usize = 221;
    /// `order_type` (u8: 0 limit, 1 market).
    pub const ORDER_TYPE: usize = 222;
    /// `tif` (u8: 0 GTC, 1 GTT, 2 IOC).
    pub const TIF: usize = 223;
    /// `flags` (u8: post_only, reduce_only, allow_missing_replace, replace_with_open_size).
    pub const FLAGS: usize = 224;
    /// `padding1` (7 bytes, zero).
    pub const PADDING1: usize = 225;
}

const FLAG_POST_ONLY: u8 = 1;
const FLAG_REDUCE_ONLY: u8 = 2;
const FLAG_ALLOW_MISSING_REPLACE: u8 = 4;
const FLAG_REPLACE_WITH_OPEN_SIZE: u8 = 8;

/// Why a permit could not be built, decoded or verified.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PermitError {
    /// Malformed or inconsistent request fields (`INVALID_REQUEST`).
    #[error("invalid request: {0}")]
    Invalid(String),
    /// Price outside (0, 1) (`INVALID_PRICE`).
    #[error("invalid price: {0}")]
    Price(String),
    /// Size zero (`INVALID_SIZE`).
    #[error("invalid size: {0}")]
    Size(String),
    /// Expiry inconsistent with the time in force (`INVALID_EXPIRATION`).
    #[error("invalid expiration: {0}")]
    Expiration(String),
    /// `recv_window_ms` above one minute (`RECV_WINDOW_TOO_LARGE`).
    #[error("recv_window_ms {0} exceeds {MAX_RECV_WINDOW_MS}")]
    RecvWindowTooLarge(u32),
    /// Request older than its receive window (`CLIENT_TS_EXPIRED`).
    #[error("request expired: client_ts_ms {client_ts_ms} + {recv_window_ms} ms < {now_ms}")]
    Expired {
        /// Header timestamp.
        client_ts_ms: u64,
        /// Header window.
        recv_window_ms: u32,
        /// The time it was checked at.
        now_ms: u64,
    },
    /// Request timestamp ahead of the clock beyond the tolerance (`CLIENT_TS_TOO_FAR_IN_FUTURE`).
    #[error("client_ts_ms {client_ts_ms} is ahead of {now_ms}")]
    InFuture {
        /// Header timestamp.
        client_ts_ms: u64,
        /// The time it was checked at.
        now_ms: u64,
    },
    /// Builder fee rate negative or above the cap (`BUILDER_FEE_RATE_INVALID`).
    #[error("builder fee rate: {0}")]
    BuilderFeeRate(String),
    /// Builder key rejected: zero key or the order's own account (`BUILDER_INVALID`).
    #[error("builder: {0}")]
    Builder(String),
    /// Signature missing, malformed, or not valid for the signer (`UNAUTHORIZED`).
    #[error("unauthorized: {0}")]
    Unauthorized(String),
}

impl PermitError {
    /// The Pascal error code a server would answer with.
    pub fn api_code(&self) -> ApiErrorCode {
        match self {
            PermitError::Invalid(_) => ApiErrorCode::InvalidRequest,
            PermitError::Price(_) => ApiErrorCode::InvalidPrice,
            PermitError::Size(_) => ApiErrorCode::InvalidSize,
            PermitError::Expiration(_) => ApiErrorCode::InvalidExpiration,
            PermitError::RecvWindowTooLarge(_) => ApiErrorCode::RecvWindowTooLarge,
            PermitError::Expired { .. } => ApiErrorCode::ClientTsExpired,
            PermitError::InFuture { .. } => ApiErrorCode::ClientTsTooFarInFuture,
            PermitError::BuilderFeeRate(_) => ApiErrorCode::BuilderFeeRateInvalid,
            PermitError::Builder(_) => ApiErrorCode::BuilderInvalid,
            PermitError::Unauthorized(_) => ApiErrorCode::Unauthorized,
        }
    }
}

impl From<PermitError> for crate::Error {
    fn from(e: PermitError) -> Self {
        crate::Error::InvalidArgument(e.to_string())
    }
}

fn invalid(msg: impl Into<String>) -> PermitError {
    PermitError::Invalid(msg.into())
}

// ---------- encodings ----------

/// Base58 (Bitcoin alphabet, as Solana and Pascal use it).
pub fn base58(bytes: &[u8]) -> String {
    bs58::encode(bytes).into_string()
}

/// Decodes a base58 public key.
pub fn decode_pubkey(s: &str, what: &str) -> Result<Pubkey, PermitError> {
    let v = bs58::decode(s)
        .into_vec()
        .map_err(|e| invalid(format!("{what} {s:?} is not base58: {e}")))?;
    v.try_into()
        .map_err(|v: Vec<u8>| invalid(format!("{what} must be 32 bytes, got {}", v.len())))
}

fn decode_signature(s: &str) -> Result<[u8; 64], PermitError> {
    let v = bs58::decode(s)
        .into_vec()
        .map_err(|e| PermitError::Unauthorized(format!("signature is not base58: {e}")))?;
    v.try_into().map_err(|v: Vec<u8>| {
        PermitError::Unauthorized(format!("signature must be 64 bytes, got {}", v.len()))
    })
}

/// Lower-case hex.
pub fn to_hex(bytes: &[u8]) -> String {
    use fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Parses hex (even length, either case).
pub fn from_hex(s: &str) -> Result<Vec<u8>, PermitError> {
    if !s.len().is_multiple_of(2) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid("invalid hex string"));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| invalid("invalid hex string")))
        .collect()
}

fn check_symbol(symbol: &str) -> Result<(), PermitError> {
    let ok = (1..=MAX_SYMBOL_LEN).contains(&symbol.len())
        && symbol.bytes().all(|b| (0x21..=0x7e).contains(&b));
    if ok {
        Ok(())
    } else {
        Err(invalid(format!(
            "symbol {symbol:?} must be 1-32 printable ASCII bytes"
        )))
    }
}

fn put_symbol(buf: &mut [u8], at: usize, symbol: &str) {
    buf[at..at + symbol.len()].copy_from_slice(symbol.as_bytes());
}

fn read_symbol(buf: &[u8], at: usize) -> Result<String, PermitError> {
    let field = &buf[at..at + MAX_SYMBOL_LEN];
    let len = field.iter().position(|&b| b == 0).unwrap_or(MAX_SYMBOL_LEN);
    if field[len..].iter().any(|&b| b != 0) {
        return Err(invalid("symbol field is not zero-padded"));
    }
    let s = std::str::from_utf8(&field[..len])
        .map_err(|_| invalid("symbol is not ASCII"))?
        .to_owned();
    check_symbol(&s)?;
    Ok(s)
}

fn u64_at(buf: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(buf[at..at + 8].try_into().expect("8 bytes"))
}

fn zeros(buf: &[u8], what: &str) -> Result<(), PermitError> {
    if buf.iter().all(|&b| b == 0) {
        Ok(())
    } else {
        Err(invalid(format!("{what} must be zero")))
    }
}

fn side_byte(s: Side) -> u8 {
    match s {
        Side::Bid => 0,
        Side::Ask => 1,
    }
}

fn order_type_byte(t: OrderType) -> u8 {
    match t {
        OrderType::Limit => 0,
        OrderType::Market => 1,
    }
}

fn tif_byte(t: TimeInForce) -> u8 {
    match t {
        TimeInForce::Gtc => 0,
        TimeInForce::Gtt => 1,
        TimeInForce::Ioc => 2,
    }
}

// ---------- header ----------

/// Request-level auth parameters: everything in the header except the signer, which comes
/// from the signing key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthParams {
    /// Account owner (wallet public key).
    pub owner: Pubkey,
    /// Deployment (3 = prod, [`DEPLOYMENT_PROD`]).
    pub deployment_id: u8,
    /// Freshness window, at most [`MAX_RECV_WINDOW_MS`].
    pub recv_window_ms: u32,
    /// Client timestamp, Unix ms.
    pub client_ts_ms: u64,
}

impl AuthParams {
    /// Production deployment, default 5 s window.
    pub fn new(owner: Pubkey, client_ts_ms: u64) -> Self {
        AuthParams {
            owner,
            deployment_id: DEPLOYMENT_PROD,
            recv_window_ms: DEFAULT_RECV_WINDOW_MS,
            client_ts_ms,
        }
    }

    /// [`AuthParams::new`] stamped with the system clock.
    pub fn now(owner: Pubkey) -> Self {
        Self::new(owner, now_ms())
    }

    /// Sets `recv_window_ms`.
    pub fn recv_window_ms(mut self, ms: u32) -> Self {
        self.recv_window_ms = ms;
        self
    }

    /// Sets `deployment_id`.
    pub fn deployment_id(mut self, id: u8) -> Self {
        self.deployment_id = id;
        self
    }

    fn header(&self, cmd_type: u8, signer: Pubkey) -> PermitHeader {
        PermitHeader {
            version: PERMIT_VERSION,
            cmd_type,
            deployment_id: self.deployment_id,
            recv_window_ms: self.recv_window_ms,
            client_ts_ms: self.client_ts_ms,
            owner: self.owner,
            signer,
        }
    }
}

/// System clock in Unix ms.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The 80-byte permit header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermitHeader {
    /// Always [`PERMIT_VERSION`].
    pub version: u8,
    /// Command discriminator.
    pub cmd_type: u8,
    /// Deployment.
    pub deployment_id: u8,
    /// Freshness window.
    pub recv_window_ms: u32,
    /// Client timestamp, Unix ms.
    pub client_ts_ms: u64,
    /// Account owner.
    pub owner: Pubkey,
    /// Key that produced the signature.
    pub signer: Pubkey,
}

impl PermitHeader {
    /// Encodes the header (little-endian).
    pub fn encode(&self) -> Result<[u8; HEADER_LEN], PermitError> {
        if self.recv_window_ms > MAX_RECV_WINDOW_MS {
            return Err(PermitError::RecvWindowTooLarge(self.recv_window_ms));
        }
        let mut b = [0u8; HEADER_LEN];
        b[layout::VERSION] = self.version;
        b[layout::CMD_TYPE] = self.cmd_type;
        b[layout::DEPLOYMENT_ID] = self.deployment_id;
        b[layout::RECV_WINDOW_MS..layout::RECV_WINDOW_MS + 4]
            .copy_from_slice(&self.recv_window_ms.to_le_bytes());
        b[layout::CLIENT_TS_MS..layout::CLIENT_TS_MS + 8]
            .copy_from_slice(&self.client_ts_ms.to_le_bytes());
        b[layout::OWNER..layout::OWNER + 32].copy_from_slice(&self.owner);
        b[layout::SIGNER..layout::SIGNER + 32].copy_from_slice(&self.signer);
        Ok(b)
    }

    /// Decodes the first 80 bytes of a permit, checking the version, padding and window.
    pub fn decode(permit: &[u8]) -> Result<Self, PermitError> {
        if permit.len() < HEADER_LEN {
            return Err(invalid(format!(
                "permit of {} bytes is shorter than the header",
                permit.len()
            )));
        }
        let h = PermitHeader {
            version: permit[layout::VERSION],
            cmd_type: permit[layout::CMD_TYPE],
            deployment_id: permit[layout::DEPLOYMENT_ID],
            recv_window_ms: u32::from_le_bytes(
                permit[layout::RECV_WINDOW_MS..layout::RECV_WINDOW_MS + 4]
                    .try_into()
                    .expect("4 bytes"),
            ),
            client_ts_ms: u64_at(permit, layout::CLIENT_TS_MS),
            owner: permit[layout::OWNER..layout::OWNER + 32]
                .try_into()
                .expect("32 bytes"),
            signer: permit[layout::SIGNER..layout::SIGNER + 32]
                .try_into()
                .expect("32 bytes"),
        };
        if h.version != PERMIT_VERSION {
            return Err(invalid(format!("permit version {}", h.version)));
        }
        zeros(&permit[layout::PADDING0..layout::PADDING0 + 1], "padding0")?;
        if h.recv_window_ms > MAX_RECV_WINDOW_MS {
            return Err(PermitError::RecvWindowTooLarge(h.recv_window_ms));
        }
        Ok(h)
    }

    /// Pascal's freshness rule: accepted while `now_ms - client_ts_ms <= recv_window_ms`, and
    /// not more than `max_skew_ms` ahead of `now_ms`.
    pub fn check_fresh(&self, now_ms: u64, max_skew_ms: u64) -> Result<(), PermitError> {
        if self.client_ts_ms > now_ms.saturating_add(max_skew_ms) {
            return Err(PermitError::InFuture {
                client_ts_ms: self.client_ts_ms,
                now_ms,
            });
        }
        if now_ms.saturating_sub(self.client_ts_ms) > u64::from(self.recv_window_ms) {
            return Err(PermitError::Expired {
                client_ts_ms: self.client_ts_ms,
                recv_window_ms: self.recv_window_ms,
                now_ms,
            });
        }
        Ok(())
    }

    fn auth_json(&self, signature: &[u8; 64]) -> RequestAuth {
        RequestAuth {
            client_ts_ms: self.client_ts_ms,
            recv_window_ms: self.recv_window_ms,
            owner: base58(&self.owner),
            signer: base58(&self.signer),
            signature: base58(signature),
        }
    }
}

// ---------- place order ----------

/// Builder attribution signed into a place-order permit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuilderInfo {
    /// Registered builder account (never the order's own account).
    pub pubkey: Pubkey,
    /// Fee rate in micro-units: 1_000 = 0.1%, at most [`BUILDER_FEE_RATE_CAP_U`].
    pub fee_rate_u: u32,
}

impl BuilderInfo {
    fn validate(&self) -> Result<(), PermitError> {
        if self.pubkey == [0; 32] {
            return Err(PermitError::Builder(
                "builder_pubkey must not be the zero key".into(),
            ));
        }
        if self.fee_rate_u > BUILDER_FEE_RATE_CAP_U {
            return Err(PermitError::BuilderFeeRate(format!(
                "{} is above the 0.100000 cap",
                crate::num::format_micros(i64::from(self.fee_rate_u))
            )));
        }
        Ok(())
    }
}

/// A place-order request before signing. Build one with [`PlaceOrder::limit`] and adjust the
/// public fields; [`PlaceOrder::validate`] runs the checks Pascal documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceOrder {
    /// Market symbol (1-32 printable ASCII bytes).
    pub symbol: String,
    /// Bid or ask.
    pub side: Side,
    /// Limit price, micro-USD, strictly inside (0, 1_000_000).
    pub price: MicroUsd,
    /// Contracts, at least 1.
    pub size: u64,
    /// Client order id: a plain desk-wide counter, unique among open orders, below `u64::MAX`.
    pub client_order_id: u64,
    /// Order to replace atomically.
    pub replace_client_order_id: Option<u64>,
    /// Limit (default) or market (requires IOC).
    pub order_type: OrderType,
    /// GTC (default), GTT (needs `expires_ts_ms`) or IOC.
    pub tif: TimeInForce,
    /// GTT expiry, Unix ms.
    pub expires_ts_ms: Option<u64>,
    /// Maker only.
    pub post_only: bool,
    /// Reduce only (unwinds).
    pub reduce_only: bool,
    /// Place even if `replace_client_order_id` is not found.
    pub allow_missing_replace: bool,
    /// Keep the replaced order's open size.
    pub replace_with_open_size: bool,
    /// Client metadata byte (0 unless you know otherwise; the Python quickstart writes 2).
    pub metadata: u8,
    /// Builder code.
    pub builder: Option<BuilderInfo>,
}

impl PlaceOrder {
    /// A GTC limit order with no flags, no builder and metadata 0.
    pub fn limit(
        symbol: impl Into<String>,
        side: Side,
        price: MicroUsd,
        size: u64,
        client_order_id: u64,
    ) -> Self {
        PlaceOrder {
            symbol: symbol.into(),
            side,
            price,
            size,
            client_order_id,
            replace_client_order_id: None,
            order_type: OrderType::Limit,
            tif: TimeInForce::Gtc,
            expires_ts_ms: None,
            post_only: false,
            reduce_only: false,
            allow_missing_replace: false,
            replace_with_open_size: false,
            metadata: 0,
            builder: None,
        }
    }

    /// An immediate-or-cancel limit order (the desk's inception hedge).
    pub fn ioc(
        symbol: impl Into<String>,
        side: Side,
        price: MicroUsd,
        size: u64,
        client_order_id: u64,
    ) -> Self {
        PlaceOrder {
            tif: TimeInForce::Ioc,
            ..Self::limit(symbol, side, price, size, client_order_id)
        }
    }

    /// Sets the builder code.
    pub fn with_builder(mut self, builder: BuilderInfo) -> Self {
        self.builder = Some(builder);
        self
    }

    /// Checks the order on its own (the owner-dependent builder check is in signing).
    pub fn validate(&self) -> Result<(), PermitError> {
        check_symbol(&self.symbol)?;
        if self.size == 0 {
            return Err(PermitError::Size("size must be at least 1 contract".into()));
        }
        if self.price <= 0 || self.price >= MICROS_PER_UNIT {
            return Err(PermitError::Price(format!(
                "price {} must be strictly between 0 and 1",
                crate::num::format_micros(self.price)
            )));
        }
        if self.client_order_id == u64::MAX {
            return Err(invalid("client_order_id must be < 2^64 - 1"));
        }
        if self.replace_client_order_id == Some(OMITTED_REPLACE_CLIENT_ORDER_ID) {
            return Err(invalid(
                "replace_client_order_id must be < 2^64 - 1 (all bits set means omitted)",
            ));
        }
        match (self.tif, self.expires_ts_ms) {
            (TimeInForce::Gtt, None) => {
                return Err(PermitError::Expiration(
                    "expires_ts_ms is required for GTT orders".into(),
                ));
            }
            (TimeInForce::Gtt, Some(0)) => {
                return Err(PermitError::Expiration(
                    "expires_ts_ms must be non-zero (0 encodes no expiry)".into(),
                ));
            }
            (TimeInForce::Gtc | TimeInForce::Ioc, Some(_)) => {
                return Err(PermitError::Expiration(
                    "expires_ts_ms is only valid for GTT orders".into(),
                ));
            }
            _ => {}
        }
        if self.order_type == OrderType::Market && self.tif != TimeInForce::Ioc {
            return Err(invalid("MARKET orders require tif IOC"));
        }
        if self.post_only && self.tif == TimeInForce::Ioc {
            return Err(invalid(
                "post_only is incompatible with IOC and MARKET orders",
            ));
        }
        if (self.allow_missing_replace || self.replace_with_open_size)
            && self.replace_client_order_id.is_none()
        {
            return Err(invalid(
                "allow_missing_replace and replace_with_open_size require replace_client_order_id",
            ));
        }
        if self.allow_missing_replace && self.replace_with_open_size {
            return Err(invalid(
                "allow_missing_replace is incompatible with replace_with_open_size",
            ));
        }
        if let Some(b) = &self.builder {
            b.validate()?;
        }
        Ok(())
    }

    /// Checks that need the header: the builder is not the order's own account, and a GTT
    /// expiry lies after the request timestamp.
    fn validate_with(&self, header: &PermitHeader) -> Result<(), PermitError> {
        self.validate()?;
        if let Some(b) = &self.builder
            && b.pubkey == header.owner
        {
            return Err(PermitError::Builder(
                "builder_pubkey must not be the order's own account".into(),
            ));
        }
        if let Some(exp) = self.expires_ts_ms
            && exp <= header.client_ts_ms
        {
            return Err(PermitError::Expiration(
                "expires_ts_ms must be after client_ts_ms for GTT orders".into(),
            ));
        }
        Ok(())
    }

    fn flags(&self) -> u8 {
        (if self.post_only { FLAG_POST_ONLY } else { 0 })
            | (if self.reduce_only {
                FLAG_REDUCE_ONLY
            } else {
                0
            })
            | (if self.allow_missing_replace {
                FLAG_ALLOW_MISSING_REPLACE
            } else {
                0
            })
            | (if self.replace_with_open_size {
                FLAG_REPLACE_WITH_OPEN_SIZE
            } else {
                0
            })
    }

    /// The JSON element of `POST /api/v1/orders` for this order, with field order and
    /// omissions as in the docs example.
    pub fn to_request(&self, auth: RequestAuth) -> PlaceOrderRequest {
        PlaceOrderRequest {
            client_order_id: self.client_order_id,
            replace_client_order_id: self.replace_client_order_id,
            symbol: self.symbol.clone(),
            side: self.side,
            price: self.price,
            size: self.size,
            tif: (self.tif != TimeInForce::Gtc).then_some(self.tif),
            order_type: (self.order_type != OrderType::Limit).then_some(self.order_type),
            post_only: self.post_only,
            reduce_only: self.reduce_only,
            allow_missing_replace: self.allow_missing_replace,
            replace_with_open_size: self.replace_with_open_size,
            expires_ts_ms: self.expires_ts_ms,
            metadata: (self.metadata != 0).then_some(self.metadata),
            builder_info: self.builder.map(|b| BuilderInfoRequest {
                builder_pubkey: base58(&b.pubkey),
                builder_fee_rate: i64::from(b.fee_rate_u),
            }),
            auth,
        }
    }

    /// The order a JSON request describes (not validated; see [`PlaceOrder::validate`]).
    pub fn from_request(req: &PlaceOrderRequest) -> Result<Self, PermitError> {
        let builder = match &req.builder_info {
            None => None,
            Some(b) => {
                let fee_rate_u = u32::try_from(b.builder_fee_rate).map_err(|_| {
                    PermitError::BuilderFeeRate(format!(
                        "{} is out of range",
                        crate::num::format_micros(b.builder_fee_rate)
                    ))
                })?;
                Some(BuilderInfo {
                    pubkey: decode_pubkey(&b.builder_pubkey, "builder_pubkey")
                        .map_err(|e| PermitError::Builder(e.to_string()))?,
                    fee_rate_u,
                })
            }
        };
        Ok(PlaceOrder {
            symbol: req.symbol.clone(),
            side: req.side,
            price: req.price,
            size: req.size,
            client_order_id: req.client_order_id,
            replace_client_order_id: req.replace_client_order_id,
            order_type: req.order_type.unwrap_or(OrderType::Limit),
            tif: req.tif.unwrap_or(TimeInForce::Gtc),
            expires_ts_ms: req.expires_ts_ms,
            post_only: req.post_only,
            reduce_only: req.reduce_only,
            allow_missing_replace: req.allow_missing_replace,
            replace_with_open_size: req.replace_with_open_size,
            metadata: req.metadata.unwrap_or(0),
            builder,
        })
    }
}

/// Encodes header + body: the exact 232 bytes the trading key signs. The header's `cmd_type`
/// must be [`CMD_PLACE_ORDER`].
pub fn encode_place_order_permit(
    header: &PermitHeader,
    order: &PlaceOrder,
) -> Result<[u8; PLACE_ORDER_PERMIT_LEN], PermitError> {
    if header.cmd_type != CMD_PLACE_ORDER {
        return Err(invalid(format!(
            "cmd_type {} is not place order",
            header.cmd_type
        )));
    }
    order.validate_with(header)?;
    let mut b = [0u8; PLACE_ORDER_PERMIT_LEN];
    b[..HEADER_LEN].copy_from_slice(&header.encode()?);
    let put_u64 = |b: &mut [u8], at: usize, v: u64| b[at..at + 8].copy_from_slice(&v.to_le_bytes());
    put_u64(&mut b, layout::SIZE, order.size);
    // validate() keeps the price inside (0, 1e6), so the cast is lossless.
    put_u64(&mut b, layout::PRICE, order.price as u64);
    put_u64(&mut b, layout::CLIENT_ORDER_ID, order.client_order_id);
    put_u64(
        &mut b,
        layout::REPLACE_CLIENT_ORDER_ID,
        order
            .replace_client_order_id
            .unwrap_or(OMITTED_REPLACE_CLIENT_ORDER_ID),
    );
    put_u64(
        &mut b,
        layout::EXPIRE_TS_MS,
        order.expires_ts_ms.unwrap_or(0),
    );
    if let Some(builder) = &order.builder {
        b[layout::BUILDER_PUBKEY..layout::BUILDER_PUBKEY + 32].copy_from_slice(&builder.pubkey);
        // validate() caps the rate at 100_000, so it fits an i32.
        let rate = builder.fee_rate_u as i32;
        b[layout::BUILDER_FEE_RATE..layout::BUILDER_FEE_RATE + 4]
            .copy_from_slice(&rate.to_le_bytes());
    }
    put_symbol(&mut b, layout::SYMBOL, &order.symbol);
    b[layout::OFFCHAIN_FIELDS_DIGEST..layout::OFFCHAIN_FIELDS_DIGEST + 32]
        .copy_from_slice(&OFFCHAIN_FIELDS_DIGEST);
    b[layout::METADATA] = order.metadata;
    b[layout::SIDE] = side_byte(order.side);
    b[layout::ORDER_TYPE] = order_type_byte(order.order_type);
    b[layout::TIF] = tif_byte(order.tif);
    b[layout::FLAGS] = order.flags();
    Ok(b)
}

/// Decodes and validates a 232-byte place-order permit.
pub fn decode_place_order_permit(permit: &[u8]) -> Result<(PermitHeader, PlaceOrder), PermitError> {
    if permit.len() != PLACE_ORDER_PERMIT_LEN {
        return Err(invalid(format!(
            "place-order permit must be {PLACE_ORDER_PERMIT_LEN} bytes, got {}",
            permit.len()
        )));
    }
    let header = PermitHeader::decode(permit)?;
    if header.cmd_type != CMD_PLACE_ORDER {
        return Err(invalid(format!(
            "cmd_type {} is not place order",
            header.cmd_type
        )));
    }
    if permit[layout::OFFCHAIN_FIELDS_DIGEST..layout::OFFCHAIN_FIELDS_DIGEST + 32]
        != OFFCHAIN_FIELDS_DIGEST
    {
        return Err(invalid("unknown offchain_fields_digest"));
    }
    zeros(&permit[layout::PADDING1..], "padding1")?;
    let side = match permit[layout::SIDE] {
        0 => Side::Bid,
        1 => Side::Ask,
        v => return Err(invalid(format!("side byte {v}"))),
    };
    let order_type = match permit[layout::ORDER_TYPE] {
        0 => OrderType::Limit,
        1 => OrderType::Market,
        v => return Err(invalid(format!("order_type byte {v}"))),
    };
    let tif = match permit[layout::TIF] {
        0 => TimeInForce::Gtc,
        1 => TimeInForce::Gtt,
        2 => TimeInForce::Ioc,
        v => return Err(invalid(format!("tif byte {v}"))),
    };
    let flags = permit[layout::FLAGS];
    if flags & !0x0f != 0 {
        return Err(invalid(format!("unknown flag bits {flags:#04x}")));
    }
    let builder_key: Pubkey = permit[layout::BUILDER_PUBKEY..layout::BUILDER_PUBKEY + 32]
        .try_into()
        .expect("32 bytes");
    let rate = i32::from_le_bytes(
        permit[layout::BUILDER_FEE_RATE..layout::BUILDER_FEE_RATE + 4]
            .try_into()
            .expect("4 bytes"),
    );
    let builder = if builder_key == [0; 32] {
        if rate != 0 {
            return Err(PermitError::Builder(
                "builder_fee_rate set without a builder_pubkey".into(),
            ));
        }
        None
    } else {
        Some(BuilderInfo {
            pubkey: builder_key,
            fee_rate_u: u32::try_from(rate)
                .map_err(|_| PermitError::BuilderFeeRate(format!("negative rate {rate}")))?,
        })
    };
    let price = u64_at(permit, layout::PRICE);
    let replace = u64_at(permit, layout::REPLACE_CLIENT_ORDER_ID);
    let expire = u64_at(permit, layout::EXPIRE_TS_MS);
    let order = PlaceOrder {
        symbol: read_symbol(permit, layout::SYMBOL)?,
        side,
        price: i64::try_from(price)
            .map_err(|_| PermitError::Price(format!("price {price} out of range")))?,
        size: u64_at(permit, layout::SIZE),
        client_order_id: u64_at(permit, layout::CLIENT_ORDER_ID),
        replace_client_order_id: (replace != OMITTED_REPLACE_CLIENT_ORDER_ID).then_some(replace),
        order_type,
        tif,
        expires_ts_ms: (expire != 0).then_some(expire),
        post_only: flags & FLAG_POST_ONLY != 0,
        reduce_only: flags & FLAG_REDUCE_ONLY != 0,
        allow_missing_replace: flags & FLAG_ALLOW_MISSING_REPLACE != 0,
        replace_with_open_size: flags & FLAG_REPLACE_WITH_OPEN_SIZE != 0,
        metadata: permit[layout::METADATA],
        builder,
    };
    order.validate_with(&header)?;
    Ok((header, order))
}

// ---------- cancel ----------

/// Which id a cancel names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelTarget {
    /// By client order id (`cancel_by` 0).
    ClientOrderId(u64),
    /// By exchange order id (`cancel_by` 1).
    OrderId(u64),
}

/// A cancel before signing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CancelOrder {
    /// The order to cancel.
    pub target: CancelTarget,
    /// Client metadata byte at offset 81 (0; the Python quickstart writes 2).
    pub metadata: u8,
}

impl CancelOrder {
    /// Cancel by client order id.
    pub fn by_client_order_id(id: u64) -> Self {
        CancelOrder {
            target: CancelTarget::ClientOrderId(id),
            metadata: 0,
        }
    }

    /// Cancel by exchange order id.
    pub fn by_order_id(id: u64) -> Self {
        CancelOrder {
            target: CancelTarget::OrderId(id),
            metadata: 0,
        }
    }

    fn to_request(self, auth: RequestAuth) -> CancelOrderRequest {
        let (client_order_id, order_id) = match self.target {
            CancelTarget::ClientOrderId(id) => (Some(id), None),
            CancelTarget::OrderId(id) => (None, Some(id)),
        };
        CancelOrderRequest {
            client_order_id,
            order_id,
            metadata: (self.metadata != 0).then_some(self.metadata),
            auth,
        }
    }

    fn from_request(req: &CancelOrderRequest) -> Result<Self, PermitError> {
        let target = match (req.client_order_id, req.order_id) {
            (Some(id), None) => CancelTarget::ClientOrderId(id),
            (None, Some(id)) => CancelTarget::OrderId(id),
            _ => {
                return Err(invalid(
                    "a cancel names exactly one of client_order_id and order_id",
                ));
            }
        };
        Ok(CancelOrder {
            target,
            metadata: req.metadata.unwrap_or(0),
        })
    }
}

/// Encodes a 96-byte cancel permit (`cmd_type` must be [`CMD_CANCEL_ORDER`]).
pub fn encode_cancel_permit(
    header: &PermitHeader,
    cancel: &CancelOrder,
) -> Result<[u8; CANCEL_PERMIT_LEN], PermitError> {
    if header.cmd_type != CMD_CANCEL_ORDER {
        return Err(invalid(format!(
            "cmd_type {} is not cancel",
            header.cmd_type
        )));
    }
    let mut b = [0u8; CANCEL_PERMIT_LEN];
    b[..HEADER_LEN].copy_from_slice(&header.encode()?);
    let (by, id) = match cancel.target {
        CancelTarget::ClientOrderId(id) => (0, id),
        CancelTarget::OrderId(id) => (1, id),
    };
    b[80] = by;
    b[81] = cancel.metadata;
    b[88..96].copy_from_slice(&id.to_le_bytes());
    Ok(b)
}

/// Decodes a 96-byte cancel permit.
pub fn decode_cancel_permit(permit: &[u8]) -> Result<(PermitHeader, CancelOrder), PermitError> {
    if permit.len() != CANCEL_PERMIT_LEN {
        return Err(invalid(format!(
            "cancel permit must be {CANCEL_PERMIT_LEN} bytes, got {}",
            permit.len()
        )));
    }
    let header = PermitHeader::decode(permit)?;
    if header.cmd_type != CMD_CANCEL_ORDER {
        return Err(invalid(format!(
            "cmd_type {} is not cancel",
            header.cmd_type
        )));
    }
    zeros(&permit[82..88], "cancel padding")?;
    let id = u64_at(permit, 88);
    let target = match permit[80] {
        0 => CancelTarget::ClientOrderId(id),
        1 => CancelTarget::OrderId(id),
        v => return Err(invalid(format!("cancel_by byte {v}"))),
    };
    Ok((
        header,
        CancelOrder {
            target,
            metadata: permit[81],
        },
    ))
}

// ---------- mass cancel ----------

/// A mass cancel (every resting order of the account in one market) before signing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MassCancel {
    /// Market symbol.
    pub symbol: String,
    /// Client metadata byte at offset 80.
    pub metadata: u8,
}

impl MassCancel {
    /// Mass cancel of `symbol`.
    pub fn new(symbol: impl Into<String>) -> Self {
        MassCancel {
            symbol: symbol.into(),
            metadata: 0,
        }
    }
}

/// Encodes a 120-byte mass-cancel permit (`cmd_type` must be [`CMD_MASS_CANCEL`]).
pub fn encode_mass_cancel_permit(
    header: &PermitHeader,
    mc: &MassCancel,
) -> Result<[u8; MASS_CANCEL_PERMIT_LEN], PermitError> {
    if header.cmd_type != CMD_MASS_CANCEL {
        return Err(invalid(format!(
            "cmd_type {} is not mass cancel",
            header.cmd_type
        )));
    }
    check_symbol(&mc.symbol)?;
    let mut b = [0u8; MASS_CANCEL_PERMIT_LEN];
    b[..HEADER_LEN].copy_from_slice(&header.encode()?);
    b[80] = mc.metadata;
    put_symbol(&mut b, 88, &mc.symbol);
    Ok(b)
}

/// Decodes a 120-byte mass-cancel permit.
pub fn decode_mass_cancel_permit(permit: &[u8]) -> Result<(PermitHeader, MassCancel), PermitError> {
    if permit.len() != MASS_CANCEL_PERMIT_LEN {
        return Err(invalid(format!(
            "mass-cancel permit must be {MASS_CANCEL_PERMIT_LEN} bytes, got {}",
            permit.len()
        )));
    }
    let header = PermitHeader::decode(permit)?;
    if header.cmd_type != CMD_MASS_CANCEL {
        return Err(invalid(format!(
            "cmd_type {} is not mass cancel",
            header.cmd_type
        )));
    }
    zeros(&permit[81..88], "mass-cancel padding")?;
    Ok((
        header,
        MassCancel {
            symbol: read_symbol(permit, 88)?,
            metadata: permit[80],
        },
    ))
}

// ---------- JSON requests ----------

/// `RequestAuth`: the `auth` object of every signed request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestAuth {
    /// Client timestamp, Unix ms.
    #[serde(with = "str_num")]
    pub client_ts_ms: u64,
    /// Freshness window.
    #[serde(with = "str_num")]
    pub recv_window_ms: u32,
    /// Owner (base58).
    pub owner: String,
    /// Signer (base58).
    pub signer: String,
    /// Ed25519 signature over the permit (base58).
    pub signature: String,
}

impl RequestAuth {
    fn header(&self, cmd_type: u8, deployment_id: u8) -> Result<PermitHeader, PermitError> {
        Ok(PermitHeader {
            version: PERMIT_VERSION,
            cmd_type,
            deployment_id,
            recv_window_ms: self.recv_window_ms,
            client_ts_ms: self.client_ts_ms,
            owner: decode_pubkey(&self.owner, "owner")?,
            signer: decode_pubkey(&self.signer, "signer")
                .map_err(|e| PermitError::Unauthorized(e.to_string()))?,
        })
    }
}

/// `BuilderInfo` as sent in a place-order request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuilderInfoRequest {
    /// Builder account (base58).
    pub builder_pubkey: String,
    /// Fee rate, micro-units (`"0.001000"` on the wire).
    #[serde(with = "micros")]
    pub builder_fee_rate: MicroUsd,
}

/// One element of the `POST /api/v1/orders` body (`PlaceOrderRequest`). Field order and
/// omissions follow the docs example, so a request serialises byte for byte as Pascal's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceOrderRequest {
    /// Client order id.
    #[serde(with = "str_num")]
    pub client_order_id: u64,
    /// Order replaced atomically.
    #[serde(default, with = "opt_str_num", skip_serializing_if = "Option::is_none")]
    pub replace_client_order_id: Option<u64>,
    /// Market symbol.
    pub symbol: String,
    /// Side.
    pub side: Side,
    /// Limit price, micro-USD (`"0.550000"` on the wire).
    #[serde(with = "micros")]
    pub price: MicroUsd,
    /// Contracts.
    #[serde(with = "str_num")]
    pub size: u64,
    /// Time in force (omitted = GTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tif: Option<TimeInForce>,
    /// Order type (omitted = LIMIT).
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub order_type: Option<OrderType>,
    /// Maker only.
    #[serde(default, skip_serializing_if = "is_false")]
    pub post_only: bool,
    /// Reduce only.
    #[serde(default, skip_serializing_if = "is_false")]
    pub reduce_only: bool,
    /// Place even if the replaced order is missing.
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_missing_replace: bool,
    /// Keep the replaced order's open size.
    #[serde(default, skip_serializing_if = "is_false")]
    pub replace_with_open_size: bool,
    /// GTT expiry, Unix ms.
    #[serde(default, with = "opt_str_num", skip_serializing_if = "Option::is_none")]
    pub expires_ts_ms: Option<u64>,
    /// Client metadata byte (omitted when 0).
    #[serde(default, with = "opt_str_num", skip_serializing_if = "Option::is_none")]
    pub metadata: Option<u8>,
    /// Builder code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builder_info: Option<BuilderInfoRequest>,
    /// Signed auth.
    pub auth: RequestAuth,
}

/// One element of the `POST /api/v1/cancels` body (`CancelOrderRequest`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelOrderRequest {
    /// Client id of the order (one of the two ids).
    #[serde(default, with = "opt_str_num", skip_serializing_if = "Option::is_none")]
    pub client_order_id: Option<u64>,
    /// Exchange id of the order (one of the two ids).
    #[serde(default, with = "opt_str_num", skip_serializing_if = "Option::is_none")]
    pub order_id: Option<u64>,
    /// Client metadata byte (omitted when 0).
    #[serde(default, with = "opt_str_num", skip_serializing_if = "Option::is_none")]
    pub metadata: Option<u8>,
    /// Signed auth.
    pub auth: RequestAuth,
}

/// One element of the `POST /api/v1/mass-cancels` body (`MassCancelRequest`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MassCancelRequest {
    /// Market whose resting orders are cancelled.
    pub symbol: String,
    /// Client metadata byte (omitted when 0).
    #[serde(default, with = "opt_str_num", skip_serializing_if = "Option::is_none")]
    pub metadata: Option<u8>,
    /// Signed auth.
    pub auth: RequestAuth,
}

fn is_false(b: &bool) -> bool {
    !*b
}

// ---------- signer ----------

/// A signed request: the JSON element to send, the exact permit bytes and the signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signed<R> {
    /// The JSON request element.
    pub request: R,
    /// The permit bytes that were signed.
    pub permit: Vec<u8>,
    /// The Ed25519 signature.
    pub signature: [u8; 64],
}

impl<R> Signed<R> {
    /// The permit as lower-case hex (what the docs call `message_hex`).
    pub fn permit_hex(&self) -> String {
        to_hex(&self.permit)
    }

    /// The signature in base58.
    pub fn signature_base58(&self) -> String {
        base58(&self.signature)
    }
}

/// A trading key. `Debug` prints only the public key.
#[derive(Clone)]
pub struct TradingSigner {
    key: SigningKey,
}

impl fmt::Debug for TradingSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TradingSigner")
            .field("pubkey", &self.pubkey_base58())
            .finish_non_exhaustive()
    }
}

impl TradingSigner {
    /// From a 32-byte Ed25519 seed.
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        TradingSigner {
            key: SigningKey::from_bytes(seed),
        }
    }

    /// From raw key bytes: a 32-byte seed or a 64-byte Solana secret key (`seed || pubkey`,
    /// whose public half must match the seed).
    pub fn from_bytes(raw: &[u8]) -> Result<Self, PermitError> {
        let seed: [u8; 32] = raw
            .get(..32)
            .and_then(|s| s.try_into().ok())
            .ok_or_else(|| invalid("secret key shorter than 32 bytes"))?;
        let signer = Self::from_seed(&seed);
        match raw.len() {
            32 => Ok(signer),
            64 if raw[32..] == signer.pubkey() => Ok(signer),
            64 => Err(invalid(
                "64-byte secret key: the public half does not match the seed",
            )),
            n => Err(invalid(format!(
                "expected a 32-byte seed or a 64-byte secret key, got {n} bytes"
            ))),
        }
    }

    /// From text as Pascal and Solana hand keys out: a JSON byte array (Solana keypair file),
    /// 64 hex characters (seed), or base58 (seed or 64-byte secret key).
    pub fn from_secret_str(text: &str) -> Result<Self, PermitError> {
        let s = text.trim();
        let mut raw: Vec<u8> = if s.starts_with('[') {
            serde_json::from_str(s)
                .map_err(|_| invalid("JSON secret key must be an array of byte values"))?
        } else if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
            from_hex(s)?
        } else {
            bs58::decode(s)
                .into_vec()
                .map_err(|_| invalid("secret key is neither a JSON array, hex nor base58"))?
        };
        let out = Self::from_bytes(&raw);
        raw.fill(0);
        out
    }

    /// Reads a key file in any format [`TradingSigner::from_secret_str`] accepts.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, PermitError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|e| invalid(format!("cannot read key file {}: {e}", path.display())))?;
        let out = Self::from_secret_str(&text);
        // Best-effort wipe of our copy of the key text.
        let mut bytes = text.into_bytes();
        bytes.fill(0);
        out
    }

    /// Public key.
    pub fn pubkey(&self) -> Pubkey {
        self.key.verifying_key().to_bytes()
    }

    /// Public key in base58.
    pub fn pubkey_base58(&self) -> String {
        base58(&self.pubkey())
    }

    /// Signs arbitrary bytes.
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.key.sign(message).to_bytes()
    }

    /// Signs a place order.
    pub fn sign_place_order(
        &self,
        auth: &AuthParams,
        order: &PlaceOrder,
    ) -> Result<Signed<PlaceOrderRequest>, PermitError> {
        let header = auth.header(CMD_PLACE_ORDER, self.pubkey());
        let permit = encode_place_order_permit(&header, order)?;
        let signature = self.sign(&permit);
        Ok(Signed {
            request: order.to_request(header.auth_json(&signature)),
            permit: permit.to_vec(),
            signature,
        })
    }

    /// Signs a cancel.
    pub fn sign_cancel(
        &self,
        auth: &AuthParams,
        cancel: &CancelOrder,
    ) -> Result<Signed<CancelOrderRequest>, PermitError> {
        let header = auth.header(CMD_CANCEL_ORDER, self.pubkey());
        let permit = encode_cancel_permit(&header, cancel)?;
        let signature = self.sign(&permit);
        Ok(Signed {
            request: cancel.to_request(header.auth_json(&signature)),
            permit: permit.to_vec(),
            signature,
        })
    }

    /// Signs a mass cancel.
    pub fn sign_mass_cancel(
        &self,
        auth: &AuthParams,
        mc: &MassCancel,
    ) -> Result<Signed<MassCancelRequest>, PermitError> {
        let header = auth.header(CMD_MASS_CANCEL, self.pubkey());
        let permit = encode_mass_cancel_permit(&header, mc)?;
        let signature = self.sign(&permit);
        Ok(Signed {
            request: MassCancelRequest {
                symbol: mc.symbol.clone(),
                metadata: (mc.metadata != 0).then_some(mc.metadata),
                auth: header.auth_json(&signature),
            },
            permit: permit.to_vec(),
            signature,
        })
    }
}

// ---------- verification ----------

/// A request whose signature checked out, with the decoded fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified<T> {
    /// The permit header (owner, signer, timestamp, window, deployment).
    pub header: PermitHeader,
    /// The decoded command.
    pub body: T,
    /// The permit bytes the signature covers.
    pub permit: Vec<u8>,
    /// The signature.
    pub signature: [u8; 64],
}

impl<T> Verified<T> {
    /// Fails with `UNAUTHORIZED` unless the request was signed by `trading_key`.
    pub fn require_signer(self, trading_key: &Pubkey) -> Result<Self, PermitError> {
        if &self.header.signer == trading_key {
            Ok(self)
        } else {
            Err(PermitError::Unauthorized(format!(
                "signed by {}, expected {}",
                base58(&self.header.signer),
                base58(trading_key)
            )))
        }
    }
}

/// Verifies an Ed25519 signature (strict: rejects small-order keys and malleable signatures).
pub fn verify_signature(
    message: &[u8],
    signature: &[u8; 64],
    signer: &Pubkey,
) -> Result<(), PermitError> {
    let key = VerifyingKey::from_bytes(signer)
        .map_err(|e| PermitError::Unauthorized(format!("signer is not an Ed25519 key: {e}")))?;
    key.verify_strict(message, &Signature::from_bytes(signature))
        .map_err(|_| {
            PermitError::Unauthorized(format!(
                "signature does not verify for signer {}",
                base58(signer)
            ))
        })
}

/// Rebuilds the place-order permit from a JSON request for `deployment_id`, verifies the
/// signature against `auth.signer`, and returns the decoded order. The caller still decides
/// whether `auth.signer` is a trading key of `auth.owner` ([`Verified::require_signer`]) and
/// whether the request is fresh ([`PermitHeader::check_fresh`]).
pub fn verify_place_order(
    req: &PlaceOrderRequest,
    deployment_id: u8,
) -> Result<Verified<PlaceOrder>, PermitError> {
    let header = req.auth.header(CMD_PLACE_ORDER, deployment_id)?;
    let order = PlaceOrder::from_request(req)?;
    let permit = encode_place_order_permit(&header, &order)?;
    let signature = decode_signature(&req.auth.signature)?;
    verify_signature(&permit, &signature, &header.signer)?;
    Ok(Verified {
        header,
        body: order,
        permit: permit.to_vec(),
        signature,
    })
}

/// [`verify_place_order`] for a cancel.
pub fn verify_cancel(
    req: &CancelOrderRequest,
    deployment_id: u8,
) -> Result<Verified<CancelOrder>, PermitError> {
    let header = req.auth.header(CMD_CANCEL_ORDER, deployment_id)?;
    let cancel = CancelOrder::from_request(req)?;
    let permit = encode_cancel_permit(&header, &cancel)?;
    let signature = decode_signature(&req.auth.signature)?;
    verify_signature(&permit, &signature, &header.signer)?;
    Ok(Verified {
        header,
        body: cancel,
        permit: permit.to_vec(),
        signature,
    })
}

/// [`verify_place_order`] for a mass cancel.
pub fn verify_mass_cancel(
    req: &MassCancelRequest,
    deployment_id: u8,
) -> Result<Verified<MassCancel>, PermitError> {
    let header = req.auth.header(CMD_MASS_CANCEL, deployment_id)?;
    let mc = MassCancel {
        symbol: req.symbol.clone(),
        metadata: req.metadata.unwrap_or(0),
    };
    let permit = encode_mass_cancel_permit(&header, &mc)?;
    let signature = decode_signature(&req.auth.signature)?;
    verify_signature(&permit, &signature, &header.signer)?;
    Ok(Verified {
        header,
        body: mc,
        permit: permit.to_vec(),
        signature,
    })
}

/// Parses a whole `POST /api/v1/orders` body (a JSON array) and verifies each element on its
/// own, as Pascal does. The outer error is for a body that is not a JSON array at all
/// (`INVALID_JSON` / `INVALID_REQUEST`).
pub fn verify_place_order_body(
    body: &[u8],
    deployment_id: u8,
) -> Result<Vec<Result<Verified<PlaceOrder>, PermitError>>, PermitError> {
    let items: Vec<serde_json::Value> = serde_json::from_slice(body)
        .map_err(|e| invalid(format!("body is not a JSON array: {e}")))?;
    Ok(items
        .into_iter()
        .map(|v| {
            let req: PlaceOrderRequest =
                serde_json::from_value(v).map_err(|e| invalid(format!("malformed order: {e}")))?;
            verify_place_order(&req, deployment_id)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner() -> Pubkey {
        TradingSigner::from_seed(&[7; 32]).pubkey()
    }

    #[test]
    fn validation_mirrors_the_reference() {
        let base = PlaceOrder::limit("A.B", Side::Bid, 500_000, 1, 1);
        assert!(base.validate().is_ok());
        let bad = |f: &dyn Fn(&mut PlaceOrder)| {
            let mut o = base.clone();
            f(&mut o);
            o.validate().unwrap_err()
        };
        assert!(matches!(bad(&|o| o.size = 0), PermitError::Size(_)));
        assert!(matches!(bad(&|o| o.price = 0), PermitError::Price(_)));
        assert!(matches!(
            bad(&|o| o.price = 1_000_000),
            PermitError::Price(_)
        ));
        assert!(matches!(
            bad(&|o| o.client_order_id = u64::MAX),
            PermitError::Invalid(_)
        ));
        assert!(matches!(
            bad(&|o| o.tif = TimeInForce::Gtt),
            PermitError::Expiration(_)
        ));
        assert!(matches!(
            bad(&|o| o.expires_ts_ms = Some(5)),
            PermitError::Expiration(_)
        ));
        assert!(matches!(
            bad(&|o| o.order_type = OrderType::Market),
            PermitError::Invalid(_)
        ));
        assert!(matches!(
            bad(&|o| {
                o.tif = TimeInForce::Ioc;
                o.post_only = true;
            }),
            PermitError::Invalid(_)
        ));
        assert!(matches!(
            bad(&|o| o.allow_missing_replace = true),
            PermitError::Invalid(_)
        ));
        assert!(matches!(
            bad(&|o| o.symbol = String::new()),
            PermitError::Invalid(_)
        ));
        assert!(matches!(
            bad(&|o| o.symbol = "A B".into()),
            PermitError::Invalid(_)
        ));
        assert!(matches!(
            bad(&|o| o.symbol = "X".repeat(33)),
            PermitError::Invalid(_)
        ));
        assert!(matches!(
            bad(&|o| o.builder = Some(BuilderInfo {
                pubkey: [1; 32],
                fee_rate_u: 100_001
            })),
            PermitError::BuilderFeeRate(_)
        ));
        assert!(matches!(
            bad(&|o| o.builder = Some(BuilderInfo {
                pubkey: [0; 32],
                fee_rate_u: 1_000
            })),
            PermitError::Builder(_)
        ));
    }

    #[test]
    fn builder_cannot_be_the_owner_and_gtt_must_follow_client_ts() {
        let signer = TradingSigner::from_seed(&[8; 32]);
        let auth = AuthParams::new(owner(), 1_000);
        let o = PlaceOrder::limit("A.B", Side::Bid, 500_000, 1, 1).with_builder(BuilderInfo {
            pubkey: owner(),
            fee_rate_u: 1_000,
        });
        let e = signer.sign_place_order(&auth, &o).unwrap_err();
        assert_eq!(e.api_code(), ApiErrorCode::BuilderInvalid);
        let mut o = PlaceOrder::limit("A.B", Side::Bid, 500_000, 1, 1);
        o.tif = TimeInForce::Gtt;
        o.expires_ts_ms = Some(1_000);
        let e = signer.sign_place_order(&auth, &o).unwrap_err();
        assert_eq!(e.api_code(), ApiErrorCode::InvalidExpiration);
        o.expires_ts_ms = Some(1_001);
        assert!(signer.sign_place_order(&auth, &o).is_ok());
        let e = signer
            .sign_place_order(
                &auth.recv_window_ms(60_001),
                &PlaceOrder::limit("A.B", Side::Bid, 1, 1, 1),
            )
            .unwrap_err();
        assert_eq!(e.api_code(), ApiErrorCode::RecvWindowTooLarge);
    }

    #[test]
    fn freshness_follows_the_docs_rule() {
        let h = AuthParams::new(owner(), 10_000).header(CMD_PLACE_ORDER, [8; 32]);
        assert!(h.check_fresh(15_000, 1_000).is_ok()); // exactly the window
        assert_eq!(
            h.check_fresh(15_001, 1_000).unwrap_err().api_code(),
            ApiErrorCode::ClientTsExpired
        );
        assert!(h.check_fresh(9_000, 1_000).is_ok());
        assert_eq!(
            h.check_fresh(8_999, 1_000).unwrap_err().api_code(),
            ApiErrorCode::ClientTsTooFarInFuture
        );
    }

    #[test]
    fn secret_key_formats() {
        let seed = [8u8; 32];
        let want = TradingSigner::from_seed(&seed).pubkey();
        let mut kp = seed.to_vec();
        kp.extend_from_slice(&want);
        let json = serde_json::to_string(&kp).unwrap();
        for text in [to_hex(&seed), base58(&seed), base58(&kp), json] {
            assert_eq!(
                TradingSigner::from_secret_str(&text).unwrap().pubkey(),
                want,
                "{text}"
            );
        }
        kp[63] ^= 1;
        assert!(TradingSigner::from_bytes(&kp).is_err());
        assert!(TradingSigner::from_bytes(&[1; 16]).is_err());
        assert!(TradingSigner::from_secret_str("not a key 0OIl").is_err());
        let dbg = format!("{:?}", TradingSigner::from_seed(&seed));
        assert!(
            dbg.contains("2KW2XRd9kwqet15Aha2oK3tYvd3nWbTFH1MBiRAv1BE1") && !dbg.contains("0808")
        );
    }

    #[test]
    fn decode_rejects_tampering() {
        let signer = TradingSigner::from_seed(&[8; 32]);
        let auth = AuthParams::new(owner(), 1_000);
        let s = signer
            .sign_place_order(&auth, &PlaceOrder::ioc("A.B", Side::Ask, 300_000, 2, 9))
            .unwrap();
        let (h, o) = decode_place_order_permit(&s.permit).unwrap();
        assert_eq!(h.signer, signer.pubkey());
        assert_eq!(o, PlaceOrder::ioc("A.B", Side::Ask, 300_000, 2, 9));
        for (at, v) in [
            (layout::VERSION, 2),
            (layout::CMD_TYPE, 2),
            (layout::PADDING0, 1),
            (layout::SIDE, 2),
            (layout::TIF, 3),
            (layout::FLAGS, 0x10),
            (layout::PADDING1 + 6, 1),
            (layout::OFFCHAIN_FIELDS_DIGEST, 0),
            (layout::SYMBOL + 5, b'X'),
            (layout::BUILDER_FEE_RATE, 1),
        ] {
            let mut p = s.permit.clone();
            p[at] = v;
            assert!(decode_place_order_permit(&p).is_err(), "byte {at} = {v}");
        }
        assert!(decode_place_order_permit(&s.permit[..231]).is_err());
    }
}
