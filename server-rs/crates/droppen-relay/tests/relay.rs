//! M3b-3 (docs/relay-and-plans.md §2.3–§2.4, §2.10–§2.11): a relay on its own
//! — no database, no Redis, a fake Hub where one is needed — checked from the
//! outside: tickets, sign-ins, revocation, the outage grace, metering and the
//! limits.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message as AxMessage, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::routing::get;
use ed25519_dalek::SigningKey;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use rh_proto::frame::Frame;
use rh_proto::sig::{HubLogin, LoginRole};
use rh_proto::ticket::{RelayTicket, TicketLimits, TicketRole, TICKET_TTL_MS};
use rh_relay::config::{hex, Config, Timing, TrustProxy};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const HUB_KEY: [u8; 32] = [42; 32];
const SECS: Duration = Duration::from_secs(5);

fn now_ms() -> u64 {
    rh_relay::server::now_ms()
}

fn hub_key() -> SigningKey {
    SigningKey::from_bytes(&HUB_KEY)
}

fn hub_public() -> [u8; 32] {
    hub_key().verifying_key().to_bytes()
}

fn limits() -> TicketLimits {
    TicketLimits {
        bytes_per_sec: 4 * 1024 * 1024,
        frames_per_sec: 1000,
        channels: 64,
        account_conns: 48,
    }
}

/// A holder: its key and a ticket for it.
struct Holder {
    key: SigningKey,
    ticket: RelayTicket,
}

impl Holder {
    fn new(relay: &str, account: &str, role: TicketRole, id: &str, seed: u8, peers: &[&str]) -> Holder {
        let key = SigningKey::from_bytes(&[seed; 32]);
        let now = now_ms();
        Holder {
            ticket: RelayTicket {
                kid: 1,
                relay: relay.into(),
                account: account.into(),
                role,
                id: id.into(),
                sign_key: key.verifying_key().to_bytes(),
                peers: peers.iter().map(|s| s.to_string()).collect(),
                limits: limits(),
                flags: 0,
                issued_ms: now,
                expires_ms: now + TICKET_TTL_MS,
            },
            key,
        }
    }

    fn text(&self) -> String {
        self.ticket.sign(&hub_key()).unwrap()
    }
}

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// A device or agent on a relay.
struct Client {
    sink: SplitSink<Ws, Message>,
    stream: SplitStream<Ws>,
    nonce: [u8; 32],
    closed: Option<u16>,
    controls: Vec<Value>,
}

impl Client {
    async fn dial(origin: &str) -> Client {
        let (ws, _) = tokio_tungstenite::connect_async(format!("{origin}/v1/connect")).await.unwrap();
        let (sink, mut stream) = ws.split();
        let challenge = match stream.next().await.unwrap().unwrap() {
            Message::Text(t) => serde_json::from_str::<Value>(t.as_str()).unwrap(),
            other => panic!("{other:?}"),
        };
        assert_eq!(challenge["t"], "challenge");
        let nonce = rh_relay::config::unhex(challenge["nonce"].as_str().unwrap()).unwrap().try_into().unwrap();
        Client {
            sink,
            stream,
            nonce,
            closed: None,
            controls: Vec::new(),
        }
    }

    fn sig(&self, h: &Holder, role: LoginRole, origin: &str, time_ms: u64) -> String {
        hex(&HubLogin {
            role,
            account: h.ticket.account.clone(),
            id: h.ticket.id.clone(),
            hub: origin.into(),
            nonce: self.nonce,
            time_ms,
        }
        .sign(&h.key)
        .unwrap())
    }

    async fn send_json(&mut self, v: Value) {
        self.sink.send(Message::Text(v.to_string().into())).await.unwrap();
    }

    async fn send(&mut self, f: Frame) {
        self.sink.send(Message::Binary(f.encode().unwrap().into())).await.unwrap();
    }

    /// Sign in with `h`'s ticket text `ticket`; the first answer.
    async fn login_with(&mut self, h: &Holder, ticket: &str, origin: &str) -> Value {
        let t = now_ms();
        let sig = self.sig(h, h.ticket.role.login_role(), origin, t);
        self.send_json(json!({"t": "login", "ticket": ticket, "time_ms": t, "sig": sig})).await;
        self.next_text(SECS).await
    }

    async fn login(origin: &str, h: &Holder) -> Client {
        let mut c = Client::dial(origin).await;
        let welcome = c.login_with(h, &h.text(), origin).await;
        assert_eq!(welcome["t"], "welcome", "{welcome}");
        c
    }

    /// The next text message (`Null` once closed, its code kept).
    async fn next_text(&mut self, within: Duration) -> Value {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(left, self.stream.next()).await {
                Ok(Some(Ok(Message::Text(t)))) => return serde_json::from_str(t.as_str()).unwrap(),
                Ok(Some(Ok(Message::Close(f)))) => {
                    self.closed = f.map(|f| u16::from(f.code));
                    return Value::Null;
                }
                Ok(Some(Ok(_))) => continue,
                Ok(Some(Err(_)) | None) => return Value::Null,
                Err(_) => return json!({"t": "timeout"}),
            }
        }
    }

    /// The next frame (control messages are kept aside; `None` once closed or after `within`).
    async fn next_frame(&mut self, within: Duration) -> Option<Frame> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(left, self.stream.next()).await {
                Ok(Some(Ok(Message::Binary(b)))) => return Frame::decode(&b).ok(),
                Ok(Some(Ok(Message::Text(t)))) => self.controls.push(serde_json::from_str(t.as_str()).unwrap()),
                Ok(Some(Ok(Message::Close(f)))) => {
                    self.closed = f.map(|f| u16::from(f.code));
                    return None;
                }
                Ok(Some(Ok(_))) => continue,
                _ => return None,
            }
        }
    }

    /// Wait for the connection to close; its code.
    async fn closed_within(&mut self, within: Duration) -> Option<u16> {
        let deadline = tokio::time::Instant::now() + within;
        while tokio::time::Instant::now() < deadline {
            if self.closed.is_some() {
                break;
            }
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(left, self.stream.next()).await {
                Ok(Some(Ok(Message::Close(f)))) => self.closed = f.map(|f| u16::from(f.code)),
                Ok(Some(Ok(Message::Text(t)))) => self.controls.push(serde_json::from_str(t.as_str()).unwrap()),
                Ok(Some(Ok(_))) => {}
                Ok(Some(Err(_)) | None) => return self.closed.or(Some(0)),
                Err(_) => break,
            }
        }
        self.closed
    }
}

