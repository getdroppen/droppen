//! An established end-to-end channel on one hub `ch`: the Noise transport and
//! the sub-stream multiplexer glued together, sans-IO.
//!
//! Outbound, every mux frame becomes one transport message and so one `DATA`
//! frame; inbound, every `DATA` frame on this `ch` is one transport message.
//! The hub sees a `DATA` frame's length and timing and nothing else.
//!
//! # Two paths
//!
//! A channel whose two ends negotiated [`crate::path::CLIENT_FEATURE`] is made
//! with [`Session::with_paths`]: every mux frame then travels in a sequenced
//! [`Envelope`] ([`crate::path::Lanes`]), and a second transport — a Noise
//! session run over a WebRTC DataChannel ([`crate::path`]) — can be attached
//! with [`Session::attach_direct`]. While it is attached, new frames go that
//! way ([`Wire::Direct`]); frames sent there are kept until the peer
//! acknowledges them. When it fails (the DataChannel closes, nothing comes
//! over it for [`DIRECT_TIMEOUT_MS`], what it carries waits for an
//! acknowledgement while the path makes no progress for
//! [`UNACKED_TIMEOUT_MS`] or ten round trips, a message on it does not open,
//! or the peer says so with [`Envelope::Down`]), [`Session::detach_direct`] sends
//! everything unacknowledged again over the hub, and the receiver drops what
//! it already had. So the multiplexer above never sees a frame lost,
//! repeated or out of order, whichever way each one went.
//!
//! The direct path is probed every [`DIRECT_PROBE_MS`] (its keep-alive, and
//! its round-trip time); the hub path every [`RELAY_PROBE_MS`] when
//! [`PathOptions::probe_relay`] asks for it (the device does, for the path
//! indicator). Timers are driven by [`Session::tick`] at
//! [`Session::next_deadline_us`]. Path times are microseconds of the same wall
//! clock the transports use for rekeying (in milliseconds).

use std::collections::{HashMap, VecDeque};

use thiserror::Error;

use crate::frame::Frame;
use crate::mux::{Mux, MuxError, MuxEvent, MuxFrame, Role};
use crate::noise::{ChannelError, Transport};
use crate::path::{
    DcMessage, DownReason, Envelope, LaneStats, Lanes, PathError, Via, DIRECT_PROBE_MS, DIRECT_TIMEOUT_MS, RELAY_PROBE_MS, UNACKED_LIMIT,
    UNACKED_TIMEOUT_MS,
};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SessionError {
    #[error(transparent)]
    Channel(#[from] ChannelError),
    #[error(transparent)]
    Mux(#[from] MuxError),
    #[error(transparent)]
    Path(#[from] PathError),
}

/// A sealed frame, and the stream data it carried (`(sid, len)`), if any.
pub type Polled = (Frame, Option<(u32, usize)>);

/// A sealed message for one of the paths, and the stream data it carried.
pub type PolledWire = (Wire, Option<(u32, usize)>);

/// Where a sealed message goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wire {
    /// A `DATA` frame for the hub.
    Relay(Frame),
    /// One DataChannel message ([`DcMessage::Sealed`], encoded).
    Direct(Vec<u8>),
}

/// Which paths can take a message right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Room {
    pub relay: bool,
    pub direct: bool,
}

/// How a two-path session behaves.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PathOptions {
    /// Probe the hub path every [`RELAY_PROBE_MS`] for its round-trip time.
    pub probe_relay: bool,
}

/// Something about the paths the owner may want to know.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathEvent {
    /// A direct path was attached; new frames go that way.
    DirectUp,
    /// The direct path is no longer used (`remote`: the peer said so); `why`
    /// says what gave it up, for the logs.
    DirectDown { reason: DownReason, remote: bool, why: &'static str },
    /// A probe came back.
    Rtt { via: Via, ms: f64 },
}

/// The paths' state, for logs and the path indicator.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PathStats {
    pub direct: bool,
    pub relay_rtt_ms: Option<f64>,
    pub direct_rtt_ms: Option<f64>,
    pub ups: u32,
    pub downs: u32,
    pub lanes: LaneStats,
}

/// The most probes outstanding at once (older ones are forgotten).
const MAX_PROBES: usize = 16;

struct Paths {
    opts: PathOptions,
    lanes: Lanes,
    direct: Option<Transport>,
    /// Envelopes for the hub path outside the multiplexer's order: a `DOWN`,
    /// resends after a fallback (sequenced already), probes and their answers.
    out_relay: VecDeque<Envelope>,
    /// Probes and their answers for the direct path.
    out_direct: VecDeque<Envelope>,
    probes: HashMap<u64, (Via, u64)>,
    next_nonce: u64,
    last_direct_rx_us: u64,
    /// When the direct path last showed it delivers: the peer's data arrived
    /// over it, or an acknowledgement freed frames sent over it. Probes do
    /// not count: they say the peer is there, not that what this side sent
    /// got through.
    last_direct_progress_us: u64,
    next_direct_probe_us: u64,
    next_relay_probe_us: u64,
    events: VecDeque<PathEvent>,
    stats: PathStats,
}

