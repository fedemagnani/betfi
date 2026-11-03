//! Off-chain profiles: a handle per wallet and a slip title per own tip, each signed by the wallet
//! (l6-contracts L6-2, step 6.0c).
//!
//! | Route | What |
//! | --- | --- |
//! | `PUT /profile` | `{"wallet","handle","title","ticket","ts","sig"}`: sets the handle and, with `ticket`, that tip's title; 7.0c adds `links` and `follow` |
//! | `GET /profile/{wallet}` | `{"wallet","handle","since","verified","links","followers","following"}`; an unknown wallet has `handle: null`, never 404 |
//!
//! A tip can also be named in the bet drawer, without a handle or a signature here: the name
//! rides on `POST /quote` (`title` + `wallet`) and the indexer copies it into `tip_titles` when
//! that wallet's own tip lands with that quote (L9, `quoter::quote`, `indexer::store`).
//!
//! # The signed message
//!
//! `sig` is a base64 ed25519 signature by `wallet` over the UTF-8 bytes of [`message`]:
//!
//! ```text
//! betfi/profile/v1\n<wallet>\n<handle>\n<ticket or ->\n<title or ->\n<ts>
//! ```
//!
//! No trailing newline; `<ts>` in decimal Unix seconds; `<title>` exactly as sent (the desk
//! stores it trimmed). The fields cannot carry a newline (handle `[a-z0-9_]`, base58 wallet and
//! ticket, no control characters in a title, `-` alone refused as a title), so the framing is
//! unambiguous; the `betfi/profile/v1` tag keeps it apart from every other message the same key
//! could sign (the program's quotes start with `schedina/quote/v1`, a transaction message is
//! binary). Phantom's `signMessage` and the burner sign these bytes as they are.
//!
//! ## v2: links and follows (l7-contracts L7-1, step 7.0c)
//!
//! A request carrying `links` or `follow` is signed over [`message_v2`] instead; a request with
//! neither may be signed over either message, so every v1 client keeps working:
//!
//! ```text
//! betfi/profile/v2\n<wallet>\n<handle or ->\n<ticket or ->\n<title or ->\n<links>\n<follow>\n<ts>
//! <links>  = -   (no `links`: unchanged)
//!          | x=<x or ->;youtube=<youtube or ->;telegram=<telegram or ->;instagram=<instagram or ->
//! <follow> = -   (no `follow`) | <wallet>:on | <wallet>:off
//! ```
//!
//! No trailing newline; every value exactly as sent (a link may start with `@`, which the desk
//! drops when it stores it). A link is a bare handle or username, checked per network before the
//! signature ([`check_link`]: letters, digits and `_`, plus `.`/`-` where the network allows
//! them), so no value can carry `\n`, `;`, `=` or `:` and the framing stays unambiguous; `-`
//! alone is never a valid value. The `betfi/profile/v2` tag keeps v2 apart from v1 and from
//! every other message the key could sign. Replay protection is v1's: `ts` within the window,
//! each signature accepted once (the same `profile_sigs` table).
//!
//! `links` replaces all four (a `null` one is cleared). `follow` is idempotent both ways. The
//! handle may be empty (absent) only on a follow-only request (no `links`, `ticket` or `title`),
//! so a wallet without a handle can follow; links need a handle (as titles do, D-385). A wallet
//! follows at most [`MAX_FOLLOWING`] others and never itself.
//!
//! # Rules
//!
//! - `ts` within ±[`TS_WINDOW_S`] of the desk's clock; a signature is accepted once (replay guard).
//! - Handle `^[a-z0-9_]{3,20}$`, not reserved ([`RESERVED`]), unique case-insensitively, at most
//!   one change per [`HANDLE_EVERY_S`] (the first set counts: it starts the clock). A handle given
//!   up stays held for its previous wallet for [`RELEASED_HOLD_S`].
//! - A title: 1–[`MAX_TITLE_CHARS`] characters after trimming, no control or invisible formatting
//!   characters; once per own tip (`tipster = user`, not a copy), by that tip's wallet, while the
//!   tip is pending or open, its games in play included (D-660).
//! - Writes are limited per IP (before any check) and per wallet (after the signature verified,
//!   so nobody can spend another wallet's budget): [`IP_LIMIT`], [`WALLET_LIMIT`].
//!
//! Errors: 422 (malformed, bad signature, stale `ts`, not your tip), 409 (taken handle, change too
//! soon, title already set, tip closed, signature reused), 429 (rate limit), each with the C4
//! body `{"status":"rejected","reason"}`. All checks and writes of one request run in one
//! transaction; the `ticket` frame of a titled tip is emitted after it commits.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::Mutex;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{Extensions, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use solana_pubkey::Pubkey;
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::api::types::{
    ErrorBody, ErrorStatus, FollowRequest, ProfileLinks, ProfileRequest, ProfileResponse,
};
use crate::now_ms;
use crate::state::{AppState, DeskEvent};

