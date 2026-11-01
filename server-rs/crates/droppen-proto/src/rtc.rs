//! A WebRTC DataChannel peer on str0m (sans-IO WebRTC, pure-Rust crypto)
//! and tokio's UDP sockets (feature `rtc`): what the agent answers a
//! browser's offer with, and what a headless device offers with.
//!
//! - **Sockets** are bound per peer and closed with it: one on this
//!   machine's IPv4 default-route address and one on its IPv6 one (the
//!   address the OS would send from towards the internet; no packet is sent to
//!   learn it). Loopback and link-local addresses are never offered unless
//!   [`RtcOptions::loopback`] asks (tests on a machine without a route only).
//!   The same-machine case does not need them: a browser on this machine
//!   reaches the default-route address like any other host.
//! - **Candidates**: host candidates on those sockets (in the SDP), and
//!   server-reflexive ones from public STUN ([`DEFAULT_STUN`]: Cloudflare,
//!   Google), trickled as they come. A browser hides its LAN address behind an
//!   mDNS name str0m cannot resolve; those candidates are skipped, and the
//!   browser's checks arriving from it become peer-reflexive candidates — IPv4
//!   and IPv6 alike. No TURN: the hub is the fallback.
//! - **Messages**: one reliable, ordered DataChannel ([`crate::path::DC_LABEL`]).
//!   [`Peer::outgoing`] takes messages to write (held while SCTP's buffer is
//!   full, so [`Peer::queued`] counts what has not reached the wire);
//!   [`Peer::messages`] yields what arrives. Dropping every `outgoing` sender
//!   tears the peer down and closes its sockets; the peer going away closes
//!   `messages`.
//! - [`RtcOptions::blackhole`] drops every datagram both ways while set: a
//!   network that stops carrying UDP, for tests and the end-to-end harness.

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use rand_core::{OsRng, RngCore};
use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::channel::ChannelId;
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc, RtcConfig};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

/// Public STUN servers asked for this machine's public address.
pub const DEFAULT_STUN: [&str; 2] = ["stun.cloudflare.com:3478", "stun.l.google.com:19302"];
/// SCTP's buffer is refilled when it drains below this.
const BUFFERED_LOW: usize = 256 * 1024;

#[derive(Debug, Clone)]
pub struct RtcOptions {
    /// STUN servers (`host:port`); empty for none.
    pub stun: Vec<String>,
    /// Offer host candidates on the default-route addresses.
    pub default_route: bool,
    /// Offer loopback host candidates too (tests on a machine without a route).
    pub loopback: bool,
    /// While set, every datagram is dropped both ways.
    pub blackhole: Option<Arc<AtomicBool>>,
}

