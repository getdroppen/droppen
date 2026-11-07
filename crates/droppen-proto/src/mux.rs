//! Sub-streams inside one end-to-end channel.
//!
//! One Noise channel per (device, machine) carries every conversation between
//! them: `rpc` (one per HTTP request to the agent's `/api/*`), `mux` (the feed
//! socket), `term` (one per terminal) and `blob` (file transfers). Each is a
//! sub-stream with its own credit window, and the sender drains them by
//! priority — **control > term > rpc/mux > blob** — so a keystroke never waits
//! behind a file chunk. Within a priority, streams take turns.
//!
//! # Frames (inside a transport message's data)
//!
//! | kind | body |
//! |---|---|
//! | `OPEN` 1 | `sid u32, class u8, window u32, meta…` |
//! | `DATA` 2 | `sid u32, bytes…` |
//! | `CREDIT` 3 | `sid u32, delta u32` |
//! | `CLOSE` 4 | `sid u32, code u16` |
//! | `PING` 5 / `PONG` 6 | `nonce u64` |
//!
//! Stream ids are odd for streams the initiator (device) opens and even for the
//! responder's, increase monotonically and are never reused — the rule
//! `feed/mux.rs` already follows for `sub`. `OPEN`'s `window` is how much the
//! opener will accept; the opener may send [`INITIAL_WINDOW`] before hearing
//! otherwise. Receiving past the window is a protocol error, not a slowdown.
//!
//! `CLOSE` is queued behind the stream's data, so everything sent before it
//! arrives. The receiver answers with its own `CLOSE` (if it had not sent one)
//! and both forget the stream; a late frame for a forgotten id is dropped.

use std::collections::{BTreeMap, VecDeque};

use thiserror::Error;

use crate::codec::{DecodeError, Reader};
use crate::noise::MAX_TRANSPORT_DATA;

/// What every stream may be sent before its first `CREDIT`.
pub const INITIAL_WINDOW: u32 = 256 * 1024;
/// `kind u8 + sid u32`.
pub const DATA_HEADER: usize = 5;
/// The most data one `DATA` frame carries: what fits in one transport
/// message after the frame's header and, on a channel with two paths, the
/// sequencing envelope's ([`crate::path::ENVELOPE_HEADER`]).
pub const MAX_CHUNK: usize = MAX_TRANSPORT_DATA - crate::path::ENVELOPE_HEADER - DATA_HEADER;
/// Blob chunks are kept small so a terminal frame queued behind one waits for
/// at most this much on the wire.
pub const BLOB_CHUNK: usize = 16 * 1024;
/// The largest `OPEN` metadata (a request line and headers, a terminal id).
pub const MAX_META: usize = 8 * 1024;
/// The most streams open at once on one channel.
pub const MAX_STREAMS: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum StreamClass {
    Rpc = 1,
    Mux = 2,
    Term = 3,
    Blob = 4,
}

