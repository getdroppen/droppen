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
//! over it for [`DIRECT_TIMEOUT_MS`](crate::path::DIRECT_TIMEOUT_MS), what it
//! carries waits for an acknowledgement while the path makes no progress for
//! [`UNACKED_TIMEOUT_MS`](crate::path::UNACKED_TIMEOUT_MS) or ten round
//! trips — both stretched by what the path carries,
//! [`crate::path::direct_deadline`] — a message on it does not open, or the
//! peer says so with [`Envelope::Down`]), [`Session::detach_direct`] sends
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
    direct_deadline, next_rate, DcMessage, DirectDeadline, DirectLoad, DirectTimer, DownReason, Envelope, LaneStats, Lanes, PathError, Via,
    ASSUMED_BYTES_PER_MS, BULK_RECENT_MS, DIRECT_PROBE_MS, RATE_SAMPLE_BYTES, RELAY_PROBE_MS, UNACKED_LIMIT,
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
    /// The latest round trip a probe measured on the hub path; never a guess.
    pub relay_rtt_ms: Option<f64>,
    pub direct_rtt_ms: Option<f64>,
    /// How long the oldest hub probe still out had gone unanswered (or waited
    /// for credit to leave), as of the latest probe tick or answer (`None`
    /// with none out then): the hub path is at least that slow. Kept apart
    /// from [`PathStats::relay_rtt_ms`].
    ///
    /// This and [`PathStats::relay_quiet_ms`] mirror the page's
    /// `relayProbeAgeMs` / `relayQuietMs` (`web/src/lib/e2e/channel.ts`), so
    /// the two halves of the protocol keep the same stats the same way and
    /// the ported tests pin both. Only the page acts on them (it replaces a
    /// channel whose hub path is gone, `relayPathGone`, and not while a direct
    /// path carries the channel: that judgment also reads how long since
    /// anything came in either way, which only the page's driver keeps); the
    /// agent never probes the hub path, and a headless device only reports
    /// them, so nothing here mirrors the judgment.
    pub relay_probe_age_ms: Option<f64>,
    /// How long nothing had arrived on the hub path at that same moment
    /// (`None` with no probe out): a slow path keeps delivering, a gone one
    /// does not.
    pub relay_quiet_ms: Option<f64>,
    pub ups: u32,
    pub downs: u32,
    pub lanes: LaneStats,
}

/// The most probes outstanding at once (older ones are forgotten).
const MAX_PROBES: usize = 16;

/// A direct path given up on a deadline: which, after how long, and what the
/// deadline was made of ([`Session::last_direct_timeout`], for logs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DirectTimeout {
    pub timer: DirectTimer,
    /// How long the path had gone without (ms).
    pub elapsed_ms: f64,
    pub deadline: DirectDeadline,
    pub load: DirectLoad,
}

