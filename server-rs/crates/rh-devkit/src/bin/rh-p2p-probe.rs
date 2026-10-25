//! The machine end of the direct-path measurement (not the production
//! feature): how often can a browser reach this machine directly (WebRTC,
//! ICE hole punching, no TURN), and how much faster is that than the hub?
//!
//!   rh-p2p-probe [--hub https://api.droppen.app] [--page http://localhost:5180] [--room <32 hex>] [--new-room] [--results <file>]
//!
//! It joins a signaling room on the hub (`/v1/probe/rooms/{room}`, only
//! there when the hub runs with `DROPPEN_HUB_P2P_PROBE=1`) and prints the page to
//! open, `<page>/p2p-probe.html#<room>`. The page is no part of a web build:
//! the web dev server serves it (`web/dev/`; `RH_DEV_HUB=<hub> pnpm dev:web`,
//! which proxies the room to the hub), at `--page`. For every page load it answers the
//! page's WebRTC offer with a DataChannel peer (str0m, sans-IO): host
//! candidates on this machine's default-route addresses, server-reflexive ones
//! from public STUN (Cloudflare, Google), peer-reflexive ones as checks arrive
//! (Chrome and Safari hide the browser's own LAN address behind mDNS, so the
//! same-machine and same-LAN cases pair that way). It echoes the page's pings
//! on the DataChannel, and the pings that come through the room — the hub path,
//! browser → hub → machine → hub → browser, the shape terminal echo has. The
//! page's measurements come back (over the DataChannel, else the room) and
//! are appended as one JSON line each to
//! `~/.local/share/rh-p2p-probe/results.jsonl`, with this end's view added.
//!
//! **A measurement tool only.** It carries no session data and touches no
//! remote-hand state: its files live in `~/.local/share/rh-p2p-probe/`, never
//! under `~/.local/share/remote-hand/`. The room id is 128 random bits (kept
//! in that directory so the link survives a restart; `--new-room` makes a new
//! one); the hub relays the room's messages without storing them and sees only
//! that a probe ran. UDP sockets are bound per page load and closed with it.

use std::collections::HashMap;
use std::io::Write as _;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use rand::RngCore;
use serde_json::{json, Value};
use str0m::change::SdpOffer;
use str0m::channel::ChannelId;
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, CandidateKind, Event, Input, Output, Rtc, RtcConfig};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

const STUN_SERVERS: [&str; 2] = ["stun.cloudflare.com:3478", "stun.l.google.com:19302"];
/// The longest a page's session may run.
const SESSION_LIMIT: Duration = Duration::from_secs(180);
/// Sessions at once.
const MAX_SESSIONS: usize = 8;
/// The hub closes a room socket after 15 minutes; rejoin before that while idle.
const ROTATE_AFTER: Duration = Duration::from_secs(12 * 60);

macro_rules! log {
    ($($arg:tt)*) => {
        eprintln!("{} {}", utc_now(), format!($($arg)*))
    };
}

fn main() {
    let args = match Args::parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("rh-p2p-probe: {e}");
            std::process::exit(2);
        }
    };
    str0m::crypto::from_feature_flags().install_process_default();
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("a runtime");
    if let Err(e) = runtime.block_on(run(args)) {
        log!("fatal: {e}");
        std::process::exit(1);
    }
}

struct Args {
    hub: String,
    /// Where the probe page is served: the web dev server (`web/build/dev-tools.ts`).
    page: String,
    room: Option<String>,
    new_room: bool,
    results: Option<PathBuf>,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut args = Args {
            hub: "https://api.droppen.app".into(),
            page: "http://localhost:5180".into(),
            room: None,
            new_room: false,
            results: None,
        };
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            match a.as_str() {
                "--hub" => args.hub = it.next().ok_or("--hub takes a URL")?.trim_end_matches('/').to_string(),
                "--page" => args.page = it.next().ok_or("--page takes a URL")?.trim_end_matches('/').to_string(),
                "--room" => args.room = Some(it.next().ok_or("--room takes 32 hex digits")?),
                "--new-room" => args.new_room = true,
                "--results" => args.results = Some(it.next().ok_or("--results takes a file")?.into()),
                "-h" | "--help" => {
                    println!("rh-p2p-probe [--hub https://api.droppen.app] [--page http://localhost:5180] [--room <32 hex>] [--new-room] [--results <file>]");
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument {other:?}")),
            }
        }
        if !(args.hub.starts_with("https://") || args.hub.starts_with("http://")) {
            return Err("--hub is an http(s) URL".into());
        }
        if !(args.page.starts_with("https://") || args.page.starts_with("http://")) {
            return Err("--page is an http(s) URL".into());
        }
        Ok(args)
    }
}

