//! The agent on the account's relays (`docs/relay-and-plans.md` §2.6–§2.9).
//!
//! The hub tells the agent which relays the account uses (`relay_set {relays,
//! emergency, limit, directory}`, after `welcome` and on every change); the
//! agent asks it for a ticket for each (`relay_tickets`, renewed before they
//! run out) and signs in on every one of them (`/v1/connect`, a
//! `RelayAgent` signature over the relay's origin). A device's channel then
//! arrives on whichever relay it chose: the same `OPEN` and Noise handshake as
//! on the hub, accepted by [`super::link`]'s rules, each connection with its
//! own routes — so a channel is known by (relay, `ch`).
//!
//! - **Where it dials.** A relay names its own origin and is not trusted
//!   with the networks behind this machine (`rh_proto::netpolicy`). Taken
//!   from `relay_set`: ids `[a-z][a-z0-9-]{0,31}`, origins `wss://host[:port]`
//!   of at most 256 characters, written as a URL parser writes them back, at
//!   a public name or address (no loopback, private, link-local, metadata,
//!   CGNAT, `localhost`, single-label or `*.internal`-style names); 16 relays
//!   to be on (`relays` and `emergency` together) and 64 to measure at most,
//!   the first of any id or origin listed twice; the rest is ignored. Every
//!   connection and every ping resolves the name here and dials only if every
//!   address it resolves to is public — or, for a public-looking name, if
//!   every one is a local proxy's fake address (`198.18.0.0/15`, fake-ip mode:
//!   the proxy carries the connection, TLS still verifies the name); pings
//!   follow no redirects and use no proxy. `DROPPEN_AGENT_RELAY_INSECURE=1` (local
//!   testing only) allows loopback besides, and plain `ws://` to loopback only.
//! - **Measuring.** Every 15 minutes (and when the list changes) the agent
//!   times `GET /v1/ping` on every listed relay — four requests, the first
//!   dropped (DNS, TLS), the median of the rest; a relay still being timed
//!   is not timed again meanwhile — and tells the hub
//!   (`relay_state {rtts, attached}`) when what it is signed in on changed or
//!   a round trip moved by more than 20%. Devices add it to their own to
//!   choose a relay.
//! - **Independent of the hub.** Relay connections live on while the hub is
//!   down or deploying: tickets last 15 minutes and a relay that lost its own
//!   link to the hub keeps established connections for two hours.
//! - **Leaving.** A relay dropped from the set is left once no channel is
//!   open on it. A relay that says `goaway` is draining: its channels move on
//!   (the device opens new ones elsewhere) and the agent reconnects once it
//!   is back.
//! - `DROPPEN_AGENT_RELAYS=0` turns all of this off (then devices can reach this
//!   machine only while the hub still relays; after the cut-over, not at all).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use rh_proto::frame::Frame;
use rh_proto::netpolicy;
use rh_proto::sig::{HubLogin, LoginRole};
use serde_json::{json, Value};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use super::hub::{self, hex, now_ms, unhex32, Backoff, Peer, HEARTBEAT, SILENCE};
use super::link::{drop_ended, drop_removed, expire_pending, on_channel_frame, Pending, Routes, Shared};

/// What this agent names in `agent_info` when it attaches to relays.
pub const FEATURE: &str = "relays/1";
/// Tickets are asked for again when they have less than this left.
const RENEW_BEFORE_MS: u64 = 5 * 60 * 1000;
const MEASURE_EVERY: Duration = Duration::from_secs(15 * 60);
/// At most this many relays to be on (`relays` and `emergency` together)…
const MAX_WANTED: usize = 16;
/// …and to measure (`directory`).
const MAX_DIRECTORY: usize = 64;
/// The longest origin taken.
const MAX_ORIGIN: usize = 256;

/// Whether this instance attaches to relays (`DROPPEN_AGENT_RELAYS=0` turns it off).
pub fn enabled() -> bool {
    crate::env::flag_or("DROPPEN_AGENT_RELAYS", true)
}

/// Whether relays on loopback may be used, `ws://` among them
/// (`DROPPEN_AGENT_RELAY_INSECURE=1`; local testing only).
fn insecure() -> bool {
    crate::env::var("DROPPEN_AGENT_RELAY_INSECURE").as_deref() == Some("1")
}

/// What the manager hears.
pub enum Event {
    /// `relay_set` or `relay_tickets` from the hub.
    Hub(Value),
    /// Signed in on the hub: where control messages go.
    HubUp(mpsc::UnboundedSender<String>),
    /// The hub connection went away.
    HubDown,
    /// A relay connection signed in (`true`) or ended.
    Attached(String, bool),
    /// Round trips measured: the relays tried, and those that answered.
    Measured(Vec<String>, BTreeMap<String, u32>),
    /// A relay refused this agent's ticket as revoked (or closed its
    /// connection 4403): the hub revoked the tickets issued to this machine so
    /// far, so that one is dropped and a fresh one asked for.
    Revoked(String),
}

/// What a relay closes a connection with when the hub revoked the ticket's
/// holder (`rh_relay::server::CLOSE_REVOKED`).
const CLOSE_REVOKED: u16 = 4403;
/// The relay refused this ticket as revoked ([`session`]'s error).
const REVOKED: &str = "the relay refused the ticket as revoked";

/// The manager's mailbox.
#[derive(Clone)]
pub struct Handle {
    tx: mpsc::UnboundedSender<Event>,
}

impl Handle {
    /// Tell the manager.
    pub fn send(&self, event: Event) {
        let _ = self.tx.send(event);
    }
}

/// Start the manager.
pub(super) fn spawn(shared: Arc<Shared>) -> Handle {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(manage(shared, rx, tx.clone()));
    Handle { tx }
}

/// A relay this agent should be on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Wanted {
    origin: String,
}

/// A running relay connection.
struct Conn {
    origin: String,
    ticket: watch::Sender<Option<(String, u64)>>,
    cancel: CancellationToken,
    channels: Arc<AtomicUsize>,
    done: tokio::task::JoinHandle<()>,
}

struct Manager {
    shared: Arc<Shared>,
    events: mpsc::UnboundedSender<Event>,
    wanted: BTreeMap<String, Wanted>,
    /// Every listed relay: `id → origin`.
    directory: BTreeMap<String, String>,
    tickets: HashMap<String, (String, u64)>,
    conns: HashMap<String, Conn>,
    hub: Option<mpsc::UnboundedSender<String>>,
    attached: BTreeSet<String>,
    rtts: BTreeMap<String, u32>,
    reported: Option<(BTreeMap<String, u32>, BTreeSet<String>)>,
    asked_ms: u64,
    /// `DROPPEN_AGENT_RELAY_INSECURE=1`: relays on loopback too (local testing).
    insecure: bool,
    /// Pings, by the same rules as connections (`None` if it cannot be built).
    http: Option<reqwest::Client>,
    measuring: Measuring,
}

