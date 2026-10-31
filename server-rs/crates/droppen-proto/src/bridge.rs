//! What travels on a sub-stream when the agent bridges it to its own loopback
//! API (architecture doc §5, "MVP 的 agent 侧桥接"). End-to-end encrypted like
//! everything in a channel; the hub never sees any of it.
//!
//! # Opening
//!
//! A stream's `OPEN` metadata is a [`RequestHead`]: method, path (with query)
//! and headers. What the agent does with it depends on the stream class:
//!
//! - `rpc` — one HTTP request to `/api/*`. The device sends the body as
//!   [`Kind::Binary`] messages, then [`Kind::End`]. The agent answers with one
//!   [`Kind::Head`] ([`ResponseHead`]), the body as [`Kind::Binary`] messages,
//!   and closes the stream with code 0.
//! - `mux` — the feed socket, `/api/events?v=2&z=1`; `term` — a terminal,
//!   `/api/terminals/{id}/ws`. Each websocket message either way is one
//!   [`Kind::Text`] or [`Kind::Binary`] message; closing the stream closes the
//!   socket and the other way round.
//!
//! # Messages
//!
//! The stream's bytes are a sequence of `[kind u8][len u32][payload]`, so a
//! websocket message or a response head keeps its boundaries however the
//! multiplexer chunks the stream ([`MessageReader`]).
//!
//! # Close codes
//!
//! `0` done; [`CLOSE_BAD_REQUEST`], [`CLOSE_FORBIDDEN`] (a path the class may
//! not reach), [`CLOSE_UPSTREAM`] (the loopback request or socket failed).

use crate::codec::{put_str16, DecodeError, Reader};

pub const CLOSE_DONE: u16 = 0;
pub const CLOSE_BAD_REQUEST: u16 = 4400;
pub const CLOSE_FORBIDDEN: u16 = 4403;
pub const CLOSE_UPSTREAM: u16 = 4502;

/// The largest single message (a feed snapshot can be a few hundred KiB).
pub const MAX_MESSAGE: usize = 32 * 1024 * 1024;
/// The most headers a head carries.
pub const MAX_HEADERS: usize = 64;
const MAX_FIELD: usize = 4096;
const HEAD_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Text = 1,
    Binary = 2,
    /// An rpc response's [`ResponseHead`].
    Head = 3,
    /// The end of an rpc request's body.
    End = 4,
}

impl Kind {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            1 => Kind::Text,
            2 => Kind::Binary,
            3 => Kind::Head,
            4 => Kind::End,
            _ => return None,
        })
    }
}

/// `u8 version | str16 method | str16 path | u16 n | (str16 name | str16 value)×n`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestHead {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
}

/// `u8 version | u16 status | u16 n | (str16 name | str16 value)×n`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseHead {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

fn put_headers(out: &mut Vec<u8>, headers: &[(String, String)]) -> Result<(), DecodeError> {
    if headers.len() > MAX_HEADERS {
        return Err(DecodeError::TooLarge {
            what: "headers",
            len: headers.len(),
            max: MAX_HEADERS,
        });
    }
    out.extend_from_slice(&(headers.len() as u16).to_be_bytes());
    for (name, value) in headers {
        check_field(name)?;
        check_field(value)?;
        put_str16(out, name);
        put_str16(out, value);
    }
    Ok(())
}

fn get_headers(r: &mut Reader<'_>) -> Result<Vec<(String, String)>, DecodeError> {
    let n = usize::from(r.u16()?);
    if n > MAX_HEADERS {
        return Err(DecodeError::TooLarge {
            what: "headers",
            len: n,
            max: MAX_HEADERS,
        });
    }
    (0..n)
        .map(|_| Ok((r.str16("header name", MAX_FIELD)?, r.str16("header value", MAX_FIELD)?)))
        .collect()
}

fn check_field(value: &str) -> Result<(), DecodeError> {
    if value.len() > MAX_FIELD {
        return Err(DecodeError::TooLarge {
            what: "head field",
            len: value.len(),
            max: MAX_FIELD,
        });
    }
    Ok(())
}