fn valid_room(room: &str) -> bool {
    room.len() == 32 && room.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `~/.local/share/rh-p2p-probe`, made if need be.
fn state_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    let dir = PathBuf::from(home).join(".local/share/rh-p2p-probe");
    if dir.components().any(|c| c.as_os_str() == "remote-hand") {
        return Err("refusing to write under a remote-hand directory".into());
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
}

fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The room: `--room`, else the one kept in the state directory, else a new one.
fn room(args: &Args, dir: &std::path::Path) -> Result<String, String> {
    if let Some(room) = &args.room {
        return if valid_room(room) {
            Ok(room.clone())
        } else {
            Err("--room is 32 lower-case hex digits".into())
        };
    }
    let file = dir.join("room");
    if !args.new_room {
        if let Ok(kept) = std::fs::read_to_string(&file) {
            let kept = kept.trim().to_string();
            if valid_room(&kept) {
                return Ok(kept);
            }
        }
    }
    let room = random_hex(16);
    std::fs::write(&file, format!("{room}\n")).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(room)
}

/// The results file: one JSON line per page load.
struct Results {
    path: PathBuf,
    file: Mutex<std::fs::File>,
}

impl Results {
    fn open(path: PathBuf) -> Result<Self, String> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self {
            path,
            file: Mutex::new(file),
        })
    }

    fn append(&self, record: &Value) {
        let mut line = record.to_string();
        line.push('\n');
        let mut file = self.file.lock().expect("results file");
        if let Err(e) = file.write_all(line.as_bytes()).and_then(|()| file.flush()) {
            log!("writing {}: {e}", self.path.display());
        }
    }
}

