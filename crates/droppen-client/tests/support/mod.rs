//! What the end-to-end tests of a standalone device stand on, all in this
//! process and none of it the hub: an account's chain made here (an admin
//! device, a host that accepts the device under test, that device), a relay
//! (`droppen-relay-core`, the library the relay binary runs) with the test
//! hub's ticket key pinned, a hub that answers the few routes a device reads,
//! and a host's agent on the relay that admits channels exactly as the real
//! one does (`droppen_proto::handshake::accept` with a live clock) and
//! bridges each `rpc` pipe to a handler the test gives.
//!
//! Shared by `droppen-client`'s tests and the a2a proxy's (`#[path]`).

#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use droppen_client::device::{DeviceOptions, StandaloneDevice};
use droppen_client::identity::DeviceIdentity;
use droppen_client::relays::RelayOptions;
use droppen_client::verified::VerifiedChain;
use droppen_client::{hex, now_ms};
use droppen_proto::bridge::{self, Kind, MessageReader, RequestHead, ResponseHead};
use droppen_proto::chain::{
    genesis3_draft, machine_key_commit, user_key_commit, Accept, Action, Attestation, ChainClock, ChainState, DeviceV3, Entry, Flags, KeyCommit,
    KeyCommits, Signer,
};
use droppen_proto::channel::Session;
use droppen_proto::driver::{Channel, Incoming, StreamEvent, StreamWriter};
use droppen_proto::frame::Frame;
use droppen_proto::handshake::{self, Accepted, AgentSide, FRESHNESS_MAX_AGE_MS};
use droppen_proto::identity::fingerprint;
use droppen_proto::keys::DhKeypair;
use droppen_proto::mux::{Role, StreamClass};
use droppen_proto::noise::{RekeyPolicy, ReplayGuard};
use droppen_proto::sig::{HubLogin, LoginRole};
use droppen_proto::ticket::{RelayTicket, TicketLimits, TicketRole};
use droppen_relay_core::config::{Config, Timing, TrustProxy};
use ed25519_dalek::SigningKey;
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

pub const ACCOUNT: &str = "acct-1";
pub const ADMIN: &str = "admin-1";
pub const HOST: &str = "host-1";
pub const PROXY: &str = "proxy-1";
const HUB_KEY: [u8; 32] = [42; 32];

pub fn hub_key() -> SigningKey {
    SigningKey::from_bytes(&HUB_KEY)
}

/// Keys for one id, the same in every run.
pub fn keys(seed: u8) -> (SigningKey, DhKeypair) {
    (SigningKey::from_bytes(&[seed; 32]), DhKeypair::from_secret([seed.wrapping_add(100); 32]))
}

/// An account's chain as its admin device writes it.
pub struct Account {
    pub chain: ChainState,
    pub admin: SigningKey,
    pub fingerprint: [u8; 16],
}

impl Account {
    /// The account as it was up to and including entry `seq`: a branch to
    /// write a fork on.
    pub fn at(&self, seq: u64) -> Account {
        let entries = &self.chain.entries()[..=seq as usize];
        Account {
            chain: ChainState::verify(entries, None, &mut ChainClock::history()).unwrap(),
            admin: self.admin.clone(),
            fingerprint: self.fingerprint,
        }
    }

