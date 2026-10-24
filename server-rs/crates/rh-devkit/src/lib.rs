//! A headless remote-hand device, for tests and the local end-to-end harness.
//!
//! It does what the browser does, with the same protocol: sign up with an
//! invite, register its keys, create the account's genesis on chain v3
//! (generating `AK` and dropping it: [`Device::create_genesis3`]), issue an
//! enrollment code, check the agent's proof before adding the host with its
//! first machine key epoch ([`Device::enroll_host3`]), mail it that
//! key, vouch for its chain head, and open end-to-end channels to the machine
//! over the hub's `/v1/client` socket — rpc, mux and term streams through the
//! agent's loopback bridge ([`rh_proto::bridge`]).
//!
//! The chain v2 flows ([`Device::create_genesis`], [`Device::complete_enrollment`],
//! [`Device::add_and_deliver`], [`Device::remove_and_rotate`] and
//! [`Device::create_genesis_with_recovery`]) are deprecated: they stand for
//! an account made before chain v3, in tests about such accounts. Every new
//! account is v3 from its genesis (`docs/local-aggregator.md` §3.5).

use std::time::Duration;

use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use rand::RngCore;
use rh_proto::bridge::{self, Kind, MessageReader, RequestHead, ResponseHead};
use rh_proto::chain::{genesis_draft, Action, Attestation, ChainClock, ChainState, DeviceInfo, Entry, MachineInfo, Signer};
use rh_proto::channel::Session;
use rh_proto::driver::{Channel, Stream, StreamEvent};
use rh_proto::frame::Frame;
use rh_proto::handshake;
use rh_proto::identity::{fingerprint, EnrollCode};
use rh_proto::keys::DhKeypair;
use rh_proto::mux::{Role, StreamClass};
use rh_proto::noise::RekeyPolicy;
use rh_proto::wrap::{self, Purpose, SealContext};
use serde_json::{json, Value};

pub mod agent;
pub mod keys;
pub mod passkey;
pub mod relays;
pub mod streams;
pub mod traffic;

// What a device does at a hub lives in `rh-client`, shared with the local
// install; these are its names as the tests and the e2e harness know them.
pub use rh_client::link::{next_json, Link, CONNECT_PATH};
pub(crate) use rh_client::{err, key32};
pub use rh_client::{follow, hex, now_ms, unhex, HubUrl, Result};

/// A signed-in device with its keys and its verified view of the chain.
pub struct Device {
    pub hub: HubUrl,
    http: reqwest::Client,
    pub account: String,
    pub token: String,
    pub id: String,
    sign: SigningKey,
    dh: DhKeypair,
    chain: Option<ChainState>,
    clock: ChainClock,
    /// The user key epochs this device holds (a test device keeps them in memory).
    pub user_keys: std::collections::BTreeMap<u32, [u8; 32]>,
}

/// Register a fresh device key pair with a login session, then trade the
/// device's signature for a device session: `(device id, device token)`.
pub(crate) async fn register_device(
    http: &reqwest::Client,
    hub: &HubUrl,
    account: &str,
    login_token: &str,
    sign: &SigningKey,
    dh: &DhKeypair,
) -> Result<(String, String)> {
    let v: Value = post(
        http,
        &format!("{}/v1/devices", hub.http),
        Some(login_token),
        &json!({"sign_key": hex(&sign.verifying_key().to_bytes()), "dh_key": hex(&dh.public())}),
    )
    .await?;
    let id = v["device"].as_str().ok_or("no device id")?.to_string();
    let token = device_session(http, hub, account, &id, sign).await?;
    Ok((id, token))
}

/// A device session for `device`: the hub's challenge signed with the
/// device's key (`HubLogin` of role device-session). No bearer: the
/// signature is what signs it in.
pub async fn device_session(http: &reqwest::Client, hub: &HubUrl, account: &str, device: &str, sign: &SigningKey) -> Result<String> {
    let minted = rh_client::session::mint(http, hub, account, device, sign, now_ms()).await?;
    Ok(minted.token.to_string())
}