async fn run(args: Args) -> Result<(), String> {
    let dir = state_dir()?;
    let room = room(&args, &dir)?;
    let results = Arc::new(Results::open(args.results.clone().unwrap_or_else(|| dir.join("results.jsonl")))?);
    std::fs::write(dir.join("probe.pid"), format!("{}\n", std::process::id())).map_err(|e| e.to_string())?;
    let page = format!("{}/p2p-probe.html#{room}", args.page);
    let ws_url = format!(
        "{}/v1/probe/rooms/{room}?role=probe",
        args.hub.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1)
    );
    log!("rh-p2p-probe {} (pid {})", env!("CARGO_PKG_VERSION"), std::process::id());
    log!("results: {}", results.path.display());
    log!("open this page: {page}");
    println!("{page}");
    let (out_tx, out_rx) = mpsc::unbounded_channel();
    let (done_tx, done_rx) = mpsc::unbounded_channel();
    let mut probe = Probe {
        sessions: HashMap::new(),
        out_tx,
        out_rx,
        done_tx,
        done_rx,
        results,
        last_activity: Instant::now(),
    };
    let mut backoff = Duration::from_secs(1);
    loop {
        let connected = tokio::select! {
            r = tokio_tungstenite::connect_async_with_config(ws_url.as_str(), None, true) => r,
            _ = tokio::signal::ctrl_c() => return Ok(()),
        };
        match connected {
            Ok((ws, _)) => {
                log!("joined the room");
                backoff = Duration::from_secs(1);
                match probe.serve(ws).await {
                    Ok(Rejoin::Now) => continue,
                    Ok(Rejoin::Stop) => return Ok(()),
                    Ok(Rejoin::Later(why)) => log!("left the room: {why}"),
                    Err(e) => log!("room connection failed: {e}"),
                }
            }
            Err(e) => log!("cannot join the room: {e}"),
        }
        let jitter = Duration::from_millis(u64::from(rand::rngs::OsRng.next_u32() % 500));
        tokio::select! {
            () = tokio::time::sleep(backoff + jitter) => {}
            _ = tokio::signal::ctrl_c() => return Ok(()),
        }
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

/// What a page's session is told from the room.
enum ToSession {
    Candidate(String),
    Result(Value),
    Bye,
}

struct Probe {
    sessions: HashMap<String, mpsc::UnboundedSender<ToSession>>,
    out_tx: mpsc::UnboundedSender<String>,
    out_rx: mpsc::UnboundedReceiver<String>,
    done_tx: mpsc::UnboundedSender<String>,
    done_rx: mpsc::UnboundedReceiver<String>,
    results: Arc<Results>,
    last_activity: Instant,
}

enum Rejoin {
    Now,
    Later(String),
    Stop,
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

impl Probe {
    async fn serve(&mut self, mut ws: Ws) -> Result<Rejoin, String> {
        let joined = Instant::now();
        let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
        loop {
            tokio::select! {
                message = ws.next() => match message {
                    Some(Ok(Message::Text(text))) => {
                        if let Some(reply) = self.on_room_message(text.as_str()) {
                            ws.send(Message::text(reply)).await.map_err(|e| e.to_string())?;
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        return Ok(Rejoin::Later(frame.map_or_else(|| "closed".into(), |f| format!("{} {}", u16::from(f.code), f.reason))));
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e.to_string()),
                    None => return Ok(Rejoin::Later("the socket ended".into())),
                },
                Some(out) = self.out_rx.recv() => {
                    ws.send(Message::text(out)).await.map_err(|e| e.to_string())?;
                }
                Some(sid) = self.done_rx.recv() => {
                    self.sessions.remove(&sid);
                }
                _ = heartbeat.tick() => {
                    let idle = self.sessions.is_empty() && self.last_activity.elapsed() > Duration::from_secs(60);
                    if joined.elapsed() > ROTATE_AFTER && idle {
                        let _ = ws.close(None).await;
                        return Ok(Rejoin::Now);
                    }
                    ws.send(Message::Ping(Vec::new().into())).await.map_err(|e| e.to_string())?;
                }
                _ = tokio::signal::ctrl_c() => {
                    let _ = ws.close(None).await;
                    return Ok(Rejoin::Stop);
                }
            }
        }
    }

    /// Handle one message from a page; the immediate reply, if any (pings are
    /// answered right here, on the room's socket).
    fn on_room_message(&mut self, text: &str) -> Option<String> {
        let message: Value = serde_json::from_str(text).ok()?;
        if let Some(notice) = message.get("hub").and_then(Value::as_str) {
            if notice == "peers" {
                log!("room: {} page(s) here", message["pages"]);
            }
            return None;
        }
        let sid = message
            .get("s")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 64)?
            .to_string();
        let kind = message.get("t").and_then(Value::as_str)?;
        self.last_activity = Instant::now();
        match kind {
            "ping" => Some(json!({"t": "pong", "s": sid, "seq": message["seq"]}).to_string()),
            "hello" => {
                log!("page {sid} says hello ({})", message["label"].as_str().unwrap_or("?"));
                Some(json!({"t": "hello", "s": sid, "probe": env!("CARGO_PKG_VERSION")}).to_string())
            }
            "offer" => {
                let sdp = message.get("sdp").and_then(Value::as_str)?.to_string();
                if self.sessions.contains_key(&sid) {
                    return None;
                }
                if self.sessions.len() >= MAX_SESSIONS {
                    return Some(json!({"t": "busy", "s": sid}).to_string());
                }
                let (tx, rx) = mpsc::unbounded_channel();
                self.sessions.insert(sid.clone(), tx);
                let session = Session {
                    sid,
                    label: message["label"].as_str().unwrap_or_default().to_string(),
                    out: self.out_tx.clone(),
                    results: self.results.clone(),
                };
                let done = self.done_tx.clone();
                tokio::spawn(async move {
                    let sid = session.sid.clone();
                    if let Err(e) = session.run(sdp, rx).await {
                        log!("session {sid}: {e}");
                    }
                    let _ = done.send(sid);
                });
                None
            }
            "cand" => {
                if let (Some(tx), Some(c)) = (self.sessions.get(&sid), message.get("c").and_then(Value::as_str)) {
                    let _ = tx.send(ToSession::Candidate(c.to_string()));
                }
                None
            }
            "bye" => {
                if let Some(tx) = self.sessions.get(&sid) {
                    let _ = tx.send(ToSession::Bye);
                }
                None
            }
            "result" => {
                let result = message.get("r").cloned().unwrap_or(Value::Null);
                match self.sessions.get(&sid) {
                    Some(tx) => {
                        let _ = tx.send(ToSession::Result(result));
                    }
                    None => {
                        self.results.append(&record(&sid, "hub", result, Value::Null));
                        log!("page {sid}: result saved (via the hub; its session had ended)");
                    }
                }
                Some(json!({"t": "saved", "s": sid}).to_string())
            }
            _ => None,
        }
    }
}

/// One results line.
fn record(sid: &str, via: &str, page: Value, probe: Value) -> Value {
    json!({"at": utc_now(), "sid": sid, "via": via, "page": page, "probe": probe})
}

/// One page load's WebRTC peer.
struct Session {
    sid: String,
    label: String,
    out: mpsc::UnboundedSender<String>,
    results: Arc<Results>,
}

/// What the probe saw of a session, for its results line.
#[derive(Default)]
struct Seen {
    host: Vec<String>,
    srflx: Vec<(String, SocketAddr, SocketAddr)>,
    remote: HashMap<&'static str, u32>,
    ice: Vec<(String, u128)>,
    connected_ms: Option<u128>,
    channel_ms: Option<u128>,
    path: Option<(SocketAddr, SocketAddr)>,
    pings: u32,
}

impl Seen {
    fn to_json(&self) -> Value {
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "host": self.host,
            "srflx": self.srflx.iter().map(|(server, base, mapped)| json!({"server": server, "base": base.to_string(), "mapped": mapped.to_string()})).collect::<Vec<_>>(),
            "nat_hint": nat_hint(&self.srflx),
            "remote_candidates": self.remote,
            "ice": self.ice.iter().map(|(s, ms)| json!([s, ms])).collect::<Vec<_>>(),
            "connected_ms": self.connected_ms,
            "channel_open_ms": self.channel_ms,
            "path": self.path.map(|(local, remote)| json!({"local": local.to_string(), "remote": remote.to_string()})),
            "dc_pings_echoed": self.pings,
        })
    }
}

