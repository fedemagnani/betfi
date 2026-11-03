//! Desk client: typed C4 requests against the desk's HTTP API, or, in demo mode (no
//! `SCHEDINA_DESK_URL` at build time), the same shapes from the example responses bundled in
//! `fixtures/desk/`. Screens never know which one they talk to.

pub mod types;

use std::fmt;

use gloo_net::http::{Request, Response};
use serde::de::DeserializeOwned;

use types::{
    Card, CashoutQuote, CopyQuoteRequest, ErrorBody, House, Quote, QuoteRequest, SentimentResponse,
    SwapQuote, SwapQuoteRequest, Ticket, TicketsResponse, TipsterProfile, TipsterSort,
    TipstersResponse, TokenInfo, TokensResponse,
};

/// The desk's C4 example responses (copies of the repo's `fixtures/desk/`, refreshed by
/// `fixtures/gen_fixtures.py`), compiled into the app for demo mode.
///
/// A build with `SCHEDINA_DESIGN_FIXTURES` (6.0a, [`crate::config::DESIGN_FIXTURES`]) takes the
/// mockup's fictional data set from `fixtures/design/` instead, file by file where one exists
/// (`design_or`). The choice is a constant, so a normal build carries none of the design files.
pub mod fixtures {
    use crate::config::DESIGN_FIXTURES;

