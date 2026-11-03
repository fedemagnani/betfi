//! Client-side rate limiting: a token bucket per host (plus per-route buckets for heavy
//! endpoints) and exponential backoff with jitter.
//!
//! Pascal publishes no read limits beyond text search (60/min/IP), but the ~5 MB markets list
//! answers a second call within seconds with HTTP 429. Every request takes a token from its
//! host's bucket; the markets list also takes one from a slow dedicated bucket. When a 429
//! arrives the whole host bucket is paused, so concurrent tasks back off together.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::Instant;

/// Rate and burst of one token bucket.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BucketConfig {
    /// Sustained requests per second.
    pub per_second: f64,
    /// Maximum burst (bucket capacity), at least 1.
    pub burst: f64,
}

impl BucketConfig {
    /// `n` requests per `period`, with a burst of `burst`.
    pub fn per(n: f64, period: Duration, burst: f64) -> Self {
        BucketConfig {
            per_second: n / period.as_secs_f64(),
            burst: burst.max(1.0),
        }
    }
}

/// A token bucket. Waiters sleep until a token is available.
#[derive(Debug)]
pub struct TokenBucket {
    config: BucketConfig,
    state: Mutex<BucketState>,
}

#[derive(Debug)]
struct BucketState {
    tokens: f64,
    last: Instant,
    paused_until: Option<Instant>,
}

impl TokenBucket {
    /// A full bucket.
    pub fn new(config: BucketConfig) -> Self {
        TokenBucket {
            config,
            state: Mutex::new(BucketState {
                tokens: config.burst,
                last: Instant::now(),
                paused_until: None,
            }),
        }
    }

    /// Takes one token, sleeping until one is available.
    pub async fn acquire(&self) {
        loop {
            let wait = self.try_take(Instant::now());
            match wait {
                None => return,
                Some(d) => tokio::time::sleep(d).await,
            }
        }
    }

    /// Takes a token if available, else returns how long to wait before retrying.
    fn try_take(&self, now: Instant) -> Option<Duration> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(until) = s.paused_until {
            if now < until {
                return Some(until - now);
            }
            s.paused_until = None;
            // A pause empties the bucket: resume at the sustained rate, not with a burst.
            s.tokens = s.tokens.min(1.0);
            s.last = until;
        }
        let elapsed = now.saturating_duration_since(s.last).as_secs_f64();
        s.tokens = (s.tokens + elapsed * self.config.per_second).min(self.config.burst);
        s.last = now;
        if s.tokens >= 1.0 {
            s.tokens -= 1.0;
            None
        } else {
            let secs = (1.0 - s.tokens) / self.config.per_second;
            Some(Duration::from_secs_f64(secs.max(0.001)))
        }
    }

    /// Blocks every acquirer for `d` (used after a 429), and empties the bucket.
    pub fn pause_for(&self, d: Duration) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let until = Instant::now() + d;
        if s.paused_until.is_none_or(|u| u < until) {
            s.paused_until = Some(until);
        }
        s.tokens = 0.0;
    }
}

/// Limits for [`RateLimiter`].
#[derive(Debug, Clone, PartialEq)]
pub struct RateLimits {
    /// Bucket shared by every request to one host.
    pub per_host: BucketConfig,
    /// Extra buckets for specific routes, keyed by path (e.g. `/api/v1/markets`).
    pub per_route: HashMap<String, BucketConfig>,
}

/// Path of the ~5 MB live market list, which has its own slow bucket by default.
pub const MARKETS_LIST_ROUTE: &str = "/api/v1/markets";

impl Default for RateLimits {
    /// 5 requests/s per host with a burst of 10; the markets list at most once per 30 s.
    fn default() -> Self {
        let mut per_route = HashMap::new();
        per_route.insert(
            MARKETS_LIST_ROUTE.to_owned(),
            BucketConfig::per(1.0, Duration::from_secs(30), 1.0),
        );
        RateLimits {
            per_host: BucketConfig {
                per_second: 5.0,
                burst: 10.0,
            },
            per_route,
        }
    }
}