pub struct Session {
    ch: u32,
    transport: Transport,
    mux: Mux,
    paths: Option<Paths>,
}

impl Session {
    pub fn new(ch: u32, transport: Transport, role: Role) -> Self {
        Self {
            ch,
            transport,
            mux: Mux::new(role),
            paths: None,
        }
    }

    /// A session whose ends negotiated [`crate::path::CLIENT_FEATURE`]: every
    /// frame sequenced, and a direct path can be attached.
    pub fn with_paths(ch: u32, transport: Transport, role: Role, opts: PathOptions, now_us: u64) -> Self {
        Self {
            ch,
            transport,
            mux: Mux::new(role),
            paths: Some(Paths {
                opts,
                lanes: Lanes::new(),
                direct: None,
                out_relay: VecDeque::new(),
                out_direct: VecDeque::new(),
                probes: HashMap::new(),
                next_nonce: 1,
                last_direct_rx_us: now_us,
                last_direct_progress_us: now_us,
                next_direct_probe_us: now_us,
                // The first hub probe goes out at once, so the indicator has a number.
                next_relay_probe_us: now_us,
                events: VecDeque::new(),
                stats: PathStats::default(),
            }),
        }
    }

    pub fn ch(&self) -> u32 {
        self.ch
    }

    pub fn mux(&mut self) -> &mut Mux {
        &mut self.mux
    }

    /// The hub channel's transport.
    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    pub fn has_paths(&self) -> bool {
        self.paths.is_some()
    }

    /// Whether new frames go over a direct path now.
    pub fn direct_active(&self) -> bool {
        self.paths.as_ref().is_some_and(|p| p.direct.is_some())
    }

    pub fn stats(&self) -> PathStats {
        self.paths.as_ref().map_or_else(PathStats::default, |p| {
            let mut s = p.stats;
            s.direct = p.direct.is_some();
            s.lanes = p.lanes.stats();
            s
        })
    }

    /// What happened to the paths since the last call.
    pub fn take_events(&mut self) -> Vec<PathEvent> {
        self.paths.as_mut().map(|p| p.events.drain(..).collect()).unwrap_or_default()
    }

    /// Start sending over a direct path whose handshake has just completed. A
    /// direct path already attached is let go first (its unacknowledged frames
    /// go again over the hub).
    pub fn attach_direct(&mut self, transport: Transport, now_us: u64) -> Result<(), SessionError> {
        let Some(p) = self.paths.as_mut() else {
            return Err(PathError::Protocol("this channel did not negotiate paths").into());
        };
        if p.direct.is_some() {
            p.detach(DownReason::Closed, false);
        }
        p.direct = Some(transport);
        p.last_direct_rx_us = now_us;
        p.last_direct_progress_us = now_us;
        p.next_direct_probe_us = now_us;
        p.out_direct.clear();
        p.stats.ups += 1;
        p.events.push_back(PathEvent::DirectUp);
        Ok(())
    }

    /// Stop using the direct path: everything sent there and not acknowledged
    /// goes again over the hub, after a `DOWN` that tells the peer to stop too
    /// (`tell_peer`, unless it was the peer that said so).
    pub fn detach_direct(&mut self, reason: DownReason, tell_peer: bool) {
        self.detach_direct_because(reason, tell_peer, "its owner let it go (the DataChannel closed, or by hand)");
    }

    fn detach_direct_because(&mut self, reason: DownReason, tell_peer: bool, why: &'static str) {
        if let Some(p) = self.paths.as_mut() {
            if p.direct.is_some() {
                p.detach(reason, tell_peer);
                p.events.push_back(PathEvent::DirectDown { reason, remote: false, why });
            }
        }
    }

    /// Everything the multiplexer can send right now, sealed into `DATA` frames
    /// (the hub path only).
    pub fn flush(&mut self, now_ms: u64) -> Result<Vec<Frame>, SessionError> {
        self.flush_observed(now_ms, |_, _| {})
    }

    /// [`Session::flush`], telling `on_data(sid, len)` about every chunk of
    /// stream data that leaves — so a producer feeding a stream can be held
    /// back while what it queued has not gone yet.
    pub fn flush_observed(&mut self, now_ms: u64, mut on_data: impl FnMut(u32, usize)) -> Result<Vec<Frame>, SessionError> {
        let mut out = Vec::new();
        while let Some((frame, data)) = self.poll_frame(now_ms)? {
            if let Some((sid, len)) = data {
                on_data(sid, len);
            }
            out.push(frame);
        }
        Ok(out)
    }

    /// The next frame for the hub, sealed, if there is one — for a caller
    /// that has to stop when the hub's credit runs out and has no direct path.
    pub fn poll_frame(&mut self, now_ms: u64) -> Result<Option<Polled>, SessionError> {
        let room = Room { relay: true, direct: false };
        Ok(match self.poll(now_ms.saturating_mul(1000), room)? {
            Some((Wire::Relay(frame), data)) => Some((frame, data)),
            Some((Wire::Direct(_), _)) => unreachable!("no room on the direct path"),
            None => None,
        })
    }