impl RequestHead {
    pub fn get(path: &str) -> Self {
        Self {
            method: "GET".into(),
            path: path.into(),
            headers: Vec::new(),
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, DecodeError> {
        check_field(&self.method)?;
        check_field(&self.path)?;
        let mut out = vec![HEAD_VERSION];
        put_str16(&mut out, &self.method);
        put_str16(&mut out, &self.path);
        put_headers(&mut out, &self.headers)?;
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        if r.u8()? != HEAD_VERSION {
            return Err(DecodeError::Invalid("request head version"));
        }
        let head = Self {
            method: r.str16("method", MAX_FIELD)?,
            path: r.str16("path", MAX_FIELD)?,
            headers: get_headers(&mut r)?,
        };
        r.finish()?;
        Ok(head)
    }
}

impl ResponseHead {
    pub fn encode(&self) -> Result<Vec<u8>, DecodeError> {
        let mut out = vec![HEAD_VERSION];
        out.extend_from_slice(&self.status.to_be_bytes());
        put_headers(&mut out, &self.headers)?;
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        if r.u8()? != HEAD_VERSION {
            return Err(DecodeError::Invalid("response head version"));
        }
        let head = Self {
            status: r.u16()?,
            headers: get_headers(&mut r)?,
        };
        r.finish()?;
        Ok(head)
    }
}

/// One message, framed for a stream.
pub fn message(kind: Kind, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + payload.len());
    out.push(kind as u8);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Reassembles messages from a stream's data, however it was chunked.
#[derive(Debug, Default)]
pub struct MessageReader {
    buf: Vec<u8>,
}

impl MessageReader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// The next whole message. An error poisons the stream: close it.
    pub fn next_message(&mut self) -> Result<Option<(Kind, Vec<u8>)>, DecodeError> {
        if self.buf.len() < 5 {
            return Ok(None);
        }
        let kind = Kind::from_u8(self.buf[0]).ok_or(DecodeError::Unknown {
            what: "bridge message kind",
            value: self.buf[0],
        })?;
        let len = u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
        if len > MAX_MESSAGE {
            return Err(DecodeError::TooLarge {
                what: "bridge message",
                len,
                max: MAX_MESSAGE,
            });
        }
        if self.buf.len() < 5 + len {
            return Ok(None);
        }
        let payload = self.buf[5..5 + len].to_vec();
        self.buf.drain(..5 + len);
        Ok(Some((kind, payload)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heads_round_trip_and_refuse_junk() {
        let req = RequestHead {
            method: "POST".into(),
            path: "/api/terminals?x=1".into(),
            headers: vec![("content-type".into(), "application/json".into())],
        };
        assert_eq!(RequestHead::decode(&req.encode().unwrap()).unwrap(), req);
        let res = ResponseHead {
            status: 201,
            headers: vec![("etag".into(), "\"x\"".into())],
        };
        assert_eq!(ResponseHead::decode(&res.encode().unwrap()).unwrap(), res);
        let mut junk = res.encode().unwrap();
        junk.push(0);
        assert!(ResponseHead::decode(&junk).is_err());
        assert!(RequestHead::decode(&[2, 0, 0]).is_err());
    }

    #[test]
    fn messages_survive_any_chunking() {
        let wire = [
            message(Kind::Text, b"hello"),
            message(Kind::Binary, &[0; 70_000]),
            message(Kind::End, b""),
        ]
        .concat();
        for chunk in [1, 3, 4096, wire.len()] {
            let mut r = MessageReader::new();
            let mut got = Vec::new();
            for piece in wire.chunks(chunk) {
                r.push(piece);
                while let Some(m) = r.next_message().unwrap() {
                    got.push(m);
                }
            }
            assert_eq!(got.len(), 3, "chunk {chunk}");
            assert_eq!(got[0], (Kind::Text, b"hello".to_vec()));
            assert_eq!(got[1].1.len(), 70_000);
            assert_eq!(got[2], (Kind::End, vec![]));
        }
        let mut bad = MessageReader::new();
        bad.push(&[9, 0, 0, 0, 0]);
        assert!(bad.next_message().is_err());
    }
}