/// The message tag.
pub const DOMAIN: &str = "betfi/profile/v1";
/// The tag of the v2 message (links and follows, 7.0c).
pub const DOMAIN_V2: &str = "betfi/profile/v2";
/// Most wallets one wallet may follow.
pub const MAX_FOLLOWING: i64 = 1_000;
/// `ts` may differ from the desk's clock by this much, s.
pub const TS_WINDOW_S: i64 = 300;
/// At most one handle change per this many s.
pub const HANDLE_EVERY_S: i64 = 86_400;
/// A released handle stays held for its previous wallet this long, s.
pub const RELEASED_HOLD_S: i64 = 30 * 86_400;
/// Longest title, characters (Unicode scalar values) after trimming.
pub const MAX_TITLE_CHARS: usize = 40;
/// Longest title as sent, bytes (before trimming).
pub const MAX_TITLE_BYTES: usize = 200;
/// Used signatures are forgotten after this long (well past the ±300 s window), ms.
pub const SIG_KEEP_MS: u64 = 3_600_000;
/// Writes per IP: `(count, window_ms)`.
pub const IP_LIMIT: (usize, u64) = (30, 60_000);
/// Verified writes per wallet: `(count, window_ms)`.
pub const WALLET_LIMIT: (usize, u64) = (10, 60_000);
/// Handles nobody can take, and words no handle may contain (impersonation of the house or the
/// venue).
pub const RESERVED: [&str; 12] = [
    "betfi",
    "schedina",
    "pascal",
    "admin",
    "official",
    "support",
    "verified",
    "moderator",
    "desk",
    "house",
    "system",
    "null",
];

/// Mounts `PUT /profile` and `GET /profile/{wallet}`.
pub fn mount(router: Router<AppState>, _state: &AppState) -> Router<AppState> {
    router
        .route("/profile", axum::routing::put(put_profile))
        .route("/profile/{wallet}", get(get_profile))
}

// --- the message and the field rules ------------------------------------------------------------

/// The exact UTF-8 message `wallet` signs.
pub fn message(
    wallet: &str,
    handle: &str,
    ticket: Option<&str>,
    title: Option<&str>,
    ts: i64,
) -> String {
    format!(
        "{DOMAIN}\n{wallet}\n{handle}\n{}\n{}\n{ts}",
        ticket.unwrap_or("-"),
        title.unwrap_or("-")
    )
}

/// The exact UTF-8 v2 message `wallet` signs for a request with `links` or `follow`
/// (module docs). `handle` is `""` when the request has none.
pub fn message_v2(
    wallet: &str,
    handle: &str,
    ticket: Option<&str>,
    title: Option<&str>,
    links: Option<&ProfileLinks>,
    follow: Option<&FollowRequest>,
    ts: i64,
) -> String {
    let v = |o: &Option<String>| o.clone().unwrap_or_else(|| "-".to_owned());
    let links = links.map_or_else(
        || "-".to_owned(),
        |l| {
            format!(
                "x={};youtube={};telegram={};instagram={}",
                v(&l.x),
                v(&l.youtube),
                v(&l.telegram),
                v(&l.instagram)
            )
        },
    );
    let follow = follow.map_or_else(
        || "-".to_owned(),
        |f| format!("{}:{}", f.wallet, if f.on { "on" } else { "off" }),
    );
    let handle = if handle.is_empty() { "-" } else { handle };
    format!(
        "{DOMAIN_V2}\n{wallet}\n{handle}\n{}\n{}\n{links}\n{follow}\n{ts}",
        ticket.unwrap_or("-"),
        title.unwrap_or("-")
    )
}

/// The network of a profile link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    /// X (Twitter): `[A-Za-z0-9_]{1,15}`.
    X,
    /// YouTube handle: `[A-Za-z0-9._-]{3,30}`.
    Youtube,
    /// Telegram username: `[A-Za-z][A-Za-z0-9_]{4,31}`.
    Telegram,
    /// Instagram username: `[A-Za-z0-9._]{1,30}`, no leading, trailing or doubled `.`.
    Instagram,
}