    /// The next sealed message for a path with room, if there is one. The
    /// `(sid, len)` is the stream data it carried, if any.
    pub fn poll(&mut self, now_us: u64, room: Room) -> Result<Option<PolledWire>, SessionError> {
        let now_ms = now_us / 1000;
        let Some(p) = self.paths.as_mut() else {
            if !room.relay {
                return Ok(None);
            }
            let Some(frame) = self.mux.poll() else { return Ok(None) };
            let data = data_of(&frame);
            let sealed = self.transport.seal(now_ms, &frame.encode())?;
            return Ok(Some((
                Wire::Relay(Frame::Data {
                    ch: self.ch,
                    payload: sealed,
                }),
                data,
            )));
        };
        // Queued envelopes for each path first: on the hub path that is what
        // a fallback resends, which must go before anything newer.
        if room.relay {
            if let Some(env) = p.out_relay.pop_front() {
                p.note_probe(&env, Via::Relay, now_us);
                let sealed = self.transport.seal(now_ms, &env.encode())?;
                return Ok(Some((
                    Wire::Relay(Frame::Data {
                        ch: self.ch,
                        payload: sealed,
                    }),
                    None,
                )));
            }
        }
        if room.direct && p.direct.is_some() {
            if let Some(env) = p.out_direct.pop_front() {
                p.note_probe(&env, Via::Direct, now_us);
                let sealed = p.direct.as_mut().expect("checked").seal(now_ms, &env.encode())?;
                return Ok(Some((Wire::Direct(DcMessage::Sealed(sealed).encode()), None)));
            }
        }
        let via = if p.direct.is_some() { Via::Direct } else { Via::Relay };
        if !p.path_room(room) {
            return Ok(None);
        }
        // New frames on the direct path also wait while too much it carries
        // is unacknowledged; an acknowledgement never does (it is not kept).
        let data_room = via == Via::Relay || p.lanes.unacked_bytes() < UNACKED_LIMIT;
        let frame = if data_room { self.mux.poll() } else { None };
        let (env, data) = if let Some(frame) = frame {
            let data = data_of(&frame);
            (p.lanes.stamp_at(frame.encode(), via == Via::Direct, now_ms), data)
        } else if p.lanes.ack_due_ms().is_some_and(|due| due <= now_ms) {
            (Envelope::Ack { ack: p.lanes.take_ack() }, None)
        } else {
            return Ok(None);
        };
        Ok(Some(match via {
            Via::Relay => {
                let sealed = self.transport.seal(now_ms, &env.encode())?;
                (
                    Wire::Relay(Frame::Data {
                        ch: self.ch,
                        payload: sealed,
                    }),
                    data,
                )
            }
            Via::Direct => {
                let sealed = p.direct.as_mut().expect("checked").seal(now_ms, &env.encode())?;
                (Wire::Direct(DcMessage::Sealed(sealed).encode()), data)
            }
        }))
    }

    /// One `DATA` payload from the hub (milliseconds). Any error is fatal to
    /// the channel.
    pub fn receive(&mut self, payload: &[u8], now_ms: u64) -> Result<Vec<MuxEvent>, SessionError> {
        self.receive_relay(payload, now_ms.saturating_mul(1000))
    }

    /// One `DATA` payload from the hub. Any error is fatal to the channel.
    pub fn receive_relay(&mut self, payload: &[u8], now_us: u64) -> Result<Vec<MuxEvent>, SessionError> {
        let plaintext = self.transport.open(payload)?;
        if self.paths.is_none() {
            let frame = MuxFrame::decode(&plaintext).map_err(MuxError::from)?;
            return Ok(self.mux.receive(frame)?.into_iter().collect());
        }
        let env = Envelope::decode(&plaintext).map_err(PathError::from)?;
        self.on_envelope(env, Via::Relay, now_us)
    }

    /// One DataChannel message. Trouble with the direct path itself (a message
    /// that does not open or parse) lets the direct path go — the hub path
    /// carries on; only a violation in what it carried is fatal.
    pub fn receive_direct(&mut self, message: &[u8], now_us: u64) -> Result<Vec<MuxEvent>, SessionError> {
        let Some(p) = self.paths.as_mut() else {
            return Ok(Vec::new());
        };
        let Some(direct) = p.direct.as_mut() else {
            // Left over from a direct path already let go.
            return Ok(Vec::new());
        };
        let opened = match DcMessage::decode(message) {
            Ok(DcMessage::Sealed(sealed)) => direct.open(&sealed).ok(),
            _ => None,
        };
        let env = match opened.map(|plain| Envelope::decode(&plain)) {
            // `DOWN` belongs on the hub path only.
            Some(Ok(Envelope::Down { .. })) | Some(Err(_)) | None => {
                self.detach_direct_because(DownReason::Failed, true, "a message on it did not open");
                return Ok(Vec::new());
            }
            Some(Ok(env)) => env,
        };
        p.last_direct_rx_us = now_us;
        self.on_envelope(env, Via::Direct, now_us)
    }