/// A relay in this process.
struct TestRelay {
    origin: String,
    server: Arc<rh_relay::Server>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    done: tokio::task::JoinHandle<Result<(), String>>,
}

async fn relay(id: &str, hub: Option<&str>, timing: Timing, tweak: impl FnOnce(&mut Config)) -> TestRelay {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("ws://{}", listener.local_addr().unwrap());
    let mut config = Config {
        bind: listener.local_addr().unwrap(),
        origin: origin.clone(),
        hub: hub.map(str::to_string),
        id: id.into(),
        key: SigningKey::from_bytes(&[7; 32]),
        registration_token: None,
        pinned_keys: hub.is_none().then(|| vec![(1, hub_public())]),
        state_dir: None,
        trust_proxy: TrustProxy::None,
        max_conns: 1000,
        drain: Duration::from_millis(300),
        timing,
    };
    tweak(&mut config);
    let server = rh_relay::Server::new(config);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let done = tokio::spawn(rh_relay::run(server.clone(), listener, async move {
        let _ = stopped.await;
    }));
    TestRelay {
        origin,
        server,
        stop: Some(stop),
        done,
    }
}

/// A device on `ch` reaches the machine; the agent's side of the pair.
async fn pair(dev: &mut Client, agent: &mut Client, ch: u32, machine: &str) -> u32 {
    dev.send(Frame::Open { ch, peer: machine.into() }).await;
    match agent.next_frame(SECS).await {
        Some(Frame::Open { ch: agent_ch, .. }) => agent_ch,
        other => panic!("the agent got {other:?}"),
    }
}

/// Send `n` bytes of `DATA` each way through a pair.
async fn traffic(dev: &mut Client, agent: &mut Client, dev_ch: u32, agent_ch: u32, n: usize) {
    let mut sent = 0;
    while sent < n {
        let len = (n - sent).min(16 * 1024);
        dev.send(Frame::Data {
            ch: dev_ch,
            payload: vec![1; len],
        })
        .await;
        loop {
            match agent.next_frame(SECS).await {
                Some(Frame::Data { payload, .. }) => {
                    assert_eq!(payload.len(), len);
                    break;
                }
                Some(Frame::Credit { .. }) => continue,
                other => panic!("{other:?}"),
            }
        }
        agent
            .send(Frame::Credit {
                ch: agent_ch,
                bytes: len as u32,
            })
            .await;
        agent
            .send(Frame::Data {
                ch: agent_ch,
                payload: vec![2; len],
            })
            .await;
        loop {
            match dev.next_frame(SECS).await {
                Some(Frame::Data { payload, .. }) => {
                    assert_eq!(payload.len(), len);
                    break;
                }
                Some(Frame::Credit { .. }) => continue,
                other => panic!("{other:?}"),
            }
        }
        dev.send(Frame::Credit {
            ch: dev_ch,
            bytes: len as u32,
        })
        .await;
        sent += len;
    }
}

// ---------------------------------------------------------------- a fake Hub

#[derive(Default)]
struct HubSeen {
    /// `(boot, seq)` → `{account: bytes}`.
    reports: BTreeMap<(String, u64), BTreeMap<String, u64>>,
    duplicates: usize,
    logins: usize,
    /// What the relay named in its last `login` (`features`).
    features: Value,
    links: Vec<tokio::sync::mpsc::UnboundedSender<String>>,
    /// Do not say `replayed` after the welcome (a Hub from before it).
    no_replay: bool,
}

struct FakeHub {
    origin: String,
    seen: Arc<Mutex<HubSeen>>,
    stop: tokio_util::sync::CancellationToken,
}

async fn hub_link(
    State(seen): State<Arc<Mutex<HubSeen>>>,
    ws: WebSocketUpgrade,
    origin: String,
    stop: tokio_util::sync::CancellationToken,
) -> axum::response::Response {
    ws.on_upgrade(move |socket| hub_conn(socket, seen, origin, stop))
}

async fn hub_conn(socket: WebSocket, seen: Arc<Mutex<HubSeen>>, origin: String, stop: tokio_util::sync::CancellationToken) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    seen.lock().unwrap().links.push(tx.clone());
    let _ = sink
        .send(AxMessage::Text(
            json!({"t": "challenge", "nonce": hex(&[5; 32]), "hub": origin, "version": 1})
                .to_string()
                .into(),
        ))
        .await;
    loop {
        tokio::select! {
            m = stream.next() => {
                let Some(Ok(AxMessage::Text(t))) = m else { break };
                let v: Value = serde_json::from_str(t.as_str()).unwrap();
                match v["t"].as_str() {
                    Some("login") => {
                        let no_replay = {
                            let mut s = seen.lock().unwrap();
                            s.logins += 1;
                            s.features = v["features"].clone();
                            s.no_replay
                        };
                        let _ = tx.send(json!({"t": "welcome", "id": v["id"], "ticket_keys": [{"kid": 1, "key": hex(&hub_public())}]}).to_string());
                        if !no_replay {
                            let _ = tx.send(json!({"t": "replayed", "revocations": 0}).to_string());
                        }
                    }
                    Some("usage") => {
                        let key = (v["boot"].as_str().unwrap().to_string(), v["seq"].as_u64().unwrap());
                        let accounts: BTreeMap<String, u64> = v["accounts"].as_array().unwrap().iter().map(|a| (a["account"].as_str().unwrap().to_string(), a["bytes"].as_u64().unwrap())).collect();
                        let mut s = seen.lock().unwrap();
                        if s.reports.contains_key(&key) { s.duplicates += 1; } else { s.reports.insert(key.clone(), accounts); }
                        let _ = tx.send(json!({"t": "usage_ok", "boot": key.0, "seq": key.1}).to_string());
                    }
                    _ => {}
                }
            }
            Some(out) = rx.recv() => {
                if sink.send(AxMessage::Text(out.into())).await.is_err() { break; }
            }
            () = stop.cancelled() => break,
        }
    }
    let _ = sink.close().await;
}

