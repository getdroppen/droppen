//! The frames the hub can see: `[type u8][ch u32][len u16][payload]`.
//!
//! Browsers, phones and agents only ever dial *out* to the hub, and the hub
//! pairs them up by (device, machine, `ch`) and forwards. Everything the hub
//! needs to route, meter, store and rate-limit is in this envelope or in the
//! body of a stream frame; everything else — the whole of an end-to-end channel
//! — is the opaque payload of a `DATA` frame, which the hub copies without
//! parsing (see [`crate::noise`] for what is inside).
//!
//! # Versioning
//!
//! The hub-visible protocol version is the URL path ([`AGENT_PATH`],
//! [`CLIENT_PATH`]): a `/v2/` would be a different envelope. Within v1, the
//! type byte's high bit is the extension rule — a peer that meets an unknown
//! type `>= 0x80` skips the frame ([`Frame::Unknown`]), and one that meets an
//! unknown type below that closes the connection, because a frame it was
//! required to understand has been lost on it. Feature lists between a device
//! and its agent travel inside the encrypted channel, where the hub cannot see
//! (or strip) them, and record formats are versioned by the record's own `fmt`.
//!
//! # Limits
//!
//! A frame is at most [`MAX_FRAME_LEN`] bytes including its header: big enough
//! for a 64 KiB terminal paint, small enough that one connection's frame can
//! never hold up another's for long, and the bound every per-connection
//! rate-limiter in the hub is written against.

use crate::codec::{DecodeError, Reader};
use crate::record::{SealedRecord, StreamId};

/// The path an agent connects to.
pub const AGENT_PATH: &str = "/v1/agent";
/// The path a browser or phone connects to.
pub const CLIENT_PATH: &str = "/v1/client";
/// `[type u8][ch u32][len u16]`.
pub const HEADER_LEN: usize = 7;
/// The largest frame, header included.
pub const MAX_FRAME_LEN: usize = 64 * 1024;
/// The largest payload a frame can carry.
pub const MAX_PAYLOAD_LEN: usize = MAX_FRAME_LEN - HEADER_LEN;
/// The longest peer id in an `OPEN` (a device or machine id).
pub const MAX_PEER_ID_LEN: usize = 64;
/// The longest human-readable reason in a `CLOSE` — a websocket close reason's
/// limit, so a hub can pass one through unchanged.
pub const MAX_CLOSE_REASON_LEN: usize = 123;
/// Types at or above this are optional extensions that may be skipped.
pub const IGNORABLE_TYPES: u8 = 0x80;

/// Every frame type v1 defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    /// Start a channel to a peer; the hub rewrites the peer to the sender when
    /// it forwards, so each side learns who is on the other end.
    Open = 0x01,
    /// An opaque payload on an open channel: in practice a Noise message.
    Data = 0x02,
    /// The channel is over, with a code and a reason.
    Close = 0x03,
    /// Hub-level flow control: the receiver may now be sent this many more bytes.
    Credit = 0x04,
    /// Liveness, on `ch` 0. Answered by [`FrameType::Pong`] with the same nonce.
    Ping = 0x05,
    Pong = 0x06,
    /// Append records to a stream, if and only if its head is `expect_seq` (CAS).
    Append = 0x10,
    /// The append is durable; the stream's head is now `head`.
    Ack = 0x11,
    /// The append lost the CAS; the stream's head is `head`.
    Conflict = 0x12,
    /// Send me a stream's records from `from_seq`, then keep sending.
    Sub = 0x13,
    /// Records of a subscribed stream.
    Recs = 0x14,
    /// The records asked for have been compacted away; restart from the
    /// checkpoint at `first_seq`.
    Gone = 0x15,
    /// Everything up to `upto_seq` is covered by this signed manifest.
    Checkpoint = 0x16,
    /// An encrypted payload for a stream's live subscribers that is never stored
    /// (drafts of a message still being written).
    Ephemeral = 0x17,
    /// A stream head the machine signed ([`crate::record::SignedHead`]): the
    /// hub keeps the newest per stream and hands it to readers with the records.
    Head = 0x18,
    /// A snapshot page ([`crate::stream::SealedPage`]) an agent uploads,
    /// named by the SHA-256 of its encoding, which the hub checks.
    Page = 0x19,
    /// Device-chain entries, each one signed (see [`crate::chain`]).
    Chain = 0x20,
    /// A Web Push payload the agent encrypted for a subscription; the hub adds
    /// the VAPID header and delivers it.
    PushSend = 0x21,
    /// A full device's chain attestation ([`crate::chain::Attestation`]), which
    /// the hub relays to the account's agents.
    Attest = 0x22,
}