impl Default for RtcOptions {
    fn default() -> Self {
        Self {
            stun: DEFAULT_STUN.iter().map(|s| s.to_string()).collect(),
            default_route: true,
            loopback: false,
            blackhole: None,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RtcError {
    #[error("no address to bind (no default route, and loopback not allowed)")]
    NoAddress,
    #[error("{0}")]
    Sdp(String),
    #[error("{0}")]
    Io(String),
}

/// What the peer tells its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerEvent {
    /// A local candidate to trickle to the other side (an SDP `candidate:`
    /// line), or `None` once gathering is over.
    Candidate(Option<String>),
    /// The DataChannel is open; `pair` is the (local, remote) addresses its
    /// packets go between, as far as known.
    Open { pair: Option<(SocketAddr, SocketAddr)> },
    /// The peer is gone.
    Closed(String),
}

enum Ctl {
    Remote(String),
    Answer(String),
    Close,
}

/// Remote candidates, the answer (offering side), closing.
#[derive(Clone)]
pub struct PeerControl(mpsc::UnboundedSender<Ctl>);

impl PeerControl {
    /// A candidate line from the other side (`candidate:…`, with or without `a=`).
    pub fn remote_candidate(&self, line: &str) {
        let _ = self.0.send(Ctl::Remote(line.trim().trim_start_matches("a=").to_string()));
    }

    /// The other side's answer to this peer's offer.
    pub fn answer(&self, sdp: &str) {
        let _ = self.0.send(Ctl::Answer(sdp.to_string()));
    }

    pub fn close(&self) {
        let _ = self.0.send(Ctl::Close);
    }
}

/// One WebRTC peer with one DataChannel, running in a task of its own.
pub struct Peer {
    /// This side's DTLS certificate fingerprint (SHA-256).
    pub local_fp: [u8; 32],
    /// The addresses its host candidates are on.
    pub hosts: Vec<SocketAddr>,
    pub control: PeerControl,
    pub events: mpsc::UnboundedReceiver<PeerEvent>,
    /// DataChannel messages received.
    pub messages: mpsc::UnboundedReceiver<Vec<u8>>,
    /// DataChannel messages to send.
    pub outgoing: mpsc::UnboundedSender<Vec<u8>>,
    /// Bytes given to `outgoing` and not yet handed to SCTP.
    pub queued: Arc<AtomicUsize>,
}

static CRYPTO: Once = Once::new();

/// This machine's address towards the internet in one family, if it has a
/// usable one (no packet is sent).
fn route_ip(v6: bool) -> Option<IpAddr> {
    let (bind, target) = if v6 {
        ("[::]:0", "[2606:4700:4700::1111]:53")
    } else {
        ("0.0.0.0:0", "1.1.1.1:53")
    };
    let socket = std::net::UdpSocket::bind(bind).ok()?;
    socket.connect(target).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    usable(ip).then_some(ip)
}

/// Not loopback, unspecified or link-local.
pub fn usable(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_unspecified() && !v4.is_link_local(),
        IpAddr::V6(v6) => !v6.is_loopback() && !v6.is_unspecified() && (v6.segments()[0] & 0xffc0) != 0xfe80,
    }
}

async fn bind(opts: &RtcOptions) -> Result<Vec<Arc<UdpSocket>>, RtcError> {
    let mut ips = Vec::new();
    if opts.default_route {
        ips.extend([route_ip(false), route_ip(true)].into_iter().flatten());
    }
    if opts.loopback {
        ips.push(IpAddr::V4(Ipv4Addr::LOCALHOST));
        ips.push(IpAddr::V6(Ipv6Addr::LOCALHOST));
    }
    let mut sockets = Vec::new();
    for ip in ips {
        if let Ok(s) = UdpSocket::bind(SocketAddr::new(ip, 0)).await {
            sockets.push(Arc::new(s));
        }
    }
    if sockets.is_empty() {
        return Err(RtcError::NoAddress);
    }
    Ok(sockets)
}

fn fingerprint(rtc: &mut Rtc) -> [u8; 32] {
    let fp = rtc.direct_api().local_dtls_fingerprint().clone();
    fp.bytes.as_slice().try_into().expect("a sha-256 fingerprint")
}

enum Role {
    Answering,
    Offering { pending: SdpPendingOffer },
}

impl Peer {
    /// Answer `offer`: the running peer and the answer's SDP (with this
    /// side's host candidates in it).
    pub async fn answer(offer: &str, opts: &RtcOptions) -> Result<(Peer, String), RtcError> {
        let (mut rtc, sockets) = Self::prepare(opts).await?;
        let offer = SdpOffer::from_sdp_string(offer).map_err(|e| RtcError::Sdp(format!("the offer: {e}")))?;
        let answer = rtc.sdp_api().accept_offer(offer).map_err(|e| RtcError::Sdp(format!("answering: {e}")))?;
        let sdp = answer.to_sdp_string();
        Ok((Self::spawn(rtc, sockets, opts, Role::Answering), sdp))
    }

    /// Offer one DataChannel: the running peer (hand it the answer with
    /// [`PeerControl::answer`]) and the offer's SDP.
    pub async fn offer(opts: &RtcOptions) -> Result<(Peer, String), RtcError> {
        let (mut rtc, sockets) = Self::prepare(opts).await?;
        let mut api = rtc.sdp_api();
        api.add_channel(crate::path::DC_LABEL.into());
        let (offer, pending) = api.apply().ok_or_else(|| RtcError::Sdp("nothing to offer".into()))?;
        let sdp = offer.to_sdp_string();
        Ok((Self::spawn(rtc, sockets, opts, Role::Offering { pending }), sdp))
    }

    async fn prepare(opts: &RtcOptions) -> Result<(Rtc, Vec<Arc<UdpSocket>>), RtcError> {
        CRYPTO.call_once(|| str0m::crypto::from_feature_flags().install_process_default());
        let sockets = bind(opts).await?;
        let mut rtc = RtcConfig::new().build(Instant::now());
        for s in &sockets {
            let local = s.local_addr().map_err(|e| RtcError::Io(e.to_string()))?;
            let c = Candidate::host(local, "udp").map_err(|e| RtcError::Sdp(e.to_string()))?;
            rtc.add_local_candidate(c);
        }
        Ok((rtc, sockets))
    }