impl FakeHub {
    async fn start_on(addr: Option<SocketAddr>, seen: Arc<Mutex<HubSeen>>) -> FakeHub {
        let listener = match addr {
            Some(a) => {
                let mut tries = 0;
                loop {
                    match tokio::net::TcpListener::bind(a).await {
                        Ok(l) => break l,
                        Err(_) if tries < 50 => {
                            tries += 1;
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                        Err(e) => panic!("{e}"),
                    }
                }
            }
            None => tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(),
        };
        let origin = format!("ws://{}", listener.local_addr().unwrap());
        let stop = tokio_util::sync::CancellationToken::new();
        let (o, s) = (origin.clone(), stop.clone());
        let app = axum::Router::new()
            .route(
                "/v1/relay",
                get(move |state: State<Arc<Mutex<HubSeen>>>, ws: WebSocketUpgrade| hub_link(state, ws, o.clone(), s.clone())),
            )
            .with_state(seen.clone());
        let stopped = stop.clone();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move { stopped.cancelled().await })
                .await
        });
        FakeHub { origin, seen, stop }
    }

    fn tell(&self, v: Value) {
        for l in &self.seen.lock().unwrap().links {
            let _ = l.send(v.to_string());
        }
    }

    fn addr(&self) -> SocketAddr {
        self.origin.trim_start_matches("ws://").parse().unwrap()
    }

    fn metered(&self, account: &str) -> u64 {
        self.seen.lock().unwrap().reports.values().filter_map(|a| a.get(account)).sum()
    }
}

async fn eventually(within: Duration, mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    f()
}

// ---------------------------------------------------------------- the tests

/// The binary, with nothing in its environment but its own variables: no
/// database, no Redis. Tickets verify; the canary crosses and is never logged.
#[tokio::test]
async fn the_binary_verifies_tickets_with_no_database_or_redis_and_never_logs_the_canary() {
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let origin = format!("ws://127.0.0.1:{port}");
    let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_droppen-relay"))
        .env_clear()
        .env("PORT", port.to_string())
        .env("DROPPEN_RELAY_BIND", "127.0.0.1")
        .env("DROPPEN_RELAY_ORIGIN", &origin)
        .env("DROPPEN_RELAY_ID", "sin-1")
        .env("DROPPEN_RELAY_KEY", hex(&[8; 32]))
        .env("DROPPEN_RELAY_TICKET_KEYS", format!("1:{}", hex(&hub_public())))
        .env("DROPPEN_RELAY_DRAIN_SECS", "0")
        .env("RUST_LOG", "trace")
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let up = eventually(Duration::from_secs(10), || std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()).await;
    assert!(up, "the relay listens");
    let agent_h = Holder::new("sin-1", "a-1", TicketRole::Agent, "m-1", 11, &[]);
    let dev_h = Holder::new("sin-1", "a-1", TicketRole::Device, "d-1", 12, &["m-1"]);
    let mut agent = Client::login(&origin, &agent_h).await;
    let mut dev = Client::login(&origin, &dev_h).await;
    let presence = dev.next_text(SECS).await;
    assert_eq!(presence["machines"][0]["state"], "online", "{presence}");
    let agent_ch = pair(&mut dev, &mut agent, 1, "m-1").await;
    let canary = format!("RH-CANARY-{}", hex(&rand::random::<[u8; 10]>()));
    dev.send(Frame::Data {
        ch: 1,
        payload: canary.clone().into_bytes(),
    })
    .await;
    match agent.next_frame(SECS).await {
        Some(Frame::Data { ch, payload }) => {
            assert_eq!(ch, agent_ch);
            assert_eq!(payload, canary.as_bytes());
        }
        other => panic!("{other:?}"),
    }
    agent
        .send(Frame::Data {
            ch: agent_ch,
            payload: canary.clone().into_bytes(),
        })
        .await;
    assert!(matches!(dev.next_frame(SECS).await, Some(Frame::Data { ch: 1, .. })));
    drop(dev);
    drop(agent);
    tokio::time::sleep(Duration::from_millis(200)).await;
    // SIGTERM: the relay drains and stops.
    let pid = child.id().unwrap();
    let _ = std::process::Command::new("kill").arg("-TERM").arg(pid.to_string()).status();
    let out = tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    let logs = String::from_utf8_lossy(&out.stderr);
    assert!(logs.contains("signed in"), "the log capture works: {logs}");
    assert!(logs.contains("frame in"), "trace level: frames are logged");
    for spelling in [canary.clone(), hex(canary.as_bytes())] {
        let hits: Vec<&str> = logs.lines().filter(|l| l.contains(&spelling)).collect();
        assert!(hits.is_empty(), "the canary is in the relay's log: {hits:?}");
    }
    assert!(!logs.contains(&dev_h.text()[..40]), "a ticket is in the relay's log");
}