impl Device {
    /// Sign up with an invite and register a fresh device key pair.
    pub async fn sign_up(hub: &HubUrl, invite: &str, email: &str, password: &str) -> Result<Device> {
        let http = reqwest::Client::builder().no_proxy().build().map_err(err("http client"))?;
        let v: Value = post(
            &http,
            &format!("{}/v1/signup", hub.http),
            None,
            &json!({"invite": invite, "email": email, "password": password}),
        )
        .await?;
        let account = v["account"].as_str().ok_or("no account")?.to_string();
        let login = v["token"].as_str().ok_or("no token")?.to_string();
        let sign = SigningKey::generate(&mut OsRng);
        let dh = DhKeypair::generate(&mut OsRng);
        let (id, token) = register_device(&http, hub, &account, &login, &sign, &dh).await?;
        Ok(Device {
            hub: hub.clone(),
            http,
            account,
            token,
            id,
            sign,
            dh,
            chain: None,
            clock: ChainClock::live(now_ms(), 0),
            user_keys: Default::default(),
        })
    }

    /// Register a fresh device key pair with a login session had some other
    /// way (a passkey's, say): signed in with its device session, no chain
    /// view yet.
    pub async fn from_login(hub: &HubUrl, account: &str, login_token: &str) -> Result<Device> {
        let http = reqwest::Client::builder().no_proxy().build().map_err(err("http client"))?;
        let sign = SigningKey::generate(&mut OsRng);
        let dh = DhKeypair::generate(&mut OsRng);
        let (id, token) = register_device(&http, hub, account, login_token, &sign, &dh).await?;
        Ok(Device {
            hub: hub.clone(),
            http,
            account: account.into(),
            token,
            id,
            sign,
            dh,
            chain: None,
            clock: ChainClock::live(now_ms(), 0),
            user_keys: Default::default(),
        })
    }

    pub fn chain(&self) -> Option<&ChainState> {
        self.chain.as_ref()
    }

    /// A device with given keys and id — a host's own identity (chain v3:
    /// a host controls other hosts as a device): signed in with a device
    /// session, its chain view fetched.
    pub async fn from_keys(hub: &HubUrl, account: &str, id: &str, sign: SigningKey, dh: DhKeypair) -> Result<Device> {
        let http = reqwest::Client::builder().no_proxy().build().map_err(err("http client"))?;
        let token = device_session(&http, hub, account, id, &sign).await?;
        let mut device = Device {
            hub: hub.clone(),
            http,
            account: account.into(),
            token,
            id: id.into(),
            sign,
            dh,
            chain: None,
            clock: ChainClock::live(now_ms(), 0),
            user_keys: Default::default(),
        };
        device.sync_chain().await?;
        Ok(device)
    }

    /// This device's keys in a v3 entry, with `flags`.
    pub fn device_v3(&self, host: bool, admin: bool) -> rh_proto::chain::DeviceV3 {
        rh_proto::chain::DeviceV3 {
            id: self.id.clone(),
            sign_key: self.sign.verifying_key().to_bytes(),
            dh_key: self.dh.public(),
            flags: rh_proto::chain::Flags { host, admin },
        }
    }

    /// This device as the chain names it (a full device).
    pub fn device_info(&self) -> DeviceInfo {
        self.info()
    }

    fn info(&self) -> DeviceInfo {
        DeviceInfo {
            id: self.id.clone(),
            sign_key: self.sign.verifying_key().to_bytes(),
            dh_key: self.dh.public(),
            full: true,
        }
    }

    /// Create the account's chain as accounts were made before chain v3: a
    /// fresh `AK` signs a v2 genesis naming this device, and is dropped.
    /// Returns the account-key fingerprint machines pin. (The web app makes
    /// every new account on v3: [`Device::create_genesis3`].)
    #[deprecated(note = "chain v2: every new account is v3 from its genesis — use Device::create_genesis3(false); this stands only for an account made before chain v3")]
    pub async fn create_genesis(&mut self) -> Result<[u8; 16]> {
        let ak = SigningKey::generate(&mut OsRng);
        let entry = genesis_draft(&self.account, ak.verifying_key().to_bytes(), self.info(), None, now_ms()).sign(&ak);
        drop(ak);
        self.push(std::slice::from_ref(&entry)).await?;
        self.clock.set_now(now_ms());
        self.chain = Some(ChainState::verify(&[entry], None, &mut self.clock).map_err(err("genesis"))?);
        Ok(self.fingerprint().expect("just created"))
    }

