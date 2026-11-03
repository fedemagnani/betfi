//! Error types: transport failures, Pascal's error envelope, and decode failures.

use serde::{Deserialize, Serialize};

/// Result alias used throughout the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything that can go wrong talking to Pascal.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Pascal answered with its error envelope (`{"status":"error","data":{code,message}}`).
    #[error(transparent)]
    Api(#[from] ApiError),
    /// A non-success HTTP status whose body is not Pascal's error envelope (e.g. a proxy page).
    #[error("HTTP {status} from {url}: {body}")]
    HttpStatus {
        /// HTTP status code.
        status: u16,
        /// Request URL.
        url: String,
        /// First bytes of the body, for debugging.
        body: String,
    },
    /// Connection, TLS or timeout failure from the HTTP stack.
    #[error("http transport error: {0}")]
    Transport(#[from] reqwest::Error),
    /// The body was valid JSON but did not match the typed model (an API shape change).
    #[error("cannot decode {context}: {source}")]
    Decode {
        /// What was being decoded (endpoint path or WebSocket channel).
        context: String,
        /// The underlying serde error, with the JSON path where it failed.
        #[source]
        source: serde_json::Error,
    },
    /// Invalid argument passed by the caller (e.g. more than 120 symbols for `/trades`).
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// Retries were exhausted; carries the last error.
    #[error("gave up after {attempts} attempts: {last}")]
    RetriesExhausted {
        /// Number of attempts made.
        attempts: u32,
        /// The error of the final attempt.
        last: Box<Error>,
    },
}

impl Error {
    /// Pascal's error code when this is (or wraps) an API error envelope.
    pub fn api_code(&self) -> Option<&str> {
        match self {
            Error::Api(e) => Some(&e.code),
            Error::RetriesExhausted { last, .. } => last.api_code(),
            _ => None,
        }
    }

    /// The HTTP status, when the error came from an HTTP response.
    pub fn http_status(&self) -> Option<u16> {
        match self {
            Error::Api(e) => e.http_status,
            Error::HttpStatus { status, .. } => Some(*status),
            Error::Transport(e) => e.status().map(|s| s.as_u16()),
            Error::RetriesExhausted { last, .. } => last.http_status(),
            _ => None,
        }
    }
}

/// An error that interrupted a multi-page collection, with the items collected before it.
///
/// Returned by [`Pager::collect_while`](crate::Pager::collect_while); the pager's cursor still
/// points at the page that failed, so the caller can keep `items` and resume.
#[derive(Debug)]
pub struct PartialError<T> {
    /// Items collected before the error.
    pub items: Vec<T>,
    /// The error.
    pub error: Error,
}

impl<T> std::fmt::Display for PartialError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} (after collecting {} items)",
            self.error,
            self.items.len()
        )
    }
}

impl<T: std::fmt::Debug> std::error::Error for PartialError<T> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl<T> From<PartialError<T>> for Error {
    fn from(p: PartialError<T>) -> Self {
        p.error
    }
}

/// Pascal's error envelope, mapped to a typed error.
///
/// `code` is kept as the exact string Pascal sent so that a code added after this crate was
/// written is never lost; [`ApiError::kind`] maps it to [`ApiErrorCode`] when known.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Pascal API error {code} (HTTP {http_status:?}): {message}")]
pub struct ApiError {
    /// HTTP status of the response (`None` for WebSocket error frames).
    pub http_status: Option<u16>,
    /// Stable machine-readable code, e.g. `RATE_LIMITED`.
    pub code: String,
    /// Human-readable message; may change over time.
    pub message: String,
    /// Envelope `seq`, absent when the request never reached the matching engine.
    pub seq: Option<u64>,
    /// Envelope `server_time_ms`, absent on some validation and rate-limit errors.
    pub server_time_ms: Option<u64>,
}

impl ApiError {
    /// The typed error code, or `None` for a code this crate does not know yet.
    pub fn kind(&self) -> Option<ApiErrorCode> {
        ApiErrorCode::parse(&self.code)
    }
}

/// The `data` object of an error envelope or WebSocket error frame.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ErrorBody {
    /// Stable machine-readable code.
    pub code: String,
    /// Human-readable message.
    pub message: String,
}