    /// `design` in a design-fixture build, else `demo`. Evaluated at compile time.
    const fn design_or(design: &'static str, demo: &'static str) -> &'static str {
        if DESIGN_FIXTURES { design } else { demo }
    }

    pub const CARD: &str = design_or(
        include_str!("../../fixtures/design/card.json"),
        include_str!("../../fixtures/desk/card.json"),
    );
    pub const QUOTE: &str = design_or(
        include_str!("../../fixtures/design/quote.json"),
        include_str!("../../fixtures/desk/quote.json"),
    );
    pub const TICKETS: &str = design_or(
        include_str!("../../fixtures/design/tickets.json"),
        include_str!("../../fixtures/desk/tickets.json"),
    );
    pub const TICKET: &str = design_or(
        include_str!("../../fixtures/design/ticket.json"),
        include_str!("../../fixtures/desk/ticket.json"),
    );
    pub const CASHOUT_QUOTE: &str = design_or(
        include_str!("../../fixtures/design/cashout_quote.json"),
        include_str!("../../fixtures/desk/cashout_quote.json"),
    );
    pub const REJECTED: &str = include_str!("../../fixtures/desk/error_rejected.json");
    /// The pending the demo stream mints, and the one it refunds.
    pub const PROGRESS_ESCROWED: &str =
        include_str!("../../fixtures/desk/stream_progress_escrowed.json");
    pub const PROGRESS_REFUNDED: &str =
        include_str!("../../fixtures/desk/stream_progress_refunded.json");
    /// `[{"delay_ms": u32, "frame": "<raw text frame>"}]`, replayed by the demo stream.
    pub const STREAM: &str = include_str!("../../fixtures/desk/stream.json");
    /// L4-2 routes: written from the contract's examples by 4.B until the desk ships its own
    /// (`gen_fixtures.py` then copies those by shape). `tipsters.json` is the `all=1` answer.
    pub const TIPSTERS: &str = design_or(
        include_str!("../../fixtures/design/tipsters.json"),
        include_str!("../../fixtures/desk/tipsters.json"),
    );
    pub const TIPSTER: &str = design_or(
        include_str!("../../fixtures/design/tipster.json"),
        include_str!("../../fixtures/desk/tipster.json"),
    );
    pub const HOUSE: &str = include_str!("../../fixtures/desk/house.json");
    /// The example quote plus its 2% copy fee: the copy fee is not part of the signed bytes
    /// (the program charges it from the stake), so the signed message stays the desk's.
    pub const COPY_QUOTE: &str = design_or(
        include_str!("../../fixtures/design/copy_quote.json"),
        include_str!("../../fixtures/desk/copy_quote.json"),
    );
    /// L4-3 swap routes: mock USDC and mockBONK, and the swap of `fixtures/swap/vectors.json`'s
    /// worked example (10,000 mockBONK into a 5,000,000 : 5,000 pool). A design build lists
    /// mock USDC alone (the mockup pays in euros: no "Pay with" chip).
    pub const TOKENS: &str = design_or(
        include_str!("../../fixtures/design/tokens.json"),
        include_str!("../../fixtures/desk/tokens.json"),
    );
    pub const SWAP_QUOTE: &str = include_str!("../../fixtures/desk/swap_quote.json");
    /// L5-4: the desk's `/sentiment` answer on the 28 Oct tape's trades (the example card's
    /// games), checked against the desk in `crates/desk/tests/sentiment.rs`.
    pub const SENTIMENT: &str = design_or(
        include_str!("../../fixtures/design/sentiment.json"),
        include_str!("../../fixtures/desk/sentiment.json"),
    );
    /// L6-2 `GET /fx` and `GET /profile/{wallet}`: written from the contract's examples by
    /// 6.0a until the desk ships its own (`gen_fixtures.py` copies the desk's by name).
    pub const FX: &str = design_or(
        include_str!("../../fixtures/design/fx.json"),
        include_str!("../../fixtures/desk/fx.json"),
    );
    pub const PROFILE: &str = design_or(
        include_str!("../../fixtures/design/profile.json"),
        include_str!("../../fixtures/desk/profile.json"),
    );

    /// Design builds only: what the mockup shows that is not a desk answer. `None` otherwise.
    pub mod design {
        use crate::config::DESIGN_FIXTURES;

        /// The mockup's 3-pick slip: picks by symbol and side, with the odds each one showed.
        /// Read by the slip screen (6.1c) to seed its picks in a design build.
        pub const SLIP: Option<&str> = if DESIGN_FIXTURES {
            Some(include_str!("../../fixtures/design/slip.json"))
        } else {
            None
        };
        /// `{"mobile_order": [game_id, …]}`: Home's "Tonight" as the mobile mockup orders it.
        const HOME: Option<&str> = if DESIGN_FIXTURES {
            Some(include_str!("../../fixtures/design/home.json"))
        } else {
            None
        };

        /// Design builds: a game's place in the mobile mockup's "Tonight" (`01-home.png` lists
        /// other games than the desktop mockup, whose order is the card's). Home applies it below
        /// 1024 px as a CSS order. `None` in every other build.
        pub fn mobile_rank(game_id: &str) -> Option<usize> {
            #[derive(serde::Deserialize)]
            struct Home {
                mobile_order: Vec<String>,
            }
            static ORDER: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
            ORDER
                .get_or_init(|| {
                    HOME.and_then(|j| serde_json::from_str::<Home>(j).ok())
                        .map(|h| h.mobile_order)
                        .unwrap_or_default()
                })
                .iter()
                .position(|g| g == game_id)
        }
        /// The mockup's wallet balance (`{"usdc_u"}`), shown in place of an on-chain read.
        pub const WALLET: Option<&str> = if DESIGN_FIXTURES {
            Some(include_str!("../../fixtures/design/wallet.json"))
        } else {
            None
        };
        /// L7-1 (7.1d): the copiers drawer's list for the design tipster's first tip
        /// (`GET /tipsters/{wallet}/copiers?ticket=`). A plain demo build has no copies to list.
        pub const COPIERS: Option<&str> = if DESIGN_FIXTURES {
            Some(include_str!("../../fixtures/design/copiers.json"))
        } else {
            None
        };
    }
}

/// Why a desk call failed, in words the UI can show.
#[derive(Debug, Clone, PartialEq)]
pub enum DeskError {
    /// 422: the desk refuses this request (e.g. two legs on one game). Show the reason.
    Rejected(String),
    /// 503: no quote right now (stale books, rate limit). Retry later.
    Unavailable(String),
    /// Any other HTTP status.
    Http(u16),
    /// The request did not reach the desk.
    Network(String),
    /// The desk answered with a body that is not the C4 shape.
    Decode(String),
}

