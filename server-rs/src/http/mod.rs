//! The HTTP kit every route module builds on: the error body and its status
//! codes ([`error`]), the extractors that answer a bad request with that body
//! ([`extract`]), the `201`/`202` responses ([`created`]), the websocket
//! keepalive and bounded sends, and the build's identity.
//!
//! It sits below the subsystems and the router alike, so a subsystem can
//! answer a request without depending on the router that mounts it.

use futures_util::SinkExt;
use gotcha::axum::extract::ws::{Message, WebSocket};
use tokio_util::sync::CancellationToken;

pub mod bridge;
pub mod created;
pub mod error;
pub mod extract;

/// How often a live socket is pinged.
const PING_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
/// How long a socket may go without a frame from the client before it is dropped.
///
/// Three missed pings. A browser answers a `Ping` with a `Pong` on its own, so a
/// tab that is still there always beats this; what does not is a connection that
/// died without a `FIN` — a laptop that slept, a NAT that forgot, a phone that
/// changed network. Those sockets would otherwise hold a feed watcher (and, for
/// a terminal, a pty) open until the process restarts.
pub(crate) const IDLE_LIMIT: std::time::Duration = std::time::Duration::from_secs(90);

/// The keepalive half of a websocket loop.
///
/// Every socket in this server is server-to-client except for the terminal's
/// control frames, so without a ping there is nothing to notice a dead peer
/// with: a `send` to a black hole succeeds into the kernel buffer for a long
/// time before it ever fails.
pub struct Heartbeat {
    ticker: tokio::time::Interval,
    idle_limit: std::time::Duration,
    last_seen: tokio::time::Instant,
}

impl Default for Heartbeat {
    fn default() -> Self {
        Self::new(PING_INTERVAL, IDLE_LIMIT)
    }
}