    /// Genesis (the admin), user key epoch 0, the host with its first
    /// machine key epoch, the device under test (no flags), and the host
    /// accepting it.
    pub fn new() -> Account {
        let ak = SigningKey::from_bytes(&[9; 32]);
        let (admin, admin_dh) = keys(1);
        let genesis = genesis3_draft(
            ACCOUNT,
            ak.verifying_key().to_bytes(),
            DeviceV3 {
                id: ADMIN.into(),
                sign_key: admin.verifying_key().to_bytes(),
                dh_key: admin_dh.public(),
                flags: Flags {
                    host: false,
                    admin: true,
                    a2a: false,
                },
            },
            None,
            now_ms(),
        )
        .sign(&ak);
        let chain = ChainState::verify(&[genesis], None, &mut ChainClock::history()).unwrap();
        let mut account = Account {
            fingerprint: fingerprint(&chain.account_key()),
            chain,
            admin,
        };
        account.append(vec![Action::KeyEpochs(KeyCommits {
            user: Some(KeyCommit {
                epoch: 0,
                hash: user_key_commit(ACCOUNT, 0, &[7; 32]),
            }),
            machines: Vec::new(),
        })]);
        let (host, host_dh) = keys(2);
        account.append(vec![
            Action::AddDevice3(DeviceV3 {
                id: HOST.into(),
                sign_key: host.verifying_key().to_bytes(),
                dh_key: host_dh.public(),
                flags: Flags {
                    host: true,
                    admin: false,
                    a2a: false,
                },
            }),
            Action::KeyEpochs(KeyCommits {
                user: None,
                machines: vec![(
                    HOST.into(),
                    KeyCommit {
                        epoch: 0,
                        hash: machine_key_commit(ACCOUNT, HOST, 0, &[8; 32]),
                    },
                )],
            }),
        ]);
        let (proxy, proxy_dh) = keys(3);
        account.append(vec![Action::AddDevice3(DeviceV3 {
            id: PROXY.into(),
            sign_key: proxy.verifying_key().to_bytes(),
            dh_key: proxy_dh.public(),
            flags: Flags::NONE,
        })]);
        account.append(vec![Action::SetAccept {
            host: HOST.into(),
            from: Accept::Devices(vec![PROXY.into()]),
        }]);
        account
    }

    /// Sign and apply `actions` as the admin; the new entries.
    pub fn append(&mut self, actions: Vec<Action>) -> Vec<Entry> {
        let mut out = Vec::new();
        for action in actions {
            let entry = self.chain.draft(Signer::Device(ADMIN.into()), action, now_ms()).sign(&self.admin);
            self.chain.apply(&entry, &mut ChainClock::history()).unwrap();
            out.push(entry);
        }
        out
    }

    /// The host's machine key epoch `epoch` (what an entry that narrows whom
    /// the host accepts must be followed by).
    pub fn host_key(epoch: u32) -> Action {
        Action::KeyEpochs(KeyCommits {
            user: None,
            machines: vec![(
                HOST.into(),
                KeyCommit {
                    epoch,
                    hash: machine_key_commit(ACCOUNT, HOST, epoch, &[8 + epoch as u8; 32]),
                },
            )],
        })
    }

    /// The admin vouching for the chain as it is now.
    pub fn attestation(&self) -> Attestation {
        Attestation::create(&self.chain, ADMIN, &self.admin, now_ms())
    }

    pub fn entries(&self) -> Vec<Entry> {
        self.chain.entries().to_vec()
    }
}

// ------------------------------------------------------------------ relay

/// A relay in this process, tickets checked against the test hub's key.
pub struct TestRelay {
    pub id: String,
    pub origin: String,
    pub addr: SocketAddr,
    pub server: Arc<droppen_relay_core::Server>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    done: Option<tokio::task::JoinHandle<Result<(), String>>>,
    timing: Timing,
}

pub fn timing(renew_grace_ms: u64) -> Timing {
    Timing {
        login_timeout: Duration::from_secs(10),
        login_skew_ms: 60_000,
        renew_grace_ms,
        // Without a hub the control link is always down: keep the outage
        // grace from ever keeping an expired ticket alive.
        outage_after: Duration::from_secs(3600),
        outage_grace_ms: 0,
        silence: Duration::from_secs(90),
        usage_every: Duration::from_secs(60),
        usage_keep: Duration::from_secs(600),
        health_every: Duration::from_secs(20),
        uplink_backoff_max: Duration::from_secs(30),
    }
}

impl TestRelay {
    pub async fn start(id: &str, timing: Timing) -> TestRelay {
        Self::start_on(id, "127.0.0.1:0".parse().unwrap(), timing).await
    }

    async fn start_on(id: &str, addr: SocketAddr, timing: Timing) -> TestRelay {
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let origin = format!("ws://{addr}");
        let config = Config {
            bind: addr,
            origin: origin.clone(),
            hub: None,
            id: id.into(),
            key: SigningKey::from_bytes(&[77; 32]),
            registration_token: None,
            pinned_keys: Some(vec![(1, hub_key().verifying_key().to_bytes())]),
            state_dir: None,
            trust_proxy: TrustProxy::None,
            max_conns: 1000,
            drain: Duration::from_millis(100),
            timing,
        };
        let server = droppen_relay_core::Server::new(config);
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let done = tokio::spawn(droppen_relay_core::run(server.clone(), listener, async move {
            let _ = stopped.await;
        }));
        TestRelay {
            id: id.into(),
            origin,
            addr,
            server,
            stop: Some(stop),
            done: Some(done),
            timing,
        }
    }

