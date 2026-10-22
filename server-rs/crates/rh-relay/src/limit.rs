//! Per-connection token buckets (and the bucket every other limit is built on).
//!
//! A relay cannot look inside what it relays, so abuse is managed by volume
//! (architecture doc §6): every connection gets a byte bucket and a frame
//! bucket. A connection that runs dry is not cut off — its reader simply waits
//! for the bucket to refill, which stops the hub reading its socket, which is
//! TCP backpressure all the way back to the sender. A terminal never notices;
//! a bulk upload is held to the configured rate.

use std::collections::HashMap;
use std::hash::Hash;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// One bucket: `rate` tokens a second, holding at most `burst`.
#[derive(Debug, Clone)]
pub struct Bucket {
    rate: f64,
    burst: f64,
    tokens: f64,
    last: Instant,
}

impl Bucket {
    pub fn new(rate_per_sec: u64, burst: u64, now: Instant) -> Self {
        Self {
            rate: rate_per_sec.max(1) as f64,
            burst: burst.max(1) as f64,
            tokens: burst.max(1) as f64,
            last: now,
        }
    }

    /// `n` a `period`, holding at most `n`: a sliding "n per period".
    pub fn window(n: u32, period: Duration, now: Instant) -> Self {
        let n = f64::from(n.max(1));
        Self {
            rate: n / period.as_secs_f64().max(1e-3),
            burst: n,
            tokens: n,
            last: now,
        }
    }

    /// Whether `cost` tokens are there now (nothing taken); else the wait.
    pub fn check(&mut self, cost: f64, now: Instant) -> Option<Duration> {
        self.refill(now);
        if self.tokens >= cost.min(self.burst) {
            None
        } else {
            Some(Duration::from_secs_f64((cost.min(self.burst) - self.tokens) / self.rate))
        }
    }

    /// Take `cost` tokens unconditionally (after [`Bucket::check`]).
    pub fn spend(&mut self, cost: f64) {
        self.tokens -= cost;
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.rate).min(self.burst);
        self.last = now;
    }

    /// Take `cost` tokens (going into debt if need be) and say how long the
    /// caller must wait before acting, which is zero while there is credit. A
    /// cost larger than the burst is allowed; it just waits longer.
    /// Undo a [`Bucket::take`] whose work was refused rather than waited for.
    pub fn take_back(&mut self, cost: u64) {
        self.tokens = (self.tokens + cost as f64).min(self.burst);
    }

    /// Whether the bucket has refilled to its burst (an idle client).
    pub fn full(&mut self, now: Instant) -> bool {
        self.refill(now);
        self.tokens >= self.burst
    }

    /// Take `cost` tokens only if there is credit for it now; else the wait
    /// until there will be, taking nothing. A batch larger than the burst
    /// goes through once the bucket is full, into debt (else it would be
    /// refused for ever — the M2 review's livelock at low settings).
    pub fn try_take(&mut self, cost: u64, now: Instant) -> Option<Duration> {
        self.refill(now);
        let need = (cost as f64).min(self.burst);
        if self.tokens >= need {
            self.tokens -= cost as f64;
            None
        } else {
            Some(Duration::from_secs_f64((need - self.tokens) / self.rate))
        }
    }

    pub fn take(&mut self, cost: u64, now: Instant) -> Duration {
        self.refill(now);
        self.tokens -= cost as f64;
        if self.tokens >= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(-self.tokens / self.rate)
        }
    }
}

/// A connection's two buckets.
#[derive(Debug, Clone)]
pub struct ConnLimiter {
    bytes: Bucket,
    frames: Bucket,
}

impl ConnLimiter {
    /// `bytes_per_sec` and `frames_per_sec`, each with a burst of twice that.
    pub fn new(bytes_per_sec: u64, frames_per_sec: u64, now: Instant) -> Self {
        Self {
            bytes: Bucket::new(bytes_per_sec, bytes_per_sec.saturating_mul(2), now),
            frames: Bucket::new(frames_per_sec, frames_per_sec.saturating_mul(2), now),
        }
    }

    /// Charge one inbound message of `len` bytes; the wait before handling it.
    pub fn charge(&mut self, len: usize, frames: u64, now: Instant) -> Duration {
        self.bytes.take(len as u64, now).max(self.frames.take(frames, now))
    }
}

/// The most keys (addresses, accounts, devices) one table tracks.
pub const MAX_KEYS: usize = 50_000;

/// "n per period" for every window, per key.
pub struct Keyed<K> {
    windows: Vec<(u32, Duration)>,
    max: usize,
    table: Mutex<HashMap<K, (Vec<Bucket>, Instant)>>,
}

impl<K: Hash + Eq + Clone> Keyed<K> {
    pub fn new(windows: &[(u32, Duration)]) -> Self {
        Self::bounded(windows, MAX_KEYS)
    }

    pub fn bounded(windows: &[(u32, Duration)], max: usize) -> Self {
        Self {
            windows: windows.to_vec(),
            max: max.max(1),
            table: Mutex::new(HashMap::new()),
        }
    }