    pub fn fingerprint(&self) -> Option<[u8; 16]> {
        self.chain.as_ref().map(|c| fingerprint(&c.account_key()))
    }

    async fn push(&self, entries: &[Entry]) -> Result<u64> {
        let v: Value = post(
            &self.http,
            &format!("{}/v1/chain", self.hub.http),
            Some(&self.token),
            &json!({"entries": entries.iter().map(|e| hex(&e.encode())).collect::<Vec<_>>()}),
        )
        .await?;
        v["seq"].as_u64().ok_or_else(|| "no seq".into())
    }

    /// Sign and append one entry.
    pub async fn append(&mut self, action: Action) -> Result<u64> {
        let chain = self.chain.as_mut().ok_or("no chain yet")?;
        let entry = chain.draft(Signer::Device(self.id.clone()), action, now_ms()).sign(&self.sign);
        self.clock.set_now(now_ms());
        let mut next = chain.clone();
        next.apply(&entry, &mut self.clock).map_err(err("own entry"))?;
        let seq = self.push(&[entry]).await?;
        self.chain = Some(next);
        Ok(seq)
    }

    /// Sign and append several entries as one batch (the hub takes it whole
    /// or not at all).
    pub async fn append_batch(&mut self, actions: Vec<Action>) -> Result<u64> {
        let mut next = self.chain.clone().ok_or("no chain yet")?;
        self.clock.set_now(now_ms());
        let mut entries = Vec::new();
        for action in actions {
            let entry = next.draft(Signer::Device(self.id.clone()), action, now_ms()).sign(&self.sign);
            next.apply(&entry, &mut self.clock).map_err(err("own entry"))?;
            entries.push(entry);
        }
        let seq = self.push(&entries).await?;
        self.chain = Some(next);
        Ok(seq)
    }

    /// Sign and append entries as one batch without applying them locally
    /// first (a test pushing what the hub must refuse). The local view is
    /// not changed.
    pub async fn push_raw(&mut self, actions: Vec<Action>) -> Result<u64> {
        let mut next = self.chain.clone().ok_or("no chain yet")?;
        let mut clock = ChainClock::history();
        let mut entries = Vec::new();
        for action in actions {
            let entry = next.draft(Signer::Device(self.id.clone()), action, now_ms()).sign(&self.sign);
            let _ = next.apply(&entry, &mut clock);
            entries.push(entry);
        }
        self.push(&entries).await
    }

    /// Issue an enrollment code for a new machine and announce its token.
    pub async fn new_enrollment(&self) -> Result<EnrollCode> {
        let code = EnrollCode::generate(self.fingerprint().ok_or("no chain yet")?, &mut OsRng);
        let _: Value = post(
            &self.http,
            &format!("{}/v1/enrollments", self.hub.http),
            Some(&self.token),
            &json!({"device": self.id, "token": hex(&code.token())}),
        )
        .await?;
        Ok(code)
    }

    /// Wait for the agent's proof, check it, and add the machine to the chain
    /// with exactly the keys the proof covers — as chain v2's `AddMachine`,
    /// which a v3 chain refuses ([`Device::complete_enrollment_v3`] is what
    /// the web writes there).
    #[deprecated(note = "chain v2's AddMachine: on a chain v3 account use Device::complete_enrollment_v3 (the host joins with its first machine key epoch in the same batch); this stands only for an account made before chain v3")]
    pub async fn complete_enrollment(&mut self, code: &EnrollCode, timeout: Duration) -> Result<MachineInfo> {
        let machine = self.await_proof(code, timeout).await?;
        self.append(Action::AddMachine(machine.clone())).await?;
        Ok(machine)
    }

