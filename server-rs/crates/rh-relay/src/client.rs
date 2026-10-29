//! A device's or an agent's connection to a relay: `GET /v1/connect` (§2.4).
//!
//! 1. relay → `challenge {nonce, relay, origin, version: 1}`;
//! 2. peer → `login {ticket, time_ms, sig}` within 10 s, the signature a
//!    `HubLogin` with the ticket's role (`RelayDevice`/`RelayAgent`), this
//!    relay's origin and the nonce, under the ticket's `sign_key`;
//! 3. relay → `welcome {relay, id, expires_ms}`, and a device `presence
//!    {machines}` (its ticket's machines, `online` when signed in here), then
//!    only changes;
//! 4. then binary frames — `OPEN`/`DATA`/`CREDIT`/`CLOSE`/`PING`/`PONG`
//!    only; any other critical frame closes the connection (4400 "not on a
//!    relay") — and `ticket {ticket}` renewals (→ `ticket_ok {expires_ms}`).
//!    The relay may send `goaway {retry_ms, reason}` and `error {code, message}`.
//!
//! Each inbound message is charged to the connection's buckets — the
//! ticket's `bytes_per_sec` (or the Hub's throttle) and `frames_per_sec`,
//! bursts of twice that; a throttled account's bytes also to the bucket all
//! its connections share — and the reader waits when they run dry. Outbound
//! has a 32 MiB budget; a peer that does not read is cut off (1013).
//!
//! **Logging.** Never a payload, never a ticket, never a signature: the most
//! a log line says about a frame is its type, `ch` and length.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use futures_util::StreamExt;
use rand::RngCore;
use rh_proto::frame::{Frame, FrameType, RawFrame, IGNORABLE_TYPES};
use rh_proto::ticket::TicketRole;
use rh_proto::wire::connect::{PeerToRelay, RelayToPeer};

use crate::config::hex;
use crate::conn::Outbound;
use crate::limit::ConnLimiter;
use crate::server::{ConnRate, Server, CLOSE_BUSY, CLOSE_RESTART};

/// Bytes queued towards one connection before it is cut off.
pub const OUTBOUND_BUDGET: usize = 32 * 1024 * 1024;
/// Frames a second before sign-in (and for control messages, ten each).
const FRAMES_PER_SEC: u64 = 1000;
const CONTROL_COST: u64 = 10;
/// The longest control message (a ticket is well under this).
const MAX_TEXT: usize = 16 * 1024;
/// A connection relaying more than this, on average over a window, is logged.
const SUSTAINED_BYTES_PER_SEC: u64 = 2 * 1024 * 1024;
const SUSTAINED_WINDOW: Duration = Duration::from_secs(600);

