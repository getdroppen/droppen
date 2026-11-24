//! What a sender keeps in flight on one path: about what that path carries in
//! a round trip, not the whole window its receiver allows.
//!
//! The hub window ([`crate::flow::INITIAL_CREDIT`], 512 KiB a direction) and a
//! direct path's limit ([`crate::path::UNACKED_LIMIT`]) are what a receiver
//! lets a sender have outstanding. On a 1 Mbps link 512 KiB is four seconds of
//! queue in the sockets, the relay and the DataChannel between the two ends,
//! and whatever is sent after it — an rpc response, a probe's answer — waits
//! that long behind it. [`Pace`] is the sender's own bound below those, purely
//! local: nothing on the wire changes, and a receiver of any version that
//! speaks paths works with it.
//!
//! ```text
//! cap = clamp(GAIN × rate × min_rtt, FLOOR, ceiling)
//! ```
//!
//! where `rate` is the delivery rate the path has shown and `min_rtt` the
//! shortest round trip it has taken lately, both measured here; `ceiling` is
//! the limit the receiver already sets (the hub window, or the direct path's).
//!
//! # Markers
//!
//! The sender follows its data with *markers*: [`crate::path::Envelope::Probe`]s
//! numbered from the session's probe nonces, which every receiver that speaks
//! [`crate::path::CLIENT_FEATURE`] answers at once on the path they came by,
//! old ones included. A path delivers in order, so a marker's answer says the
//! receiver has read everything sent before the marker on that path — as an
//! acknowledgement says it for TCP. From the answers:
//!
//! - **in flight**: data sent on the path less what an answered marker covers;
//! - **delivery rate**, sample by sample as BBR takes it: what was delivered
//!   between the delivery last heard of when a marker left and that marker's
//!   answer, over the longer of the time it took to send and to deliver. While
//!   the sender ran out of data (app-limited) a sample counts only when it is
//!   higher than the estimate. The rate is the highest sample of the last
//!   [`RATE_WINDOW_US`], so it grows back as fast as the path allows and falls
//!   within a second when the path slows;
//! - **minimum round trip**: a marker's own (sent to answered). A marker with
//!   data queued ahead of it times the queue too, so a sample only ever lowers
//!   the minimum; it is refreshed — taken anew even when higher — only from a
//!   *clean* marker, one with no more than [`SMALL`] ahead of it, at most every
//!   [`MIN_RTT_REFRESH_US`]. Taken from a queue, the minimum would feed itself:
//!   a bigger cap, a longer queue, a longer round trip, a bigger cap. A path
//!   kept busy for [`MIN_RTT_WINDOW_US`] without a clean marker is *drained*:
//!   no new data until what is out has been delivered, then a clean marker and
//!   data again behind it (BBR's ProbeRTT: about a round trip of idle link
//!   every ten seconds).
//!
//! Markers go **before** the first data of a flight when the minimum is due a
//! refresh (or unknown), **every** quarter of the cap and never closer than
//! [`MIN_SPACING`], and **after** the last data when sending stops: held by the
//! cap, or out of data with more than [`SMALL`] unmarked. A few keystrokes'
//! worth is not followed by a probe each time; it counts as delivered once the
//! path has been quiet for a round trip.
//!
//! # Under the cap
//!
//! New data goes by [`Budget`]: term, rpc and mux frames may go [`HEADROOM`]
//! past the cap, blob frames only up to it, so a small answer never waits for
//! a bulk frame to be delivered first — only behind what is already on the
//! wire, which the cap keeps short. A frame bigger than what is left is cut to
//! fit ([`crate::mux::Mux::poll_within`]). Control frames (the multiplexer's
//! own, probes and their answers, acknowledgements, the hub's credits) never
//! wait for the cap.
//!
//! # The numbers
//!
//! - [`FLOOR`] 64 KiB: a whole frame ([`crate::frame::MAX_PAYLOAD_LEN`]) always
//!   fits; at 1 Mbps it is half a second of queue, the most the cap adds where
//!   the product is smaller (1 Mbps × 70 ms is 9 KB).
//! - [`GAIN`] 2: twice the product keeps the link busy while room comes back
//!   in steps (a marker's spacing, a quarter of the cap) a round trip late, and
//!   adds about one more round trip of queue. BBR's window gain is 2 for the
//!   same reasons.
//! - Until a path has a rate and a round trip, its cap is the floor. A new
//!   path then grows with [`STARTUP_GAIN`] (BBR's 2/ln 2) until three rounds
//!   in a row bring less than a quarter more rate, and with [`GAIN`] after:
//!   from the floor it reaches 50 Mbps × 20 ms in about two round trips.
//!
//! Times are microseconds of the session's clock; the browser's half
//! (`web/src/lib/e2e/pace.ts`) keeps milliseconds, with the same rules, held
//! to the same numbers by a shared vector (`pace_scenario`, in `vectors/rust.json`).