/// The stored form of a link: `v` without one leading `@`, checked against `net`'s rule. A URL,
/// a path, spaces or any other character are refused.
pub fn check_link(net: Network, v: &str) -> Result<String, String> {
    let h = v.strip_prefix('@').unwrap_or(v);
    let (min, max, extra): (usize, usize, &[u8]) = match net {
        Network::X => (1, 15, b"_"),
        Network::Youtube => (3, 30, b"._-"),
        Network::Telegram => (5, 32, b"_"),
        Network::Instagram => (1, 30, b"._"),
    };
    let chars_ok = h
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || extra.contains(&b));
    let shape_ok = match net {
        Network::Telegram => h.bytes().next().is_some_and(|b| b.is_ascii_alphabetic()),
        Network::Instagram => !h.starts_with('.') && !h.ends_with('.') && !h.contains(".."),
        Network::X | Network::Youtube => true,
    };
    if !(min..=max).contains(&h.len()) || !chars_ok || !shape_ok {
        let name = match net {
            Network::X => "x",
            Network::Youtube => "youtube",
            Network::Telegram => "telegram",
            Network::Instagram => "instagram",
        };
        return Err(format!(
            "{name} link {v:?}: a bare handle or username ({min} to {max} characters), not a URL"
        ));
    }
    Ok(h.to_owned())
}

/// Every link of `l` in its stored form ([`check_link`]); `null` stays `null`.
pub fn clean_links(l: &ProfileLinks) -> Result<ProfileLinks, String> {
    let c = |net: Network, o: &Option<String>| o.as_deref().map(|v| check_link(net, v)).transpose();
    Ok(ProfileLinks {
        x: c(Network::X, &l.x)?,
        youtube: c(Network::Youtube, &l.youtube)?,
        telegram: c(Network::Telegram, &l.telegram)?,
        instagram: c(Network::Instagram, &l.instagram)?,
    })
}

/// `^[a-z0-9_]{3,20}$` and not reserved.
pub fn check_handle(h: &str) -> Result<(), String> {
    let ok = (3..=20).contains(&h.len())
        && h.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if !ok {
        return Err(format!(
            "handle {h:?}: 3 to 20 characters, lower-case letters, digits or _"
        ));
    }
    if RESERVED.iter().any(|r| h.contains(r)) {
        return Err(format!("handle {h:?} is reserved"));
    }
    Ok(())
}

/// Invisible or direction-changing characters a title may not carry (spoofing).
fn is_format_char(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2069}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}')
}

/// The title to store: `t` trimmed, 1–40 characters, no control or formatting characters.
pub fn clean_title(t: &str) -> Result<String, String> {
    if t.len() > MAX_TITLE_BYTES {
        return Err(format!("title over {MAX_TITLE_CHARS} characters"));
    }
    if t.chars().any(|c| c.is_control() || is_format_char(c)) {
        return Err("title has control or invisible characters".into());
    }
    let trimmed = t.trim();
    if trimmed.is_empty() || trimmed == "-" {
        return Err("title is empty".into());
    }
    if trimmed.chars().count() > MAX_TITLE_CHARS {
        return Err(format!("title over {MAX_TITLE_CHARS} characters"));
    }
    Ok(trimmed.to_owned())
}

/// Checks `sig` (base64, 64 bytes) by `wallet` (base58) over `msg`.
pub fn verify(wallet: &str, msg: &[u8], sig_b64: &str) -> Result<(), String> {
    let key: Pubkey = wallet
        .parse()
        .map_err(|_| format!("{wallet} is not a wallet address"))?;
    let vk = VerifyingKey::from_bytes(&key.to_bytes())
        .map_err(|_| format!("{wallet} is not an ed25519 key"))?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(sig_b64)
        .map_err(|_| "sig is not base64".to_owned())?;
    let sig = Signature::from_slice(&raw).map_err(|_| "sig is not 64 bytes".to_owned())?;
    vk.verify_strict(msg, &sig)
        .map_err(|_| format!("signature does not match wallet {wallet}"))
}

// --- rate limits --------------------------------------------------------------------------------

/// Sliding-window write limits per key (`ip:…`, `wallet:…`), in [`crate::state::Shared::profiles`].
/// Its lock is taken alone and never across an `.await` (D-043).
#[derive(Debug, Default)]
pub struct Limiter {
    hits: Mutex<HashMap<String, VecDeque<u64>>>,
}

impl Limiter {
    /// Records a hit for `key` at `now_ms` if fewer than `limit.0` happened in the last
    /// `limit.1` ms; `false` (nothing recorded) otherwise.
    pub fn allow(&self, key: &str, limit: (usize, u64), now_ms: u64) -> bool {
        let (count, window) = limit;
        let since = now_ms.saturating_sub(window);
        let mut map = self
            .hits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if map.len() > 10_000 {
            map.retain(|_, q| q.back().is_some_and(|t| *t > since));
        }
        let q = map.entry(key.to_owned()).or_default();
        while q.front().is_some_and(|t| *t <= since) {
            q.pop_front();
        }
        if q.len() >= count {
            return false;
        }
        q.push_back(now_ms);
        true
    }
}

