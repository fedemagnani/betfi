//! Live scores of started games, and which games are over (31 Oct).
//!
//! Pascal publishes no scores (D-300) and resolves a market some time after the final whistle,
//! so between the two a game used to stay on `/card` as live with its book pinned at 0.999 /
//! 0.001. Every Pascal sports market names its Polymarket reference market
//! (`display_attributes.reference.market_slug`, e.g. `bun-bay-dor-2026-10-31-dor`); the event
//! slug is that slug up to its date (`bun-bay-dor-2026-10-31`), and Polymarket's public Gamma
//! API answers `GET /events?slug=…&slug=…` with each event's `score` (`"2-1"`, in the order of
//! its `teams`, each tagged `ordering: home|away`), `period` (`1H`, `HT`, `2H`, `Q3`, `VFT`…),
//! `elapsed` (minutes, or a game clock) and `live` / `ended` flags.
//!
//! [`run`] reads them every `DESK_SCORES_S` (20 s) for the games that have started and are not
//! over, in batches of [`BATCH`], and keeps the answers in memory ([`ScoreCache`], in
//! [`crate::state::Shared::scores`]). Nothing waits on the network: `/card` and the quoter only
//! read the cache. It runs only while the venue clock is the wall clock: a replayed tape's games
//! are not the ones Polymarket reports on today.
//!
//! [`finished`] decides when a game is over: its winner book is decided (an outcome at
//! [`DECIDED_PRICE_U`] or more), started or not and whatever the feed says (2 Nov: nothing is
//! left to price); or, once started, the feed says so; or, with no live reading from the feed,
//! Pascal's expected resolution time is [`RESOLUTION_GRACE_MS`] behind. `/card` leaves such a
//! game out ([`crate::catalogue::card`]) and the quoter refuses its legs.
//!
//! Gamma is a read-only host with no key; it is not a Pascal host (the `live_guard` scan is
//! unaffected). `DESK_SCORES_URL` defaults to [`GAMMA_URL_DEFAULT`] on `demo` and `live`,
//! nothing on `dev` and in [`crate::Config::local`] (an empty URL never fetches).

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;

use crate::api::types::GameScore;
use crate::catalogue::Catalogue;
use crate::now_ms;
use crate::state::AppState;

/// Default `DESK_SCORES_URL`: Polymarket's Gamma API.
pub const GAMMA_URL_DEFAULT: &str = "https://gamma-api.polymarket.com";
/// Event slugs per request.
pub const BATCH: usize = 20;
/// Per-request timeout.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest answer read, bytes (an event with its markets is about 20 KB).
pub const MAX_BODY: usize = 4 * 1024 * 1024;
/// A reading older than this no longer says the game is live, nor shows its score, ms.
pub const STALE_MS: u64 = 3 * 60_000;
/// A started game is followed for this long after its start, ms.
pub const FOLLOW_MS: u64 = 6 * 3_600_000;
/// The venue clock may differ from the wall clock by this much for the feed to apply, ms.
pub const LIVE_CLOCK_SLACK_MS: u64 = 10 * 60_000;
/// With no live reading, a winner outcome priced at this or more means the game is decided.
pub const DECIDED_PRICE_U: u64 = 990_000;
/// With no live reading, a game this long past Pascal's expected resolution time is over, ms.
pub const RESOLUTION_GRACE_MS: u64 = 30 * 60_000;

/// One game as the feed last reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    /// The score, once the feed has one.
    pub score: Option<GameScore>,
    /// The feed says the game is being played.
    pub live: bool,
    /// The feed says the game is over (final).
    pub ended: bool,
    /// When it was read, Unix ms.
    pub read_ms: u64,
}

impl Reading {
    /// Whether it was read within [`STALE_MS`] of `now_ms`.
    pub fn fresh(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.read_ms) <= STALE_MS
    }
}

/// The latest reading per game id. Its lock is taken alone and never across an `.await`.
#[derive(Debug, Default)]
pub struct ScoreCache {
    games: Mutex<HashMap<String, Reading>>,
}