impl FrameType {
    pub fn from_u8(value: u8) -> Option<Self> {
        use FrameType::*;
        Some(match value {
            0x01 => Open,
            0x02 => Data,
            0x03 => Close,
            0x04 => Credit,
            0x05 => Ping,
            0x06 => Pong,
            0x10 => Append,
            0x11 => Ack,
            0x12 => Conflict,
            0x13 => Sub,
            0x14 => Recs,
            0x15 => Gone,
            0x16 => Checkpoint,
            0x17 => Ephemeral,
            0x18 => Head,
            0x19 => Page,
            0x20 => Chain,
            0x21 => PushSend,
            0x22 => Attest,
            _ => return None,
        })
    }
}

/// A frame split into its envelope, payload unparsed: all a relay needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFrame {
    pub ty: u8,
    pub ch: u32,
    pub payload: Vec<u8>,
}

impl RawFrame {
    pub fn encode(&self) -> Result<Vec<u8>, DecodeError> {
        if self.payload.len() > MAX_PAYLOAD_LEN {
            return Err(DecodeError::TooLarge {
                what: "frame payload",
                len: self.payload.len(),
                max: MAX_PAYLOAD_LEN,
            });
        }
        let mut out = Vec::with_capacity(HEADER_LEN + self.payload.len());
        out.push(self.ty);
        out.extend_from_slice(&self.ch.to_be_bytes());
        out.extend_from_slice(&(self.payload.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.payload);
        Ok(out)
    }

    /// The first frame in `buf` and how many bytes it took, or `None` if `buf`
    /// does not hold a whole one yet.
    pub fn decode_prefix(buf: &[u8]) -> Result<Option<(RawFrame, usize)>, DecodeError> {
        if buf.len() < HEADER_LEN {
            return Ok(None);
        }
        let ty = buf[0];
        let ch = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]);
        let len = usize::from(u16::from_be_bytes([buf[5], buf[6]]));
        if len > MAX_PAYLOAD_LEN {
            return Err(DecodeError::TooLarge {
                what: "frame payload",
                len,
                max: MAX_PAYLOAD_LEN,
            });
        }
        if buf.len() < HEADER_LEN + len {
            return Ok(None);
        }
        let payload = buf[HEADER_LEN..HEADER_LEN + len].to_vec();
        Ok(Some((RawFrame { ty, ch, payload }, HEADER_LEN + len)))
    }

    /// Exactly one frame, and nothing after it.
    pub fn decode(buf: &[u8]) -> Result<RawFrame, DecodeError> {
        match Self::decode_prefix(buf)? {
            Some((frame, used)) if used == buf.len() => Ok(frame),
            Some((_, used)) => Err(DecodeError::TrailingBytes(buf.len() - used)),
            None => Err(DecodeError::Truncated),
        }
    }
}

/// Splits a byte stream (a TCP connection, or several frames batched into one
/// websocket message) into frames.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next whole frame, if one has arrived. An error poisons the stream:
    /// framing once lost cannot be found again, so the caller must close.
    pub fn next_frame(&mut self) -> Result<Option<RawFrame>, DecodeError> {
        match RawFrame::decode_prefix(&self.buf)? {
            Some((frame, used)) => {
                self.buf.drain(..used);
                Ok(Some(frame))
            }
            None => Ok(None),
        }
    }
}