#[tokio::test]
async fn bad_tickets_and_bad_sign_ins_are_refused() {
    let r = relay("sin-1", None, Timing::default(), |_| {}).await;
    let o = r.origin.as_str();
    let good = Holder::new("sin-1", "a-1", TicketRole::Device, "d-1", 21, &["m-1"]);
    let refused = |v: &Value| v["t"] == "error";

    // Expired.
    let mut old = Holder::new("sin-1", "a-1", TicketRole::Device, "d-1", 21, &[]);
    old.ticket.issued_ms -= 2 * TICKET_TTL_MS;
    old.ticket.expires_ms = now_ms() - 1;
    let mut c = Client::dial(o).await;
    assert!(refused(&c.login_with(&old, &old.text(), o).await));
    assert_eq!(c.closed_within(SECS).await, Some(4419));
    // Another relay's.
    let other = Holder::new("nrt-1", "a-1", TicketRole::Device, "d-1", 21, &[]);
    let mut c = Client::dial(o).await;
    assert!(refused(&c.login_with(&other, &other.text(), o).await));
    assert_eq!(c.closed_within(SECS).await, Some(4401));
    // An unknown kid.
    let mut kid = Holder::new("sin-1", "a-1", TicketRole::Device, "d-1", 21, &[]);
    kid.ticket.kid = 9;
    let mut c = Client::dial(o).await;
    assert!(refused(&c.login_with(&kid, &kid.text(), o).await));
    // Signed by another key than the Hub's.
    let forged = good.ticket.sign(&SigningKey::from_bytes(&[66; 32])).unwrap();
    let mut c = Client::dial(o).await;
    assert!(refused(&c.login_with(&good, &forged, o).await));
    // The holder's signature under another key.
    let thief = Holder {
        key: SigningKey::from_bytes(&[67; 32]),
        ticket: good.ticket.clone(),
    };
    let mut c = Client::dial(o).await;
    assert!(refused(&c.login_with(&thief, &good.text(), o).await));
    assert_eq!(c.closed_within(SECS).await, Some(4401));
    // A Hub sign-in replayed here: role Device and the Hub's origin.
    for (role, origin) in [
        (LoginRole::Device, "wss://hub.example"),
        (LoginRole::Device, o),
        (LoginRole::RelayDevice, "wss://hub.example"),
        (LoginRole::RelayAgent, o),
    ] {
        let mut c = Client::dial(o).await;
        let t = now_ms();
        let sig = c.sig(&good, role, origin, t);
        c.send_json(json!({"t": "login", "ticket": good.text(), "time_ms": t, "sig": sig})).await;
        assert!(refused(&c.next_text(SECS).await), "{role:?} for {origin}");
    }
    // Another connection's nonce.
    let mut a = Client::dial(o).await;
    let mut b = Client::dial(o).await;
    let t = now_ms();
    let sig = a.sig(&good, LoginRole::RelayDevice, o, t);
    b.send_json(json!({"t": "login", "ticket": good.text(), "time_ms": t, "sig": sig})).await;
    assert!(refused(&b.next_text(SECS).await));
    // Too far off the relay's clock.
    let t = now_ms() - 120_000;
    let sig = a.sig(&good, LoginRole::RelayDevice, o, t);
    a.send_json(json!({"t": "login", "ticket": good.text(), "time_ms": t, "sig": sig})).await;
    assert!(refused(&a.next_text(SECS).await));
    // And the real thing works.
    let _ok = Client::login(o, &good).await;
    // Frames before signing in, or a hub frame after: closed.
    let mut c = Client::dial(o).await;
    c.send(Frame::Ping { nonce: 1 }).await;
    assert_eq!(c.closed_within(SECS).await, Some(4401));
    let mut c = Client::login(o, &good).await;
    c.send(Frame::Chain {
        ch: 0,
        entries: vec![vec![1]],
    })
    .await;
    assert_eq!(c.closed_within(SECS).await, Some(4400), "not on a relay");
}

#[tokio::test]
async fn a_revoke_closes_its_connections_within_a_second_and_a_throttle_slows_them() {
    let seen = Arc::new(Mutex::new(HubSeen::default()));
    let hub = FakeHub::start_on(None, seen.clone()).await;
    let r = relay("sin-1", Some(&hub.origin), Timing::default(), |_| {}).await;
    assert!(eventually(SECS, || r.server.has_keys()).await, "keys from the Hub");
    let o = r.origin.as_str();
    let dev_h = Holder::new("sin-1", "a-1", TicketRole::Device, "d-1", 31, &["m-1"]);
    let other_h = Holder::new("sin-1", "a-1", TicketRole::Device, "d-2", 32, &["m-1"]);
    let agent_h = Holder::new("sin-1", "a-1", TicketRole::Agent, "m-1", 33, &[]);
    let mut agent = Client::login(o, &agent_h).await;
    let mut dev = Client::login(o, &dev_h).await;
    let mut other = Client::login(o, &other_h).await;

    // Throttled to 64 KiB/s (bursts of 128 KiB): 384 KiB take seconds.
    hub.tell(json!({"t": "throttle", "account": "a-1", "bytes_per_sec": 65536}));
    tokio::time::sleep(Duration::from_millis(200)).await;
    let ch = pair(&mut other, &mut agent, 1, "m-1").await;
    let t0 = Instant::now();
    traffic(&mut other, &mut agent, 1, ch, 384 * 1024).await;
    assert!(t0.elapsed() > Duration::from_millis(1500), "throttled: {:?}", t0.elapsed());
    hub.tell(json!({"t": "throttle", "account": "a-1", "bytes_per_sec": null}));

    let t0 = Instant::now();
    hub.tell(json!({"t": "revoke", "account": "a-1", "id": "d-1", "at_ms": now_ms()}));
    assert_eq!(dev.closed_within(Duration::from_secs(1)).await, Some(4403));
    assert!(t0.elapsed() < Duration::from_secs(1));
    assert_eq!(other.closed, None, "only d-1");
    // Its old ticket is no good any more; a newer one is.
    let mut again = Client::dial(o).await;
    assert!(again.login_with(&dev_h, &dev_h.text(), o).await["t"] == "error");
    tokio::time::sleep(Duration::from_millis(5)).await;
    let fresh = Holder::new("sin-1", "a-1", TicketRole::Device, "d-1", 31, &["m-1"]);
    let _ok = Client::login(o, &fresh).await;
    hub.stop.cancel();
}

