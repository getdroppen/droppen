//! HTTP requests over a channel's `rpc` sub-streams ("pipes"), answered as
//! they come: the response head as soon as it arrives, the body as a stream —
//! an SSE answer can run for hours.
//!
//! The framing is the agent bridge's (`droppen_proto::bridge`, and
//! `web/src/lib/transport/tunnel.ts`, which this mirrors): the pipe opens with
//! the request head (method, path and query, headers); the request body goes
//! up as `binary` messages, then `end` (the agent reads it whole, up to
//! 64 MiB, before it answers). The agent answers with one `head` (status and
//! headers), the body as `binary` messages, and closes the pipe with code 0.
//! A pipe that closes with any other code, or before the head, is a request
//! that failed.
//!
//! The body is taken from the pipe only as it is read
//! ([`RpcBody::chunk`]), and taking it is what grants the agent credit to
//! send more: a slow reader slows the agent, nothing piles up here. Dropping
//! an unfinished body closes the pipe, so the agent stops too.

use std::sync::Arc;

use droppen_proto::bridge::{self, Kind, MessageReader, RequestHead, ResponseHead, CLOSE_BAD_REQUEST, CLOSE_DONE, CLOSE_FORBIDDEN};
use droppen_proto::driver::{Stream, StreamEvent};
use droppen_proto::mux::StreamClass;
use serde_json::Value;

use crate::channel::HostChannel;
use crate::failure::{Failure, Refuser};

/// The most of a request body sent as one message.
const BODY_MESSAGE: usize = 256 * 1024;
/// The most of a request body the agent takes (`droppen-agent`'s bridge).
pub const MAX_REQUEST_BODY: usize = 64 * 1024 * 1024;

/// A request for the host's loopback server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcRequest {
    pub method: String,
    /// Path and query, e.g. `/a2a/agents/main/rest/tasks?pageSize=5`.
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// The answer: its head, and its body still to come.
pub struct RpcResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: RpcBody,
}

impl std::fmt::Debug for RpcResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcResponse")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .finish_non_exhaustive()
    }
}

impl RpcResponse {
    /// The first value of header `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// Why [`send`] failed, and whether any of the request left first.
pub(crate) enum RpcError {
    /// The pipe did not open: nothing reached the host, and the request may
    /// go again on another channel.
    NotSent(Failure),
    Failed(Failure),
}

impl RpcError {
    pub(crate) fn failure(self) -> Failure {
        match self {
            RpcError::NotSent(f) | RpcError::Failed(f) => f,
        }
    }
}

/// Send `request` on a new stream of `class` (`rpc`, or `blob` for file
/// bytes) and wait for the answer's head.
pub async fn rpc(channel: Arc<HostChannel>, class: StreamClass, request: RpcRequest) -> Result<RpcResponse, Failure> {
    send(channel, class, request).await.map_err(RpcError::failure)
}

pub(crate) async fn send(channel: Arc<HostChannel>, class: StreamClass, request: RpcRequest) -> Result<RpcResponse, RpcError> {
    if request.body.len() > MAX_REQUEST_BODY {
        return Err(RpcError::NotSent(Failure::Other(format!(
            "the request body is larger than the {MAX_REQUEST_BODY} bytes a device takes"
        ))));
    }
    let head = RequestHead {
        method: request.method,
        path: request.path,
        headers: request.headers,
    }
    .encode()
    .map_err(|e| RpcError::NotSent(Failure::Other(format!("the request head: {e}"))))?;
    let stream = channel.open_stream(class, &head).await.map_err(RpcError::NotSent)?;
    let mut body = RpcBody {
        stream,
        reader: MessageReader::new(),
        channel,
        done: false,
    };
    let mut sent = Ok(());
    for piece in request.body.chunks(BODY_MESSAGE) {
        sent = body.stream.send(&bridge::message(Kind::Binary, piece)).await;
        if sent.is_err() {
            break;
        }
    }
    if sent.is_ok() {
        sent = body.stream.send(&bridge::message(Kind::End, b"")).await;
    }
    if sent.is_err() {
        // Closed under the request: by the host (its code is on the way), or
        // with the channel.
        body.done = true;
        let code = body.drain_close().await;
        return Err(RpcError::Failed(body.closed_failure(code).await));
    }
    loop {
        match body.reader.next_message() {
            Ok(Some((Kind::Head, payload))) => {
                let head = ResponseHead::decode(&payload).map_err(|e| RpcError::Failed(body.poisoned(format!("the answer's head: {e}"))))?;
                return Ok(RpcResponse {
                    status: head.status,
                    headers: head.headers,
                    body,
                });
            }
            Ok(Some((Kind::End, _))) => continue,
            Ok(Some((Kind::Binary | Kind::Text, _))) => return Err(RpcError::Failed(body.poisoned("the answer did not start with a head".into()))),
            Ok(None) => {}
            Err(e) => return Err(RpcError::Failed(body.poisoned(format!("the answer: {e}")))),
        }
        match body.stream.recv().await {
            Some(StreamEvent::Data(data)) => body.reader.push(&data),
            Some(StreamEvent::Closed(code)) => {
                body.done = true;
                return Err(RpcError::Failed(body.closed_failure(Some(code)).await));
            }
            None => {
                body.done = true;
                return Err(RpcError::Failed(body.closed_failure(None).await));
            }
        }
    }
}

/// An answer's body, read as it arrives.
pub struct RpcBody {
    stream: Stream,
    reader: MessageReader,
    channel: Arc<HostChannel>,
    done: bool,
}

impl Drop for RpcBody {
    fn drop(&mut self) {
        // The reader walked away: so does the request.
        if !self.done {
            self.stream.close(CLOSE_DONE);
        }
    }
}

impl RpcBody {
    /// The next piece of the body; `None` once it ended (the pipe closed with
    /// code 0); an error when it ended any other way — the channel or the
    /// relay connection under it went away, the host closed it.
    pub async fn chunk(&mut self) -> Option<Result<Vec<u8>, Failure>> {
        if self.done {
            return None;
        }
        loop {
            match self.reader.next_message() {
                Ok(Some((Kind::Binary | Kind::Text, payload))) => return Some(Ok(payload)),
                Ok(Some((Kind::Head | Kind::End, _))) => continue,
                Ok(None) => {}
                Err(e) => return Some(Err(self.poisoned(format!("the answer: {e}")))),
            }
            match self.stream.recv().await {
                Some(StreamEvent::Data(data)) => self.reader.push(&data),
                Some(StreamEvent::Closed(CLOSE_DONE)) => {
                    self.done = true;
                    return None;
                }
                Some(StreamEvent::Closed(code)) => {
                    self.done = true;
                    return Some(Err(self.closed_failure(Some(code)).await));
                }
                None => {
                    self.done = true;
                    return Some(Err(self.closed_failure(None).await));
                }
            }
        }
    }