// --- errors -------------------------------------------------------------------------------------

/// A refused profile write: 422, 409 or 429 with `{"status":"rejected","reason"}`; 503
/// `unavailable` for a database error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileError(pub StatusCode, pub ErrorBody);

impl ProfileError {
    fn new(code: StatusCode, reason: impl Into<String>) -> Self {
        ProfileError(
            code,
            ErrorBody {
                status: ErrorStatus::Rejected,
                reason: reason.into(),
            },
        )
    }

    /// 422.
    pub fn invalid(reason: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, reason)
    }

    /// 409.
    pub fn conflict(reason: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, reason)
    }

    /// 429.
    pub fn too_many(reason: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, reason)
    }
}

impl IntoResponse for ProfileError {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}

impl From<sqlx::Error> for ProfileError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!(error = %e, "profiles: database error");
        ProfileError(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorBody {
                status: ErrorStatus::Unavailable,
                reason: "database error".into(),
            },
        )
    }
}

// --- reads --------------------------------------------------------------------------------------

/// `wallet`'s profile (`handle: None`, `since: 0` without one), with its links, follow counts
/// and `verified` (on the tipster board, D-304).
pub async fn profile_of(db: &SqlitePool, wallet: &str) -> Result<ProfileResponse, sqlx::Error> {
    let row: Option<(String, i64)> =
        sqlx::query_as("SELECT handle, handle_set_at FROM profiles WHERE wallet = ?")
            .bind(wallet)
            .fetch_optional(db)
            .await?;
    let links = links_of(db, wallet).await?;
    let count = |sql: &'static str| async move {
        sqlx::query_scalar::<_, i64>(sql)
            .bind(wallet)
            .fetch_one(db)
            .await
            .map(|n| u64::try_from(n).unwrap_or(0))
    };
    let followers = count("SELECT COUNT(*) FROM follows WHERE followee = ?").await?;
    let following = count("SELECT COUNT(*) FROM follows WHERE follower = ?").await?;
    let verified = match crate::tipsters::store::books(db, Some(wallet)).await?.pop() {
        Some(book) => {
            crate::tipsters::metrics::on_board(&crate::tipsters::metrics::score(&book, now_s()).row)
        }
        None => false,
    };
    Ok(ProfileResponse {
        wallet: wallet.to_owned(),
        since: row.as_ref().map_or(0, |r| r.1),
        handle: row.map(|r| r.0),
        verified,
        links,
        followers,
        following,
    })
}

/// `wallet`'s links (all `null` without any).
pub async fn links_of(db: &SqlitePool, wallet: &str) -> Result<ProfileLinks, sqlx::Error> {
    type Row = (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let row: Option<Row> = sqlx::query_as(
        "SELECT x, youtube, telegram, instagram FROM profile_links WHERE wallet = ?",
    )
    .bind(wallet)
    .fetch_optional(db)
    .await?;
    Ok(row.map_or_else(
        ProfileLinks::default,
        |(x, youtube, telegram, instagram)| ProfileLinks {
            x,
            youtube,
            telegram,
            instagram,
        },
    ))
}

/// The handles of `wallets` that have one.
pub async fn handles_of(
    db: &SqlitePool,
    wallets: &[String],
) -> Result<HashMap<String, String>, sqlx::Error> {
    let mut out = HashMap::new();
    for chunk in wallets.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(", ");
        let sql = format!("SELECT wallet, handle FROM profiles WHERE wallet IN ({marks})");
        let mut q = sqlx::query_as::<_, (String, String)>(sqlx::AssertSqlSafe(sql));
        for w in chunk {
            q = q.bind(w);
        }
        out.extend(q.fetch_all(db).await?);
    }
    Ok(out)
}

/// The title of `ticket`, if its tipster set one.
pub async fn title_of(db: &SqlitePool, ticket: &str) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT title FROM tip_titles WHERE ticket = ?")
        .bind(ticket)
        .fetch_optional(db)
        .await
}

/// `GET /profile/{wallet}`.
async fn get_profile(
    State(state): State<AppState>,
    Path(wallet): Path<String>,
) -> Result<Json<ProfileResponse>, ProfileError> {
    if wallet.parse::<Pubkey>().is_err() {
        return Err(ProfileError::invalid(format!(
            "{wallet} is not a wallet address"
        )));
    }
    Ok(Json(profile_of(&state.db, &wallet).await?))
}

