//! The direct path: a WebRTC DataChannel between a device and its machine,
//! next to the hub, carrying the same channel faster when the two can reach
//! each other (architecture doc §9, "直连通道").
//!
//! Everything here is sans-IO. What it defines:
//!
//! - [`Signal`] — offer, answer, ICE candidates and goodbyes. They travel as
//!   binary messages on a sub-stream of the *existing* end-to-end channel
//!   (`mux` class, path [`SIGNAL_PATH`]), so the hub relays them as ciphertext
//!   and never learns either side's addresses or certificates. There is no
//!   signaling room on the hub.
//! - **Fingerprint binding.** Each side states its own DTLS certificate
//!   fingerprint next to its SDP ([`Signal::Offer::fp`], [`Signal::Answer::fp`]),
//!   and the receiver refuses an SDP whose `a=fingerprint` is anything else
//!   ([`check_fingerprint`]). The statement is authenticated end to end by the
//!   channel it rides in, and the browser's and str0m's DTLS stacks refuse a
//!   certificate that does not hash to the SDP's fingerprint — so a hub, a STUN
//!   provider or anyone on the network cannot sit between the two DTLS ends.
//! - **Noise on the DataChannel** ([`DirectBinding`]). DTLS is only the
//!   transport. On the DataChannel the device runs a *fresh* Noise IK
//!   handshake with the agent, with the same static keys as the hub channel and
//!   a prologue that binds the account, device and machine, the hub channel's
//!   handshake hash, a nonce the agent chose for this attempt, and both DTLS
//!   fingerprints. The agent admits it exactly as it admits a hub channel
//!   ([`crate::handshake::accept_direct`]: chain, freshness, replay guard), so
//!   a removed or stale device cannot get a direct path any more than a hub
//!   one, and the direct path is as strong as the hub path whatever the
//!   WebRTC stack does. The direct transport has its own keys and nonces; no
//!   key is ever used on both paths.
//! - [`Envelope`] and [`Lanes`] — sequenced, acknowledged frames, so the
//!   channel's sub-stream multiplexer can move between the hub and the direct
//!   path with nothing lost, duplicated or reordered. With the
//!   [`CLIENT_FEATURE`] negotiated in the handshake, every multiplexer frame
//!   (on either path) is a [`Envelope::Data`] with a per-direction sequence
//!   number and a cumulative ack. Frames sent on the direct path are kept until
//!   acknowledged; when the direct path fails they are sent again over the hub,
//!   and the receiver drops what it already has. Frames that cross from the
//!   hub to the faster direct path are held until the gap fills.
//! - [`DcMessage`] — what one DataChannel message is: the handshake's two
//!   messages, then sealed envelopes.

use std::collections::{BTreeMap, VecDeque};

use thiserror::Error;

use crate::codec::{put_str16, DecodeError, Reader};
use crate::noise::{Binding, ChannelError};

/// What a device puts in its hello's client info, and an agent in its
/// features, when it speaks this module: sequenced frames and a direct path.
pub const CLIENT_FEATURE: &str = "paths/1";
/// The agent-local route of the signaling sub-stream (`mux` class).
pub const SIGNAL_PATH: &str = "/rh/direct";
/// The DataChannel's label.
pub const DC_LABEL: &str = "rh";

/// `kind u8 + seq u64 + ack u64`: what [`Envelope::Data`] adds to a frame.
pub const ENVELOPE_HEADER: usize = 1 + 8 + 8;
/// The most bytes a receiver holds while it waits for a gap to fill (frames
/// that came the fast way ahead of ones still on the hub).
pub const MAX_HELD_BYTES: usize = 16 * 1024 * 1024;
/// How far ahead of the next expected frame a sequence number may be.
pub const MAX_AHEAD: u64 = 1 << 20;
/// A sender stops putting new frames on the direct path while this much it
/// sent there is unacknowledged (it can still be resent over the hub).
pub const UNACKED_LIMIT: usize = 4 * 1024 * 1024;
/// A frame received on the direct path is acknowledged within this long…
pub const ACK_DELAY_MS: u64 = 15;
/// …or after this many, whichever is first.
pub const ACK_EVERY: u32 = 16;
/// The direct path is probed this often (also its keep-alive)…
pub const DIRECT_PROBE_MS: u64 = 1_000;
/// …and declared dead when nothing has come over it for this long.
pub const DIRECT_TIMEOUT_MS: u64 = 3_500;
/// While frames sent on the direct path wait for an acknowledgement, it is
/// declared dead once it has made no progress for this long (or ten round
/// trips, whichever is longer) — sooner than [`DIRECT_TIMEOUT_MS`] of silence,
/// so a typist waits less for a path that went dark mid-typing. Progress is
/// data from the peer arriving over it, or an acknowledgement freeing what was
/// sent over it — not the mere age of the oldest frame: a path slowly draining
/// a backlog (a DataChannel buffer filled faster than a cellular link empties
/// it, with the peer's acknowledgements queued behind its own) is not dead.
pub const UNACKED_TIMEOUT_MS: u64 = 1_500;
/// The hub path is probed this often for its round-trip time (for display).
pub const RELAY_PROBE_MS: u64 = 5_000;