/// A revocation scoped to a ticket role (`revoke-role/1`, which the relay
/// names when it signs in): a host's device-role tickets go and its agent's
/// stay — the agent keeps its connection and still takes channels, the way a
/// host whose identity key is being renewed stays reachable to be asked for
/// it. An unscoped revocation still takes both; a role the relay does not
/// know takes nothing.
#[tokio::test]
async fn a_revocation_scoped_to_the_device_role_leaves_the_hosts_agent_alone() {
    let seen = Arc::new(Mutex::new(HubSeen::default()));
    let hub = FakeHub::start_on(None, seen.clone()).await;
    let r = relay("sin-1", Some(&hub.origin), Timing::default(), |_| {}).await;
    assert!(eventually(SECS, || r.server.has_keys()).await, "keys from the Hub");
    assert_eq!(seen.lock().unwrap().features, json!(["revoke-role/1"]), "the relay names what it takes");
    let o = r.origin.as_str();
    let host_agent = Holder::new("sin-1", "a-1", TicketRole::Agent, "h-1", 41, &[]);
    let host_device = Holder::new("sin-1", "a-1", TicketRole::Device, "h-1", 41, &["h-2"]);
    let admin_h = Holder::new("sin-1", "a-1", TicketRole::Device, "d-1", 43, &["h-1"]);
    let mut agent = Client::login(o, &host_agent).await;
    let mut as_device = Client::login(o, &host_device).await;
    let mut admin = Client::login(o, &admin_h).await;
    hub.tell(json!({"t": "revoke", "account": "a-1", "id": "h-1", "role": "device", "at_ms": now_ms()}));
    assert_eq!(as_device.closed_within(Duration::from_secs(1)).await, Some(4403), "its device role goes");
    assert_eq!(agent.closed_within(Duration::from_millis(800)).await, None, "its agent stays");
    // Its device-role ticket is refused from now on…
    let mut again = Client::dial(o).await;
    assert_eq!(again.login_with(&host_device, &host_device.text(), o).await["t"], "error");
    // …and its agent still takes a channel: an admin reaches it.
    let ch = pair(&mut admin, &mut agent, 1, "h-1").await;
    traffic(&mut admin, &mut agent, 1, ch, 1000).await;
    // A role this relay does not know revokes nothing.
    hub.tell(json!({"t": "revoke", "account": "a-1", "id": "d-1", "role": "printer", "at_ms": now_ms()}));
    assert_eq!(admin.closed_within(Duration::from_millis(500)).await, None);
    // Unscoped, as before: every ticket of the id.
    hub.tell(json!({"t": "revoke", "account": "a-1", "id": "h-1", "at_ms": now_ms()}));
    assert_eq!(agent.closed_within(Duration::from_secs(1)).await, Some(4403));
    let mut again = Client::dial(o).await;
    assert_eq!(again.login_with(&host_agent, &host_agent.text(), o).await["t"], "error");
    hub.stop.cancel();
}

/// The Hub goes away: established connections outlive their tickets, the
/// meter keeps its windows, and when the Hub is back every report arrives —
/// counted once — and expired tickets close (4419).
#[tokio::test]
async fn an_outage_keeps_connections_alive_and_usage_arrives_once_after_it() {
    let seen = Arc::new(Mutex::new(HubSeen::default()));
    let hub = FakeHub::start_on(None, seen.clone()).await;
    let timing = Timing {
        renew_grace_ms: 200,
        uplink_backoff_max: Duration::from_millis(500),
        outage_after: Duration::from_millis(400),
        usage_every: Duration::from_millis(300),
        health_every: Duration::from_millis(300),
        ..Timing::default()
    };
    let r = relay("sin-1", Some(&hub.origin), timing, |_| {}).await;
    assert!(eventually(SECS, || r.server.has_keys()).await);
    let o = r.origin.as_str();
    let short = |h: &mut Holder| {
        h.ticket.expires_ms = now_ms() + 1_500;
    };
    let mut dev_h = Holder::new("sin-1", "a-1", TicketRole::Device, "d-1", 41, &["m-1"]);
    let mut agent_h = Holder::new("sin-1", "a-1", TicketRole::Agent, "m-1", 42, &[]);
    short(&mut dev_h);
    short(&mut agent_h);
    let mut agent = Client::login(o, &agent_h).await;
    let mut dev = Client::login(o, &dev_h).await;
    let ch = pair(&mut dev, &mut agent, 1, "m-1").await;
    traffic(&mut dev, &mut agent, 1, ch, 10_000).await;
    assert!(
        eventually(SECS, || hub.metered("a-1") == 20_000).await,
        "both directions, once: {}",
        hub.metered("a-1")
    );

    // The Hub goes; traffic goes on; the tickets run out during the outage.
    let addr = hub.addr();
    hub.stop.cancel();
    traffic(&mut dev, &mut agent, 1, ch, 30_000).await;
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    assert!(now_ms() > dev_h.ticket.expires_ms + 200, "the tickets ran out");
    traffic(&mut dev, &mut agent, 1, ch, 5_000).await;
    assert_eq!(dev.closed, None, "an outage keeps established connections");
    // A new sign-in still needs a valid ticket.
    let mut late = Client::dial(o).await;
    assert!(late.login_with(&dev_h, &dev_h.text(), o).await["t"] == "error");

    // The Hub is back: the reports of the outage arrive, nothing twice-counted.
    let back = FakeHub::start_on(Some(addr), seen.clone()).await;
    assert!(
        eventually(Duration::from_secs(10), || back.metered("a-1") == 90_000).await,
        "every byte, once: {}",
        back.metered("a-1")
    );
    let windows: Vec<(String, u64)> = seen.lock().unwrap().reports.keys().cloned().collect();
    assert!(windows.len() >= 3, "the windows kept apart: {windows:?}");
    // Out of the outage: the expired tickets close.
    assert_eq!(dev.closed_within(Duration::from_secs(5)).await, Some(4419));
    back.stop.cancel();
}

