//! Droppen's relay (`docs/relay-and-plans.md` §2), `droppen-relay`: the one
//! place a device's end-to-end channel meets a machine's. A relay pairs an
//! `OPEN` from a device with the machine's connection, copies `DATA` between
//! the two without parsing it, and enforces the credit windows and the limits
//! that bound what any one peer can make it hold. It has no database and no
//! keys to anything: it checks the Hub's tickets offline, holds nothing a
//! restart would miss, and only ever sees ciphertext.
//!
//! - [`route`] — the routing table and credit, sans-IO;
//! - [`server`] — who is signed in on which ticket, revocations, throttles;
//! - [`client`] — a device's or an agent's connection (`/v1/connect`);
//! - [`http`] — `/healthz`, `/v1/ping`, `/v1/relay-id`, and the per-address limits;
//! - [`uplink`] — the control link to the Hub (`/v1/relay`): registration,
//!   heartbeats, usage, revocations;
//! - [`meter`] — transfer per account;
//! - [`conn`] — a connection's budgeted sending half and its writer task;
//! - [`limit`] — token buckets;
//! - [`config`] — the environment.

pub mod client;
pub mod config;
pub mod conn;
pub mod http;
pub mod limit;
pub mod meter;
pub mod route;
pub mod server;
pub mod traffic;
pub mod uplink;

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use droppen_proto::wire::relay::RelayToHub;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub use config::Config;
pub use server::Server;

/// Maximum time a newly accepted TCP connection may take to deliver its
/// initial HTTP request headers (mitigates Slowloris before axum/hyper parses
/// the request). Once `\r\n\r\n` (or `\n\n`) is seen, the stream becomes a
/// plain pass-through so long-lived WebSocket connections are unaffected.
const HTTP_HEADER_TIMEOUT: Duration = Duration::from_secs(10);

/// A `TcpListener` wrapper that bounds total concurrent TCP connections and
/// wraps each accepted socket in [`HeaderTimeoutStream`].
struct BoundedListener {
    inner: tokio::net::TcpListener,
    permits: Arc<Semaphore>,
    header_timeout: Duration,
}

impl BoundedListener {
    fn new(inner: tokio::net::TcpListener, max_tcp_conns: usize, header_timeout: Duration) -> Self {
        Self {
            inner,
            permits: Arc::new(Semaphore::new(max_tcp_conns.max(1))),
            header_timeout,
        }
    }
}

impl axum::serve::Listener for BoundedListener {
    type Io = HeaderTimeoutStream;
    type Addr = std::net::SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let permit = self
            .permits
            .clone()
            .acquire_owned()
            .await
            .expect("listener semaphore is never closed");
        loop {
            match self.inner.accept().await {
                Ok((stream, addr)) => {
                    let _ = stream.set_nodelay(true);
                    return (HeaderTimeoutStream::new(stream, permit, self.header_timeout), addr);
                }
                Err(err) if is_transient_accept_error(&err) => continue,
                Err(err) => {
                    tracing::warn!(%err, "accept error; backing off briefly");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

/// Peer socket address extracted by `axum::extract::ConnectInfo` from
/// [`BoundedListener`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct PeerAddr(pub std::net::SocketAddr);

impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, BoundedListener>> for PeerAddr {
    fn connect_info(stream: axum::serve::IncomingStream<'_, BoundedListener>) -> Self {
        Self(*stream.remote_addr())
    }
}

fn is_transient_accept_error(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::ConnectionRefused | io::ErrorKind::ConnectionAborted | io::ErrorKind::ConnectionReset
    )
}

/// Wraps a `TcpStream` to enforce a deadline until the initial HTTP header
/// terminator (`\r\n\r\n` or `\n\n`) is read, then passes reads through with
/// zero timer overhead. Also holds the listener's concurrency permit until drop.
struct HeaderTimeoutStream {
    stream: tokio::net::TcpStream,
    _permit: OwnedSemaphorePermit,
    deadline: Option<Pin<Box<tokio::time::Sleep>>>,
    tail: [u8; 3],
    tail_len: usize,
}

impl HeaderTimeoutStream {
    fn new(stream: tokio::net::TcpStream, permit: OwnedSemaphorePermit, timeout: Duration) -> Self {
        Self {
            stream,
            _permit: permit,
            deadline: Some(Box::pin(tokio::time::sleep(timeout))),
            tail: [0; 3],
            tail_len: 0,
        }
    }

    fn observe_bytes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() || self.deadline.is_none() {
            return;
        }
        // Check boundary spanning `self.tail[..self.tail_len]` + `bytes`.
        let mut window = [0u8; 7];
        let prefix = self.tail_len;
        window[..prefix].copy_from_slice(&self.tail[..prefix]);
        let take = bytes.len().min(4);
        window[prefix..prefix + take].copy_from_slice(&bytes[..take]);
        if has_header_end(&window[..prefix + take]) || has_header_end(bytes) {
            self.deadline = None;
            return;
        }
        // Keep up to the last 3 bytes for the next chunk boundary.
        let total = prefix + bytes.len();
        let keep = total.min(3);
        let mut next = [0u8; 3];
        if bytes.len() >= keep {
            next[..keep].copy_from_slice(&bytes[bytes.len() - keep..]);
        } else {
            let from_tail = keep - bytes.len();
            next[..from_tail].copy_from_slice(&self.tail[prefix - from_tail..prefix]);
            next[from_tail..keep].copy_from_slice(bytes);
        }
        self.tail = next;
        self.tail_len = keep;
    }
}

fn has_header_end(bytes: &[u8]) -> bool {
    bytes.windows(4).any(|w| w == b"\r\n\r\n") || bytes.windows(2).any(|w| w == b"\n\n")
}

impl AsyncRead for HeaderTimeoutStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Some(sleep) = this.deadline.as_mut() {
            if sleep.as_mut().poll(cx).is_ready() {
                return Poll::Ready(Err(io::Error::new(io::ErrorKind::TimedOut, "http header read timeout")));
            }
        }
        let before = buf.filled().len();
        let poll = Pin::new(&mut this.stream).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &poll {
            let filled = &buf.filled()[before..];
            this.observe_bytes(filled);
        }
        poll
    }
}