impl fmt::Display for DeskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeskError::Rejected(r) => write!(f, "Not accepted: {r}"),
            DeskError::Unavailable(r) if r.is_empty() => write!(f, "Temporarily unavailable"),
            DeskError::Unavailable(r) => write!(f, "Temporarily unavailable: {r}"),
            DeskError::Http(s) => write!(f, "The desk answered HTTP {s}"),
            DeskError::Network(e) => write!(f, "Cannot reach the desk: {e}"),
            DeskError::Decode(e) => write!(f, "Unexpected answer from the desk: {e}"),
        }
    }
}

impl DeskError {
    /// The error in the app's words (spec 7: team names, no symbols or market words): the
    /// desk's game ids and symbols become "Away at Home" from the Card, "legs" become "picks",
    /// "books" become "prices".
    pub fn plain(&self, card: Option<&Card>) -> String {
        match self {
            DeskError::Rejected(r) if r.is_empty() => {
                "The desk does not accept this bet.".to_string()
            }
            DeskError::Rejected(r) => format!("Not accepted: {}.", plain_reason(r, card)),
            DeskError::Unavailable(r) if r.is_empty() => {
                "No price right now. Trying again in a few seconds.".to_string()
            }
            DeskError::Unavailable(r) => format!(
                "No price right now ({}). Trying again in a few seconds.",
                plain_reason(r, card)
            ),
            DeskError::Http(404) => "Not found.".to_string(),
            other => other.to_string(),
        }
    }
}

