//! The response envelope `{status, data, seq, state_time_ms, server_time_ms}`, paginated
//! pages, and access to the raw JSON beside the typed value.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{ApiError, Error, ErrorBody, Result};
use crate::num::opt_str_num;

/// A successful response: the typed `data`, the envelope's freshness fields, and the raw body.
#[derive(Debug, Clone)]
pub struct Response<T> {
    /// The typed payload.
    pub data: T,
    /// Matching-engine sequence of the state served (optional per the docs; present on reads).
    pub seq: Option<u64>,
    /// Matching-engine time of the last matching round, Unix ms.
    pub state_time_ms: Option<u64>,
    /// API server time when the response was produced, Unix ms.
    pub server_time_ms: Option<u64>,
    /// The whole response body as served, for storing beside typed columns.
    pub raw: Value,
}

impl<T> Response<T> {
    /// The raw `data` member of the envelope.
    pub fn raw_data(&self) -> &Value {
        &self.raw["data"]
    }

    /// The raw list items: `data` itself when it is an array, else `data.items`.
    ///
    /// For a strictly decoded list (`Vec<T>`, `Page<T>`) index `i` is the raw JSON of the `i`-th
    /// typed item. For a [`Lenient`] list use the index stored with each item
    /// ([`Lenient::items`]), since failed items are left out of the typed list.
    pub fn raw_items(&self) -> &[Value] {
        let data = self.raw_data();
        data.as_array()
            .or_else(|| data.get("items").and_then(Value::as_array))
            .map_or(&[], Vec::as_slice)
    }

    /// Maps the typed payload, keeping the envelope and raw body.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Response<U> {
        Response {
            data: f(self.data),
            seq: self.seq,
            state_time_ms: self.state_time_ms,
            server_time_ms: self.server_time_ms,
            raw: self.raw,
        }
    }
}

/// A page of a cursor-paginated endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Page<T> {
    /// Items of this page, in the endpoint's order (newest first for the tape).
    pub items: Vec<T>,
    /// Cursor for the next page; pass it back as `before_cursor`. `None` on the last page.
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// A list decoded item by item: items that fail to decode are reported instead of failing the
/// whole response. Used for the ~3,000-entry market lists, where one market with a new enum
/// value should not hide the other 2,999. Failures are never silent.
///
/// Every decoded item keeps its index in the raw list, so `raw_items()[index]` is its raw JSON
/// even when earlier items failed:
///
/// ```no_run
/// # async fn demo(client: pascal_rs::Client) -> pascal_rs::Result<()> {
/// let resp = client.markets_lenient().await?;
/// for (index, market) in &resp.data.items {
///     let raw = &resp.raw_items()[*index];
///     println!("{} {raw}", market.symbol);
/// }
/// for f in &resp.data.failures {
///     eprintln!("market #{} did not decode: {}", f.index, f.error);
/// }
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lenient<T> {
    /// Items that decoded, each with its index in the raw list, in list order.
    pub items: Vec<(usize, T)>,
    /// Items that did not.
    pub failures: Vec<ItemFailure>,
}

/// One list item that failed to decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemFailure {
    /// Index into the raw list.
    pub index: usize,
    /// The serde error message.
    pub error: String,
}

impl<T: DeserializeOwned> Lenient<T> {
    /// Decodes a JSON array element by element, borrowing each element (no copy of the list).
    pub fn from_value(v: &Value) -> Result<Self, serde_json::Error> {
        let arr = v.as_array().ok_or_else(|| {
            serde::de::Error::invalid_type(unexpected(v), &"an array of list items")
        })?;
        let mut items = Vec::with_capacity(arr.len());
        let mut failures = Vec::new();
        for (index, el) in arr.iter().enumerate() {
            match T::deserialize(el) {
                Ok(t) => items.push((index, t)),
                Err(e) => failures.push(ItemFailure {
                    index,
                    error: e.to_string(),
                }),
            }
        }
        Ok(Lenient { items, failures })
    }
}