    /// Stop it (everyone hears `goaway`, then a close) and start it again on
    /// the same address.
    pub async fn restart(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(done) = self.done.take() {
            let _ = done.await;
        }
        let again = Self::start_on(&self.id, self.addr, self.timing).await;
        *self = again;
    }
}

// -------------------------------------------------------------------- hub

/// What the test hub knows and answers.
pub struct HubState {
    pub entries: Vec<Entry>,
    /// `(id, origin)`.
    pub relays: Vec<(String, String)>,
    /// The relays the host's agent is attached to, with its round trip.
    pub agents: HashMap<String, HashMap<String, u64>>,
    /// How long a ticket lasts.
    pub ticket_ttl_ms: u64,
    /// The device's signing key, which its tickets name.
    pub device_key: [u8; 32],
    /// Hosts its tickets name (`None`: the hosts the chain lets it control).
    pub peers: Option<Vec<String>>,
    /// Answer every session with this status and error code.
    pub refuse_sessions: Option<(u16, &'static str)>,
    pub tickets: u32,
    pub sessions: u32,
}

#[derive(Clone)]
pub struct TestHub {
    pub url: String,
    pub state: Arc<Mutex<HubState>>,
}

impl TestHub {
    pub async fn start(account: &Account, relays: &[&TestRelay]) -> TestHub {
        let (proxy, _) = keys(3);
        let state = Arc::new(Mutex::new(HubState {
            entries: account.entries(),
            relays: relays.iter().map(|r| (r.id.clone(), r.origin.clone())).collect(),
            agents: HashMap::from([(HOST.to_string(), relays.iter().map(|r| (r.id.clone(), 10)).collect())]),
            ticket_ttl_ms: 15 * 60 * 1000,
            device_key: proxy.verifying_key().to_bytes(),
            peers: None,
            refuse_sessions: None,
            tickets: 0,
            sessions: 0,
        }));
        let app = Router::new()
            .route("/v1/session/challenge", post(|| async { Json(json!({"nonce": hex(&[5; 32])})) }))
            .route("/v1/session", post(session))
            .route("/v1/chain", get(chain))
            .route("/v1/relays", get(directory))
            .route("/v1/relays/tickets", post(tickets))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        TestHub { url, state }
    }

    pub fn set_entries(&self, entries: Vec<Entry>) {
        self.state.lock().unwrap().entries = entries;
    }