/// Rewrite a desk reason word by word: game ids and symbols to team names, market words to
/// the app's words. Anything else is kept.
pub fn plain_reason(reason: &str, card: Option<&Card>) -> String {
    let games = card.map(|c| c.games.as_slice()).unwrap_or_default();
    reason
        .split(' ')
        .map(|word| {
            let core = word.trim_end_matches([',', '.', ';', ':', ')']);
            let rest = &word[core.len()..];
            let open = core.starts_with('(');
            let core = core.trim_start_matches('(');
            let swapped =
                if let Some(g) = games.iter().find(|g| g.game_id == core || g.symbol == core) {
                    format!("{} at {}", g.away.name, g.home.name)
                } else {
                    match core {
                        "leg" => "pick".to_string(),
                        "legs" => "picks".to_string(),
                        "book" | "books" | "orderbook" | "orderbooks" => "prices".to_string(),
                        other => other.to_string(),
                    }
                };
            format!("{}{swapped}{rest}", if open { "(" } else { "" })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Why `PUT /profile` wrote nothing (6.1e).
#[derive(Debug, Clone, PartialEq)]
pub enum ProfileWriteError {
    /// Demo mode: no desk.
    Demo,
    /// 409 (taken, too soon, already titled, started), 422 (malformed, stale, not yours) or 429
    /// (rate limit), with the desk's reason.
    Refused { status: u16, reason: String },
    /// Anything else.
    Desk(DeskError),
}

/// Where the data comes from. Provided once as context by the root component.
#[derive(Debug, Clone, PartialEq)]
pub enum Desk {
    Live { base: String },
    Demo,
}

impl Desk {
    pub fn from_config() -> Self {
        if crate::config::DESIGN_FIXTURES {
            return Desk::Demo;
        }
        match crate::config::desk_url() {
            Some(base) => Desk::Live { base },
            None => Desk::Demo,
        }
    }

    pub fn is_demo(&self) -> bool {
        matches!(self, Desk::Demo)
    }

    /// `ws(s)://…/stream`, or `None` in demo mode.
    pub fn stream_url(&self) -> Option<String> {
        match self {
            Desk::Live { base } => Some(format!("{}/stream", base.replacen("http", "ws", 1))),
            Desk::Demo => None,
        }
    }

    /// `GET /health`: the desk's profile (`dev`, `demo`, `live`) and pause flag. Demo mode
    /// answers `None` (no desk).
    pub async fn health(&self) -> Result<Option<types::Health>, DeskError> {
        match self {
            Desk::Demo => Ok(None),
            Desk::Live { base } => send(Request::get(&format!("{base}/health")))
                .await
                .map(Some),
        }
    }

    /// `GET /card?sport=nba&date=` (date `YYYY-MM-DD`; `None` sends none: every game the desk
    /// lists that is not over, whatever its day, 2 Nov). `sport` `"all"` (L7-1) sends no `sport`,
    /// so the desk answers with every sport (7.0b, D-462).
    pub async fn card(&self, sport: &str, date: Option<&str>) -> Result<Card, DeskError> {
        match self {
            Desk::Demo => decode(fixtures::CARD),
            Desk::Live { base } => {
                let url = format!("{base}/card");
                let query: Vec<(&str, &str)> = [("sport", sport)]
                    .into_iter()
                    .filter(|_| sport != "all")
                    .chain(date.map(|d| ("date", d)))
                    .collect();
                send(Request::get(&url).query(query)).await
            }
        }
    }

    /// `GET /sentiment?symbols=` (L5-4): what others think, per game of the card.
    pub async fn sentiment(&self, symbols: &[String]) -> Result<SentimentResponse, DeskError> {
        match self {
            Desk::Demo => decode(fixtures::SENTIMENT),
            Desk::Live { base } => {
                let url = format!("{base}/sentiment");
                let joined = symbols.join(",");
                send(Request::get(&url).query([("symbols", joined.as_str())])).await
            }
        }
    }

    /// `POST /quote`. In demo mode the desk's example quote is returned as is, whatever the
    /// picks: the client never computes odds, even for a demo.
    pub async fn quote(&self, req: &QuoteRequest) -> Result<Quote, DeskError> {
        match self {
            Desk::Demo if req.legs.is_empty() => decode_error(fixtures::REJECTED, 422),
            // A copy (L9: the drawer's copy mode) is priced like the copy-quote, with its fee.
            Desk::Demo if req.copied_from.is_some() => decode(fixtures::COPY_QUOTE),
            Desk::Demo => decode(fixtures::QUOTE),
            Desk::Live { base } => {
                let url = format!("{base}/quote");
                let builder = Request::post(&url)
                    .json(req)
                    .map_err(|e| DeskError::Decode(e.to_string()))?;
                let resp = builder
                    .send()
                    .await
                    .map_err(|e| DeskError::Network(e.to_string()))?;
                read(resp).await
            }
        }
    }

    /// `GET /fx` (L6-2, U-4): the euro rate for showing USDC amounts in euros.
    pub async fn fx(&self) -> Result<types::Fx, DeskError> {
        match self {
            Desk::Demo => decode(fixtures::FX),
            Desk::Live { base } => send(Request::get(&format!("{base}/fx"))).await,
        }
    }

    /// `GET /profile/{wallet}` (L6-2): the wallet's handle. Demo mode answers the example
    /// profile for its own wallet and "no handle" for any other.
    pub async fn profile(&self, wallet: &str) -> Result<types::Profile, DeskError> {
        if !is_address(wallet) {
            return Err(DeskError::Http(404));
        }
        match self {
            Desk::Demo => {
                let p: types::Profile = decode(fixtures::PROFILE)?;
                Ok(if p.wallet == wallet {
                    p
                } else {
                    types::Profile {
                        wallet: wallet.to_string(),
                        handle: None,
                        since: 0,
                    }
                })
            }
            Desk::Live { base } => send(Request::get(&format!("{base}/profile/{wallet}"))).await,
        }
    }

    /// `PUT /profile` (L6-2, 6.1e): set the signed handle and, with `ticket`, that own tip's
    /// title. The desk's 409 and 429 answers keep their status and reason, so the screen can
    /// word them. Demo mode has no desk to write to.
    pub async fn put_profile(
        &self,
        body: &types::ProfileUpdate,
    ) -> Result<types::Profile, ProfileWriteError> {
        match self {
            Desk::Demo => Err(ProfileWriteError::Demo),
            Desk::Live { base } => {
                let resp = Request::put(&format!("{base}/profile"))
                    .json(body)
                    .map_err(|e| ProfileWriteError::Desk(DeskError::Decode(e.to_string())))?
                    .send()
                    .await
                    .map_err(|e| ProfileWriteError::Desk(DeskError::Network(e.to_string())))?;
                let status = resp.status();
                if matches!(status, 409 | 422 | 429) {
                    let text = resp.text().await.unwrap_or_default();
                    let reason = serde_json::from_str::<ErrorBody>(&text)
                        .map(|b| b.reason)
                        .unwrap_or_default();
                    return Err(ProfileWriteError::Refused { status, reason });
                }
                read(resp).await.map_err(ProfileWriteError::Desk)
            }
        }
    }

    /// L7-1 (7.1d): `GET /profile/{wallet}` with the v2 fields (links, follower counts,
    /// `verified`). Demo mode answers the example profile for its wallet and an empty one
    /// (no handle, no links, no followers) for any other.
    pub async fn public_profile(&self, wallet: &str) -> Result<types::PublicProfile, DeskError> {
        if !is_address(wallet) {
            return Err(DeskError::Http(404));
        }
        match self {
            Desk::Demo => {
                let p: types::PublicProfile = decode(fixtures::PROFILE)?;
                Ok(if p.wallet == wallet {
                    p
                } else {
                    types::PublicProfile {
                        wallet: wallet.to_string(),
                        handle: None,
                        since: 0,
                        verified: false,
                        links: None,
                        followers: Some(0),
                        following: Some(0),
                    }
                })
            }
            Desk::Live { base } => send(Request::get(&format!("{base}/profile/{wallet}"))).await,
        }
    }

    /// L7-1 (7.1d): `PUT /profile` with `links` or `follow`, signed over the v2 message
    /// (D-471). Answers the wallet's profile after the write; 409, 422 and 429 keep their
    /// reason, as [`Desk::put_profile`]. Demo mode has no desk to write to.
    pub async fn put_profile_v2(
        &self,
        body: &types::ProfileUpdateV2,
    ) -> Result<types::PublicProfile, ProfileWriteError> {
        match self {
            Desk::Demo => Err(ProfileWriteError::Demo),
            Desk::Live { base } => {
                let resp = Request::put(&format!("{base}/profile"))
                    .json(body)
                    .map_err(|e| ProfileWriteError::Desk(DeskError::Decode(e.to_string())))?
                    .send()
                    .await
                    .map_err(|e| ProfileWriteError::Desk(DeskError::Network(e.to_string())))?;
                let status = resp.status();
                if matches!(status, 409 | 422 | 429) {
                    let text = resp.text().await.unwrap_or_default();
                    let reason = serde_json::from_str::<ErrorBody>(&text)
                        .map(|b| b.reason)
                        .unwrap_or_default();
                    return Err(ProfileWriteError::Refused { status, reason });
                }
                read(resp).await.map_err(ProfileWriteError::Desk)
            }
        }
    }

    /// L7-1 (7.1d): `GET /tipsters/{wallet}/copiers?ticket=<pda>`, newest first. Both ids come
    /// from links, so anything but base58 addresses is refused before it reaches the URL. Demo
    /// mode: the design build's list for its tip, else no copies.
    pub async fn copiers(
        &self,
        wallet: &str,
        ticket: &str,
    ) -> Result<types::CopiersResponse, DeskError> {
        if !is_address(wallet) || !is_address(ticket) {
            return Err(DeskError::Http(404));
        }
        match self {
            Desk::Demo => {
                let design = fixtures::design::COPIERS
                    .and_then(|j| decode::<types::CopiersResponse>(j).ok())
                    .filter(|c| c.ticket == ticket);
                Ok(design.unwrap_or(types::CopiersResponse {
                    ticket: ticket.to_string(),
                    copiers: Vec::new(),
                    pool_u: 0,
                }))
            }
            Desk::Live { base } => {
                send(
                    Request::get(&format!("{base}/tipsters/{wallet}/copiers"))
                        .query([("ticket", ticket)]),
                )
                .await
            }
        }
    }

    /// `GET /tickets/:user`.
    pub async fn tickets(&self, user: &str) -> Result<TicketsResponse, DeskError> {
        match self {
            Desk::Demo => decode(fixtures::TICKETS),
            Desk::Live { base } => send(Request::get(&format!("{base}/tickets/{user}"))).await,
        }
    }

    /// `GET /ticket/:id`. The id comes from a link, so anything but a base58 address is
    /// refused before it reaches a URL path. Demo mode looks the id up in every example
    /// ticket (My schedine's, the tipster's).
    pub async fn ticket(&self, id: &str) -> Result<Ticket, DeskError> {
        if !is_address(id) {
            return Err(DeskError::Http(404));
        }
        match self {
            Desk::Demo => demo_tickets()
                .into_iter()
                .find(|t| t.ticket == id)
                .ok_or(DeskError::Http(404)),
            Desk::Live { base } => send(Request::get(&format!("{base}/ticket/{id}"))).await,
        }
    }

    /// `POST /ticket/{id}/copy-quote {"stake_u"}` (L4-2): the original's legs priced fresh,
    /// with `copy_fee_u`. Demo mode answers the example copy quote whatever the stake, as
    /// [`Desk::quote`] does.
    pub async fn copy_quote(&self, id: &str, stake_u: u64) -> Result<Quote, DeskError> {
        if !is_address(id) {
            return Err(DeskError::Http(404));
        }
        match self {
            Desk::Demo => decode(fixtures::COPY_QUOTE),
            Desk::Live { base } => {
                let url = format!("{base}/ticket/{id}/copy-quote");
                let builder = Request::post(&url)
                    .json(&CopyQuoteRequest { stake_u })
                    .map_err(|e| DeskError::Decode(e.to_string()))?;
                let resp = builder
                    .send()
                    .await
                    .map_err(|e| DeskError::Network(e.to_string()))?;
                read(resp).await
            }
        }
    }

    /// `GET /tipsters?sort=&all=`. Demo mode filters and orders the example rows the way
    /// L4-2 says the desk does (the bar: 5 settled and €20 of own money staked).
    pub async fn tipsters(
        &self,
        sort: TipsterSort,
        all: bool,
    ) -> Result<TipstersResponse, DeskError> {
        match self {
            Desk::Demo => {
                let mut resp: TipstersResponse = decode(fixtures::TIPSTERS)?;
                if !all {
                    resp.tipsters
                        .retain(|t| t.settled >= BOARD_MIN_SETTLED && t.skin_u >= BOARD_MIN_SKIN_U);
                }
                demo_sort(&mut resp.tipsters, sort);
                Ok(resp)
            }
            Desk::Live { base } => {
                let all = if all { "1" } else { "0" };
                send(
                    Request::get(&format!("{base}/tipsters"))
                        .query([("sort", sort.param()), ("all", all)]),
                )
                .await
            }
        }
    }

    /// `GET /tipsters/{wallet}`; 404 for a wallet with no own tips. Demo mode has one full
    /// example profile; another example row gets its row and no tickets.
    pub async fn tipster(&self, wallet: &str) -> Result<TipsterProfile, DeskError> {
        if !is_address(wallet) {
            return Err(DeskError::Http(404));
        }
        match self {
            Desk::Demo => {
                let profile: TipsterProfile = decode(fixtures::TIPSTER)?;
                if profile.tipster.wallet == wallet {
                    return Ok(profile);
                }
                let rows: TipstersResponse = decode(fixtures::TIPSTERS)?;
                rows.tipsters
                    .into_iter()
                    .find(|t| t.wallet == wallet)
                    .map(|tipster| TipsterProfile {
                        tipster,
                        history: Vec::new(),
                        settled: Vec::new(),
                        open: Vec::new(),
                    })
                    .ok_or(DeskError::Http(404))
            }
            Desk::Live { base } => send(Request::get(&format!("{base}/tipsters/{wallet}"))).await,
        }
    }

    /// `GET /house`.
    pub async fn house(&self) -> Result<House, DeskError> {
        match self {
            Desk::Demo => decode(fixtures::HOUSE),
            Desk::Live { base } => send(Request::get(&format!("{base}/house"))).await,
        }
    }

    /// `GET /tokens` (L4-3): mock USDC first, then every mock mint with a pool. Only mock USDC
    /// where the cluster has no AMM.
    pub async fn tokens(&self) -> Result<Vec<TokenInfo>, DeskError> {
        let resp: TokensResponse = match self {
            Desk::Demo => decode(fixtures::TOKENS)?,
            Desk::Live { base } => send(Request::get(&format!("{base}/tokens"))).await?,
        };
        Ok(resp.tokens)
    }

    /// `POST /swap-quote {"mint","amount_in"}` (L4-3). Demo mode answers the example swap
    /// whatever the amount, as [`Desk::quote`] does.
    pub async fn swap_quote(&self, mint: &str, amount_in: u64) -> Result<SwapQuote, DeskError> {
        match self {
            Desk::Demo => decode(fixtures::SWAP_QUOTE),
            Desk::Live { base } => {
                let body = SwapQuoteRequest {
                    mint: mint.to_string(),
                    amount_in: amount_in.to_string(),
                };
                let resp = Request::post(&format!("{base}/swap-quote"))
                    .json(&body)
                    .map_err(|e| DeskError::Decode(e.to_string()))?
                    .send()
                    .await
                    .map_err(|e| DeskError::Network(e.to_string()))?;
                read(resp).await
            }
        }
    }

    /// `GET /relay` (L8-2): the desk's sponsor, or `None` when it sponsors nothing (`503`, a
    /// desk without the route, or demo mode). Only a desk that cannot be reached is an error.
    pub async fn relay_info(&self) -> Result<Option<types::RelayInfo>, DeskError> {
        match self {
            Desk::Demo => Ok(None),
            Desk::Live { base } => match send(Request::get(&format!("{base}/relay"))).await {
                Ok(info) => Ok(Some(info)),
                Err(DeskError::Unavailable(_) | DeskError::Http(404 | 405)) => Ok(None),
                Err(e) => Err(e),
            },
        }
    }

    /// `POST /relay {"tx"}` (L8-2): the desk adds the sponsor's signature and submits `tx_b64`
    /// ([`types::RelayRequest`]). Answers the transaction's signature (base58) once the RPC
    /// accepted it; `422` is [`DeskError::Rejected`] with the desk's reason, `503` (sponsoring
    /// off, or the RPC down) [`DeskError::Unavailable`]. Demo mode sends nothing.
    pub async fn relay(&self, tx_b64: String) -> Result<String, DeskError> {
        match self {
            Desk::Demo => Err(DeskError::Unavailable(String::new())),
            Desk::Live { base } => {
                let body = types::RelayRequest { tx: tx_b64 };
                let resp = Request::post(&format!("{base}/relay"))
                    .json(&body)
                    .map_err(|e| DeskError::Decode(e.to_string()))?
                    .send()
                    .await
                    .map_err(|e| DeskError::Network(e.to_string()))?;
                read::<types::RelayResponse>(resp)
                    .await
                    .map(|r| r.signature)
            }
        }
    }

    /// `GET /cashout-quote/:ticket`.
    pub async fn cashout_quote(&self, ticket: &str) -> Result<CashoutQuote, DeskError> {
        match self {
            Desk::Demo => decode(fixtures::CASHOUT_QUOTE),
            Desk::Live { base } => {
                send(Request::get(&format!("{base}/cashout-quote/{ticket}"))).await
            }
        }
    }
}

/// Demo mode: the pending PDA of a fixture `progress` frame.
pub fn demo_pending(frame: &str) -> Option<String> {
    match serde_json::from_str::<types::Frame>(frame).ok()? {
        types::Frame::Progress { pending, .. } => Some(pending),
        _ => None,
    }
}

/// The leaderboard's bar (spec 6, L4-2): at least 5 settled own tips and €20 of own money
/// staked over 30 days. The desk applies it; the app only names it and mirrors it in demo mode.
pub const BOARD_MIN_SETTLED: u32 = 5;
pub const BOARD_MIN_SKIN_U: u64 = 20_000_000;

/// Demo mode: the desk's order (best first; Brier lower is better; no followers' result last).
fn demo_sort(rows: &mut [types::TipsterRow], sort: TipsterSort) {
    use std::cmp::Reverse;
    match sort {
        TipsterSort::Roi => rows.sort_by_key(|t| Reverse(t.roi_ppm)),
        TipsterSort::Brier => rows.sort_by_key(|t| t.brier_ppm),
        TipsterSort::Skin => rows.sort_by_key(|t| Reverse(t.skin_u)),
        TipsterSort::FollowersRoi => {
            rows.sort_by_key(|t| (t.followers_roi_ppm.is_none(), Reverse(t.followers_roi_ppm)))
        }
    }
}

/// Every example ticket: My schedine's list, the single ticket, the tipster's tips.
fn demo_tickets() -> Vec<Ticket> {
    let mut all = decode::<TicketsResponse>(fixtures::TICKETS)
        .map(|r| r.tickets)
        .unwrap_or_default();
    all.extend(decode::<Ticket>(fixtures::TICKET));
    if let Ok(p) = decode::<TipsterProfile>(fixtures::TIPSTER) {
        all.extend(p.open);
        all.extend(p.settled);
    }
    all
}

/// A base58 Solana address (32 bytes: 32 to 44 characters of the base58 alphabet). Ids from
/// links are checked with it before they become part of a desk URL path.
pub fn is_address(s: &str) -> bool {
    (32..=44).contains(&s.len()) && is_base58(s)
}

/// A base58 transaction signature (64 bytes: 64 to 88 characters). Only these become explorer
/// links.
pub fn is_signature(s: &str) -> bool {
    (64..=88).contains(&s.len()) && is_base58(s)
}

fn is_base58(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() && !matches!(b, b'0' | b'O' | b'I' | b'l'))
}

/// The demo-mode user: the owner of the example tickets.
pub fn demo_user() -> Option<String> {
    decode::<TicketsResponse>(fixtures::TICKETS)
        .ok()
        .map(|t| t.user)
}

async fn send<T: DeserializeOwned>(req: gloo_net::http::RequestBuilder) -> Result<T, DeskError> {
    let resp = req
        .send()
        .await
        .map_err(|e| DeskError::Network(e.to_string()))?;
    read(resp).await
}

async fn read<T: DeserializeOwned>(resp: Response) -> Result<T, DeskError> {
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| DeskError::Network(e.to_string()))?;
    if (200..300).contains(&status) {
        decode(&text)
    } else {
        decode_error(&text, status)
    }
}

fn decode<T: DeserializeOwned>(text: &str) -> Result<T, DeskError> {
    serde_json::from_str(text).map_err(|e| DeskError::Decode(e.to_string()))
}

/// Map a non-2xx answer to the C4 error kinds; the body is optional.
fn decode_error<T>(text: &str, status: u16) -> Result<T, DeskError> {
    let body: Option<ErrorBody> = serde_json::from_str(text).ok();
    let reason = body.map(|b| b.reason).unwrap_or_default();
    Err(match status {
        422 => DeskError::Rejected(reason),
        503 => DeskError::Unavailable(reason),
        s => DeskError::Http(s),
    })
}

/// The night the Card asks for: the build's `SCHEDINA_SLATE_DATE` (a replayed tape's night),
/// else `None`, every game the desk lists that is not over, whatever its day (2 Nov: the live
/// card used to be today's US Eastern date only, so late in the evening it held one or two
/// games while Pascal listed the next day's).
pub fn slate_date() -> Option<String> {
    crate::config::slate_date_override()
}