/// A frame with its body parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Open {
        ch: u32,
        peer: String,
    },
    Data {
        ch: u32,
        payload: Vec<u8>,
    },
    Close {
        ch: u32,
        code: u16,
        reason: String,
    },
    Credit {
        ch: u32,
        bytes: u32,
    },
    Ping {
        nonce: u64,
    },
    Pong {
        nonce: u64,
    },
    Append {
        ch: u32,
        stream: StreamId,
        expect_seq: u64,
        records: Vec<SealedRecord>,
    },
    Ack {
        ch: u32,
        stream: StreamId,
        head: u64,
    },
    Conflict {
        ch: u32,
        stream: StreamId,
        head: u64,
    },
    Sub {
        ch: u32,
        stream: StreamId,
        from_seq: u64,
    },
    Recs {
        ch: u32,
        stream: StreamId,
        records: Vec<SealedRecord>,
    },
    Gone {
        ch: u32,
        stream: StreamId,
        first_seq: u64,
    },
    Checkpoint {
        ch: u32,
        stream: StreamId,
        upto_seq: u64,
        manifest: Vec<u8>,
    },
    Ephemeral {
        ch: u32,
        stream: StreamId,
        payload: Vec<u8>,
    },
    Head {
        ch: u32,
        head: Vec<u8>,
    },
    Page {
        ch: u32,
        stream: StreamId,
        hash: [u8; 32],
        data: Vec<u8>,
    },
    Chain {
        ch: u32,
        entries: Vec<Vec<u8>>,
    },
    PushSend {
        ch: u32,
        subscription: String,
        ttl: u32,
        payload: Vec<u8>,
    },
    Attest {
        ch: u32,
        attestation: Vec<u8>,
    },
    /// An optional extension this version does not know; safe to skip.
    Unknown {
        ty: u8,
        ch: u32,
        payload: Vec<u8>,
    },
}

/// The longest subscription id in a `PUSH_SEND`.
pub const MAX_SUBSCRIPTION_ID_LEN: usize = 128;

impl Frame {
    pub fn encode(&self) -> Result<Vec<u8>, DecodeError> {
        self.to_raw()?.encode()
    }

    pub fn decode(buf: &[u8]) -> Result<Frame, DecodeError> {
        Self::from_raw(RawFrame::decode(buf)?)
    }

