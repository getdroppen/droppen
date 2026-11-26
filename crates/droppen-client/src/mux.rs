//! A websocket route over a channel's `mux` sub-stream: the feed socket,
//! `/api/events?v=2` — the one socket on which a session's live frames ride,
//! many sessions at once ([`crate::rpc`]'s sibling).
//!
//! The agent bridges a `mux` pipe to the matching websocket route
//! (`droppen_proto::bridge`, `web/src/lib/transport/tunnel.ts`): the pipe
//! opens with the route alone, the agent answers one `head`, and status 101
//! is the socket open; any other status — or a close before it — is a refused
//! handshake. After it, every message either way is one [`Kind::Text`] or
//! [`Kind::Binary`] message, and closing the pipe closes the socket.
//!
//! [`MuxPipe`] is deliberately thin: [`MuxPipe::open`] and
//! [`MuxPipe::send_text`] write, [`MuxPipe::next`] reads the next message as
//! it comes, and [`MuxPipe::close`] (or dropping it) ends it. What the text
//! means — `hello`, then `sub` frames the server tags with `sub` — is the
//! caller's.

use std::sync::Arc;

use droppen_proto::bridge::{self, Kind, MessageReader, RequestHead, ResponseHead, CLOSE_BAD_REQUEST, CLOSE_DONE, CLOSE_FORBIDDEN};
use droppen_proto::driver::{Stream, StreamEvent};
use droppen_proto::mux::StreamClass;

use crate::channel::HostChannel;
use crate::failure::{Failure, Refuser};

/// A `mux` pipe, open or ended.
pub struct MuxPipe {
    stream: Stream,
    reader: MessageReader,
    channel: Arc<HostChannel>,
    done: bool,
}

impl Drop for MuxPipe {
    fn drop(&mut self) {
        // The reader walked away: so does the socket.
        if !self.done {
            self.stream.close(CLOSE_DONE);
        }
    }
}

/// Open `path` (the feed route, query and all) on `channel`: the agent's
/// `head` with status 101, or why it would not open.
pub async fn open(channel: Arc<HostChannel>, path: &str) -> Result<MuxPipe, Failure> {
    let head = RequestHead::get(path)
        .encode()
        .map_err(|e| Failure::Other(format!("the request head: {e}")))?;
    let mut stream = channel.open_stream(StreamClass::Mux, &head).await?;
    let mut reader = MessageReader::new();
    loop {
        match reader.next_message() {
            Ok(Some((Kind::Head, payload))) => {
                let head = ResponseHead::decode(&payload).map_err(|e| Failure::Other(format!("the answer's head: {e}")))?;
                if head.status == 101 {
                    return Ok(MuxPipe {
                        stream,
                        reader,
                        channel,
                        done: false,
                    });
                }
                stream.close(CLOSE_DONE);
                return Err(Failure::Refused {
                    by: Refuser::Bridge,
                    code: head.status,
                    why: format!("the device refused the socket ({})", head.status),
                });
            }
            Ok(Some((Kind::End, _))) => continue,
            Ok(Some((Kind::Text | Kind::Binary, _))) => {
                stream.close(CLOSE_BAD_REQUEST);
                return Err(Failure::Other("the socket answered before its head".into()));
            }
            Ok(None) => {}
            Err(e) => {
                stream.close(CLOSE_BAD_REQUEST);
                return Err(Failure::Other(format!("the answer: {e}")));
            }
        }
        match stream.recv().await {
            Some(StreamEvent::Data(data)) => reader.push(&data),
            Some(StreamEvent::Closed(code)) => return Err(closed_failure(&channel, Some(code)).await),
            None => return Err(closed_failure(&channel, None).await),
        }
    }
}

impl MuxPipe {
    /// One websocket message up the socket.
    pub async fn send_text(&self, text: &str) -> Result<(), Failure> {
        self.stream
            .send(&bridge::message(Kind::Text, text.as_bytes()))
            .await
            .map_err(|e| Failure::Offline(format!("the socket: {e}")))
    }

    /// The next message down the socket; `None` once it ended cleanly (closed
    /// with code 0), an error when it ended any other way.
    pub async fn next(&mut self) -> Option<Result<Vec<u8>, Failure>> {
        if self.done {
            return None;
        }
        loop {
            match self.reader.next_message() {
                Ok(Some((Kind::Text | Kind::Binary, payload))) => return Some(Ok(payload)),
                Ok(Some((Kind::Head | Kind::End, _))) => continue,
                Ok(None) => {}
                Err(e) => {
                    self.done = true;
                    self.stream.close(CLOSE_BAD_REQUEST);
                    return Some(Err(Failure::Other(format!("the answer: {e}"))));
                }
            }
            match self.stream.recv().await {
                Some(StreamEvent::Data(data)) => self.reader.push(&data),
                Some(StreamEvent::Closed(CLOSE_DONE)) => {
                    self.done = true;
                    return None;
                }
                Some(StreamEvent::Closed(code)) => {
                    self.done = true;
                    return Some(Err(closed_failure(&self.channel, Some(code)).await));
                }
                None => {
                    self.done = true;
                    return Some(Err(closed_failure(&self.channel, None).await));
                }
            }
        }
    }

    /// Close the socket (what was sent before it still goes).
    pub fn close(&self, code: u16) {
        self.stream.close(code);
    }
}

/// Why the pipe closed (`code`, or `None` when the channel's task went): the
/// channel's own end first, since its streams close with its code.
async fn closed_failure(channel: &HostChannel, code: Option<u16>) -> Failure {
    if let Some(ended) = channel.ended() {
        return ended;
    }
    if !channel.is_open() || code.is_none() {
        if let Ok(ended) = tokio::time::timeout(std::time::Duration::from_millis(200), channel.wait_ended()).await {
            return ended;
        }
    }
    match code {
        Some(CLOSE_FORBIDDEN) => Failure::Refused {
            by: Refuser::Bridge,
            code: CLOSE_FORBIDDEN,
            why: "the device does not serve that socket to this device".into(),
        },
        Some(CLOSE_BAD_REQUEST) => Failure::Other("the device refused the socket as malformed".into()),
        Some(code) => Failure::Offline(format!("the device's socket did not answer ({code})")),
        None => Failure::Offline("the channel ended".into()),
    }
}