// --- PUT /profile -------------------------------------------------------------------------------

fn now_s() -> i64 {
    i64::try_from(now_ms() / 1000).unwrap_or(i64::MAX)
}

/// A request that passed every stateless check.
struct Checked {
    wallet: String,
    /// Empty on a follow-only request without a handle.
    handle: String,
    /// `(ticket, trimmed title)`.
    title: Option<(String, String)>,
    /// Links in their stored form.
    links: Option<ProfileLinks>,
    follow: Option<FollowRequest>,
    sig: String,
}

/// Shape, clock and signature checks (no I/O).
pub fn check_request(req: &ProfileRequest, now_s: i64) -> Result<(), ProfileError> {
    if req.wallet.parse::<Pubkey>().is_err() {
        return Err(ProfileError::invalid(format!(
            "{} is not a wallet address",
            req.wallet
        )));
    }
    let follow_only =
        req.follow.is_some() && req.links.is_none() && req.ticket.is_none() && req.title.is_none();
    if !(req.handle.is_empty() && follow_only) {
        check_handle(&req.handle).map_err(ProfileError::invalid)?;
    }
    if let Some(l) = &req.links {
        clean_links(l).map_err(ProfileError::invalid)?;
    }
    if let Some(f) = &req.follow {
        if f.wallet.parse::<Pubkey>().is_err() {
            return Err(ProfileError::invalid(format!(
                "follow: {} is not a wallet address",
                f.wallet
            )));
        }
        if f.wallet == req.wallet {
            return Err(ProfileError::invalid("a wallet cannot follow itself"));
        }
    }
    match (&req.ticket, &req.title) {
        (None, None) => {}
        (Some(t), Some(title)) => {
            if t.parse::<Pubkey>().is_err() {
                return Err(ProfileError::invalid(format!(
                    "{t} is not a ticket address"
                )));
            }
            clean_title(title).map_err(ProfileError::invalid)?;
        }
        (Some(_), None) => return Err(ProfileError::invalid("a ticket needs a title")),
        (None, Some(_)) => return Err(ProfileError::invalid("a title needs its ticket")),
    }
    if (now_s - req.ts).abs() > TS_WINDOW_S {
        return Err(ProfileError::invalid(format!(
            "ts {} is more than {TS_WINDOW_S} s from the desk's clock ({now_s}): sign again",
            req.ts
        )));
    }
    let v2 = message_v2(
        &req.wallet,
        &req.handle,
        req.ticket.as_deref(),
        req.title.as_deref(),
        req.links.as_ref(),
        req.follow.as_ref(),
        req.ts,
    );
    if req.links.is_some() || req.follow.is_some() {
        return verify(&req.wallet, v2.as_bytes(), &req.sig).map_err(ProfileError::invalid);
    }
    let v1 = message(
        &req.wallet,
        &req.handle,
        req.ticket.as_deref(),
        req.title.as_deref(),
        req.ts,
    );
    verify(&req.wallet, v1.as_bytes(), &req.sig)
        .or_else(|e| verify(&req.wallet, v2.as_bytes(), &req.sig).map_err(|_| e))
        .map_err(ProfileError::invalid)
}

/// `PUT /profile`.
async fn put_profile(
    State(state): State<AppState>,
    ext: Extensions,
    body: Result<Json<ProfileRequest>, JsonRejection>,
) -> Result<Json<ProfileResponse>, ProfileError> {
    let now = now_ms();
    let ip = ext
        .get::<ConnectInfo<SocketAddr>>()
        .map_or_else(|| "unknown".to_owned(), |c| c.0.ip().to_string());
    if !state.profiles.allow(&format!("ip:{ip}"), IP_LIMIT, now) {
        return Err(ProfileError::too_many(
            "too many profile writes from this address; try again in a minute",
        ));
    }
    let Json(req) = body.map_err(|e| ProfileError::invalid(e.body_text()))?;
    check_request(&req, now_s())?;
    if !state
        .profiles
        .allow(&format!("wallet:{}", req.wallet), WALLET_LIMIT, now)
    {
        return Err(ProfileError::too_many(
            "too many profile writes for this wallet; try again in a minute",
        ));
    }
    let checked = Checked {
        title: match (req.ticket, req.title) {
            (Some(t), Some(title)) => {
                Some((t, clean_title(&title).map_err(ProfileError::invalid)?))
            }
            _ => None,
        },
        links: req
            .links
            .as_ref()
            .map(clean_links)
            .transpose()
            .map_err(ProfileError::invalid)?,
        follow: req.follow,
        wallet: req.wallet,
        handle: req.handle,
        sig: req.sig,
    };
    let titled = apply(&state, &checked).await?;
    // DB before emit: the titled tip's `ticket` frame after the commit. The write stands
    // whatever happens next, so nothing below turns it into an error.
    if let Some(ticket) = titled {
        match crate::crank::tickets::load_ticket(&state, &ticket).await {
            Ok(Some(t)) => state.emit(DeskEvent::TicketChanged(std::sync::Arc::new(t))),
            Ok(None) => {}
            Err(e) => tracing::warn!(error = %e, ticket, "profiles: titled tip not reloaded"),
        }
    }
    let since = now_s();
    let profile = profile_of(&state.db, &checked.wallet)
        .await
        .unwrap_or_else(|_| ProfileResponse {
            wallet: checked.wallet.clone(),
            handle: (!checked.handle.is_empty()).then(|| checked.handle.clone()),
            since,
            verified: false,
            links: checked.links.clone().unwrap_or_default(),
            followers: 0,
            following: 0,
        });
    Ok(Json(profile))
}