    fn spawn(mut rtc: Rtc, sockets: Vec<Arc<UdpSocket>>, opts: &RtcOptions, role: Role) -> Peer {
        let local_fp = fingerprint(&mut rtc);
        let hosts: Vec<SocketAddr> = sockets.iter().filter_map(|s| s.local_addr().ok()).collect();
        let (ctl_tx, ctl_rx) = mpsc::unbounded_channel();
        let (events_tx, events) = mpsc::unbounded_channel();
        let (messages_tx, messages) = mpsc::unbounded_channel();
        let (outgoing, outgoing_rx) = mpsc::unbounded_channel();
        let queued = Arc::new(AtomicUsize::new(0));
        let runner = Runner {
            rtc,
            sockets,
            locals: hosts.clone(),
            role: Some(role),
            events: events_tx,
            messages: messages_tx,
            queued: queued.clone(),
            channel: None,
            pending: VecDeque::new(),
            blackhole: opts.blackhole.clone(),
            last_pair: None,
            stun: opts.stun.clone(),
        };
        tokio::spawn(runner.run(ctl_rx, outgoing_rx));
        Peer {
            local_fp,
            hosts,
            control: PeerControl(ctl_tx),
            events,
            messages,
            outgoing,
            queued,
        }
    }
}

struct Runner {
    rtc: Rtc,
    sockets: Vec<Arc<UdpSocket>>,
    locals: Vec<SocketAddr>,
    role: Option<Role>,
    events: mpsc::UnboundedSender<PeerEvent>,
    messages: mpsc::UnboundedSender<Vec<u8>>,
    queued: Arc<AtomicUsize>,
    channel: Option<ChannelId>,
    /// Messages not yet handed to SCTP.
    pending: VecDeque<Vec<u8>>,
    blackhole: Option<Arc<AtomicBool>>,
    last_pair: Option<(SocketAddr, SocketAddr)>,
    stun: Vec<String>,
}

impl Runner {
    fn dark(&self) -> bool {
        self.blackhole.as_ref().is_some_and(|b| b.load(Ordering::Relaxed))
    }