/// Milliseconds as whole microseconds.
fn us(ms: f64) -> u64 {
    (ms * 1000.0) as u64
}

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
    /// Hub probes queued and not sent yet, and since when (µs): the hub window
    /// is spent and the peer has handed no credit back. One that cannot leave
    /// says the hub path is as slow as one unanswered.
    unsent_relay_probes: HashMap<u64, u64>,
    next_nonce: u64,
    last_direct_rx_us: u64,
    /// When the direct path last showed it delivers: the peer's data arrived
    /// over it, or an acknowledgement freed frames sent over it. Probes do
    /// not count: they say the peer is there, not that what this side sent
    /// got through.
    last_direct_progress_us: u64,
    next_direct_probe_us: u64,
    next_relay_probe_us: u64,
    /// When anything last arrived for this channel over the hub: a `DATA`
    /// frame (data, an acknowledgement, a probe or its answer) or a `CREDIT`.
    last_relay_rx_us: u64,
    /// The slowest the channel's link has lately carried, on either path and
    /// either way (bytes/ms, [`next_rate`]): from acknowledgements freeing
    /// what this side sent on the direct path, and from the peer's bulk
    /// arriving back to back. What the direct path's deadlines time the bytes
    /// ahead at.
    bytes_per_ms: Option<f64>,
    /// Acknowledged bytes, and the time they took (µs), gathered toward the
    /// next sample while the direct path stays backlogged.
    acked_bytes: usize,
    acked_us: u64,
    /// When an acknowledgement last freed frames sent on the direct path
    /// (`None`: none since it was attached).
    last_acked_us: Option<u64>,
    /// When bulk (a message of [`RATE_SAMPLE_BYTES`] or more) last came over
    /// the hub path, and over the direct path.
    bulk_in_relay_us: Option<u64>,
    bulk_in_direct_us: Option<u64>,
    /// When this side last sealed bulk for the hub path.
    relay_bulk_out_us: Option<u64>,
    last_timeout: Option<DirectTimeout>,
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
                unsent_relay_probes: HashMap::new(),
                next_nonce: 1,
                last_direct_rx_us: now_us,
                last_direct_progress_us: now_us,
                next_direct_probe_us: now_us,
                // The first hub probe goes out at once, so the indicator has a number.
                next_relay_probe_us: now_us,
                last_relay_rx_us: now_us,
                bytes_per_ms: None,
                acked_bytes: 0,
                acked_us: 0,
                last_acked_us: None,
                bulk_in_relay_us: None,
                bulk_in_direct_us: None,
                relay_bulk_out_us: None,
                last_timeout: None,
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

    /// The latest time a deadline gave the direct path up: which, after how
    /// long, and what the deadline was made of.
    pub fn last_direct_timeout(&self) -> Option<DirectTimeout> {
        self.paths.as_ref().and_then(|p| p.last_timeout)
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
        p.attached();
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
                let plain = env.encode();
                p.sent_relay(plain.len(), now_us);
                let sealed = self.transport.seal(now_ms, &plain)?;
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
                let plain = env.encode();
                p.sent_relay(plain.len(), now_us);
                let sealed = self.transport.seal(now_ms, &plain)?;
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
        self.heard_relay(now_us);
        if let Some(p) = self.paths.as_mut() {
            p.heard_bulk(Via::Relay, payload.len(), now_us);
        }
        let env = Envelope::decode(&plaintext).map_err(PathError::from)?;
        self.on_envelope(env, Via::Relay, now_us)
    }

    /// Something for this channel came over the hub (a `DATA` frame, or a
    /// `CREDIT`): the hub path still carries, however late its probes are
    /// answered.
    pub fn heard_relay(&mut self, now_us: u64) {
        if let Some(p) = self.paths.as_mut() {
            p.last_relay_rx_us = p.last_relay_rx_us.max(now_us);
        }
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
        p.heard_bulk(Via::Direct, message.len(), now_us);
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
                    // What is still out is younger: say how old now.
                    if sent_via == Via::Relay {
                        p.age_relay_probe(now_us);
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
            let (load, silence, progress) = p.direct_deadlines();
            let quiet_us = now_us.saturating_sub(p.last_direct_rx_us);
            let silent = quiet_us > us(silence.limit_ms);
            let stuck = progress.filter(|(since, d)| now_us.saturating_sub(*since) > us(d.limit_ms));
            if silent || stuck.is_some() {
                let (timer, deadline, elapsed_us) = match stuck {
                    Some((since, d)) if !silent => (DirectTimer::Progress, d, now_us.saturating_sub(since)),
                    _ => (DirectTimer::Silence, silence, quiet_us),
                };
                let timeout = DirectTimeout {
                    timer,
                    elapsed_ms: elapsed_us as f64 / 1000.0,
                    deadline,
                    load,
                };
                #[cfg(feature = "runtime")]
                tracing::info!(
                    timer = timer.name(),
                    elapsed_ms = timeout.elapsed_ms,
                    limit_ms = deadline.limit_ms,
                    floor_ms = deadline.floor_ms,
                    load_ms = deadline.load_ms,
                    ahead_bytes = deadline.ahead_bytes,
                    bytes_per_ms = deadline.bytes_per_ms,
                    rate_measured = load.bytes_per_ms.is_some(),
                    unacked_bytes = load.unacked_bytes,
                    rtt_ms = ?load.rtt_ms,
                    relay_bulk_sent = load.relay_bulk_sent,
                    relay_bulk_received = load.relay_bulk_received,
                    direct_bulk_received = load.direct_bulk_received,
                    "a direct path missed its deadline"
                );
                p.last_timeout = Some(timeout);
                p.detach(DownReason::Timeout, true);
                p.events.push_back(PathEvent::DirectDown {
                    reason: DownReason::Timeout,
                    remote: false,
                    why: match timer {
                        DirectTimer::Silence => "nothing came over it within DIRECT_TIMEOUT_MS, stretched by what it carried",
                        DirectTimer::Progress => {
                            "what it carried went unacknowledged while it made no progress within UNACKED_TIMEOUT_MS, stretched by what it carried"
                        }
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
            p.age_relay_probe(now_us);
            let nonce = p.nonce();
            p.out_relay.push_back(Envelope::Probe { nonce });
            p.unsent_relay_probes.insert(nonce, now_us);
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
            let (_, silence, progress) = p.direct_deadlines();
            take(p.last_direct_rx_us + us(silence.limit_ms) + 1);
            if let Some((since, d)) = progress {
                take(since + us(d.limit_ms) + 1);
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

    /// What the direct path carries now, for its deadlines
    /// ([`direct_deadline`]).
    fn load(&self) -> DirectLoad {
        let heard = self.last_direct_rx_us.max(self.last_relay_rx_us);
        let recent = |at: Option<u64>| at.is_some_and(|at| heard.saturating_sub(at) <= BULK_RECENT_MS * 1000);
        // A whole hub window this side sent may still be on its way for as
        // long as the link takes to carry one.
        let window_ms = (BULK_RECENT_MS as f64).max(f64::from(crate::flow::INITIAL_CREDIT) / self.bytes_per_ms.unwrap_or(ASSUMED_BYTES_PER_MS));
        let oldest_us = self.lanes.oldest_unacked_ms().map(|ms| ms * 1000);
        DirectLoad {
            rtt_ms: self.stats.direct_rtt_ms,
            unacked_bytes: self.lanes.unacked_bytes(),
            bytes_per_ms: self.bytes_per_ms,
            relay_bulk_sent: matches!((oldest_us, self.relay_bulk_out_us), (Some(o), Some(b)) if o.saturating_sub(b) as f64 <= window_ms * 1000.0),
            relay_bulk_received: recent(self.bulk_in_relay_us),
            direct_bulk_received: recent(self.bulk_in_direct_us),
        }
    }

    /// The direct path's deadlines now: silence, counted from the last thing
    /// it brought; and, while frames sent on it wait for an acknowledgement,
    /// no progress, counted from (`since`, µs) the later of the oldest such
    /// frame leaving and the path's last progress. Counted from the frame
    /// alone, a backlog the link drains more slowly than that (the
    /// DataChannel's own buffer, or the peer's acknowledgement queued behind
    /// its own backlog) gave a healthy path up.
    fn direct_deadlines(&self) -> (DirectLoad, DirectDeadline, Option<(u64, DirectDeadline)>) {
        let load = self.load();
        let silence = direct_deadline(DirectTimer::Silence, &load);
        let progress = self.lanes.oldest_unacked_ms().map(|ms| {
            (
                (ms * 1000).max(self.last_direct_progress_us),
                direct_deadline(DirectTimer::Progress, &load),
            )
        });
        (load, silence, progress)
    }

    /// An acknowledgement came (on either path): progress on the direct path
    /// when it freed frames sent there — and, gathered while the path stays
    /// backlogged, a sample of how fast it carries what this side sends: the
    /// bytes freed over the time since the previous acknowledgement, or since
    /// the oldest of them left when the path had nothing waiting before.
    fn acked(&mut self, ack: u64, now_us: u64) -> Result<(), PathError> {
        let before = self.lanes.unacked_bytes();
        let oldest_us = self.lanes.oldest_unacked_ms().map(|ms| ms * 1000);
        if self.lanes.on_ack(ack)? == 0 {
            return Ok(());
        }
        self.last_direct_progress_us = now_us;
        let since = oldest_us.unwrap_or(now_us).max(self.last_acked_us.unwrap_or(0));
        self.last_acked_us = Some(now_us);
        self.acked_bytes += before - self.lanes.unacked_bytes();
        self.acked_us += now_us.saturating_sub(since);
        if self.acked_bytes >= RATE_SAMPLE_BYTES && self.acked_us > 0 {
            self.bytes_per_ms = Some(next_rate(self.bytes_per_ms, self.acked_bytes as f64 / (self.acked_us as f64 / 1000.0)));
            self.acked_bytes = 0;
            self.acked_us = 0;
        } else if self.lanes.unacked_bytes() == 0 {
            // Nothing waits any more: what comes next is not timed from this.
            self.acked_bytes = 0;
            self.acked_us = 0;
        }
        Ok(())
    }

    /// A message of `len` bytes came over `via`: bulk when it weighs
    /// [`RATE_SAMPLE_BYTES`] or more, and two bulk messages in a row on one
    /// path, within [`BULK_RECENT_MS`], time the link — the second took the
    /// time between them to come.
    fn heard_bulk(&mut self, via: Via, len: usize, now_us: u64) {
        if len < RATE_SAMPLE_BYTES {
            return;
        }
        let prev = match via {
            Via::Relay => self.bulk_in_relay_us,
            Via::Direct => self.bulk_in_direct_us,
        };
        if let Some(prev) = prev {
            let gap_us = now_us.saturating_sub(prev);
            if gap_us > 0 && gap_us <= BULK_RECENT_MS * 1000 {
                self.bytes_per_ms = Some(next_rate(self.bytes_per_ms, len as f64 / (gap_us as f64 / 1000.0)));
            }
        }
        match via {
            Via::Relay => self.bulk_in_relay_us = Some(now_us),
            Via::Direct => self.bulk_in_direct_us = Some(now_us),
        }
    }

    /// This side sealed `len` bytes for the hub path.
    fn sent_relay(&mut self, len: usize, now_us: u64) {
        if len >= RATE_SAMPLE_BYTES {
            self.relay_bulk_out_us = Some(now_us);
        }
    }

    /// A new direct path: what was timed on the one before about its
    /// acknowledgements and its bulk is not this one's.
    fn attached(&mut self) {
        self.acked_bytes = 0;
        self.acked_us = 0;
        self.last_acked_us = None;
        self.bulk_in_direct_us = None;
    }

    fn nonce(&mut self) -> u64 {
        let n = self.next_nonce;
        self.next_nonce += 1;
        n
    }

    /// A hub probe still unanswered (or still waiting for credit to leave)
    /// says the hub path is at least that slow: note how long the oldest one
    /// has been out, and how long nothing has arrived on the hub path, as of
    /// `now_us` (whole milliseconds). Kept apart from the round trip, which
    /// only a probe's answer sets: the page says an old probe as "slow", and
    /// the two together tell a slow path, which keeps delivering, from one
    /// that is gone.
    fn age_relay_probe(&mut self, now_us: u64) {
        let sent = self.probes.values().filter(|(via, _)| *via == Via::Relay).map(|(_, at)| *at);
        let oldest = sent.chain(self.unsent_relay_probes.values().copied()).min();
        let whole_ms = |us: u64| (us as f64 / 1000.0).round();
        self.stats.relay_probe_age_ms = oldest.map(|at| whole_ms(now_us.saturating_sub(at)));
        self.stats.relay_quiet_ms = oldest.map(|_| whole_ms(now_us.saturating_sub(self.last_relay_rx_us)));
    }

    /// Remember when a probe left (and on which path).
    fn note_probe(&mut self, env: &Envelope, via: Via, now_us: u64) {
        if let Envelope::Probe { nonce } = env {
            if via == Via::Relay {
                self.unsent_relay_probes.remove(nonce);
            }
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
    use crate::path::{DIRECT_TIMEOUT_MS, UNACKED_TIMEOUT_MS};
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

    /// A hub path that stops answering its probes says how long the oldest
    /// has been out, and how long nothing has arrived — kept apart from its
    /// last good round trip, which it never passes off as a new one.
    #[test]
    fn a_hub_path_that_stops_answering_its_probes_says_how_long() {
        let mut p = Pair::new();
        open_stream(&mut p);
        p.a.tick(p.now);
        p.send_all();
        p.now = 3_000;
        while p.deliver(0, Via::Relay) {}
        p.send_all();
        while p.deliver(1, Via::Relay) {}
        assert_eq!(p.a.stats().relay_rtt_ms, Some(3.0));
        assert_eq!(p.a.stats().relay_probe_age_ms, None, "nothing out");
        // The hub stops carrying anything: the probes queue up, unanswered.
        for at in [RELAY_PROBE_MS, 2 * RELAY_PROBE_MS, 3 * RELAY_PROBE_MS] {
            p.now = at * 1000;
            p.a.tick(p.now);
            p.send_all();
        }
        let s = p.a.stats();
        assert_eq!(s.relay_rtt_ms, Some(3.0), "the round trip measured, not the probe's age");
        assert_eq!(
            s.relay_probe_age_ms,
            Some((2 * RELAY_PROBE_MS) as f64),
            "the oldest unanswered probe's age"
        );
        assert_eq!(
            s.relay_quiet_ms,
            Some((3 * RELAY_PROBE_MS - 3) as f64),
            "nothing since the answer at 3 ms"
        );
        // When the hub carries again, the answers are round trips again.
        p.now = 4 * RELAY_PROBE_MS * 1000;
        p.a.tick(p.now);
        p.send_all();
        while p.deliver(0, Via::Relay) {}
        p.send_all();
        while p.deliver(1, Via::Relay) {}
        let s = p.a.stats();
        assert_eq!(s.relay_rtt_ms, Some(0.0), "measured afresh: the latest probe was answered at once");
        assert_eq!((s.relay_probe_age_ms, s.relay_quiet_ms), (None, None), "nothing out any more");
    }

    /// A hub probe that cannot leave (the hub window is spent, no credit comes
    /// back) is timed from when it was queued: on a path that died while this
    /// side was sending, it is the only probe out.
    #[test]
    fn a_hub_probe_that_cannot_leave_is_timed_from_when_it_was_queued() {
        let mut p = Pair::new();
        open_stream(&mut p);
        for at in [0, RELAY_PROBE_MS, 2 * RELAY_PROBE_MS] {
            p.now = at * 1000;
            p.a.tick(p.now);
        }
        assert_eq!(
            p.a.stats().relay_probe_age_ms,
            Some((2 * RELAY_PROBE_MS) as f64),
            "queued at 0, and still there"
        );
        // They leave; from then on they are timed from when they left.
        p.send_all();
        p.now = 3 * RELAY_PROBE_MS * 1000;
        p.a.tick(p.now);
        assert_eq!(p.a.stats().relay_probe_age_ms, Some(RELAY_PROBE_MS as f64));
    }

    /// A slow hub path — its probes' answers queued behind a download the link
    /// carries one frame a second — keeps delivering: its oldest probe grows
    /// old while the hub path is never quiet for long, and once the answers
    /// arrive their round trips are what it says.
    #[test]
    fn a_slow_hub_path_that_keeps_delivering_is_never_quiet() {
        let mut p = Pair::new();
        let sid = open_stream(&mut p);
        p.a.tick(p.now);
        p.send_all();
        p.now = 3_000;
        while p.deliver(0, Via::Relay) {}
        p.send_all();
        while p.deliver(1, Via::Relay) {}
        for _ in 0..40 {
            p.b.mux().send(sid, &[7; 1000]).unwrap();
        }
        p.send_all();
        let (mut oldest, mut loudest) = (0.0f64, 0.0f64);
        for t in 1..=30u64 {
            p.now = t * 1_000_000;
            p.a.tick(p.now);
            p.send_all();
            // a's probes reach b at once; b answers behind its download.
            while p.deliver(0, Via::Relay) {}
            p.send_all();
            let s = p.a.stats();
            oldest = oldest.max(s.relay_probe_age_ms.unwrap_or(0.0));
            loudest = loudest.max(s.relay_quiet_ms.unwrap_or(0.0));
            // One frame a second reaches a.
            p.deliver(1, Via::Relay);
        }
        assert!(oldest >= (2 * RELAY_PROBE_MS) as f64, "a probe went {oldest} ms unanswered");
        assert!(loudest <= 1_000.0, "yet something arrived every second: {loudest} ms of quiet at most");
        assert_eq!(p.a.stats().relay_rtt_ms, Some(3.0), "no round trip measured meanwhile");
        while !p.relay[1].is_empty() {
            p.now += 1_000_000;
            p.deliver(1, Via::Relay);
        }
        let s = p.a.stats();
        assert!(
            s.relay_rtt_ms.unwrap() >= (2 * RELAY_PROBE_MS) as f64,
            "the late answer's round trip: {s:?}"
        );
        assert_eq!(s.relay_probe_age_ms, None, "every probe answered");
        assert_eq!(p.got[0].len(), 40 * 1000, "all of it arrived");
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

    /// The direct path's deadlines under load, over an access link both paths
    /// share, on a clock the test moves: the ports of the page's
    /// `web/test/direct-load.test.ts` (a field report: a 1 Mbit/s link, the
    /// direct path's round trip measured idle at 41–82 ms, given up after
    /// 1.5 s "without progress" again and again while it was only full).
    mod load {
        use super::*;
        use crate::flow::INITIAL_CREDIT;
        use crate::frame::MAX_PAYLOAD_LEN;
        use crate::path::DIRECT_TIMEOUT_MAX_MS;

        /// How far the clock moves at a time (µs).
        const STEP_US: u64 = 10_000;
        /// 1 Mbit/s, in bytes a millisecond.
        const MBIT: f64 = 125.0;
        /// What SCTP, DTLS, UDP and IP (or the WebSocket) add to a message.
        const OVERHEAD: usize = 60;
        /// What a DataChannel's owner takes before it holds new frames back (`driver::DIRECT_BUDGET`).
        const BUDGET: usize = 1024 * 1024;
        /// Three answers, so that a stream's window is not what holds a transfer back.
        const STREAMS: usize = 3;

        enum Msg {
            Relay(Vec<u8>),
            Direct(Vec<u8>),
        }

        impl Msg {
            fn via(&self) -> Via {
                match self {
                    Msg::Relay(_) => Via::Relay,
                    Msg::Direct(_) => Via::Direct,
                }
            }
        }

        /// One direction of the access link both paths share: one FIFO,
        /// `bytes_per_ms` through it (nothing while `stalled`), then
        /// `delay_us` on the way; a message arrives whole once its last byte
        /// is through. Traffic the channel does not carry is what it takes of
        /// `bytes_per_ms`.
        struct Lane {
            bytes_per_ms: f64,
            delay_us: u64,
            stalled: bool,
            queue: VecDeque<(f64, Msg)>,
            flying: VecDeque<(u64, Msg)>,
            cut_direct: bool,
        }

        impl Lane {
            fn new(bytes_per_ms: f64) -> Self {
                Self {
                    bytes_per_ms,
                    delay_us: 25_000,
                    stalled: false,
                    queue: VecDeque::new(),
                    flying: VecDeque::new(),
                    cut_direct: false,
                }
            }

            fn put(&mut self, msg: Msg) {
                if self.cut_direct && msg.via() == Via::Direct {
                    return;
                }
                let len = match &msg {
                    Msg::Relay(m) | Msg::Direct(m) => m.len(),
                };
                self.queue.push_back(((len + OVERHEAD) as f64, msg));
            }

            fn step(&mut self, now_us: u64) {
                if self.stalled {
                    return;
                }
                let mut budget = self.bytes_per_ms * (STEP_US / 1000) as f64;
                while budget > 0.0 {
                    let Some((left, _)) = self.queue.front_mut() else { break };
                    let take = budget.min(*left);
                    *left -= take;
                    budget -= take;
                    if *left > 0.0 {
                        break;
                    }
                    let (_, msg) = self.queue.pop_front().expect("front");
                    self.flying.push_back((now_us + self.delay_us, msg));
                }
            }

            fn due(&mut self, now_us: u64) -> Vec<Msg> {
                let mut out = Vec::new();
                while self.flying.front().is_some_and(|(at, _)| *at <= now_us) {
                    out.push(self.flying.pop_front().expect("front").1);
                }
                out
            }

            /// A DataChannel's buffered amount: what of the direct path's is not through yet.
            fn buffered_direct(&self) -> usize {
                self.queue
                    .iter()
                    .filter_map(|(_, m)| match m {
                        Msg::Direct(m) => Some(m.len()),
                        Msg::Relay(_) => None,
                    })
                    .sum()
            }

            /// The direct path is gone this way, with what it had on the link.
            fn kill_direct(&mut self) {
                self.cut_direct = true;
                self.queue.retain(|(_, m)| m.via() == Via::Relay);
                self.flying.retain(|(_, m)| m.via() == Via::Relay);
            }
        }

        /// A direct path given up, by which side, when, and how (`why`).
        struct Down {
            side: usize,
            at_us: u64,
            remote: bool,
            why: &'static str,
        }

        /// A device (`a`) and an agent (`b`) on one channel, over `down`
        /// (agent → device) and `up`: each side's hub window is
        /// `INITIAL_CREDIT`, its DataChannel takes new frames under `BUDGET`,
        /// and both read every stream as data comes.
        struct Sim {
            now: u64,
            a: Session,
            b: Session,
            down: Lane,
            up: Lane,
            got: [usize; 2],
            downs: Vec<Down>,
            relay_out: [usize; 2],
        }

        impl Sim {
            fn new(bytes_per_ms: f64) -> Self {
                let (it, rt) = transports(1);
                Self {
                    now: 0,
                    a: Session::with_paths(1, it, Role::Initiator, PathOptions { probe_relay: true }, 0),
                    b: Session::with_paths(1, rt, Role::Responder, PathOptions::default(), 0),
                    down: Lane::new(bytes_per_ms),
                    up: Lane::new(bytes_per_ms),
                    got: [0, 0],
                    downs: Vec::new(),
                    relay_out: [0, 0],
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

            fn take(&mut self, i: usize, events: Vec<MuxEvent>) {
                for e in events {
                    if let MuxEvent::Data { sid, data } = e {
                        self.got[i] += data.len();
                        self.side(i).mux().consumed(sid, data.len());
                    }
                }
            }

            fn pump(&mut self) {
                for i in 0..2 {
                    loop {
                        let lane = if i == 0 { &self.up } else { &self.down };
                        let room = Room {
                            relay: self.relay_out[i] + MAX_PAYLOAD_LEN <= INITIAL_CREDIT as usize,
                            direct: lane.buffered_direct() < BUDGET,
                        };
                        let now = self.now;
                        let Some((wire, _)) = self.side(i).poll(now, room).unwrap() else { break };
                        let msg = match wire {
                            Wire::Relay(Frame::Data { payload, .. }) => {
                                self.relay_out[i] += payload.len();
                                Msg::Relay(payload)
                            }
                            Wire::Relay(_) => continue,
                            Wire::Direct(m) => Msg::Direct(m),
                        };
                        if i == 0 {
                            self.up.put(msg);
                        } else {
                            self.down.put(msg);
                        }
                    }
                }
            }

            /// Move the clock by `for_ms`, `each` run every step before the
            /// sides' timers: the link carries, both sides read, tick and send.
            fn run(&mut self, for_ms: u64, mut each: impl FnMut(&mut Sim)) {
                let until = self.now + for_ms * 1000;
                while self.now < until {
                    self.now += STEP_US;
                    let now = self.now;
                    self.down.step(now);
                    self.up.step(now);
                    for (to, msg) in self
                        .down
                        .due(now)
                        .into_iter()
                        .map(|m| (0, m))
                        .chain(self.up.due(now).into_iter().map(|m| (1, m)))
                    {
                        let events = match msg {
                            Msg::Relay(payload) => {
                                self.relay_out[1 - to] -= payload.len();
                                self.side(to).receive_relay(&payload, now).unwrap()
                            }
                            Msg::Direct(m) => self.side(to).receive_direct(&m, now).unwrap(),
                        };
                        self.take(to, events);
                    }
                    each(self);
                    for i in 0..2 {
                        self.side(i).tick(now);
                        for e in self.side(i).take_events() {
                            if let PathEvent::DirectDown { remote, why, .. } = e {
                                self.downs.push(Down {
                                    side: i,
                                    at_us: now,
                                    remote,
                                    why,
                                });
                            }
                        }
                    }
                    self.pump();
                }
            }

            fn first_down(&self, side: usize) -> &Down {
                self.downs.iter().find(|d| d.side == side).expect("a side let the direct path go")
            }

            fn kept(&self, what: &str) {
                assert!(self.downs.is_empty(), "{what}: nobody gave the direct path up");
                for s in [&self.a, &self.b] {
                    let st = s.stats();
                    assert!(st.direct, "{what}: still direct");
                    assert_eq!((st.downs, st.lanes.resent), (0, 0), "{what}: nothing given up, nothing sent twice");
                }
            }
        }

        /// Attached, idle for three seconds: the direct round trip measured on an idle path.
        fn idle_direct() -> Sim {
            let mut s = Sim::new(MBIT);
            s.attach();
            s.run(3_000, |_| {});
            let rtt = s.a.stats().direct_rtt_ms.expect("measured");
            assert!(rtt < 100.0, "measured idle: {rtt} ms");
            s
        }

        /// The agent answers three streams the device opened with `bytes` in
        /// all; the device opens a terminal to type into (the sid returned).
        fn transfer(s: &mut Sim, bytes: usize) -> u32 {
            let feeds: Vec<u32> = (0..STREAMS)
                .map(|k| s.a.mux().open(StreamClass::Rpc, format!("GET /h/{k}").as_bytes()).unwrap())
                .collect();
            let term = s.a.mux().open(StreamClass::Term, b"t-1").unwrap();
            s.pump();
            s.run(200, |_| {});
            for sid in feeds {
                s.b.mux().send(sid, &vec![7u8; bytes / STREAMS]).unwrap();
            }
            term
        }

        /// A keystroke every 500 ms into `term`.
        fn typing(term: u32) -> impl FnMut(&mut Sim) {
            move |s: &mut Sim| {
                if s.now.is_multiple_of(500_000) {
                    s.a.mux().send(term, b"k").unwrap();
                }
            }
        }

        const BYTES: usize = 600 * 1024;

        #[test]
        fn six_hundred_kb_on_a_shared_one_megabit_path_is_kept_though_acknowledgements_come_seconds_apart() {
            let mut s = idle_direct();
            let term = transfer(&mut s, BYTES);
            // Other traffic takes two thirds of the downlink: each 64 KiB
            // message takes about 1.6 s, longer than UNACKED_TIMEOUT_MS.
            s.down.bytes_per_ms = MBIT / 3.0;
            let (mut gap_us, mut last_got, mut last_at) = (0u64, 0usize, s.now);
            let mut keys = typing(term);
            s.run(25_000, |s| {
                keys(s);
                if s.got[0] != last_got {
                    gap_us = gap_us.max(s.now - last_at);
                    last_got = s.got[0];
                    last_at = s.now;
                }
            });
            s.kept("shared");
            assert_eq!(s.got[0], BYTES, "all of it, over the direct path");
            assert!(gap_us > UNACKED_TIMEOUT_MS * 1000, "data came as much as {gap_us} µs apart");
        }

        #[test]
        fn six_hundred_kb_on_a_one_megabit_path_that_stalls_for_two_seconds_is_kept() {
            let mut s = idle_direct();
            let term = transfer(&mut s, BYTES);
            let stall_at = s.now + 1_500_000;
            let mut keys = typing(term);
            s.run(15_000, |s| {
                s.down.stalled = s.now >= stall_at && s.now < stall_at + 2_000_000;
                keys(s);
            });
            s.kept("stalled");
            assert_eq!(s.got[0], BYTES);
        }

        #[test]
        fn right_after_a_switch_the_direct_paths_frames_wait_behind_the_hub_backlog_and_the_path_is_kept() {
            let mut s = Sim::new(MBIT);
            let feeds: Vec<u32> = (0..STREAMS)
                .map(|k| s.a.mux().open(StreamClass::Rpc, format!("GET /h/{k}").as_bytes()).unwrap())
                .collect();
            let term = s.a.mux().open(StreamClass::Term, b"t-1").unwrap();
            s.pump();
            s.run(500, |_| {});
            // The agent's answers go over the hub path until the direct path
            // comes up: a whole hub window of them on the link.
            for sid in feeds {
                s.b.mux().send(sid, &vec![7u8; BYTES / STREAMS]).unwrap();
            }
            s.pump();
            s.run(100, |_| {});
            s.attach();
            s.run(15_000, typing(term));
            s.kept("switch");
            assert_eq!(s.got[0], BYTES);
        }

        #[test]
        fn a_path_that_carries_nothing_much_is_given_up_on_the_floors() {
            // Idle: only probes.
            let mut s = idle_direct();
            s.down.kill_direct();
            s.up.kill_direct();
            let died = s.now;
            s.run(6_000, |_| {});
            for side in 0..2 {
                let d = s.first_down(side);
                assert!(!d.remote && d.why.contains("nothing came"), "{side}: {}", d.why);
                assert!(
                    d.at_us - died <= (DIRECT_TIMEOUT_MS + 1_000) * 1000 + 2 * STEP_US,
                    "{side} after {} µs",
                    d.at_us - died
                );
            }
            let t = s.a.last_direct_timeout().unwrap();
            assert_eq!(
                (t.timer, t.deadline.limit_ms),
                (DirectTimer::Silence, DIRECT_TIMEOUT_MS as f64),
                "nothing carried, nothing added"
            );
            // A keystroke on an idle path.
            let mut s = idle_direct();
            s.down.kill_direct();
            s.up.kill_direct();
            let term = s.a.mux().open(StreamClass::Term, b"t-1").unwrap();
            s.a.mux().send(term, b"k").unwrap();
            let sent = s.now;
            s.run(3_000, |_| {});
            let d = s.first_down(0);
            assert!(d.why.contains("unacknowledged"), "{}", d.why);
            assert!(d.at_us - sent <= UNACKED_TIMEOUT_MS * 1000 + 2 * STEP_US, "after {} µs", d.at_us - sent);
            let t = s.a.last_direct_timeout().unwrap();
            assert!(
                t.deadline.limit_ms < UNACKED_TIMEOUT_MS as f64 + 10.0,
                "a keystroke adds next to nothing: {t:?}"
            );
        }

        #[test]
        fn a_path_that_dies_under_load_is_given_up_within_the_cap_and_what_it_did_not_deliver_comes_over_the_hub() {
            let mut s = idle_direct();
            let term = transfer(&mut s, BYTES);
            s.run(2_000, typing(term));
            assert!(s.got[0] > 0 && s.got[0] < BYTES, "mid-transfer");
            s.down.kill_direct();
            s.up.kill_direct();
            let died = s.now;
            s.run(30_000, typing(term));
            // The agent, sending, knows soonest: the device's acknowledgements and probes stopped.
            let agent = s.first_down(1);
            assert!(!agent.remote && agent.why.contains("nothing came"), "{}", agent.why);
            assert!(
                agent.at_us - died <= (DIRECT_TIMEOUT_MS + 1_000) * 1000 + 2 * STEP_US,
                "the agent after {} µs",
                agent.at_us - died
            );
            let device = s.first_down(0);
            assert!(
                device.at_us - died <= DIRECT_TIMEOUT_MAX_MS * 1000 + 2 * STEP_US,
                "the device after {} µs",
                device.at_us - died
            );
            assert_eq!(s.got[0], BYTES, "all of it, once");
            assert!(!s.a.direct_active() && !s.b.direct_active());
        }

        #[test]
        fn a_downlink_that_stalls_for_good_under_load_is_given_up_within_the_cap_by_both_sides() {
            let mut s = idle_direct();
            let term = transfer(&mut s, BYTES);
            s.run(2_000, typing(term));
            assert!(s.got[0] > 0 && s.got[0] < BYTES, "mid-transfer");
            s.down.stalled = true;
            let stuck = s.now;
            s.run(DIRECT_TIMEOUT_MAX_MS + 1_000, typing(term));
            let device = s.first_down(0);
            assert!(!device.remote && device.why.contains("nothing came"), "{}", device.why);
            for side in 0..2 {
                let d = s.first_down(side);
                assert!(
                    d.at_us - stuck <= DIRECT_TIMEOUT_MAX_MS * 1000 + 2 * STEP_US,
                    "{side} after {} µs",
                    d.at_us - stuck
                );
            }
            s.down.stalled = false;
            s.run(30_000, typing(term));
            assert_eq!(s.got[0], BYTES, "all of it, once, in the end");
        }
    }
}