async fn manage(shared: Arc<Shared>, mut rx: mpsc::UnboundedReceiver<Event>, events: mpsc::UnboundedSender<Event>) {
    let mut m = Manager {
        shared: shared.clone(),
        events,
        wanted: BTreeMap::new(),
        directory: BTreeMap::new(),
        tickets: HashMap::new(),
        conns: HashMap::new(),
        hub: None,
        attached: BTreeSet::new(),
        rtts: BTreeMap::new(),
        reported: None,
        asked_ms: 0,
        insecure: insecure(),
        http: None,
        measuring: Measuring::default(),
    };
    m.http = http_client(m.insecure);
    if m.http.is_none() {
        tracing::warn!("cannot build the HTTP client relays are measured with; not measuring");
    }
    let mut tick = tokio::time::interval(Duration::from_secs(30));
    let mut measure = tokio::time::interval(MEASURE_EVERY);
    measure.tick().await;
    loop {
        tokio::select! {
            event = rx.recv() => match event {
                Some(event) => m.on_event(event),
                None => break,
            },
            _ = tick.tick() => {
                m.ask_tickets(false);
                m.reconcile();
            }
            _ = measure.tick() => m.measure(),
            () = shared.cancel.cancelled() => break,
        }
    }
    for (_, conn) in m.conns.drain() {
        conn.cancel.cancel();
    }
}

/// A relay id: `[a-z][a-z0-9-]{0,31}`.
fn id_ok(id: &str) -> bool {
    let b = id.as_bytes();
    (1..=32).contains(&b.len()) && b[0].is_ascii_lowercase() && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// Whether this agent may dial a relay at `origin`, judged on its text:
/// `wss://host[:port]` written the one way a URL parser writes it back (lower
/// case, no default port, no user, path, query or fragment), at a public name
/// or address ([`netpolicy::host_allowed`]). Local testing (`insecure`)
/// allows loopback besides, and plain `ws://` to loopback only.
fn origin_ok(origin: &str, insecure: bool) -> bool {
    if origin.len() > MAX_ORIGIN || origin.contains('@') {
        return false;
    }
    let Ok(url) = reqwest::Url::parse(origin) else { return false };
    // No parser differentials: what is dialled is exactly what was checked.
    if url.as_str().strip_suffix('/') != Some(origin) || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let Some(host) = url.host_str() else { return false };
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    match url.scheme() {
        "wss" => netpolicy::host_allowed(host, insecure),
        "ws" => insecure && (host == "localhost" || host.parse().is_ok_and(netpolicy::is_loopback)),
        _ => false,
    }
}

/// The relays of `{id, origin}` lists, at most `cap` of them: sane ids
/// ([`id_ok`]), origins this agent may dial ([`origin_ok`]), the first of an
/// id or origin listed twice.
fn relay_list<'a>(lists: impl IntoIterator<Item = &'a Value>, cap: usize, insecure: bool) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut refused = 0usize;
    for r in lists.into_iter().flat_map(|v| v.as_array().into_iter().flatten()) {
        if out.len() == cap {
            refused += 1;
            break;
        }
        let (Some(id), Some(origin)) = (r["id"].as_str(), r["origin"].as_str()) else {
            refused += 1;
            continue;
        };
        if !id_ok(id) || !origin_ok(origin, insecure) {
            refused += 1;
            continue;
        }
        if out.iter().any(|(i, o)| i == id || o == origin) {
            continue;
        }
        out.push((id.to_string(), origin.to_string()));
    }
    if refused > 0 {
        tracing::warn!(
            refused,
            "ignored relays the hub listed: a bad id, an origin this agent does not dial, or too many"
        );
    }
    out
}

/// Whether `ip` is in `198.18.0.0/15` (or that range mapped into IPv6): the
/// benchmarking range, where a local proxy in fake-ip mode (Clash, mihomo,
/// Stash, Surge's enhanced mode) answers every name it then carries itself.
fn proxy_fake(ip: IpAddr) -> bool {
    let v4 = match ip {
        IpAddr::V4(v4) => v4,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4,
            None => return false,
        },
    };
    let [a, b, _, _] = v4.octets();
    a == 198 && (b == 18 || b == 19)
}

/// Whether `name` may resolve to a local proxy's fake addresses: a name that
/// looks public. Those are dialled over TLS only, which verifies the name
/// ([`origin_ok`] takes plain `ws://` to `localhost` and loopback literals
/// only, and `localhost` is not one of them).
fn fake_ok(name: &str) -> bool {
    !netpolicy::internal_name(name)
}

/// The addresses that may be dialled, all of them: none if any one is
/// internal (a name that resolves partly inside the network is refused).
/// `name` is what they were resolved from (`None`: an address literal).
///
/// One exception, for names only: a proxy in fake-ip mode answers every name
/// with an address in `198.18.0.0/15` and carries the connection itself, so
/// a relay behind such a proxy resolves to nothing else. A name ([`fake_ok`])
/// whose addresses are *all* in that range is dialled: whatever the proxy
/// connects it to, TLS verifies the relay's name, so it cannot steer this
/// agent to a plaintext service inside the network. An address literal in
/// that range, or a name that resolves to it and to anything refused, is
/// still refused (`rh_proto::netpolicy` itself, which the hub vets relay
/// origins with, keeps refusing the range).
fn vet(addrs: Vec<SocketAddr>, allow_loopback: bool, name: Option<&str>) -> Result<Vec<SocketAddr>, String> {
    if addrs.is_empty() {
        return Err("it resolves to no address".into());
    }
    if name.is_some_and(fake_ok) && addrs.iter().all(|a| proxy_fake(a.ip())) {
        return Ok(addrs);
    }
    match addrs.iter().find(|a| !netpolicy::allowed(a.ip(), allow_loopback)) {
        Some(a) => Err(format!("it resolves to {}, which this agent does not dial", a.ip())),
        None => Ok(addrs),
    }
}

/// Whether a relay was reached through a proxy's fake address yet: said once
/// a run, not on every reconnect or ping.
static THROUGH_FAKE_IP: AtomicBool = AtomicBool::new(false);

/// Resolve `host` here and keep it only if every address passes [`vet`].
async fn lookup(host: &str, port: u16, allow_loopback: bool) -> Result<Vec<SocketAddr>, String> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port)).await.map_err(|e| format!("{host}: {e}"))?.collect();
    let addrs = vet(addrs, allow_loopback, Some(host)).map_err(|e| format!("{host}: {e}"))?;
    if addrs.iter().all(|a| proxy_fake(a.ip())) && !THROUGH_FAKE_IP.swap(true, Ordering::Relaxed) {
        tracing::info!(
            %host,
            address = %addrs[0].ip(),
            "a relay resolves to a local proxy's fake address (fake-ip DNS); dialling it through the proxy, TLS still verifies the relay's name"
        );
    }
    Ok(addrs)
}