impl ScoreCache {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Reading>> {
        self.games
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `game_id`'s latest reading.
    pub fn get(&self, game_id: &str) -> Option<Reading> {
        self.lock().get(game_id).cloned()
    }

    /// Records `reading` for `game_id`. An ended game stays ended.
    pub fn set(&self, game_id: &str, reading: Reading) {
        let mut g = self.lock();
        let ended = g.get(game_id).is_some_and(|r| r.ended);
        let reading = Reading {
            ended: reading.ended || ended,
            live: reading.live && !ended,
            ..reading
        };
        g.insert(game_id.to_owned(), reading);
    }

    /// Whether the feed has said `game_id` is over.
    pub fn ended(&self, game_id: &str) -> bool {
        self.lock().get(game_id).is_some_and(|r| r.ended)
    }

    /// Forgets the games not in `keep` (resolved and gone from the catalogue).
    pub fn retain(&self, keep: &HashSet<String>) {
        self.lock().retain(|id, _| keep.contains(id));
    }
}

/// Whether a game is over at `venue_now` (see the module docs). `prices` are its winner
/// outcomes' prices, micro-USD.
pub fn finished(
    reading: Option<&Reading>,
    prices: &[u64],
    start_ms: u64,
    expected_resolution_ms: u64,
    venue_now: u64,
    wall_now: u64,
) -> bool {
    if prices.iter().any(|&p| p >= DECIDED_PRICE_U) {
        return true;
    }
    if venue_now < start_ms {
        return false;
    }
    if let Some(r) = reading {
        if r.ended {
            return true;
        }
        if r.live && r.fresh(wall_now) {
            return false;
        }
    }
    venue_now >= expected_resolution_ms.saturating_add(RESOLUTION_GRACE_MS)
}

/// The Polymarket event slug of a reference market slug: everything up to its `-YYYY-MM-DD`
/// date (`bun-bay-dor-2026-10-31-dor` → `bun-bay-dor-2026-10-31`; an NBA slug is already one).
pub fn event_slug(market_slug: &str) -> Option<&str> {
    let b = market_slug.as_bytes();
    let is_date = |i: usize| {
        b.len() >= i + 11
            && b[i] == b'-'
            && b[i + 5] == b'-'
            && b[i + 8] == b'-'
            && [1, 2, 3, 4, 6, 7, 9, 10]
                .iter()
                .all(|&k| b[i + k].is_ascii_digit())
    };
    (0..b.len())
        .rev()
        .find(|&i| is_date(i))
        .map(|i| &market_slug[..i + 11])
}

/// The two numbers of a score such as `"2-1"` or `"114 - 109"`.
fn score_pair(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.split_once('-')?;
    let num = |x: &str| {
        let d: String = x.trim().chars().take_while(char::is_ascii_digit).collect();
        d.parse::<u32>().ok()
    };
    Some((num(a)?, num(b.trim_start())?))
}

/// A non-empty string field.
fn text(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// One Gamma event as a [`Reading`] (read at `read_ms`), with its slug. The score's first
/// number belongs to the first of `teams`; with no `teams`, an `nba-` event lists the away team
/// first and any other the home team first (as its title does).
pub fn parse_event(e: &Value, read_ms: u64) -> Option<(String, Reading)> {
    let slug = e["slug"].as_str()?.to_owned();
    let first_is_home = match e["teams"].as_array().and_then(|t| t.first()) {
        Some(t) => t["ordering"].as_str() != Some("away"),
        None => !slug.starts_with("nba-"),
    };
    // "NS" (not started) lags the kick-off by minutes: its 0-0 is kept, for a game the venue
    // already has in play (the card attaches a score to those only), and the period dropped so
    // the clock counts from the start.
    let period = text(&e["period"]).filter(|p| p != "NS");
    let final_period = period
        .as_deref()
        .is_some_and(|p| matches!(p, "FT" | "VFT" | "AOT" | "AET" | "PEN" | "FT_PEN"));
    let ended = e["ended"].as_bool().unwrap_or(false) || final_period;
    let score = e["score"].as_str().and_then(score_pair).map(|(a, b)| {
        let (home, away) = if first_is_home { (a, b) } else { (b, a) };
        GameScore {
            home,
            away,
            period: period.clone(),
            elapsed: text(&e["elapsed"]),
        }
    });
    let live = e["live"].as_bool().unwrap_or(false) && !ended;
    Some((
        slug,
        Reading {
            score,
            live,
            ended,
            read_ms,
        },
    ))
}

/// Every event in a Gamma `/events` answer.
pub fn parse(body: &str, read_ms: u64) -> Result<Vec<(String, Reading)>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("not JSON: {e}"))?;
    let items = v.as_array().ok_or("not a list of events")?;
    Ok(items
        .iter()
        .filter_map(|e| parse_event(e, read_ms))
        .collect())
}