    pub fn to_raw(&self) -> Result<RawFrame, DecodeError> {
        let mut p = Vec::new();
        let (ty, ch) = match self {
            Frame::Open { ch, peer } => {
                check_len("peer id", peer.len(), MAX_PEER_ID_LEN)?;
                put_str8(&mut p, peer);
                (FrameType::Open as u8, *ch)
            }
            Frame::Data { ch, payload } => {
                p.extend_from_slice(payload);
                (FrameType::Data as u8, *ch)
            }
            Frame::Close { ch, code, reason } => {
                check_len("close reason", reason.len(), MAX_CLOSE_REASON_LEN)?;
                p.extend_from_slice(&code.to_be_bytes());
                p.extend_from_slice(reason.as_bytes());
                (FrameType::Close as u8, *ch)
            }
            Frame::Credit { ch, bytes } => {
                p.extend_from_slice(&bytes.to_be_bytes());
                (FrameType::Credit as u8, *ch)
            }
            Frame::Ping { nonce } => {
                p.extend_from_slice(&nonce.to_be_bytes());
                (FrameType::Ping as u8, 0)
            }
            Frame::Pong { nonce } => {
                p.extend_from_slice(&nonce.to_be_bytes());
                (FrameType::Pong as u8, 0)
            }
            Frame::Append {
                ch,
                stream,
                expect_seq,
                records,
            } => {
                p.extend_from_slice(&stream.0);
                p.extend_from_slice(&expect_seq.to_be_bytes());
                put_records(&mut p, records)?;
                (FrameType::Append as u8, *ch)
            }
            Frame::Ack { ch, stream, head } => {
                p.extend_from_slice(&stream.0);
                p.extend_from_slice(&head.to_be_bytes());
                (FrameType::Ack as u8, *ch)
            }
            Frame::Conflict { ch, stream, head } => {
                p.extend_from_slice(&stream.0);
                p.extend_from_slice(&head.to_be_bytes());
                (FrameType::Conflict as u8, *ch)
            }
            Frame::Sub { ch, stream, from_seq } => {
                p.extend_from_slice(&stream.0);
                p.extend_from_slice(&from_seq.to_be_bytes());
                (FrameType::Sub as u8, *ch)
            }
            Frame::Recs { ch, stream, records } => {
                p.extend_from_slice(&stream.0);
                put_records(&mut p, records)?;
                (FrameType::Recs as u8, *ch)
            }
            Frame::Gone { ch, stream, first_seq } => {
                p.extend_from_slice(&stream.0);
                p.extend_from_slice(&first_seq.to_be_bytes());
                (FrameType::Gone as u8, *ch)
            }
            Frame::Checkpoint {
                ch,
                stream,
                upto_seq,
                manifest,
            } => {
                p.extend_from_slice(&stream.0);
                p.extend_from_slice(&upto_seq.to_be_bytes());
                p.extend_from_slice(manifest);
                (FrameType::Checkpoint as u8, *ch)
            }
            Frame::Ephemeral { ch, stream, payload } => {
                p.extend_from_slice(&stream.0);
                p.extend_from_slice(payload);
                (FrameType::Ephemeral as u8, *ch)
            }
            Frame::Chain { ch, entries } => {
                p.extend_from_slice(&u16::try_from(entries.len()).map_err(|_| too_many("chain entries"))?.to_be_bytes());
                for entry in entries {
                    p.extend_from_slice(&u16::try_from(entry.len()).map_err(|_| too_many("chain entry"))?.to_be_bytes());
                    p.extend_from_slice(entry);
                }
                (FrameType::Chain as u8, *ch)
            }
            Frame::PushSend {
                ch,
                subscription,
                ttl,
                payload,
            } => {
                check_len("subscription id", subscription.len(), MAX_SUBSCRIPTION_ID_LEN)?;
                put_str8(&mut p, subscription);
                p.extend_from_slice(&ttl.to_be_bytes());
                p.extend_from_slice(payload);
                (FrameType::PushSend as u8, *ch)
            }
            Frame::Head { ch, head } => {
                p.extend_from_slice(head);
                (FrameType::Head as u8, *ch)
            }
            Frame::Page { ch, stream, hash, data } => {
                p.extend_from_slice(&stream.0);
                p.extend_from_slice(hash);
                p.extend_from_slice(data);
                (FrameType::Page as u8, *ch)
            }
            Frame::Attest { ch, attestation } => {
                p.extend_from_slice(attestation);
                (FrameType::Attest as u8, *ch)
            }
            Frame::Unknown { ty, ch, payload } => {
                if *ty < IGNORABLE_TYPES {
                    return Err(DecodeError::Unknown {
                        what: "critical frame type",
                        value: *ty,
                    });
                }
                p.extend_from_slice(payload);
                (*ty, *ch)
            }
        };
        let raw = RawFrame { ty, ch, payload: p };
        if raw.payload.len() > MAX_PAYLOAD_LEN {
            return Err(DecodeError::TooLarge {
                what: "frame payload",
                len: raw.payload.len(),
                max: MAX_PAYLOAD_LEN,
            });
        }
        Ok(raw)
    }