const ENV_DATA: u8 = 0x01;
const ENV_ACK: u8 = 0x02;
const ENV_PROBE: u8 = 0x03;
const ENV_PROBE_ACK: u8 = 0x04;
const ENV_DOWN: u8 = 0x05;

const DC_HELLO: u8 = 0x01;
const DC_REPLY: u8 = 0x02;
const DC_SEALED: u8 = 0x03;

const SIG_OFFER: u8 = 0x01;
const SIG_ANSWER: u8 = 0x02;
const SIG_CANDIDATE: u8 = 0x03;
const SIG_BYE: u8 = 0x04;

/// The longest SDP a signal carries (one DataChannel's is about 1 KiB).
pub const MAX_SDP: usize = 16 * 1024;
/// The longest ICE candidate line.
pub const MAX_CANDIDATE: usize = 1024;
/// The longest goodbye detail.
pub const MAX_DETAIL: usize = 256;

/// The prologue's leading label for a direct-path handshake.
pub const DIRECT_PROLOGUE_LABEL: &[u8] = b"rh/v1 direct\0";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PathError {
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error("path protocol violation: {0}")]
    Protocol(&'static str),
    #[error("the DTLS fingerprint does not match: {0}")]
    Fingerprint(&'static str),
    #[error("more than {MAX_HELD_BYTES} bytes held waiting for a gap")]
    Overflow,
}

/// Which way a message went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Via {
    Relay,
    Direct,
}

/// Why a side stopped using the direct path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DownReason {
    /// Closed on purpose (the device turned direct paths off, the page left).
    Closed = 0,
    /// Nothing arrived over it for [`DIRECT_TIMEOUT_MS`].
    Timeout = 1,
    /// The DataChannel or the peer connection failed, or a message on it did not open.
    Failed = 2,
}

impl DownReason {
    fn from_u8(v: u8) -> Result<Self, DecodeError> {
        Ok(match v {
            0 => Self::Closed,
            1 => Self::Timeout,
            2 => Self::Failed,
            other => {
                return Err(DecodeError::Unknown {
                    what: "down reason",
                    value: other,
                })
            }
        })
    }
}

/// One transport plaintext on a channel that negotiated [`CLIENT_FEATURE`].
///
/// | kind | body | path |
/// |---|---|---|
/// | `DATA` 1 | `seq u64, ack u64, mux frame…` | either |
/// | `ACK` 2 | `ack u64` | either |
/// | `PROBE` 3 | `nonce u64` | either; answered on the same path |
/// | `PROBE_ACK` 4 | `nonce u64` | the path the probe came on |
/// | `DOWN` 5 | `reason u8` | hub only: "I stopped using the direct path" |
///
/// `seq` counts `DATA` envelopes per direction from 1; `ack` is the highest
/// `seq` the sender has received with nothing missing before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Envelope {
    Data { seq: u64, ack: u64, frame: Vec<u8> },
    Ack { ack: u64 },
    Probe { nonce: u64 },
    ProbeAck { nonce: u64 },
    Down { reason: DownReason },
}

impl Envelope {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(match self {
            Envelope::Data { frame, .. } => ENVELOPE_HEADER + frame.len(),
            _ => 9,
        });
        match self {
            Envelope::Data { seq, ack, frame } => {
                out.push(ENV_DATA);
                out.extend_from_slice(&seq.to_be_bytes());
                out.extend_from_slice(&ack.to_be_bytes());
                out.extend_from_slice(frame);
            }
            Envelope::Ack { ack } => {
                out.push(ENV_ACK);
                out.extend_from_slice(&ack.to_be_bytes());
            }
            Envelope::Probe { nonce } => {
                out.push(ENV_PROBE);
                out.extend_from_slice(&nonce.to_be_bytes());
            }
            Envelope::ProbeAck { nonce } => {
                out.push(ENV_PROBE_ACK);
                out.extend_from_slice(&nonce.to_be_bytes());
            }
            Envelope::Down { reason } => {
                out.push(ENV_DOWN);
                out.push(*reason as u8);
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let env = match r.u8()? {
            ENV_DATA => {
                let seq = r.u64()?;
                let ack = r.u64()?;
                if seq == 0 {
                    return Err(DecodeError::Invalid("a data envelope with seq 0"));
                }
                Envelope::Data {
                    seq,
                    ack,
                    frame: r.rest().to_vec(),
                }
            }
            ENV_ACK => Envelope::Ack { ack: r.u64()? },
            ENV_PROBE => Envelope::Probe { nonce: r.u64()? },
            ENV_PROBE_ACK => Envelope::ProbeAck { nonce: r.u64()? },
            ENV_DOWN => Envelope::Down {
                reason: DownReason::from_u8(r.u8()?)?,
            },
            other => {
                return Err(DecodeError::Unknown {
                    what: "envelope",
                    value: other,
                })
            }
        };
        r.finish()?;
        Ok(env)
    }
}

/// One DataChannel message: the direct handshake's two messages, then sealed
/// envelopes (Noise transport messages of the direct session).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DcMessage {
    Hello(Vec<u8>),
    Reply(Vec<u8>),
    Sealed(Vec<u8>),
}