    pub fn tickets(&self) -> u32 {
        self.state.lock().unwrap().tickets
    }
}

async fn session(State(state): State<Arc<Mutex<HubState>>>, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
    let mut st = state.lock().unwrap();
    if let Some((status, error)) = st.refuse_sessions {
        return (StatusCode::from_u16(status).unwrap(), Json(json!({"error": error})));
    }
    st.sessions += 1;
    (
        StatusCode::OK,
        Json(
            json!({"account": body["account"], "device": body["device"], "token": format!("session-{}", st.sessions), "expires_ms": now_ms() + 3_600_000}),
        ),
    )
}

type Answer = (StatusCode, Json<Value>);

/// A session the hub no longer takes, once it refuses new ones.
fn denied(st: &HubState) -> Option<Answer> {
    st.refuse_sessions.map(|_| (StatusCode::UNAUTHORIZED, Json(json!({"error": "denied"}))))
}

async fn chain(State(state): State<Arc<Mutex<HubState>>>, Query(q): Query<HashMap<String, String>>) -> Answer {
    let st = state.lock().unwrap();
    if let Some(denied) = denied(&st) {
        return denied;
    }
    let from: usize = q.get("from").and_then(|f| f.parse().ok()).unwrap_or(0);
    let entries: Vec<String> = st.entries.iter().skip(from).map(|e| hex(&e.encode())).collect();
    (
        StatusCode::OK,
        Json(json!({"entries": entries, "seq": st.entries.last().map(|e| e.seq())})),
    )
}

async fn directory(State(state): State<Arc<Mutex<HubState>>>) -> Answer {
    let st = state.lock().unwrap();
    if let Some(denied) = denied(&st) {
        return denied;
    }
    (
        StatusCode::OK,
        Json(json!({
            "relays": st.relays.iter().map(|(id, origin)| json!({"id": id, "city": "test", "region": "test", "origin": origin, "state": "up", "load": 0.1})).collect::<Vec<_>>(),
            "limit": 4,
            "account_set": st.relays.iter().map(|(id, _)| json!({"id": id, "last_used_ms": 0})).collect::<Vec<_>>(),
            "emergency": [],
            "agents": st.agents,
            "throttled": false,
        })),
    )
}

async fn tickets(State(state): State<Arc<Mutex<HubState>>>, Json(body): Json<Value>) -> Answer {
    let mut st = state.lock().unwrap();
    if let Some(denied) = denied(&st) {
        return denied;
    }
    let mut clock = ChainClock::history();
    let chain = ChainState::verify(&st.entries, None, &mut clock).unwrap();
    let peers = st
        .peers
        .clone()
        .unwrap_or_else(|| chain.hosts_admitting(PROXY).map(|d| d.info.id.clone()).collect());
    let mut out = Vec::new();
    for relay in body["relays"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let Some((_, origin)) = st.relays.iter().find(|(id, _)| id == relay).cloned() else {
            continue;
        };
        let now = now_ms();
        let ticket = RelayTicket {
            kid: 1,
            relay: relay.into(),
            account: ACCOUNT.into(),
            role: TicketRole::Device,
            id: PROXY.into(),
            sign_key: st.device_key,
            peers: peers.clone(),
            limits: limits(),
            flags: 0,
            issued_ms: now,
            expires_ms: now + st.ticket_ttl_ms,
        };
        st.tickets += 1;
        out.push(json!({"relay": relay, "origin": origin, "ticket": ticket.sign(&hub_key()).unwrap(), "expires_ms": ticket.expires_ms}));
    }
    (StatusCode::OK, Json(json!({"tickets": out, "errors": []})))
}

fn limits() -> TicketLimits {
    TicketLimits {
        bytes_per_sec: 64 * 1024 * 1024,
        frames_per_sec: 100_000,
        channels: 64,
        account_conns: 48,
    }
}

// ------------------------------------------------------------------ agent

/// One request the test agent's bridge took: its head and whole body.
pub struct Request {
    pub class: StreamClass,
    pub head: RequestHead,
    pub body: Vec<u8>,
}

/// How a handler answers: a head, the body in pieces, a close.
pub struct Responder {
    writer: StreamWriter,
}

impl Responder {
    pub async fn head(&self, status: u16, headers: &[(&str, &str)]) {
        let head = ResponseHead {
            status,
            headers: headers.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect(),
        };
        let _ = self.writer.send(&bridge::message(Kind::Head, &head.encode().unwrap())).await;
    }

    pub async fn chunk(&self, data: &[u8]) -> bool {
        self.writer.send(&bridge::message(Kind::Binary, data)).await.is_ok()
    }

    pub fn close(&self, code: u16) {
        self.writer.close(code);
    }

    /// A whole JSON answer.
    pub async fn json(&self, status: u16, body: &Value) {
        self.head(status, &[("content-type", "application/json")]).await;
        self.chunk(body.to_string().as_bytes()).await;
        self.close(0);
    }
}

pub type Handler = Arc<dyn Fn(Request, Responder) -> BoxFuture<'static, ()> + Send + Sync>;

/// A handler from an async closure.
pub fn handler<F, Fut>(f: F) -> Handler
where
    F: Fn(Request, Responder) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    Arc::new(move |request, responder| Box::pin(f(request, responder)))
}

/// Answers every request with what it was: `{method, path, headers, body}`.
pub fn echo() -> Handler {
    handler(|request: Request, responder: Responder| async move {
        let body = json!({
            "method": request.head.method,
            "path": request.head.path,
            "headers": request.head.headers,
            "body": String::from_utf8_lossy(&request.body),
        });
        responder.json(200, &body).await;
    })
}

/// The agent's view of the account, and what it holds.
pub struct AgentState {
    pub chain: ChainState,
    pub clock: ChainClock,
    pub guard: ReplayGuard,
    pub attestations: Vec<Attestation>,
    /// Handshake outcomes: `open`, `need`, `stale`, or the refusal.
    pub outcomes: Vec<String>,
    /// After answering "send entries" this many times, catch up as the hub
    /// would push the chain (`None`: never).
    pub catch_up_after_need: Option<(u32, Vec<Entry>)>,
    channels: Vec<Channel>,
}