/// Where the host of a vetted origin may be dialled: an address literal as
/// it is, a name by [`lookup`].
async fn resolve(host: &str, port: u16, allow_loopback: bool) -> Result<Vec<SocketAddr>, String> {
    let bare = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    if !netpolicy::host_allowed(bare, allow_loopback) {
        return Err(format!("{host}: not a host this agent dials"));
    }
    match bare.parse() {
        Ok(ip) => vet(vec![SocketAddr::new(ip, port)], allow_loopback, None),
        Err(_) => lookup(bare, port, allow_loopback).await,
    }
}

/// The pings' resolver: names resolve to addresses that pass [`vet`] or to
/// nothing. (Address literals never reach a resolver: [`ping`] checks them.)
struct Resolver {
    allow_loopback: bool,
}

impl reqwest::dns::Resolve for Resolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        let allow_loopback = self.allow_loopback;
        Box::pin(async move {
            let addrs = resolve(&host, 0, allow_loopback).await?;
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// The client relays are pinged with: no redirects, no proxy (it would
/// resolve the name itself), names resolved by [`Resolver`].
fn http_client(insecure: bool) -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .dns_resolver(Arc::new(Resolver { allow_loopback: insecure }))
        .build()
        .ok()
}

/// The websocket to `<origin>/v1/connect`, over a TCP connection dialled
/// here to an address the rules allow; TLS (its SNI) and `Host` still name
/// the origin's host.
async fn connect(origin: &str, insecure: bool) -> Result<hub::Socket, String> {
    if !origin_ok(origin, insecure) {
        return Err(format!("{origin}: not an origin this agent dials"));
    }
    let url = reqwest::Url::parse(origin).map_err(|e| format!("{origin}: {e}"))?;
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
        return Err(format!("{origin}: no host"));
    };
    let addrs = resolve(host, port, insecure).await?;
    if url.scheme() == "ws" && addrs.iter().any(|a| !netpolicy::is_loopback(a.ip())) {
        return Err(format!("{origin}: plain ws:// only on this device"));
    }
    let mut failed = String::new();
    let mut stream = None;
    for addr in addrs {
        match tokio::net::TcpStream::connect(addr).await {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => failed = format!("{origin} ({addr}): {e}"),
        }
    }
    let stream = stream.ok_or(failed)?;
    let _ = stream.set_nodelay(true);
    let request = format!("{origin}/v1/connect");
    let (socket, _) = tokio_tungstenite::client_async_tls_with_config(request.as_str(), stream, None, None)
        .await
        .map_err(|e| format!("{request}: {e}"))?;
    Ok(socket)
}

/// The relays being measured, so a relay is measured once at a time.
#[derive(Clone, Default)]
struct Measuring(Arc<Mutex<BTreeSet<String>>>);

/// Relays being measured, until this is dropped.
struct InFlight {
    ids: Vec<String>,
    measuring: Measuring,
}

impl Measuring {
    /// Of `targets`, those not being measured already, now marked as being.
    fn begin(&self, targets: Vec<(String, String)>) -> (Vec<(String, String)>, InFlight) {
        let mut set = self.0.lock().expect("measuring");
        let targets: Vec<(String, String)> = targets.into_iter().filter(|(id, _)| set.insert(id.clone())).collect();
        let ids = targets.iter().map(|(id, _)| id.clone()).collect();
        (
            targets,
            InFlight {
                ids,
                measuring: self.clone(),
            },
        )
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let mut set = self.measuring.0.lock().unwrap_or_else(|e| e.into_inner());
        for id in &self.ids {
            set.remove(id);
        }
    }
}

impl Manager {
    fn on_event(&mut self, event: Event) {
        match event {
            Event::Hub(v) if v["t"] == "relay_set" => {
                let wanted: BTreeMap<String, Wanted> = relay_list([&v["relays"], &v["emergency"]], MAX_WANTED, self.insecure)
                    .into_iter()
                    .map(|(id, origin)| (id, Wanted { origin }))
                    .collect();
                let directory: BTreeMap<String, String> = relay_list([&v["directory"]], MAX_DIRECTORY, self.insecure).into_iter().collect();
                let new_dir = directory != self.directory;
                if wanted != self.wanted {
                    tracing::info!(relays = ?wanted.keys().collect::<Vec<_>>(), "the account's relays");
                }
                self.wanted = wanted;
                self.directory = directory;
                self.rtts.retain(|id, _| self.directory.contains_key(id));
                self.ask_tickets(true);
                self.reconcile();
                if new_dir {
                    self.measure();
                }
            }
            Event::Hub(v) if v["t"] == "relay_tickets" => {
                for t in v["tickets"].as_array().into_iter().flatten() {
                    let (Some(relay), Some(ticket), Some(expires)) = (t["relay"].as_str(), t["ticket"].as_str(), t["expires_ms"].as_u64()) else {
                        continue;
                    };
                    self.tickets.insert(relay.to_string(), (ticket.to_string(), expires));
                    if let Some(conn) = self.conns.get(relay) {
                        conn.ticket.send_replace(Some((ticket.to_string(), expires)));
                    }
                }
                for e in v["errors"].as_array().into_iter().flatten() {
                    tracing::info!(relay = %e["relay"], error = %e["error"], "no ticket for a relay");
                }
                self.reconcile();
            }
            Event::Hub(_) => {}
            Event::HubUp(text) => {
                self.hub = Some(text);
                self.reported = None;
                self.asked_ms = 0;
                self.ask_tickets(true);
                self.report();
            }
            Event::HubDown => self.hub = None,
            Event::Attached(id, up) => {
                if up {
                    self.attached.insert(id);
                } else {
                    self.attached.remove(&id);
                }
                self.report();
            }
            Event::Revoked(id) => {
                // Never try that ticket again: ask for a fresh one now (at
                // most every 20 s, else at the next tick) — not only once it
                // nears expiry, which kept this machine off the relay for
                // minutes after a revocation.
                tracing::info!(relay = %id, "a relay refused this machine's ticket as revoked; asking for a fresh one");
                self.tickets.remove(&id);
                if let Some(conn) = self.conns.get(&id) {
                    conn.ticket.send_replace(None);
                }
                self.ask_tickets(false);
            }
            Event::Measured(tried, rtts) => {
                for id in tried {
                    match rtts.get(&id) {
                        Some(ms) => self.rtts.insert(id, *ms),
                        None => self.rtts.remove(&id),
                    };
                }
                self.rtts.retain(|id, _| self.directory.contains_key(id));
                self.report();
            }
        }
    }