/// All writes of one request, in one transaction. Returns the titled ticket.
async fn apply(state: &AppState, c: &Checked) -> Result<Option<String>, ProfileError> {
    let now = now_s();
    let now_ms_i = i64::try_from(now_ms()).unwrap_or(i64::MAX);
    let mut tx = state.db.begin().await?;
    // The first statement writes, so the transaction holds SQLite's write lock from here on.
    sqlx::query("DELETE FROM profile_sigs WHERE used_ms < ?")
        .bind(now_ms_i - i64::try_from(SIG_KEEP_MS).unwrap_or(0))
        .execute(&mut *tx)
        .await?;
    let fresh = sqlx::query(
        "INSERT INTO profile_sigs (sig, wallet, used_ms) VALUES (?, ?, ?)
         ON CONFLICT (sig) DO NOTHING",
    )
    .bind(&c.sig)
    .bind(&c.wallet)
    .bind(now_ms_i)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if fresh == 0 {
        return Err(ProfileError::conflict(
            "this signature was already used: sign again",
        ));
    }
    if !c.handle.is_empty() {
        set_handle(&mut tx, c, now).await?;
    }
    let titled = match &c.title {
        Some((ticket, title)) => {
            set_title(&mut tx, &c.wallet, ticket, title, now).await?;
            Some(ticket.clone())
        }
        None => None,
    };
    if let Some(l) = &c.links {
        set_links(&mut tx, &c.wallet, l, now_ms_i).await?;
    }
    if let Some(f) = &c.follow {
        set_follow(&mut tx, &c.wallet, f, now_ms_i).await?;
    }
    tx.commit().await?;
    Ok(titled)
}