    /// Wait for the agent's proof of `code` and check it: the machine with
    /// exactly the keys the proof covers.
    async fn await_proof(&self, code: &EnrollCode, timeout: Duration) -> Result<MachineInfo> {
        let deadline = tokio::time::Instant::now() + timeout;
        let url = format!("{}/v1/enrollments/{}", self.hub.http, hex(&code.token()));
        let v = loop {
            let v: Value = get(&self.http, &url, &self.token).await?;
            if v["state"] == "proved" {
                break v;
            }
            if v["state"] == "expired" || tokio::time::Instant::now() > deadline {
                return Err(format!("enrollment not proved: {}", v["state"]));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        };
        let machine = MachineInfo {
            id: v["machine"].as_str().ok_or("no machine")?.to_string(),
            sign_key: key32(v["sign_key"].as_str().ok_or("no sign key")?)?,
            dh_key: key32(v["dh_key"].as_str().ok_or("no dh key")?)?,
        };
        let proof = key32(v["proof"].as_str().ok_or("no proof")?)?;
        if !code.verify_proof(&self.account, &machine.id, &machine.sign_key, &machine.dh_key, &proof) {
            return Err("the agent's enrollment proof does not verify: refusing to add the machine".into());
        }
        Ok(machine)
    }

    /// [`Device::complete_enrollment`] for chain v3: add the host as a device
    /// that runs AI (and manages the account if `admin`), with its first
    /// machine key epoch in the same batch — the bare batch, nothing checked
    /// or done around it ([`Device::enroll_host3`] is what the web does).
    pub async fn complete_enrollment_v3(&mut self, code: &EnrollCode, admin: bool, mk: &[u8; 32], timeout: Duration) -> Result<MachineInfo> {
        let machine = self.await_proof(code, timeout).await?;
        let device = rh_proto::chain::DeviceV3 {
            id: machine.id.clone(),
            sign_key: machine.sign_key,
            dh_key: machine.dh_key,
            flags: rh_proto::chain::Flags { host: true, admin },
        };
        let commit = rh_proto::chain::KeyCommits {
            user: None,
            machines: vec![(
                machine.id.clone(),
                rh_proto::chain::KeyCommit {
                    epoch: 0,
                    hash: rh_proto::chain::machine_key_commit(&self.account, &machine.id, 0, mk),
                },
            )],
        };
        self.append_batch(vec![Action::AddDevice3(device), Action::KeyEpochs(commit)]).await?;
        Ok(machine)
    }

    /// Seal `mk` (epoch `epoch`) to a machine and leave it in its mailbox.
    pub async fn mail_machine_key(&self, machine: &str, epoch: u32, mk: &[u8; 32]) -> Result<()> {
        let chain = self.chain.as_ref().ok_or("no chain yet")?;
        let recipient = chain.machine(machine).ok_or("no such machine in the chain")?.info.dh_key;
        let ctx = SealContext {
            purpose: Purpose::MachineKey,
            account: self.account.clone(),
            sender: self.id.clone(),
            recipient: machine.into(),
            epoch,
        };
        let sealed = wrap::seal_to(&self.dh, &recipient, &ctx, mk, &mut OsRng).map_err(err("seal"))?;
        let _: Value = post(
            &self.http,
            &format!("{}/v1/mailbox", self.hub.http),
            Some(&self.token),
            &json!({"recipient": machine, "sender": self.id, "purpose": Purpose::MachineKey as u8, "epoch": epoch, "sealed": hex(&sealed.encode())}),
        )
        .await?;
        Ok(())
    }

    /// A snapshot page of `stream` from the hub (`GET /v1/streams/{stream}/pages/{hash}`).
    /// Sign this device in again (a new device session), e.g. after the
    /// hub restarted or the session expired.
    pub async fn renew_session(&mut self) -> Result<()> {
        self.token = device_session(&self.http, &self.hub, &self.account, &self.id, &self.sign).await?;
        Ok(())
    }

    pub async fn fetch_page(&self, stream: &rh_proto::record::StreamId, hash: &[u8; 32]) -> Result<Vec<u8>> {
        self.pages().fetch(stream, hash).await
    }

    /// Where this device fetches a stream's snapshot pages.
    pub fn pages(&self) -> rh_client::streams::Pages<'_> {
        rh_client::streams::Pages {
            http: &self.http,
            hub: &self.hub,
            token: &self.token,
        }
    }

