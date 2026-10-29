//! Failed-login throttling, per account name.
//!
//! Five wrong passwords in a row and the account stops answering for five
//! minutes; one right one clears the count. That is the whole mechanism, and it
//! is deliberately about the *account* rather than the client address: this
//! server is normally reached through a tunnel, a reverse proxy or Tailscale, so
//! every request arrives from the same handful of addresses and a per-IP counter
//! would be a per-deployment counter.
//!
//! The lockout is invisible from outside. A locked account answers exactly what a
//! wrong password answers, after exactly as much work (see
//! [`super::users::UsersStore::burn_a_verification`]) — otherwise the lockout
//! itself would say "this name exists and somebody is attacking it", which is
//! two facts the attacker did not have.
//!
//! In memory, never on disk. Restarting the server clears it, which is a real
//! limitation and an accepted one: the alternative is a file written on every
//! failed login, i.e. an unauthenticated write amplifier. What stands between
//! the API and an offline-speed guessing attack is argon2id at 64 MiB serialized
//! to one hash at a time; the lockout is the cheap layer on top.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How the lockout is configured, resolved from [`droppen_http::config::AuthSettings`].
#[derive(Debug, Clone, Copy)]
pub struct LockoutPolicy {
    /// Consecutive failures that trip the lock.
    pub attempts: u32,
    /// How long the lock holds.
    pub window: Duration,
}

#[derive(Debug, Default)]
struct Entry {
    failures: u32,
    locked_until: Option<Instant>,
}

/// The per-account failure counters.
///
/// Entries are only ever created for names that are *on file*: locking out an
/// account that does not exist protects nothing, and a map keyed on whatever an
/// attacker types is a map an attacker can grow. That bounds this table by the
/// number of accounts.
#[derive(Debug, Default)]
pub struct Lockouts {
    entries: Mutex<HashMap<String, Entry>>,
}

impl Lockouts {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().expect("auth lockouts poisoned")
    }

    /// Whether this account is currently refusing to be logged in to.
    pub fn locked(&self, name: &str) -> bool {
        self.locked_at(name, Instant::now())
    }

    fn locked_at(&self, name: &str, now: Instant) -> bool {
        let mut entries = self.lock();
        let Some(entry) = entries.get_mut(name) else { return false };
        match entry.locked_until {
            // The lock has run out: the count goes with it, so the account gets a
            // fresh allowance rather than being one mistake from another lockout.
            Some(until) if until <= now => {
                entry.locked_until = None;
                entry.failures = 0;
                false
            }
            Some(_) => true,
            None => false,
        }
    }

    /// Record a failed attempt. `true` if this one tripped the lock.
    ///
    /// Counting the failure *after* the password was checked is only sound
    /// because the caller holds [`super::Auth::login_gate`] across both: an
    /// argon2id verification takes on the order of a hundred milliseconds, and
    /// "ask whether the account is locked, spend 100 ms, then record the
    /// failure" would otherwise be a check-then-act with a 100 ms window in it,
    /// through which a burst of concurrent guesses all pass a check that none of
    /// them has yet had the chance to fail.
    pub fn record_failure(&self, name: &str, policy: LockoutPolicy) -> bool {
        self.record_failure_at(name, policy, Instant::now())
    }

    fn record_failure_at(&self, name: &str, policy: LockoutPolicy, now: Instant) -> bool {
        let mut entries = self.lock();
        let entry = entries.entry(name.to_string()).or_default();
        entry.failures = entry.failures.saturating_add(1);
        if policy.attempts > 0 && entry.failures >= policy.attempts {
            entry.locked_until = Some(now + policy.window);
            return true;
        }
        false
    }

    /// Record a success: the account starts from zero again.
    pub fn record_success(&self, name: &str) {
        self.lock().remove(name);
    }

    /// How many accounts are being counted; the sweep is observable through it.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether nothing is being counted.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: LockoutPolicy = LockoutPolicy {
        attempts: 5,
        window: Duration::from_secs(300),
    };

    #[test]
    fn five_in_a_row_locks_and_the_fifth_is_the_one_that_does_it() {
        let lockouts = Lockouts::new();
        let start = Instant::now();
        for attempt in 1..5 {
            assert!(!lockouts.record_failure_at("ada", POLICY, start), "attempt {attempt} is still allowed");
            assert!(!lockouts.locked_at("ada", start));
        }
        assert!(lockouts.record_failure_at("ada", POLICY, start), "the fifth trips it");
        assert!(lockouts.locked_at("ada", start));
    }

    #[test]
    fn the_lock_lets_go_by_itself() {
        let lockouts = Lockouts::new();
        let start = Instant::now();
        for _ in 0..5 {
            lockouts.record_failure_at("ada", POLICY, start);
        }
        assert!(lockouts.locked_at("ada", start + Duration::from_secs(299)));
        assert!(!lockouts.locked_at("ada", start + Duration::from_secs(301)));
        // And with a clean slate: four more mistakes are allowed before the next
        // lock, rather than the next one re-tripping it immediately.
        for _ in 0..4 {
            assert!(!lockouts.record_failure_at("ada", POLICY, start + Duration::from_secs(301)));
        }
    }

    #[test]
    fn a_success_clears_the_count() {
        let lockouts = Lockouts::new();
        let start = Instant::now();
        for _ in 0..4 {
            lockouts.record_failure_at("ada", POLICY, start);
        }
        lockouts.record_success("ada");
        assert!(lockouts.is_empty());
        assert!(!lockouts.record_failure_at("ada", POLICY, start), "counting starts again from one");
    }

    #[test]
    fn one_account_being_attacked_does_not_lock_another_out() {
        let lockouts = Lockouts::new();
        let start = Instant::now();
        for _ in 0..6 {
            lockouts.record_failure_at("ada", POLICY, start);
        }
        assert!(lockouts.locked_at("ada", start));
        assert!(!lockouts.locked_at("grace", start));
    }

    /// `lockout_attempts = 0` is "never lock", not "lock immediately" — a
    /// configuration that locked every account out of the machine on the first
    /// typo would be a very expensive way to read a config file wrong.
    #[test]
    fn zero_attempts_disables_the_lockout_rather_than_making_it_instant() {
        let lockouts = Lockouts::new();
        let policy = LockoutPolicy { attempts: 0, ..POLICY };
        let start = Instant::now();
        for _ in 0..50 {
            assert!(!lockouts.record_failure_at("ada", policy, start));
        }
        assert!(!lockouts.locked_at("ada", start));
    }
}