impl DcMessage {
    pub fn encode(&self) -> Vec<u8> {
        let (kind, body) = match self {
            DcMessage::Hello(b) => (DC_HELLO, b),
            DcMessage::Reply(b) => (DC_REPLY, b),
            DcMessage::Sealed(b) => (DC_SEALED, b),
        };
        let mut out = Vec::with_capacity(1 + body.len());
        out.push(kind);
        out.extend_from_slice(body);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let (&kind, body) = bytes.split_first().ok_or(DecodeError::Truncated)?;
        if body.len() > crate::frame::MAX_PAYLOAD_LEN {
            return Err(DecodeError::TooLarge {
                what: "direct message",
                len: body.len(),
                max: crate::frame::MAX_PAYLOAD_LEN,
            });
        }
        Ok(match kind {
            DC_HELLO => DcMessage::Hello(body.to_vec()),
            DC_REPLY => DcMessage::Reply(body.to_vec()),
            DC_SEALED => DcMessage::Sealed(body.to_vec()),
            other => {
                return Err(DecodeError::Unknown {
                    what: "direct message",
                    value: other,
                })
            }
        })
    }
}

/// Why a side ended a direct attempt, in a [`Signal::Bye`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ByeReason {
    /// Finished with it (the page left, the path was replaced).
    Done = 0,
    /// Direct paths are turned off on this side.
    Disabled = 1,
    /// Too many direct paths at once; try later.
    Busy = 2,
    /// ICE, DTLS or the DataChannel did not come up, or went down.
    Failed = 3,
    /// An SDP's fingerprint is not the one stated next to it.
    BadFingerprint = 4,
    /// The direct handshake was not admitted (removed device, stale view…).
    Refused = 5,
}

impl ByeReason {
    fn from_u8(v: u8) -> Result<Self, DecodeError> {
        Ok(match v {
            0 => Self::Done,
            1 => Self::Disabled,
            2 => Self::Busy,
            3 => Self::Failed,
            4 => Self::BadFingerprint,
            5 => Self::Refused,
            other => {
                return Err(DecodeError::Unknown {
                    what: "bye reason",
                    value: other,
                })
            }
        })
    }
}

/// A signaling message, on the channel's [`SIGNAL_PATH`] sub-stream. `id`
/// names one attempt (the device counts them per channel).
///
/// | kind | body |
/// |---|---|
/// | `OFFER` 1 | `id u32, fp [32], str16 sdp` |
/// | `ANSWER` 2 | `id u32, fp [32], nonce [32], str16 sdp` |
/// | `CANDIDATE` 3 | `id u32, str16 candidate` (empty: no more candidates) |
/// | `BYE` 4 | `id u32, reason u8, str16 detail` |
///
/// `fp` is the sender's own DTLS certificate's SHA-256; `nonce` is the agent's
/// per-attempt value for the direct handshake's prologue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    Offer { id: u32, fp: [u8; 32], sdp: String },
    Answer { id: u32, fp: [u8; 32], nonce: [u8; 32], sdp: String },
    Candidate { id: u32, candidate: Option<String> },
    Bye { id: u32, reason: ByeReason, detail: String },
}

