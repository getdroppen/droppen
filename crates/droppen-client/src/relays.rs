//! A standalone device's relay connections (`docs/relay-and-plans.md`
//! §2.3–§2.9): one per relay it reaches hosts through, made when a channel
//! needs it.
//!
//! - **Signed in** with a ticket from the hub (`POST /v1/relays/tickets`) and
//!   the device's own key ([`crate::link::connect_relay_welcome`]).
//! - **Renewed** before the ticket runs out: two thirds of the way to its
//!   expiry (ten minutes of fifteen) a fresh ticket goes up as `ticket
//!   {ticket}` and the relay's `ticket_ok {expires_ms}` sets the next. A relay
//!   closes a connection whose ticket ran out a minute ago (4419); a renewal
//!   that fails is tried again, sooner each time, until then.
//! - **Presence**: the relay's `presence` snapshot after `welcome` (every host
//!   the ticket names, `online` when signed in there), then each change —
//!   where to open a channel, and whether the host is offline.
//! - **Let go** when it goes silent (the link pings every 20 s and the relay
//!   answers each: a minute without a word is a dead connection), or when the
//!   relay says `goaway` (draining: no new channel goes there, the ones on it
//!   finish). The next channel that needs the relay makes a new connection.
//! - **Where it dials.** A relay's origin comes from the hub and is not
//!   trusted with the network this device is on (`droppen_proto::netpolicy`,
//!   as the agent): `wss://` at a public name or address, every address the
//!   name resolves to public. [`RelayOptions::allow_loopback`] (local testing
//!   only) takes `ws://` to loopback besides.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use droppen_proto::netpolicy;
use droppen_proto::wire::connect::PeerToRelay;
use ed25519_dalek::SigningKey;
use tokio::sync::watch;

use crate::failure::{Failure, Refuser};
use crate::hub::{HubSession, Ticket};
use crate::identity::DeviceIdentity;
use crate::link::{connect_relay_welcome, ConnectError, Link};
use crate::now_ms;

/// How the pool runs (shortened in tests).
#[derive(Debug, Clone)]
pub struct RelayOptions {
    /// Relays on loopback, over plain `ws://` too: local testing only.
    pub allow_loopback: bool,
    /// A connection that says nothing for this long is let go.
    pub silence: Duration,
    /// How long a new connection waits for its `presence` snapshot.
    pub presence_wait: Duration,
    /// Renew tickets before they run out. Always, but for a test that
    /// watches a relay close a connection whose ticket did (4419).
    pub renew: bool,
}

impl Default for RelayOptions {
    fn default() -> Self {
        Self {
            allow_loopback: false,
            silence: Duration::from_secs(60),
            presence_wait: Duration::from_secs(3),
            renew: true,
        }
    }
}

/// Where a host stands on one relay connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Online,
    Offline,
    /// The ticket does not name it: the hub would not route this device there.
    NotNamed,
}

/// What a connection's driver and its users share.
struct ConnState {
    /// The hosts the ticket names, `true` when signed in on this relay;
    /// `None` until the snapshot came.
    presence: watch::Sender<Option<BTreeMap<String, bool>>>,
    /// When (ms) the ticket in use runs out.
    expires_ms: AtomicU64,
    /// Renewals the relay took.
    renewed: AtomicU32,
    /// The relay said `goaway`.
    draining: AtomicBool,
}

/// One signed-in relay connection.
pub struct RelayConn {
    pub relay: String,
    pub origin: String,
    link: Arc<Link>,
    state: Arc<ConnState>,
    presence_wait: Duration,
    driver: tokio::task::JoinHandle<()>,
}

impl Drop for RelayConn {
    fn drop(&mut self) {
        self.driver.abort();
        self.link.stop();
    }
}

impl RelayConn {
    /// The link channels open on.
    pub fn link(&self) -> &Link {
        &self.link
    }

    /// Whether the relay closed it, or it was let go.
    pub fn is_closed(&self) -> bool {
        self.link.is_closed()
    }