impl Heartbeat {
    /// A heartbeat on its own schedule; the defaults are [`Heartbeat::default`].
    pub fn new(interval: std::time::Duration, idle_limit: std::time::Duration) -> Self {
        let start = tokio::time::Instant::now();
        // `interval` fires immediately on its first tick, which would ping a
        // socket that has not finished opening; `interval_at` makes the first
        // one a real delay like every later one.
        let mut ticker = tokio::time::interval_at(start + interval, interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        Self {
            ticker,
            idle_limit,
            last_seen: start,
        }
    }

    /// Resolves when it is time to ping. `false` means the peer has been quiet
    /// for longer than the idle limit and the socket should be dropped.
    ///
    /// Cancel-safe: [`tokio::time::Interval::tick`] is, and nothing else here
    /// awaits.
    pub async fn tick(&mut self) -> bool {
        self.ticker.tick().await;
        self.last_seen.elapsed() < self.idle_limit
    }

    /// Note that the client is still there.
    pub fn saw_frame(&mut self) {
        self.last_seen = tokio::time::Instant::now();
    }
}

/// How long one frame may take to leave before its socket is given up on.
///
/// The bound is per frame: a frame that has not left within it ends the
/// socket, however recently the frame before it went. It is the heartbeat's
/// idle limit, for the same reason: a client that cannot take one frame in
/// that long is as gone as one that has sent nothing for that long. Without a
/// bound, a client that stopped reading altogether parked its socket task on
/// a full TCP window for good, with its queue, and the heartbeat could not
/// fire while the send was waiting. Every socket reads it from
/// [`AppState::send_limit`](crate::api::AppState::send_limit).
pub(crate) const SEND_LIMIT: std::time::Duration = IDLE_LIMIT;

/// How long a feed socket's close may take, after its last frame.
///
/// Short, and not [`SEND_LIMIT`]: every send waits until its frame has left,
/// so nothing is queued ahead of the close frame, and a client that is still
/// reading takes it at once. Only one that has stopped reading keeps it
/// waiting, and after a send that already waited out the limit for it, a
/// second full limit would only hold the socket task for nothing.
pub(crate) const CLOSE_LIMIT: std::time::Duration = std::time::Duration::from_secs(2);

/// A websocket's write half.
pub(crate) type WsSender = futures_util::stream::SplitSink<WebSocket, Message>;

/// Send one frame, and say whether it went: `false` for a closed socket, a
/// frame that did not leave within `limit`, or a server shutting down.
///
/// Biased toward the send, so a frame that can go at once still goes during
/// a shutdown; only one that would have to wait gives way to it.
pub(crate) async fn send_within(sender: &mut WsSender, message: Message, limit: std::time::Duration, shutdown: &CancellationToken) -> bool {
    tokio::select! {
        biased;
        sent = tokio::time::timeout(limit, sender.send(message)) => match sent {
            Ok(sent) => sent.is_ok(),
            Err(_) => {
                tracing::info!("websocket: a frame did not leave within {limit:?}; giving the socket up");
                false
            }
        },
        () = shutdown.cancelled() => false,
    }
}

/// Close a websocket's write half: its close frame, unless one went already,
/// and whatever was still queued ahead of it.
///
/// Within `limit` ([`CLOSE_LIMIT`] for the feed sockets), and not at all on a
/// server shutting down, for the reason [`send_within`] is: a client that has
/// stopped reading leaves no room for the close frame either. Biased toward
/// the close, so on shutdown one that can go at once still goes. Nothing
/// waits for the client's answer; the socket is dropped right after.
pub(crate) async fn close_within(sender: &mut WsSender, limit: std::time::Duration, shutdown: &CancellationToken) {
    tokio::select! {
        biased;
        _ = tokio::time::timeout(limit, sender.close()) => {}
        () = shutdown.cancelled() => {}
    }
}

/// What this server reports where the contract asks for `nodeVersion`.
pub const RUNTIME_VERSION: &str = concat!("rust/", env!("CARGO_PKG_VERSION"));

/// Exactly which build of the server this is.
///
/// The crate version alone cannot answer the question self-update asks —
/// "is the server I am talking to still the one this page loaded against?" —
/// because during development `0.1.0` is replaced by `0.1.0` all day. So the
/// commit is part of the identity, stamped in by `build.rs`; see there for where
/// it comes from and what happens when there is none.
///
/// This is not the OpenAPI document's version (`API_VERSION` in [`crate::api`]), which is the
/// *contract*'s and must stay stable across commits or every build would
/// re-generate the exported spec.
pub const SERVER_VERSION: &str = if env!("DROPPEN_BUILD_REV").is_empty() {
    env!("CARGO_PKG_VERSION")
} else {
    concat!(env!("CARGO_PKG_VERSION"), "+g", env!("DROPPEN_BUILD_REV"))
};

/// Node's `process.platform` naming, so the field means the same thing it did.
pub(crate) fn node_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

pub(crate) fn hostname() -> String {
    let mut buffer = vec![0u8; 256];
    // SAFETY: the buffer is owned, correctly sized, and the call only writes a
    // NUL-terminated name into it.
    let result = unsafe { libc::gethostname(buffer.as_mut_ptr() as *mut libc::c_char, buffer.len()) };
    if result != 0 {
        return String::new();
    }
    let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn heartbeat() -> Heartbeat {
        Heartbeat::new(Duration::from_secs(30), Duration::from_secs(90))
    }

    /// The first tick is a real delay: pinging the instant a socket opens would
    /// race the client's own handshake bookkeeping for nothing.
    #[tokio::test(start_paused = true)]
    async fn the_first_ping_waits_a_full_interval() {
        let start = tokio::time::Instant::now();
        assert!(heartbeat().tick().await);
        assert_eq!(start.elapsed(), Duration::from_secs(30));
    }

    /// A connection that died without a `FIN` — a slept laptop, a forgotten NAT
    /// binding — answers no ping. Three unanswered ones and it is dropped.
    #[tokio::test(start_paused = true)]
    async fn a_peer_that_never_answers_is_given_up_on() {
        let mut heartbeat = heartbeat();
        assert!(heartbeat.tick().await, "30s in, still within the idle limit");
        assert!(heartbeat.tick().await, "60s in, still within the idle limit");
        assert!(!heartbeat.tick().await, "90s of silence is the limit");
    }

    /// Any frame counts, which is what makes a browser's automatic `Pong` enough.
    #[tokio::test(start_paused = true)]
    async fn a_frame_from_the_client_resets_the_clock() {
        let mut heartbeat = heartbeat();
        assert!(heartbeat.tick().await);
        // A pong at 30s: the idle window now runs from there, not from connect.
        heartbeat.saw_frame();
        assert!(heartbeat.tick().await, "60s in, 30s since the frame");
        assert!(
            heartbeat.tick().await,
            "90s in, 60s since the frame — a clock from connect would give up here"
        );
        assert!(!heartbeat.tick().await, "120s in, 90s since the frame");
    }
}