impl StreamClass {
    pub fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            1 => Self::Rpc,
            2 => Self::Mux,
            3 => Self::Term,
            4 => Self::Blob,
            _ => return None,
        })
    }

    /// 1 is drained first; control frames are 0.
    pub fn priority(self) -> usize {
        match self {
            Self::Term => 1,
            Self::Rpc | Self::Mux => 2,
            Self::Blob => 3,
        }
    }

    fn chunk(self) -> usize {
        if self == Self::Blob {
            BLOB_CHUNK
        } else {
            MAX_CHUNK
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MuxFrame {
    Open {
        sid: u32,
        class: StreamClass,
        window: u32,
        meta: Vec<u8>,
    },
    Data {
        sid: u32,
        data: Vec<u8>,
    },
    Credit {
        sid: u32,
        delta: u32,
    },
    Close {
        sid: u32,
        code: u16,
    },
    Ping {
        nonce: u64,
    },
    Pong {
        nonce: u64,
    },
}

impl MuxFrame {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            MuxFrame::Open { sid, class, window, meta } => {
                out.push(1);
                out.extend_from_slice(&sid.to_be_bytes());
                out.push(*class as u8);
                out.extend_from_slice(&window.to_be_bytes());
                out.extend_from_slice(meta);
            }
            MuxFrame::Data { sid, data } => {
                out.push(2);
                out.extend_from_slice(&sid.to_be_bytes());
                out.extend_from_slice(data);
            }
            MuxFrame::Credit { sid, delta } => {
                out.push(3);
                out.extend_from_slice(&sid.to_be_bytes());
                out.extend_from_slice(&delta.to_be_bytes());
            }
            MuxFrame::Close { sid, code } => {
                out.push(4);
                out.extend_from_slice(&sid.to_be_bytes());
                out.extend_from_slice(&code.to_be_bytes());
            }
            MuxFrame::Ping { nonce } => {
                out.push(5);
                out.extend_from_slice(&nonce.to_be_bytes());
            }
            MuxFrame::Pong { nonce } => {
                out.push(6);
                out.extend_from_slice(&nonce.to_be_bytes());
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let frame = match r.u8()? {
            1 => {
                let sid = r.u32()?;
                let class = r.u8()?;
                let class = StreamClass::from_u8(class).ok_or(DecodeError::Unknown {
                    what: "stream class",
                    value: class,
                })?;
                let window = r.u32()?;
                let meta = r.rest();
                if meta.len() > MAX_META {
                    return Err(DecodeError::TooLarge {
                        what: "open metadata",
                        len: meta.len(),
                        max: MAX_META,
                    });
                }
                MuxFrame::Open {
                    sid,
                    class,
                    window,
                    meta: meta.to_vec(),
                }
            }
            2 => MuxFrame::Data {
                sid: r.u32()?,
                data: r.rest().to_vec(),
            },
            3 => MuxFrame::Credit {
                sid: r.u32()?,
                delta: r.u32()?,
            },
            4 => MuxFrame::Close {
                sid: r.u32()?,
                code: r.u16()?,
            },
            5 => MuxFrame::Ping { nonce: r.u64()? },
            6 => MuxFrame::Pong { nonce: r.u64()? },
            other => {
                return Err(DecodeError::Unknown {
                    what: "mux frame",
                    value: other,
                })
            }
        };
        r.finish()?;
        Ok(frame)
    }
}

/// Which end of the channel this is; decides stream id parity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Initiator,
    Responder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MuxEvent {
    Opened { sid: u32, class: StreamClass, meta: Vec<u8> },
    Data { sid: u32, data: Vec<u8> },
    Closed { sid: u32, code: u16 },
    Pong { nonce: u64 },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MuxError {
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error("protocol violation: {0}")]
    Protocol(&'static str),
    #[error("stream {0} sent past its window")]
    FlowControl(u32),
    #[error("no such stream {0}")]
    UnknownStream(u32),
    #[error("too many open streams")]
    TooManyStreams,
}

struct Stream {
    class: StreamClass,
    send_credit: u64,
    recv_window: u64,
    recv_unacked: u64,
    pending: VecDeque<Vec<u8>>,
    pending_close: Option<u16>,
    local_closed: bool,
}

/// One end's sub-stream state. Sans-IO: feed it decoded frames with
/// [`Mux::receive`], and take frames to send from [`Mux::poll`].
pub struct Mux {
    role: Role,
    next_sid: u32,
    peer_next_sid: u32,
    window: u32,
    streams: BTreeMap<u32, Stream>,
    control: VecDeque<MuxFrame>,
    /// Per priority, the sid served last, for round-robin.
    last_served: [u32; 4],
}

impl Mux {
    pub fn new(role: Role) -> Self {
        Self::with_window(role, INITIAL_WINDOW)
    }

    /// `window` is the receive window this end advertises on the streams it opens.
    pub fn with_window(role: Role, window: u32) -> Self {
        let (mine, theirs) = match role {
            Role::Initiator => (1, 2),
            Role::Responder => (2, 1),
        };
        Self {
            role,
            next_sid: mine,
            peer_next_sid: theirs,
            window,
            streams: BTreeMap::new(),
            control: VecDeque::new(),
            last_served: [0; 4],
        }
    }

    pub fn role(&self) -> Role {
        self.role
    }

    fn is_mine(&self, sid: u32) -> bool {
        (sid % 2 == 1) == (self.role == Role::Initiator)
    }

    /// Open a stream; its `OPEN` goes out before anything else queued.
    pub fn open(&mut self, class: StreamClass, meta: &[u8]) -> Result<u32, MuxError> {
        if meta.len() > MAX_META {
            return Err(DecodeError::TooLarge {
                what: "open metadata",
                len: meta.len(),
                max: MAX_META,
            }
            .into());
        }
        if self.streams.len() >= MAX_STREAMS {
            return Err(MuxError::TooManyStreams);
        }
        let sid = self.next_sid;
        self.next_sid = self.next_sid.checked_add(2).ok_or(MuxError::TooManyStreams)?;
        self.streams.insert(
            sid,
            Stream {
                class,
                send_credit: u64::from(INITIAL_WINDOW),
                recv_window: u64::from(self.window),
                recv_unacked: 0,
                pending: VecDeque::new(),
                pending_close: None,
                local_closed: false,
            },
        );
        self.control.push_back(MuxFrame::Open {
            sid,
            class,
            window: self.window,
            meta: meta.to_vec(),
        });
        Ok(sid)
    }

    /// Queue data on a stream, cut into chunks its class allows.
    pub fn send(&mut self, sid: u32, data: &[u8]) -> Result<(), MuxError> {
        let stream = self
            .streams
            .get_mut(&sid)
            .filter(|s| s.pending_close.is_none() && !s.local_closed)
            .ok_or(MuxError::UnknownStream(sid))?;
        for chunk in data.chunks(stream.class.chunk()) {
            stream.pending.push_back(chunk.to_vec());
        }
        Ok(())
    }

    /// Close a stream once everything already queued on it has gone.
    pub fn close(&mut self, sid: u32, code: u16) -> Result<(), MuxError> {
        let stream = self
            .streams
            .get_mut(&sid)
            .filter(|s| !s.local_closed)
            .ok_or(MuxError::UnknownStream(sid))?;
        stream.pending_close.get_or_insert(code);
        Ok(())
    }

    /// The application has taken `n` bytes of a stream's data: let the peer send
    /// that much more, in batches of half a window so `CREDIT`s stay rare.
    pub fn consumed(&mut self, sid: u32, n: usize) {
        let window = u64::from(if self.is_mine(sid) { self.window } else { INITIAL_WINDOW });
        if let Some(stream) = self.streams.get_mut(&sid) {
            stream.recv_unacked += n as u64;
            if stream.recv_unacked >= window / 2 {
                let delta = stream.recv_unacked.min(u64::from(u32::MAX));
                stream.recv_unacked -= delta;
                stream.recv_window += delta;
                self.control.push_back(MuxFrame::Credit { sid, delta: delta as u32 });
            }
        }
    }

    pub fn ping(&mut self, nonce: u64) {
        self.control.push_back(MuxFrame::Ping { nonce });
    }

    /// Whether [`Mux::poll`] has anything to give right now.
    pub fn has_pending(&self) -> bool {
        !self.control.is_empty()
            || self
                .streams
                .values()
                .any(|s| (!s.pending.is_empty() && s.send_credit > 0) || (s.pending.is_empty() && s.pending_close.is_some()))
    }

    /// The next frame to send, by priority, or `None` if everything sendable has gone.
    pub fn poll(&mut self) -> Option<MuxFrame> {
        if let Some(frame) = self.control.pop_front() {
            return Some(frame);
        }
        for priority in 1..=3 {
            let ready: Vec<u32> = self
                .streams
                .iter()
                .filter(|(_, s)| s.class.priority() == priority)
                .filter(|(_, s)| (!s.pending.is_empty() && s.send_credit > 0) || (s.pending.is_empty() && s.pending_close.is_some()))
                .map(|(&sid, _)| sid)
                .collect();
            if ready.is_empty() {
                continue;
            }
            let last = self.last_served[priority];
            let sid = ready.iter().copied().find(|&sid| sid > last).unwrap_or(ready[0]);
            self.last_served[priority] = sid;
            let stream = self.streams.get_mut(&sid).expect("listed above");
            if let Some(front) = stream.pending.front_mut() {
                let n = front.len().min(stream.send_credit.min(usize::MAX as u64) as usize);
                let data = if n == front.len() {
                    stream.pending.pop_front().expect("front")
                } else {
                    front.drain(..n).collect()
                };
                stream.send_credit -= n as u64;
                return Some(MuxFrame::Data { sid, data });
            }
            let code = stream.pending_close.take().expect("ready to close");
            stream.local_closed = true;
            return Some(MuxFrame::Close { sid, code });
        }
        None
    }

    /// Take one frame from the peer.
    pub fn receive(&mut self, frame: MuxFrame) -> Result<Option<MuxEvent>, MuxError> {
        match frame {
            MuxFrame::Open { sid, class, window, meta } => {
                if self.is_mine(sid) {
                    return Err(MuxError::Protocol("the peer opened a stream with this end's parity"));
                }
                if sid < self.peer_next_sid {
                    return Err(MuxError::Protocol("a stream id was reused or went backwards"));
                }
                if self.streams.len() >= MAX_STREAMS {
                    return Err(MuxError::TooManyStreams);
                }
                self.peer_next_sid = sid.checked_add(2).ok_or(MuxError::TooManyStreams)?;
                self.streams.insert(
                    sid,
                    Stream {
                        class,
                        send_credit: u64::from(window),
                        recv_window: u64::from(INITIAL_WINDOW),
                        recv_unacked: 0,
                        pending: VecDeque::new(),
                        pending_close: None,
                        local_closed: false,
                    },
                );
                Ok(Some(MuxEvent::Opened { sid, class, meta }))
            }
            MuxFrame::Data { sid, data } => {
                let Some(stream) = self.streams.get_mut(&sid) else {
                    return self.forgotten(sid);
                };
                if data.len() as u64 > stream.recv_window {
                    return Err(MuxError::FlowControl(sid));
                }
                stream.recv_window -= data.len() as u64;
                Ok(Some(MuxEvent::Data { sid, data }))
            }
            MuxFrame::Credit { sid, delta } => {
                let Some(stream) = self.streams.get_mut(&sid) else {
                    return self.forgotten(sid);
                };
                stream.send_credit = stream
                    .send_credit
                    .checked_add(u64::from(delta))
                    .filter(|&c| c <= u64::from(u32::MAX))
                    .ok_or(MuxError::Protocol("credit overflow"))?;
                Ok(None)
            }
            MuxFrame::Close { sid, code } => {
                let Some(stream) = self.streams.remove(&sid) else {
                    return self.forgotten(sid);
                };
                if stream.local_closed {
                    // The answer to this end's own close: nothing to tell anyone.
                    return Ok(None);
                }
                // Answer, so the peer can forget the stream too; anything still
                // queued on it is moot.
                self.control.push_back(MuxFrame::Close { sid, code });
                Ok(Some(MuxEvent::Closed { sid, code }))
            }
            MuxFrame::Ping { nonce } => {
                self.control.push_back(MuxFrame::Pong { nonce });
                Ok(None)
            }
            MuxFrame::Pong { nonce } => Ok(Some(MuxEvent::Pong { nonce })),
        }
    }

    /// A frame for a stream this end does not hold: fine if the stream existed
    /// and has been closed (the frame crossed a `CLOSE` in flight), an error if
    /// it never existed.
    fn forgotten(&self, sid: u32) -> Result<Option<MuxEvent>, MuxError> {
        let existed = if self.is_mine(sid) {
            sid < self.next_sid
        } else {
            sid < self.peer_next_sid
        };
        if existed {
            Ok(None)
        } else {
            Err(MuxError::UnknownStream(sid))
        }
    }

    pub fn is_open(&self, sid: u32) -> bool {
        self.streams.contains_key(&sid)
    }
}

/// A fixed sequence of operations and the frames it must produce, shared with
/// the TypeScript half as a vector so both schedulers agree.
#[cfg(test)]
pub(crate) fn scheduling_scenario() -> (Vec<String>, Vec<MuxFrame>) {
    let mut ops = Vec::new();
    let mut mux = Mux::new(Role::Initiator);
    let mut out = Vec::new();
    let blob = mux.open(StreamClass::Blob, b"file").unwrap();
    let rpc = mux.open(StreamClass::Rpc, b"GET /api/sessions").unwrap();
    let term = mux.open(StreamClass::Term, b"t-1").unwrap();
    let feed = mux.open(StreamClass::Mux, b"/api/events?v=2").unwrap();
    ops.push("open blob 'file'; open rpc 'GET /api/sessions'; open term 't-1'; open mux '/api/events?v=2'".to_string());
    mux.send(blob, &vec![0xbb; BLOB_CHUNK + 10]).unwrap();
    mux.send(rpc, b"request body").unwrap();
    mux.send(feed, b"sub").unwrap();
    mux.send(term, b"ls\r").unwrap();
    mux.close(rpc, 0).unwrap();
    mux.ping(7);
    ops.push("send blob 16394×0xbb; send rpc 'request body'; send mux 'sub'; send term 'ls\\r'; close rpc 0; ping 7".to_string());
    while let Some(frame) = mux.poll() {
        out.push(frame);
    }
    (ops, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pump(from: &mut Mux, to: &mut Mux) -> Vec<MuxEvent> {
        let mut events = Vec::new();
        while let Some(frame) = from.poll() {
            let decoded = MuxFrame::decode(&frame.encode()).unwrap();
            assert_eq!(decoded, frame);
            if let Some(event) = to.receive(decoded).unwrap() {
                events.push(event);
            }
        }
        events
    }

    #[test]
    fn priorities_drain_control_then_term_then_rpc_and_mux_then_blob() {
        let (_, frames) = scheduling_scenario();
        let kinds: Vec<String> = frames
            .iter()
            .map(|f| match f {
                MuxFrame::Open { sid, .. } => format!("open{sid}"),
                MuxFrame::Data { sid, data } => format!("data{sid}:{}", data.len()),
                MuxFrame::Close { sid, .. } => format!("close{sid}"),
                MuxFrame::Ping { .. } => "ping".into(),
                other => format!("{other:?}"),
            })
            .collect();
        // sids: blob 1, rpc 3, term 5, mux 7. rpc and mux share a priority and
        // take turns; the rpc stream's close waits behind its data.
        assert_eq!(
            kinds,
            [
                "open1",
                "open3",
                "open5",
                "open7",
                "ping",
                "data5:3",
                "data3:12",
                "data7:3",
                "close3",
                "data1:16384",
                "data1:10"
            ]
        );
    }

    #[test]
    fn streams_carry_data_both_ways_and_close_cleanly() {
        let mut a = Mux::new(Role::Initiator);
        let mut b = Mux::new(Role::Responder);
        let sid = a.open(StreamClass::Rpc, b"GET /").unwrap();
        a.send(sid, b"hello").unwrap();
        assert_eq!(
            pump(&mut a, &mut b),
            [
                MuxEvent::Opened {
                    sid,
                    class: StreamClass::Rpc,
                    meta: b"GET /".to_vec()
                },
                MuxEvent::Data {
                    sid,
                    data: b"hello".to_vec()
                }
            ]
        );
        b.send(sid, b"200 OK").unwrap();
        b.close(sid, 0).unwrap();
        assert_eq!(
            pump(&mut b, &mut a),
            [
                MuxEvent::Data {
                    sid,
                    data: b"200 OK".to_vec()
                },
                MuxEvent::Closed { sid, code: 0 }
            ]
        );
        // a answers the close; b forgets the stream.
        assert!(pump(&mut a, &mut b).is_empty());
        assert!(!a.is_open(sid) && !b.is_open(sid));
        // The responder's streams are even.
        let back = b.open(StreamClass::Term, b"").unwrap();
        assert_eq!(back % 2, 0);
    }

    #[test]
    fn credit_limits_what_is_sent_and_the_receiver_enforces_it() {
        let mut a = Mux::new(Role::Initiator);
        let mut b = Mux::with_window(Role::Responder, 1000);
        let sid = b.open(StreamClass::Blob, b"").unwrap();
        pump(&mut b, &mut a);
        a.send(sid, &vec![1; 2500]).unwrap();
        let sent: usize = pump(&mut a, &mut b)
            .iter()
            .map(|e| if let MuxEvent::Data { data, .. } = e { data.len() } else { 0 })
            .sum();
        assert_eq!(sent, 1000, "stops at the window");
        assert!(!a.has_pending());
        b.consumed(sid, 1000);
        pump(&mut b, &mut a);
        let sent: usize = pump(&mut a, &mut b)
            .iter()
            .map(|e| if let MuxEvent::Data { data, .. } = e { data.len() } else { 0 })
            .sum();
        assert_eq!(sent, 1000);
        // A peer that ignores the window is cut off.
        let mut c = Mux::with_window(Role::Responder, 10);
        let sid = c.open(StreamClass::Rpc, b"").unwrap();
        let mut d = Mux::new(Role::Initiator);
        pump(&mut c, &mut d);
        assert_eq!(c.receive(MuxFrame::Data { sid, data: vec![0; 11] }), Err(MuxError::FlowControl(sid)));
    }

    #[test]
    fn stream_ids_must_follow_the_rules() {
        let mut b = Mux::new(Role::Responder);
        let open = |sid| MuxFrame::Open {
            sid,
            class: StreamClass::Rpc,
            window: 10,
            meta: vec![],
        };
        assert!(b.receive(open(2)).is_err(), "the initiator's streams are odd");
        b.receive(open(5)).unwrap();
        assert!(b.receive(open(3)).is_err(), "never backwards");
        assert!(b.receive(open(5)).is_err(), "never reused");
        assert_eq!(b.receive(MuxFrame::Data { sid: 9, data: vec![] }), Err(MuxError::UnknownStream(9)));
        // A frame for a stream closed in the meantime is dropped quietly.
        b.receive(MuxFrame::Close { sid: 5, code: 0 }).unwrap();
        assert_eq!(b.receive(MuxFrame::Data { sid: 5, data: vec![1] }), Ok(None));
        assert_eq!(
            b.receive(MuxFrame::Data { sid: 3, data: vec![1] }),
            Ok(None),
            "3 was skipped, which also counts as gone"
        );
    }

    #[test]
    fn frames_round_trip_and_junk_is_refused() {
        for frame in [
            MuxFrame::Open {
                sid: 1,
                class: StreamClass::Term,
                window: 5,
                meta: b"x".to_vec(),
            },
            MuxFrame::Data { sid: 1, data: vec![] },
            MuxFrame::Credit { sid: 1, delta: 9 },
            MuxFrame::Close { sid: 1, code: 4 },
            MuxFrame::Ping { nonce: 1 },
            MuxFrame::Pong { nonce: 2 },
        ] {
            assert_eq!(MuxFrame::decode(&frame.encode()).unwrap(), frame);
        }
        assert!(MuxFrame::decode(&[9]).is_err());
        assert!(MuxFrame::decode(&[1, 0, 0, 0, 1, 9, 0, 0, 0, 0]).is_err(), "unknown class");
        assert!(MuxFrame::decode(&[3, 0, 0, 0, 1, 0, 0, 0, 1, 0]).is_err(), "trailing byte");
    }
}