    /// Charge one request by `key`: `Ok` and every window spent one, or
    /// `Err(wait)` and nothing spent.
    pub fn allow(&self, key: &K, now: Instant) -> Result<(), Duration> {
        self.allow_n(key, 1.0, now)
    }

    /// [`Keyed::allow`] for a request that costs `cost` (a halved limit costs 2).
    pub fn allow_n(&self, key: &K, cost: f64, now: Instant) -> Result<(), Duration> {
        let mut table = self.table.lock().expect("rate table");
        if table.len() >= self.max && !table.contains_key(key) {
            evict(&mut table, self.max, now);
        }
        let (buckets, last) = table
            .entry(key.clone())
            .or_insert_with(|| (self.windows.iter().map(|(n, period)| Bucket::window(*n, *period, now)).collect(), now));
        *last = now;
        let wait = buckets.iter_mut().filter_map(|b| b.check(cost, now)).max();
        match wait {
            Some(wait) => Err(wait),
            None => {
                for b in buckets.iter_mut() {
                    b.spend(cost);
                }
                Ok(())
            }
        }
    }

    /// Whether [`Keyed::allow`] would let `key` through now, without
    /// charging it: to check several limits before spending any.
    pub fn peek(&self, key: &K, now: Instant) -> Result<(), Duration> {
        let mut table = self.table.lock().expect("rate table");
        let Some((buckets, _)) = table.get_mut(key) else {
            return Ok(());
        };
        match buckets.iter_mut().filter_map(|b| b.check(1.0, now)).max() {
            Some(wait) => Err(wait),
            None => Ok(()),
        }
    }

    /// How many keys the table holds (for tests and the memory bound).
    pub fn len(&self) -> usize {
        self.table.lock().expect("rate table").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Make room: forget keys whose buckets refilled (idle), then the least
/// recently used tenth.
fn evict<K: Hash + Eq + Clone>(table: &mut HashMap<K, (Vec<Bucket>, Instant)>, max: usize, now: Instant) {
    table.retain(|_, (buckets, _)| !buckets.iter_mut().all(|b| b.full(now)));
    if table.len() < max {
        return;
    }
    let mut by_age: Vec<(Instant, K)> = table.iter().map(|(k, (_, last))| (*last, k.clone())).collect();
    by_age.sort_by_key(|(last, _)| *last);
    for (_, k) in by_age.into_iter().take((max / 10).max(1)) {
        table.remove(&k);
    }
}

/// The client address as every per-address limit sees it: an IPv4 address
/// as it is (an IPv4-mapped IPv6 one too), an IPv6 address by its /64 — one
/// host is routinely handed a whole /64, so per /128 it had 2^64 addresses
/// (M3a review) — and unknown addresses share one key.
pub fn address_key(ip: Option<IpAddr>) -> IpAddr {
    match ip {
        Some(IpAddr::V6(v6)) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let mut octets = v6.octets();
                octets[8..].fill(0);
                IpAddr::V6(std::net::Ipv6Addr::from(octets))
            }
        },
        Some(v4) => v4,
        None => IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A batch larger than the burst used to be refused for ever.
    #[test]
    fn a_batch_larger_than_the_burst_goes_through_once_the_bucket_is_full() {
        let t0 = Instant::now();
        let mut b = Bucket::new(10, 200, t0);
        assert_eq!(b.try_take(201, t0), None, "a full bucket takes it, into debt");
        let wait = b.try_take(1, t0).expect("in debt");
        assert!(wait > Duration::from_millis(100));
        // Refilled to full again: the big batch passes again.
        let later = t0 + Duration::from_secs(21);
        assert_eq!(b.try_take(201, later), None);
        // Not full: it waits (and takes nothing meanwhile).
        let mut c = Bucket::new(10, 200, t0);
        assert_eq!(c.try_take(150, t0), None);
        let w = c.try_take(201, t0).unwrap();
        assert!((w.as_secs_f64() - 15.0).abs() < 0.01, "{w:?}");
        assert_eq!(c.try_take(201, t0 + w), None);
    }

    #[test]
    fn a_bucket_allows_its_burst_then_paces_at_its_rate() {
        let t0 = Instant::now();
        let mut b = Bucket::new(100, 200, t0);
        assert_eq!(b.take(200, t0), Duration::ZERO, "the burst");
        let wait = b.take(50, t0);
        assert!((wait.as_secs_f64() - 0.5).abs() < 1e-6, "{wait:?}");
        // After waiting it out, the debt is paid.
        assert_eq!(b.take(0, t0 + Duration::from_millis(500)), Duration::ZERO);
        // Refill never exceeds the burst.
        assert_eq!(b.take(200, t0 + Duration::from_secs(100)), Duration::ZERO);
        assert!(b.take(1, t0 + Duration::from_secs(100)) > Duration::ZERO);
    }

    #[test]
    fn a_connection_waits_for_whichever_bucket_is_shorter() {
        let t0 = Instant::now();
        let mut c = ConnLimiter::new(1000, 10, t0);
        for _ in 0..20 {
            assert_eq!(c.charge(1, 1, t0), Duration::ZERO);
        }
        assert!(c.charge(1, 1, t0) > Duration::ZERO, "out of frames long before bytes");
    }
}