/// This machine's own mapping behaviour, from the two STUN servers' answers
/// to the same socket.
fn nat_hint(srflx: &[(String, SocketAddr, SocketAddr)]) -> Value {
    let mut out = serde_json::Map::new();
    for v4 in [true, false] {
        let seen: Vec<_> = srflx.iter().filter(|(_, base, _)| base.is_ipv4() == v4).collect();
        let family = if v4 { "ipv4" } else { "ipv6" };
        let hint = match seen.as_slice() {
            [] => continue,
            [(_, base, mapped), ..] if seen.iter().all(|(_, _, m)| m == mapped) && base == mapped => "no NAT (the address is public)",
            [(_, _, mapped), rest @ ..] if !rest.is_empty() && rest.iter().all(|(_, _, m)| m == mapped) => {
                "same mapping for both STUN servers (endpoint-independent: hole punching friendly)"
            }
            [_] => "only one STUN server answered",
            _ => "mapping differs per destination (symmetric-like: hole punching unlikely)",
        };
        out.insert(family.into(), hint.into());
    }
    Value::Object(out)
}

/// This machine's address towards the internet, per family (no packet is sent).
fn route_ip(target: &str) -> Option<IpAddr> {
    let bind = if target.starts_with('[') { "[::]:0" } else { "0.0.0.0:0" };
    let socket = std::net::UdpSocket::bind(bind).ok()?;
    socket.connect(target).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    let usable = match ip {
        IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_unspecified() && !v4.is_link_local(),
        IpAddr::V6(v6) => !v6.is_loopback() && !v6.is_unspecified() && (v6.segments()[0] & 0xffc0) != 0xfe80,
    };
    usable.then_some(ip)
}

impl Session {
    fn send(&self, mut message: Value) {
        message["s"] = Value::String(self.sid.clone());
        let _ = self.out.send(message.to_string());
    }