    /// This device's signed statement of the head it sees now.
    pub fn attestation(&self) -> Result<Attestation> {
        Ok(Attestation::create(
            self.chain.as_ref().ok_or("no chain yet")?,
            &self.id,
            &self.sign,
            now_ms(),
        ))
    }

    pub async fn me(&self) -> Result<Value> {
        get(&self.http, &format!("{}/v1/me", self.hub.http), &self.token).await
    }

    /// `GET /v1/relays`: the relays this account may use, its set and limit,
    /// and its machines' round trips.
    pub async fn relays(&self) -> Result<Value> {
        get(&self.http, &format!("{}/v1/relays", self.hub.http), &self.token).await
    }

    /// `POST /v1/relays/tickets`: `{tickets: [{relay, origin, ticket, expires_ms}], errors}`.
    pub async fn relay_tickets(&self, relays: &[&str]) -> Result<Value> {
        post(
            &self.http,
            &format!("{}/v1/relays/tickets", self.hub.http),
            Some(&self.token),
            &json!({"relays": relays}),
        )
        .await
    }

    /// `PATCH /v1/relays/prefs {standby}`: whether the account keeps a standby
    /// relay (「备用中继」) → `{standby}`.
    pub async fn set_relay_standby(&self, on: bool) -> Result<Value> {
        Ok(rh_client::http::send(
            &self.http,
            reqwest::Method::PATCH,
            &format!("{}/v1/relays/prefs", self.hub.http),
            Some(&self.token),
            Some(&json!({"standby": on})),
        )
        .await?)
    }

    /// Sign in on `/v1/client`.
    pub async fn connect(&self) -> Result<Link> {
        rh_client::link::connect(&self.hub, &self.account, &self.id, &self.sign, &self.token).await
    }

    /// Sign in on a relay (`origin`, e.g. `wss://sin-1.relay.example`) with a
    /// ticket from [`Device::relay_tickets`]: a link channels open on like the
    /// Hub's. The relay's `welcome` and `presence` come first.
    pub async fn connect_relay(&self, origin: &str, ticket: &str) -> Result<Link> {
        rh_client::link::connect_relay(origin, &self.account, &self.id, &self.sign, ticket).await
    }

    /// Open an end-to-end channel to `machine` over `link`.
    pub async fn open_channel(&mut self, link: &Link, machine: &str) -> Result<Channel> {
        Ok(self.open_channel_info(link, machine).await?.channel)
    }