use std::collections::VecDeque;

use crate::mux::Budget;

/// The least a cap ever is: one whole frame in flight.
pub const FLOOR: u64 = 64 * 1024;
/// The cap is this many times the measured bandwidth-delay product…
pub const GAIN: f64 = 2.0;
/// …and this many while a new path's rate is still growing (BBR's startup gain, 2/ln 2).
pub const STARTUP_GAIN: f64 = 2.885;
/// A round's rate counts as growth when it is this much more than the last that did…
pub const STARTUP_GROWTH: f64 = 1.25;
/// …and startup ends after this many rounds without.
pub const STARTUP_ROUNDS: u32 = 3;
/// How far past the cap term, rpc and mux data may go (blob may not).
pub const HEADROOM: u64 = 16 * 1024;
/// A tail this small needs no marker; a marker with no more than this ahead of it is clean.
pub const SMALL: u64 = 4 * 1024;
/// Markers go every quarter of the cap, never closer than this.
pub const MIN_SPACING: u64 = 16 * 1024;
/// The rate is the highest sample of this long.
pub const RATE_WINDOW_US: u64 = 1_000_000;
/// A clean marker refreshes the minimum round trip once it is this old.
pub const MIN_RTT_REFRESH_US: u64 = 5_000_000;
/// A minimum round trip this old, on a path that has had no clean marker since, drains it for one.
pub const MIN_RTT_WINDOW_US: u64 = 10_000_000;
/// Before a round trip is known, how long a path must be quiet for a small unmarked tail to count as delivered.
pub const QUIET_US: u64 = 250_000;
/// The most markers out at once; past it the oldest is forgotten (a later one covers what it did).
pub const MAX_MARKERS: usize = 64;
/// What sealing adds to a multiplexer frame on its way to the wire (envelope 17, data header 5, tag 16, a DataChannel message's kind 1), rounded up.
pub const OVERHEAD: u64 = 64;

/// What the cap lets go next, given that there is data waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// A marker first.
    Marker,
    /// New data, within this budget.
    Data(Budget),
}

/// A path's pace, for logs.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PaceStats {
    /// What may be in flight now (bytes).
    pub cap: u64,
    /// Data sent and not yet known to be delivered (bytes).
    pub inflight: u64,
    /// The delivery rate measured (bytes a second).
    pub rate: Option<f64>,
    /// The minimum round trip measured (ms).
    pub min_rtt_ms: Option<f64>,
    /// Markers out.
    pub markers: usize,
    /// Held for a clean round trip ([`MIN_RTT_WINDOW_US`]).
    pub draining: bool,
    /// Still finding the path's rate ([`STARTUP_GAIN`]).
    pub startup: bool,
}

#[derive(Debug, Clone, Copy)]
struct Marker {
    nonce: u64,
    /// `sent` when it left: everything before it.
    offset: u64,
    sent_us: u64,
    /// The delivery state when it left (BBR's `delivered`, `delivered_time`, `first_sent_time`).
    delivered: u64,
    delivered_at_us: u64,
    first_sent_us: u64,
    app_limited: bool,
    clean: bool,
}

/// The highest sample of a window: a deque falling in rate, rising in time.
#[derive(Debug, Clone, Default)]
struct MaxFilter {
    samples: VecDeque<(u64, f64)>,
}

impl MaxFilter {
    fn push(&mut self, at_us: u64, rate: f64) {
        while self.samples.back().is_some_and(|&(_, r)| r <= rate) {
            self.samples.pop_back();
        }
        self.samples.push_back((at_us, rate));
        // The newest always stays: across a quiet spell the last estimate holds.
        while self.samples.len() > 1 && self.samples.front().is_some_and(|&(t, _)| at_us.saturating_sub(t) > RATE_WINDOW_US) {
            self.samples.pop_front();
        }
    }

