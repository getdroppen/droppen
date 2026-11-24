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

use axum::extract::ws::{Message as AxMessage, WebSocket, WebSocketUpgrade};
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
    genesis3_draft, machine_key_commit, user_key_commit, Action, Attestation, ChainClock, ChainState, DeviceV3, Entry, Flags, Freshness, KeyCommit,
    KeyCommits, Signer,
};
use droppen_proto::channel::Session;
use droppen_proto::driver::{Channel, Incoming, StreamEvent, StreamWriter};
use droppen_proto::frame::Frame;
use droppen_proto::handshake::{
    self, Accepted, AgentSide, HandshakeError, HelloBody, ReplyBody, ReplyStatus, FRESHNESS_MAX_AGE_MS, MAX_HELLO_ENTRIES,
};
use droppen_proto::identity::fingerprint;
use droppen_proto::keys::DhKeypair;
use droppen_proto::mux::{Role, StreamClass};
use droppen_proto::noise::{self, Binding, RekeyPolicy, ReplayGuard};
use droppen_proto::sig::{HubLogin, LoginRole};
use droppen_proto::ticket::{RelayTicket, TicketLimits, TicketRole};
use droppen_relay_core::config::{Config, Timing, TrustProxy};
use ed25519_dalek::SigningKey;
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// The test CA, a relay certificate it issued for `127.0.0.1` and
/// `localhost` (ECDSA P-256, good until 2126) and its key: fixtures for the
/// `wss://` relay, made once with openssl and kept here.
const TLS_CA: &[u8] = include_bytes!("tls/ca.der");
const TLS_RELAY_CERT: &[u8] = include_bytes!("tls/relay.der");
const TLS_RELAY_KEY: &[u8] = include_bytes!("tls/relay.key.der");