    /// Ask the hub for tickets for the wanted relays whose tickets run out
    /// soon (all of them with `now`).
    fn ask_tickets(&mut self, all: bool) {
        let Some(hub) = &self.hub else { return };
        let now = now_ms();
        let due: Vec<&String> = self
            .wanted
            .keys()
            .filter(|id| all || self.tickets.get(*id).is_none_or(|(_, exp)| *exp < now + RENEW_BEFORE_MS))
            .collect();
        if due.is_empty() || (!all && now.saturating_sub(self.asked_ms) < 20_000) {
            return;
        }
        self.asked_ms = now;
        for chunk in due.chunks(8) {
            let _ = hub.send(json!({"t": "relay_tickets", "relays": chunk}).to_string());
        }
    }

    /// Start a connection to every wanted relay with a ticket; leave the
    /// ones no longer wanted once their channels are gone.
    fn reconcile(&mut self) {
        let now = now_ms();
        self.conns.retain(|_, c| !c.done.is_finished());
        for (id, w) in &self.wanted {
            let Some(ticket) = self.tickets.get(id).filter(|(_, exp)| *exp > now) else {
                continue;
            };
            if let Some(conn) = self.conns.get(id) {
                if conn.origin == w.origin {
                    continue;
                }
                conn.cancel.cancel();
            }
            let (tx, rx) = watch::channel(Some(ticket.clone()));
            let cancel = CancellationToken::new();
            let channels = Arc::new(AtomicUsize::new(0));
            let done = tokio::spawn(run(
                self.shared.clone(),
                id.clone(),
                Target {
                    origin: w.origin.clone(),
                    insecure: self.insecure,
                },
                rx,
                cancel.clone(),
                channels.clone(),
                self.events.clone(),
            ));
            self.conns.insert(
                id.clone(),
                Conn {
                    origin: w.origin.clone(),
                    ticket: tx,
                    cancel,
                    channels,
                    done,
                },
            );
        }
        let leaving: Vec<String> = self
            .conns
            .iter()
            .filter(|(id, c)| !self.wanted.contains_key(*id) && c.channels.load(Ordering::Relaxed) == 0)
            .map(|(id, _)| id.clone())
            .collect();
        for id in leaving {
            tracing::info!(relay = %id, "leaving a relay the account no longer uses");
            if let Some(c) = self.conns.remove(&id) {
                c.cancel.cancel();
            }
        }
    }

    /// Time every listed relay (in the background), but none that is
    /// still being timed.
    fn measure(&self) {
        let Some(http) = self.http.clone() else { return };
        let targets: Vec<(String, String)> = self.directory.iter().map(|(i, o)| (i.clone(), o.clone())).collect();
        let (targets, in_flight) = self.measuring.begin(targets);
        if targets.is_empty() {
            return;
        }
        let events = self.events.clone();
        let insecure = self.insecure;
        tokio::spawn(async move {
            let mut rtts = BTreeMap::new();
            let mut tried = Vec::new();
            for (id, origin) in targets {
                if let Some(ms) = ping(&http, &origin, insecure).await {
                    rtts.insert(id.clone(), ms);
                }
                tried.push(id);
            }
            drop(in_flight);
            let _ = events.send(Event::Measured(tried, rtts));
        });
    }

    /// `relay_state` to the hub, when what it knows moved.
    fn report(&mut self) {
        let Some(hub) = &self.hub else { return };
        let moved = match &self.reported {
            None => true,
            Some((rtts, attached)) => {
                *attached != self.attached
                    || self.rtts.keys().ne(rtts.keys())
                    || self
                        .rtts
                        .iter()
                        .any(|(id, ms)| rtts.get(id).is_none_or(|old| ms.abs_diff(*old) * 5 > (*old).max(1)))
            }
        };
        if !moved {
            return;
        }
        let _ = hub.send(json!({"t": "relay_state", "rtts": self.rtts, "attached": self.attached}).to_string());
        self.reported = Some((self.rtts.clone(), self.attached.clone()));
    }
}