impl AsyncWrite for HeaderTimeoutStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}

/// Serve on `listener` until `stop` resolves or the Hub refuses this relay
/// for good; then `goaway` everyone, tell the Hub until when this relay
/// drains and close the control link, wait out the drain and close (1012).
/// `Err` carries a fatal refusal's reason.
pub async fn run(server: Arc<Server>, listener: tokio::net::TcpListener, stop: impl Future<Output = ()> + Send + 'static) -> Result<(), String> {
    server.load_saved_keys();
    let app = http::router(server.clone());
    let max_tcp = server.config.max_conns.saturating_add((server.config.max_conns / 4).clamp(16, 1024)).saturating_add(256);
    let listener = BoundedListener::new(listener, max_tcp, HTTP_HEADER_TIMEOUT);
    let http_stop = tokio_util::sync::CancellationToken::new();
    let serving = {
        let stopped = http_stop.clone();
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<PeerAddr>())
                .with_graceful_shutdown(async move { stopped.cancelled().await })
                .await
        })
    };
    // Usage windows close on time, whether or not the Hub is reachable.
    let cutter = server.clone();
    server.tasks.spawn(async move {
        let mut tick = tokio::time::interval(cutter.config.timing.usage_every);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => cutter.meter.cut(server::now_ms()),
                () = cutter.shutdown.cancelled() => break,
            }
        }
    });
    // The per-address limits forget each address once it is done counting,
    // a day after its last request at the latest (droppen.app/privacy).
    let forgetter = server.clone();
    server.tasks.spawn(async move {
        let mut tick = tokio::time::interval(limit::SWEEP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    forgetter.unauth.forget_old(Instant::now());
                }
                () = forgetter.shutdown.cancelled() => break,
            }
        }
    });
    // The control link is a task of its own, so that a `stop` leaves it up
    // for `drain` to tell the Hub on.
    let uplink = server.config.hub.is_some().then(|| server.tasks.spawn(uplink::run(server.clone())));
    let fatal = async move {
        match uplink {
            Some(link) => link.await.unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic())),
            None => std::future::pending().await,
        }
    };
    let result = tokio::select! {
        () = stop => Ok(()),
        reason = fatal => Err(reason),
    };
    drain(&server, &result).await;
    http_stop.cancel();
    server.shutdown.cancel();
    server.tasks.close();
    if tokio::time::timeout(Duration::from_secs(5), server.tasks.wait()).await.is_err() {
        tracing::warn!("some connections did not close in time");
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), serving).await;
    result
}

/// `goaway`, tell the Hub and close its control link, then wait (at most the
/// drain time) for clients to move on.
async fn drain(server: &Arc<Server>, why: &Result<(), String>) {
    let drain = server.config.drain;
    let reason = match why {
        Ok(()) => "restart",
        Err(_) => "stopping",
    };
    tracing::info!(drain_s = drain.as_secs(), reason, "draining: goaway to every connection");
    server.goaway(drain.as_millis() as u64, reason);
    server.tell_hub(&RelayToHub::Draining {
        until_ms: server::now_ms() + drain.as_millis() as u64,
    });
    // The link sends that before it closes.
    server.hub_link.cancel();
    let until = Instant::now() + drain;
    while Instant::now() < until && server.counts().0 > 0 {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn header_timeout_cuts_slowloris_and_disarms_after_crlf_crlf() {
        // 1. Incomplete headers time out with TimedOut.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
        let (server_stream, _) = listener.accept().await.unwrap();
        let sem = Arc::new(Semaphore::new(2));
        let permit = sem.clone().acquire_owned().await.unwrap();
        let mut wrapped = HeaderTimeoutStream::new(server_stream, permit, Duration::from_millis(40));
        client.write_all(b"GET /healthz HTTP/1.1\r\nHost: x").await.unwrap();
        let mut buf = [0u8; 64];
        let n = wrapped.read(&mut buf).await.unwrap();
        assert!(n > 0);
        assert!(wrapped.deadline.is_some(), "header terminator not yet seen");
        let err = wrapped.read(&mut buf).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);

        // 2. Once `\r\n\r\n` arrives across chunks, deadline is cleared and reads succeed past timeout.
        let mut client2 = tokio::net::TcpStream::connect(addr).await.unwrap();
        let (server_stream2, _) = listener.accept().await.unwrap();
        let permit2 = sem.clone().acquire_owned().await.unwrap();
        let mut wrapped2 = HeaderTimeoutStream::new(server_stream2, permit2, Duration::from_millis(40));
        client2.write_all(b"GET / HTTP/1.1\r\n\r").await.unwrap();
        let _ = wrapped2.read(&mut buf).await.unwrap();
        assert!(wrapped2.deadline.is_some());
        client2.write_all(b"\nbody").await.unwrap();
        let _ = wrapped2.read(&mut buf).await.unwrap();
        assert!(wrapped2.deadline.is_none(), "deadline disarmed across chunk boundary");
        tokio::time::sleep(Duration::from_millis(60)).await;
        client2.write_all(b"after").await.unwrap();
        let n = wrapped2.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"after");
    }
}