    fn max(&self) -> Option<(u64, f64)> {
        self.samples.front().copied()
    }
}

/// One path's in-flight cap and what it measures (module docs).
#[derive(Debug, Clone)]
pub struct Pace {
    ceiling: u64,
    /// Data bytes sent on the path, and how many of them an answered marker covers.
    sent: u64,
    delivered: u64,
    /// Data bytes sent since the last marker.
    unmarked: u64,
    last_data_us: Option<u64>,
    /// When `delivered` last moved, and when the data its sample interval starts with left (BBR).
    delivered_at_us: u64,
    first_sent_us: u64,
    /// While nonzero, samples are app-limited: the sender ran out of data with this much sent.
    app_limited_until: u64,
    markers: VecDeque<Marker>,
    rate: MaxFilter,
    /// The minimum round trip and when it was taken (µs).
    min_rtt: Option<(u64, u64)>,
    draining: bool,
    /// Startup: the rate still grows ([`STARTUP_GAIN`]); the best round's
    /// rate so far and how many rounds since have not beaten it by a quarter.
    startup: bool,
    full_rate: f64,
    flat_rounds: u32,
    /// A round ends when a marker sent with this much delivered is answered.
    next_round: u64,
}

impl Pace {
    /// A fresh path's pace under `ceiling` (the receiver's own limit).
    pub fn new(ceiling: usize) -> Self {
        Self {
            ceiling: (ceiling as u64).max(FLOOR),
            sent: 0,
            delivered: 0,
            unmarked: 0,
            last_data_us: None,
            delivered_at_us: 0,
            first_sent_us: 0,
            app_limited_until: 0,
            markers: VecDeque::new(),
            rate: MaxFilter::default(),
            min_rtt: None,
            draining: false,
            startup: true,
            full_rate: 0.0,
            flat_rounds: 0,
            next_round: 0,
        }
    }

    /// A new path's pace that starts from another path's rate (`(when, bytes a
    /// second)`): the bottleneck two paths share is usually the access link
    /// under both. Its own round trip is measured by its first marker; until
    /// then the cap is the floor.
    pub fn seeded(ceiling: usize, rate: Option<(u64, f64)>) -> Self {
        let mut pace = Self::new(ceiling);
        pace.seed_rate(rate);
        pace
    }

    /// Take another path's rate when this one has none of its own.
    pub fn seed_rate(&mut self, rate: Option<(u64, f64)>) {
        if let (None, Some((at, r))) = (self.rate.max(), rate) {
            self.rate.push(at, r);
        }
    }

    /// The rate estimate and when its sample was taken.
    pub fn rate_sample(&self) -> Option<(u64, f64)> {
        self.rate.max()
    }

    pub fn inflight(&self) -> u64 {
        self.sent - self.delivered
    }

    pub fn cap(&self) -> u64 {
        match (self.rate.max(), self.min_rtt) {
            (Some((_, rate)), Some((rtt_us, _))) => {
                // In milliseconds, as the browser's half has it, so the two round alike.
                let bdp = rate * (rtt_us as f64 / 1e3) / 1e3;
                let gain = if self.startup { STARTUP_GAIN } else { GAIN };
                ((gain * bdp) as u64).clamp(FLOOR, self.ceiling)
            }
            _ => FLOOR,
        }
    }

    pub fn stats(&self) -> PaceStats {
        PaceStats {
            cap: self.cap(),
            inflight: self.inflight(),
            rate: self.rate.max().map(|(_, r)| r),
            min_rtt_ms: self.min_rtt.map(|(rtt, _)| rtt as f64 / 1000.0),
            markers: self.markers.len(),
            draining: self.draining,
            startup: self.startup,
        }
    }

