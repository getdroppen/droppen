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

    /// Refill up to `now`. Monotonic: a `now` older than the last refill
    /// (taken by a caller before another one refilled — a sweep on its way
    /// through its tables, a request that waited for a lock) adds nothing
    /// and leaves the clock where it is, so no stretch of time is ever
    /// refilled twice.
    fn refill(&mut self, now: Instant) {
        let now = self.last.max(now);
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

/// The longest a table holds a key after its last request: a day, as
/// droppen.app/privacy says ("count requests per address and per network
/// in memory, for a day at most"). No window may be longer ([`Keyed::new`]).
pub const FORGET_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// How often a server forgets what it no longer needs ([`Keyed::forget_old`]).
/// A key is forgotten at the last sweep before [`FORGET_AFTER`] is up, so
/// none outlives it.
pub const SWEEP_EVERY: Duration = Duration::from_secs(60);

/// "n per period" for every window, per key. Memory only, and no key stays
/// longer than it counts: [`Keyed::forget_old`], called every
/// [`SWEEP_EVERY`], forgets each one whose windows have all refilled since
/// its last request, and every one [`FORGET_AFTER`] after it at the latest.
pub struct Keyed<K> {
    windows: Vec<(u32, Duration)>,
    max: usize,
    table: Mutex<HashMap<K, (Vec<Bucket>, Instant)>>,
}

impl<K: Hash + Eq + Clone> Keyed<K> {
    pub fn new(windows: &[(u32, Duration)]) -> Self {
        Self::bounded(windows, MAX_KEYS)
    }

    /// At most `max` keys. A window longer than [`FORGET_AFTER`] is refused:
    /// a key could not be forgotten when the privacy notice says without
    /// loosening it.
    pub fn bounded(windows: &[(u32, Duration)], max: usize) -> Self {
        assert!(
            windows.iter().all(|(_, period)| *period <= FORGET_AFTER),
            "a limit's window is longer than a key may be held ({FORGET_AFTER:?}): {windows:?}"
        );
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

    /// Forget every key there is nothing more to count against — its every
    /// window refilled since its last request, so a new key would answer
    /// the same — and every key whose last request is `older_than` old
    /// by the next sweep ([`SWEEP_EVERY`]), whatever its windows (one in
    /// debt, after a request that cost more than its window holds): how
    /// many went. Called every [`SWEEP_EVERY`] with `older_than` at most
    /// [`FORGET_AFTER`], no key outlives that age; a key still counting
    /// (not refilled, and asked within it) is never forgotten, so no limit
    /// is reset early. [`FORGET_AFTER`] is the age when the privacy notice's
    /// day is the bound, which is every caller in this binary.
    pub fn forget_old(&self, now: Instant, older_than: Duration) -> usize {
        let mut table = self.table.lock().expect("rate table");
        let before = table.len();
        table.retain(|_, (buckets, last)| {
            // Asked after the sweep's clock: counting, left as it is.
            if *last > now {
                return true;
            }
            let old = now.saturating_duration_since(*last) + SWEEP_EVERY >= older_than;
            !old && !buckets.iter_mut().all(|b| b.full(now))
        });
        before - table.len()
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

    /// An older `now` never moves a bucket's clock back: the time between it
    /// and the last refill is refilled once, not again (#121 review).
    #[test]
    fn an_older_now_never_refills_twice() {
        let t = Instant::now();
        let mut b = Bucket::window(1, Duration::from_secs(20), t);
        assert_eq!(b.try_take(1, t + Duration::from_secs(10)), None, "spent at 10 s");
        // A caller with a clock from before (5 s): nothing added, and the
        // bucket's clock stays at 10 s…
        assert!(!b.full(t + Duration::from_secs(5)));
        assert!(b.check(1.0, t + Duration::from_secs(5)).is_some());
        // …so 15 s after the spend it has 0.75 of a token, not 1.0 (as it
        // would, refilled from 5 s).
        let wait = b.check(1.0, t + Duration::from_secs(25)).expect("not yet");
        assert!((wait.as_secs_f64() - 5.0).abs() < 1e-6, "{wait:?}");
        assert!(b.check(1.0, t + Duration::from_secs(31)).is_none(), "20 s after the spend");
        // The same through a table: a sweep with an older clock, then a request.
        let k: Keyed<u32> = Keyed::new(&[(1, Duration::from_secs(20))]);
        k.allow(&1, t + Duration::from_secs(10)).unwrap();
        assert_eq!(k.forget_old(t, FORGET_AFTER), 0, "still counting");
        assert!(k.allow(&1, t + Duration::from_secs(25)).is_err(), "refilled once, from 10 s");
        assert!(k.allow(&1, t + Duration::from_secs(31)).is_ok());
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
    fn a_key_goes_once_its_windows_refill_and_a_day_after_its_last_request_at_most() {
        let t = Instant::now();
        let minute = Duration::from_secs(60);
        let hour = 60 * minute;
        // A minute's window: gone once it refilled, not before.
        let k: Keyed<u32> = Keyed::new(&[(1, minute)]);
        k.allow(&1, t).unwrap();
        assert_eq!(k.forget_old(t + Duration::from_secs(30), FORGET_AFTER), 0, "still counting");
        assert!(k.allow(&1, t + Duration::from_secs(30)).is_err(), "and still limited");
        assert_eq!(k.forget_old(t + Duration::from_secs(90), FORGET_AFTER), 1, "refilled: nothing more to count");
        assert!(k.is_empty());
        // Two windows (the relay's upgrades), a burst spent: gone once both refilled.
        let k: Keyed<u32> = Keyed::new(&[(60, minute), (20, Duration::from_secs(20))]);
        for _ in 0..20 {
            k.allow(&1, t).unwrap();
        }
        assert!(k.allow(&1, t).is_err(), "the burst is spent");
        assert_eq!(k.forget_old(t + Duration::from_secs(10), FORGET_AFTER), 0, "still counting");
        assert_eq!(k.forget_old(t + Duration::from_secs(21), FORGET_AFTER), 1, "both refilled");
        // A day's window, partly spent: gone once refilled, not before.
        let k: Keyed<u32> = Keyed::new(&[(5, hour), (10, FORGET_AFTER)]);
        for _ in 0..6 {
            k.allow(&1, t).unwrap_or(());
        }
        assert_eq!(k.forget_old(t + 11 * hour, FORGET_AFTER), 0, "five of ten spent: not refilled 11 hours on");
        assert_eq!(k.forget_old(t + 12 * hour + Duration::from_secs(1), FORGET_AFTER), 1, "refilled");
        // In debt, a key would take two days to refill: it goes by a day all
        // the same, at the last sweep before the day is up.
        let k: Keyed<u32> = Keyed::new(&[(1, FORGET_AFTER)]);
        k.allow_n(&1, 2.0, t).unwrap();
        assert_eq!(k.forget_old(t + FORGET_AFTER - SWEEP_EVERY - Duration::from_secs(1), FORGET_AFTER), 0);
        assert_eq!(k.forget_old(t + FORGET_AFTER - SWEEP_EVERY, FORGET_AFTER), 1, "a day after its last request, at most");
        // A request since keeps a key; the rest of the table goes.
        let k: Keyed<u32> = Keyed::new(&[(1, FORGET_AFTER)]);
        for i in 0..100 {
            k.allow(&i, t).unwrap();
        }
        let _ = k.allow(&7, t + 12 * hour);
        assert_eq!(k.forget_old(t + FORGET_AFTER - SWEEP_EVERY, FORGET_AFTER), 99, "but the one asked since");
        assert_eq!(k.len(), 1);
    }

    /// The age a key goes at is the caller's (the hub sweeps its tables with
    /// its own day): a key still counting is held until that age, then gone
    /// at the last sweep before it.
    #[test]
    fn a_key_goes_at_the_age_the_caller_gives() {
        let t = Instant::now();
        let hour = Duration::from_secs(60 * 60);
        let k: Keyed<u32> = Keyed::new(&[(1, FORGET_AFTER)]);
        k.allow(&1, t).unwrap();
        assert_eq!(k.forget_old(t + hour, FORGET_AFTER), 0, "still counting, and within the day");
        assert_eq!(k.forget_old(t + hour + SWEEP_EVERY, hour), 1, "at the age given, whatever the window");
        // The idle rule is unchanged: a refilled window goes before the age.
        let k: Keyed<u32> = Keyed::new(&[(1, Duration::from_secs(60))]);
        k.allow(&1, t).unwrap();
        assert_eq!(k.forget_old(t + Duration::from_secs(61), FORGET_AFTER), 1, "refilled long before the day");
    }

    #[test]
    #[should_panic(expected = "longer than a key may be held")]
    fn no_window_is_longer_than_a_key_is_held() {
        let _: Keyed<u32> = Keyed::new(&[(7, FORGET_AFTER + Duration::from_secs(1))]);
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