    /// Up, and not draining: a new channel may go here.
    pub fn is_live(&self) -> bool {
        !self.is_closed() && !self.state.draining.load(Ordering::Relaxed)
    }

    /// When (ms) the ticket in use runs out.
    pub fn expires_ms(&self) -> u64 {
        self.state.expires_ms.load(Ordering::Relaxed)
    }

    /// How many renewals the relay took.
    pub fn renewals(&self) -> u32 {
        self.state.renewed.load(Ordering::Relaxed)
    }

    /// Where `host` stands as the relay said last; `None` before its snapshot.
    pub fn presence_now(&self, host: &str) -> Option<Presence> {
        self.state.presence.borrow().as_ref().map(|machines| match machines.get(host) {
            Some(true) => Presence::Online,
            Some(false) => Presence::Offline,
            None => Presence::NotNamed,
        })
    }

    /// [`RelayConn::presence_now`], waiting a little for the snapshot of a
    /// connection just made (none in time: offline).
    pub async fn presence(&self, host: &str) -> Presence {
        let mut rx = self.state.presence.subscribe();
        let _ = tokio::time::timeout(self.presence_wait, rx.wait_for(|p| p.is_some())).await;
        self.presence_now(host).unwrap_or(Presence::Offline)
    }
}

/// One relay's connection, made or replaced under its own lock.
type Slot = Arc<tokio::sync::Mutex<Option<Arc<RelayConn>>>>;

/// The device's relay connections, by relay.
pub struct RelayPool {
    hub: Arc<HubSession>,
    account: String,
    device: String,
    sign: SigningKey,
    options: RelayOptions,
    slots: tokio::sync::Mutex<HashMap<String, Slot>>,
}

