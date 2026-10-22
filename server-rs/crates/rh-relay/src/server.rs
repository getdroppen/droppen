//! A relay's shared state: the routing table, who is signed in on what
//! ticket, the accounts' connection counts, revocations and throttles, and
//! the ticket keys tickets are checked under.
//!
//! One `std::sync::Mutex` guards the tables and is never held across an
//! `.await`. The ticket keys come from the Hub's control link (or are pinned
//! by the operator); everything else is this process's alone and forgotten on
//! restart — a relay has no keys of its own to anything and stores nothing.
//! What a restart (or a time away from the Hub) forgets, the Hub tells again
//! when the control link signs in: every revocation still in force, then
//! `replayed`, and the whole throttle table (`throttles`). Until the first
//! replay a sign-in waits (at most [`SYNC_WAIT`] after the start), and is
//! then refused, busy (1013): any ticket might have been revoked unheard.
//!
//! A throttled account (the Hub's `throttle`, or a ticket flagged throttled)
//! shares one byte bucket across all its connections here: forty connections
//! get the throttled rate between them, not forty times it.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use rh_proto::frame::Frame;
use rh_proto::sig::HubLogin;
use rh_proto::ticket::{RelayTicket, SignedTicket, TicketError, TicketRole};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::config::Config;
use crate::conn::Outbound;
use crate::limit::Bucket;
use crate::meter::Meter;
use crate::route::{ConnId, Delivery, Peer, Relay};