    /// [`Device::open_channel`], with what the handshake settled: whether the
    /// agent speaks sequenced frames (`paths/1`) and offers direct paths
    /// (`direct/1`), and the channel's handshake hash (a direct path binds to it).
    pub async fn open_channel_info(&mut self, link: &Link, machine: &str) -> Result<OpenedChannel> {
        let chain = self.chain.as_ref().ok_or("no chain yet")?;
        let (ch, mut inbound) = link.reserve();
        link.send(Frame::Open { ch, peer: machine.into() })?;
        let client = client_info();
        let (initiator, hello) = handshake::start(chain, &self.dh, &self.id, machine, now_ms(), client.as_bytes()).map_err(err("handshake"))?;
        link.send(Frame::Data { ch, payload: hello })?;
        let reply = match tokio::time::timeout(Duration::from_secs(10), inbound.recv()).await {
            Ok(Some(Frame::Data { payload, .. })) => payload,
            Ok(Some(Frame::Close { code, reason, .. })) => return Err(format!("channel refused: {code} {reason}")),
            Ok(other) => return Err(format!("unexpected answer to a handshake: {:?}", other.map(|f| f.to_raw().map(|r| r.ty)))),
            Err(_) => return Err("the handshake timed out".into()),
        };
        // Entries the agent has and this device lacks are merged into its view.
        let mut view = chain.clone();
        let result = handshake::finish(initiator, &mut view, &mut self.clock, &reply, RekeyPolicy::default(), now_ms());
        if view.seq() > chain.seq() {
            self.chain = Some(view);
        }
        let (transport, body) = result.map_err(err("handshake"))?;
        let features: Vec<String> = serde_json::from_slice::<Value>(&body.server)
            .ok()
            .and_then(|v| {
                v["features"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|f| f.as_str().map(String::from)).collect())
            })
            .unwrap_or_default();
        let paths = features.iter().any(|f| f == rh_proto::path::CLIENT_FEATURE);
        let direct = paths && features.iter().any(|f| f == "direct/1");
        let relay_hash = transport.handshake_hash();
        let session = if paths {
            let now_us = now_ms() * 1000;
            Session::with_paths(
                ch,
                transport,
                Role::Initiator,
                rh_proto::channel::PathOptions { probe_relay: true },
                now_us,
            )
        } else {
            Session::new(ch, transport, Role::Initiator)
        };
        let (channel, _incoming) = Channel::spawn(session, inbound, link.outbound());
        Ok(OpenedChannel {
            channel,
            machine: machine.to_string(),
            relay_hash,
            paths,
            direct,
        })
    }

    /// Try one direct path for `opened` (attempt `id`): a str0m DataChannel
    /// offered over the channel's signaling stream, with Noise on top. On
    /// success the channel's frames go that way until it fails.
    #[cfg(feature = "direct")]
    pub async fn direct(
        &mut self,
        opened: &OpenedChannel,
        signaling: &mut rh_proto::direct::Signaling,
        id: u32,
        opts: rh_proto::rtc::RtcOptions,
        timeout: Duration,
    ) -> Result<rh_proto::direct::Attached> {
        let chain = self.chain.as_ref().ok_or("no chain yet")?.clone();
        let client = client_info();
        rh_proto::direct::attempt(
            signaling,
            id,
            &opened.channel,
            rh_proto::direct::DeviceDirect {
                chain: &chain,
                clock: &mut self.clock,
                key: &self.dh,
                binding: rh_proto::noise::Binding {
                    account: self.account.clone(),
                    device: self.id.clone(),
                    machine: opened.machine.clone(),
                },
                relay_hash: opened.relay_hash,
                client: client.as_bytes(),
                opts,
                timeout,
            },
        )
        .await
        .map_err(|e| e.to_string())
    }
}

/// A channel and what its handshake settled.
pub struct OpenedChannel {
    pub channel: Channel,
    pub machine: String,
    /// The channel's Noise handshake hash.
    pub relay_hash: [u8; 32],
    /// The agent speaks sequenced frames.
    pub paths: bool,
    /// The agent offers direct paths.
    pub direct: bool,
}

pub(crate) async fn post(http: &reqwest::Client, url: &str, token: Option<&str>, body: &Value) -> Result<Value> {
    Ok(rh_client::http::post(http, url, token, body).await?)
}

pub(crate) async fn get(http: &reqwest::Client, url: &str, token: &str) -> Result<Value> {
    Ok(rh_client::http::get(http, url, token).await?)
}

/// An rpc answer.
#[derive(Debug)]
pub struct RpcResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RpcResponse {
    pub fn json(&self) -> Result<Value> {
        serde_json::from_slice(&self.body).map_err(err("json body"))
    }
}

/// One HTTP request to the agent's `/api/*` through `channel`.
pub async fn rpc(channel: &Channel, method: &str, path: &str, body: Option<&Value>) -> Result<RpcResponse> {
    rpc_class(channel, StreamClass::Rpc, method, path, body).await
}

/// A `GET` over a low-priority `blob` stream (files, pictures).
pub async fn blob_get(channel: &Channel, path: &str) -> Result<RpcResponse> {
    rpc_class(channel, StreamClass::Blob, "GET", path, None).await
}