    fn on_envelope(&mut self, env: Envelope, via: Via, now_us: u64) -> Result<Vec<MuxEvent>, SessionError> {
        let p = self.paths.as_mut().expect("paths");
        let mut events = Vec::new();
        match env {
            Envelope::Data { seq, ack, frame } => {
                p.acked(ack, now_us)?;
                if via == Via::Direct {
                    p.last_direct_progress_us = now_us;
                }
                for bytes in p.lanes.receive(seq, frame, via == Via::Direct, now_us / 1000)? {
                    let frame = MuxFrame::decode(&bytes).map_err(MuxError::from)?;
                    if let Some(event) = self.mux.receive(frame)? {
                        events.push(event);
                    }
                }
            }
            Envelope::Ack { ack } => p.acked(ack, now_us)?,
            Envelope::Probe { nonce } => {
                // Answered at once, with any acknowledgement owed: a side whose
                // timers are held back (a page in a background tab) still
                // acknowledges once per probe, which comes from the other side.
                let owed = (p.lanes.owed() > 0).then(|| Envelope::Ack { ack: p.lanes.take_ack() });
                let out = match via {
                    Via::Relay => &mut p.out_relay,
                    Via::Direct => &mut p.out_direct,
                };
                out.push_back(Envelope::ProbeAck { nonce });
                out.extend(owed);
            }
            Envelope::ProbeAck { nonce } => {
                if let Some((sent_via, sent_us)) = p.probes.remove(&nonce) {
                    if sent_via == via {
                        let ms = now_us.saturating_sub(sent_us) as f64 / 1000.0;
                        match via {
                            Via::Relay => p.stats.relay_rtt_ms = Some(ms),
                            Via::Direct => p.stats.direct_rtt_ms = Some(ms),
                        }
                        p.events.push_back(PathEvent::Rtt { via, ms });
                    }
                }
            }
            Envelope::Down { reason } => {
                if p.direct.is_some() {
                    p.detach(reason, false);
                    p.events.push_back(PathEvent::DirectDown {
                        reason,
                        remote: true,
                        why: "the peer gave it up",
                    });
                }
            }
        }
        Ok(events)
    }

    /// Run the paths' timers: probes, the direct path's keep-alive and
    /// timeout. A standalone acknowledgement that falls due is sent by
    /// [`Session::poll`].
    pub fn tick(&mut self, now_us: u64) {
        let Some(p) = self.paths.as_mut() else { return };
        if p.direct.is_some() {
            let silent = now_us.saturating_sub(p.last_direct_rx_us) > DIRECT_TIMEOUT_MS * 1000;
            let stuck = p.unacked_deadline_us().is_some_and(|at| now_us > at);
            if silent || stuck {
                p.detach(DownReason::Timeout, true);
                p.events.push_back(PathEvent::DirectDown {
                    reason: DownReason::Timeout,
                    remote: false,
                    why: if silent {
                        "nothing came over it for DIRECT_TIMEOUT_MS"
                    } else {
                        "what it carried went unacknowledged while it made no progress for UNACKED_TIMEOUT_MS"
                    },
                });
            } else if now_us >= p.next_direct_probe_us {
                p.next_direct_probe_us = now_us + DIRECT_PROBE_MS * 1000;
                let nonce = p.nonce();
                p.out_direct.push_back(Envelope::Probe { nonce });
            }
        }
        if p.opts.probe_relay && now_us >= p.next_relay_probe_us {
            p.next_relay_probe_us = now_us + RELAY_PROBE_MS * 1000;
            p.age_relay_rtt(now_us);
            let nonce = p.nonce();
            p.out_relay.push_back(Envelope::Probe { nonce });
        }
    }

    /// When [`Session::tick`] (or [`Session::poll`], for an acknowledgement)
    /// next has something to do, given the `room` there is now: an
    /// acknowledgement that could not go anyway is left out — waking for it
    /// would only spin until room comes, and whatever brings room sends it.
    pub fn next_deadline_us(&self, room: Room) -> Option<u64> {
        let p = self.paths.as_ref()?;
        let mut next = if p.path_room(room) {
            p.lanes.ack_due_ms().map(|ms| ms * 1000)
        } else {
            None
        };
        let mut take = |t: u64| next = Some(next.map_or(t, |n: u64| n.min(t)));
        if p.direct.is_some() {
            take(p.next_direct_probe_us);
            take(p.last_direct_rx_us + DIRECT_TIMEOUT_MS * 1000 + 1);
            if let Some(at) = p.unacked_deadline_us() {
                take(at + 1);
            }
        }
        if p.opts.probe_relay {
            take(p.next_relay_probe_us);
        }
        next
    }
}

impl Paths {
    /// Whether the path new envelopes go on (the direct one when attached)
    /// can take one now. The hub path waits behind its own queued envelopes.
    fn path_room(&self, room: Room) -> bool {
        if self.direct.is_some() {
            room.direct
        } else {
            room.relay && self.out_relay.is_empty()
        }
    }