/// A host's agent on a relay.
#[derive(Clone)]
pub struct TestAgent {
    pub state: Arc<Mutex<AgentState>>,
    handler: Arc<Mutex<Handler>>,
    pub attached: Arc<AtomicU32>,
}

impl TestAgent {
    /// An agent whose view is `entries`, vouched for by `attestation`.
    pub fn new(entries: &[Entry], attestation: Option<Attestation>, handler: Handler) -> TestAgent {
        let mut clock = ChainClock::live(now_ms(), 0);
        let chain = ChainState::verify(entries, None, &mut clock).unwrap();
        TestAgent {
            state: Arc::new(Mutex::new(AgentState {
                chain,
                clock,
                guard: ReplayGuard::after_restart(now_ms().saturating_sub(60_000)),
                attestations: attestation.into_iter().collect(),
                outcomes: Vec::new(),
                catch_up_after_need: None,
                channels: Vec::new(),
            })),
            handler: Arc::new(Mutex::new(handler)),
            attached: Arc::default(),
        }
    }

    pub fn set_handler(&self, handler: Handler) {
        *self.handler.lock().unwrap() = handler;
    }

    pub fn outcomes(&self) -> Vec<String> {
        self.state.lock().unwrap().outcomes.clone()
    }

    /// Close every open channel with `code`.
    pub fn close_channels(&self, code: u16, reason: &str) {
        for channel in self.state.lock().unwrap().channels.drain(..) {
            channel.close(code, reason);
        }
    }