impl Signal {
    pub fn id(&self) -> u32 {
        match self {
            Signal::Offer { id, .. } | Signal::Answer { id, .. } | Signal::Candidate { id, .. } | Signal::Bye { id, .. } => *id,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, DecodeError> {
        let too_large = |what, len, max| DecodeError::TooLarge { what, len, max };
        let mut out = Vec::new();
        match self {
            Signal::Offer { id, fp, sdp } => {
                if sdp.len() > MAX_SDP {
                    return Err(too_large("sdp", sdp.len(), MAX_SDP));
                }
                out.push(SIG_OFFER);
                out.extend_from_slice(&id.to_be_bytes());
                out.extend_from_slice(fp);
                put_str16(&mut out, sdp);
            }
            Signal::Answer { id, fp, nonce, sdp } => {
                if sdp.len() > MAX_SDP {
                    return Err(too_large("sdp", sdp.len(), MAX_SDP));
                }
                out.push(SIG_ANSWER);
                out.extend_from_slice(&id.to_be_bytes());
                out.extend_from_slice(fp);
                out.extend_from_slice(nonce);
                put_str16(&mut out, sdp);
            }
            Signal::Candidate { id, candidate } => {
                let c = candidate.as_deref().unwrap_or("");
                if c.len() > MAX_CANDIDATE {
                    return Err(too_large("candidate", c.len(), MAX_CANDIDATE));
                }
                out.push(SIG_CANDIDATE);
                out.extend_from_slice(&id.to_be_bytes());
                put_str16(&mut out, c);
            }
            Signal::Bye { id, reason, detail } => {
                if detail.len() > MAX_DETAIL {
                    return Err(too_large("detail", detail.len(), MAX_DETAIL));
                }
                out.push(SIG_BYE);
                out.extend_from_slice(&id.to_be_bytes());
                out.push(*reason as u8);
                put_str16(&mut out, detail);
            }
        }
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let signal = match r.u8()? {
            SIG_OFFER => Signal::Offer {
                id: r.u32()?,
                fp: r.array()?,
                sdp: r.str16("sdp", MAX_SDP)?,
            },
            SIG_ANSWER => Signal::Answer {
                id: r.u32()?,
                fp: r.array()?,
                nonce: r.array()?,
                sdp: r.str16("sdp", MAX_SDP)?,
            },
            SIG_CANDIDATE => {
                let id = r.u32()?;
                let c = r.str16("candidate", MAX_CANDIDATE)?;
                Signal::Candidate {
                    id,
                    candidate: (!c.is_empty()).then_some(c),
                }
            }
            SIG_BYE => Signal::Bye {
                id: r.u32()?,
                reason: ByeReason::from_u8(r.u8()?)?,
                detail: r.str16("detail", MAX_DETAIL)?,
            },
            other => {
                return Err(DecodeError::Unknown {
                    what: "signal",
                    value: other,
                })
            }
        };
        r.finish()?;
        Ok(signal)
    }
}

/// The `sha-256` fingerprint an SDP names (`a=fingerprint:sha-256 AB:CD:…`),
/// which must be the only one it names.
pub fn sdp_fingerprint(sdp: &str) -> Result<[u8; 32], PathError> {
    let mut found: Option<[u8; 32]> = None;
    for line in sdp.lines() {
        let Some(value) = line.trim_end_matches('\r').strip_prefix("a=fingerprint:") else {
            continue;
        };
        let (func, hexes) = value.trim().split_once(' ').ok_or(PathError::Fingerprint("malformed a=fingerprint"))?;
        if !func.eq_ignore_ascii_case("sha-256") {
            return Err(PathError::Fingerprint("only sha-256 fingerprints are accepted"));
        }
        let fp = parse_fingerprint_hex(hexes.trim()).ok_or(PathError::Fingerprint("malformed sha-256 fingerprint"))?;
        match found {
            Some(prev) if prev != fp => return Err(PathError::Fingerprint("an SDP with two different fingerprints")),
            _ => found = Some(fp),
        }
    }
    found.ok_or(PathError::Fingerprint("an SDP without a fingerprint"))
}

/// `AB:CD:…` (32 colon-separated bytes) as bytes.
fn parse_fingerprint_hex(text: &str) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    let mut n = 0;
    for part in text.split(':') {
        if part.len() != 2 || n == 32 || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        out[n] = u8::from_str_radix(part, 16).ok()?;
        n += 1;
    }
    (n == 32).then_some(out)
}

/// `AB:CD:…`, the SDP spelling.
pub fn fingerprint_hex(fp: &[u8; 32]) -> String {
    fp.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// Refuse `sdp` unless its fingerprint is exactly the `stated` one.
pub fn check_fingerprint(sdp: &str, stated: &[u8; 32]) -> Result<(), PathError> {
    if sdp_fingerprint(sdp)? == *stated {
        Ok(())
    } else {
        Err(PathError::Fingerprint("the SDP names a different certificate than the one stated"))
    }
}

/// What a direct-path handshake is bound to (its Noise prologue).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectBinding {
    /// The same `account | device | machine` as the hub channel.
    pub binding: Binding,
    /// The hub channel's Noise handshake hash: the direct path belongs to that
    /// channel and no other.
    pub relay_hash: [u8; 32],
    /// The agent's nonce for this attempt (from its [`Signal::Answer`]).
    pub nonce: [u8; 32],
    /// The device's DTLS certificate fingerprint, as its offer stated it.
    pub device_fp: [u8; 32],
    /// The agent's DTLS certificate fingerprint, as its answer stated it.
    pub agent_fp: [u8; 32],
}

impl DirectBinding {
    /// `"rh/v1 direct\0" | str16 account | str16 device | str16 machine |
    /// relay_hash | nonce | device_fp | agent_fp`.
    pub fn prologue(&self) -> Result<Vec<u8>, ChannelError> {
        // The hub channel's prologue checks the ids the same way.
        self.binding.prologue()?;
        let mut out = Vec::with_capacity(DIRECT_PROLOGUE_LABEL.len() + 3 * 66 + 4 * 32);
        out.extend_from_slice(DIRECT_PROLOGUE_LABEL);
        put_str16(&mut out, &self.binding.account);
        put_str16(&mut out, &self.binding.device);
        put_str16(&mut out, &self.binding.machine);
        out.extend_from_slice(&self.relay_hash);
        out.extend_from_slice(&self.nonce);
        out.extend_from_slice(&self.device_fp);
        out.extend_from_slice(&self.agent_fp);
        Ok(out)
    }
}

/// Whether `info` (a hello's client info, space-separated tokens) or a
/// server-info `features` list names [`CLIENT_FEATURE`].
pub fn client_wants_paths(info: &[u8]) -> bool {
    std::str::from_utf8(info).is_ok_and(|s| s.split_whitespace().any(|t| t == CLIENT_FEATURE))
}

/// What the sequencing has done so far, for logs and the path indicator.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaneStats {
    /// Frames handed up in order.
    pub delivered: u64,
    /// Frames that arrived again (a resend over the hub of something the
    /// direct path had delivered) and were dropped.
    pub duplicates: u64,
    /// Frames that arrived ahead of a gap and were held.
    pub held: u64,
    /// Frames sent again over the hub after the direct path failed.
    pub resent: u64,
}