    async fn run(mut self, mut ctl: mpsc::UnboundedReceiver<Ctl>, mut outgoing: mpsc::UnboundedReceiver<Vec<u8>>) {
        // Every socket's datagrams into one queue.
        let (udp_tx, mut udp_rx) = mpsc::channel::<(usize, Vec<u8>, SocketAddr, Instant)>(1024);
        let readers: Vec<_> = self
            .sockets
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let (s, tx) = (s.clone(), udp_tx.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 2048];
                    while let Ok((n, from)) = s.recv_from(&mut buf).await {
                        if tx.send((i, buf[..n].to_vec(), from, Instant::now())).await.is_err() {
                            break;
                        }
                    }
                })
            })
            .collect();
        drop(udp_tx);
        let mut gather = Gather::new(&self.locals, &self.stun).await;
        if !self.dark() {
            gather.send(&self.sockets).await;
        }
        let mut gathering = true;
        let mut ctl_open = true;
        let why = 'run: loop {
            // Everything str0m wants done now, then how long it can wait.
            let timeout = loop {
                if !self.rtc.is_alive() {
                    break 'run "the peer connection ended".to_string();
                }
                let output = match self.rtc.poll_output() {
                    Ok(o) => o,
                    Err(e) => break 'run format!("rtc: {e}"),
                };
                match output {
                    Output::Timeout(t) => break t,
                    Output::Transmit(t) => {
                        if self.dark() {
                            continue;
                        }
                        if let Some(i) = self.locals.iter().position(|l| *l == t.source) {
                            let _ = self.sockets[i].send_to(&t.contents, t.destination).await;
                            self.last_pair = Some((t.source, t.destination));
                        }
                    }
                    Output::Event(event) => match event {
                        Event::ChannelOpen(id, _) => {
                            self.channel = Some(id);
                            if let Some(mut ch) = self.rtc.channel(id) {
                                ch.set_buffered_amount_low_threshold(BUFFERED_LOW);
                            }
                            let _ = self.events.send(PeerEvent::Open { pair: self.last_pair });
                            if let Err(e) = self.flush() {
                                break 'run e;
                            }
                        }
                        Event::ChannelData(data) => {
                            if self.messages.send(data.data).is_err() {
                                // Nobody reads it any more.
                                break 'run "the owner let the messages go".to_string();
                            }
                        }
                        Event::ChannelBufferedAmountLow(_) => {
                            if let Err(e) = self.flush() {
                                break 'run e;
                            }
                        }
                        Event::ChannelClose(_) => break 'run "the data channel closed".to_string(),
                        Event::IceConnectionStateChange(IceConnectionState::Disconnected) if self.channel.is_some() => {
                            // str0m may recover; the channel's own probes decide
                            // when the path is dead.
                        }
                        _ => {}
                    },
                }
            };
            let wait = timeout.saturating_duration_since(Instant::now());
            tokio::select! {
                datagram = udp_rx.recv() => {
                    let Some((i, data, from, at)) = datagram else { break "the sockets closed".to_string() };
                    if self.dark() {
                        continue;
                    }
                    if let Some(mapped) = gather.answer(i, &data) {
                        if mapped != self.locals[i] {
                            if let Ok(c) = Candidate::server_reflexive(mapped, self.locals[i], "udp") {
                                if let Some(added) = self.rtc.add_local_candidate(c) {
                                    let _ = self.events.send(PeerEvent::Candidate(Some(added.to_sdp_string())));
                                }
                            }
                        }
                        if gathering && gather.done() {
                            gathering = false;
                            let _ = self.events.send(PeerEvent::Candidate(None));
                        }
                        continue;
                    }
                    // Anything not STUN, DTLS or SCTP is someone else's datagram.
                    if let Ok(receive) = Receive::new(Protocol::Udp, from, self.locals[i], &data) {
                        if let Err(e) = self.rtc.handle_input(Input::Receive(at, receive)) {
                            break format!("rtc input: {e}");
                        }
                    }
                }
                c = ctl.recv(), if ctl_open => match c {
                    Some(Ctl::Remote(line)) => {
                        // An mDNS host name str0m cannot resolve fails to parse:
                        // the browser's checks from it arrive as peer-reflexive.
                        if let Ok(c) = Candidate::from_sdp_string(&line) {
                            self.rtc.add_remote_candidate(c);
                        }
                    }
                    Some(Ctl::Answer(sdp)) => {
                        let Some(Role::Offering { pending }) = self.role.take() else { continue };
                        let answer = match SdpAnswer::from_sdp_string(&sdp) {
                            Ok(a) => a,
                            Err(e) => break format!("the answer: {e}"),
                        };
                        if let Err(e) = self.rtc.sdp_api().accept_answer(pending, answer) {
                            break format!("accepting the answer: {e}");
                        }
                    }
                    Some(Ctl::Close) => break "closed".to_string(),
                    // The control handle went; `outgoing` decides when to stop.
                    None => ctl_open = false,
                },
                m = outgoing.recv() => match m {
                    Some(m) => {
                        self.pending.push_back(m);
                        if let Err(e) = self.flush() {
                            break e;
                        }
                    }
                    None => break "the owner let the peer go".to_string(),
                },
                () = tokio::time::sleep(wait) => {
                    if let Err(e) = self.rtc.handle_input(Input::Timeout(Instant::now())) {
                        break format!("rtc timeout: {e}");
                    }
                }
                () = gather.next_retry() => {
                    if !self.dark() {
                        gather.send(&self.sockets).await;
                    } else {
                        gather.skip();
                    }
                    if gathering && gather.done() {
                        gathering = false;
                        let _ = self.events.send(PeerEvent::Candidate(None));
                    }
                }
            }
        };
        self.shutdown().await;
        for r in readers {
            r.abort();
        }
        let _ = self.events.send(PeerEvent::Closed(why));
    }

    /// Close politely — the DataChannel's stream reset, then SCTP shutdown
    /// and DTLS close_notify — so the other side hears it at once rather
    /// than by timing out; then stop.
    async fn shutdown(&mut self) {
        if let Some(id) = self.channel.take() {
            self.rtc.direct_api().close_data_channel(id);
            self.drain().await;
        }
        if self.rtc.is_alive() && self.rtc.close().is_ok() {
            self.drain().await;
        }
        self.rtc.disconnect();
    }

    /// Send what str0m has to send right now (a few rounds of its timers).
    async fn drain(&mut self) {
        for _ in 0..4 {
            let _ = self.rtc.handle_input(Input::Timeout(Instant::now()));
            let mut sent = 0;
            for _ in 0..64 {
                if !self.rtc.is_alive() {
                    return;
                }
                match self.rtc.poll_output() {
                    Ok(Output::Transmit(t)) => {
                        sent += 1;
                        if !self.dark() {
                            if let Some(i) = self.locals.iter().position(|l| *l == t.source) {
                                let _ = self.sockets[i].send_to(&t.contents, t.destination).await;
                            }
                        }
                    }
                    Ok(Output::Event(_)) => {}
                    Ok(Output::Timeout(_)) | Err(_) => break,
                }
            }
            if sent == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Hand SCTP what it takes.
    fn flush(&mut self) -> Result<(), String> {
        let Some(id) = self.channel else { return Ok(()) };
        let Some(mut ch) = self.rtc.channel(id) else {
            return Err("the data channel is gone".into());
        };
        while let Some(m) = self.pending.front() {
            match ch.write(true, m) {
                Ok(true) => {
                    let m = self.pending.pop_front().expect("front");
                    self.queued.fetch_sub(m.len(), Ordering::Relaxed);
                }
                Ok(false) => break,
                Err(e) => return Err(format!("writing to the data channel: {e}")),
            }
        }
        Ok(())
    }
}

/// One STUN question: from socket `socket` to `server`.
struct Ask {
    socket: usize,
    server: SocketAddr,
    tid: [u8; 12],
    answered: bool,
}

/// Server-reflexive discovery: a STUN binding request from each socket to
/// each server of its family, retried at 0, 250 ms, 750 ms and 1.75 s.
struct Gather {
    asks: Vec<Ask>,
    sent: u32,
    next: tokio::time::Instant,
}

impl Gather {
    async fn new(locals: &[SocketAddr], servers: &[String]) -> Self {
        let mut asks = Vec::new();
        for name in servers {
            let Ok(Ok(addrs)) = tokio::time::timeout(Duration::from_secs(2), tokio::net::lookup_host(name.as_str())).await else {
                continue;
            };
            let addrs: Vec<SocketAddr> = addrs.collect();
            for (socket, local) in locals.iter().enumerate() {
                if !usable(local.ip()) {
                    continue;
                }
                if let Some(server) = addrs.iter().find(|a| a.is_ipv4() == local.is_ipv4()) {
                    let mut tid = [0u8; 12];
                    OsRng.fill_bytes(&mut tid);
                    asks.push(Ask {
                        socket,
                        server: *server,
                        tid,
                        answered: false,
                    });
                }
            }
        }
        Self {
            asks,
            sent: 0,
            next: tokio::time::Instant::now(),
        }
    }

    fn done(&self) -> bool {
        self.sent >= 4 || self.asks.iter().all(|a| a.answered)
    }

    async fn send(&mut self, sockets: &[Arc<UdpSocket>]) {
        for ask in self.asks.iter().filter(|a| !a.answered) {
            let _ = sockets[ask.socket].send_to(&stun::binding_request(&ask.tid), ask.server).await;
        }
        self.skip();
    }

    fn skip(&mut self) {
        self.sent += 1;
        self.next = tokio::time::Instant::now() + Duration::from_millis(250 << (self.sent - 1).min(2));
    }

    async fn next_retry(&self) {
        if self.done() {
            std::future::pending::<()>().await;
        }
        tokio::time::sleep_until(self.next).await;
    }

    /// A STUN answer to one of ours on socket `socket`: the mapped address.
    fn answer(&mut self, socket: usize, data: &[u8]) -> Option<SocketAddr> {
        let (tid, mapped) = stun::parse_binding_success(data)?;
        let ask = self.asks.iter_mut().find(|a| a.socket == socket && a.tid == tid && !a.answered)?;
        ask.answered = true;
        Some(mapped)
    }
}

/// Just enough of STUN (RFC 8489) to learn a mapped address.
pub mod stun {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

    pub const MAGIC: [u8; 4] = [0x21, 0x12, 0xa4, 0x42];

    /// A Binding request with no attributes.
    pub fn binding_request(tid: &[u8; 12]) -> [u8; 20] {
        let mut m = [0u8; 20];
        m[..2].copy_from_slice(&[0x00, 0x01]);
        m[4..8].copy_from_slice(&MAGIC);
        m[8..].copy_from_slice(tid);
        m
    }

    /// A Binding success response: its transaction id and the mapped address
    /// (XOR-MAPPED-ADDRESS, else MAPPED-ADDRESS).
    pub fn parse_binding_success(m: &[u8]) -> Option<([u8; 12], SocketAddr)> {
        if m.len() < 20 || m[..2] != [0x01, 0x01] || m[4..8] != MAGIC {
            return None;
        }
        let len = usize::from(u16::from_be_bytes([m[2], m[3]]));
        let body = m.get(20..20 + len)?;
        let tid: [u8; 12] = m[8..20].try_into().ok()?;
        let mut plain = None;
        let mut at = 0;
        while at + 4 <= body.len() {
            let kind = u16::from_be_bytes([body[at], body[at + 1]]);
            let alen = usize::from(u16::from_be_bytes([body[at + 2], body[at + 3]]));
            let value = body.get(at + 4..at + 4 + alen)?;
            match kind {
                0x0020 => return Some((tid, address(value, Some(&tid))?)),
                0x0001 => plain = address(value, None),
                _ => {}
            }
            at += 4 + alen.div_ceil(4) * 4;
        }
        plain.map(|a| (tid, a))
    }

    fn address(v: &[u8], xor: Option<&[u8; 12]>) -> Option<SocketAddr> {
        let mut key = [0u8; 16];
        if let Some(tid) = xor {
            key[..4].copy_from_slice(&MAGIC);
            key[4..].copy_from_slice(tid);
        }
        let port = u16::from_be_bytes([v.get(2)? ^ key[0], v.get(3)? ^ key[1]]);
        let ip = match v.get(1)? {
            0x01 if v.len() >= 8 => IpAddr::V4(Ipv4Addr::from(std::array::from_fn::<u8, 4, _>(|i| v[4 + i] ^ key[i]))),
            0x02 if v.len() >= 20 => IpAddr::V6(Ipv6Addr::from(std::array::from_fn::<u8, 16, _>(|i| v[4 + i] ^ key[i]))),
            _ => return None,
        };
        Some(SocketAddr::new(ip, port))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const TID: [u8; 12] = [0xb7, 0xe7, 0xa7, 0x01, 0xbc, 0x34, 0xd6, 0x86, 0xfa, 0x87, 0xdf, 0xae];

        fn response(attrs: &[u8]) -> Vec<u8> {
            let mut m = vec![0x01, 0x01];
            m.extend_from_slice(&(attrs.len() as u16).to_be_bytes());
            m.extend_from_slice(&MAGIC);
            m.extend_from_slice(&TID);
            m.extend_from_slice(attrs);
            m
        }

        /// RFC 5769 §2.2 and §2.3.
        #[test]
        fn rfc_5769_responses_and_garbage() {
            let v4 = response(&[
                0x80, 0x22, 0x00, 0x0b, 0x74, 0x65, 0x73, 0x74, 0x20, 0x76, 0x65, 0x63, 0x74, 0x6f, 0x72, 0x20, //
                0x00, 0x20, 0x00, 0x08, 0x00, 0x01, 0xa1, 0x47, 0xe1, 0x12, 0xa6, 0x43,
            ]);
            assert_eq!(parse_binding_success(&v4), Some((TID, "192.0.2.1:32853".parse().unwrap())));
            let v6 = response(&[
                0x00, 0x20, 0x00, 0x14, 0x00, 0x02, 0xa1, 0x47, 0x01, 0x13, 0xa9, 0xfa, 0xa5, 0xd3, 0xf1, 0x79, 0xbc, 0x25, 0xf4, 0xb5, 0xbe, 0xd2,
                0xb9, 0xd9,
            ]);
            assert_eq!(
                parse_binding_success(&v6),
                Some((TID, "[2001:db8:1234:5678:11:2233:4455:6677]:32853".parse().unwrap()))
            );
            assert_eq!(parse_binding_success(&binding_request(&TID)), None, "a request is not an answer");
            assert_eq!(parse_binding_success(b"\x16\xfe\xfd not stun at all, a DTLS record"), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::sdp_fingerprint;

    fn opts(loopback: bool, default_route: bool) -> RtcOptions {
        RtcOptions {
            stun: Vec::new(),
            default_route,
            loopback,
            blackhole: None,
        }
    }

    /// Offer, answer and candidates between two peers in this process; both
    /// sides' first event of interest.
    async fn connect(a_opts: RtcOptions, b_opts: RtcOptions, limit: Duration) -> (Peer, Peer, bool) {
        let (mut a, offer) = Peer::offer(&a_opts).await.unwrap();
        let (mut b, answer) = Peer::answer(&offer, &b_opts).await.unwrap();
        assert_eq!(
            sdp_fingerprint(&offer).unwrap(),
            a.local_fp,
            "the offer names the offering side's certificate"
        );
        assert_eq!(sdp_fingerprint(&answer).unwrap(), b.local_fp);
        a.control.answer(&answer);
        let opened = tokio::time::timeout(limit, async {
            let (mut a_open, mut b_open) = (false, false);
            while !(a_open && b_open) {
                tokio::select! {
                    Some(e) = a.events.recv() => match e {
                        PeerEvent::Candidate(Some(c)) => b.control.remote_candidate(&c),
                        PeerEvent::Open { .. } => a_open = true,
                        PeerEvent::Closed(why) => panic!("a closed: {why}"),
                        _ => {}
                    },
                    Some(e) = b.events.recv() => match e {
                        PeerEvent::Candidate(Some(c)) => a.control.remote_candidate(&c),
                        PeerEvent::Open { .. } => b_open = true,
                        PeerEvent::Closed(why) => panic!("b closed: {why}"),
                        _ => {}
                    },
                }
            }
        })
        .await
        .is_ok();
        (a, b, opened)
    }

    async fn exchange(a: &mut Peer, b: &mut Peer) {
        a.queued.fetch_add(5, Ordering::Relaxed);
        a.outgoing.send(b"hello".to_vec()).unwrap();
        assert_eq!(b.messages.recv().await.unwrap(), b"hello");
        let big = vec![7u8; 60_000];
        b.queued.fetch_add(big.len(), Ordering::Relaxed);
        b.outgoing.send(big.clone()).unwrap();
        assert_eq!(a.messages.recv().await.unwrap(), big);
        assert_eq!(a.queued.load(Ordering::Relaxed), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_peers_open_a_data_channel_over_loopback() {
        let (mut a, mut b, opened) = connect(opts(true, false), opts(true, false), Duration::from_secs(10)).await;
        assert!(opened);
        exchange(&mut a, &mut b).await;
        // Dropping the sender tears the peer down; the other side hears it close.
        drop(a.outgoing);
        let closed = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match b.events.recv().await {
                    Some(PeerEvent::Closed(_)) | None => return,
                    _ => {}
                }
            }
        })
        .await;
        assert!(closed.is_ok(), "the other side saw the channel close");
    }

    /// The same-machine case as production runs it: host candidates on the
    /// default-route addresses only, no loopback. Skipped on a machine without
    /// a route.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_same_machine_connects_over_its_default_route_address() {
        if route_ip(false).is_none() && route_ip(true).is_none() {
            eprintln!("skipped: no default route");
            return;
        }
        let (mut a, mut b, opened) = connect(opts(false, true), opts(false, true), Duration::from_secs(10)).await;
        assert!(opened);
        for host in a.hosts.iter().chain(&b.hosts) {
            assert!(usable(host.ip()), "never loopback or link-local: {host}");
        }
        exchange(&mut a, &mut b).await;
    }

    /// UDP blocked: nothing opens (and the caller's timeout decides).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn nothing_opens_when_udp_is_blocked() {
        let blocked = RtcOptions {
            blackhole: Some(Arc::new(AtomicBool::new(true))),
            ..opts(true, false)
        };
        let (_a, _b, opened) = connect(blocked, opts(true, false), Duration::from_secs(3)).await;
        assert!(!opened);
    }

    #[test]
    fn link_local_and_loopback_are_not_usable() {
        for ip in ["127.0.0.1", "::1", "169.254.1.2", "fe80::1", "0.0.0.0", "::"] {
            assert!(!usable(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["192.168.1.2", "203.0.113.9", "2001:db8::1", "fd00::1"] {
            assert!(usable(ip.parse().unwrap()), "{ip}");
        }
    }
}