/// Token buckets keyed by host and by `host + route`. Share one instance (it is cheap to clone
/// the [`Arc`]) between every client that talks to the same host.
#[derive(Debug, Default)]
pub struct RateLimiter {
    limits: RateLimits,
    buckets: Mutex<HashMap<String, Arc<TokenBucket>>>,
}

impl RateLimiter {
    /// A limiter with the given limits.
    pub fn new(limits: RateLimits) -> Self {
        RateLimiter {
            limits,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    fn bucket(&self, key: String, config: BucketConfig) -> Arc<TokenBucket> {
        let mut b = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        b.entry(key)
            .or_insert_with(|| Arc::new(TokenBucket::new(config)))
            .clone()
    }

    /// The bucket shared by all requests to `host`.
    pub fn host_bucket(&self, host: &str) -> Arc<TokenBucket> {
        self.bucket(host.to_owned(), self.limits.per_host)
    }

    /// Waits for a token for `host` (and for `route` when it has its own bucket).
    pub async fn acquire(&self, host: &str, route: &str) {
        if let Some(cfg) = self.limits.per_route.get(route).copied() {
            self.bucket(format!("{host}{route}"), cfg).acquire().await;
        }
        self.host_bucket(host).acquire().await;
    }
}

/// Exponential backoff with jitter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Backoff {
    /// Delay before the first retry (before jitter).
    pub base: Duration,
    /// Upper bound on any delay.
    pub max: Duration,
}

impl Backoff {
    /// Delay before retry number `attempt` (0-based): `min(max, base · 2^attempt)`, of which the
    /// upper half is randomised ("equal jitter"), so delays never collapse to zero.
    pub fn delay(&self, attempt: u32) -> Duration {
        let exp = self
            .base
            .saturating_mul(2u32.saturating_pow(attempt.min(30)));
        let cap = exp.min(self.max);
        let half = cap / 2;
        half + half.mul_f64(fastrand::f64())
    }
}

/// When and how often to retry a REST request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryPolicy {
    /// Maximum attempts in total (1 = no retry).
    pub max_attempts: u32,
    /// Backoff between attempts.
    pub backoff: Backoff,
}

impl Default for RetryPolicy {
    /// 8 attempts, 500 ms doubling up to 60 s (about 2.5 minutes of patience in total).
    fn default() -> Self {
        RetryPolicy {
            max_attempts: 8,
            backoff: Backoff {
                base: Duration::from_millis(500),
                max: Duration::from_secs(60),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        let b = Backoff {
            base: Duration::from_millis(100),
            max: Duration::from_secs(2),
        };
        for attempt in 0..40 {
            let d = b.delay(attempt);
            let cap = (Duration::from_millis(100) * 2u32.saturating_pow(attempt.min(30)))
                .min(Duration::from_secs(2));
            assert!(
                d >= cap / 2 && d <= cap,
                "attempt {attempt}: {d:?} vs {cap:?}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn bucket_limits_rate() {
        let bucket = TokenBucket::new(BucketConfig {
            per_second: 2.0,
            burst: 2.0,
        });
        let start = Instant::now();
        for _ in 0..6 {
            bucket.acquire().await;
        }
        // 2 from the burst, then 4 more at 2/s = 2 s.
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(1990) && elapsed < Duration::from_millis(2100),
            "{elapsed:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn pause_blocks_acquirers() {
        let bucket = TokenBucket::new(BucketConfig {
            per_second: 100.0,
            burst: 10.0,
        });
        bucket.pause_for(Duration::from_secs(5));
        let start = Instant::now();
        bucket.acquire().await;
        assert!(start.elapsed() >= Duration::from_secs(5));
    }

    #[tokio::test(start_paused = true)]
    async fn route_bucket_is_separate() {
        let limiter = RateLimiter::new(RateLimits::default());
        let start = Instant::now();
        limiter.acquire("h", MARKETS_LIST_ROUTE).await;
        limiter.acquire("h", "/api/v1/time").await;
        assert!(start.elapsed() < Duration::from_millis(10));
        limiter.acquire("h", MARKETS_LIST_ROUTE).await;
        assert!(start.elapsed() >= Duration::from_secs(29));
    }
}