    /// When frames wait for an acknowledgement on the direct path and it has
    /// made no progress for too long: [`UNACKED_TIMEOUT_MS`] or ten round
    /// trips, counted from the later of the oldest such frame leaving and the
    /// path's last progress. Counted from the frame alone, a backlog the link
    /// drains more slowly than that (the DataChannel's own buffer, or the
    /// peer's acknowledgement queued behind its own backlog) gave a healthy
    /// path up.
    fn unacked_deadline_us(&self) -> Option<u64> {
        let sent_us = self.lanes.oldest_unacked_ms()? * 1000;
        let rtt_ms = self.stats.direct_rtt_ms.unwrap_or(0.0);
        let wait_ms = (UNACKED_TIMEOUT_MS as f64).max(rtt_ms * 10.0) as u64;
        Some(sent_us.max(self.last_direct_progress_us) + wait_ms * 1000)
    }

    /// An acknowledgement came (on either path): progress on the direct path
    /// when it freed frames sent there.
    fn acked(&mut self, ack: u64, now_us: u64) -> Result<(), PathError> {
        if self.lanes.on_ack(ack)? > 0 {
            self.last_direct_progress_us = now_us;
        }
        Ok(())
    }

    fn nonce(&mut self) -> u64 {
        let n = self.next_nonce;
        self.next_nonce += 1;
        n
    }

    /// A hub probe still unanswered says the hub path is at least that slow:
    /// the round trip shown is raised to its age, so the indicator never keeps
    /// an older, faster number for a path that stopped answering.
    fn age_relay_rtt(&mut self, now_us: u64) {
        let Some(oldest) = self.probes.values().filter(|(via, _)| *via == Via::Relay).map(|(_, at)| *at).min() else {
            return;
        };
        let age_ms = now_us.saturating_sub(oldest) as f64 / 1000.0;
        if self.stats.relay_rtt_ms.is_none_or(|ms| age_ms > ms) {
            self.stats.relay_rtt_ms = Some(age_ms);
        }
    }

    /// Remember when a probe left (and on which path).
    fn note_probe(&mut self, env: &Envelope, via: Via, now_us: u64) {
        if let Envelope::Probe { nonce } = env {
            if self.probes.len() >= MAX_PROBES {
                if let Some(oldest) = self.probes.keys().min().copied() {
                    self.probes.remove(&oldest);
                }
            }
            self.probes.insert(*nonce, (via, now_us));
        }
    }

    fn detach(&mut self, reason: DownReason, tell_peer: bool) {
        self.direct = None;
        self.out_direct.clear();
        self.probes.retain(|_, (via, _)| *via == Via::Relay);
        self.stats.downs += 1;
        // The resends go first, ahead of anything queued for the hub path
        // since (a probe, a probe's answer), and the `DOWN` ahead of them.
        let resend = self.lanes.take_unacked();
        for env in resend.into_iter().rev() {
            self.out_relay.push_front(env);
        }
        if tell_peer {
            self.out_relay.push_front(Envelope::Down { reason });
        }
    }
}

