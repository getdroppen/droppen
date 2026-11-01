//! What every relaying connection shares, on the Hub (until it stops
//! relaying) and on `droppen-relay`: a sending half with a byte budget, and
//! the writer task that drains it into the socket.
//!
//! Each connection is a reader (the caller's loop) and a writer task. The
//! reader charges every inbound message to the connection's token buckets
//! ([`crate::limit`]) and simply waits when they run dry, which is
//! backpressure on the peer's socket. A peer that does not read what it is
//! sent is cut off by its [`Outbound`] budget instead of being buffered for
//! without bound.
//!
//! **Logging.** Never a payload, never a token, never a signature: the most a
//! log line says about a frame is its type, `ch` and length. `Frame`'s `Debug`
//! prints payload bytes, so frames are never logged with `?`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use futures_util::stream::SplitSink;
use futures_util::SinkExt;
use droppen_proto::frame::Frame;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// What a connection's writer sends.
#[derive(Debug)]
pub enum Out {
    Binary(Vec<u8>),
    Text(String),
    Close(u16, String),
}

/// The sending half of a connection, with a byte budget: a peer that does not
/// read what it is sent is cut off rather than buffered for without bound.
#[derive(Clone)]
pub struct Outbound {
    tx: mpsc::UnboundedSender<Out>,
    queued: Arc<AtomicUsize>,
    limit: usize,
}

impl Outbound {
    pub fn new(limit: usize) -> (Self, mpsc::UnboundedReceiver<Out>, Arc<AtomicUsize>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let queued = Arc::new(AtomicUsize::new(0));
        (
            Self {
                tx,
                queued: queued.clone(),
                limit,
            },
            rx,
            queued,
        )
    }

    fn push(&self, out: Out, len: usize) -> bool {
        if self.queued.load(Ordering::Relaxed) + len > self.limit {
            let _ = self.tx.send(Out::Close(1013, "not reading fast enough".into()));
            return false;
        }
        self.queued.fetch_add(len, Ordering::Relaxed);
        self.tx.send(out).is_ok()
    }

    pub fn frame(&self, frame: &Frame) -> bool {
        match frame.encode() {
            Ok(bytes) => {
                let len = bytes.len();
                self.push(Out::Binary(bytes), len)
            }
            Err(error) => {
                tracing::warn!(%error, "dropping a frame that does not encode");
                false
            }
        }
    }

    /// Send a control message: one JSON object, tagged by `t`.
    pub fn text<T: serde::Serialize>(&self, message: &T) -> bool {
        let text = serde_json::to_string(message).expect("control messages serialize");
        let len = text.len();
        self.push(Out::Text(text), len)
    }

    pub fn close(&self, code: u16, reason: &str) {
        let _ = self.tx.send(Out::Close(code, reason.into()));
    }

    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    /// Bytes queued and not yet written to the socket.
    pub fn queued(&self) -> usize {
        self.queued.load(Ordering::Relaxed)
    }

    /// The budget past which the peer is cut off.
    pub fn limit(&self) -> usize {
        self.limit
    }
}

/// Drain `rx` into `sink` until a close is sent, every sender is gone, or
/// `shutdown` fires (then the socket is closed with `shutdown_close`).
pub fn spawn_writer(
    mut sink: SplitSink<WebSocket, Message>,
    mut rx: mpsc::UnboundedReceiver<Out>,
    queued: Arc<AtomicUsize>,
    shutdown: CancellationToken,
    shutdown_close: (u16, &'static str),
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let item = tokio::select! {
                biased;
                item = rx.recv() => item,
                () = shutdown.cancelled() => Some(Out::Close(shutdown_close.0, shutdown_close.1.into())),
            };
            // Every sender gone without a close: the reader ended; say so plainly.
            let item = item.unwrap_or(Out::Close(1000, String::new()));
            let (message, len, last) = match item {
                Out::Binary(bytes) => {
                    let len = bytes.len();
                    (Message::Binary(bytes.into()), len, false)
                }
                Out::Text(text) => {
                    let len = text.len();
                    (Message::Text(text.into()), len, false)
                }
                Out::Close(code, reason) => (Message::Close(Some(CloseFrame { code, reason: reason.into() })), 0, true),
            };
            let sent = sink.send(message).await;
            queued.fetch_sub(len.min(queued.load(Ordering::Relaxed)), Ordering::Relaxed);
            if sent.is_err() || last {
                break;
            }
        }
        let _ = sink.close().await;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_outbound_queue_cuts_off_a_peer_that_does_not_read() {
        let (out, mut rx, queued) = Outbound::new(100);
        assert!(out.frame(&Frame::Ping { nonce: 1 }));
        assert_eq!(queued.load(Ordering::Relaxed), 15);
        assert!(!out.frame(&Frame::Data {
            ch: 1,
            payload: vec![0; 200]
        }));
        assert!(matches!(rx.try_recv(), Ok(Out::Binary(_))));
        assert!(matches!(rx.try_recv(), Ok(Out::Close(1013, _))));
    }

    #[test]
    fn text_is_the_messages_json() {
        #[derive(serde::Serialize)]
        struct M {
            t: &'static str,
            n: u32,
        }
        let (out, mut rx, _) = Outbound::new(1000);
        assert!(out.text(&M { t: "x", n: 1 }));
        match rx.try_recv() {
            Ok(Out::Text(t)) => assert_eq!(t, r#"{"t":"x","n":1}"#),
            other => panic!("{other:?}"),
        }
    }
}
