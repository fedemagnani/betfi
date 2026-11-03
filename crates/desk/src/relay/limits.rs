//! The relay's rate limits (rule 6), in memory and per process: relayed transactions per user
//! and per client IP over a sliding hour, the simulated sponsor spend per UTC day, every
//! `POST /relay` attempt per IP (refused ones included, so a caller cannot make the desk simulate
//! without end), and the token accounts the sponsor already paid for. A restart forgets them
//! (the daily budget then starts again; the per-tx cost cap still holds).

use std::collections::{HashMap, HashSet, VecDeque};

use solana_pubkey::Pubkey;
use std::sync::Mutex;

/// Relayed transactions per user per hour.
pub const PER_USER_PER_HOUR: usize = 30;
/// Relayed transactions per client IP per hour.
pub const PER_IP_PER_HOUR: usize = 60;
/// `POST /relay` attempts (refused or not) per client IP per hour.
pub const ATTEMPTS_PER_IP_PER_HOUR: usize = 300;
const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 86_400_000;

/// The limits and what has been spent against them. Its lock is taken alone and never across an
/// `.await`.
#[derive(Debug)]
pub struct Limits {
    daily_lamports: u64,
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    users: HashMap<String, VecDeque<u64>>,
    ips: HashMap<String, VecDeque<u64>>,
    attempts: HashMap<String, VecDeque<u64>>,
    /// Token accounts whose creation the sponsor paid for.
    atas: HashSet<Pubkey>,
    day: u64,
    spent: u64,
}

fn trim(map: &mut HashMap<String, VecDeque<u64>>, key: &str, now_ms: u64) -> usize {
    let since = now_ms.saturating_sub(HOUR_MS);
    if map.len() > 10_000 {
        map.retain(|_, q| q.back().is_some_and(|t| *t > since));
    }
    map.get_mut(key).map_or(0, |q| {
        while q.front().is_some_and(|t| *t <= since) {
            q.pop_front();
        }
        q.len()
    })
}

impl Limits {
    /// Limits with a daily budget of `daily_lamports` of simulated sponsor spend.
    pub fn new(daily_lamports: u64) -> Self {
        Limits {
            daily_lamports,
            inner: Mutex::default(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether one more transaction of `spend` lamports for `user` from `ip` fits at `now_ms`,
    /// recording nothing (checked before the simulation, so a limited caller costs no RPC).
    pub fn allows(&self, user: &str, ip: &str, spend: u64, now_ms: u64) -> bool {
        let mut g = self.lock();
        Self::fits(&mut g, self.daily_lamports, user, ip, spend, now_ms)
    }

    /// Like [`Self::allows`], and records the transaction when it fits.
    pub fn take(&self, user: &str, ip: &str, spend: u64, now_ms: u64) -> bool {
        let mut g = self.lock();
        if !Self::fits(&mut g, self.daily_lamports, user, ip, spend, now_ms) {
            return false;
        }
        g.users
            .entry(user.to_owned())
            .or_default()
            .push_back(now_ms);
        g.ips.entry(ip.to_owned()).or_default().push_back(now_ms);
        g.spent = g.spent.saturating_add(spend);
        true
    }

    fn fits(g: &mut Inner, daily: u64, user: &str, ip: &str, spend: u64, now_ms: u64) -> bool {
        let day = now_ms / DAY_MS;
        if g.day != day {
            g.day = day;
            g.spent = 0;
        }
        trim(&mut g.users, user, now_ms) < PER_USER_PER_HOUR
            && trim(&mut g.ips, ip, now_ms) < PER_IP_PER_HOUR
            && g.spent.saturating_add(spend) <= daily
    }

    /// Counts one `POST /relay` attempt from `ip` at `now_ms`; false once the IP made
    /// [`ATTEMPTS_PER_IP_PER_HOUR`] in the last hour (the attempt is then not counted).
    pub fn attempt(&self, ip: &str, now_ms: u64) -> bool {
        let mut g = self.lock();
        if trim(&mut g.attempts, ip, now_ms) >= ATTEMPTS_PER_IP_PER_HOUR {
            return false;
        }
        g.attempts
            .entry(ip.to_owned())
            .or_default()
            .push_back(now_ms);
        true
    }

    /// Whether the sponsor already paid to create the token account `ata` (it has been closed
    /// since if it is missing again).
    pub fn ata_paid(&self, ata: &Pubkey) -> bool {
        self.lock().atas.contains(ata)
    }

    /// Records that the sponsor paid to create each of `atas`.
    pub fn record_atas(&self, atas: &[Pubkey]) {
        let mut g = self.lock();
        if g.atas.len() > 1_000_000 {
            // Bounded memory; at worst a closed account can be re-funded once more.
            g.atas.clear();
        }
        g.atas.extend(atas.iter().copied());
    }

    /// Simulated sponsor spend so far this UTC day, lamports.
    pub fn spent_today(&self, now_ms: u64) -> u64 {
        let g = self.lock();
        if g.day == now_ms / DAY_MS { g.spent } else { 0 }
    }
}