    async fn run(self, offer_sdp: String, mut inbox: mpsc::UnboundedReceiver<ToSession>) -> Result<(), String> {
        let started = Instant::now();
        let ms = || started.elapsed().as_millis();
        let mut seen = Seen::default();
        log!("page {} ({}): offer", self.sid, self.label);

        // Sockets on this machine's default-route addresses, for this page load only.
        let mut sockets: Vec<Arc<UdpSocket>> = Vec::new();
        for target in ["1.1.1.1:53", "[2606:4700:4700::1111]:53"] {
            if let Some(ip) = route_ip(target) {
                match UdpSocket::bind(SocketAddr::new(ip, 0)).await {
                    Ok(s) => sockets.push(Arc::new(s)),
                    Err(e) => log!("bind {ip}: {e}"),
                }
            }
        }
        if sockets.is_empty() {
            return Err("no usable local address".into());
        }
        let locals: Vec<SocketAddr> = sockets.iter().map(|s| s.local_addr().expect("bound")).collect();

        let mut rtc: Rtc = RtcConfig::new().build(Instant::now());
        for local in &locals {
            let candidate = Candidate::host(*local, "udp").map_err(|e| e.to_string())?;
            if rtc.add_local_candidate(candidate).is_some() {
                seen.host.push(local.to_string());
            }
        }
        let offer = SdpOffer::from_sdp_string(&offer_sdp).map_err(|e| format!("the offer: {e}"))?;
        let answer = rtc.sdp_api().accept_offer(offer).map_err(|e| format!("answering: {e}"))?;
        self.send(json!({"t": "answer", "sdp": answer.to_sdp_string()}));

        // Every socket's datagrams into one queue.
        let (udp_tx, mut udp_rx) = mpsc::channel::<(usize, Vec<u8>, SocketAddr, Instant)>(1024);
        let readers: Vec<_> = sockets
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

        // Server-reflexive candidates: ask both STUN servers from every socket.
        let mut gather = Gather::new(&locals).await;
        gather.send(&sockets).await;

        let deadline = tokio::time::Instant::now() + SESSION_LIMIT;
        let mut channel: Option<ChannelId> = None;
        let mut saved = false;
        let outcome: Result<(), String> = 'session: loop {
            // Everything str0m wants done now, then how long it can wait.
            let timeout = loop {
                let output = match rtc.poll_output() {
                    Ok(o) => o,
                    Err(e) => break 'session Err(format!("rtc: {e}")),
                };
                match output {
                    Output::Timeout(t) => break t,
                    Output::Transmit(t) => {
                        if let Some(i) = locals.iter().position(|l| *l == t.source) {
                            let _ = sockets[i].send_to(&t.contents, t.destination).await;
                            if channel.is_some() {
                                seen.path = Some((t.source, t.destination));
                            }
                        }
                    }
                    Output::Event(event) => match event {
                        Event::IceConnectionStateChange(state) => {
                            log!("page {}: ice {state:?} at {} ms", self.sid, ms());
                            seen.ice.push((format!("{state:?}"), ms()));
                        }
                        Event::Connected => {
                            seen.connected_ms = Some(ms());
                            log!("page {}: connected at {} ms", self.sid, ms());
                        }
                        Event::ChannelOpen(id, label) => {
                            seen.channel_ms = Some(ms());
                            channel = Some(id);
                            log!("page {}: channel {label:?} open at {} ms", self.sid, ms());
                        }
                        Event::ChannelData(data) => {
                            let Ok(message) = serde_json::from_slice::<Value>(&data.data) else {
                                continue;
                            };
                            match message.get("t").and_then(Value::as_str) {
                                Some("ping") => {
                                    seen.pings += 1;
                                    let pong = json!({"t": "pong", "seq": message["seq"]}).to_string();
                                    if let Some(mut ch) = rtc.channel(data.id) {
                                        let _ = ch.write(false, pong.as_bytes());
                                    }
                                }
                                Some("result") => {
                                    let page = message.get("r").cloned().unwrap_or(Value::Null);
                                    self.save("dc", page, &seen);
                                    saved = true;
                                    let ack = json!({"t": "saved"}).to_string();
                                    if let Some(mut ch) = rtc.channel(data.id) {
                                        let _ = ch.write(false, ack.as_bytes());
                                    }
                                }
                                Some("bye") => break 'session Ok(()),
                                _ => {}
                            }
                        }
                        Event::ChannelClose(_) => break 'session Ok(()),
                        _ => {}
                    },
                }
            };
            let now = Instant::now();
            let wait = timeout.saturating_duration_since(now);
            tokio::select! {
                Some((i, data, from, at)) = udp_rx.recv() => {
                    if let Some((server, mapped)) = gather.answer(i, &data) {
                        seen.srflx.push((server, locals[i], mapped));
                        if mapped != locals[i] {
                            if let Ok(c) = Candidate::server_reflexive(mapped, locals[i], "udp") {
                                if let Some(added) = rtc.add_local_candidate(c) {
                                    let line = added.to_sdp_string();
                                    self.send(json!({"t": "cand", "c": line}));
                                }
                            }
                        }
                        continue;
                    }
                    // Anything not STUN, DTLS or RTP is someone else's datagram.
                    if let Ok(receive) = Receive::new(Protocol::Udp, from, locals[i], &data) {
                        if let Err(e) = rtc.handle_input(Input::Receive(at, receive)) {
                            break Err(format!("rtc input: {e}"));
                        }
                    }
                }
                message = inbox.recv() => match message {
                    Some(ToSession::Candidate(line)) => {
                        let kind = match Candidate::from_sdp_string(&line) {
                            Ok(c) => {
                                let kind = match c.kind() {
                                    CandidateKind::Host => "host",
                                    CandidateKind::ServerReflexive => "srflx",
                                    CandidateKind::PeerReflexive => "prflx",
                                    CandidateKind::Relayed => "relay",
                                };
                                rtc.add_remote_candidate(c);
                                kind
                            }
                            Err(_) if line.contains(".local ") => "mdns",
                            Err(_) => "unparsed",
                        };
                        *seen.remote.entry(kind).or_default() += 1;
                    }
                    Some(ToSession::Result(page)) => {
                        self.save("hub", page, &seen);
                        saved = true;
                        if channel.is_none() {
                            break Ok(());
                        }
                    }
                    Some(ToSession::Bye) | None => break Ok(()),
                },
                () = tokio::time::sleep(wait) => {
                    if let Err(e) = rtc.handle_input(Input::Timeout(Instant::now())) {
                        break Err(format!("rtc timeout: {e}"));
                    }
                }
                () = gather.next_retry() => gather.send(&sockets).await,
                () = tokio::time::sleep_until(deadline) => break Err("the session ran out of time".into()),
            }
        };
        for r in readers {
            r.abort();
        }
        if !saved {
            log!("page {}: ended without a result", self.sid);
        }
        log!("page {}: closed after {} ms", self.sid, ms());
        outcome
    }

    fn save(&self, via: &str, page: Value, seen: &Seen) {
        let mut probe = seen.to_json();
        probe["label"] = Value::String(self.label.clone());
        let summary = page.get("summary").and_then(Value::as_str).unwrap_or("-").to_string();
        self.results.append(&record(&self.sid, via, page, probe));
        log!("page {}: result saved (via {via}): {summary}", self.sid);
    }
}