/// The started games to follow at `venue_now`: event slug → game id, for games started within
/// [`FOLLOW_MS`] that `skip` does not rule out. Also every game id on the catalogue.
pub fn to_follow(
    cat: &Catalogue,
    venue_now: u64,
    skip: impl Fn(&str) -> bool,
) -> (HashMap<String, String>, HashSet<String>) {
    let nba = cat.games.keys().filter_map(|id| cat.game(id));
    let soccer = cat
        .soccer
        .keys()
        .filter_map(|id| cat.soccer_game(id).map(|[home, ..]| home));
    let mut follow = HashMap::new();
    let mut all = HashSet::new();
    for m in nba.chain(soccer) {
        all.insert(m.game_id.clone());
        let started = m.start_ms <= venue_now && venue_now - m.start_ms <= FOLLOW_MS;
        if !started || skip(&m.game_id) {
            continue;
        }
        if let Some(slug) = m.polymarket_slug.as_deref().and_then(event_slug) {
            follow.insert(slug.to_owned(), m.game_id.clone());
        }
    }
    (follow, all)
}

/// The client [`run`] reads with.
pub fn client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .user_agent(concat!("schedina-desk/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// One read of `slugs` from `base` (`{base}/events?slug=…&slug=…`).
pub async fn fetch(
    client: &reqwest::Client,
    base: &str,
    slugs: &[&str],
) -> Result<Vec<(String, Reading)>, String> {
    let mut url = reqwest::Url::parse(&format!("{}/events", base.trim_end_matches('/')))
        .map_err(|e| format!("bad DESK_SCORES_URL: {e}"))?;
    // Gamma answers with `cache-control: max-age=300` behind CloudFront, so the same URL every
    // 20 s would read a score up to 5 minutes old: a parameter that changes on every read makes
    // each one a cache miss.
    url.query_pairs_mut()
        .extend_pairs(slugs.iter().map(|s| ("slug", *s)))
        .append_pair("_", &now_ms().to_string());
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("request failed: {}", e.without_url()))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("read failed: {}", e.without_url()))?
    {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BODY {
            return Err(format!("answer over {MAX_BODY} bytes"));
        }
    }
    let text = String::from_utf8(body).map_err(|_| "answer is not UTF-8".to_owned())?;
    parse(&text, now_ms())
}

/// One pass: reads the started games that are not over and records what the feed says.
async fn pass(state: &AppState, client: &reqwest::Client) {
    let Some(cat) = state.catalogue() else {
        return;
    };
    let wall = now_ms();
    let venue_now = cat.venue_now(wall);
    let (follow, all) = to_follow(&cat, venue_now, |id| state.scores.ended(id));
    state.scores.retain(&all);
    if venue_now.abs_diff(wall) > LIVE_CLOCK_SLACK_MS || follow.is_empty() {
        return;
    }
    let mut slugs: Vec<&str> = follow.keys().map(String::as_str).collect();
    slugs.sort_unstable();
    for batch in slugs.chunks(BATCH) {
        match fetch(client, &state.config.scores_url, batch).await {
            Ok(events) => {
                metrics::counter!("desk_scores_fetch_total", "result" => "ok").increment(1);
                for (slug, reading) in events {
                    if let Some(game_id) = follow.get(&slug) {
                        tracing::debug!(game_id, ?reading, "score read");
                        state.scores.set(game_id, reading);
                    }
                }
            }
            Err(e) => {
                metrics::counter!("desk_scores_fetch_total", "result" => "error").increment(1);
                tracing::warn!(error = %e, "scores: read failed; keeping the last readings");
            }
        }
    }
}