pub async fn connection(server: Arc<Server>, socket: WebSocket) {
    let id = server.next_conn_id();
    let (out, rx, queued) = Outbound::new(OUTBOUND_BUDGET);
    let (sink, mut stream) = socket.split();
    let writer = crate::conn::spawn_writer(sink, rx, queued, server.shutdown.clone(), (CLOSE_RESTART, "the relay is restarting"));
    if server.draining() {
        out.text(&RelayToPeer::Error {
            code: "busy".into(),
            message: "the relay is shutting down".into(),
            retry_ms: Some(1000),
        });
        out.close(CLOSE_BUSY, "the relay is shutting down");
        let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
        return;
    }
    // Just after a start: the Hub's replay of revocations first.
    server.wait_synced().await;
    let mut nonce = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    out.text(&RelayToPeer::Challenge {
        nonce: hex(&nonce),
        relay: server.config.id.clone(),
        origin: server.config.origin.clone(),
        version: 1,
    });
    let timing = server.config.timing;
    let opened = Instant::now();
    let mut last_inbound = Instant::now();
    let mut signed: Option<(TicketRole, String, ConnRate)> = None;
    let mut rate = 4 * 1024 * 1024u64;
    let mut limiter = ConnLimiter::new(rate, FRAMES_PER_SEC, Instant::now());
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let mut window_start = Instant::now();
    let mut window_bytes = 0u64;
    loop {
        let message = tokio::select! {
            m = stream.next() => m,
            _ = tick.tick() => {
                if last_inbound.elapsed() > timing.silence {
                    out.close(4002, "no heartbeat");
                    break;
                }
                if signed.is_none() && opened.elapsed() > timing.login_timeout {
                    out.close(crate::server::CLOSE_DENIED, "sign in first");
                    break;
                }
                if signed.is_some() {
                    if let Err((code, reason)) = server.still_valid(id) {
                        out.text(&RelayToPeer::Error {
                            code: "expired".into(),
                            message: reason.into(),
                            retry_ms: None,
                        });
                        out.close(code, reason);
                        break;
                    }
                }
                if window_start.elapsed() >= SUSTAINED_WINDOW {
                    let secs = window_start.elapsed().as_secs().max(1);
                    if window_bytes / secs > SUSTAINED_BYTES_PER_SEC {
                        if let Some((_, account, _)) = &signed {
                            tracing::warn!(conn = id, %account, mib_per_s = window_bytes / secs / (1024 * 1024), "a connection relayed more than 2 MiB/s for ten minutes");
                        }
                    }
                    window_start = Instant::now();
                    window_bytes = 0;
                }
                if out.is_closed() {
                    break;
                }
                continue;
            }
            () = server.shutdown.cancelled() => {
                out.close(CLOSE_RESTART, "the relay is restarting");
                break;
            }
        };
        let Some(Ok(message)) = message else { break };
        last_inbound = Instant::now();
        if let Some((_, _, current)) = &signed {
            let now_rate = current.rate.load(Ordering::Relaxed);
            if now_rate != rate {
                rate = now_rate;
                limiter = ConnLimiter::new(rate, FRAMES_PER_SEC, Instant::now());
            }
        }
        let (len, cost) = match &message {
            Message::Binary(b) => (b.len(), 1),
            Message::Text(t) => (t.len(), CONTROL_COST),
            _ => (0, 1),
        };
        window_bytes += len as u64;
        let now = Instant::now();
        let mut wait = limiter.charge(len, cost, now);
        if let Some((_, account, current)) = &signed {
            if current.throttled.load(Ordering::Relaxed) {
                wait = wait.max(server.charge_account(account, rate, len, now));
            }
        }
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        let keep = match message {
            Message::Text(text) => {
                if text.len() > MAX_TEXT {
                    out.close(1009, "control message too large");
                    false
                } else {
                    on_text(&server, id, &out, &nonce, &mut signed, &mut rate, &mut limiter, text.as_str())
                }
            }
            Message::Binary(bytes) => match &signed {
                Some((role, _, _)) => on_binary(&server, id, &out, *role, &bytes),
                None => {
                    out.close(crate::server::CLOSE_DENIED, "sign in first");
                    false
                }
            },
            Message::Close(_) => false,
            Message::Ping(_) | Message::Pong(_) => true,
        };
        if !keep {
            break;
        }
    }
    if let Some((role, account, _)) = &signed {
        tracing::info!(conn = id, %account, kind = if *role == TicketRole::Agent { "agent" } else { "device" }, "signed out");
        server.unregister(id);
    }
    drop(out);
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
}