    /// The whole body, up to `max` bytes (more is an error).
    pub async fn read_to_end(mut self, max: usize) -> Result<Vec<u8>, Failure> {
        let mut out = Vec::new();
        while let Some(piece) = self.chunk().await {
            out.extend_from_slice(&piece?);
            if out.len() > max {
                return Err(Failure::Other(format!("the answer is larger than {max} bytes")));
            }
        }
        Ok(out)
    }

    /// The body as a stream of pieces.
    pub fn into_stream(self) -> impl futures_util::Stream<Item = Result<Vec<u8>, Failure>> + Send + 'static {
        futures_util::stream::unfold(self, |mut body| async move { body.chunk().await.map(|piece| (piece, body)) })
    }

    /// The code the pipe closed with, what came before it dropped.
    async fn drain_close(&mut self) -> Option<u16> {
        loop {
            match self.stream.recv().await {
                Some(StreamEvent::Data(_)) => continue,
                Some(StreamEvent::Closed(code)) => return Some(code),
                None => return None,
            }
        }
    }

    /// A pipe that broke the framing: closed, and why.
    fn poisoned(&mut self, why: String) -> Failure {
        self.done = true;
        self.stream.close(CLOSE_BAD_REQUEST);
        Failure::Other(why)
    }

    /// Why the pipe closed (`code`, or `None` when the channel's task went):
    /// the channel's own end first, since its streams close with its code;
    /// then the bridge's code.
    async fn closed_failure(&self, code: Option<u16>) -> Failure {
        if let Some(ended) = self.channel.ended() {
            return ended;
        }
        // The channel's end is noted before its streams hear of it, but a
        // channel whose task failed on its own closes them first: a moment
        // for the note.
        if !self.channel.is_open() || code.is_none() {
            if let Ok(ended) = tokio::time::timeout(std::time::Duration::from_millis(200), self.channel.wait_ended()).await {
                return ended;
            }
        }
        match code {
            Some(CLOSE_FORBIDDEN) => Failure::Refused {
                by: Refuser::Bridge,
                code: CLOSE_FORBIDDEN,
                why: "the device does not serve that path to this device".into(),
            },
            Some(CLOSE_BAD_REQUEST) => Failure::Other("the device refused the request as malformed".into()),
            Some(CLOSE_DONE) => Failure::Other("the device closed the request before answering".into()),
            Some(code) => Failure::Offline(format!("the device's server did not answer ({code})")),
            None => Failure::Offline("the channel ended".into()),
        }
    }
}

/// The bridge's own refusal in an answer, if `status` and `body` are one:
/// `412 {"error": "stale", "reason": …}` — `lapsed` (the host takes no control
/// from this device now: stale, removed or no longer accepted) is
/// [`Failure::Lapsed`], the management routes' `stale`/`behind`/`fork`
/// [`Failure::Stale`]. Any other answer is the route's own.
pub fn refusal_of(status: u16, body: &[u8]) -> Option<Failure> {
    if status != 412 {
        return None;
    }
    let v: Value = serde_json::from_slice(body).ok()?;
    if v["error"] != "stale" {
        return None;
    }
    let message = v["message"].as_str().unwrap_or_default().to_string();
    match v["reason"].as_str() {
        Some("lapsed") => Some(Failure::Lapsed(message)),
        Some(_) => Some(Failure::Stale(message)),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_bridges_412_is_its_refusal() {
        let lapsed = br#"{"error":"stale","reason":"lapsed","message":"the device you reach takes no control"}"#;
        assert!(matches!(refusal_of(412, lapsed), Some(Failure::Lapsed(_))));
        assert!(matches!(
            refusal_of(412, br#"{"error":"stale","reason":"stale","message":"m"}"#),
            Some(Failure::Stale(_))
        ));
        assert_eq!(refusal_of(200, lapsed), None);
        assert_eq!(refusal_of(412, br#"{"error":"precondition"}"#), None);
        assert_eq!(refusal_of(412, b"not json"), None);
    }
}