macro_rules! error_codes {
    ($($(#[$doc:meta])* $variant:ident = $s:literal,)*) => {
        /// Pascal's documented error codes (`ApiErrorCode` in the API reference).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
        pub enum ApiErrorCode {
            $($(#[$doc])* #[serde(rename = $s)] $variant,)*
        }

        impl ApiErrorCode {
            /// Maps the wire string to a code, `None` when unknown.
            pub fn parse(s: &str) -> Option<Self> {
                match s {
                    $($s => Some(Self::$variant),)*
                    _ => None,
                }
            }

            /// The wire string of this code.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $s,)*
                }
            }
        }
    };
}

error_codes! {
    /// Body is not valid JSON.
    InvalidJson = "INVALID_JSON",
    /// Request failed validation (bad field, out-of-range value).
    InvalidRequest = "INVALID_REQUEST",
    /// Signature missing or invalid.
    Unauthorized = "UNAUTHORIZED",
    /// Unknown symbol.
    SymbolNotFound = "SYMBOL_NOT_FOUND",
    /// Unknown account owner.
    AccountNotFound = "ACCOUNT_NOT_FOUND",
    /// Account restricted by compliance policy.
    AccountRestricted = "ACCOUNT_RESTRICTED",
    /// Restricted by geography policy.
    GeoRestricted = "GEO_RESTRICTED",
    /// Cloudflare geo headers missing or malformed.
    InvalidGeoHeaders = "INVALID_GEO_HEADERS",
    /// Order does not exist or is not open.
    OrderNotFound = "ORDER_NOT_FOUND",
    /// Trading key does not exist for the account.
    TradingKeyNotFound = "TRADING_KEY_NOT_FOUND",
    /// Same signature already received.
    DuplicateRequest = "DUPLICATE_REQUEST",
    /// Open order with the same client order id exists.
    DuplicateClientOrderId = "DUPLICATE_CLIENT_ORDER_ID",
    /// Order type not supported.
    UnsupportedOrderType = "UNSUPPORTED_ORDER_TYPE",
    /// Invalid order size.
    InvalidSize = "INVALID_SIZE",
    /// Invalid order price.
    InvalidPrice = "INVALID_PRICE",
    /// Invalid expiration for the time in force.
    InvalidExpiration = "INVALID_EXPIRATION",
    /// Post-only order would cross.
    WouldCross = "WOULD_CROSS",
    /// Would self-trade.
    SelfTrade = "SELF_TRADE",
    /// Not enough free collateral.
    InsufficientCollateral = "INSUFFICIENT_COLLATERAL",
    /// Account not funded.
    NotFunded = "NOT_FUNDED",
    /// No deposit address registered.
    DepositAddressNotRegistered = "DEPOSIT_ADDRESS_NOT_REGISTERED",
    /// Market has resolved.
    MarketResolved = "MARKET_RESOLVED",
    /// Position limit exceeded.
    PositionLimitExceeded = "POSITION_LIMIT_EXCEEDED",
    /// Too many resting orders in the market.
    RestingOrderLimitExceeded = "RESTING_ORDER_LIMIT_EXCEEDED",
    /// Maximum number of trading keys reached.
    TradingKeySlotsFull = "TRADING_KEY_SLOTS_FULL",
    /// Trading key public key already used.
    TradingKeyPubKeyAlreadyUsed = "TRADING_KEY_PUB_KEY_ALREADY_USED",
    /// Trading key name already used.
    TradingKeyNameAlreadyUsed = "TRADING_KEY_NAME_ALREADY_USED",
    /// Overwrite needs a later expiration.
    TradingKeyMustOverwriteWithGreaterExpiration = "TRADING_KEY_MUST_OVERWRITE_WITH_GREATER_EXPIRATION",
    /// Reduce-only order would increase the position.
    ReduceOnlyRejected = "REDUCE_ONLY_REJECTED",
    /// Another wallet operation is in flight.
    PendingWalletOpConflicts = "PENDING_WALLET_OP_CONFLICTS",
    /// Deposit-address registration in progress.
    PendingDepositRegistration = "PENDING_DEPOSIT_REGISTRATION",
    /// Order to replace not found.
    ReplacementOrderNotFound = "REPLACEMENT_ORDER_NOT_FOUND",
    /// Replacement is in a different market.
    ReplacementOrderDiffMarket = "REPLACEMENT_ORDER_DIFF_MARKET",
    /// Replacement is on a different side.
    ReplacementOrderDiffSide = "REPLACEMENT_ORDER_DIFF_SIDE",
    /// `recv_window_ms` too large.
    RecvWindowTooLarge = "RECV_WINDOW_TOO_LARGE",
    /// `client_ts_ms` too far in the future.
    ClientTsTooFarInFuture = "CLIENT_TS_TOO_FAR_IN_FUTURE",
    /// Request expired.
    ClientTsExpired = "CLIENT_TS_EXPIRED",
    /// Wallet-signed timestamp nonce did not increase.
    ClientTsNotIncreasing = "CLIENT_TS_NOT_INCREASING",
    /// Conflicts with existing state.
    Conflict = "CONFLICT",
    /// Rate limit exceeded; back off.
    RateLimited = "RATE_LIMITED",
    /// Server shedding load; retry shortly.
    Backpressure = "BACKPRESSURE",
    /// Service starting or catching up.
    NotReady = "NOT_READY",
    /// History not indexed up to the requested boundary; retry the same page.
    HistoryNotReady = "HISTORY_NOT_READY",
    /// Transient upstream failure.
    BadGateway = "BAD_GATEWAY",
    /// Temporarily unavailable.
    ServiceUnavailable = "SERVICE_UNAVAILABLE",
    /// Write reached the pipeline but no result could be returned.
    WriteResultUnavailable = "WRITE_RESULT_UNAVAILABLE",
    /// Request timed out.
    Timeout = "TIMEOUT",
    /// Unexpected internal error.
    InternalError = "INTERNAL_ERROR",
    /// Referral code could not be resolved.
    ReferralCodeInvalid = "REFERRAL_CODE_INVALID",
    /// Builder fee rate out of range.
    BuilderFeeRateInvalid = "BUILDER_FEE_RATE_INVALID",
    /// Builder pubkey not accepted.
    BuilderInvalid = "BUILDER_INVALID",
    /// Builder below minimum collateral.
    BuilderCollateralBelowMinimum = "BUILDER_COLLATERAL_BELOW_MINIMUM",
}

impl ApiErrorCode {
    /// Whether the docs say to retry the same request after a delay.
    pub fn is_transient(self) -> bool {
        matches!(
            self,
            Self::RateLimited
                | Self::Backpressure
                | Self::NotReady
                | Self::HistoryNotReady
                | Self::BadGateway
                | Self::ServiceUnavailable
                | Self::Timeout
        )
    }
}