async fn set_links(
    tx: &mut Transaction<'_, Sqlite>,
    wallet: &str,
    l: &ProfileLinks,
    now_ms: i64,
) -> Result<(), ProfileError> {
    sqlx::query(
        "INSERT INTO profile_links (wallet, x, youtube, telegram, instagram, updated_ms)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (wallet) DO UPDATE SET x = excluded.x, youtube = excluded.youtube,
            telegram = excluded.telegram, instagram = excluded.instagram,
            updated_ms = excluded.updated_ms",
    )
    .bind(wallet)
    .bind(&l.x)
    .bind(&l.youtube)
    .bind(&l.telegram)
    .bind(&l.instagram)
    .bind(now_ms)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn set_follow(
    tx: &mut Transaction<'_, Sqlite>,
    wallet: &str,
    f: &FollowRequest,
    now_ms: i64,
) -> Result<(), ProfileError> {
    if !f.on {
        sqlx::query("DELETE FROM follows WHERE follower = ? AND followee = ?")
            .bind(wallet)
            .bind(&f.wallet)
            .execute(&mut **tx)
            .await?;
        return Ok(());
    }
    let following: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM follows WHERE follower = ? AND followee != ?")
            .bind(wallet)
            .bind(&f.wallet)
            .fetch_one(&mut **tx)
            .await?;
    if following >= MAX_FOLLOWING {
        return Err(ProfileError::conflict(format!(
            "a wallet follows at most {MAX_FOLLOWING} others"
        )));
    }
    sqlx::query(
        "INSERT INTO follows (follower, followee, created_ms) VALUES (?, ?, ?)
         ON CONFLICT (follower, followee) DO NOTHING",
    )
    .bind(wallet)
    .bind(&f.wallet)
    .bind(now_ms)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn set_handle(
    tx: &mut Transaction<'_, Sqlite>,
    c: &Checked,
    now: i64,
) -> Result<(), ProfileError> {
    let current: Option<(String, i64)> =
        sqlx::query_as("SELECT handle, handle_set_at FROM profiles WHERE wallet = ?")
            .bind(&c.wallet)
            .fetch_optional(&mut **tx)
            .await?;
    if let Some((h, set_at)) = &current {
        if *h == c.handle {
            return Ok(());
        }
        if now - set_at < HANDLE_EVERY_S {
            return Err(ProfileError::conflict(format!(
                "handle changed less than 24 h ago: next change from {} (Unix s)",
                set_at + HANDLE_EVERY_S
            )));
        }
    }
    let held: Option<(String, i64)> = sqlx::query_as(
        "SELECT wallet, released_at FROM profile_released_handles
         WHERE handle = ? COLLATE NOCASE",
    )
    .bind(&c.handle)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some((w, at)) = held
        && w != c.wallet
        && now - at < RELEASED_HOLD_S
    {
        return Err(ProfileError::conflict(format!(
            "handle @{} was given up less than 30 days ago and is held for its previous owner",
            c.handle
        )));
    }
    let taken: Option<String> =
        sqlx::query_scalar("SELECT wallet FROM profiles WHERE handle = ? COLLATE NOCASE")
            .bind(&c.handle)
            .fetch_optional(&mut **tx)
            .await?;
    if taken.is_some_and(|w| w != c.wallet) {
        return Err(ProfileError::conflict(format!(
            "handle @{} is taken",
            c.handle
        )));
    }
    let now_ms_i = i64::try_from(now_ms()).unwrap_or(i64::MAX);
    let written = match &current {
        Some((old, _)) => {
            sqlx::query(
                "INSERT INTO profile_released_handles (handle, wallet, released_at)
                 VALUES (?, ?, ?)
                 ON CONFLICT (handle) DO UPDATE SET wallet = excluded.wallet,
                    released_at = excluded.released_at",
            )
            .bind(old)
            .bind(&c.wallet)
            .bind(now)
            .execute(&mut **tx)
            .await?;
            sqlx::query(
                "UPDATE profiles SET handle = ?, handle_set_at = ?, updated_ms = ?
                 WHERE wallet = ? AND handle_set_at <= ?",
            )
            .bind(&c.handle)
            .bind(now)
            .bind(now_ms_i)
            .bind(&c.wallet)
            .bind(now - HANDLE_EVERY_S)
            .execute(&mut **tx)
            .await
        }
        None => {
            sqlx::query(
                "INSERT INTO profiles (wallet, handle, handle_set_at, created_at, updated_ms)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&c.wallet)
            .bind(&c.handle)
            .bind(now)
            .bind(now)
            .bind(now_ms_i)
            .execute(&mut **tx)
            .await
        }
    };
    match written {
        Ok(r) if r.rows_affected() == 1 => {}
        Ok(_) => {
            return Err(ProfileError::conflict("handle changed less than 24 h ago"));
        }
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(ProfileError::conflict(format!(
                "handle @{} is taken",
                c.handle
            )));
        }
        Err(e) => return Err(e.into()),
    }
    // Taking back one's own released handle ends its hold.
    sqlx::query("DELETE FROM profile_released_handles WHERE handle = ? COLLATE NOCASE")
        .bind(&c.handle)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn set_title(
    tx: &mut Transaction<'_, Sqlite>,
    wallet: &str,
    ticket: &str,
    title: &str,
    now: i64,
) -> Result<(), ProfileError> {
    let row: Option<(String, Option<String>, Option<String>, String)> =
        sqlx::query_as("SELECT user, tipster, copied_from, state FROM tickets WHERE ticket = ?")
            .bind(ticket)
            .fetch_optional(&mut **tx)
            .await?;
    let Some((user, tipster, copied_from, st)) = row else {
        return Err(ProfileError::invalid(format!("no ticket {ticket}")));
    };
    if user != wallet {
        return Err(ProfileError::invalid(format!(
            "ticket {ticket} is not yours: only its tipster can title it"
        )));
    }
    if tipster.as_deref() != Some(wallet) || copied_from.is_some() {
        return Err(ProfileError::invalid(format!(
            "ticket {ticket} is not a tip of yours (only own tips take a title)"
        )));
    }
    // Pending or open, in play included (D-660: a tip may be shared with a leg live).
    if st != "pending" && st != "open" {
        return Err(ProfileError::conflict(format!(
            "ticket {ticket} is {st}: titles are set while the tip is open"
        )));
    }
    let set = sqlx::query(
        "INSERT INTO tip_titles (ticket, wallet, title, set_at) VALUES (?, ?, ?, ?)
         ON CONFLICT (ticket) DO NOTHING",
    )
    .bind(ticket)
    .bind(wallet)
    .bind(title)
    .bind(now)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if set == 0 {
        return Err(ProfileError::conflict(format!(
            "ticket {ticket} already has a title"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_follow_the_rule() {
        for ok in ["abc", "lorenzo_hoops", "a_1", "x1234567890123456789"] {
            assert!(check_handle(ok).is_ok(), "{ok}");
        }
        for bad in [
            "ab",
            "Lorenzo",
            "lorenzo-hoops",
            "lorenzo hoops",
            "x12345678901234567890",
            "",
            "méo",
            "the_betfi_team",
            "pascal",
            "lorenzo\n",
        ] {
            assert!(check_handle(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn titles_are_trimmed_and_bounded() {
        assert_eq!(
            clean_title("  Thursday treble ").unwrap(),
            "Thursday treble"
        );
        assert_eq!(clean_title(&"é".repeat(40)).unwrap().chars().count(), 40);
        for bad in [
            "",
            "   ",
            "-",
            " - ",
            "a\nb",
            "a\tb",
            "nul\u{0}",
            "rtl \u{202E}gnp.exe",
            "zero\u{200B}width",
        ] {
            assert!(clean_title(bad).is_err(), "{bad:?}");
        }
        assert!(clean_title(&"x".repeat(41)).is_err());
        assert!(clean_title(&"x".repeat(MAX_TITLE_BYTES + 1)).is_err());
    }

    #[test]
    fn links_are_bare_handles() {
        for (net, ok) in [
            (Network::X, "lorenzo_hoops"),
            (Network::X, "@LorenzoHoops"),
            (Network::Youtube, "lorenzo.hoops-tv"),
            (Network::Telegram, "lorenzo_hoops"),
            (Network::Instagram, "lorenzo.hoops"),
        ] {
            assert!(check_link(net, ok).is_ok(), "{ok}");
        }
        assert_eq!(check_link(Network::X, "@abc").unwrap(), "abc");
        for (net, bad) in [
            (Network::X, "https://x.com/lorenzo"),
            (Network::X, "x.com/lorenzo"),
            (Network::X, "a_very_long_handle_x"),
            (Network::X, ""),
            (Network::X, "@"),
            (Network::X, "-"),
            (Network::X, "lo renzo"),
            (Network::X, "lorenzo\n"),
            (Network::Youtube, "ab"),
            (Network::Youtube, "@chan/videos"),
            (Network::Telegram, "1abcde"),
            (Network::Telegram, "abcd"),
            (Network::Telegram, "t.me/abcde"),
            (Network::Instagram, ".lorenzo"),
            (Network::Instagram, "lorenzo."),
            (Network::Instagram, "lo..renzo"),
            (Network::Instagram, "lorenzo;x=1"),
            (Network::Instagram, "ab:on"),
        ] {
            assert!(check_link(net, bad).is_err(), "{net:?} {bad:?}");
        }
    }

    #[test]
    fn the_v2_message_frames_links_and_follows() {
        let links = ProfileLinks {
            x: Some("@lorenzo_hoops".into()),
            youtube: None,
            telegram: Some("lorenzo_tg".into()),
            instagram: None,
        };
        let f = FollowRequest {
            wallet: "T1pst3r".into(),
            on: true,
        };
        assert_eq!(
            message_v2("W", "lorenzo_hoops", None, None, Some(&links), None, 7),
            "betfi/profile/v2\nW\nlorenzo_hoops\n-\n-\nx=@lorenzo_hoops;youtube=-;telegram=lorenzo_tg;instagram=-\n-\n7"
        );
        assert_eq!(
            message_v2("W", "", None, None, None, Some(&f), 7),
            "betfi/profile/v2\nW\n-\n-\n-\n-\nT1pst3r:on\n7"
        );
        // No links and all-null links are different messages.
        assert_ne!(
            message_v2("W", "h", None, None, None, None, 7),
            message_v2(
                "W",
                "h",
                None,
                None,
                Some(&ProfileLinks::default()),
                None,
                7
            )
        );
    }

    #[test]
    fn the_limiter_slides() {
        let l = Limiter::default();
        assert!((0..3).all(|i| l.allow("k", (3, 1000), 10 + i)));
        assert!(!l.allow("k", (3, 1000), 500));
        assert!(l.allow("other", (3, 1000), 500));
        assert!(l.allow("k", (3, 1000), 1011));
    }
}