/// The relay could not take this sign-in: a close code and a reason.
pub type Refusal = (u16, &'static str);

/// The relay is shutting down or full: try elsewhere, or later.
pub const CLOSE_RESTART: u16 = 1012;
pub const CLOSE_BUSY: u16 = 1013;
/// The ticket ran out and was not renewed.
pub const CLOSE_EXPIRED: u16 = 4419;
/// Over the account's (or the relay's) connection limit.
pub const CLOSE_LIMIT: u16 = 4429;
/// The Hub revoked the device or machine (removed from the account).
pub const CLOSE_REVOKED: u16 = 4403;
/// A sign-in that does not hold.
pub const CLOSE_DENIED: u16 = 4401;
/// How long after a start sign-ins wait for the Hub's replay of revocations.
pub const SYNC_WAIT: Duration = Duration::from_secs(5);

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Who is on a connection, by the ticket it signed in with.
pub struct ConnInfo {
    pub out: Outbound,
    pub account: String,
    pub id: String,
    pub role: TicketRole,
    pub sign_key: [u8; 32],
    /// Devices: the machines the ticket lets it reach.
    pub peers: HashSet<String>,
    pub issued_ms: u64,
    pub expires_ms: u64,
    /// The ticket's byte rate (or the Hub's throttle): the connection's
    /// reader re-reads it.
    pub rate: Arc<AtomicU64>,
    pub ticket_rate: u64,
    /// Whether the account is throttled (the Hub said so, or the ticket):
    /// then its connections share one bucket ([`Server::charge_account`]).
    pub throttled: Arc<AtomicBool>,
    /// The ticket was flagged throttled.
    pub ticket_throttled: bool,
    pub since_ms: u64,
}

/// What a connection reads its limits from (the Hub may change them).
#[derive(Clone)]
pub struct ConnRate {
    pub rate: Arc<AtomicU64>,
    pub throttled: Arc<AtomicBool>,
}

struct Revocation {
    account: String,
    id: Option<String>,
    /// Only the tickets of this role (`None`: both roles).
    role: Option<TicketRole>,
    at_ms: u64,
    until_ms: u64,
}

#[derive(Default)]
struct Inner {
    route: Relay,
    conns: HashMap<ConnId, ConnInfo>,
    by_account: HashMap<String, HashSet<ConnId>>,
    revoked: Vec<Revocation>,
    /// Accounts the Hub throttled, and to what.
    throttles: HashMap<String, u64>,
    /// The bucket a throttled account's connections share: `(rate, bucket)`.
    account_buckets: HashMap<String, (u64, Bucket)>,
}

pub struct Server {
    pub config: Config,
    keys: RwLock<Vec<(u8, [u8; 32])>>,
    inner: Mutex<Inner>,
    next_conn: AtomicU64,
    pub meter: Meter,
    pub shutdown: CancellationToken,
    pub tasks: TaskTracker,
    draining: AtomicBool,
    /// Since when the Hub's control link has been down (`None`: up).
    link_down_since: Mutex<Option<Instant>>,
    pub started: Instant,
    pub unauth: crate::http::UnauthLimits,
    /// Messages for the Hub, while the control link is up.
    uplink: Mutex<Option<tokio::sync::mpsc::UnboundedSender<String>>>,
    /// The Hub replayed its revocations since this process started.
    synced: tokio::sync::watch::Sender<bool>,
}

impl Server {
    pub fn new(config: Config) -> Arc<Self> {
        let keys = config.pinned_keys.clone().unwrap_or_default();
        let keep = (config.timing.usage_keep.as_secs_f64() / config.timing.usage_every.as_secs_f64().max(0.001)).ceil() as usize;
        Arc::new(Self {
            keys: RwLock::new(keys),
            inner: Mutex::new(Inner::default()),
            next_conn: AtomicU64::new(1),
            meter: Meter::new(now_ms(), keep.max(1)),
            shutdown: CancellationToken::new(),
            tasks: TaskTracker::new(),
            draining: AtomicBool::new(false),
            link_down_since: Mutex::new(Some(Instant::now())),
            started: Instant::now(),
            unauth: crate::http::UnauthLimits::default(),
            uplink: Mutex::new(None),
            synced: tokio::sync::watch::channel(false).0,
            config,
        })
    }

    /// The Hub replayed what this process may have missed (`replayed`).
    pub fn mark_synced(&self) {
        self.synced.send_replace(true);
    }

    pub fn synced(&self) -> bool {
        *self.synced.borrow()
    }

    /// Right after a start, wait (at most until [`SYNC_WAIT`] after it) for
    /// the Hub's replay of revocations; [`Server::admit`] refuses every
    /// ticket until it came. Without a Hub, or later, at once.
    pub async fn wait_synced(&self) {
        if self.config.hub.is_none() || self.synced() {
            return;
        }
        let Some(left) = SYNC_WAIT.checked_sub(self.started.elapsed()) else {
            return;
        };
        let mut rx = self.synced.subscribe();
        let _ = tokio::time::timeout(left, rx.wait_for(|s| *s)).await;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("relay state")
    }

    pub fn next_conn_id(&self) -> ConnId {
        self.next_conn.fetch_add(1, Ordering::Relaxed)
    }

    // ------------------------------------------------------------ keys and the Hub

    /// The Hub's ticket keys (ignored when the operator pinned them).
    pub fn set_keys(&self, keys: Vec<(u8, [u8; 32])>) {
        if self.config.pinned_keys.is_some() {
            return;
        }
        *self.keys.write().expect("keys") = keys.clone();
        if let Some(dir) = &self.config.state_dir {
            let text = keys
                .iter()
                .map(|(kid, k)| format!("{kid}:{}", crate::config::hex(k)))
                .collect::<Vec<_>>()
                .join(",");
            let _ = crate::config::write_private(&dir.join("ticket-keys"), &text);
        }
    }

    /// Ticket keys kept from a previous run (a restart while the Hub is down).
    pub fn load_saved_keys(&self) {
        if self.config.pinned_keys.is_some() || !self.keys.read().expect("keys").is_empty() {
            return;
        }
        let Some(dir) = &self.config.state_dir else { return };
        if let Some(keys) = std::fs::read_to_string(dir.join("ticket-keys"))
            .ok()
            .and_then(|t| crate::config::parse_keys(&t))
        {
            *self.keys.write().expect("keys") = keys;
        }
    }

    pub fn key_of(&self, kid: u8) -> Option<[u8; 32]> {
        self.keys.read().expect("keys").iter().find(|(k, _)| *k == kid).map(|(_, key)| *key)
    }

    pub fn has_keys(&self) -> bool {
        !self.keys.read().expect("keys").is_empty()
    }

    pub fn set_link(&self, up: Option<tokio::sync::mpsc::UnboundedSender<String>>) {
        let mut since = self.link_down_since.lock().expect("link");
        match (&up, *since) {
            (Some(_), _) => *since = None,
            (None, None) => *since = Some(Instant::now()),
            (None, Some(_)) => {}
        }
        *self.uplink.lock().expect("uplink") = up;
    }

    /// Tell the Hub, if the control link is up.
    pub fn tell_hub(&self, message: &serde_json::Value) -> bool {
        self.uplink
            .lock()
            .expect("uplink")
            .as_ref()
            .is_some_and(|tx| tx.send(message.to_string()).is_ok())
    }

    /// Whether the control link has been down long enough that established
    /// connections outlive their tickets (§2.3).
    pub fn in_outage(&self) -> bool {
        self.link_down_since
            .lock()
            .expect("link")
            .is_some_and(|t| t.elapsed() >= self.config.timing.outage_after)
    }

    pub fn draining(&self) -> bool {
        self.draining.load(Ordering::Relaxed)
    }

    // ------------------------------------------------------------ sign-in

    /// Check a ticket for this relay now: its signature, its relay, its
    /// expiry and whether the Hub revoked its holder since it was issued.
    pub fn check_ticket(&self, text: &str) -> Result<RelayTicket, TicketError> {
        let signed = SignedTicket::parse(text)?;
        let ticket = signed.verify(|kid| self.key_of(kid), &self.config.id, now_ms())?;
        if self.revoked(&ticket.account, &ticket.id, ticket.role, ticket.issued_ms) {
            return Err(TicketError::Malformed("revoked".into()));
        }
        Ok(ticket)
    }

    fn revoked(&self, account: &str, id: &str, role: TicketRole, issued_ms: u64) -> bool {
        let now = now_ms();
        let mut inner = self.lock();
        inner.revoked.retain(|r| r.until_ms > now);
        inner.revoked.iter().any(|r| {
            r.account == account && r.id.as_deref().is_none_or(|i| i == id) && r.role.is_none_or(|rr| rr == role) && issued_ms <= r.at_ms
        })
    }

    /// A sign-in: `login {ticket, time_ms, sig}` against the challenge
    /// `nonce`. The signature is a `HubLogin` under the ticket's key, with
    /// the ticket's role and this relay's own origin, so neither a Hub
    /// sign-in nor another relay's is good here.
    pub fn admit(&self, ticket: &str, time_ms: u64, sig: &str, nonce: [u8; 32]) -> Result<RelayTicket, Refusal> {
        const DENIED: Refusal = (CLOSE_DENIED, "sign-in refused");
        // Before the Hub's first replay every ticket was issued before it,
        // and any may have been revoked while this relay was not listening.
        if self.config.hub.is_some() && !self.synced() {
            return Err((CLOSE_BUSY, "the relay is starting: try again shortly"));
        }
        let ticket = match self.check_ticket(ticket) {
            Ok(t) => t,
            Err(TicketError::Expired) => return Err((CLOSE_EXPIRED, "the ticket expired")),
            Err(TicketError::Malformed(m)) if m == "revoked" => return Err((CLOSE_REVOKED, "revoked")),
            Err(_) => return Err(DENIED),
        };
        let Some(sig) = crate::config::unhex(sig).and_then(|s| <[u8; 64]>::try_from(s).ok()) else {
            return Err(DENIED);
        };
        if now_ms().abs_diff(time_ms) > self.config.timing.login_skew_ms {
            return Err(DENIED);
        }
        let login = HubLogin {
            role: ticket.role.login_role(),
            account: ticket.account.clone(),
            id: ticket.id.clone(),
            hub: self.config.origin.clone(),
            nonce,
            time_ms,
        };
        if !login.verify(&ticket.sign_key, &sig) {
            return Err(DENIED);
        }
        Ok(ticket)
    }

    /// Register a signed-in connection; refused when the relay or the
    /// account is full. An agent's devices here hear it came.
    pub fn register(&self, conn: ConnId, ticket: &RelayTicket, out: Outbound) -> Result<ConnRate, Refusal> {
        if self.draining() {
            return Err((CLOSE_BUSY, "the relay is shutting down"));
        }
        let mut inner = self.lock();
        if inner.conns.len() >= self.config.max_conns {
            return Err((CLOSE_BUSY, "the relay is full"));
        }
        let held = inner.by_account.get(&ticket.account).map_or(0, HashSet::len);
        if held >= usize::from(ticket.limits.account_conns) {
            return Err((CLOSE_LIMIT, "too many connections for this account on this relay"));
        }
        let peer = match ticket.role {
            TicketRole::Device => Peer::Device {
                account: ticket.account.clone(),
                device: ticket.id.clone(),
            },
            TicketRole::Agent => Peer::Agent {
                account: ticket.account.clone(),
                machine: ticket.id.clone(),
            },
        };
        let replaced = inner.route.add(conn, peer, usize::from(ticket.limits.channels).max(1));
        let ticket_rate = u64::from(ticket.limits.bytes_per_sec.max(1));
        let hub_throttle = inner.throttles.get(&ticket.account).copied();
        let rate = Arc::new(AtomicU64::new(hub_throttle.unwrap_or(ticket_rate)));
        let throttled = Arc::new(AtomicBool::new(hub_throttle.is_some() || ticket.throttled()));
        inner.conns.insert(
            conn,
            ConnInfo {
                out: out.clone(),
                account: ticket.account.clone(),
                id: ticket.id.clone(),
                role: ticket.role,
                sign_key: ticket.sign_key,
                peers: ticket.peers.iter().cloned().collect(),
                issued_ms: ticket.issued_ms,
                expires_ms: ticket.expires_ms,
                rate: rate.clone(),
                ticket_rate,
                throttled: throttled.clone(),
                ticket_throttled: ticket.throttled(),
                since_ms: now_ms(),
            },
        );
        inner.by_account.entry(ticket.account.clone()).or_default().insert(conn);
        if let Some(old) = replaced.and_then(|old| inner.conns.get(&old)) {
            old.out.close(4001, "signed in again elsewhere");
        }
        if ticket.role == TicketRole::Agent {
            presence_change(&inner, &ticket.account, &ticket.id, true);
        }
        Ok(ConnRate { rate, throttled })
    }

    /// Charge `len` bytes of a throttled account's connection to the bucket
    /// all its connections share; the wait before handling them.
    pub fn charge_account(&self, account: &str, rate: u64, len: usize, now: Instant) -> Duration {
        let rate = rate.max(1);
        let mut inner = self.lock();
        let entry = inner
            .account_buckets
            .entry(account.to_string())
            .or_insert_with(|| (rate, Bucket::new(rate, rate.saturating_mul(2), now)));
        if entry.0 != rate {
            *entry = (rate, Bucket::new(rate, rate.saturating_mul(2), now));
        }
        entry.1.take(len as u64, now)
    }

    /// A device's presence snapshot (after its `welcome`): every machine its
    /// ticket names, `online` when signed in here.
    pub fn presence_snapshot(&self, conn: ConnId) {
        let inner = self.lock();
        let Some(info) = inner.conns.get(&conn).filter(|i| i.role == TicketRole::Device) else {
            return;
        };
        let mut peers: Vec<&String> = info.peers.iter().collect();
        peers.sort();
        let machines: Vec<serde_json::Value> = peers
            .into_iter()
            .map(|m| {
                let online = inner
                    .route
                    .agent(m)
                    .is_some_and(|c| inner.conns.get(&c).is_some_and(|i| i.account == info.account));
                json!({"id": m, "state": if online { "online" } else { "offline" }})
            })
            .collect();
        info.out.text(&json!({"t": "presence", "machines": machines}));
    }

    /// A renewal on connection `conn`: the same holder, a fresh expiry and
    /// (for a device) the peers as they are now.
    pub fn renew(&self, conn: ConnId, ticket: &RelayTicket) -> Result<u64, Refusal> {
        let mut inner = self.lock();
        let throttle = inner.throttles.get(&ticket.account).copied();
        let Some(info) = inner.conns.get_mut(&conn) else {
            return Err((CLOSE_DENIED, "not signed in"));
        };
        if info.account != ticket.account || info.id != ticket.id || info.role != ticket.role || info.sign_key != ticket.sign_key {
            return Err((CLOSE_DENIED, "a ticket for someone else"));
        }
        info.expires_ms = ticket.expires_ms;
        info.issued_ms = ticket.issued_ms;
        info.peers = ticket.peers.iter().cloned().collect();
        info.ticket_rate = u64::from(ticket.limits.bytes_per_sec.max(1));
        info.ticket_throttled = ticket.throttled();
        info.rate.store(throttle.unwrap_or(info.ticket_rate), Ordering::Relaxed);
        info.throttled.store(throttle.is_some() || info.ticket_throttled, Ordering::Relaxed);
        Ok(ticket.expires_ms)
    }

    /// The connection went away: its channels close on the other side.
    pub fn unregister(&self, conn: ConnId) {
        let deliveries = {
            let mut inner = self.lock();
            let deliveries = inner.route.remove(conn);
            if let Some(info) = inner.conns.remove(&conn) {
                if let Some(set) = inner.by_account.get_mut(&info.account) {
                    set.remove(&conn);
                    if set.is_empty() {
                        inner.by_account.remove(&info.account);
                        inner.account_buckets.remove(&info.account);
                    }
                }
                if info.role == TicketRole::Agent && inner.route.agent(&info.id).is_none() {
                    presence_change(&inner, &info.account, &info.id, false);
                }
            }
            deliveries
        };
        self.deliver(deliveries);
    }

    /// Whether `conn`'s ticket (renewal included) still holds; `Err` with
    /// the close when it ran out (unless the Hub's link is in an outage).
    pub fn still_valid(&self, conn: ConnId) -> Result<(), Refusal> {
        let now = now_ms();
        let Some(expires) = self.lock().conns.get(&conn).map(|i| i.expires_ms) else {
            return Ok(());
        };
        let timing = &self.config.timing;
        if now <= expires + timing.renew_grace_ms || (self.in_outage() && now <= expires + timing.outage_grace_ms) {
            return Ok(());
        }
        Err((CLOSE_EXPIRED, "the ticket expired"))
    }

    // ------------------------------------------------------------ frames

    pub fn deliver(&self, deliveries: Vec<Delivery>) {
        if deliveries.is_empty() {
            return;
        }
        let inner = self.lock();
        for d in deliveries {
            if let Some(info) = inner.conns.get(&d.to) {
                info.out.frame(&d.frame);
            }
        }
    }

    /// `OPEN {ch, peer}` from a device: only to a machine its ticket names,
    /// and never to itself (chain v3: a host signs in here as an agent and,
    /// with another ticket, as a device — one identity, two roles; its device
    /// connection does not reach its own agent connection).
    pub fn open(&self, conn: ConnId, ch: u32, machine: &str) {
        let deliveries = {
            let mut inner = self.lock();
            let allowed = match inner.conns.get(&conn) {
                Some(info) if info.role == TicketRole::Device && info.id == machine => Err("a device does not open a channel to itself"),
                Some(info) if info.role == TicketRole::Device && info.peers.contains(machine) => Ok(()),
                Some(info) if info.role == TicketRole::Device => Err("the ticket does not name that device"),
                _ => Ok(()),
            };
            inner.route.open(conn, ch, machine, allowed)
        };
        self.deliver(deliveries);
    }

    /// `DATA`: forwarded untouched and counted for the account.
    pub fn data(&self, conn: ConnId, ch: u32, payload: Vec<u8>) {
        let len = payload.len();
        let (deliveries, account) = {
            let mut inner = self.lock();
            let deliveries = inner.route.data(conn, ch, payload);
            let account = deliveries
                .iter()
                .any(|d| matches!(d.frame, Frame::Data { .. }))
                .then(|| inner.conns.get(&conn).map(|i| i.account.clone()))
                .flatten();
            (deliveries, account)
        };
        if let Some(account) = account {
            self.meter.count(&account, len);
        }
        self.deliver(deliveries);
    }

    pub fn credit(&self, conn: ConnId, ch: u32, bytes: u32) {
        let deliveries = self.lock().route.credit(conn, ch, bytes);
        self.deliver(deliveries);
    }

    pub fn close(&self, conn: ConnId, ch: u32, code: u16, reason: String) {
        let deliveries = self.lock().route.close(conn, ch, code, reason);
        self.deliver(deliveries);
    }

    // ------------------------------------------------------------ the Hub's orders

    /// Close every connection of `account` (of `id` only, when given) on a
    /// ticket issued at or before `at_ms`, and refuse such tickets from now
    /// on. The Hub replays revocations on every sign-in of the control link:
    /// one heard again changes nothing, and a connection on a ticket issued
    /// after it (renewed since) is left alone.
    pub fn revoke(&self, account: &str, id: Option<&str>, at_ms: u64) {
        self.revoke_role(account, id, None, at_ms);
    }

    /// [`Server::revoke`], for the tickets of `role` only when given: a
    /// host's device-role tickets carry the same id as its agent's, and a
    /// renewal of its identity key revokes the former and leaves the latter
    /// (the agent must stay reachable to be asked for it). A Hub sends a
    /// scoped revocation only to a relay that signed in with
    /// [`crate::uplink::FEATURE_REVOKE_ROLE`].
    pub fn revoke_role(&self, account: &str, id: Option<&str>, role: Option<TicketRole>, at_ms: u64) {
        let outs: Vec<Outbound> = {
            let mut inner = self.lock();
            let horizon = now_ms() + rh_proto::ticket::TICKET_TTL_MS + self.config.timing.renew_grace_ms;
            let known = inner
                .revoked
                .iter_mut()
                .find(|r| r.account == account && r.id.as_deref() == id && r.role == role && r.at_ms == at_ms);
            match known {
                Some(r) => r.until_ms = r.until_ms.max(horizon),
                None => inner.revoked.push(Revocation {
                    account: account.to_string(),
                    id: id.map(str::to_string),
                    role,
                    at_ms,
                    until_ms: horizon,
                }),
            }
            inner
                .by_account
                .get(account)
                .into_iter()
                .flatten()
                .filter_map(|c| inner.conns.get(c))
                .filter(|i| id.is_none_or(|id| i.id == id) && role.is_none_or(|r| i.role == r) && i.issued_ms <= at_ms)
                .map(|i| i.out.clone())
                .collect()
        };
        tracing::info!(%account, ?id, ?role, closed = outs.len(), "revoked by the Hub");
        for out in outs {
            out.close(CLOSE_REVOKED, "removed from the account");
        }
    }

    /// The Hub throttles `account` (or lifts it, `None`): every connection of
    /// the account takes the rate at its next message.
    pub fn throttle(&self, account: &str, bytes_per_sec: Option<u64>) {
        let mut inner = self.lock();
        set_throttle(&mut inner, account, bytes_per_sec);
        tracing::info!(%account, ?bytes_per_sec, "throttle from the Hub");
    }

    /// The Hub's whole throttle table (`throttles`, on every sign-in of the
    /// control link): accounts not in it are not throttled — a lift this
    /// relay missed while it was away takes effect now.
    pub fn throttle_snapshot(&self, table: &HashMap<String, u64>) {
        let mut inner = self.lock();
        let mut accounts: HashSet<String> = inner.throttles.keys().cloned().collect();
        accounts.extend(table.keys().cloned());
        for account in accounts {
            set_throttle(&mut inner, &account, table.get(&account).copied());
        }
        tracing::info!(throttled = table.len(), "the Hub's throttle table");
    }

    /// The throttle on `account` now, if the Hub set one.
    pub fn throttle_of(&self, account: &str) -> Option<u64> {
        self.lock().throttles.get(account).copied()
    }

    /// Tell everyone to move on (`goaway`), and take no new sign-ins.
    pub fn goaway(&self, retry_ms: u64, reason: &str) {
        self.draining.store(true, Ordering::Relaxed);
        let inner = self.lock();
        for info in inner.conns.values() {
            info.out.text(&json!({"t": "goaway", "retry_ms": retry_ms, "reason": reason}));
        }
    }

    /// `(connections, channels)`.
    pub fn counts(&self) -> (usize, usize) {
        let inner = self.lock();
        let channels = inner.conns.keys().map(|c| inner.route.channels(*c)).sum::<usize>() / 2;
        (inner.conns.len(), channels)
    }

    /// Load for the Hub's directory: connections over the limit.
    pub fn load(&self) -> f32 {
        let (conns, _) = self.counts();
        (conns as f32 / self.config.max_conns.max(1) as f32).min(1.0)
    }
}

fn set_throttle(inner: &mut Inner, account: &str, bytes_per_sec: Option<u64>) {
    match bytes_per_sec {
        Some(rate) => {
            inner.throttles.insert(account.to_string(), rate.max(1));
        }
        None => {
            inner.throttles.remove(account);
        }
    }
    inner.account_buckets.remove(account);
    let Some(conns) = inner.by_account.get(account) else { return };
    for c in conns {
        if let Some(info) = inner.conns.get(c) {
            info.rate.store(bytes_per_sec.unwrap_or(info.ticket_rate).max(1), Ordering::Relaxed);
            info.throttled.store(bytes_per_sec.is_some() || info.ticket_throttled, Ordering::Relaxed);
        }
    }
}

/// Tell the account's devices on this relay whose ticket names `machine`
/// that it came or went.
fn presence_change(inner: &Inner, account: &str, machine: &str, online: bool) {
    let message = json!({"t": "presence", "machines": [{"id": machine, "state": if online { "online" } else { "offline" }, "since_ms": now_ms()}]});
    for c in inner.by_account.get(account).into_iter().flatten() {
        if let Some(info) = inner.conns.get(c) {
            if info.role == TicketRole::Device && info.peers.contains(machine) {
                info.out.text(&message);
            }
        }
    }
}