    /// With data waiting to go: a marker first, or the data's budget.
    pub fn next(&mut self, now_us: u64) -> Next {
        self.fold(now_us);
        let clean_out = self.markers.iter().any(|m| m.clean);
        if self.inflight() == 0 {
            // A new flight: time the empty path first when the minimum is due.
            self.draining = false;
            let due = self.min_rtt.is_none_or(|(_, at)| now_us.saturating_sub(at) >= MIN_RTT_REFRESH_US);
            if due && !clean_out {
                return Next::Marker;
            }
        } else if self.draining || (!clean_out && self.min_rtt.is_some_and(|(_, at)| now_us.saturating_sub(at) > MIN_RTT_WINDOW_US)) {
            // Nothing new until what is out has been delivered (the tail is
            // marked when the sender finds itself held, [`Pace::stopped`]).
            self.draining = true;
            return Next::Data(Budget::NONE);
        }
        if self.unmarked >= MIN_SPACING.max(self.cap() / 4) {
            return Next::Marker;
        }
        let used = self.inflight() + OVERHEAD;
        let cap = self.cap();
        Next::Data(Budget {
            interactive: (cap + HEADROOM).saturating_sub(used) as usize,
            bulk: cap.saturating_sub(used) as usize,
        })
    }

    /// `len` bytes of data (a sequenced envelope, sealed) left on the path.
    pub fn data_sent(&mut self, len: usize, now_us: u64) {
        if self.inflight() == 0 {
            // A flight starts its own sampling interval (BBR).
            self.delivered_at_us = now_us;
            self.first_sent_us = now_us;
        }
        self.sent += len as u64;
        self.unmarked += len as u64;
        self.last_data_us = Some(now_us);
    }

    /// A marker with `nonce` left on the path.
    pub fn marker_sent(&mut self, nonce: u64, now_us: u64) {
        self.markers.push_back(Marker {
            nonce,
            offset: self.sent,
            sent_us: now_us,
            delivered: self.delivered,
            delivered_at_us: self.delivered_at_us,
            first_sent_us: self.first_sent_us,
            app_limited: self.app_limited_until != 0,
            clean: self.inflight() <= SMALL,
        });
        self.unmarked = 0;
        if self.markers.len() > MAX_MARKERS {
            self.markers.pop_front();
        }
    }

    /// Sending stopped: `held` with data waiting (the cap, a drain, the
    /// path's own limit), or out of data. Whether a marker should follow what
    /// went.
    pub fn stopped(&mut self, held: bool) -> bool {
        if !held && self.inflight() < self.cap() {
            self.app_limited_until = self.sent.max(1);
        }
        self.unmarked > 0 && (held || self.unmarked > SMALL)
    }

    /// The answer to a probe with `nonce` came on this path: whether it was
    /// one of this pace's markers.
    pub fn answered(&mut self, nonce: u64, now_us: u64) -> bool {
        let Some(i) = self.markers.iter().position(|m| m.nonce == nonce) else {
            return false;
        };
        // Those before it were read before it: its answer covers them.
        let m = self.markers.drain(..=i).next_back().expect("found");
        self.note_rtt(now_us.saturating_sub(m.sent_us), m.clean, now_us);
        if m.offset > self.delivered {
            self.delivered = m.offset;
            self.delivered_at_us = now_us;
            self.first_sent_us = m.sent_us;
        }
        let data = m.offset - m.delivered;
        let interval = m.sent_us.saturating_sub(m.first_sent_us).max(now_us.saturating_sub(m.delivered_at_us));
        if data > 0 && interval > 0 {
            let rate = data as f64 * 1e6 / interval as f64;
            if !m.app_limited || self.rate.max().is_none_or(|(_, max)| rate >= max) {
                self.rate.push(now_us, rate);
            }
        }
        if m.delivered >= self.next_round {
            // A round trip's worth delivered since the last round ended (BBR's rounds).
            self.next_round = self.delivered;
            if self.startup && !m.app_limited {
                let rate = self.rate.max().map_or(0.0, |(_, r)| r);
                if rate >= self.full_rate * STARTUP_GROWTH {
                    self.full_rate = rate;
                    self.flat_rounds = 0;
                } else {
                    self.flat_rounds += 1;
                    self.startup = self.flat_rounds < STARTUP_ROUNDS;
                }
            }
        }
        if self.app_limited_until != 0 && self.delivered >= self.app_limited_until {
            self.app_limited_until = 0;
        }
        true
    }

    fn note_rtt(&mut self, rtt_us: u64, clean: bool, now_us: u64) {
        let take = match self.min_rtt {
            None => true,
            Some((min, at)) => rtt_us <= min || (clean && now_us.saturating_sub(at) >= MIN_RTT_REFRESH_US),
        };
        if take {
            self.min_rtt = Some((rtt_us, now_us));
        }
    }