/// Sequencing for one channel's two directions: numbers outgoing frames,
/// keeps the ones sent on the direct path until they are acknowledged, and
/// hands incoming frames up exactly once, in order, whichever path brought
/// them.
#[derive(Debug)]
pub struct Lanes {
    next_seq: u64,
    /// `(seq, frame, sent at ms)` for what went over the direct path.
    unacked: VecDeque<(u64, Vec<u8>, u64)>,
    unacked_bytes: usize,
    peer_acked: u64,
    rx_next: u64,
    held: BTreeMap<u64, Vec<u8>>,
    held_bytes: usize,
    owed: u32,
    owed_since_ms: Option<u64>,
    stats: LaneStats,
}

impl Default for Lanes {
    fn default() -> Self {
        Self::new()
    }
}

impl Lanes {
    pub fn new() -> Self {
        Self {
            next_seq: 1,
            unacked: VecDeque::new(),
            unacked_bytes: 0,
            peer_acked: 0,
            rx_next: 1,
            held: BTreeMap::new(),
            held_bytes: 0,
            owed: 0,
            owed_since_ms: None,
            stats: LaneStats::default(),
        }
    }

    pub fn stats(&self) -> LaneStats {
        self.stats
    }

    /// The highest `seq` received with nothing missing before it — and, as
    /// the caller is about to send it, the acknowledgement owed is paid.
    pub fn take_ack(&mut self) -> u64 {
        self.owed = 0;
        self.owed_since_ms = None;
        self.rx_next - 1
    }

    /// Frames received on the direct path that have not been acknowledged yet.
    pub fn owed(&self) -> u32 {
        self.owed
    }

    /// When a standalone acknowledgement becomes due, if one is owed.
    pub fn ack_due_ms(&self) -> Option<u64> {
        let since = self.owed_since_ms?;
        Some(if self.owed >= ACK_EVERY { since } else { since + ACK_DELAY_MS })
    }

    /// Bytes sent on the direct path and not acknowledged yet.
    pub fn unacked_bytes(&self) -> usize {
        self.unacked_bytes
    }

    /// Number the next outgoing frame. `keep` when the path it goes on can
    /// lose it (the direct path): it is kept until acknowledged.
    pub fn stamp(&mut self, frame: Vec<u8>, keep: bool) -> Envelope {
        self.stamp_at(frame, keep, 0)
    }

    /// [`Lanes::stamp`], noting when a kept frame left (milliseconds), so a
    /// direct path that stops acknowledging can be given up early
    /// ([`Lanes::oldest_unacked_ms`]).
    pub fn stamp_at(&mut self, frame: Vec<u8>, keep: bool, now_ms: u64) -> Envelope {
        let seq = self.next_seq;
        self.next_seq += 1;
        if keep {
            self.unacked_bytes += frame.len();
            self.unacked.push_back((seq, frame.clone(), now_ms));
        }
        Envelope::Data {
            seq,
            ack: self.take_ack(),
            frame,
        }
    }

    /// When the oldest frame still waiting for an acknowledgement left.
    pub fn oldest_unacked_ms(&self) -> Option<u64> {
        self.unacked.front().map(|(_, _, at)| *at)
    }

    /// The peer has everything up to `ack`: how many of the frames kept for
    /// the direct path that frees. An ack for something never sent is a
    /// protocol error; an older ack (one that took the slow path) is moot.
    pub fn on_ack(&mut self, ack: u64) -> Result<usize, PathError> {
        if ack >= self.next_seq {
            return Err(PathError::Protocol("an acknowledgement for a frame never sent"));
        }
        if ack <= self.peer_acked {
            return Ok(0);
        }
        self.peer_acked = ack;
        let mut freed = 0;
        while self.unacked.front().is_some_and(|(seq, _, _)| *seq <= ack) {
            let (_, frame, _) = self.unacked.pop_front().expect("checked");
            self.unacked_bytes -= frame.len();
            freed += 1;
        }
        Ok(freed)
    }

    /// One incoming frame: the frames now ready to hand up, in order (none if
    /// it was a duplicate or arrived ahead of a gap). `owe` when it came on the
    /// direct path, whose sender keeps it until acknowledged.
    pub fn receive(&mut self, seq: u64, frame: Vec<u8>, owe: bool, now_ms: u64) -> Result<Vec<Vec<u8>>, PathError> {
        if seq == 0 {
            return Err(PathError::Protocol("seq 0"));
        }
        if owe {
            self.owed += 1;
            self.owed_since_ms.get_or_insert(now_ms);
        }
        if seq < self.rx_next || self.held.contains_key(&seq) {
            self.stats.duplicates += 1;
            return Ok(Vec::new());
        }
        if seq - self.rx_next > MAX_AHEAD {
            return Err(PathError::Protocol("a sequence number far beyond the next expected"));
        }
        if seq > self.rx_next {
            self.held_bytes += frame.len();
            if self.held_bytes > MAX_HELD_BYTES {
                return Err(PathError::Overflow);
            }
            self.held.insert(seq, frame);
            self.stats.held += 1;
            return Ok(Vec::new());
        }
        let mut out = vec![frame];
        self.rx_next += 1;
        while let Some(next) = self.held.remove(&self.rx_next) {
            self.held_bytes -= next.len();
            out.push(next);
            self.rx_next += 1;
        }
        self.stats.delivered += out.len() as u64;
        Ok(out)
    }