impl<T> Lenient<T> {
    /// The decoded items without their indices.
    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.items.iter().map(|(_, t)| t)
    }

    /// Consumes the list, dropping the indices.
    pub fn into_values(self) -> Vec<T> {
        self.items.into_iter().map(|(_, t)| t).collect()
    }
}

fn unexpected(v: &Value) -> serde::de::Unexpected<'_> {
    use serde::de::Unexpected;
    match v {
        Value::Null => Unexpected::Unit,
        Value::Bool(b) => Unexpected::Bool(*b),
        Value::Number(_) => Unexpected::Other("number"),
        Value::String(s) => Unexpected::Str(s),
        Value::Array(_) => Unexpected::Seq,
        Value::Object(_) => Unexpected::Map,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Status {
    Success,
    Error,
}

#[derive(Deserialize)]
struct Head {
    status: Status,
    #[serde(default, with = "opt_str_num")]
    seq: Option<u64>,
    #[serde(default, with = "opt_str_num")]
    state_time_ms: Option<u64>,
    #[serde(default, with = "opt_str_num")]
    server_time_ms: Option<u64>,
}

/// Decodes a response body into a typed [`Response`], or the error envelope into
/// [`Error::Api`].
///
/// `http_status` is the HTTP status (use 200 for recorded fixtures); `context` names the
/// endpoint in decode errors. This is what the client uses internally and is public so that
/// recorded bodies (fixtures, raw rows) can be decoded the same way.
pub fn decode_response<T: DeserializeOwned>(
    http_status: u16,
    body: &[u8],
    context: &str,
) -> Result<Response<T>> {
    decode_with(http_status, body, context, |data| T::deserialize(data))
}

/// Like [`decode_response`] for a list `data`, decoded item by item into a [`Lenient`].
pub fn decode_lenient_response<T: DeserializeOwned>(
    http_status: u16,
    body: &[u8],
    context: &str,
) -> Result<Response<Lenient<T>>> {
    decode_with(http_status, body, context, Lenient::from_value)
}

/// Decodes the envelope and hands the `data` member to `decode`.
pub(crate) fn decode_with<R>(
    http_status: u16,
    body: &[u8],
    context: &str,
    decode: impl FnOnce(&Value) -> Result<R, serde_json::Error>,
) -> Result<Response<R>> {
    let ok = (200..300).contains(&http_status);
    let raw: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(source) if ok => {
            return Err(Error::Decode {
                context: context.to_owned(),
                source,
            });
        }
        Err(_) => return Err(http_status_error(http_status, context, body)),
    };
    let head = match Head::deserialize(&raw) {
        Ok(h) => h,
        Err(source) if ok => {
            return Err(Error::Decode {
                context: context.to_owned(),
                source,
            });
        }
        Err(_) => return Err(http_status_error(http_status, context, body)),
    };
    match head.status {
        Status::Error => {
            let body = ErrorBody::deserialize(&raw["data"]).map_err(|source| Error::Decode {
                context: format!("{context} (error envelope)"),
                source,
            })?;
            Err(Error::Api(ApiError {
                http_status: Some(http_status),
                code: body.code,
                message: body.message,
                seq: head.seq,
                server_time_ms: head.server_time_ms,
            }))
        }
        Status::Success => {
            let data = raw.get("data").ok_or_else(|| Error::Decode {
                context: context.to_owned(),
                source: serde::de::Error::missing_field("data"),
            })?;
            let data = decode(data).map_err(|source| Error::Decode {
                context: context.to_owned(),
                source,
            })?;
            Ok(Response {
                data,
                seq: head.seq,
                state_time_ms: head.state_time_ms,
                server_time_ms: head.server_time_ms,
                raw,
            })
        }
    }
}

fn http_status_error(status: u16, url: &str, body: &[u8]) -> Error {
    let text = String::from_utf8_lossy(&body[..body.len().min(300)]).into_owned();
    Error::HttpStatus {
        status,
        url: url.to_owned(),
        body: text,
    }
}