#[allow(clippy::too_many_arguments)]
fn on_text(
    server: &Server,
    id: u64,
    out: &Outbound,
    nonce: &[u8; 32],
    signed: &mut Option<(TicketRole, String, ConnRate)>,
    rate: &mut u64,
    limiter: &mut ConnLimiter,
    text: &str,
) -> bool {
    let fail = |code: u16, what: &str, message: &str| {
        out.text(&RelayToPeer::Error {
            code: what.into(),
            message: message.into(),
            retry_ms: None,
        });
        out.close(code, message);
        false
    };
    let Ok(message) = serde_json::from_str::<PeerToRelay>(text) else {
        return fail(4400, "bad_message", "not a message this relay understands");
    };
    match (signed.as_ref(), message) {
        (None, PeerToRelay::Login { ticket, time_ms, sig }) => {
            let ticket = match server.admit(&ticket, time_ms, &sig, *nonce) {
                Ok(t) => t,
                Err((code, reason)) => {
                    tracing::info!(conn = id, code, reason, "a sign-in refused");
                    return fail(code, "denied", reason);
                }
            };
            let current = match server.register(id, &ticket, out.clone()) {
                Ok(rate) => rate,
                Err((code, reason)) => return fail(code, "limit", reason),
            };
            *rate = current.rate.load(Ordering::Relaxed);
            *limiter = ConnLimiter::new(*rate, u64::from(ticket.limits.frames_per_sec.max(1)), Instant::now());
            out.text(&RelayToPeer::Welcome {
                relay: server.config.id.clone(),
                id: ticket.id.clone(),
                expires_ms: ticket.expires_ms,
            });
            server.presence_snapshot(id);
            tracing::info!(conn = id, account = %ticket.account, id = %ticket.id, kind = if ticket.role == TicketRole::Agent { "agent" } else { "device" }, "signed in");
            *signed = Some((ticket.role, ticket.account, current));
            true
        }
        (Some(_), PeerToRelay::Ticket { ticket }) => {
            let ticket = match server.check_ticket(&ticket) {
                Ok(t) => t,
                Err(error) => {
                    out.text(&RelayToPeer::Error {
                        code: "bad_ticket".into(),
                        message: error.to_string(),
                        retry_ms: None,
                    });
                    return true;
                }
            };
            match server.renew(id, &ticket) {
                Ok(expires_ms) => {
                    out.text(&RelayToPeer::TicketOk { expires_ms });
                    true
                }
                Err((code, reason)) => fail(code, "denied", reason),
            }
        }
        (None, _) => fail(crate::server::CLOSE_DENIED, "sign_in_first", "sign in first"),
        (Some(_), PeerToRelay::Login { .. }) => fail(4400, "unexpected", "already signed in"),
    }
}

fn on_binary(server: &Server, id: u64, out: &Outbound, role: TicketRole, bytes: &[u8]) -> bool {
    let fail = |code: u16, message: &str| {
        out.close(code, message);
        false
    };
    let raw = match RawFrame::decode(bytes) {
        Ok(raw) => raw,
        Err(_) => return fail(4400, "one whole frame per binary message"),
    };
    let (ty, ch, len) = (raw.ty, raw.ch, raw.payload.len());
    tracing::trace!(conn = id, ty, ch, len, "frame in");
    match FrameType::from_u8(ty) {
        Some(FrameType::Open | FrameType::Data | FrameType::Credit | FrameType::Close | FrameType::Ping | FrameType::Pong) => {}
        Some(_) => return fail(4400, "not on a relay"),
        None if ty >= IGNORABLE_TYPES => return true,
        None => return fail(4400, "a malformed or unknown critical frame"),
    }
    let Ok(frame) = Frame::from_raw(raw) else {
        return fail(4400, "a malformed frame");
    };
    match frame {
        Frame::Ping { nonce } => {
            out.frame(&Frame::Pong { nonce });
        }
        Frame::Pong { .. } => {}
        Frame::Open { ch, peer } => {
            if role == TicketRole::Agent {
                out.frame(&Frame::Close {
                    ch,
                    code: crate::route::CLOSE_PROTOCOL,
                    reason: "agents do not open channels".into(),
                });
            } else {
                server.open(id, ch, &peer);
            }
        }
        Frame::Data { ch, payload } => server.data(id, ch, payload),
        Frame::Credit { ch, bytes } => server.credit(id, ch, bytes),
        Frame::Close { ch, code, reason } => server.close(id, ch, code, reason),
        _ => {}
    }
    true
}