/// One STUN question: from socket `socket` to `server`.
struct Ask {
    socket: usize,
    name: String,
    server: SocketAddr,
    tid: [u8; 12],
    answered: bool,
}

/// Server-reflexive discovery: STUN binding requests from each socket to
/// each server of its family, retried a few times.
struct Gather {
    asks: Vec<Ask>,
    sent: u32,
    next: tokio::time::Instant,
}

impl Gather {
    async fn new(locals: &[SocketAddr]) -> Self {
        let mut asks = Vec::new();
        for name in STUN_SERVERS {
            let Ok(addrs) = tokio::net::lookup_host(name).await else {
                log!("cannot resolve {name}");
                continue;
            };
            let addrs: Vec<SocketAddr> = addrs.collect();
            for (socket, local) in locals.iter().enumerate() {
                if let Some(server) = addrs.iter().find(|a| a.is_ipv4() == local.is_ipv4()) {
                    let mut tid = [0u8; 12];
                    rand::rngs::OsRng.fill_bytes(&mut tid);
                    asks.push(Ask {
                        socket,
                        name: name.to_string(),
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

    async fn send(&mut self, sockets: &[Arc<UdpSocket>]) {
        for ask in self.asks.iter().filter(|a| !a.answered) {
            let _ = sockets[ask.socket].send_to(&stun::binding_request(&ask.tid), ask.server).await;
        }
        self.sent += 1;
        // At 0, 250 ms, 750 ms and 1.75 s; then it gives up.
        self.next = tokio::time::Instant::now() + Duration::from_millis(250 << (self.sent - 1).min(2));
    }

    async fn next_retry(&self) {
        if self.sent >= 4 || self.asks.iter().all(|a| a.answered) {
            std::future::pending::<()>().await;
        }
        tokio::time::sleep_until(self.next).await;
    }

    /// A STUN answer to one of ours on socket `socket`: the server's name and
    /// the mapped address.
    fn answer(&mut self, socket: usize, data: &[u8]) -> Option<(String, SocketAddr)> {
        let (tid, mapped) = stun::parse_binding_success(data)?;
        let ask = self.asks.iter_mut().find(|a| a.socket == socket && a.tid == tid && !a.answered)?;
        ask.answered = true;
        Some((ask.name.clone(), mapped))
    }
}

/// Just enough of STUN (RFC 8489) to learn a mapped address.
mod stun {
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

    /// An address attribute's value; XOR-ed with the cookie (and the
    /// transaction id, for IPv6) when `xor` has the transaction id.
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

        /// RFC 5769 §2.2 (its SOFTWARE attribute, then XOR-MAPPED-ADDRESS).
        #[test]
        fn the_rfc_5769_ipv4_response() {
            let m = response(&[
                0x80, 0x22, 0x00, 0x0b, 0x74, 0x65, 0x73, 0x74, 0x20, 0x76, 0x65, 0x63, 0x74, 0x6f, 0x72, 0x20, //
                0x00, 0x20, 0x00, 0x08, 0x00, 0x01, 0xa1, 0x47, 0xe1, 0x12, 0xa6, 0x43,
            ]);
            assert_eq!(parse_binding_success(&m), Some((TID, "192.0.2.1:32853".parse().unwrap())));
        }

        /// RFC 5769 §2.3's XOR-MAPPED-ADDRESS.
        #[test]
        fn the_rfc_5769_ipv6_response() {
            let m = response(&[
                0x00, 0x20, 0x00, 0x14, 0x00, 0x02, 0xa1, 0x47, 0x01, 0x13, 0xa9, 0xfa, 0xa5, 0xd3, 0xf1, 0x79, 0xbc, 0x25, 0xf4, 0xb5, 0xbe, 0xd2,
                0xb9, 0xd9,
            ]);
            assert_eq!(
                parse_binding_success(&m),
                Some((TID, "[2001:db8:1234:5678:11:2233:4455:6677]:32853".parse().unwrap()))
            );
        }

        #[test]
        fn plain_mapped_address_and_garbage() {
            let m = response(&[0x00, 0x01, 0x00, 0x08, 0x00, 0x01, 0x12, 0x34, 10, 0, 0, 1]);
            assert_eq!(parse_binding_success(&m), Some((TID, "10.0.0.1:4660".parse().unwrap())));
            assert_eq!(parse_binding_success(&binding_request(&TID)), None, "a request is not an answer");
            assert_eq!(parse_binding_success(&m[..30]), None, "truncated");
            assert_eq!(parse_binding_success(b"\x16\xfe\xfd not stun at all, a DTLS record"), None);
        }
    }
}

/// Now, as `2026-10-16T01:02:03.456Z`.
fn utc_now() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs();
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Howard Hinnant's civil_from_days.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        now.subsec_millis()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nat_hints_compare_the_two_servers() {
        let base: SocketAddr = "192.168.1.2:5000".parse().unwrap();
        let a: SocketAddr = "203.0.113.9:6000".parse().unwrap();
        let b: SocketAddr = "203.0.113.9:6001".parse().unwrap();
        let same = vec![("cf".to_string(), base, a), ("g".to_string(), base, a)];
        assert!(nat_hint(&same)["ipv4"].as_str().unwrap().starts_with("same mapping"));
        let differs = vec![("cf".to_string(), base, a), ("g".to_string(), base, b)];
        assert!(nat_hint(&differs)["ipv4"].as_str().unwrap().starts_with("mapping differs"));
        let public: SocketAddr = "[2001:db8::1]:7000".parse().unwrap();
        let open = vec![("cf".to_string(), public, public), ("g".to_string(), public, public)];
        assert!(nat_hint(&open)["ipv6"].as_str().unwrap().starts_with("no NAT"));
        assert_eq!(nat_hint(&[("cf".to_string(), base, a)])["ipv4"], "only one STUN server answered");
    }

    #[test]
    fn utc_dates() {
        assert_eq!(utc_now().len(), "2026-10-16T01:02:03.456Z".len());
        assert!(utc_now().starts_with("20"));
    }
}