async fn rpc_class(channel: &Channel, class: StreamClass, method: &str, path: &str, body: Option<&Value>) -> Result<RpcResponse> {
    let mut headers = vec![("accept".to_string(), "application/json".to_string())];
    if body.is_some() {
        headers.push(("content-type".into(), "application/json".into()));
    }
    let head = RequestHead {
        method: method.into(),
        path: path.into(),
        headers,
    };
    let mut stream = channel.open(class, &head.encode().map_err(err("head"))?).await.map_err(err("open"))?;
    if let Some(body) = body {
        stream
            .send(&bridge::message(Kind::Binary, body.to_string().as_bytes()))
            .await
            .map_err(err("send"))?;
    }
    stream.send(&bridge::message(Kind::End, b"")).await.map_err(err("send"))?;
    let mut reader = MessageReader::new();
    let mut head: Option<ResponseHead> = None;
    let mut out = Vec::new();
    loop {
        match stream.recv().await {
            Some(StreamEvent::Data(d)) => {
                reader.push(&d);
                while let Some((kind, payload)) = reader.next_message().map_err(err("bridge"))? {
                    match kind {
                        Kind::Head => head = Some(ResponseHead::decode(&payload).map_err(err("head"))?),
                        Kind::Binary | Kind::Text => out.extend_from_slice(&payload),
                        Kind::End => {}
                    }
                }
            }
            Some(StreamEvent::Closed(0)) | None if head.is_some() => break,
            Some(StreamEvent::Closed(code)) => return Err(format!("rpc {method} {path} failed with code {code}")),
            None => return Err("the channel ended".into()),
        }
    }
    let head = head.expect("checked");
    Ok(RpcResponse {
        status: head.status,
        headers: head.headers,
        body: out,
    })
}

/// A websocket on the agent (`mux` or `term` class), message by message.
pub struct WsStream {
    stream: Stream,
    reader: MessageReader,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsMessage {
    Text(String),
    Binary(Vec<u8>),
    Closed(u16),
}

impl WsStream {
    pub async fn open(channel: &Channel, class: StreamClass, path: &str) -> Result<WsStream> {
        let head = RequestHead::get(path);
        let stream = channel.open(class, &head.encode().map_err(err("head"))?).await.map_err(err("open"))?;
        Ok(WsStream {
            stream,
            reader: MessageReader::new(),
        })
    }

    pub async fn send_text(&self, text: &str) -> Result<()> {
        self.stream.send(&bridge::message(Kind::Text, text.as_bytes())).await.map_err(err("send"))
    }

    pub async fn recv(&mut self) -> Result<WsMessage> {
        loop {
            if let Some((kind, payload)) = self.reader.next_message().map_err(err("bridge"))? {
                return Ok(match kind {
                    Kind::Text => WsMessage::Text(String::from_utf8_lossy(&payload).into_owned()),
                    Kind::Binary => WsMessage::Binary(payload),
                    // The bridge's answer to the upgrade: 101 is the socket's
                    // `open`; anything else is a refused handshake.
                    Kind::Head => match ResponseHead::decode(&payload) {
                        Ok(head) if head.status == 101 => continue,
                        Ok(head) => WsMessage::Closed(head.status),
                        Err(_) => WsMessage::Closed(bridge::CLOSE_BAD_REQUEST),
                    },
                    Kind::End => continue,
                });
            }
            match self.stream.recv().await {
                Some(StreamEvent::Data(d)) => self.reader.push(&d),
                Some(StreamEvent::Closed(code)) => return Ok(WsMessage::Closed(code)),
                None => return Ok(WsMessage::Closed(bridge::CLOSE_UPSTREAM)),
            }
        }
    }

    pub fn close(&self) {
        self.stream.close(0)
    }
}

/// A fresh random 32-byte key (a machine key for the mailbox).
pub fn random_key() -> [u8; 32] {
    let mut k = [0u8; 32];
    OsRng.fill_bytes(&mut k);
    k
}

/// What the headless device's hello says it speaks: sequenced frames, and
/// streams named per key epoch (so an agent it talks to names them so).
pub fn client_info() -> String {
    format!("rh-devkit {} {}", rh_proto::path::CLIENT_FEATURE, rh_proto::stream::STREAMS_FEATURE)
}