    /// Sign in on `relay` as the host and serve.
    pub async fn attach(&self, relay: &TestRelay) {
        let (key, _) = keys(2);
        let now = now_ms();
        let ticket = RelayTicket {
            kid: 1,
            relay: relay.id.clone(),
            account: ACCOUNT.into(),
            role: TicketRole::Agent,
            id: HOST.into(),
            sign_key: key.verifying_key().to_bytes(),
            peers: Vec::new(),
            limits: limits(),
            flags: 0,
            issued_ms: now,
            expires_ms: now + 15 * 60 * 1000,
        }
        .sign(&hub_key())
        .unwrap();
        let (ws, _) = tokio_tungstenite::connect_async(format!("{}/v1/connect", relay.origin)).await.unwrap();
        let (mut sink, mut stream) = ws.split();
        let challenge: Value = match stream.next().await.unwrap().unwrap() {
            Message::Text(t) => serde_json::from_str(t.as_str()).unwrap(),
            other => panic!("{other:?}"),
        };
        let nonce: [u8; 32] = droppen_client::key32(challenge["nonce"].as_str().unwrap()).unwrap();
        let sig = HubLogin {
            role: LoginRole::RelayAgent,
            account: ACCOUNT.into(),
            id: HOST.into(),
            hub: relay.origin.clone(),
            nonce,
            time_ms: now,
        }
        .sign(&key)
        .unwrap();
        sink.send(Message::Text(
            json!({"t": "login", "ticket": ticket, "time_ms": now, "sig": hex(&sig)})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        let welcome: Value = match stream.next().await.unwrap().unwrap() {
            Message::Text(t) => serde_json::from_str(t.as_str()).unwrap(),
            other => panic!("{other:?}"),
        };
        assert_eq!(welcome["t"], "welcome", "{welcome}");
        let (out, mut out_rx) = mpsc::unbounded_channel::<Frame>();
        tokio::spawn(async move {
            let mut ping = tokio::time::interval(Duration::from_secs(20));
            loop {
                let message = tokio::select! {
                    f = out_rx.recv() => match f { Some(f) => Message::Binary(f.encode().unwrap().into()), None => break },
                    _ = ping.tick() => Message::Binary(Frame::Ping { nonce: 1 }.encode().unwrap().into()),
                };
                if sink.send(message).await.is_err() {
                    break;
                }
            }
        });
        self.attached.fetch_add(1, Ordering::Relaxed);
        let agent = self.clone();
        tokio::spawn(async move {
            let mut routes: HashMap<u32, mpsc::UnboundedSender<Frame>> = HashMap::new();
            let mut pending: HashMap<u32, String> = HashMap::new();
            while let Some(Ok(message)) = stream.next().await {
                let Message::Binary(bytes) = message else { continue };
                let Ok(frame) = Frame::decode(&bytes) else { continue };
                match frame {
                    Frame::Open { ch, peer } => {
                        pending.insert(ch, peer);
                    }
                    Frame::Data { ch, payload } => {
                        if let Some(route) = routes.get(&ch) {
                            let _ = route.send(Frame::Data { ch, payload });
                        } else if let Some(device) = pending.remove(&ch) {
                            if let Some(route) = agent.admit(ch, &device, &payload, &out) {
                                routes.insert(ch, route);
                            }
                        }
                    }
                    Frame::Credit { ch, bytes } => {
                        if let Some(route) = routes.get(&ch) {
                            let _ = route.send(Frame::Credit { ch, bytes });
                        }
                    }
                    Frame::Close { ch, code, reason } => {
                        if let Some(route) = routes.remove(&ch) {
                            let _ = route.send(Frame::Close { ch, code, reason });
                        }
                        pending.remove(&ch);
                    }
                    _ => {}
                }
            }
        });
    }

    /// A device's first message on `ch`, admitted as the real agent does.
    fn admit(&self, ch: u32, device: &str, hello: &[u8], out: &mpsc::UnboundedSender<Frame>) -> Option<mpsc::UnboundedSender<Frame>> {
        let mut guard = self.state.lock().unwrap();
        let st = &mut *guard;
        let (_, host_dh) = keys(2);
        let attestations = st.attestations.clone();
        let server = json!({"agent": "test-agent", "version": "0", "features": []}).to_string();
        let side = AgentSide {
            chain: &mut st.chain,
            clock: &mut st.clock,
            guard: &mut st.guard,
            key: &host_dh,
            machine: HOST,
            policy: RekeyPolicy::default(),
            server_info: server.as_bytes(),
            attestations: &attestations,
            freshness_max_age_ms: FRESHNESS_MAX_AGE_MS,
        };
        let refuse = |code: u16, reason: &str| {
            let _ = out.send(Frame::Close {
                ch,
                code,
                reason: reason.into(),
            });
        };
        match handshake::accept(side, device, hello, now_ms()) {
            Ok(Accepted::Open { reply, transport, .. }) => {
                st.outcomes.push("open".into());
                let _ = out.send(Frame::Data { ch, payload: reply });
                let (tx, rx) = mpsc::unbounded_channel();
                let (channel, mut incoming) = Channel::spawn(Session::new(ch, *transport, Role::Responder), rx, out.clone());
                st.channels.push(channel);
                let handler = self.handler.clone();
                tokio::spawn(async move {
                    while let Some(incoming) = incoming.recv().await {
                        let handler = handler.lock().unwrap().clone();
                        tokio::spawn(serve(incoming, handler));
                    }
                });
                Some(tx)
            }
            Ok(Accepted::NeedEntries { reply, .. }) => {
                st.outcomes.push("need".into());
                let _ = out.send(Frame::Data { ch, payload: reply });
                refuse(4409, "send chain entries");
                if let Some((left, entries)) = st.catch_up_after_need.as_mut() {
                    *left = left.saturating_sub(1);
                    if *left == 0 {
                        let entries = std::mem::take(entries);
                        st.chain.merge(&entries, &mut st.clock).unwrap();
                        st.catch_up_after_need = None;
                    }
                }
                None
            }
            Ok(Accepted::Stale { reply, .. }) => {
                st.outcomes.push("stale".into());
                let _ = out.send(Frame::Data { ch, payload: reply });
                refuse(4412, "stale chain view: attest and retry");
                None
            }
            Err(error) => {
                st.outcomes.push(format!("refused: {error}"));
                refuse(4401, "handshake refused");
                None
            }
        }
    }
}

/// One stream, bridged to `handler` as the agent's bridge would: the request
/// head and its whole body first.
async fn serve(incoming: Incoming, handler: Handler) {
    let Incoming { class, meta, mut stream } = incoming;
    let Ok(head) = RequestHead::decode(&meta) else {
        return stream.close(bridge::CLOSE_BAD_REQUEST);
    };
    let mut reader = MessageReader::new();
    let mut body = Vec::new();
    'read: loop {
        match stream.recv().await {
            Some(StreamEvent::Data(d)) => {
                reader.push(&d);
                while let Some((kind, payload)) = reader.next_message().unwrap() {
                    match kind {
                        Kind::End => break 'read,
                        Kind::Binary | Kind::Text => body.extend_from_slice(&payload),
                        Kind::Head => {}
                    }
                }
            }
            _ => return,
        }
    }
    let responder = Responder { writer: stream.writer() };
    handler(Request { class, head, body }, responder).await;
    // Held until the handler is done, so the stream is not let go under it.
    drop(stream);
}

// ----------------------------------------------------------------- device

/// Options for a device under test: loopback relays, short waits.
pub fn device_options() -> DeviceOptions {
    DeviceOptions {
        client: "droppen-client-tests".into(),
        reply_timeout: Duration::from_secs(5),
        retry_step: Duration::from_millis(50),
        relays: RelayOptions {
            allow_loopback: true,
            silence: Duration::from_secs(60),
            presence_wait: Duration::from_secs(2),
            renew: true,
        },
    }
}

/// A fresh scratch directory.
pub fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "droppen-client-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The device under test, joined to `account` at `hub`, kept in `dir`.
pub fn device_in(dir: &std::path::Path, hub: &TestHub, account: &Account, options: DeviceOptions) -> StandaloneDevice {
    let (sign, dh) = keys(3);
    let identity = DeviceIdentity::new(&hub.url, ACCOUNT, PROXY, sign, dh);
    identity.save(dir).unwrap();
    let chain = VerifiedChain::pinned(dir, ACCOUNT, account.fingerprint);
    StandaloneDevice::new(identity, chain, options).unwrap()
}

/// How a [`Stand`] is set up.
pub struct StandConfig {
    pub handler: Handler,
    pub timing: Timing,
    pub ticket_ttl_ms: u64,
    /// The host's agent signs in on the relay.
    pub attach: bool,
    /// An admin vouched for the agent's view.
    pub vouched: bool,
    pub options: DeviceOptions,
}

impl StandConfig {
    pub fn new(handler: Handler) -> StandConfig {
        StandConfig {
            handler,
            timing: timing(60_000),
            ticket_ttl_ms: 15 * 60 * 1000,
            attach: true,
            vouched: true,
            options: device_options(),
        }
    }
}

/// The whole stand: a relay, a hub, a host attached there with a fresh view
/// vouched for, and the device (its chain synced).
pub struct Stand {
    pub account: Account,
    pub relay: TestRelay,
    pub hub: TestHub,
    pub agent: TestAgent,
    pub device: Arc<StandaloneDevice>,
    pub dir: PathBuf,
}

impl Stand {
    pub async fn new(handler: Handler) -> Stand {
        Self::with(StandConfig::new(handler)).await
    }