#[tokio::test]
async fn limits_hold_for_accounts_channels_frames_sign_ins_and_addresses() {
    let timing = Timing {
        login_timeout: Duration::from_millis(300),
        ..Timing::default()
    };
    let r = relay("sin-1", None, timing, |c| c.max_conns = 6).await;
    let o = r.origin.as_str();
    let small = |seed: u8, id: &str, role: TicketRole| {
        let mut h = Holder::new("sin-1", "a-1", role, id, seed, &["m-1"]);
        h.ticket.limits.account_conns = 3;
        h.ticket.limits.channels = 2;
        h
    };
    let agent_h = small(51, "m-1", TicketRole::Agent);
    let mut agent = Client::login(o, &agent_h).await;
    let d1 = small(52, "d-1", TicketRole::Device);
    let mut dev = Client::login(o, &d1).await;
    let _d2 = Client::login(o, &small(53, "d-2", TicketRole::Device)).await;
    // A fourth connection of the account: over its limit.
    let d4 = small(54, "d-4", TicketRole::Device);
    let mut c = Client::dial(o).await;
    assert_eq!(c.login_with(&d4, &d4.text(), o).await["t"], "error");
    assert_eq!(c.closed_within(SECS).await, Some(4429));
    // Two channels per device connection.
    pair(&mut dev, &mut agent, 1, "m-1").await;
    pair(&mut dev, &mut agent, 2, "m-1").await;
    dev.send(Frame::Open { ch: 3, peer: "m-1".into() }).await;
    assert!(matches!(dev.next_frame(SECS).await, Some(Frame::Close { ch: 3, code: 4409, .. })));
    // Only machines the ticket names.
    dev.send(Frame::Close {
        ch: 2,
        code: 1000,
        reason: String::new(),
    })
    .await;
    dev.send(Frame::Open { ch: 4, peer: "m-9".into() }).await;
    assert!(matches!(dev.next_frame(SECS).await, Some(Frame::Close { ch: 4, code: 4403, .. })));
    // Past the credit window: closed both ways.
    for _ in 0..9 {
        dev.send(Frame::Data {
            ch: 1,
            payload: vec![0; 60_000],
        })
        .await;
    }
    let mut flow = false;
    for _ in 0..20 {
        match dev.next_frame(SECS).await {
            Some(Frame::Close { ch: 1, code: 4408, .. }) => {
                flow = true;
                break;
            }
            Some(_) => continue,
            None => break,
        }
    }
    assert!(flow, "4408 after the window");
    // Sign in within the timeout.
    let mut slow = Client::dial(o).await;
    assert_eq!(slow.closed_within(SECS).await, Some(4401));
    // The relay's own limit (6): a full relay says so.
    let mut held = Vec::new();
    for (i, seed) in (60..64).enumerate() {
        let h = Holder::new("sin-1", &format!("a-{i}0"), TicketRole::Device, "d-x", seed, &[]);
        let mut c = Client::dial(o).await;
        let v = c.login_with(&h, &h.text(), o).await;
        held.push((c, v));
    }
    assert!(
        held.iter().any(|(c, v)| v["t"] == "error" && c.closed.is_none_or(|code| code == 1013)),
        "a full relay refuses"
    );
    // Addresses: 20 upgrades in a burst, then 429.
    let mut codes = Vec::new();
    for _ in 0..25 {
        let r = tokio_tungstenite::connect_async(format!("{o}/v1/connect")).await;
        codes.push(match r {
            Ok(_) => 101,
            Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => resp.status().as_u16(),
            Err(_) => 0,
        });
    }
    assert!(codes.contains(&429), "{codes:?}");
    // The ping a browser times, readable cross-origin.
    let http = r.origin.replace("ws://", "http://");
    let resp = reqwest_lite(&format!("{http}/v1/ping")).await;
    assert!(resp.starts_with("HTTP/1.1 204"), "{resp}");
    assert!(resp.to_ascii_lowercase().contains("access-control-allow-origin: *"));
    assert!(resp.to_ascii_lowercase().contains("timing-allow-origin: *"));
    // No page a relay answers may use the app's passkeys (docs/passkeys.md
    // §3.8): every response, an unknown path's 404 included.
    for path in ["/v1/ping", "/healthz", "/nope"] {
        let resp = reqwest_lite(&format!("{http}{path}")).await.to_ascii_lowercase();
        assert!(
            resp.contains("permissions-policy: publickey-credentials-get=(), publickey-credentials-create=()"),
            "{path}: {resp}"
        );
    }
    // The origin proof verifies under the relay's key.
    let nonce = [9u8; 32];
    let body = reqwest_lite(&format!("{http}/v1/relay-id?nonce={}", hex(&nonce))).await;
    let json: Value = serde_json::from_str(body.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    let key: [u8; 32] = rh_relay::config::unhex(json["sign_key"].as_str().unwrap()).unwrap().try_into().unwrap();
    let sig: [u8; 64] = rh_relay::config::unhex(json["sig"].as_str().unwrap()).unwrap().try_into().unwrap();
    assert!(rh_proto::ticket::verify_origin_proof(&key, o, &nonce, &sig));
    assert_eq!(json["relay_id"], "sin-1");
}

/// A bare HTTP/1.1 GET (the relay crate has no HTTP client).
async fn reqwest_lite(url: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let rest = url.trim_start_matches("http://");
    let (authority, path) = rest.split_once('/').map(|(a, p)| (a, format!("/{p}"))).unwrap();
    let mut s = TcpStream::connect(authority).await.unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.unwrap();
    String::from_utf8_lossy(&out).into_owned()
}

#[tokio::test]
async fn presence_follows_agents_and_a_second_sign_in_replaces_the_first() {
    let r = relay("sin-1", None, Timing::default(), |_| {}).await;
    let o = r.origin.as_str();
    let dev_h = Holder::new("sin-1", "a-1", TicketRole::Device, "d-1", 71, &["m-1", "m-2"]);
    let mut dev = Client::login(o, &dev_h).await;
    let snapshot = dev.next_text(SECS).await;
    assert_eq!(snapshot["t"], "presence");
    assert!(snapshot["machines"].as_array().unwrap().iter().all(|m| m["state"] == "offline"));
    let agent_h = Holder::new("sin-1", "a-1", TicketRole::Agent, "m-1", 72, &[]);
    let mut agent = Client::login(o, &agent_h).await;
    let online = dev.next_text(SECS).await;
    assert_eq!(
        (online["machines"][0]["id"].as_str(), online["machines"][0]["state"].as_str()),
        (Some("m-1"), Some("online"))
    );
    let ch = pair(&mut dev, &mut agent, 1, "m-1").await;
    let _ = ch;
    // Another account's machine of the same id is not this account's.
    let stranger = Holder::new("sin-1", "a-2", TicketRole::Agent, "m-2", 73, &[]);
    let _s = Client::login(o, &stranger).await;
    dev.send(Frame::Open { ch: 2, peer: "m-2".into() }).await;
    assert!(matches!(dev.next_frame(SECS).await, Some(Frame::Close { ch: 2, code: 4404, .. })));
    // The machine signs in again: the old connection closes (4001), its channel with it.
    let mut again = Client::login(o, &agent_h).await;
    assert_eq!(agent.closed_within(SECS).await, Some(4001));
    assert!(matches!(dev.next_frame(SECS).await, Some(Frame::Close { ch: 1, code: 4410, .. })));
    // Gone: offline.
    again.sink.close().await.unwrap();
    let mut offline = false;
    for _ in 0..5 {
        let v = dev.next_text(SECS).await;
        if v["t"] == "presence" && v["machines"][0]["state"] == "offline" {
            offline = true;
            break;
        }
    }
    assert!(offline);
}

#[tokio::test]
async fn a_renewed_ticket_keeps_a_connection_and_shutting_down_says_goaway_first() {
    let timing = Timing {
        renew_grace_ms: 100,
        ..Timing::default()
    };
    let mut r = relay("sin-1", None, timing, |_| {}).await;
    let o = r.origin.clone();
    let mut h = Holder::new("sin-1", "a-1", TicketRole::Device, "d-1", 81, &["m-1"]);
    h.ticket.expires_ms = now_ms() + 800;
    let mut dev = Client::login(&o, &h).await;
    let _ = dev.next_text(SECS).await;
    let mut renewed = Holder::new("sin-1", "a-1", TicketRole::Device, "d-1", 81, &["m-1"]);
    renewed.ticket.expires_ms = now_ms() + 60_000;
    dev.send_json(json!({"t": "ticket", "ticket": renewed.text()})).await;
    assert_eq!(dev.next_text(SECS).await["t"], "ticket_ok");
    // Someone else's ticket is no renewal.
    let other = Holder::new("sin-1", "a-1", TicketRole::Device, "d-2", 82, &[]);
    let mut probe = Client::login(&o, &Holder::new("sin-1", "a-3", TicketRole::Device, "d-3", 83, &[])).await;
    let _ = probe.next_text(SECS).await;
    probe.send_json(json!({"t": "ticket", "ticket": other.text()})).await;
    assert!(probe.next_text(SECS).await["t"] == "error");
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert_eq!(dev.closed, None, "the renewal holds");
    // Shutting down: goaway, then 1012 after the drain.
    r.stop.take().unwrap().send(()).unwrap();
    let goaway = dev.next_text(SECS).await;
    assert_eq!(goaway["t"], "goaway", "{goaway}");
    assert_eq!(dev.closed_within(SECS).await, Some(1012));
    assert!(r.done.await.unwrap().is_ok());
}

// ---------------------------------------------------------------- review regressions

/// One throttled account's connections share its rate: two devices sending
/// at once take twice as long as one would, not as long.
#[tokio::test]
async fn a_throttled_accounts_connections_share_one_rate() {
    let seen = Arc::new(Mutex::new(HubSeen::default()));
    let hub = FakeHub::start_on(None, seen.clone()).await;
    let r = relay("sin-1", Some(&hub.origin), Timing::default(), |_| {}).await;
    assert!(eventually(SECS, || r.server.has_keys()).await);
    let o = r.origin.as_str();
    let agent_h = Holder::new("sin-1", "a-1", TicketRole::Agent, "m-1", 33, &[]);
    let mut agent = Client::login(o, &agent_h).await;
    let mut devs = Vec::new();
    for (i, seed) in [41u8, 42].into_iter().enumerate() {
        let h = Holder::new("sin-1", "a-1", TicketRole::Device, &format!("d-{i}"), seed, &["m-1"]);
        let mut dev = Client::login(o, &h).await;
        let _ = pair(&mut dev, &mut agent, 1, "m-1").await;
        devs.push(dev);
    }
    // 64 KiB/s, bursts of 128 KiB: 192 KiB from each of two connections.
    hub.tell(json!({"t": "throttle", "account": "a-1", "bytes_per_sec": 65536}));
    assert!(eventually(SECS, || r.server.throttle_of("a-1") == Some(65536)).await);
    let t0 = Instant::now();
    let sends = devs.into_iter().map(|mut dev| {
        tokio::spawn(async move {
            for _ in 0..12 {
                dev.send(Frame::Data {
                    ch: 1,
                    payload: vec![1; 16 * 1024],
                })
                .await;
            }
            // A ping answered once everything before it was read.
            dev.send(Frame::Ping { nonce: 7 }).await;
            loop {
                match dev.next_frame(Duration::from_secs(20)).await {
                    Some(Frame::Pong { .. }) => break,
                    Some(_) => continue,
                    None => panic!("no pong"),
                }
            }
        })
    });
    let drain = tokio::spawn(async move {
        let mut got = 0;
        while got < 2 * 192 * 1024 {
            match agent.next_frame(Duration::from_secs(20)).await {
                Some(Frame::Data { payload, .. }) => got += payload.len(),
                Some(_) => {}
                None => break,
            }
        }
        got
    });
    for s in sends.collect::<Vec<_>>() {
        s.await.unwrap();
    }
    let elapsed = t0.elapsed();
    assert_eq!(drain.await.unwrap(), 2 * 192 * 1024);
    // Apart, each would be through in about a second; together, about four.
    assert!(elapsed > Duration::from_millis(2_500), "the throttle multiplied: {elapsed:?}");
    hub.stop.cancel();
}

/// A revocation the Hub replays (on every sign-in of its link) closes what
/// it covers, and leaves alone a connection on a ticket issued after it.
#[tokio::test]
async fn a_replayed_revocation_leaves_a_later_ticket_alone() {
    let seen = Arc::new(Mutex::new(HubSeen::default()));
    let hub = FakeHub::start_on(None, seen.clone()).await;
    let r = relay("sin-1", Some(&hub.origin), Timing::default(), |_| {}).await;
    assert!(eventually(SECS, || r.server.has_keys()).await);
    let o = r.origin.as_str();
    let old = Holder::new("sin-1", "a-1", TicketRole::Device, "d-1", 31, &["m-1"]);
    let mut dev = Client::login(o, &old).await;
    let at = now_ms();
    let revoke = json!({"t": "revoke", "account": "a-1", "id": "d-1", "at_ms": at});
    hub.tell(revoke.clone());
    assert_eq!(dev.closed_within(Duration::from_secs(1)).await, Some(4403));
    tokio::time::sleep(Duration::from_millis(5)).await;
    let fresh = Holder::new("sin-1", "a-1", TicketRole::Device, "d-1", 31, &["m-1"]);
    let mut again = Client::login(o, &fresh).await;
    // The same revocation, told again.
    hub.tell(revoke);
    assert_eq!(
        again.closed_within(Duration::from_millis(800)).await,
        None,
        "a later ticket is not covered"
    );
    // A ticket issued in the same millisecond as the revocation is covered.
    let mut same = Holder::new("sin-1", "a-1", TicketRole::Device, "d-1", 31, &["m-1"]);
    same.ticket.issued_ms = at;
    let mut c = Client::dial(o).await;
    assert_eq!(c.login_with(&same, &same.text(), o).await["t"], "error");
    hub.stop.cancel();
}

/// The Hub's whole throttle table lifts a throttle whose lift was missed.
#[tokio::test]
async fn the_hubs_throttle_table_lifts_a_missed_throttle() {
    let seen = Arc::new(Mutex::new(HubSeen::default()));
    let hub = FakeHub::start_on(None, seen.clone()).await;
    let r = relay("sin-1", Some(&hub.origin), Timing::default(), |_| {}).await;
    assert!(eventually(SECS, || r.server.has_keys()).await);
    hub.tell(json!({"t": "throttle", "account": "a-1", "bytes_per_sec": 65536}));
    hub.tell(json!({"t": "throttle", "account": "a-2", "bytes_per_sec": 65536}));
    assert!(eventually(SECS, || r.server.throttle_of("a-2").is_some()).await);
    hub.tell(json!({"t": "throttles", "accounts": [{"account": "a-2", "bytes_per_sec": 65536}]}));
    assert!(
        eventually(SECS, || r.server.throttle_of("a-1").is_none()).await,
        "a-1's lift was missed; the table says so"
    );
    assert_eq!(r.server.throttle_of("a-2"), Some(65536));
    hub.stop.cancel();
}

/// Right after a start, a sign-in waits for the Hub's replay of
/// revocations — at most a few seconds, when the Hub never says it.
#[tokio::test]
async fn sign_ins_wait_for_the_hubs_replay_after_a_start() {
    let seen = Arc::new(Mutex::new(HubSeen::default()));
    let hub = FakeHub::start_on(None, seen.clone()).await;
    let r = relay("sin-1", Some(&hub.origin), Timing::default(), |_| {}).await;
    assert!(eventually(SECS, || r.server.synced()).await, "replayed");
    let t0 = Instant::now();
    let _c = Client::dial(&r.origin).await;
    assert!(t0.elapsed() < Duration::from_secs(1));
    hub.stop.cancel();

    let quiet = Arc::new(Mutex::new(HubSeen {
        no_replay: true,
        ..HubSeen::default()
    }));
    let hub = FakeHub::start_on(None, quiet.clone()).await;
    let r = relay("sin-2", Some(&hub.origin), Timing::default(), |_| {}).await;
    let started = r.server.started;
    assert!(eventually(SECS, || r.server.has_keys()).await);
    let o = r.origin.as_str();
    let h = Holder::new("sin-2", "a-1", TicketRole::Device, "d-1", 31, &["m-1"]);
    let mut c = Client::dial(o).await;
    assert!(!r.server.synced());
    assert!(
        started.elapsed() >= rh_relay::server::SYNC_WAIT - Duration::from_millis(100),
        "it waited for the replay"
    );
    // Still no replay: a ticket issued before it could be revoked unheard —
    // refused (busy: try later or elsewhere), not let in.
    let refused = c.login_with(&h, &h.text(), o).await;
    assert_eq!(refused["t"], "error", "{refused}");
    assert_eq!(c.closed_within(SECS).await, Some(1013));
    // The replay arrives: in.
    hub.tell(json!({"t": "replayed", "revocations": 0}));
    assert!(eventually(SECS, || r.server.synced()).await);
    let _in = Client::login(o, &h).await;
    hub.stop.cancel();
}

/// Chain v3 checklist item 11: a host signs in with one key as an agent and,
/// with a second ticket, as a device — two connections, two roles. As a
/// device it reaches the hosts its ticket names, never itself (even if a
/// ticket named it); as an agent it still opens nothing.
#[tokio::test]
async fn a_host_is_an_agent_and_a_device_on_two_connections_and_never_reaches_itself() {
    let r = relay("sin-1", None, Timing::default(), |_| {}).await;
    let o = r.origin.as_str();
    let h1_agent = Holder::new("sin-1", "a-1", TicketRole::Agent, "h-1", 41, &[]);
    // A ticket the hub would never issue (it names the host itself): the relay refuses that pair anyway.
    let h1_device = Holder::new("sin-1", "a-1", TicketRole::Device, "h-1", 41, &["h-1", "h-2"]);
    let h2_agent = Holder::new("sin-1", "a-1", TicketRole::Agent, "h-2", 42, &[]);
    let mut as_agent = Client::login(o, &h1_agent).await;
    let mut as_device = Client::login(o, &h1_device).await;
    let mut other = Client::login(o, &h2_agent).await;
    // The device role reaches the other host.
    let other_ch = pair(&mut as_device, &mut other, 1, "h-2").await;
    traffic(&mut as_device, &mut other, 1, other_ch, 1000).await;
    // Not itself.
    as_device.send(Frame::Open { ch: 2, peer: "h-1".into() }).await;
    match as_device.next_frame(SECS).await {
        Some(Frame::Close { ch: 2, code, .. }) => assert_eq!(code, rh_relay::route::CLOSE_FORBIDDEN),
        f => panic!("an OPEN to itself: {f:?}"),
    }
    assert!(
        as_agent.next_frame(Duration::from_millis(300)).await.is_none(),
        "its agent side heard nothing"
    );
    // The agent role still opens nothing.
    as_agent.send(Frame::Open { ch: 1, peer: "h-2".into() }).await;
    match as_agent.next_frame(SECS).await {
        Some(Frame::Close { ch: 1, code, .. }) => assert_eq!(code, rh_relay::route::CLOSE_PROTOCOL),
        f => panic!("an agent's OPEN: {f:?}"),
    }
}