/// Reads the scores every `scores_every` until shutdown. Idle when `scores_url` is empty or
/// `DESK_SCORES_S=0` (`/card` then judges a game over from its book and Pascal's times alone).
pub async fn run(state: AppState) {
    if state.config.scores_url.is_empty() || state.config.scores_every.is_zero() {
        tracing::info!("scores: not fetched (no DESK_SCORES_URL or DESK_SCORES_S=0)");
        state.shutdown_requested().await;
        return;
    }
    let client = match client() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "scores: cannot build the client; not fetched");
            state.shutdown_requested().await;
            return;
        }
    };
    let mut tick = tokio::time::interval(state.config.scores_every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = state.shutdown_requested() => break,
            _ = tick.tick() => {}
        }
        tokio::select! {
            _ = state.shutdown_requested() => break,
            () = pass(&state, &client) => {}
        }
    }
    tracing::info!("scores stopped");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_slugs_end_at_the_date() {
        assert_eq!(
            event_slug("bun-bay-dor-2026-10-31-dor"),
            Some("bun-bay-dor-2026-10-31")
        );
        assert_eq!(
            event_slug("lal-elc-val-2026-11-01-draw"),
            Some("lal-elc-val-2026-11-01")
        );
        assert_eq!(
            event_slug("nba-okc-hou-2026-10-31"),
            Some("nba-okc-hou-2026-10-31")
        );
        assert_eq!(event_slug("no-date-here"), None);
        assert_eq!(event_slug("x-2026-1-31"), None);
    }

    #[test]
    fn a_soccer_score_is_home_first_an_nba_one_follows_its_teams() {
        // Gamma, 31 Oct 2026: Milan (home) 2-0 in the first half, minute 16.
        let soccer = json!({"slug": "sea-mil-int-2026-10-31", "score": "2-0", "period": "1H",
            "elapsed": "16", "live": true, "ended": false,
            "teams": [{"abbreviation": "mil", "ordering": "home"}, {"abbreviation": "int", "ordering": "away"}]});
        let (slug, r) = parse_event(&soccer, 7).unwrap();
        assert_eq!(slug, "sea-mil-int-2026-10-31");
        assert!(r.live && !r.ended);
        let s = r.score.unwrap();
        assert_eq!((s.home, s.away), (2, 0));
        assert_eq!(
            (s.period.as_deref(), s.elapsed.as_deref()),
            (Some("1H"), Some("16"))
        );

        // 28 Oct: "76ers vs. Pacers" 114-109, the 76ers away (and the winners).
        let nba = json!({"slug": "nba-phi-ind-2026-10-28", "score": "114-109", "period": "VFT",
            "elapsed": "", "live": false, "ended": true,
            "teams": [{"abbreviation": "phi", "ordering": "away"}, {"abbreviation": "ind", "ordering": "home"}]});
        let (_, r) = parse_event(&nba, 7).unwrap();
        assert!(r.ended && !r.live);
        let s = r.score.unwrap();
        assert_eq!((s.home, s.away, s.elapsed), (109, 114, None));

        // No teams: the title's order (NBA away first).
        let bare = json!({"slug": "nba-a-b-2026-10-28", "score": "100-90", "live": true});
        assert_eq!(
            parse_event(&bare, 7)
                .unwrap()
                .1
                .score
                .map(|s| (s.home, s.away)),
            Some((90, 100))
        );
    }

    #[test]
    fn a_not_started_event_keeps_its_0_0_and_a_scoreless_one_has_none() {
        let ns = json!({"slug": "nba-okc-hou-2026-10-31", "score": "0-0", "period": "NS"});
        let (_, r) = parse_event(&ns, 1).unwrap();
        let s = r.score.unwrap();
        assert_eq!((s.home, s.away, s.period), (0, 0, None));
        assert!(!r.live && !r.ended);
        let none = json!({"slug": "epl-ast-ful-2026-10-31", "score": null, "live": null});
        assert_eq!(parse_event(&none, 1).unwrap().1.score, None);
        // A final period alone says it ended.
        let ft = json!({"slug": "x-2026-10-31", "score": "1-1", "period": "FT", "live": true});
        let (_, r) = parse_event(&ft, 1).unwrap();
        assert!(r.ended && !r.live);
        assert_eq!(parse("[]", 1).unwrap(), vec![]);
        assert!(parse("{}", 1).is_err());
    }

    fn reading(live: bool, ended: bool, read_ms: u64) -> Reading {
        Reading {
            score: None,
            live,
            ended,
            read_ms,
        }
    }

    #[test]
    fn a_game_is_over_when_the_feed_says_so_or_its_book_is_decided() {
        let (start, exp) = (1_000_000, 1_000_000 + 7_200_000);
        let now = start + 60 * 60_000;
        let open = [480_000, 270_000, 250_000];
        let decided = [999_500, 1_000, 1_000];
        // A decided book is over, before the start too, whatever the feed says.
        assert!(finished(None, &decided, start, exp, start - 1, start - 1));
        assert!(!finished(None, &open, start, exp, start - 1, start - 1));
        // Started, the book still open: the feed's word.
        assert!(finished(
            Some(&reading(false, true, now)),
            &open,
            start,
            exp,
            now,
            now
        ));
        assert!(finished(
            Some(&reading(true, false, now)),
            &decided,
            start,
            exp,
            now,
            now
        ));
        assert!(!finished(
            Some(&reading(true, false, now)),
            &open,
            start,
            exp,
            now,
            now
        ));
        // No (fresh, live) reading: the book, then the expected resolution time.
        assert!(!finished(None, &open, start, exp, now, now));
        assert!(finished(None, &decided, start, exp, now, now));
        let stale = reading(true, false, now - STALE_MS - 1);
        assert!(finished(Some(&stale), &decided, start, exp, now, now));
        let late = exp + RESOLUTION_GRACE_MS;
        assert!(!finished(None, &open, start, exp, late - 1, late - 1));
        assert!(finished(None, &open, start, exp, late, late));
    }

    #[test]
    fn an_ended_game_stays_ended() {
        let c = ScoreCache::default();
        c.set("g", reading(false, true, 1));
        c.set("g", reading(true, false, 2));
        let r = c.get("g").unwrap();
        assert!(r.ended && !r.live);
        assert!(c.ended("g"));
        c.retain(&HashSet::new());
        assert_eq!(c.get("g"), None);
    }

    #[test]
    fn score_pairs() {
        assert_eq!(score_pair("2-1"), Some((2, 1)));
        assert_eq!(score_pair("114 - 109"), Some((114, 109)));
        assert_eq!(score_pair("1-1 (4-3)"), Some((1, 1)));
        assert_eq!(score_pair(""), None);
        assert_eq!(score_pair("a-b"), None);
    }
}

/// Reads today's events from the real Gamma API through [`fetch`] (network; run by hand:
/// `cargo test -p desk --lib scores::live -- --ignored --nocapture`).
#[cfg(test)]
mod live {
    #[tokio::test]
    #[ignore = "reads gamma-api.polymarket.com"]
    async fn reads_gamma() {
        let c = super::client().unwrap();
        let slugs = [
            "sea-mil-int-2026-10-31",
            "bun-bay-dor-2026-10-31",
            "lal-bar-ala-2026-10-31",
            "epl-ast-ful-2026-10-31",
        ];
        let got = super::fetch(&c, super::GAMMA_URL_DEFAULT, &slugs)
            .await
            .unwrap();
        for (slug, r) in &got {
            println!("{slug}: {r:?}");
        }
        assert_eq!(got.len(), slugs.len());
    }
}