    pub async fn with(config: StandConfig) -> Stand {
        let account = Account::new();
        let relay = TestRelay::start("rly-1", config.timing).await;
        let hub = TestHub::start(&account, &[&relay]).await;
        hub.state.lock().unwrap().ticket_ttl_ms = config.ticket_ttl_ms;
        let agent = TestAgent::new(&account.entries(), config.vouched.then(|| account.attestation()), config.handler);
        if config.attach {
            agent.attach(&relay).await;
        }
        let dir = scratch("stand");
        let device = Arc::new(device_in(&dir, &hub, &account, config.options));
        device.sync_chain().await.unwrap();
        Stand {
            account,
            relay,
            hub,
            agent,
            device,
            dir,
        }
    }

    /// The admin appends `actions`; the hub serves them, and the agent has
    /// them when `agent` (as the hub would push them).
    pub fn append(&mut self, actions: Vec<Action>, agent: bool) {
        let entries = self.account.append(actions);
        self.hub.set_entries(self.account.entries());
        if agent {
            let mut st = self.agent.state.lock().unwrap();
            let st = &mut *st;
            st.chain.merge(&entries, &mut st.clock).unwrap();
            st.attestations = vec![self.account.attestation()];
        }
    }

    /// `GET path` with no body.
    pub fn get(path: &str) -> droppen_client::rpc::RpcRequest {
        droppen_client::rpc::RpcRequest {
            method: "GET".into(),
            path: path.into(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
}

/// A handler that answers an event stream: `data: <n>` every `every`, until
/// the pipe goes.
pub fn events(every: Duration) -> Handler {
    handler(move |_request: Request, responder: Responder| async move {
        responder.head(200, &[("content-type", "text/event-stream")]).await;
        for n in 0u64.. {
            if !responder.chunk(format!("data: {n}\n\n").as_bytes()).await {
                break;
            }
            tokio::time::sleep(every).await;
        }
    })
}

/// Poll `f` until it holds or `within` passed.
pub async fn eventually(within: Duration, mut f: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    f()
}