    /// Everything sent on the direct path and not acknowledged, to send again
    /// over the hub (which loses nothing, so none of it is kept any more).
    pub fn take_unacked(&mut self) -> Vec<Envelope> {
        let ack = self.take_ack();
        self.unacked_bytes = 0;
        let out: Vec<Envelope> = self.unacked.drain(..).map(|(seq, frame, _)| Envelope::Data { seq, ack, frame }).collect();
        self.stats.resent += out.len() as u64;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::rng;
    use rand_core::RngCore;

    #[test]
    fn envelopes_round_trip_and_junk_is_refused() {
        for env in [
            Envelope::Data {
                seq: 1,
                ack: 0,
                frame: vec![2, 0, 0, 0, 1, b'x'],
            },
            Envelope::Data {
                seq: crate::codec::MAX_SAFE_U64,
                ack: 7,
                frame: vec![],
            },
            Envelope::Ack { ack: 9 },
            Envelope::Probe { nonce: 3 },
            Envelope::ProbeAck { nonce: 3 },
            Envelope::Down { reason: DownReason::Timeout },
        ] {
            assert_eq!(Envelope::decode(&env.encode()).unwrap(), env);
        }
        assert_eq!(
            Envelope::Data {
                seq: 1,
                ack: 0,
                frame: vec![9; 10]
            }
            .encode()
            .len(),
            ENVELOPE_HEADER + 10
        );
        assert!(Envelope::decode(&[]).is_err());
        assert!(Envelope::decode(&[9, 0]).is_err(), "unknown kind");
        let mut zero = Envelope::Data {
            seq: 1,
            ack: 0,
            frame: vec![],
        }
        .encode();
        zero[8] = 0;
        assert!(Envelope::decode(&zero).is_err(), "seq 0");
        let mut ack = Envelope::Ack { ack: 1 }.encode();
        ack.push(0);
        assert!(Envelope::decode(&ack).is_err(), "trailing byte");
        let mut big = Envelope::Ack { ack: 1 }.encode();
        big[1] = 0xff;
        assert!(Envelope::decode(&big).is_err(), "above 2^53 - 1");
        assert!(Envelope::decode(&[ENV_DOWN, 9]).is_err(), "unknown reason");
    }

    #[test]
    fn dc_messages_round_trip() {
        for m in [DcMessage::Hello(vec![1, 2]), DcMessage::Reply(vec![]), DcMessage::Sealed(vec![7; 100])] {
            assert_eq!(DcMessage::decode(&m.encode()).unwrap(), m);
        }
        assert!(DcMessage::decode(&[]).is_err());
        assert!(DcMessage::decode(&[4, 1]).is_err());
        let mut huge = vec![DC_SEALED];
        huge.extend(vec![0; crate::frame::MAX_PAYLOAD_LEN + 1]);
        assert!(DcMessage::decode(&huge).is_err());
    }

    #[test]
    fn signals_round_trip_and_bounds_hold() {
        let sdp = "v=0\r\na=fingerprint:sha-256 00:01\r\n".to_string();
        for s in [
            Signal::Offer {
                id: 1,
                fp: [3; 32],
                sdp: sdp.clone(),
            },
            Signal::Answer {
                id: 2,
                fp: [4; 32],
                nonce: [5; 32],
                sdp: sdp.clone(),
            },
            Signal::Candidate {
                id: 3,
                candidate: Some("candidate:1 1 udp 2122260223 2001:db8::1 50000 typ host".into()),
            },
            Signal::Candidate { id: 3, candidate: None },
            Signal::Bye {
                id: 4,
                reason: ByeReason::BadFingerprint,
                detail: "nope".into(),
            },
        ] {
            let bytes = s.encode().unwrap();
            assert_eq!(Signal::decode(&bytes).unwrap(), s);
            assert_eq!(Signal::decode(&bytes).unwrap().id(), s.id());
            let mut extra = bytes.clone();
            extra.push(0);
            assert!(Signal::decode(&extra).is_err(), "trailing byte");
            assert!(Signal::decode(&bytes[..bytes.len() - 1]).is_err(), "truncated");
        }
        assert!(Signal::Offer {
            id: 1,
            fp: [0; 32],
            sdp: "x".repeat(MAX_SDP + 1)
        }
        .encode()
        .is_err());
        assert!(Signal::Candidate {
            id: 1,
            candidate: Some("c".repeat(MAX_CANDIDATE + 1))
        }
        .encode()
        .is_err());
        assert!(Signal::decode(&[9, 0, 0, 0, 1]).is_err(), "unknown kind");
        assert!(Signal::decode(&[SIG_BYE, 0, 0, 0, 1, 9, 0, 0]).is_err(), "unknown reason");
        // A length past the bound is refused before it is read.
        let mut long = vec![SIG_CANDIDATE, 0, 0, 0, 1];
        long.extend_from_slice(&((MAX_CANDIDATE + 1) as u16).to_be_bytes());
        long.extend(vec![b'c'; MAX_CANDIDATE + 1]);
        assert!(Signal::decode(&long).is_err());
    }

    fn sdp_with(fp_line: &str) -> String {
        format!(
            "v=0\r\no=- 1 2 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n{fp_line}\r\na=setup:actpass\r\n"
        )
    }

    #[test]
    fn fingerprints_are_parsed_and_a_tampered_one_is_refused() {
        let fp: [u8; 32] = std::array::from_fn(|i| i as u8 * 7);
        let line = format!("a=fingerprint:sha-256 {}", fingerprint_hex(&fp));
        let sdp = sdp_with(&line);
        assert_eq!(sdp_fingerprint(&sdp).unwrap(), fp);
        check_fingerprint(&sdp, &fp).unwrap();
        // Lower-case hex, and the hash name in capitals, are the same thing.
        let lower = sdp_with(&format!("a=fingerprint:SHA-256 {}", fingerprint_hex(&fp).to_lowercase()));
        check_fingerprint(&lower, &fp).unwrap();

        // Someone swapped the certificate in the SDP: the stated one no longer matches.
        let mut other = fp;
        other[31] ^= 1;
        let tampered = sdp_with(&format!("a=fingerprint:sha-256 {}", fingerprint_hex(&other)));
        assert!(matches!(check_fingerprint(&tampered, &fp), Err(PathError::Fingerprint(_))));
        // …or stated a different one next to an honest SDP.
        assert!(matches!(check_fingerprint(&sdp, &other), Err(PathError::Fingerprint(_))));
        // Two different fingerprints, a weaker hash, none, or junk: refused.
        let two = sdp_with(&format!("{line}\r\na=fingerprint:sha-256 {}", fingerprint_hex(&other)));
        assert!(sdp_fingerprint(&two).is_err());
        let same_twice = sdp_with(&format!("{line}\r\n{line}"));
        assert_eq!(sdp_fingerprint(&same_twice).unwrap(), fp, "a session- and a media-level copy");
        assert!(sdp_fingerprint(&sdp_with("a=fingerprint:sha-1 00:11")).is_err());
        assert!(sdp_fingerprint(&sdp_with("a=setup:active")).is_err());
        assert!(sdp_fingerprint(&sdp_with("a=fingerprint:sha-256 00:11:zz")).is_err());
        assert!(parse_fingerprint_hex(&format!("{}:00", fingerprint_hex(&fp))).is_none(), "33 bytes");
        let signed = fingerprint_hex(&fp).replacen("00", "+0", 1);
        assert!(parse_fingerprint_hex(&signed).is_none(), "a sign is not a hex digit");
    }

    #[test]
    fn the_direct_prologue_binds_every_part() {
        let base = DirectBinding {
            binding: Binding {
                account: "acct".into(),
                device: "dev".into(),
                machine: "m".into(),
            },
            relay_hash: [1; 32],
            nonce: [2; 32],
            device_fp: [3; 32],
            agent_fp: [4; 32],
        };
        let p = base.prologue().unwrap();
        assert!(p.starts_with(DIRECT_PROLOGUE_LABEL));
        assert_ne!(p, base.binding.prologue().unwrap(), "never the hub channel's prologue");
        let variants = [
            DirectBinding {
                relay_hash: [9; 32],
                ..base.clone()
            },
            DirectBinding {
                nonce: [9; 32],
                ..base.clone()
            },
            DirectBinding {
                device_fp: [9; 32],
                ..base.clone()
            },
            DirectBinding {
                agent_fp: [9; 32],
                ..base.clone()
            },
            // Swapping the two fingerprints is a different binding too.
            DirectBinding {
                device_fp: [4; 32],
                agent_fp: [3; 32],
                ..base.clone()
            },
        ];
        for v in variants {
            assert_ne!(v.prologue().unwrap(), p, "{v:?}");
        }
        let bad = DirectBinding {
            binding: Binding {
                device: String::new(),
                ..base.binding.clone()
            },
            ..base
        };
        assert!(bad.prologue().is_err());
    }

    #[test]
    fn the_feature_token_is_matched_whole() {
        assert!(client_wants_paths(b"web/1 paths/1"));
        assert!(client_wants_paths(b"paths/1"));
        assert!(!client_wants_paths(b"web/1"));
        assert!(!client_wants_paths(b"web/1 paths/10"));
        assert!(!client_wants_paths(&[0xff, 0xfe]));
    }

    #[test]
    fn lanes_deliver_in_order_and_drop_duplicates() {
        let mut tx = Lanes::new();
        let mut rx = Lanes::new();
        let frames: Vec<Envelope> = (0..5u8).map(|i| tx.stamp(vec![i], true)).collect();
        // 1 arrives, then 3 and 4 (held), then 2 releases the rest; then 0..4 again.
        let get = |e: &Envelope| match e {
            Envelope::Data { seq, frame, .. } => (*seq, frame.clone()),
            _ => unreachable!(),
        };
        let mut out = Vec::new();
        for i in [0, 2, 3, 1, 4, 0, 1, 2, 3, 4] {
            let (seq, frame) = get(&frames[i]);
            out.extend(rx.receive(seq, frame, true, 0).unwrap());
        }
        assert_eq!(out, (0..5u8).map(|i| vec![i]).collect::<Vec<_>>());
        let stats = rx.stats();
        assert_eq!((stats.delivered, stats.duplicates, stats.held), (5, 5, 2));
        assert_eq!(rx.owed(), 10);
        assert_eq!(rx.ack_due_ms(), Some(ACK_DELAY_MS), "ten owed: due after the delay");
        assert_eq!(rx.take_ack(), 5);
        assert_eq!(rx.owed(), 0);
        // Acks free what they cover; an old ack is moot; an ack for the future is an error.
        assert_eq!(tx.unacked_bytes(), 5);
        tx.on_ack(3).unwrap();
        assert_eq!(tx.unacked_bytes(), 2);
        tx.on_ack(1).unwrap();
        assert_eq!(tx.unacked_bytes(), 2);
        assert!(tx.on_ack(6).is_err());
        let again = tx.take_unacked();
        assert_eq!(again.iter().map(|e| get(e).0).collect::<Vec<_>>(), [4, 5]);
        assert_eq!(tx.unacked_bytes(), 0);
        assert_eq!(tx.stats().resent, 2);
        // Bounds: seq 0, far ahead, and too much held.
        let mut r = Lanes::new();
        assert!(r.receive(0, vec![], false, 0).is_err());
        assert!(r.receive(2 + MAX_AHEAD, vec![], false, 0).is_err());
        let mut r = Lanes::new();
        assert_eq!(r.receive(3, vec![0; MAX_HELD_BYTES], false, 0).unwrap(), Vec::<Vec<u8>>::new());
        assert_eq!(r.receive(4, vec![0], false, 0), Err(PathError::Overflow));
    }

    #[test]
    fn acks_fall_due_after_a_delay_or_a_count() {
        let mut rx = Lanes::new();
        assert_eq!(rx.ack_due_ms(), None);
        rx.receive(1, vec![], true, 100).unwrap();
        assert_eq!(rx.ack_due_ms(), Some(100 + ACK_DELAY_MS));
        for seq in 2..=u64::from(ACK_EVERY) {
            rx.receive(seq, vec![], true, 105).unwrap();
        }
        assert_eq!(rx.ack_due_ms(), Some(100), "at once once enough are owed");
        // Frames over the hub owe nothing.
        let mut hub = Lanes::new();
        hub.receive(1, vec![], false, 0).unwrap();
        assert_eq!(hub.ack_due_ms(), None);
    }

    /// Two paths, a sender that switches between them, a direct path that
    /// loses whatever is in flight when it dies, a hub path that is slower but
    /// loses nothing: every frame comes out exactly once, in order. Many runs,
    /// each with its own interleaving.
    #[test]
    fn sequencing_survives_switches_and_a_dying_direct_path() {
        for seed in 0..200u64 {
            let mut r = rng(seed);
            let mut tx = Lanes::new();
            let mut rx = Lanes::new();
            let mut relay: VecDeque<Envelope> = VecDeque::new();
            let mut direct: VecDeque<Envelope> = VecDeque::new();
            let mut on_direct = false;
            let mut delivered: Vec<u32> = Vec::new();
            let total = 300u32;
            let mut sent = 0u32;
            let mut steps = 0;
            while (sent < total || !relay.is_empty() || !direct.is_empty() || tx.unacked_bytes() > 0) && steps < 100_000 {
                steps += 1;
                match r.next_u32() % 10 {
                    // Send the next frame on the current path.
                    0..=2 if sent < total => {
                        let env = tx.stamp(sent.to_be_bytes().to_vec(), on_direct);
                        if on_direct {
                            direct.push_back(env);
                        } else {
                            relay.push_back(env);
                        }
                        sent += 1;
                    }
                    // The direct path comes up.
                    3 if !on_direct && r.next_u32().is_multiple_of(4) => on_direct = true,
                    // The direct path dies: what is in flight on it is lost,
                    // the sender resends what was not acknowledged over the hub.
                    4 if on_direct && r.next_u32().is_multiple_of(6) => {
                        let lost = r.next_u32() as usize % (direct.len() + 1);
                        direct.truncate(direct.len() - lost);
                        on_direct = false;
                        relay.extend(tx.take_unacked());
                        // What was not lost on the direct path still arrives
                        // (it was already on the wire); the hub copies are dups.
                    }
                    // Deliver one from the fast path (often) or the slow one.
                    5..=7 => {
                        if let Some(Envelope::Data { seq, frame, .. }) = direct.pop_front() {
                            for f in rx.receive(seq, frame, true, 0).unwrap() {
                                delivered.push(u32::from_be_bytes(f.try_into().unwrap()));
                            }
                        }
                    }
                    8 => {
                        if let Some(Envelope::Data { seq, frame, .. }) = relay.pop_front() {
                            for f in rx.receive(seq, frame, false, 0).unwrap() {
                                delivered.push(u32::from_be_bytes(f.try_into().unwrap()));
                            }
                        }
                    }
                    // The receiver acknowledges (over whichever path).
                    _ => {
                        if rx.owed() > 0 {
                            let ack = rx.take_ack();
                            tx.on_ack(ack).unwrap();
                        } else if sent == total && relay.is_empty() && direct.is_empty() {
                            // Nothing in flight and nothing owed: whatever is
                            // still unacked was delivered; the final ack.
                            let ack = rx.take_ack();
                            tx.on_ack(ack).unwrap();
                        }
                    }
                }
            }
            assert_eq!(delivered, (0..total).collect::<Vec<_>>(), "seed {seed}");
        }
    }
}