    pub fn from_raw(raw: RawFrame) -> Result<Frame, DecodeError> {
        let Some(ty) = FrameType::from_u8(raw.ty) else {
            if raw.ty >= IGNORABLE_TYPES {
                return Ok(Frame::Unknown {
                    ty: raw.ty,
                    ch: raw.ch,
                    payload: raw.payload,
                });
            }
            return Err(DecodeError::Unknown {
                what: "critical frame type",
                value: raw.ty,
            });
        };
        let ch = raw.ch;
        let mut r = Reader::new(&raw.payload);
        let frame = match ty {
            FrameType::Open => {
                let peer = get_str8(&mut r, "peer id", MAX_PEER_ID_LEN)?;
                Frame::Open { ch, peer }
            }
            FrameType::Data => Frame::Data {
                ch,
                payload: r.rest().to_vec(),
            },
            FrameType::Close => {
                let code = r.u16()?;
                let reason = r.rest();
                check_len("close reason", reason.len(), MAX_CLOSE_REASON_LEN)?;
                let reason = String::from_utf8(reason.to_vec()).map_err(|_| DecodeError::BadUtf8)?;
                Frame::Close { ch, code, reason }
            }
            FrameType::Credit => Frame::Credit { ch, bytes: r.u32()? },
            FrameType::Ping | FrameType::Pong => {
                if ch != 0 {
                    return Err(DecodeError::Invalid("ping on a channel other than 0"));
                }
                let nonce = r.u64()?;
                if ty == FrameType::Ping {
                    Frame::Ping { nonce }
                } else {
                    Frame::Pong { nonce }
                }
            }
            FrameType::Append => {
                let stream = StreamId(r.array()?);
                let expect_seq = r.u64()?;
                let records = get_records(&mut r)?;
                for (i, record) in records.iter().enumerate() {
                    // Checked: a hostile APPEND at u64::MAX must be a refusal, not
                    // a panic (debug) or a wrap-around (release) in the hub.
                    if Some(record.seq) != expect_seq.checked_add(i as u64) {
                        return Err(DecodeError::Invalid("append records that are not consecutive from expect_seq"));
                    }
                }
                Frame::Append {
                    ch,
                    stream,
                    expect_seq,
                    records,
                }
            }
            FrameType::Ack => Frame::Ack {
                ch,
                stream: StreamId(r.array()?),
                head: r.u64()?,
            },
            FrameType::Conflict => Frame::Conflict {
                ch,
                stream: StreamId(r.array()?),
                head: r.u64()?,
            },
            FrameType::Sub => Frame::Sub {
                ch,
                stream: StreamId(r.array()?),
                from_seq: r.u64()?,
            },
            FrameType::Recs => {
                let stream = StreamId(r.array()?);
                Frame::Recs {
                    ch,
                    stream,
                    records: get_records(&mut r)?,
                }
            }
            FrameType::Gone => Frame::Gone {
                ch,
                stream: StreamId(r.array()?),
                first_seq: r.u64()?,
            },
            FrameType::Checkpoint => {
                let stream = StreamId(r.array()?);
                let upto_seq = r.u64()?;
                Frame::Checkpoint {
                    ch,
                    stream,
                    upto_seq,
                    manifest: r.rest().to_vec(),
                }
            }
            FrameType::Ephemeral => {
                let stream = StreamId(r.array()?);
                Frame::Ephemeral {
                    ch,
                    stream,
                    payload: r.rest().to_vec(),
                }
            }
            FrameType::Head => Frame::Head { ch, head: r.rest().to_vec() },
            FrameType::Page => Frame::Page {
                ch,
                stream: StreamId(r.array()?),
                hash: r.array()?,
                data: r.rest().to_vec(),
            },
            FrameType::Attest => Frame::Attest {
                ch,
                attestation: r.rest().to_vec(),
            },
            FrameType::Chain => {
                let count = r.u16()?;
                let mut entries = Vec::with_capacity(usize::from(count).min(256));
                for _ in 0..count {
                    let len = usize::from(r.u16()?);
                    entries.push(r.bytes(len)?.to_vec());
                }
                Frame::Chain { ch, entries }
            }
            FrameType::PushSend => {
                let subscription = get_str8(&mut r, "subscription id", MAX_SUBSCRIPTION_ID_LEN)?;
                let ttl = r.u32()?;
                Frame::PushSend {
                    ch,
                    subscription,
                    ttl,
                    payload: r.rest().to_vec(),
                }
            }
        };
        r.finish()?;
        Ok(frame)
    }
}