impl RelayPool {
    pub fn new(hub: Arc<HubSession>, identity: &DeviceIdentity, options: RelayOptions) -> Self {
        Self {
            hub,
            account: identity.account.clone(),
            device: identity.device.clone(),
            sign: identity.sign_key().clone(),
            options,
            slots: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// The live connections (up, not draining).
    pub async fn connected(&self) -> Vec<Arc<RelayConn>> {
        let slots: Vec<_> = self.slots.lock().await.values().cloned().collect();
        let mut out = Vec::new();
        for slot in slots {
            if let Some(conn) = slot.lock().await.as_ref() {
                if conn.is_live() {
                    out.push(conn.clone());
                }
            }
        }
        out
    }

    /// The connection to `relay`: the live one, or a new one (a fresh
    /// ticket, a sign-in) in place of one that closed or drains.
    pub async fn connection(&self, relay: &str) -> Result<Arc<RelayConn>, Failure> {
        let slot = self.slots.lock().await.entry(relay.to_string()).or_default().clone();
        let mut slot = slot.lock().await;
        if let Some(conn) = slot.as_ref() {
            if conn.is_live() {
                return Ok(conn.clone());
            }
        }
        let conn = Arc::new(self.connect(relay).await?);
        // A draining one stays up for the channels on it; they hold it.
        *slot = Some(conn.clone());
        Ok(conn)
    }

    async fn connect(&self, relay: &str) -> Result<RelayConn, Failure> {
        let ticket = self.hub.ticket(relay).await?;
        check_origin(&ticket.origin, self.options.allow_loopback).await?;
        let (link, welcome) = connect_relay_welcome(&ticket.origin, &self.account, &self.device, &self.sign, &ticket.ticket)
            .await
            .map_err(|e| match e {
                ConnectError::Refused { code, answer } if code == "denied" => Failure::Refused {
                    by: Refuser::Relay,
                    code: 4401,
                    why: format!("relay {relay} refused the sign-in: {answer}"),
                },
                ConnectError::Refused { answer, .. } => Failure::Offline(format!("relay {relay} refused the sign-in: {answer}")),
                ConnectError::Failed(why) => Failure::Offline(format!("relay {relay}: {why}")),
            })?;
        let expires_ms = welcome["expires_ms"].as_u64().unwrap_or(ticket.expires_ms);
        let state = Arc::new(ConnState {
            presence: watch::channel(None).0,
            expires_ms: AtomicU64::new(expires_ms),
            renewed: AtomicU32::new(0),
            draining: AtomicBool::new(false),
        });
        let link = Arc::new(link);
        tracing::info!(relay, "signed in on a relay");
        let origin = ticket.origin.clone();
        let driver = tokio::spawn(drive(link.clone(), state.clone(), self.hub.clone(), ticket, self.options.clone()));
        Ok(RelayConn {
            relay: relay.to_string(),
            origin,
            link,
            state,
            presence_wait: self.options.presence_wait,
            driver,
        })
    }
}

/// When to renew a ticket that runs out at `expires_ms`: two thirds of the
/// way there.
fn renew_in(expires_ms: u64, now: u64) -> Duration {
    Duration::from_millis(expires_ms.saturating_sub(now) * 2 / 3)
}

/// How long to wait before trying a failed renewal again: a quarter of what
/// is left, between 200 ms and 30 s.
fn retry_in(expires_ms: u64, now: u64) -> Duration {
    Duration::from_millis((expires_ms.saturating_sub(now) / 4).clamp(200, 30_000))
}

/// A connection's own task: its control messages (presence, renewals,
/// `goaway`), the renewals themselves, and the watch on its silence.
async fn drive(link: Arc<Link>, state: Arc<ConnState>, hub: Arc<HubSession>, first: Ticket, options: RelayOptions) {
    let relay = first.relay.clone();
    let origin = first.origin.clone();
    let mut renew_at = tokio::time::Instant::now() + renew_in(state.expires_ms.load(Ordering::Relaxed), now_ms());
    let mut snapshot = true;
    let mut watch = tokio::time::interval(Duration::from_millis(500).min(options.silence / 4));
    loop {
        tokio::select! {
            control = link.next_control(Duration::from_secs(3600)) => {
                let Some(v) = control else {
                    if link.is_closed() {
                        break;
                    }
                    continue;
                };
                match v["t"].as_str() {
                    Some("presence") => {
                        let machines: Vec<(String, bool)> = v["machines"]
                            .as_array()
                            .map(|a| a.iter().filter_map(|m| Some((m["id"].as_str()?.to_string(), m["state"] == "online"))).collect())
                            .unwrap_or_default();
                        let is_snapshot = std::mem::replace(&mut snapshot, false);
                        state.presence.send_modify(|p| {
                            let known = p.get_or_insert_with(BTreeMap::new);
                            if is_snapshot {
                                known.clear();
                            }
                            known.extend(machines);
                        });
                    }
                    Some("ticket_ok") => {
                        if let Some(expires) = v["expires_ms"].as_u64() {
                            state.expires_ms.store(expires, Ordering::Relaxed);
                            state.renewed.fetch_add(1, Ordering::Relaxed);
                            renew_at = tokio::time::Instant::now() + renew_in(expires, now_ms());
                            tracing::debug!(relay, expires, "the relay took a renewed ticket");
                        }
                    }
                    Some("goaway") => {
                        tracing::info!(relay, reason = %v["reason"], "the relay is draining: no new channel goes there");
                        state.draining.store(true, Ordering::Relaxed);
                    }
                    Some("error") => {
                        tracing::warn!(relay, code = %v["code"], message = %v["message"], "the relay said");
                        if v["code"] == "bad_ticket" {
                            renew_at = tokio::time::Instant::now() + retry_in(state.expires_ms.load(Ordering::Relaxed), now_ms());
                        }
                    }
                    _ => {}
                }
            }
            () = tokio::time::sleep_until(renew_at), if options.renew => {
                // Tried again before long unless the relay's `ticket_ok` sets the next.
                renew_at = tokio::time::Instant::now() + retry_in(state.expires_ms.load(Ordering::Relaxed), now_ms());
                match hub.ticket(&relay).await {
                    Ok(ticket) if ticket.origin == origin => {
                        if link.send_relay(&PeerToRelay::Ticket { ticket: ticket.ticket }).is_err() {
                            break;
                        }
                    }
                    Ok(_) => {
                        tracing::warn!(relay, "the hub moved the relay to another origin: letting the connection go");
                        state.draining.store(true, Ordering::Relaxed);
                    }
                    Err(error) => tracing::warn!(relay, %error, "could not renew the relay ticket; trying again"),
                }
            }
            _ = watch.tick() => {
                if link.silent_ms() > options.silence.as_millis() as u64 {
                    tracing::warn!(relay, "the relay connection went silent: letting it go");
                    link.stop();
                    break;
                }
            }
            () = link.closed() => break,
        }
    }
    if let Some((code, reason)) = link.close_frame().get() {
        tracing::info!(relay, code, %reason, "the relay closed the connection");
    } else {
        tracing::info!(relay, "the relay connection ended");
    }
}

/// Whether this device may dial a relay at `origin`: `wss://host[:port]` at a
/// public name or address, every address the name resolves to public
/// (`allow_loopback`: `ws://` and loopback besides, for local testing).
async fn check_origin(origin: &str, allow_loopback: bool) -> Result<(), Failure> {
    let refuse = || Failure::Offline(format!("the hub named a relay at {origin}, which this device does not dial"));
    let (scheme, rest) = origin.split_once("://").ok_or_else(refuse)?;
    if rest.is_empty() || rest.contains(['/', '@', '?', '#']) {
        return Err(refuse());
    }
    let (host, port) = match rest.strip_prefix('[') {
        Some(inner) => {
            let (host, after) = inner.split_once(']').ok_or_else(refuse)?;
            let port = match after {
                "" => None,
                p => Some(p.strip_prefix(':').and_then(|p| p.parse::<u16>().ok()).ok_or_else(refuse)?),
            };
            (host, port)
        }
        None => match rest.rsplit_once(':') {
            Some((host, p)) => (host, Some(p.parse::<u16>().map_err(|_| refuse())?)),
            None => (rest, None),
        },
    };
    let secure = match scheme {
        "wss" => true,
        "ws" if allow_loopback => false,
        _ => return Err(refuse()),
    };
    if !netpolicy::host_allowed(host, allow_loopback) {
        return Err(refuse());
    }
    let port = port.unwrap_or(if secure { 443 } else { 80 });
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| Failure::Offline(format!("relay {origin}: {e}")))?
        .collect();
    if addrs.is_empty() || !addrs.iter().all(|a| netpolicy::allowed(a.ip(), allow_loopback)) {
        return Err(refuse());
    }
    if !secure && !addrs.iter().all(|a| netpolicy::is_loopback(a.ip())) {
        return Err(refuse());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_is_renewed_two_thirds_of_the_way_and_retried_sooner() {
        assert_eq!(renew_in(15 * 60_000, 0), Duration::from_secs(600));
        assert_eq!(renew_in(3_000, 0), Duration::from_secs(2));
        assert_eq!(renew_in(5, 10), Duration::ZERO);
        assert_eq!(retry_in(15 * 60_000, 0), Duration::from_secs(30));
        assert_eq!(retry_in(2_000, 0), Duration::from_millis(500));
        assert_eq!(retry_in(0, 10), Duration::from_millis(200));
    }

    #[tokio::test]
    async fn a_relay_is_dialled_only_where_the_policy_lets_it() {
        for bad in [
            "ws://relay.example",
            "https://relay.example",
            "wss://10.0.0.5",
            "wss://hub.railway.internal",
            "wss://localhost:8080",
            "wss://relay.example/path",
            "wss://user@relay.example",
        ] {
            assert!(check_origin(bad, false).await.is_err(), "{bad}");
        }
        assert!(check_origin("ws://127.0.0.1:9", true).await.is_ok());
        assert!(check_origin("wss://127.0.0.1:9", true).await.is_ok());
        assert!(check_origin("ws://127.0.0.1:9", false).await.is_err());
    }
}