/// The round trip to a relay: four `GET /v1/ping`, the first dropped, the
/// median of the rest (ms). `http` is [`http_client`]'s.
async fn ping(http: &reqwest::Client, origin: &str, insecure: bool) -> Option<u32> {
    // Address literals never reach the client's resolver: checked here.
    if !origin_ok(origin, insecure) {
        return None;
    }
    let url = format!("{}/v1/ping", web_of(origin));
    let mut samples = Vec::new();
    for i in 0..4 {
        let t0 = Instant::now();
        let ok = http.get(&url).send().await.is_ok_and(|r| r.status().is_success());
        if !ok {
            return None;
        }
        if i > 0 {
            samples.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    Some(samples[samples.len() / 2].round().max(1.0) as u32)
}

/// `wss://x` → `https://x`, `ws://` → `http://`.
fn web_of(origin: &str) -> String {
    match origin.split_once("://") {
        Some(("ws", rest)) => format!("http://{rest}"),
        Some((_, rest)) => format!("https://{rest}"),
        None => origin.to_string(),
    }
}

/// Where a relay connection goes: the relay's origin, and whether local
/// testing allows loopback ([`insecure`]).
struct Target {
    origin: String,
    insecure: bool,
}

/// One relay, for as long as it is wanted: sign in, serve channels,
/// reconnect with jittered backoff.
async fn run(
    shared: Arc<Shared>,
    id: String,
    target: Target,
    mut ticket: watch::Receiver<Option<(String, u64)>>,
    cancel: CancellationToken,
    channels: Arc<AtomicUsize>,
    events: mpsc::UnboundedSender<Event>,
) {
    let mut backoff = Backoff::new(hub::BACKOFF_MIN, hub::BACKOFF_MAX, hub::STABLE);
    loop {
        let started = Instant::now();
        let current = ticket.borrow().clone();
        match current {
            Some((text, expires)) if expires > now_ms() => {
                let result = tokio::select! {
                    r = session(&shared, &id, &target, &text, &mut ticket, &channels, &events) => r,
                    () = cancel.cancelled() => return,
                };
                let _ = events.send(Event::Attached(id.clone(), false));
                channels.store(0, Ordering::Relaxed);
                match result {
                    Ok(()) => tracing::info!(relay = %id, "relay connection closed; reconnecting"),
                    Err(error) if error == REVOKED => {
                        // Not with this ticket again: wait for the one the
                        // manager asks for (it drops this one first).
                        tracing::info!(relay = %id, "relay connection refused as revoked; waiting for a fresh ticket");
                        let _ = events.send(Event::Revoked(id.clone()));
                        tokio::select! {
                            r = ticket.changed() => if r.is_err() { return },
                            () = cancel.cancelled() => return,
                        }
                        continue;
                    }
                    Err(error) => tracing::warn!(relay = %id, %error, "relay connection failed; reconnecting"),
                }
            }
            _ => {
                // No ticket, or it ran out: wait for the manager to bring one.
                tokio::select! {
                    r = ticket.changed() => if r.is_err() { return },
                    () = cancel.cancelled() => return,
                }
                continue;
            }
        }
        let wait = backoff.after(started.elapsed());
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = cancel.cancelled() => return,
        }
    }
}

/// One signed-in connection to a relay, until it ends.
async fn session(
    shared: &Arc<Shared>,
    id: &str,
    target: &Target,
    ticket: &str,
    tickets: &mut watch::Receiver<Option<(String, u64)>>,
    channels: &Arc<AtomicUsize>,
    events: &mpsc::UnboundedSender<Event>,
) -> Result<(), String> {
    // This device's identity key was renewed and the link has not restarted
    // with the new one yet: no sign-in with the retired key
    // (`docs/host-identity-rotation.md` §3.8).
    if super::rotation::renewed_away(&shared.state.lock().expect("agent state"), &shared.identity) {
        return Err("waiting for this device's renewed identity key".into());
    }
    // A staged key is kept and the hub link has not shown the key in use is
    // still current: this computer's chain may be stale (§3.5).
    if !super::rotation::device_role_may_sign_in(shared) {
        return Err("waiting for the hub link to sign in (an identity key renewal is staged)".into());
    }
    let (origin, insecure) = (target.origin.as_str(), target.insecure);
    let socket = tokio::time::timeout(hub::STEP, connect(origin, insecure))
        .await
        .map_err(|_| format!("{origin}: timed out"))??;
    let (mut sink, mut source) = socket.split();
    let challenge = hub::read_json(&mut source, Peer::Relay).await?;
    let nonce = challenge["nonce"]
        .as_str()
        .and_then(unhex32)
        .ok_or("the relay's challenge has no nonce")?;
    let (account, machine) = {
        let st = shared.state.lock().expect("agent state");
        (st.account.clone(), st.machine.clone())
    };
    let time_ms = now_ms();
    let sig = HubLogin {
        role: LoginRole::RelayAgent,
        account: account.clone(),
        id: machine.clone(),
        hub: origin.to_string(),
        nonce,
        time_ms,
    }
    .sign(shared.identity.signing_key())
    .map_err(|e| e.to_string())?;
    sink.send(Message::Text(
        super::traffic::relay_value(json!({"t": "login", "ticket": ticket, "time_ms": time_ms, "sig": hex(&sig)}))
            .to_string()
            .into(),
    ))
    .await
    .map_err(|e| e.to_string())?;
    let welcome = match hub::read_json(&mut source, Peer::Relay).await {
        Ok(v) => v,
        Err(e) if e.starts_with(&format!("the relay closed the connection: {CLOSE_REVOKED} ")) => return Err(REVOKED.into()),
        Err(e) => return Err(e),
    };
    if welcome["t"] != "welcome" {
        if welcome["message"] == "revoked" {
            return Err(REVOKED.into());
        }
        return Err(format!("the relay refused the sign-in: {}", welcome["message"]));
    }
    tracing::info!(relay = %id, %origin, "signed in on a relay");
    let _ = events.send(Event::Attached(id.to_string(), true));
    tickets.mark_unchanged();

    let (frames_tx, frames_rx) = mpsc::unbounded_channel::<Frame>();
    let (text_tx, text_rx) = mpsc::unbounded_channel::<String>();
    let writer = hub::spawn_writer(sink, frames_rx, text_rx, HEARTBEAT, super::traffic::relay_out);
    let mut chain = shared.chain_version.subscribe();
    let mut routes: Routes = HashMap::new();
    let mut pending = Pending::new();
    let mut last_heard = Instant::now();
    let mut check = tokio::time::interval(Duration::from_secs(5));
    // Entries a hello teaches are acted on as from the hub; mail they free is
    // acknowledged on the hub's link when it is up.
    let hub_text = || {
        shared
            .hub_text
            .lock()
            .expect("hub text")
            .clone()
            .unwrap_or_else(|| mpsc::unbounded_channel().0)
    };
    let result = loop {
        tokio::select! {
            m = source.next() => {
                let message = match m {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => break Err(e.to_string()),
                    None => break Ok(()),
                };
                last_heard = Instant::now();
                match message {
                    Message::Binary(bytes) => {
                        let Ok(frame) = Frame::decode(&bytes) else { continue };
                        if matches!(frame, Frame::Pong { .. } | Frame::Ping { .. }) {
                            continue;
                        }
                        on_channel_frame(shared, frame, &mut routes, &mut pending, &frames_tx, &hub_text());
                        channels.store(routes.len(), Ordering::Relaxed);
                    }
                    Message::Text(text) => {
                        let Ok(v) = serde_json::from_str::<Value>(super::traffic::relay_in(text.as_str())) else { continue };
                        match v["t"].as_str() {
                            Some("goaway") => tracing::info!(relay = %id, reason = %v["reason"], "the relay is draining"),
                            Some("error") => tracing::warn!(relay = %id, code = %v["code"], message = %v["message"], "the relay reported an error"),
                            _ => {}
                        }
                    }
                    Message::Close(frame) => {
                        let code = frame.as_ref().map(|f| u16::from(f.code));
                        if code == Some(CLOSE_REVOKED) {
                            break Err(REVOKED.into());
                        }
                        break Err(format!("closed ({code:?})"));
                    }
                    _ => {}
                }
            }
            r = tickets.changed() => {
                if r.is_err() {
                    break Ok(());
                }
                let next = tickets.borrow_and_update().clone();
                if let Some((t, _)) = next {
                    let _ = text_tx.send(json!({"t": "ticket", "ticket": t}).to_string());
                }
            }
            r = chain.changed() => {
                if r.is_ok() {
                    drop_removed(shared, &mut routes, &frames_tx);
                    channels.store(routes.len(), Ordering::Relaxed);
                }
            }
            _ = check.tick() => {
                if last_heard.elapsed() > SILENCE {
                    break Err("the relay went silent".into());
                }
                if writer.is_finished() {
                    break Err("the connection's writer stopped".into());
                }
                expire_pending(&mut pending, &frames_tx, Instant::now());
                drop_ended(&mut routes);
                channels.store(routes.len(), Ordering::Relaxed);
            }
        }
    };
    routes.clear();
    drop(frames_tx);
    drop(text_tx);
    writer.abort();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::ws::WebSocketUpgrade;
    use axum::routing::get;

    fn pair(id: &str, origin: &str) -> (String, String) {
        (id.to_string(), origin.to_string())
    }

    #[test]
    fn relay_lists_take_only_sane_entries() {
        let v = json!([
            {"id": "sin-1", "origin": "wss://sin-1.relay.example"},
            {"id": "x", "origin": "ftp://nope"},
            {"origin": "wss://no-id.example"},
            {"id": "loc-1", "origin": "ws://127.0.0.1:9000"},
        ]);
        assert_eq!(relay_list([&v], 16, false), vec![pair("sin-1", "wss://sin-1.relay.example")]);
        assert_eq!(
            relay_list([&v], 16, true),
            vec![pair("sin-1", "wss://sin-1.relay.example"), pair("loc-1", "ws://127.0.0.1:9000")],
            "plain ws:// on loopback only when testing locally"
        );
        assert_eq!(web_of("wss://a.example"), "https://a.example");
        assert_eq!(web_of("ws://127.0.0.1:1"), "http://127.0.0.1:1");
    }

    /// A relay (or a bad hub entry) could name any origin and have this
    /// machine dial into its own network.
    #[test]
    fn relay_origins_must_be_public_and_wss() {
        let long = format!("wss://{}.example", "a".repeat(MAX_ORIGIN));
        let refused = [
            "http://relay.example",
            "https://relay.example",
            "ws://relay.example",
            "ws://127.0.0.1:9000",
            "wss://127.0.0.1:9000",
            "wss://localhost:9000",
            "wss://[::1]:9000",
            "wss://10.0.0.5",
            "wss://192.168.1.10:8443",
            "wss://172.16.0.1",
            "wss://100.100.100.200",
            "wss://169.254.169.254",
            "wss://[fd00:ec2::254]",
            "wss://[fe80::1]",
            "wss://[::ffff:10.0.0.1]",
            "wss://0.0.0.0",
            "wss://metadata.google.internal",
            "wss://hub.railway.internal",
            "wss://printer.local",
            "wss://redis",
            "wss://0x7f.0.0.1",
            "wss://2130706433",
            "wss://user@relay.example",
            "wss://relay.example@10.0.0.5",
            "wss://relay.example\\@10.0.0.5",
            "wss://relay.example/path",
            "wss://relay.example?x=1",
            "wss://relay.example#x",
            "wss://Relay.Example",
            "wss://relay.example:443",
            "wss://relay example",
            "wss://",
            "",
            long.as_str(),
        ];
        for origin in refused {
            assert!(!origin_ok(origin, false), "{origin}");
        }
        for origin in [
            "ws://relay.example",
            "ws://10.0.0.5:9000",
            "ws://192.168.1.2",
            "wss://10.0.0.5",
            "wss://169.254.169.254",
            "wss://hub.railway.internal",
            "http://127.0.0.1:9000",
            "ws://127.0.0.1:9000/x",
        ] {
            assert!(!origin_ok(origin, true), "local testing allows loopback and nothing else: {origin}");
        }
        for origin in [
            "wss://sin-1.relay.example",
            "wss://relay.example:8443",
            "wss://8.8.8.8",
            "wss://[2606:4700::1111]",
        ] {
            assert!(origin_ok(origin, false), "{origin}");
            assert!(origin_ok(origin, true), "{origin}");
        }
        for origin in [
            "ws://127.0.0.1:9000",
            "ws://localhost:9000",
            "ws://[::1]:9000",
            "wss://127.0.0.1:9000",
            "wss://localhost:9000",
        ] {
            assert!(origin_ok(origin, true), "{origin}");
            assert!(!origin_ok(origin, false), "{origin}");
        }
        let v = json!(refused
            .iter()
            .enumerate()
            .map(|(i, o)| json!({"id": format!("r-{i}"), "origin": o}))
            .collect::<Vec<_>>());
        assert!(relay_list([&v], MAX_DIRECTORY, false).is_empty());
    }

    #[test]
    fn relay_lists_are_capped_with_sane_ids_and_no_duplicates() {
        for bad in ["", "Sin-1", "1-sin", "-sin", "sin_1", "sin.1", "sin 1", "sïn", &"a".repeat(33)] {
            let v = json!([{"id": bad, "origin": "wss://relay.example"}]);
            assert!(relay_list([&v], 16, false).is_empty(), "{bad:?}");
        }
        for good in ["a", "sin-1", "a0-", &"a".repeat(32)] {
            assert!(id_ok(good), "{good}");
        }
        let v = json!([
            {"id": "a", "origin": "wss://a.relay.example"},
            {"id": "a", "origin": "wss://other.relay.example"},
            {"id": "b", "origin": "wss://a.relay.example"},
            {"id": "c", "origin": "wss://c.relay.example"},
        ]);
        assert_eq!(
            relay_list([&v], 16, false),
            vec![pair("a", "wss://a.relay.example"), pair("c", "wss://c.relay.example")],
            "the first of an id or origin listed twice"
        );
        let many = |prefix: &str, n: usize| {
            json!((0..n)
                .map(|i| json!({"id": format!("{prefix}-{i}"), "origin": format!("wss://{prefix}-{i}.relay.example")}))
                .collect::<Vec<_>>())
        };
        assert_eq!(relay_list([&many("d", 100)], MAX_DIRECTORY, false).len(), 64);
        let (relays, emergency) = (many("r", 10), many("e", 10));
        let wanted = relay_list([&relays, &emergency], MAX_WANTED, false);
        assert_eq!(wanted.len(), 16, "relays and emergency together");
        assert_eq!(wanted[0].0, "r-0");
        assert_eq!(wanted[15].0, "e-5");
    }

    #[test]
    fn only_addresses_the_rules_allow_are_dialled() {
        let a = |s: &str| -> SocketAddr { s.parse().unwrap() };
        for name in [None, Some("relay.example")] {
            assert!(vet(vec![a("127.0.0.1:443")], false, name).is_err());
            assert!(vet(vec![a("[::1]:443")], false, name).is_err());
            assert!(vet(vec![a("10.0.0.5:443")], false, name).is_err());
            assert!(vet(vec![a("10.0.0.5:443")], true, name).is_err(), "local testing allows loopback, nothing else");
            assert!(vet(vec![a("169.254.169.254:80")], true, name).is_err());
            assert!(vet(vec![a("[::ffff:192.168.0.1]:443")], false, name).is_err());
            assert!(
                vet(vec![a("8.8.8.8:443"), a("10.0.0.5:443")], false, name).is_err(),
                "a name with one internal address is refused"
            );
            assert!(vet(Vec::new(), false, name).is_err());
            assert_eq!(vet(vec![a("8.8.8.8:443")], false, name), Ok(vec![a("8.8.8.8:443")]));
            assert_eq!(vet(vec![a("127.0.0.1:9000")], true, name), Ok(vec![a("127.0.0.1:9000")]));
        }
    }

    /// A proxy in fake-ip mode (Clash, mihomo, Stash, Surge) answers every
    /// name with an address in `198.18.0.0/15`: a relay's name that resolves
    /// to such addresses only is dialled (TLS verifies the name); nothing
    /// else about the rules moves.
    #[test]
    fn a_name_behind_a_fake_ip_proxy_is_dialled() {
        let a = |s: &str| -> SocketAddr { s.parse().unwrap() };
        let name = Some("sin-1-production.up.railway.app");
        for fake in ["198.18.0.92:443", "198.18.0.0:443", "198.19.255.255:443", "[::ffff:198.18.0.92]:443"] {
            for insecure in [false, true] {
                assert_eq!(vet(vec![a(fake)], insecure, name), Ok(vec![a(fake)]), "{fake}");
            }
        }
        let all_fake = vec![a("198.18.0.92:443"), a("198.19.1.2:443")];
        assert_eq!(vet(all_fake.clone(), false, name), Ok(all_fake), "every address fake");

        // An address literal in the range is not a proxy's answer.
        assert!(vet(vec![a("198.18.0.92:443")], false, None).is_err());
        assert!(vet(vec![a("198.18.0.92:443")], true, None).is_err());
        for literal in ["wss://198.18.0.92", "wss://198.19.0.1:8443", "wss://[::ffff:198.18.0.92]"] {
            assert!(!origin_ok(literal, false) && !origin_ok(literal, true), "{literal}");
        }
        // Fake and anything refused together: refused.
        for other in ["10.0.0.5:443", "127.0.0.1:443", "169.254.169.254:443", "100.100.100.200:443", "[fd00::1]:443", "8.8.8.8:443"] {
            assert!(vet(vec![a("198.18.0.92:443"), a(other)], false, name).is_err(), "{other}");
            assert!(vet(vec![a(other), a("198.18.0.92:443")], false, name).is_err(), "{other}");
        }
        // Next to the range: refused or public as before.
        assert!(!proxy_fake("198.17.255.255".parse().unwrap()) && !proxy_fake("198.20.0.0".parse().unwrap()));
        assert!(!proxy_fake("::198.18.0.92".parse().unwrap()) && !proxy_fake("64:ff9b::c612:5c".parse().unwrap()));
        // Names internal by their form never get the exception: `localhost`
        // (the only name plain `ws://` may dial, local testing) stays loopback.
        for internal in ["localhost", "redis", "hub.railway.internal", "printer.local"] {
            assert!(vet(vec![a("198.18.0.92:443")], true, Some(internal)).is_err(), "{internal}");
        }
        assert!(!origin_ok("ws://sin-1-production.up.railway.app", true), "plain ws:// stays loopback only");
    }

    /// Hosts are judged on their text before anything resolves: internal
    /// names and refused literals (the fake-ip range among them) go nowhere.
    #[tokio::test]
    async fn hosts_are_refused_before_they_resolve() {
        for host in ["198.18.0.92", "[::ffff:198.18.0.92]", "hub.railway.internal", "metadata.google.internal", "redis", "box.local"] {
            for insecure in [false, true] {
                let e = resolve(host, 443, insecure).await.unwrap_err();
                assert!(e.ends_with("not a host this agent dials"), "{host}: {e}");
            }
        }
        assert!(resolve("localhost", 443, false).await.unwrap_err().ends_with("not a host this agent dials"));
    }

    /// The addresses a name resolves to are what is checked: `localhost`
    /// resolves to loopback, which only local testing may dial.
    #[tokio::test]
    async fn names_are_checked_after_they_resolve() {
        assert!(lookup("localhost", 9000, false).await.is_err());
        let addrs = lookup("localhost", 9000, true).await.unwrap();
        assert!(!addrs.is_empty() && addrs.iter().all(|a| a.ip().is_loopback() && a.port() == 9000));
        let refusing = Resolver { allow_loopback: false };
        let name: reqwest::dns::Name = "localhost".parse().unwrap();
        assert!(reqwest::dns::Resolve::resolve(&refusing, name).await.is_err());
    }

    async fn serve(app: axum::Router) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    }

    /// A relay that refuses this machine's ticket as revoked (the hub revoked
    /// its tickets) is not asked again with that ticket until it nears expiry:
    /// the manager hears `Revoked` at once, and the connection waits for a
    /// fresh ticket, then signs in with it.
    #[tokio::test]
    async fn a_ticket_refused_as_revoked_is_not_tried_again() {
        use axum::extract::ws::{CloseFrame, Message as WsMessage};
        let logins: Arc<Mutex<Vec<String>>> = Arc::default();
        let seen = logins.clone();
        let relay = serve(axum::Router::new().route(
            "/v1/connect",
            get(move |ws: WebSocketUpgrade| {
                let seen = seen.clone();
                async move {
                    ws.on_upgrade(move |mut socket| async move {
                        let challenge = json!({"t": "challenge", "nonce": hex(&[7u8; 32])}).to_string();
                        let _ = socket.send(WsMessage::Text(challenge.into())).await;
                        let Some(Ok(WsMessage::Text(login))) = socket.recv().await else { return };
                        let login: Value = serde_json::from_str(login.as_str()).unwrap();
                        let ticket = login["ticket"].as_str().unwrap_or_default().to_string();
                        seen.lock().unwrap().push(ticket.clone());
                        if ticket == "revoked" {
                            let error = json!({"t": "error", "code": "denied", "message": "revoked"}).to_string();
                            let _ = socket.send(WsMessage::Text(error.into())).await;
                            let close = CloseFrame { code: CLOSE_REVOKED, reason: "revoked".into() };
                            let _ = socket.send(WsMessage::Close(Some(close))).await;
                            return;
                        }
                        let welcome = json!({"t": "welcome", "relay": "r-1", "id": "m-1", "expires_ms": now_ms() + 600_000}).to_string();
                        let _ = socket.send(WsMessage::Text(welcome.into())).await;
                        while let Some(Ok(_)) = socket.recv().await {}
                    })
                }
            }),
        ))
        .await;
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::sessions::Paths::for_instance(dir.path().to_path_buf(), "relays");
        let st = super::super::state::AgentState::enrolling("http://hub.invalid", "a-1", "m-1", [0; 16], now_ms());
        let identity = rh_proto::identity::MachineIdentity::from_seed(&[5; 32]);
        let shared = super::super::link::test_shared(
            &paths,
            st,
            identity,
            super::super::streams::Handle::detached(),
            super::super::rotation::no_restart(),
        );
        let (tickets, rx) = watch::channel(Some(("revoked".to_string(), now_ms() + 600_000)));
        let (events, mut heard) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let target = Target {
            origin: format!("ws://{relay}"),
            insecure: true,
        };
        tokio::spawn(run(shared, "r-1".into(), target, rx, cancel.clone(), Arc::new(AtomicUsize::new(0)), events));
        // Refused once, and not again with that ticket (with backoff alone it
        // would be tried again within a second or two).
        tokio::time::sleep(Duration::from_secs(4)).await;
        assert_eq!(*logins.lock().unwrap(), vec!["revoked".to_string()], "the revoked ticket tried again");
        // The manager heard it, to drop the ticket and ask for a fresh one.
        let mut revoked = Vec::new();
        while let Ok(event) = heard.try_recv() {
            if let Event::Revoked(id) = event {
                revoked.push(id);
            }
        }
        assert_eq!(revoked, vec!["r-1".to_string()]);
        // The manager drops it, then brings a fresh one: signed in with that.
        tickets.send_replace(None);
        tickets.send_replace(Some(("fresh".to_string(), now_ms() + 600_000)));
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match heard.recv().await {
                    Some(Event::Attached(id, true)) if id == "r-1" => break,
                    Some(_) => continue,
                    None => panic!("the connection ended"),
                }
            }
        })
        .await
        .expect("signed in with the fresh ticket");
        assert_eq!(*logins.lock().unwrap(), vec!["revoked".to_string(), "fresh".to_string()]);
        cancel.cancel();
    }

    /// The manager, told a ticket was refused as revoked, drops it and asks
    /// the hub for a fresh one at once — though the old one had long to run.
    #[tokio::test]
    async fn the_manager_asks_for_a_fresh_ticket_once_one_is_revoked() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::sessions::Paths::for_instance(dir.path().to_path_buf(), "relays");
        let st = super::super::state::AgentState::enrolling("http://hub.invalid", "a-1", "m-1", [0; 16], now_ms());
        let shared = super::super::link::test_shared(
            &paths,
            st,
            rh_proto::identity::MachineIdentity::from_seed(&[5; 32]),
            super::super::streams::Handle::detached(),
            super::super::rotation::no_restart(),
        );
        let (hub, mut asked) = mpsc::unbounded_channel::<String>();
        let mut m = Manager {
            shared,
            events: mpsc::unbounded_channel().0,
            wanted: BTreeMap::from([("r-1".to_string(), Wanted { origin: "wss://r-1.relay.example".into() })]),
            directory: BTreeMap::new(),
            tickets: HashMap::from([("r-1".to_string(), ("old".to_string(), now_ms() + 14 * 60 * 1000))]),
            conns: HashMap::new(),
            hub: Some(hub),
            attached: BTreeSet::new(),
            rtts: BTreeMap::new(),
            reported: None,
            asked_ms: 0,
            insecure: false,
            http: None,
            measuring: Measuring::default(),
        };
        m.ask_tickets(false);
        assert!(asked.try_recv().is_err(), "a ticket with 14 minutes left is not asked for again");
        m.on_event(Event::Revoked("r-1".into()));
        assert!(!m.tickets.contains_key("r-1"), "the revoked ticket is dropped");
        let request: Value = serde_json::from_str(&asked.try_recv().expect("a fresh ticket asked for")).unwrap();
        assert_eq!(request, json!({"t": "relay_tickets", "relays": ["r-1"]}));
    }

    /// A `/v1/ping` that counts its hits.
    async fn counted_ping() -> (SocketAddr, Arc<AtomicUsize>) {
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        let app = axum::Router::new().route(
            "/v1/ping",
            get(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                async { "ok" }
            }),
        );
        (serve(app).await, hits)
    }

    #[tokio::test]
    async fn pings_follow_no_redirects() {
        let (target, hits) = counted_ping().await;
        let to = format!("http://{target}/v1/ping");
        let redirector = serve(axum::Router::new().route(
            "/v1/ping",
            get(move || async move { (axum::http::StatusCode::FOUND, [(axum::http::header::LOCATION, to)]) }),
        ))
        .await;
        let http = http_client(true).unwrap();
        assert_eq!(ping(&http, &format!("ws://{redirector}"), true).await, None);
        assert_eq!(hits.load(Ordering::SeqCst), 0, "the redirect was not followed");
        assert!(ping(&http, &format!("ws://{target}"), true).await.is_some());
    }

    #[tokio::test]
    async fn pings_reach_no_internal_address() {
        let (target, hits) = counted_ping().await;
        let http = http_client(false).unwrap();
        assert_eq!(ping(&http, &format!("ws://{target}"), false).await, None);
        // A name that resolves to loopback, past the text check: the resolver refuses it.
        assert!(http.get(format!("http://localhost:{}/v1/ping", target.port())).send().await.is_err());
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn relay_connections_go_only_where_the_rules_allow() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let refused = listener.local_addr().unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = accepted.clone();
        tokio::spawn(async move {
            while let Ok((_stream, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
            }
        });
        for origin in [
            format!("ws://{refused}"),
            format!("wss://{refused}"),
            format!("ws://localhost:{}", refused.port()),
        ] {
            assert!(connect(&origin, false).await.is_err(), "{origin}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(accepted.load(Ordering::SeqCst), 0, "nothing dialled");

        // Local testing: the handshake runs over the connection dialled here.
        let relay = serve(axum::Router::new().route(
            "/v1/connect",
            get(|ws: WebSocketUpgrade| async move {
                ws.on_upgrade(|mut socket| async move {
                    let _ = socket.send(axum::extract::ws::Message::Text(r#"{"t":"challenge"}"#.into())).await;
                    let _ = socket.recv().await;
                })
            }),
        ))
        .await;
        for origin in [format!("ws://{relay}"), format!("ws://localhost:{}", relay.port())] {
            let mut socket = connect(&origin, true).await.unwrap();
            assert_eq!(hub::read_json(&mut socket, Peer::Relay).await.unwrap()["t"], "challenge", "{origin}");
        }
    }

    #[test]
    fn a_relay_is_measured_once_at_a_time() {
        let ids = |targets: &[(String, String)]| targets.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>();
        let directory = |names: &[&str]| names.iter().map(|n| pair(n, &format!("wss://{n}.relay.example"))).collect();
        let measuring = Measuring::default();
        let (first, first_flight) = measuring.begin(directory(&["a", "b"]));
        assert_eq!(ids(&first), ["a", "b"]);
        let (second, second_flight) = measuring.begin(directory(&["a", "b", "c"]));
        assert_eq!(ids(&second), ["c"], "a and b are still being measured");
        drop(first_flight);
        let (third, _third_flight) = measuring.begin(directory(&["a", "b", "c"]));
        assert_eq!(ids(&third), ["a", "b"], "c is still being measured");
        drop(second_flight);
        let (fourth, _) = measuring.begin(directory(&["c"]));
        assert_eq!(ids(&fourth), ["c"]);
    }
}