fn data_of(frame: &MuxFrame) -> Option<(u32, usize)> {
    match frame {
        MuxFrame::Data { sid, data } => Some((*sid, data.len())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::DhKeypair;
    use crate::mux::StreamClass;
    use crate::noise::tests::binding;
    use crate::noise::{Initiator, RekeyPolicy, Responder};
    use crate::testing::bytes32;

    fn transports(seed: u64) -> (Transport, Transport) {
        let device = DhKeypair::from_secret(bytes32(seed));
        let agent = DhKeypair::from_secret(bytes32(seed + 1));
        let mut i = Initiator::new(&device, &agent.public(), &binding()).unwrap();
        let mut r = Responder::new(&agent, &binding()).unwrap();
        r.read_hello(&i.write_hello(0, b"").unwrap()).unwrap();
        let (m2, rt) = r.write_reply(b"", RekeyPolicy::default(), 0).unwrap();
        let (it, _) = i.read_reply(&m2, RekeyPolicy::default(), 0).unwrap();
        (it, rt)
    }

    struct Pair {
        a: Session,
        b: Session,
        /// In flight on the hub, each way.
        relay: [VecDeque<Frame>; 2],
        /// In flight on the direct path, each way.
        direct: [VecDeque<Vec<u8>>; 2],
        got: [Vec<u8>; 2],
        now: u64,
    }

    impl Pair {
        fn new() -> Self {
            let (it, rt) = transports(1);
            Self {
                a: Session::with_paths(1, it, Role::Initiator, PathOptions { probe_relay: true }, 0),
                b: Session::with_paths(1, rt, Role::Responder, PathOptions::default(), 0),
                relay: [VecDeque::new(), VecDeque::new()],
                direct: [VecDeque::new(), VecDeque::new()],
                got: [Vec::new(), Vec::new()],
                now: 0,
            }
        }

        fn attach(&mut self) {
            let (it, rt) = transports(10);
            self.a.attach_direct(it, self.now).unwrap();
            self.b.attach_direct(rt, self.now).unwrap();
        }

        fn side(&mut self, i: usize) -> &mut Session {
            if i == 0 {
                &mut self.a
            } else {
                &mut self.b
            }
        }

        /// Everything each side can send, onto the wires.
        fn send_all(&mut self) {
            for i in 0..2 {
                let now = self.now;
                let room = Room { relay: true, direct: true };
                while let Some((wire, _)) = self.side(i).poll(now, room).unwrap() {
                    match wire {
                        Wire::Relay(f) => self.relay[i].push_back(f),
                        Wire::Direct(m) => self.direct[i].push_back(m),
                    }
                }
            }
        }

        fn take(&mut self, to: usize, events: Vec<MuxEvent>) {
            for e in events {
                if let MuxEvent::Data { data, .. } = e {
                    self.got[to].extend(data);
                }
            }
        }

        /// Deliver one message from side `from`'s `via` wire.
        fn deliver(&mut self, from: usize, via: Via) -> bool {
            let to = 1 - from;
            let now = self.now;
            match via {
                Via::Relay => {
                    let Some(Frame::Data { payload, .. }) = self.relay[from].pop_front() else {
                        return false;
                    };
                    let ev = self.side(to).receive_relay(&payload, now).unwrap();
                    self.take(to, ev);
                }
                Via::Direct => {
                    let Some(m) = self.direct[from].pop_front() else { return false };
                    let ev = self.side(to).receive_direct(&m, now).unwrap();
                    self.take(to, ev);
                }
            }
            true
        }
    }

    /// A terminal stream from a to b, open on both sides.
    fn open_stream(p: &mut Pair) -> u32 {
        let sid = p.a.mux().open(StreamClass::Term, b"t").unwrap();
        p.send_all();
        while p.deliver(0, Via::Relay) {}
        sid
    }

    fn type_keys(p: &mut Pair, sid: u32, typed: &mut Vec<u8>, n: usize) {
        for _ in 0..n {
            let key = typed.len() as u8;
            p.a.mux().send(sid, &[key]).unwrap();
            typed.push(key);
            p.send_all();
        }
    }

    #[test]
    fn keystrokes_cross_a_switch_to_direct_and_a_fallback_in_order() {
        let mut p = Pair::new();
        let sid = open_stream(&mut p);
        let mut typed = Vec::new();
        // On the hub only.
        type_keys(&mut p, sid, &mut typed, 20);
        assert!(p.direct[0].is_empty());
        // The direct path comes up while those 20 keystrokes are still on the hub.
        p.attach();
        type_keys(&mut p, sid, &mut typed, 30);
        assert_eq!((p.relay[0].len(), p.direct[0].len()), (20, 30));
        // The direct path is faster: everything on it arrives first, and is held.
        while p.deliver(0, Via::Direct) {}
        assert!(p.got[1].is_empty(), "nothing is handed up ahead of the gap");
        while p.deliver(0, Via::Relay) {}
        assert_eq!(p.got[1], typed, "in order once the hub's catch up");
        // Answers flow back over the direct path, acknowledgements with them.
        p.b.mux().send(sid, b"echo").unwrap();
        p.send_all();
        while p.deliver(1, Via::Direct) {}
        assert_eq!(p.got[0], b"echo");

        // Mid-typing the direct path dies: what is in flight on it is lost.
        type_keys(&mut p, sid, &mut typed, 15);
        assert_eq!(p.direct[0].len(), 15);
        p.direct[0].clear();
        p.now += (DIRECT_TIMEOUT_MS + 1) * 1000;
        p.a.tick(p.now);
        assert!(!p.a.direct_active(), "a noticed by the timeout");
        type_keys(&mut p, sid, &mut typed, 5);
        // b hears a `DOWN` over the hub first, then the resends, then the new keys.
        while p.deliver(0, Via::Relay) {}
        assert!(!p.b.direct_active(), "b stopped using it when told");
        assert_eq!(p.got[1], typed, "nothing lost, nothing twice, in order");
        assert_eq!(p.b.stats().lanes.duplicates, 0);
        assert_eq!(p.a.stats().lanes.resent, 15);
        assert_eq!((p.a.stats().ups, p.a.stats().downs), (1, 1));
        let events = p.b.take_events();
        assert!(events.contains(&PathEvent::DirectUp));
        assert!(events.iter().any(|e| matches!(
            e,
            PathEvent::DirectDown {
                reason: DownReason::Timeout,
                remote: true,
                ..
            }
        )));
    }

    #[test]
    fn frames_the_direct_path_did_deliver_are_not_handed_up_twice() {
        let mut p = Pair::new();
        let sid = open_stream(&mut p);
        p.attach();
        for k in 0..10u8 {
            p.a.mux().send(sid, &[k]).unwrap();
        }
        p.send_all();
        // All of them arrive directly, but no acknowledgement makes it back.
        while p.deliver(0, Via::Direct) {}
        p.send_all();
        p.direct[1].clear();
        // b's DataChannel closes; a hears it over the hub and resends all ten.
        p.b.detach_direct(DownReason::Failed, true);
        p.send_all();
        while p.deliver(1, Via::Relay) {}
        assert!(!p.a.direct_active());
        p.send_all();
        while p.deliver(0, Via::Relay) {}
        assert_eq!(p.got[1], (0..10u8).collect::<Vec<_>>());
        assert_eq!(p.b.stats().lanes.duplicates, 10, "the resends were recognised and dropped");
    }

    #[test]
    fn probes_measure_each_path_and_keep_the_direct_one_alive() {
        let mut p = Pair::new();
        open_stream(&mut p);
        p.attach();
        p.now = 1_000;
        p.a.tick(p.now);
        p.send_all();
        p.now = 3_500;
        while p.deliver(0, Via::Direct) {}
        while p.deliver(0, Via::Relay) {}
        p.send_all();
        p.now = 4_000;
        while p.deliver(1, Via::Direct) {}
        while p.deliver(1, Via::Relay) {}
        let s = p.a.stats();
        assert!(s.direct);
        assert_eq!(s.direct_rtt_ms, Some(3.0));
        assert_eq!(s.relay_rtt_ms, Some(3.0));
        // b probes only the direct path.
        assert_eq!(p.b.stats().relay_rtt_ms, None);
        // Probes keep it alive past the timeout, as long as answers come.
        for step in 1..=10u64 {
            p.now = 4_000 + step * DIRECT_PROBE_MS * 1000;
            p.a.tick(p.now);
            p.b.tick(p.now);
            for _ in 0..2 {
                p.send_all();
                while p.deliver(0, Via::Direct) || p.deliver(1, Via::Direct) {}
            }
        }
        assert!(p.a.direct_active() && p.b.direct_active());
        assert!(p.a.next_deadline_us(Room { relay: true, direct: true }).is_some());
    }

    /// A hub path that stops answering its probes shows ever slower — the age
    /// of the oldest unanswered probe — never its last good round trip.
    #[test]
    fn a_hub_path_that_stops_answering_its_probes_shows_ever_slower() {
        let mut p = Pair::new();
        open_stream(&mut p);
        p.a.tick(p.now);
        p.send_all();
        p.now = 3_000;
        while p.deliver(0, Via::Relay) {}
        p.send_all();
        while p.deliver(1, Via::Relay) {}
        assert_eq!(p.a.stats().relay_rtt_ms, Some(3.0));
        // The hub stops carrying anything: the probes queue up, unanswered.
        for at in [RELAY_PROBE_MS, 2 * RELAY_PROBE_MS, 3 * RELAY_PROBE_MS] {
            p.now = at * 1000;
            p.a.tick(p.now);
            p.send_all();
        }
        assert_eq!(
            p.a.stats().relay_rtt_ms,
            Some((2 * RELAY_PROBE_MS) as f64),
            "the oldest unanswered probe's age, not 3 ms"
        );
        // When the hub carries again, the answers are round trips again.
        p.now = 4 * RELAY_PROBE_MS * 1000;
        p.a.tick(p.now);
        p.send_all();
        while p.deliver(0, Via::Relay) {}
        p.send_all();
        while p.deliver(1, Via::Relay) {}
        assert_eq!(
            p.a.stats().relay_rtt_ms,
            Some(0.0),
            "measured afresh: the latest probe was answered at once"
        );
    }

    /// An acknowledgement that frees nothing sent on the direct path is no
    /// progress on it: a peer still talking over the hub (it has not attached
    /// the direct path, or already let it go) does not keep a dead one alive.
    #[test]
    fn an_acknowledgement_that_frees_nothing_is_no_progress_on_the_direct_path() {
        let mut p = Pair::new();
        let sid = open_stream(&mut p);
        // Only a has the direct path: b answers over the hub.
        let (it, _) = transports(10);
        p.a.attach_direct(it, p.now).unwrap();
        p.now = 1_000_000;
        let sent = p.now;
        p.a.mux().send(sid, b"k").unwrap();
        p.send_all();
        p.direct[0].clear(); // The keystroke was lost on the direct path.
        let mut at = sent + 200_000;
        while at < sent + (UNACKED_TIMEOUT_MS - 100) * 1000 {
            p.now = at;
            p.b.mux().send(sid, b".").unwrap();
            p.send_all();
            while p.deliver(1, Via::Relay) {}
            p.a.tick(p.now);
            at += 200_000;
        }
        assert!(p.a.direct_active(), "not yet");
        p.now = sent + (UNACKED_TIMEOUT_MS + 1) * 1000;
        p.a.tick(p.now);
        assert!(
            !p.a.direct_active(),
            "given up on time: acknowledgements that free nothing sent there did not keep it"
        );
    }

    #[test]
    fn frames_that_go_unacknowledged_give_the_direct_path_up_before_the_silence_timeout() {
        let mut p = Pair::new();
        let sid = open_stream(&mut p);
        p.attach();
        p.now = 1_000_000;
        let sent = p.now;
        p.a.mux().send(sid, b"k").unwrap();
        p.send_all();
        p.direct[0].clear(); // The keystroke was lost and is never acknowledged…
        let deadline = p.a.next_deadline_us(Room { relay: true, direct: true }).unwrap();
        assert!(deadline <= p.now + UNACKED_TIMEOUT_MS * 1000 + 1, "a timer for it");
        // …while probes still come and go (the path is not silent): they say
        // the peer is there, not that the keystroke got through.
        p.now += 700_000;
        p.a.tick(p.now);
        p.b.tick(p.now);
        for _ in 0..2 {
            p.send_all();
            while p.deliver(0, Via::Direct) || p.deliver(1, Via::Direct) {}
        }
        p.now = sent + (UNACKED_TIMEOUT_MS - 100) * 1000;
        p.a.tick(p.now);
        assert!(p.a.direct_active(), "not yet");
        p.now += 200 * 1000;
        p.a.tick(p.now);
        assert!(!p.a.direct_active(), "given up after {UNACKED_TIMEOUT_MS} ms, not {DIRECT_TIMEOUT_MS}");
        p.send_all();
        while p.deliver(0, Via::Relay) {}
        assert_eq!(p.got[1], b"k", "and resent over the hub");
    }

    /// A direct path slowly draining a backlog (a cellular link slower than
    /// what is put on it) is not dead, however long the backlog takes: the
    /// peer's data keeps arriving and its acknowledgements keep freeing what
    /// was sent, so neither side gives it up — even though b's oldest frame,
    /// and a's keystroke (acknowledged behind b's backlog), wait far longer
    /// than [`UNACKED_TIMEOUT_MS`].
    #[test]
    fn a_direct_path_that_drains_a_backlog_slowly_is_kept_both_ways() {
        let mut p = Pair::new();
        let sid = open_stream(&mut p);
        p.attach();
        let chunks = 40u8;
        for k in 0..chunks {
            p.b.mux().send(sid, &[k; 512]).unwrap();
        }
        p.send_all();
        let step_us = 100_000;
        for step in 1..=60u64 {
            p.now = step * step_us;
            if step == 2 {
                p.a.mux().send(sid, b"k").unwrap();
            }
            p.a.tick(p.now);
            p.b.tick(p.now);
            p.send_all();
            // b → a: one message per step; a → b: at once.
            p.deliver(1, Via::Direct);
            while p.deliver(0, Via::Direct) {}
            p.send_all();
            assert!(
                p.a.direct_active() && p.b.direct_active(),
                "still direct at {} ms: {:?} {:?}",
                p.now / 1000,
                p.a.stats(),
                p.b.stats()
            );
        }
        assert!(p.direct[1].is_empty(), "the backlog drained");
        let expected: Vec<u8> = (0..chunks).flat_map(|k| [k; 512]).collect();
        assert_eq!(p.got[0], expected, "every chunk, once, in order");
        assert_eq!(p.got[1], b"k");
        for s in [p.a.stats(), p.b.stats()] {
            assert_eq!((s.downs, s.lanes.resent), (0, 0), "nothing given up, nothing sent twice: {s:?}");
        }
    }

    #[test]
    fn an_acknowledgement_with_no_room_to_go_sets_no_timer_and_one_with_room_ignores_the_unacked_limit() {
        let mut p = Pair::new();
        let sid = open_stream(&mut p);
        p.attach();
        p.now = 5_000_000;
        p.a.mux().send(sid, b"x").unwrap();
        p.send_all();
        while p.deliver(0, Via::Direct) {}
        // Its probe goes out now, the next in a second; and b owes an
        // acknowledgement, due in ACK_DELAY_MS.
        p.b.tick(p.now);
        let full = Room { relay: true, direct: false };
        let due = p.b.next_deadline_us(Room { relay: true, direct: true }).unwrap();
        assert!(due <= p.now + crate::path::ACK_DELAY_MS * 1000);
        assert!(
            p.b.next_deadline_us(full).is_none_or(|d| d > due),
            "no wake-up for an ack the direct path cannot take"
        );
        p.now += crate::path::ACK_DELAY_MS * 1000;
        assert!(p.b.poll(p.now, full).unwrap().is_none());
        let (wire, _) = p.b.poll(p.now, Room { relay: true, direct: true }).unwrap().expect("the ack");
        assert!(matches!(wire, Wire::Direct(_)));
    }

    #[test]
    fn a_garbled_direct_message_drops_the_direct_path_not_the_channel() {
        let mut p = Pair::new();
        let sid = open_stream(&mut p);
        p.attach();
        p.a.mux().send(sid, b"x").unwrap();
        p.send_all();
        let mut m = p.direct[0].pop_front().unwrap();
        let last = m.len() - 1;
        m[last] ^= 1;
        assert_eq!(p.b.receive_direct(&m, 0).unwrap(), vec![]);
        assert!(!p.b.direct_active());
        // b told a over the hub; a resends "x" there.
        p.send_all();
        while p.deliver(1, Via::Relay) {}
        p.send_all();
        while p.deliver(0, Via::Relay) {}
        assert_eq!(p.got[1], b"x");
    }

    #[test]
    fn a_session_without_paths_refuses_a_direct_path_and_speaks_plain_frames() {
        let (it, rt) = transports(1);
        let mut a = Session::new(1, it, Role::Initiator);
        let mut b = Session::new(1, rt, Role::Responder);
        let (d1, _) = transports(20);
        assert!(a.attach_direct(d1, 0).is_err());
        let sid = a.mux().open(StreamClass::Rpc, b"GET /").unwrap();
        a.mux().send(sid, b"hi").unwrap();
        let mut got = Vec::new();
        for f in a.flush(0).unwrap() {
            let Frame::Data { payload, .. } = f else { panic!() };
            got.extend(b.receive(&payload, 0).unwrap());
        }
        assert_eq!(got.len(), 2);
        assert_eq!(a.next_deadline_us(Room { relay: true, direct: true }), None);
    }
}