fn check_len(what: &'static str, len: usize, max: usize) -> Result<(), DecodeError> {
    if len > max {
        Err(DecodeError::TooLarge { what, len, max })
    } else {
        Ok(())
    }
}

fn too_many(what: &'static str) -> DecodeError {
    DecodeError::TooLarge {
        what,
        len: usize::MAX,
        max: usize::from(u16::MAX),
    }
}

fn put_str8(out: &mut Vec<u8>, value: &str) {
    out.push(value.len() as u8);
    out.extend_from_slice(value.as_bytes());
}

fn get_str8(r: &mut Reader<'_>, what: &'static str, max: usize) -> Result<String, DecodeError> {
    let len = usize::from(r.u8()?);
    check_len(what, len, max)?;
    String::from_utf8(r.bytes(len)?.to_vec()).map_err(|_| DecodeError::BadUtf8)
}

fn put_records(out: &mut Vec<u8>, records: &[SealedRecord]) -> Result<(), DecodeError> {
    out.extend_from_slice(&u16::try_from(records.len()).map_err(|_| too_many("records"))?.to_be_bytes());
    for record in records {
        record.encode_into(out);
    }
    Ok(())
}

fn get_records(r: &mut Reader<'_>) -> Result<Vec<SealedRecord>, DecodeError> {
    let count = r.u16()?;
    let mut records = Vec::with_capacity(usize::from(count).min(64));
    for _ in 0..count {
        records.push(SealedRecord::decode_from(r)?);
    }
    Ok(records)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn stream() -> StreamId {
        StreamId([7; 32])
    }

    fn record(seq: u64) -> SealedRecord {
        SealedRecord {
            seq,
            epoch: 2,
            nonce: [9; 12],
            ct: vec![1; 20],
        }
    }

    pub(crate) fn every_kind() -> Vec<Frame> {
        vec![
            Frame::Open {
                ch: 7,
                peer: "m-0001".into(),
            },
            Frame::Data {
                ch: 7,
                payload: vec![0xde, 0xad, 0xbe, 0xef],
            },
            Frame::Close {
                ch: 7,
                code: 1000,
                reason: "bye".into(),
            },
            Frame::Credit { ch: 7, bytes: 262_144 },
            Frame::Ping { nonce: 42 },
            Frame::Pong { nonce: 42 },
            Frame::Append {
                ch: 9,
                stream: stream(),
                expect_seq: 5,
                records: vec![record(5), record(6)],
            },
            Frame::Ack {
                ch: 9,
                stream: stream(),
                head: 7,
            },
            Frame::Conflict {
                ch: 9,
                stream: stream(),
                head: 6,
            },
            Frame::Sub {
                ch: 10,
                stream: stream(),
                from_seq: 0,
            },
            Frame::Recs {
                ch: 10,
                stream: stream(),
                records: vec![record(0)],
            },
            Frame::Gone {
                ch: 10,
                stream: stream(),
                first_seq: 100,
            },
            Frame::Checkpoint {
                ch: 11,
                stream: stream(),
                upto_seq: 99,
                manifest: vec![5; 10],
            },
            Frame::Ephemeral {
                ch: 12,
                stream: stream(),
                payload: vec![6; 3],
            },
            Frame::Chain {
                ch: 13,
                entries: vec![vec![1; 5], vec![2; 3]],
            },
            Frame::PushSend {
                ch: 14,
                subscription: "sub-1".into(),
                ttl: 60,
                payload: vec![3; 8],
            },
            Frame::Head { ch: 15, head: vec![4; 148] },
            Frame::Page {
                ch: 17,
                stream: stream(),
                hash: [8; 32],
                data: vec![2; 40],
            },
            Frame::Attest {
                ch: 16,
                attestation: vec![5; 20],
            },
            Frame::Unknown {
                ty: 0x90,
                ch: 1,
                payload: vec![1],
            },
        ]
    }

    #[test]
    fn every_frame_kind_round_trips() {
        for frame in every_kind() {
            let bytes = frame.encode().expect("encode");
            assert_eq!(Frame::decode(&bytes).expect("decode"), frame);
        }
    }

    #[test]
    fn the_size_limit_is_enforced_both_ways() {
        let max = Frame::Data {
            ch: 1,
            payload: vec![0; MAX_PAYLOAD_LEN],
        }
        .encode()
        .expect("exactly the limit");
        assert_eq!(max.len(), MAX_FRAME_LEN);
        assert!(Frame::Data {
            ch: 1,
            payload: vec![0; MAX_PAYLOAD_LEN + 1]
        }
        .encode()
        .is_err());
        // A header that claims more than the limit is refused before buffering it.
        let mut header = vec![FrameType::Data as u8, 0, 0, 0, 1];
        header.extend_from_slice(&u16::MAX.to_be_bytes());
        assert!(matches!(RawFrame::decode_prefix(&header), Err(DecodeError::TooLarge { .. })));
    }

    #[test]
    fn unknown_types_split_on_the_high_bit() {
        assert!(matches!(
            Frame::decode(
                &RawFrame {
                    ty: 0x7f,
                    ch: 0,
                    payload: vec![]
                }
                .encode()
                .unwrap()
            ),
            Err(DecodeError::Unknown { .. })
        ));
        assert!(matches!(
            Frame::decode(
                &RawFrame {
                    ty: 0x80,
                    ch: 0,
                    payload: vec![]
                }
                .encode()
                .unwrap()
            ),
            Ok(Frame::Unknown { .. })
        ));
    }

    #[test]
    fn malformed_bodies_are_refused() {
        // Trailing junk after an ACK body.
        let mut ack = Frame::Ack {
            ch: 1,
            stream: stream(),
            head: 3,
        }
        .to_raw()
        .unwrap();
        ack.payload.push(0);
        assert_eq!(Frame::from_raw(ack), Err(DecodeError::TrailingBytes(1)));
        // A ping on a data channel.
        let ping = RawFrame {
            ty: FrameType::Ping as u8,
            ch: 3,
            payload: 1u64.to_be_bytes().to_vec(),
        };
        assert!(Frame::from_raw(ping).is_err());
        // Append records that skip a seq: the hub must not have to trust the CAS body.
        let gap = Frame::Append {
            ch: 1,
            stream: stream(),
            expect_seq: 5,
            records: vec![record(5), record(7)],
        }
        .encode()
        .unwrap();
        assert!(Frame::decode(&gap).is_err());
        // A truncated frame is not a frame.
        let bytes = Frame::Credit { ch: 1, bytes: 1 }.encode().unwrap();
        assert_eq!(Frame::decode(&bytes[..bytes.len() - 1]), Err(DecodeError::Truncated));
    }

    #[test]
    fn the_stream_decoder_reassembles_frames_split_anywhere() {
        let frames = every_kind();
        let wire: Vec<u8> = frames.iter().flat_map(|f| f.encode().unwrap()).collect();
        for chunk in [1, 3, 7, 64, wire.len()] {
            let mut decoder = FrameDecoder::new();
            let mut out = Vec::new();
            for piece in wire.chunks(chunk) {
                decoder.push(piece);
                while let Some(raw) = decoder.next_frame().unwrap() {
                    out.push(Frame::from_raw(raw).unwrap());
                }
            }
            assert_eq!(out, frames, "chunk size {chunk}");
        }
    }
}