/// A device's TLS for the test relays: the test CA as its only root.
pub fn test_tls() -> tokio_tungstenite::Connector {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(rustls::pki_types::CertificateDer::from(TLS_CA.to_vec())).unwrap();
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    tokio_tungstenite::Connector::Rustls(Arc::new(config))
}

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
    /// machine key epoch, the device under test (an A2A proxy: the a2a flag
    /// alone), and the host taking A2A calls from it (`SetA2aAccept`).
    pub fn new() -> Account {
        let mut account = Account::hosts_only();
        let (proxy, proxy_dh) = keys(3);
        account.append(vec![Action::AddDevice3(DeviceV3 {
            id: PROXY.into(),
            sign_key: proxy.verifying_key().to_bytes(),
            dh_key: proxy_dh.public(),
            flags: Flags::A2A_DEVICE,
        })]);
        account.append(vec![Action::SetA2aAccept {
            host: HOST.into(),
            devices: vec![PROXY.into()],
        }]);
        account
    }

    /// The account before any A2A proxy joined: genesis (the admin), user key
    /// epoch 0, the host with its first machine key epoch.
    pub fn hosts_only() -> Account {
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
    /// What its sign-ins name and devices dial: `ws://` on its own port, or
    /// `wss://` on a TLS front ([`TestRelay::start_tls`]).
    pub origin: String,
    /// Where the host's agent dials it (plain, behind any TLS front).
    pub dial: String,
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
        Self::start_as(id, addr, timing, None).await
    }

    /// A relay behind a TLS front on its own port: devices dial `wss://`
    /// there, verified against the test CA ([`test_tls`]); the host's agent
    /// dials the relay itself.
    pub async fn start_tls(id: &str, timing: Timing) -> TestRelay {
        let front = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("wss://{}", front.local_addr().unwrap());
        let relay = Self::start_as(id, "127.0.0.1:0".parse().unwrap(), timing, Some(origin)).await;
        let behind = relay.addr;
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls::pki_types::CertificateDer::from(TLS_RELAY_CERT.to_vec())],
                rustls::pki_types::PrivateKeyDer::Pkcs8(TLS_RELAY_KEY.to_vec().into()),
            )
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        tokio::spawn(async move {
            while let Ok((tcp, _)) = front.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let (Ok(mut tls), Ok(mut plain)) = (acceptor.accept(tcp).await, tokio::net::TcpStream::connect(behind).await) else {
                        return;
                    };
                    let _ = tokio::io::copy_bidirectional(&mut tls, &mut plain).await;
                });
            }
        });
        relay
    }

    async fn start_as(id: &str, addr: SocketAddr, timing: Timing, origin: Option<String>) -> TestRelay {
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let dial = format!("ws://{addr}");
        let origin = origin.unwrap_or_else(|| dial.clone());
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
            dial,
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
    /// The device session tokens it minted, and whose they are.
    pub session_devices: HashMap<String, String>,
    /// Hosts its tickets name (`None`: the hosts the chain lets it control).
    pub peers: Option<Vec<String>>,
    /// Enrollment codes a page registered, by token (hex).
    pub enrollments: HashMap<String, Enrollment>,
    /// Its own origin, as sign-ins name it.
    pub origin: String,
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
        let state = Arc::new(Mutex::new(HubState {
            entries: account.entries(),
            relays: relays.iter().map(|r| (r.id.clone(), r.origin.clone())).collect(),
            agents: HashMap::from([(HOST.to_string(), relays.iter().map(|r| (r.id.clone(), 10)).collect())]),
            ticket_ttl_ms: 15 * 60 * 1000,
            session_devices: HashMap::new(),
            peers: None,
            enrollments: HashMap::new(),
            origin: String::new(),
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
            .route("/v1/agent", get(enroll_socket))
            .route("/v1/client", get(client_socket))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        state.lock().unwrap().origin = format!("ws://{}", listener.local_addr().unwrap());
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

impl TestHub {
    /// A page registers an enrollment code's token, of `kind`.
    pub fn register_code(&self, token: &[u8; 16], kind: &str) {
        self.state.lock().unwrap().enrollments.insert(
            hex(token),
            Enrollment {
                kind: kind.into(),
                proved: None,
            },
        );
    }

    /// What the device that claimed the code with `token` proved, once it did.
    pub fn proved(&self, token: &[u8; 16]) -> Option<Proved> {
        self.state.lock().unwrap().enrollments.get(&hex(token)).and_then(|e| e.proved.clone())
    }
}

/// An enrollment code a page registered.
pub struct Enrollment {
    pub kind: String,
    pub proved: Option<Proved>,
}

/// What a device proved for a code: the id the hub gave it, its keys, the proof.
#[derive(Debug, Clone)]
pub struct Proved {
    pub device: String,
    pub sign_key: [u8; 32],
    pub dh_key: [u8; 32],
    pub proof: [u8; 32],
}

/// The hub's enrollment socket (`/v1/agent`), as an A2A proxy claims a code
/// there: `enroll {token, kind}`, `enroll_assigned`, `enroll_proof` (the
/// `HubLogin {role: Device}` signature checked), `enrolled`.
async fn enroll_socket(ws: WebSocketUpgrade, State(state): State<Arc<Mutex<HubState>>>) -> axum::response::Response {
    ws.on_upgrade(move |socket| claim(socket, state))
}

fn text(v: Value) -> AxMessage {
    AxMessage::Text(v.to_string().into())
}

fn json_of(m: Option<Result<AxMessage, axum::Error>>) -> Option<Value> {
    match m {
        Some(Ok(AxMessage::Text(t))) => serde_json::from_str(t.as_str()).ok(),
        _ => None,
    }
}

async fn claim(mut socket: WebSocket, state: Arc<Mutex<HubState>>) {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let nonce = [n as u8; 32];
    if socket.send(text(json!({"t": "challenge", "nonce": hex(&nonce)}))).await.is_err() {
        return;
    }
    let Some(enroll) = json_of(socket.recv().await) else { return };
    let token = enroll["token"].as_str().unwrap_or_default().to_string();
    let known = {
        let st = state.lock().unwrap();
        st.enrollments
            .get(&token)
            .is_some_and(|e| e.proved.is_none() && Some(e.kind.as_str()) == enroll["kind"].as_str())
    };
    if enroll["t"] != "enroll" || !known {
        let _ = socket
            .send(text(json!({"t": "error", "code": "unknown_enrollment", "message": "no such code"})))
            .await;
        return;
    }
    let device = format!("a2a-{n}");
    let _ = socket
        .send(text(json!({"t": "enroll_assigned", "account": ACCOUNT, "device": device})))
        .await;
    let Some(proof) = json_of(socket.recv().await) else { return };
    let key = |name: &str| droppen_client::key32(proof[name].as_str().unwrap_or_default()).ok();
    let sig: Option<[u8; 64]> = droppen_client::unhex(proof["sig"].as_str().unwrap_or_default())
        .ok()
        .and_then(|s| s.try_into().ok());
    let origin = state.lock().unwrap().origin.clone();
    let (Some(sign_key), Some(dh_key), Some(p), Some(sig)) = (key("sign_key"), key("dh_key"), key("proof"), sig) else {
        let _ = socket.send(text(json!({"t": "error", "code": "bad_keys", "message": "malformed"}))).await;
        return;
    };
    let login = HubLogin {
        role: LoginRole::Device,
        account: ACCOUNT.into(),
        id: device.clone(),
        hub: origin,
        nonce,
        time_ms: proof["time_ms"].as_u64().unwrap_or_default(),
    };
    if proof["t"] != "enroll_proof" || proof["kind"] != "a2a" || !login.verify(&sign_key, &sig) {
        let _ = socket
            .send(text(json!({"t": "error", "code": "denied", "message": "the signature does not hold"})))
            .await;
        return;
    }
    if let Some(e) = state.lock().unwrap().enrollments.get_mut(&token) {
        e.proved = Some(Proved {
            device: device.clone(),
            sign_key,
            dh_key,
            proof: p,
        });
    }
    let _ = socket.send(text(json!({"t": "enrolled", "account": ACCOUNT, "device": device}))).await;
}

/// The hub's device socket (`/v1/client`) as far as a device follows the
/// chain on it: a sign-in taken as it comes, then `CHAIN` frames from a
/// `chain_sub`'s `from`, and every entry the hub gets after.
async fn client_socket(ws: WebSocketUpgrade, State(state): State<Arc<Mutex<HubState>>>) -> axum::response::Response {
    ws.on_upgrade(move |socket| follow(socket, state))
}

async fn follow(mut socket: WebSocket, state: Arc<Mutex<HubState>>) {
    if socket.send(text(json!({"t": "challenge", "nonce": hex(&[8; 32])}))).await.is_err() {
        return;
    }
    if json_of(socket.recv().await).is_none() || socket.send(text(json!({"t": "welcome"}))).await.is_err() {
        return;
    }
    let mut cursor: Option<usize> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    loop {
        tokio::select! {
            m = socket.recv() => match m {
                Some(Ok(AxMessage::Text(t))) => {
                    let v: Value = serde_json::from_str(t.as_str()).unwrap_or_default();
                    if v["t"] == "chain_sub" {
                        cursor = Some(v["from"].as_u64().unwrap_or(0) as usize);
                    }
                }
                Some(Ok(_)) => {}
                _ => return,
            },
            _ = tick.tick() => {
                let Some(from) = cursor else { continue };
                let fresh: Vec<Vec<u8>> = state.lock().unwrap().entries.iter().skip(from).map(Entry::encode).collect();
                for batch in fresh.chunks(16) {
                    let frame = Frame::Chain { ch: 0, entries: batch.to_vec() };
                    if socket.send(AxMessage::Binary(frame.encode().unwrap().into())).await.is_err() {
                        return;
                    }
                }
                cursor = Some(from + fresh.len());
            }
        }
    }
}

async fn session(State(state): State<Arc<Mutex<HubState>>>, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
    let mut st = state.lock().unwrap();
    if let Some((status, error)) = st.refuse_sessions {
        return (StatusCode::from_u16(status).unwrap(), Json(json!({"error": error})));
    }
    st.sessions += 1;
    let token = format!("session-{}", st.sessions);
    st.session_devices
        .insert(token.clone(), body["device"].as_str().unwrap_or_default().to_string());
    let answer = json!({"account": body["account"], "device": body["device"], "token": token, "expires_ms": now_ms() + 3_600_000});
    (StatusCode::OK, Json(answer))
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

async fn tickets(State(state): State<Arc<Mutex<HubState>>>, headers: axum::http::HeaderMap, Json(body): Json<Value>) -> Answer {
    let mut st = state.lock().unwrap();
    if let Some(denied) = denied(&st) {
        return denied;
    }
    // Whose session: its device, with the key the chain names for it.
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    let device = st.session_devices.get(token).cloned().unwrap_or_else(|| PROXY.into());
    let mut clock = ChainClock::history();
    let chain = ChainState::verify(&st.entries, None, &mut clock).unwrap();
    let Some(sign_key) = chain.device_record(&device).map(|d| d.info.sign_key) else {
        return (StatusCode::FORBIDDEN, Json(json!({"error": "forbidden"})));
    };
    // The hosts the chain lets the device reach: an A2A proxy's by
    // `may_call_a2a`, any other device's by `may_control`.
    let peers = st.peers.clone().unwrap_or_else(|| match chain.a2a_device(&device) {
        Some(_) => chain.hosts_accepting_a2a(&device).map(|d| d.info.id.clone()).collect(),
        None => chain.hosts_admitting(&device).map(|d| d.info.id.clone()).collect(),
    });
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
            id: device.clone(),
            sign_key,
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
        let (ws, _) = tokio_tungstenite::connect_async(format!("{}/v1/connect", relay.dial)).await.unwrap();
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
        let outcome = match handshake::accept(side, device, hello, now_ms()) {
            // Not one that may control the host: an A2A proxy it takes A2A
            // calls from gets in as the agent's `A2aOnly` admission will.
            Err(HandshakeError::NotAccepted(_)) if st.chain.may_call_a2a(device, HOST).is_ok() => accept_a2a(st, device, hello, server.as_bytes()),
            outcome => outcome,
        };
        match outcome {
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

/// The admission an agent gives an A2A proxy (`docs/a2a.md` §7.3, its
/// `A2aOnly` channel) — [`handshake::accept`]'s checks in its order, with
/// `may_call_a2a` where a device that controls has `may_control` — standing
/// in for the agent's own until it has one. The hello's chain entries were
/// taken already (by the `accept` that said "not accepted").
fn accept_a2a(st: &mut AgentState, device: &str, message: &[u8], server: &[u8]) -> Result<Accepted, HandshakeError> {
    let (_, host_dh) = keys(2);
    let now = now_ms();
    let binding = Binding {
        account: ACCOUNT.into(),
        device: device.into(),
        machine: HOST.into(),
    };
    let mut responder = noise::Responder::with_prologue(&host_dh, &binding.prologue()?)?;
    let hello = responder.read_hello(message)?;
    let body = HelloBody::decode(&hello.payload)?;
    st.chain
        .authorize_device(device, &hello.initiator_static)
        .map_err(|_| HandshakeError::Unauthorized)?;
    st.chain
        .may_call_a2a(device, HOST)
        .map_err(|e| HandshakeError::NotAccepted(e.to_string()))?;
    let reply = |status: ReplyStatus, chain: &ChainState| ReplyBody {
        status,
        chain_seq: chain.seq(),
        chain_head: chain.head(),
        need_from: if status == ReplyStatus::NeedEntries { chain.seq() + 1 } else { 0 },
        server: server.to_vec(),
        entries: chain
            .entries()
            .iter()
            .skip(usize::try_from(body.chain_seq + 1).unwrap_or(usize::MAX))
            .take(MAX_HELLO_ENTRIES)
            .cloned()
            .collect(),
    };
    let policy = RekeyPolicy::default();
    match st.chain.freshness(device, &st.attestations, now, FRESHNESS_MAX_AGE_MS)? {
        Freshness::Fresh => {}
        Freshness::Behind { .. } => {
            let from = st.chain.seq() + 1;
            let (reply, _) = responder.write_reply(&reply(ReplyStatus::NeedEntries, &st.chain).encode()?, policy, now)?;
            return Ok(Accepted::NeedEntries { reply, from, applied: 0 });
        }
        Freshness::Stale { newest_ms } => {
            let (reply, _) = responder.write_reply(&reply(ReplyStatus::Stale, &st.chain).encode()?, policy, now)?;
            return Ok(Accepted::Stale {
                reply,
                newest_ms,
                applied: 0,
            });
        }
    }
    st.guard.check(device, &hello, now)?;
    let (reply, transport) = responder.write_reply(&reply(ReplyStatus::Accepted, &st.chain).encode()?, policy, now)?;
    Ok(Accepted::Open {
        reply,
        transport: Box::new(transport),
        hello: body,
        applied: 0,
    })
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
            tls: None,
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
    /// The device under test is in the account already (else it is to join).
    pub proxy_in_chain: bool,
    /// The relay is `wss://`, behind a TLS front.
    pub tls: bool,
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
            proxy_in_chain: true,
            tls: false,
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

    pub async fn with(mut config: StandConfig) -> Stand {
        let account = if config.proxy_in_chain { Account::new() } else { Account::hosts_only() };
        let relay = if config.tls {
            config.options.relays.tls = Some(test_tls());
            TestRelay::start_tls("rly-1", config.timing).await
        } else {
            TestRelay::start("rly-1", config.timing).await
        };
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