    /// A small tail nobody marked, sent a quiet round trip ago with nothing
    /// else out, counts as delivered.
    fn fold(&mut self, now_us: u64) {
        let quiet = self.min_rtt.map_or(QUIET_US, |(rtt, _)| rtt);
        if self.markers.is_empty()
            && self.unmarked > 0
            && self.unmarked <= SMALL
            && self.inflight() == self.unmarked
            && self.last_data_us.is_some_and(|at| now_us.saturating_sub(at) >= quiet)
        {
            self.delivered = self.sent;
            self.unmarked = 0;
        }
    }
}

/// A fixed trace of what a sender asked its pace and what the pace said,
/// shared with the TypeScript half (`vectors/rust.json` `pace`) so both
/// estimators agree call for call. One line per call, whole milliseconds:
///
/// ```text
/// next <t> = M | D <interactive> <bulk>     mark <nonce> <t>     data <len> <t>
/// stop <held 0|1> = <0|1>                   ans <nonce> <t> = <0|1>
/// ```
///
/// each followed by ` | <cap> <inflight> <rate or -> <min rtt ms or -> <markers> <draining 0|1> <startup 0|1>`.
/// The sender behind it has endless 16 KiB chunks on a 1 Mbps path with a
/// 70 ms round trip and a deep queue for 11 s (a drain included), runs out of
/// data, waits, then moves to a 50 Mbps path with a 20 ms round trip, and
/// ends with a keystroke.
#[cfg(test)]
pub(crate) fn pace_scenario() -> Vec<String> {
    use std::collections::BTreeSet;

    struct Rec {
        pace: Pace,
        ops: Vec<String>,
    }
    impl Rec {
        fn say(&mut self, op: String) {
            let s = self.pace.stats();
            let rate = s.rate.map_or("-".to_string(), |r| (r.floor() as u64).to_string());
            let rtt = s.min_rtt_ms.map_or("-".to_string(), |r| r.to_string());
            self.ops.push(format!(
                "{op} | {} {} {rate} {rtt} {} {} {}",
                s.cap,
                s.inflight,
                s.markers,
                u8::from(s.draining),
                u8::from(s.startup)
            ));
        }
    }
    /// A deep FIFO that drains `rate` bytes a ms; a marker is answered a round trip after what is ahead of it.
    struct Link {
        rate: u64,
        rtt: u64,
        free_at: u64,
        answers: BTreeSet<(u64, u64)>,
    }
    impl Link {
        fn push(&mut self, t: u64, bytes: u64, marker: Option<u64>) {
            self.free_at = self.free_at.max(t) + bytes.div_ceil(self.rate);
            if let Some(n) = marker {
                self.answers.insert((self.free_at + self.rtt, n));
            }
        }
    }
    const CHUNK: u64 = 16 * 1024;
    let mut rec = Rec {
        pace: Pace::new(crate::flow::INITIAL_CREDIT as usize),
        ops: Vec::new(),
    };
    let mut nonce = 0u64;
    let mut send = |rec: &mut Rec, link: &mut Link, t: u64, more: bool| {
        if !more {
            let tail = rec.pace.stopped(false);
            rec.say(format!("stop 0 = {}", u8::from(tail)));
            if tail {
                nonce += 1;
                rec.pace.marker_sent(nonce, t * 1000);
                rec.say(format!("mark {nonce} {t}"));
                link.push(t, 0, Some(nonce));
            }
            return;
        }
        loop {
            match rec.pace.next(t * 1000) {
                Next::Marker => {
                    rec.say(format!("next {t} = M"));
                    nonce += 1;
                    rec.pace.marker_sent(nonce, t * 1000);
                    rec.say(format!("mark {nonce} {t}"));
                    link.push(t, 0, Some(nonce));
                }
                Next::Data(b) => {
                    rec.say(format!("next {t} = D {} {}", b.interactive, b.bulk));
                    let len = (b.bulk as u64).min(CHUNK);
                    if len < CHUNK && len < crate::mux::MIN_CUT as u64 {
                        let tail = rec.pace.stopped(true);
                        rec.say(format!("stop 1 = {}", u8::from(tail)));
                        if tail {
                            nonce += 1;
                            rec.pace.marker_sent(nonce, t * 1000);
                            rec.say(format!("mark {nonce} {t}"));
                            link.push(t, 0, Some(nonce));
                        }
                        return;
                    }
                    rec.pace.data_sent(len as usize, t * 1000);
                    rec.say(format!("data {len} {t}"));
                    link.push(t, len, None);
                }
            }
        }
    };
    let mut run = |rec: &mut Rec, link: &mut Link, from: u64, until: u64| {
        send(rec, link, from, true);
        while let Some((t, n)) = link.answers.pop_first() {
            let ours = rec.pace.answered(n, t * 1000);
            rec.say(format!("ans {n} {t} = {}", u8::from(ours)));
            send(rec, link, t, t < until);
        }
    };
    let mut slow = Link {
        rate: 125,
        rtt: 70,
        free_at: 0,
        answers: BTreeSet::new(),
    };
    run(&mut rec, &mut slow, 0, 11_000);
    let quiet = slow.free_at + 6_000;
    let mut fast = Link {
        rate: 6_250,
        rtt: 20,
        free_at: quiet,
        answers: BTreeSet::new(),
    };
    run(&mut rec, &mut fast, quiet, quiet + 120);
    // A keystroke's echo, too small to mark; a quiet round trip later it counts as delivered.
    let t = fast.free_at + 1_000;
    rec.pace.data_sent(60, t * 1000);
    rec.say(format!("data 60 {t}"));
    let tail = rec.pace.stopped(false);
    rec.say(format!("stop 0 = {}", u8::from(tail)));
    let t = t + 100;
    let next = match rec.pace.next(t * 1000) {
        Next::Marker => "M".to_string(),
        Next::Data(b) => format!("D {} {}", b.interactive, b.bulk),
    };
    rec.say(format!("next {t} = {next}"));
    rec.ops
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1000;

    /// A sender with endless bulk (16 KiB chunks) on a path that drains `rate`
    /// bytes a millisecond into a deep queue, a marker answered `rtt` ms after
    /// what is ahead of it has drained. The link can change under it; its
    /// clock and what is in flight carry on.
    struct Sim {
        now: u64,
        rate: u64,
        rtt: u64,
        queue: VecDeque<(u64, Option<u64>)>,
        answers: VecDeque<(u64, u64)>,
        nonce: u64,
        credit: u64,
        /// Delivered over the last `run`, and whether it drained during it.
        delivered: u64,
        drained: bool,
    }

    impl Sim {
        fn new(rate: u64, rtt: u64) -> Self {
            Self {
                now: 0,
                rate,
                rtt,
                queue: VecDeque::new(),
                answers: VecDeque::new(),
                nonce: 0,
                credit: 0,
                delivered: 0,
                drained: false,
            }
        }

        fn mark(&mut self, pace: &mut Pace) {
            self.nonce += 1;
            pace.marker_sent(self.nonce, self.now * MS);
            self.queue.push_back((0, Some(self.nonce)));
        }

        fn run(&mut self, pace: &mut Pace, ms: u64) -> &mut Self {
            self.delivered = 0;
            self.drained = false;
            for _ in 0..ms {
                while self.answers.front().is_some_and(|&(at, _)| at <= self.now) {
                    let (_, n) = self.answers.pop_front().unwrap();
                    pace.answered(n, self.now * MS);
                }
                loop {
                    match pace.next(self.now * MS) {
                        Next::Marker => self.mark(pace),
                        Next::Data(b) if b.bulk >= crate::mux::MIN_CUT => {
                            let n = (b.bulk as u64).min(16 * 1024);
                            pace.data_sent(n as usize, self.now * MS);
                            self.queue.push_back((n, None));
                        }
                        Next::Data(_) => {
                            if pace.stopped(true) {
                                self.mark(pace);
                            }
                            break;
                        }
                    }
                }
                self.drained |= pace.stats().draining;
                self.credit += self.rate;
                while let Some(&(bytes, marker)) = self.queue.front() {
                    if bytes > self.credit {
                        self.queue.front_mut().unwrap().0 -= self.credit;
                        self.delivered += self.credit;
                        self.credit = 0;
                        break;
                    }
                    self.credit -= bytes;
                    self.delivered += bytes;
                    self.queue.pop_front();
                    if let Some(n) = marker {
                        self.answers.push_back((self.now + self.rtt, n));
                    }
                }
                if self.queue.is_empty() {
                    self.credit = 0;
                }
                self.now += 1;
            }
            self
        }

        /// What waits in the queue, in ms of the link.
        fn queued_ms(&self) -> u64 {
            self.queue.iter().map(|(b, _)| b).sum::<u64>() / self.rate
        }
    }

    #[test]
    fn a_slow_path_is_held_near_the_floor_and_kept_busy() {
        // 1 Mbps (125 bytes a ms), 70 ms: the product is 9 KB.
        let mut pace = Pace::new(512 * 1024);
        let mut sim = Sim::new(125, 70);
        let delivered = sim.run(&mut pace, 20_000).delivered;
        let s = pace.stats();
        assert_eq!(s.cap, FLOOR, "{s:?}");
        assert!(!s.startup, "a flat rate ends startup: {s:?}");
        assert!(pace.inflight() <= FLOOR, "{s:?}");
        assert!(sim.queued_ms() <= 550, "{}", sim.queued_ms());
        // The link was kept busy: within 3% of 20 s at 1 Mbps, drains included.
        assert!(delivered as f64 >= 0.97 * 125.0 * 20_000.0, "{delivered}");
        let rate = s.rate.unwrap();
        assert!((110_000.0..=140_000.0).contains(&rate), "{rate}");
        let rtt = s.min_rtt_ms.unwrap();
        assert!((70.0..=110.0).contains(&rtt), "a clean round trip, not the queue's: {rtt}");
    }

    #[test]
    fn a_fast_path_grows_to_twice_its_product_within_a_few_round_trips() {
        // 50 Mbps (6250 bytes a ms), 20 ms: the product is 125 KB.
        let mut pace = Pace::new(512 * 1024);
        let mut sim = Sim::new(6250, 20);
        // Within the first 100 ms (five round trips) the link is busy most of the time…
        let early = sim.run(&mut pace, 100).delivered;
        assert!(early as f64 >= 0.75 * 6250.0 * 100.0, "{early} {:?}", pace.stats());
        // …and after it all of it, with twice the product in flight.
        let later = sim.run(&mut pace, 1_900).delivered;
        assert!(later as f64 >= 0.99 * 6250.0 * 1900.0, "{later}");
        let s = pace.stats();
        assert!(!s.startup && s.cap >= 230_000 && s.cap <= 330_000, "{s:?}");
        // The ceiling holds whatever the product: 50 Mbps at 200 ms is 1.25 MB.
        let mut pace = Pace::new(512 * 1024);
        Sim::new(6250, 200).run(&mut pace, 4_000);
        assert_eq!(pace.cap(), 512 * 1024);
    }

    #[test]
    fn a_path_that_slows_down_brings_its_cap_down_within_a_couple_of_seconds() {
        let mut pace = Pace::new(512 * 1024);
        let mut sim = Sim::new(6250, 20);
        sim.run(&mut pace, 2_000);
        assert!(pace.cap() > 200_000);
        // The same path, now 1 Mbps: what it had queued drains, the cap falls to the floor.
        sim.rate = 125;
        sim.run(&mut pace, 4_000);
        assert_eq!(pace.cap(), FLOOR, "{:?}", pace.stats());
        assert!(sim.queued_ms() <= 550, "{}", sim.queued_ms());
        // And back: it grows again within a second.
        sim.rate = 6250;
        let back = sim.run(&mut pace, 1_000).delivered;
        assert!(back as f64 >= 0.9 * 6250.0 * 1000.0, "{back} {:?}", pace.stats());
    }

    #[test]
    fn interactive_data_has_headroom_bulk_has_none_and_control_needs_no_budget() {
        let mut pace = Pace::new(512 * 1024);
        assert_eq!(pace.next(0), Next::Marker, "the first flight is timed first");
        pace.marker_sent(1, 0);
        pace.data_sent((FLOOR - OVERHEAD) as usize, 0);
        assert_eq!(pace.next(0), Next::Marker, "a spacing marker is due");
        pace.marker_sent(2, 0);
        assert_eq!(
            pace.next(0),
            Next::Data(Budget {
                interactive: HEADROOM as usize,
                bulk: 0
            })
        );
        assert!(!pace.stopped(true), "nothing unmarked");
        // The answers bring the room back.
        assert!(pace.answered(2, 70 * MS));
        assert_eq!(pace.inflight(), 0);
        assert!(!pace.answered(1, 70 * MS), "covered by the later one");
        assert!(!pace.answered(99, 70 * MS), "not a marker of this path");
    }

    #[test]
    fn a_small_tail_needs_no_marker_and_counts_as_delivered_after_a_quiet_round_trip() {
        let mut pace = Pace::new(512 * 1024);
        assert_eq!(pace.next(0), Next::Marker);
        pace.marker_sent(1, 0);
        pace.answered(1, 50 * MS);
        // A keystroke's echo, then nothing.
        pace.data_sent(60, 60 * MS);
        assert!(!pace.stopped(false), "too small to mark");
        assert_eq!(pace.inflight(), 60);
        // Not yet a round trip later: still out.
        let _ = pace.next(100 * MS);
        assert_eq!(pace.inflight(), 60);
        // The next one, a quiet round trip later, finds it delivered.
        let _ = pace.next(200 * MS);
        assert_eq!(pace.inflight(), 0);
        // A bigger tail is marked when the sender runs out.
        pace.data_sent(5000, 200 * MS);
        assert!(pace.stopped(false));
    }

    #[test]
    fn a_path_busy_past_the_window_is_drained_for_a_clean_round_trip() {
        let mut pace = Pace::new(512 * 1024);
        let mut sim = Sim::new(125, 70);
        assert!(!sim.run(&mut pace, 9_000).drained);
        let rtt = pace.stats().min_rtt_ms.unwrap();
        // Kept busy past the window, it drains once, and the minimum is fresh after.
        let before = pace.min_rtt.unwrap().1;
        assert!(sim.run(&mut pace, 3_000).drained, "{:?}", pace.stats());
        let (fresh, at) = pace.min_rtt.unwrap();
        assert!(at > before, "refreshed");
        assert!((fresh as f64 / 1000.0) <= rtt + 40.0, "{rtt} → {fresh} µs");
        // Draining cost the link about a round trip: 1 Mbps for 3 s, less a little.
        assert!(sim.delivered as f64 >= 0.95 * 125.0 * 3_000.0, "{}", sim.delivered);
        assert!(!sim.run(&mut pace, 5_000).drained, "not again so soon");
    }

    #[test]
    fn a_seeded_path_starts_from_the_other_paths_rate_and_measures_its_own_round_trip() {
        let mut relay = Pace::new(512 * 1024);
        Sim::new(6250, 20).run(&mut relay, 2_000);
        let (_, rate) = relay.rate_sample().unwrap();
        let mut direct = Pace::seeded(4 << 20, relay.rate_sample());
        assert_eq!(direct.cap(), FLOOR, "no round trip of its own yet");
        assert_eq!(direct.next(3_000 * MS), Next::Marker);
        direct.marker_sent(1, 3_000 * MS);
        direct.answered(1, 3_005 * MS);
        // The relay's 50 Mbps at its own 5 ms, started up.
        assert_eq!(direct.cap(), ((STARTUP_GAIN * rate * 5.0 / 1e3) as u64).max(FLOOR));
        let mut lan = Pace::seeded(4 << 20, Some((0, 50e6)));
        lan.marker_sent(1, 0);
        lan.answered(1, 10 * MS);
        assert_eq!(lan.cap(), (STARTUP_GAIN * 500_000.0) as u64);
        // A path with its own rate keeps it.
        let mut own = relay.clone();
        let before = own.rate_sample();
        own.seed_rate(Some((0, 1.0)));
        assert_eq!(own.rate_sample(), before);
    }

    #[test]
    fn the_shared_scenario_covers_a_drain_startup_and_a_quiet_tail() {
        let ops = pace_scenario();
        let flag = |op: &String, back: usize| op.split(' ').nth_back(back) == Some("1");
        assert!(ops.iter().any(|op| flag(op, 1)), "a drain");
        let started = ops.iter().position(|op| flag(op, 0)).unwrap();
        assert!(ops[started..].iter().any(|op| !flag(op, 0)), "startup ends");
        assert!(ops.iter().any(|op| op.starts_with("next ") && op.contains("= M")), "markers");
        assert!(ops.iter().any(|op| op.starts_with("stop 1 = 1")), "a tail marked when held");
        // The keystroke at the end counts as delivered: nothing in flight.
        let last = ops.last().unwrap();
        assert!(last.starts_with("next ") && last.split(' ').nth_back(5) == Some("0"), "{last}");
        assert!(ops.len() < 4_000, "{}", ops.len());
    }
}
