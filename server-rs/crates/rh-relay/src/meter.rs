//! Per-account transfer (§3.6): the `DATA` payload bytes this relay
//! forwarded for each account, both directions, once per forwarded frame.
//! `PING`/`PONG`, sign-ins and control messages are not counted.
//!
//! Every report interval the counts become a report `{boot, seq, from_ms,
//! to_ms, accounts}`: `boot` is random per process, `seq` counts up within
//! it, so the Hub records each report once however often it is sent. Reports
//! wait in memory (bounded: two hours' worth) until the Hub acknowledges
//! them, and go again, unchanged, after the control link comes back. A crash
//! loses at most one interval's counts — the account's favour.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use rh_proto::wire::relay::{UsageLine, UsageReport};

/// The most accounts in one report; more go in further reports.
pub const MAX_ACCOUNTS_PER_REPORT: usize = 500;

struct Inner {
    counts: HashMap<String, u64>,
    since_ms: u64,
    next_seq: u64,
    pending: VecDeque<UsageReport>,
    /// Totals since start, for the health report's rates.
    total: u64,
}

pub struct Meter {
    pub boot: String,
    max_pending: usize,
    inner: Mutex<Inner>,
}

impl Meter {
    /// A meter whose reports wait for at most `max_pending` of them.
    pub fn new(now_ms: u64, max_pending: usize) -> Self {
        let mut boot = [0u8; 8];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut boot);
        Self {
            boot: crate::config::hex(&boot),
            max_pending: max_pending.max(1),
            inner: Mutex::new(Inner {
                counts: HashMap::new(),
                since_ms: now_ms,
                next_seq: 1,
                pending: VecDeque::new(),
                total: 0,
            }),
        }
    }

    /// `bytes` of `DATA` payload forwarded for `account`.
    pub fn count(&self, account: &str, bytes: usize) {
        let mut inner = self.inner.lock().expect("meter");
        inner.total += bytes as u64;
        match inner.counts.get_mut(account) {
            Some(n) => *n += bytes as u64,
            None => {
                inner.counts.insert(account.to_string(), bytes as u64);
            }
        }
    }

    pub fn total(&self) -> u64 {
        self.inner.lock().expect("meter").total
    }

    /// Close the current interval into reports (none if nothing was relayed).
    pub fn cut(&self, now_ms: u64) {
        let mut inner = self.inner.lock().expect("meter");
        let from_ms = inner.since_ms;
        inner.since_ms = now_ms;
        let mut lines: Vec<UsageLine> = inner
            .counts
            .drain()
            .filter(|(_, b)| *b > 0)
            .map(|(account, bytes)| UsageLine { account, bytes })
            .collect();
        lines.sort_by(|a, b| a.account.cmp(&b.account));
        for chunk in lines.chunks(MAX_ACCOUNTS_PER_REPORT) {
            let seq = inner.next_seq;
            inner.next_seq += 1;
            inner.pending.push_back(UsageReport {
                boot: self.boot.clone(),
                seq,
                from_ms,
                to_ms: now_ms,
                accounts: chunk.to_vec(),
            });
        }
        while inner.pending.len() > self.max_pending {
            let dropped = inner.pending.pop_front();
            tracing::warn!(seq = dropped.map(|r| r.seq), "a usage report waited too long for the Hub; dropped");
        }
    }

    /// The reports not acknowledged yet, oldest first.
    pub fn pending(&self) -> Vec<UsageReport> {
        self.inner.lock().expect("meter").pending.iter().cloned().collect()
    }

    /// The Hub has report `seq` (of this boot).
    pub fn ack(&self, boot: &str, seq: u64) {
        if boot != self.boot {
            return;
        }
        self.inner.lock().expect("meter").pending.retain(|r| r.seq != seq);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_wait_for_their_ack_and_keep_their_window() {
        let m = Meter::new(1_000, 3);
        m.count("a-1", 10);
        m.count("a-2", 5);
        m.count("a-1", 1);
        m.cut(2_000);
        let p = m.pending();
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].seq, p[0].from_ms, p[0].to_ms), (1, 1_000, 2_000));
        assert_eq!(
            p[0].accounts,
            vec![
                UsageLine {
                    account: "a-1".into(),
                    bytes: 11
                },
                UsageLine {
                    account: "a-2".into(),
                    bytes: 5
                }
            ]
        );
        m.cut(3_000);
        assert_eq!(m.pending().len(), 1, "an empty interval sends nothing");
        m.count("a-1", 1);
        m.cut(4_000);
        // Resent unchanged until acknowledged.
        assert_eq!(m.pending()[0], p[0]);
        m.ack("other-boot", 1);
        assert_eq!(m.pending().len(), 2);
        m.ack(&m.boot.clone(), 1);
        assert_eq!(m.pending().iter().map(|r| r.seq).collect::<Vec<_>>(), vec![2]);
        assert_eq!(m.pending()[0].from_ms, 3_000);
        // Bounded: the oldest goes.
        for t in 0..5 {
            m.count("a-1", 1);
            m.cut(5_000 + t);
        }
        assert_eq!(m.pending().len(), 3);
        assert_eq!(m.total(), 22);
    }
}
