//! Renewing this device's identity key against fake hubs
//! (`docs/host-identity-rotation.md` §3.2–§3.8): the request, the staging,
//! the sign-in confirmation, the order of sign-ins with a staged seed,
//! promotion only on a verified entry, a renewal by somebody else, and a
//! crash between any two steps.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use rand::RngCore;
use rh_proto::chain::{
    genesis_draft, Action, Assurance, Attestation, ChainClock, ChainState, DeviceInfo, DeviceV3, Entry, Flags, HostRotation, KeyCommit,
    KeyCommits, Signer,
};
use rh_proto::frame::Frame;
use rh_proto::identity::{fingerprint, MachineIdentity};
use rh_proto::keys::DhKeypair;
use rh_proto::sig::{HubLogin, LoginRole};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;
use zeroize::Zeroizing;

use super::hub::{hex, now_ms, unhex32};
use super::link::{self, Shared};
use super::rotation::{self, Verdict};
use super::state::AgentState;
use super::{FileSeedStore, SeedStore, StagedSeed, StoredSeed};
use crate::sessions::Paths;

fn account_key() -> MachineIdentity {
    MachineIdentity::from_seed(&[1; 32])
}

/// d-1: the account's admin.
fn admin() -> MachineIdentity {
    MachineIdentity::from_seed(&[2; 32])
}

/// m-1: this machine, as enrolled.
fn machine() -> MachineIdentity {
    MachineIdentity::from_seed(&[4; 32])
}

/// Whoever holds a copy of m-1's seed renews it to these.
fn thief() -> MachineIdentity {
    MachineIdentity::from_seed(&[66; 32])
}

fn key_epoch(host: &str, epoch: u32) -> Action {
    Action::KeyEpochs(KeyCommits {
        user: None,
        machines: vec![(host.into(), KeyCommit { epoch, hash: [epoch as u8; 32] })],
    })
}

fn unhex64(text: &str) -> [u8; 64] {
    link::unhex(text).and_then(|b| <[u8; 64]>::try_from(b).ok()).expect("64 bytes of hex")
}

/// A v3 account — genesis (d-1, an admin) → m-1 (this machine, runs AI) →
/// its first machine key — as the hub has it, and this instance with m-1's
/// seed on disk.
struct Account {
    hub: ChainState,
    entries: Vec<Entry>,
    paths: Paths,
    _dir: tempfile::TempDir,
}

impl Account {
    fn new() -> Self {
        let (ak, d1, m) = (account_key(), admin(), machine());
        let genesis = genesis_draft(
            "a-1",
            ak.sign_public(),
            DeviceInfo {
                id: "d-1".into(),
                sign_key: d1.sign_public(),
                dh_key: d1.dh_public(),
                full: true,
            },
            None,
            1_000,
        )
        .sign(ak.signing_key());
        let hub = ChainState::verify(std::slice::from_ref(&genesis), None, &mut ChainClock::history()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "rotation");
        let mut a = Account {
            hub,
            entries: vec![genesis],
            paths,
            _dir: dir,
        };
        a.push(Action::AddDevice3(DeviceV3 {
            id: "m-1".into(),
            sign_key: m.sign_public(),
            dh_key: m.dh_public(),
            flags: Flags { host: true, admin: false },
        }));
        a.push(key_epoch("m-1", 0));
        let uuid = crate::identity::ensure(&a.paths).unwrap().uuid;
        FileSeedStore::for_instance(&a.paths)
            .create(&StoredSeed {
                uuid,
                seed: Zeroizing::new([4; 32]),
            })
            .unwrap();
        a
    }

    /// d-1 writes `action`.
    fn push(&mut self, action: Action) -> Entry {
        let e = self
            .hub
            .draft(Signer::Device("d-1".into()), action, 2_000 + self.hub.seq())
            .sign(admin().signing_key());
        self.hub.apply(&e, &mut ChainClock::history()).unwrap();
        self.entries.push(e.clone());
        e
    }

    /// m-1's state having taken the chain so far, its head vouched for by d-1 now.
    fn state(&self, hub: &str) -> AgentState {
        let mut st = AgentState::enrolling(hub, "a-1", "m-1", fingerprint(&account_key().sign_public()), now_ms());
        assert!(st.absorb(&self.entries, now_ms()).error.is_none());
        assert!(st.attest(Attestation::create(&self.hub, "d-1", admin().signing_key(), now_ms())));
        st
    }

    /// The link's shared state for this instance (saved as well), its restarts counted in `restarts`.
    fn shared(&self, hub: &str, restarts: &Arc<AtomicUsize>) -> Arc<Shared> {
        let st = self.state(hub);
        st.save(&self.paths).unwrap();
        let count = restarts.clone();
        let restart: rotation::Restart = Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
        });
        link::test_shared(&self.paths, st, machine(), super::streams::Handle::detached(), restart)
    }

    /// `POST /rh/rotate`'s body about the hub's head now.
    fn request(&self, nonce: [u8; 16], eph: &DhKeypair, assurance: u8) -> Vec<u8> {
        json!({"nonce": hex(&nonce), "eph": hex(&eph.public()), "chainSeq": self.hub.seq(), "chainHead": hex(&self.hub.head()), "assurance": assurance})
            .to_string()
            .into_bytes()
    }

    /// d-1 writes the renewal `answer` (to `/rh/rotate`) proves, and m-1's next machine key.
    fn renew(&mut self, answer: &Value, nonce: [u8; 16], assurance: Assurance) -> Vec<Entry> {
        self.renew_at(answer, nonce, assurance, 1)
    }

    /// [`Account::renew`], m-1's next machine key being `epoch`.
    fn renew_at(&mut self, answer: &Value, nonce: [u8; 16], assurance: Assurance, epoch: u32) -> Vec<Entry> {
        let r = HostRotation {
            host: "m-1".into(),
            sign_key: unhex32(answer["signKey"].as_str().unwrap()).unwrap(),
            dh_key: unhex32(answer["dhKey"].as_str().unwrap()).unwrap(),
            staged_seq: answer["stagedSeq"].as_u64().unwrap(),
            staged_head: unhex32(answer["stagedHead"].as_str().unwrap()).unwrap(),
            nonce,
            assurance,
            old_sig: unhex64(answer["oldSig"].as_str().unwrap()),
            new_sig: unhex64(answer["newSig"].as_str().unwrap()),
        };
        vec![self.push(Action::RotateHostKey(r)), self.push(key_epoch("m-1", epoch))]
    }

    /// Somebody holding m-1's seed renews it to `to` (signed by the old key and `to`), d-1 writing it.
    fn renew_to(&mut self, to: &MachineIdentity) -> Vec<Entry> {
        let old = machine();
        let mut r = HostRotation {
            host: "m-1".into(),
            sign_key: to.sign_public(),
            dh_key: to.dh_public(),
            staged_seq: self.hub.seq(),
            staged_head: self.hub.head(),
            nonce: [9; 16],
            assurance: Assurance::Auto,
            old_sig: [0; 64],
            new_sig: [0; 64],
        };
        let body = r.body("a-1", &old.sign_public(), &old.dh_public());
        r.old_sig = rh_proto::sig::sign(old.signing_key(), rh_proto::sig::HOST_ROTATE, &body);
        r.new_sig = rh_proto::sig::sign(to.signing_key(), rh_proto::sig::HOST_ROTATE, &body);
        vec![self.push(Action::RotateHostKey(r)), self.push(key_epoch("m-1", 1))]
    }

    /// Stage `seed` for this instance as `POST /rh/rotate` would, at the hub's head now.
    fn stage(&self, seed: [u8; 32], nonce: [u8; 16]) -> MachineIdentity {
        let uuid = crate::identity::ensure(&self.paths).unwrap().uuid;
        let staged = StagedSeed {
            uuid,
            seed: Zeroizing::new(seed),
            nonce,
            chain_seq: self.hub.seq(),
            chain_head: self.hub.head(),
            assurance: Assurance::Auto,
            created_ms: now_ms(),
            accepted_ms: Some(now_ms()),
        };
        FileSeedStore::for_instance(&self.paths).stage(&staged).unwrap();
        staged.identity()
    }

    /// What `POST /rh/rotate` would have answered for `new` staged with
    /// `nonce` at the hub's head now (call it before the renewal is written).
    fn staged_answer(&self, new: &MachineIdentity, nonce: [u8; 16]) -> Value {
        self.staged_answer_from(&machine(), new, nonce)
    }

    /// [`Account::staged_answer`] of m-1 running with `old`.
    fn staged_answer_from(&self, old: &MachineIdentity, new: &MachineIdentity, nonce: [u8; 16]) -> Value {
        let body = rh_proto::chain::host_rotate_body(
            "a-1",
            "m-1",
            self.hub.seq(),
            &self.hub.head(),
            &old.sign_public(),
            &old.dh_public(),
            &new.sign_public(),
            &new.dh_public(),
            &nonce,
            Assurance::Auto,
        );
        json!({
            "signKey": hex(&new.sign_public()),
            "dhKey": hex(&new.dh_public()),
            "stagedSeq": self.hub.seq(),
            "stagedHead": hex(&self.hub.head()),
            "oldSig": hex(&rh_proto::sig::sign(old.signing_key(), rh_proto::sig::HOST_ROTATE, &body)),
            "newSig": hex(&rh_proto::sig::sign(new.signing_key(), rh_proto::sig::HOST_ROTATE, &body)),
        })
    }

    /// The seed in use on disk, as keys.
    fn seed_in_use(&self) -> MachineIdentity {
        MachineIdentity::from_seed(&FileSeedStore::for_instance(&self.paths).load().unwrap().unwrap().seed)
    }

    fn staged(&self) -> Option<StagedSeed> {
        FileSeedStore::for_instance(&self.paths).staged().unwrap()
    }

    /// The nonces of every staging, oldest first.
    fn nonces(&self) -> Vec<[u8; 16]> {
        FileSeedStore::for_instance(&self.paths).stagings().unwrap().iter().map(|s| s.nonce).collect()
    }
}

/// How the fake hub answers a `rotation_stage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StageAnswer {
    Staged,
    Refused(&'static str),
    /// No answer at all (lost on the way).
    Silent,
}

/// What the fake hub does.
#[derive(Debug, Clone)]
struct Script {
    /// What its `welcome` offers.
    features: Vec<&'static str>,
    /// The sign keys it signs in; any other gets `error denied` and 4401…
    accept: Vec<[u8; 32]>,
    /// …but these get `error rotated`…
    rotated: Vec<[u8; 32]>,
    /// …and these `error unavailable` and close 1011: the hub's own trouble
    /// (its database), whatever the key.
    unavailable: Vec<[u8; 32]>,
    stage: StageAnswer,
    /// What `POST /v1/rotations/{host}/disputes` answers, in turn (then 200).
    dispute_answers: Vec<u16>,
}

impl Script {
    fn rotating(accept: Vec<[u8; 32]>) -> Self {
        Self {
            features: vec!["rotate/1"],
            accept,
            rotated: Vec::new(),
            unavailable: Vec::new(),
            stage: StageAnswer::Staged,
            dispute_answers: Vec::new(),
        }
    }
}

/// One sign-in the fake hub saw.
#[derive(Debug, Clone)]
struct Login {
    nonce: [u8; 32],
    time_ms: u64,
    sig: [u8; 64],
    welcomed: bool,
}

impl Login {
    /// Whether `key` signed it.
    fn by(&self, key: &MachineIdentity, url: &str) -> bool {
        HubLogin {
            role: LoginRole::Agent,
            account: "a-1".into(),
            id: "m-1".into(),
            hub: url.into(),
            nonce: self.nonce,
            time_ms: self.time_ms,
        }
        .verify(&key.sign_public(), &self.sig)
    }
}

/// A hub on loopback that signs m-1 in as its [`Script`] says, serves the
/// chain it is given on `chain_sub`, answers `rotation_stage`, keeps every
/// text the agent sends, and passes on what a test sends.
struct FakeHub {
    url: String,
    script: Arc<Mutex<Script>>,
    logins: Arc<Mutex<Vec<Login>>>,
    heard: Arc<Mutex<Vec<Value>>>,
    current: Arc<Mutex<Option<mpsc::UnboundedSender<Message>>>>,
    /// The chain it serves.
    chain: Arc<Mutex<Vec<Entry>>>,
    /// Every dispute statement delivered to it: the path and the body.
    disputes: Arc<Mutex<Vec<(String, Value)>>>,
}

impl FakeHub {
    async fn start(script: Script, chain: Vec<Entry>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let hub = FakeHub {
            url: url.clone(),
            script: Arc::new(Mutex::new(script)),
            logins: Arc::default(),
            heard: Arc::default(),
            current: Arc::default(),
            chain: Arc::new(Mutex::new(chain)),
            disputes: Arc::default(),
        };
        let (script, logins, heard, current) = (hub.script.clone(), hub.logins.clone(), hub.heard.clone(), hub.current.clone());
        let disputes = hub.disputes.clone();
        let chain = hub.chain.clone();
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                // The hub's plain HTTP on the same origin: a dispute statement.
                if starts_with_post(&tcp).await {
                    tokio::spawn(serve_dispute(tcp, script.clone(), disputes.clone()));
                    continue;
                }
                let Ok(ws) = tokio_tungstenite::accept_async(tcp).await else { continue };
                let (mut sink, mut source) = ws.split();
                let mut nonce = [0u8; 32];
                rand::rngs::OsRng.fill_bytes(&mut nonce);
                let challenge = json!({"t": "challenge", "nonce": hex(&nonce), "hub": url, "version": 1});
                if sink.send(Message::Text(challenge.to_string().into())).await.is_err() {
                    continue;
                }
                let Some(Ok(Message::Text(text))) = source.next().await else { continue };
                let login: Value = serde_json::from_str(text.as_str()).unwrap();
                let mut seen = Login {
                    nonce,
                    time_ms: login["time_ms"].as_u64().unwrap(),
                    sig: unhex64(login["sig"].as_str().unwrap()),
                    welcomed: false,
                };
                let now = script.lock().unwrap().clone();
                let signed = |key: &[u8; 32]| {
                    HubLogin {
                        role: LoginRole::Agent,
                        account: "a-1".into(),
                        id: "m-1".into(),
                        hub: url.clone(),
                        nonce,
                        time_ms: seen.time_ms,
                    }
                    .verify(key, &seen.sig)
                };
                if now.unavailable.iter().any(signed) {
                    logins.lock().unwrap().push(seen);
                    let error = json!({"t": "error", "code": "unavailable", "message": "the hub cannot sign anyone in now"});
                    let _ = sink.send(Message::Text(error.to_string().into())).await;
                    let _ = sink
                        .send(Message::Close(Some(CloseFrame {
                            code: 1011.into(),
                            reason: "unavailable".into(),
                        })))
                        .await;
                    continue;
                }
                if !now.accept.iter().any(signed) {
                    let code = if now.rotated.iter().any(signed) { "rotated" } else { "denied" };
                    logins.lock().unwrap().push(seen);
                    let error = json!({"t": "error", "code": code, "message": "sign-in refused"});
                    let _ = sink.send(Message::Text(error.to_string().into())).await;
                    if code == "rotated" {
                        // One `chain_sub` on this socket is
                        // answered with the chain it serves, then it closes.
                        if let Ok(Some(Ok(Message::Text(text)))) = tokio::time::timeout(Duration::from_secs(2), source.next()).await {
                            let v: Value = serde_json::from_str(text.as_str()).unwrap_or_default();
                            heard.lock().unwrap().push(v.clone());
                            if v["t"] == "chain_sub" {
                                let from = v["from"].as_u64().unwrap_or(0) as usize;
                                let entries: Vec<Vec<u8>> = chain.lock().unwrap().iter().skip(from).map(Entry::encode).collect();
                                if !entries.is_empty() {
                                    let frame = Frame::Chain { ch: 0, entries }.encode().unwrap();
                                    let _ = sink.send(Message::Binary(frame.into())).await;
                                }
                            }
                        }
                    }
                    let _ = sink
                        .send(Message::Close(Some(CloseFrame {
                            code: 4401.into(),
                            reason: "sign-in refused".into(),
                        })))
                        .await;
                    continue;
                }
                seen.welcomed = true;
                logins.lock().unwrap().push(seen);
                let welcome = json!({"t": "welcome", "features": now.features});
                if sink.send(Message::Text(welcome.to_string().into())).await.is_err() {
                    continue;
                }
                let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
                *current.lock().unwrap() = Some(tx);
                loop {
                    tokio::select! {
                        m = rx.recv() => {
                            let Some(m) = m else { break };
                            let close = matches!(m, Message::Close(_));
                            if sink.send(m).await.is_err() || close {
                                break;
                            }
                        }
                        m = source.next() => match m {
                            Some(Ok(Message::Text(text))) => {
                                let Ok(v) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                                heard.lock().unwrap().push(v.clone());
                                let answer = match v["t"].as_str() {
                                    Some("chain_sub") => {
                                        let from = v["from"].as_u64().unwrap_or(0) as usize;
                                        let entries: Vec<Vec<u8>> = chain.lock().unwrap().iter().skip(from).map(Entry::encode).collect();
                                        if entries.is_empty() {
                                            continue;
                                        }
                                        Message::Binary(Frame::Chain { ch: 0, entries }.encode().unwrap().into())
                                    }
                                    Some("rotation_stage") => {
                                        let nonce = link::unhex(v["nonce"].as_str().unwrap()).unwrap();
                                        let hash = hex(&Sha256::digest(&nonce));
                                        let reply = match script.lock().unwrap().stage {
                                            StageAnswer::Staged => json!({"t": "rotation_staged", "nonce_hash": hash, "ready_ms": 123}),
                                            StageAnswer::Refused(reason) => json!({"t": "rotation_refused", "nonce_hash": hash, "reason": reason}),
                                            StageAnswer::Silent => continue,
                                        };
                                        Message::Text(reply.to_string().into())
                                    }
                                    _ => continue,
                                };
                                if sink.send(answer).await.is_err() {
                                    break;
                                }
                            }
                            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                            Some(Ok(_)) => {}
                        },
                    }
                }
                *current.lock().unwrap() = None;
            }
        });
        hub
    }

    /// Send `m` to the connection signed in now, if there is one.
    fn send(&self, m: Message) {
        if let Some(tx) = self.current.lock().unwrap().clone() {
            let _ = tx.send(m);
        }
    }

    fn send_json(&self, v: Value) {
        self.send(Message::Text(v.to_string().into()));
    }

    fn send_entries(&self, entries: &[Entry]) {
        let frame = Frame::Chain {
            ch: 0,
            entries: entries.iter().map(Entry::encode).collect(),
        };
        self.send(Message::Binary(frame.encode().unwrap().into()));
    }

    /// The dispute statements delivered so far.
    fn disputes(&self) -> Vec<(String, Value)> {
        self.disputes.lock().unwrap().clone()
    }

    /// What the agent sent with `t`.
    fn heard(&self, t: &str) -> Vec<Value> {
        self.heard.lock().unwrap().iter().filter(|v| v["t"] == t).cloned().collect()
    }

    fn logins(&self) -> Vec<Login> {
        self.logins.lock().unwrap().clone()
    }
}

/// Whether the connection opens with `POST ` (an HTTP request, not a
/// websocket's `GET`).
async fn starts_with_post(tcp: &tokio::net::TcpStream) -> bool {
    let mut first = [0u8; 5];
    for _ in 0..50 {
        match tcp.peek(&mut first).await {
            Ok(5) => return &first == b"POST ",
            Ok(0) | Err(_) => return false,
            Ok(_) => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    }
    false
}

/// `POST /v1/rotations/{host}/disputes`, as the hub answers it: the next
/// scripted status (then 200), the body kept.
async fn serve_dispute(mut tcp: tokio::net::TcpStream, script: Arc<Mutex<Script>>, disputes: Arc<Mutex<Vec<(String, Value)>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    let head_end = loop {
        let n = tcp.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let path = head.split_whitespace().nth(1).unwrap_or_default().to_string();
    let len = head
        .lines()
        .find_map(|l| {
            let lower = l.to_ascii_lowercase();
            lower.strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    while buf.len() < head_end + len {
        let n = tcp.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body: Value = serde_json::from_slice(&buf[head_end..head_end + len]).unwrap_or_default();
    disputes.lock().unwrap().push((path, body));
    let status = {
        let mut script = script.lock().unwrap();
        if script.dispute_answers.is_empty() {
            200
        } else {
            script.dispute_answers.remove(0)
        }
    };
    let text = if status == 200 { r#"{"ok":true}"# } else { r#"{"error":"unavailable"}"# };
    let answer = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
    let _ = tcp.write_all(answer.as_bytes()).await;
    let _ = tcp.shutdown().await;
}

/// Wait until `f` holds (15 s at most).
async fn until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Run `shared`'s link and wait until it is signed in with the key in use.
async fn signed_in(shared: &Arc<Shared>) -> tokio::task::JoinHandle<()> {
    let task = tokio::spawn(link::run(shared.clone()));
    until("the sign-in", || {
        shared.rotation.session().is_some_and(|s| !s.staged_key) && shared.hub_text.lock().unwrap().is_some()
    })
    .await;
    task
}

/// `POST /rh/rotate` as d-1 asks it.
async fn rotate(shared: &Arc<Shared>, body: &[u8]) -> (u16, Value) {
    rotation::rotate(shared, "d-1", body).await
}

/// A new seed is staged beside the one in use (never over
/// it), the hub stages the proof over this connection, and the answer carries
/// both keys' signatures over the R1 proof and the DH confirmation MAC; the
/// same nonce gets the same answer, a new one replaces the staged seed, and a
/// staging the hub refuses leaves nothing staged.
#[tokio::test]
async fn a_renewal_request_stages_a_new_seed_and_proves_it() {
    let a = Account::new();
    let hub = FakeHub::start(Script::rotating(vec![machine().sign_public()]), a.entries.clone()).await;
    let restarts = Arc::new(AtomicUsize::new(0));
    let shared = a.shared(&hub.url, &restarts);
    let task = signed_in(&shared).await;
    let eph = DhKeypair::from_secret([5; 32]);
    let (status, answer) = rotate(&shared, &a.request([7; 16], &eph, 0)).await;
    assert_eq!(status, 200, "{answer}");
    let new_sign = unhex32(answer["signKey"].as_str().unwrap()).unwrap();
    let new_dh = unhex32(answer["dhKey"].as_str().unwrap()).unwrap();
    assert_eq!((answer["stagedSeq"].as_u64(), answer["readyMs"].as_u64()), (Some(a.hub.seq()), Some(123)));
    assert_eq!(answer["stagedHead"].as_str(), Some(hex(&a.hub.head()).as_str()));
    // Both keys signed the R1 proof; the MAC proves the new DH key.
    let old = machine();
    let r = HostRotation {
        host: "m-1".into(),
        sign_key: new_sign,
        dh_key: new_dh,
        staged_seq: a.hub.seq(),
        staged_head: a.hub.head(),
        nonce: [7; 16],
        assurance: Assurance::Auto,
        old_sig: unhex64(answer["oldSig"].as_str().unwrap()),
        new_sig: unhex64(answer["newSig"].as_str().unwrap()),
    };
    assert!(r.verify("a-1", &old.sign_public(), &old.dh_public()));
    let body = r.body("a-1", &old.sign_public(), &old.dh_public());
    let mac = unhex32(answer["mac"].as_str().unwrap()).unwrap();
    assert!(rh_proto::chain::check_rotate_confirm(&eph, &new_dh, &body, &mac));
    // Staged beside the seed in use, 0600; the seed in use untouched.
    let staged = a.staged().expect("staged");
    assert_eq!(staged.identity().sign_public(), new_sign);
    assert_eq!(a.seed_in_use().sign_public(), old.sign_public(), "machine-key.json is not touched");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let next = FileSeedStore::for_instance(&a.paths).staging_path(&[7; 16]);
        assert_eq!(std::fs::metadata(next).unwrap().permissions().mode() & 0o777, 0o600);
    }
    // The hub heard the staging, snake_case, with the same proof.
    let stages = hub.heard("rotation_stage");
    assert_eq!(stages.len(), 1);
    assert_eq!(stages[0]["nonce"], hex(&[7; 16]));
    assert_eq!(stages[0]["sign_key"], hex(&new_sign));
    assert_eq!(stages[0]["dh_key"], hex(&new_dh));
    assert_eq!(stages[0]["old_sig"], answer["oldSig"]);
    assert_eq!(stages[0]["new_sig"], answer["newSig"]);
    assert_eq!((stages[0]["staged_seq"].as_u64(), stages[0]["assurance"].as_u64()), (Some(a.hub.seq()), Some(0)));
    // The same nonce: the same answer, and nothing staged again.
    let (status, again) = rotate(&shared, &a.request([7; 16], &eph, 0)).await;
    assert_eq!((status, &again), (200, &answer));
    assert_eq!(hub.heard("rotation_stage").len(), 1);
    // The same nonce with another view or assurance is not the same request.
    assert_eq!(rotate(&shared, &a.request([7; 16], &eph, 1)).await.0, 400);
    // A new nonce: a new seed, staged beside the one the hub took (which it
    // may still commit: kept).
    let (status, other) = rotate(&shared, &a.request([8; 16], &eph, 0)).await;
    assert_eq!(status, 200);
    assert_ne!(other["signKey"], answer["signKey"]);
    assert_eq!(a.staged().unwrap().nonce, [8; 16]);
    assert_eq!(a.nonces(), vec![[7; 16], [8; 16]]);
    assert_eq!(hub.heard("rotation_stage").len(), 2);
    // A staging the hub refuses: 409 hub_refused {reason}, not kept; the
    // ones it took stay.
    hub.script.lock().unwrap().stage = StageAnswer::Refused("conflict");
    let (status, refused) = rotate(&shared, &a.request([9; 16], &eph, 0)).await;
    assert_eq!((status, refused["error"].as_str(), refused["reason"].as_str()), (409, Some("hub_refused"), Some("conflict")));
    assert_eq!(a.nonces(), vec![[7; 16], [8; 16]]);
    // One the hub did not answer in time (504) is kept for the attempt's ten
    // minutes, unanswered; its late answer makes it one the hub took.
    hub.script.lock().unwrap().stage = StageAnswer::Silent;
    let (status, lost) = rotate(&shared, &a.request([10; 16], &eph, 0)).await;
    assert_eq!((status, lost["error"].as_str()), (504, Some("hub_timeout")));
    assert_eq!(a.nonces(), vec![[7; 16], [8; 16], [10; 16]]);
    assert!(a.staged().unwrap().accepted_ms.is_none());
    hub.send_json(json!({"t": "rotation_staged", "nonce_hash": hex(&Sha256::digest([10u8; 16])), "ready_ms": 1}));
    until("the late answer", || a.staged().is_some_and(|s| s.accepted_ms.is_some())).await;
    hub.script.lock().unwrap().stage = StageAnswer::Staged;
    assert_eq!(rotate(&shared, &a.request([11; 16], &eph, 0)).await.0, 200);
    assert_eq!(a.nonces(), vec![[7; 16], [8; 16], [10; 16], [11; 16]]);
    assert_eq!(a.seed_in_use().sign_public(), old.sign_public());
    assert_eq!(restarts.load(Ordering::SeqCst), 0);
    shared.cancel.cancel();
    let _ = task.await;
}

/// The checks before anything is staged: a view not on this device's chain
/// (409 `not_on_chain`), a device that may not control this machine (412
/// `stale` `lapsed`), one that is not an admin (403 `limited_device`), an
/// admin whose view nobody vouched for (412 `stale`), a request that does
/// not parse (400) — and, with high assurance, the code it shows here for
/// ten minutes (`link.rotation`). The order is the bridge's
/// ([`super::bridge::refusal`]): whether the device may control this machine
/// at all, then whether it is an admin, then the twelve-hour view — and only
/// then what the chain says about the request (R6 re-review).
#[tokio::test]
async fn a_renewal_request_is_checked_and_a_high_assurance_one_shows_its_code() {
    let mut a = Account::new();
    // h-2 runs AI too and may control m-1, but is no admin.
    let h2 = MachineIdentity::from_seed(&[20; 32]);
    a.push(Action::AddDevice3(DeviceV3 {
        id: "h-2".into(),
        sign_key: h2.sign_public(),
        dh_key: h2.dh_public(),
        flags: Flags { host: true, admin: false },
    }));
    a.push(key_epoch("h-2", 0));
    a.push(Action::SetAccept {
        host: "m-1".into(),
        from: rh_proto::chain::Accept::Devices(vec!["h-2".into()]),
    });
    let hub = FakeHub::start(Script::rotating(vec![machine().sign_public()]), a.entries.clone()).await;
    let shared = a.shared(&hub.url, &Arc::default());
    let task = signed_in(&shared).await;
    let eph = DhKeypair::from_secret([5; 32]);
    let off_chain = json!({"nonce": hex(&[7; 16]), "eph": hex(&eph.public()), "chainSeq": a.hub.seq(), "chainHead": hex(&[3; 32]), "assurance": 0});
    let (status, body) = rotate(&shared, off_chain.to_string().as_bytes()).await;
    assert_eq!((status, body["error"].as_str()), (409, Some("not_on_chain")));
    let ahead = json!({"nonce": hex(&[7; 16]), "eph": hex(&eph.public()), "chainSeq": a.hub.seq() + 1, "chainHead": hex(&a.hub.head()), "assurance": 0});
    assert_eq!(rotate(&shared, ahead.to_string().as_bytes()).await.1["error"], "not_on_chain");
    for bad in [&b"nope"[..], br#"{"nonce":"00"}"#, br#"{"nonce":"07070707070707070707070707070707","eph":"00","chainSeq":1,"chainHead":"00","assurance":2}"#] {
        assert_eq!(rotate(&shared, bad).await.0, 400);
    }
    let refused = |answer: &(u16, Value)| (answer.0, answer.1["error"].as_str().map(str::to_string), answer.1["reason"].as_str().map(str::to_string));
    let lapsed = (412, Some("stale".to_string()), Some("lapsed".to_string()));
    let limited = (403, Some("limited_device".to_string()), None);
    let stale = (412, Some("stale".to_string()), Some("stale".to_string()));
    // Not a device that may control this machine: refused first, whatever else.
    let off_chain_too = json!({"nonce": hex(&[7; 16]), "eph": hex(&eph.public()), "chainSeq": a.hub.seq(), "chainHead": hex(&[3; 32]), "assurance": 0});
    assert_eq!(refused(&rotation::rotate(&shared, "d-9", off_chain_too.to_string().as_bytes()).await), lapsed, "not of the account");
    // May control it, no admin: 403 — before the chain view is judged.
    assert_eq!(refused(&rotation::rotate(&shared, "h-2", off_chain_too.to_string().as_bytes()).await), limited, "not an admin");
    let vouched = std::mem::take(&mut shared.state.lock().unwrap().attestations);
    assert_eq!(refused(&rotation::rotate(&shared, "d-9", &a.request([7; 16], &eph, 0)).await), lapsed, "lapsed before stale");
    assert_eq!(refused(&rotation::rotate(&shared, "h-2", &a.request([7; 16], &eph, 0)).await), limited, "no admin before stale");
    // The admin, its view vouched for by nobody: 412 stale — before the chain says anything.
    assert_eq!(refused(&rotate(&shared, &a.request([7; 16], &eph, 0)).await), stale);
    assert_eq!(refused(&rotate(&shared, off_chain_too.to_string().as_bytes()).await), stale, "stale before not_on_chain");
    shared.state.lock().unwrap().attestations = vouched;
    assert!(a.staged().is_none() && hub.heard("rotation_stage").is_empty(), "nothing staged on a refusal");
    // High assurance: the code is the R1 SAS of the proof, shown for ten minutes.
    let (status, answer) = rotate(&shared, &a.request([7; 16], &eph, 1)).await;
    assert_eq!(status, 200, "{answer}");
    let old = machine();
    let body = rh_proto::chain::host_rotate_body(
        "a-1",
        "m-1",
        a.hub.seq(),
        &a.hub.head(),
        &old.sign_public(),
        &old.dh_public(),
        &unhex32(answer["signKey"].as_str().unwrap()).unwrap(),
        &unhex32(answer["dhKey"].as_str().unwrap()).unwrap(),
        &[7; 16],
        Assurance::OnDevice,
    );
    let st = AgentState::load(&a.paths).unwrap().unwrap();
    let shown = rotation::status(&a.paths, Some(&st), now_ms());
    assert_eq!(shown.state, "staged");
    assert_eq!(shown.code.as_deref(), Some(rh_proto::chain::rotate_sas(&body).as_str()));
    assert!(shown.code_expires_ms.is_some_and(|t| t <= now_ms() + rotation::CODE_FOR_MS));
    let later = rotation::status(&a.paths, Some(&st), now_ms() + rotation::CODE_FOR_MS + 1);
    assert_eq!((later.state.as_str(), later.code), ("staged", None), "ten minutes, then no code");
    shared.cancel.cancel();
    let _ = task.await;
}

/// §5.6 「线上兼容」: on a connection whose `welcome` does not offer
/// `rotate/1` (an older hub closes on a message it does not know), the
/// request is refused 409 `hub_not_ready` and nothing is staged or sent; the
/// same with no hub connection at all.
#[tokio::test]
async fn a_hub_that_does_not_offer_rotate_gets_no_staging() {
    let a = Account::new();
    let mut script = Script::rotating(vec![machine().sign_public()]);
    script.features.clear();
    let hub = FakeHub::start(script, a.entries.clone()).await;
    let shared = a.shared(&hub.url, &Arc::default());
    let eph = DhKeypair::from_secret([5; 32]);
    let (status, body) = rotate(&shared, &a.request([7; 16], &eph, 0)).await;
    assert_eq!((status, body["error"].as_str()), (409, Some("hub_not_ready")), "no connection");
    let task = signed_in(&shared).await;
    let (status, body) = rotate(&shared, &a.request([7; 16], &eph, 0)).await;
    assert_eq!((status, body["error"].as_str()), (409, Some("hub_not_ready")), "{body}");
    assert!(a.staged().is_none());
    // `rotation_pending` from such a hub is not answered either.
    hub.send_json(json!({"t": "rotation_pending", "sign_key": hex(&[1; 32]), "dh_key": hex(&[2; 32])}));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(hub.heard("rotation_ack").is_empty() && hub.heard("rotation_unknown").is_empty());
    assert!(hub.heard("rotation_stage").is_empty());
    // It still declares what it can do.
    let info = hub.heard("agent_info");
    assert!(info[0]["features"].as_array().unwrap().iter().any(|f| f == "rotate/1"), "{info:?}");
    shared.cancel.cancel();
    let _ = task.await;
}

/// §3.3: a sign-in with the old key during the window is asked to confirm
/// the staging; this agent answers with the NEW key's R1 rotate-ack over
/// THIS connection's challenge nonce (verified here with rh-proto), and
/// `rotation_unknown` for a key it does not hold.
#[tokio::test]
async fn rotation_pending_is_confirmed_with_the_new_key_over_this_connections_nonce() {
    let a = Account::new();
    let hub = FakeHub::start(Script::rotating(vec![machine().sign_public()]), a.entries.clone()).await;
    let shared = a.shared(&hub.url, &Arc::default());
    let task = signed_in(&shared).await;
    let eph = DhKeypair::from_secret([5; 32]);
    let (status, answer) = rotate(&shared, &a.request([7; 16], &eph, 0)).await;
    assert_eq!(status, 200);
    let new_sign = unhex32(answer["signKey"].as_str().unwrap()).unwrap();
    let pending = json!({
        "t": "rotation_pending",
        "sign_key": answer["signKey"],
        "dh_key": answer["dhKey"],
        "staged_seq": answer["stagedSeq"],
        "staged_head": answer["stagedHead"],
        "nonce_hash": hex(&Sha256::digest([7u8; 16])),
        "assurance": 0,
        "body_hash": hex(&[0; 32]),
    });
    hub.send_json(pending.clone());
    until("the ack", || !hub.heard("rotation_ack").is_empty()).await;
    let ack = hub.heard("rotation_ack").remove(0);
    assert_eq!(ack["sign_key"], answer["signKey"]);
    let login = hub.logins().last().cloned().unwrap();
    let old = machine();
    let body = rh_proto::chain::host_rotate_body(
        "a-1",
        "m-1",
        a.hub.seq(),
        &a.hub.head(),
        &old.sign_public(),
        &old.dh_public(),
        &new_sign,
        &unhex32(answer["dhKey"].as_str().unwrap()).unwrap(),
        &[7; 16],
        Assurance::Auto,
    );
    let sig = unhex64(ack["sig"].as_str().unwrap());
    assert!(rh_proto::chain::verify_rotate_ack(&new_sign, "a-1", "m-1", &body, &login.nonce, &sig), "over this connection's nonce");
    assert!(!rh_proto::chain::verify_rotate_ack(&new_sign, "a-1", "m-1", &body, &[0; 32], &sig));
    // A key it does not hold.
    let mut other = pending;
    other["sign_key"] = json!(hex(&thief().sign_public()));
    hub.send_json(other);
    until("rotation_unknown", || !hub.heard("rotation_unknown").is_empty()).await;
    assert_eq!(hub.heard("rotation_unknown")[0]["sign_key"], json!(hex(&thief().sign_public())));
    assert_eq!(hub.heard("rotation_ack").len(), 1);
    shared.cancel.cancel();
    let _ = task.await;
}

/// §3.5: the chain entry naming exactly the staged keys (then
/// 4005): the agent signs in with the new key, sees the entry on its verified
/// chain, renames the staged seed into place and asks for its link to be
/// restarted — and only in that order.
#[tokio::test]
async fn a_committed_renewal_is_taken_after_signing_in_with_the_new_key() {
    let mut a = Account::new();
    let old = machine();
    let hub = FakeHub::start(Script::rotating(vec![old.sign_public()]), a.entries.clone()).await;
    let restarts = Arc::new(AtomicUsize::new(0));
    let shared = a.shared(&hub.url, &restarts);
    let task = signed_in(&shared).await;
    let eph = DhKeypair::from_secret([5; 32]);
    let (status, answer) = rotate(&shared, &a.request([7; 16], &eph, 0)).await;
    assert_eq!(status, 200);
    let new = a.staged().unwrap().identity();
    // The hub commits: the new key signs in from now on, the old is retired.
    {
        let mut script = hub.script.lock().unwrap();
        script.accept = vec![new.sign_public()];
        script.rotated = vec![old.sign_public()];
    }
    let entries = a.renew(&answer, [7; 16], Assurance::Auto);
    hub.send_entries(&entries);
    hub.send_json(json!({"t": "error", "code": "rotated", "message": "renewed", "fp": "x"}));
    until("the restart", || restarts.load(Ordering::SeqCst) == 1).await;
    tokio::time::timeout(Duration::from_secs(5), task).await.expect("the link ends").unwrap();
    // Promoted: the new seed in use, nothing staged, noted for the settings.
    assert_eq!(a.seed_in_use().sign_public(), new.sign_public());
    assert!(a.staged().is_none());
    let logins = hub.logins();
    let last = logins.last().unwrap();
    assert!(last.by(&new, &hub.url) && last.welcomed, "it signed in with the new key before taking it");
    assert!(logins[0].by(&old, &hub.url));
    let st = AgentState::load(&a.paths).unwrap().unwrap();
    assert!(st.rotation.renewed_ms.is_some());
    assert!(st.rotation.push_renew, "the push keys are renewed with it at the restart (R6 review)");
    assert_eq!(st.rotation.previous_fingerprint.as_deref(), Some(super::control::machine_fingerprint(&old).as_str()));
    let shown = rotation::status(&a.paths, Some(&st), now_ms());
    assert_eq!(shown.state, "renewed");
    assert!(st.alarms.is_empty(), "no alarm for its own renewal");
    // R6 re-review: its own renewal is disputed by nobody — no statement
    // signed, kept or delivered, and the promoted key has none to make.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(hub.disputes().is_empty(), "{:?}", hub.disputes());
    assert!(super::dispute::kept(&a.paths).is_empty());
    assert!(st.rotation.foreign_ms.is_none() && st.rotation.evidence_ms.is_none());
    assert!(super::dispute::statement(&st, &new, now_ms()).is_none());
}

/// Final review of the agent's renewal: the state — with the chain that
/// names the renewed key — is saved before the staged seed takes the place
/// of the key in use, or the seed is not swapped at all. When that save
/// failed and the seed was swapped all the same, a device renewed once
/// before was left with a state whose chain names its previous keys, and a
/// renewal on it: the next start took its own renewal for one made
/// elsewhere ("replaced elsewhere").
#[tokio::test]
async fn a_renewed_key_is_taken_only_once_the_state_naming_it_is_saved() {
    let mut a = Account::new();
    let hub = "ws://127.0.0.1:9";
    // Renewed once before: m-1 runs with `first`, and the state on disk
    // holds the chain naming it.
    let first = a.stage([44; 32], [1; 16]);
    let answer = a.staged_answer(&first, [1; 16]);
    a.renew(&answer, [1; 16], Assurance::Auto);
    FileSeedStore::for_instance(&a.paths).promote(&[1; 16]).unwrap();
    a.state(hub).save(&a.paths).unwrap();
    assert_eq!(a.seed_in_use().sign_public(), first.sign_public());
    // Renewed again, to `second`: the chain naming it is in memory only.
    let second = a.stage([77; 32], [7; 16]);
    let answer = a.staged_answer_from(&first, &second, [7; 16]);
    a.renew_at(&answer, [7; 16], Assurance::Auto, 2);
    let restarts = Arc::new(AtomicUsize::new(0));
    let count = restarts.clone();
    let restart: rotation::Restart = Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
    });
    let shared = link::test_shared(&a.paths, a.state(hub), MachineIdentity::from_seed(&[44; 32]), super::streams::Handle::detached(), restart);
    // The state cannot be saved (a directory where its file goes); what is
    // on disk stays as it was, as after any failed atomic write.
    let state_file = super::state::state_path(&a.paths);
    let on_disk = std::fs::read(&state_file).unwrap();
    std::fs::remove_file(&state_file).unwrap();
    std::fs::create_dir(&state_file).unwrap();
    let promoted = rotation::promote(&shared, &second);
    std::fs::remove_dir(&state_file).unwrap();
    super::write_private(&state_file, &on_disk).unwrap();
    // The next start does not take the key in use for one renewed away.
    assert!(
        !rotation::seed_retired(&a.paths, &a.seed_in_use()),
        "the next start takes this device's own renewal for one made elsewhere ({promoted:?})"
    );
    assert!(promoted.is_err(), "not taken while the state naming it is not saved");
    assert_eq!(a.seed_in_use().sign_public(), first.sign_public(), "the key in use stays");
    assert_eq!(a.nonces(), vec![[7; 16]], "the renewed key stays staged, to be taken at the next sign-in");
    assert_eq!(restarts.load(Ordering::SeqCst), 0);
    assert!(!shared.state.lock().unwrap().rotation.push_renew, "the push keys are renewed with a key taken only");
    // Once the state saves, it is taken.
    rotation::promote(&shared, &second).unwrap();
    assert_eq!(a.seed_in_use().sign_public(), second.sign_public());
    assert_eq!(restarts.load(Ordering::SeqCst), 1);
    assert!(!rotation::seed_retired(&a.paths, &second));
    let st = AgentState::load(&a.paths).unwrap().unwrap();
    assert_eq!(rotation::named_keys(&st), Some((second.sign_public(), second.dh_public())));
    assert!(st.rotation.push_renew);
}

/// Review of the renewal fixes: when the staged seed cannot take the place of
/// the key in use after the state was saved (its file moved away, say), the
/// key in use stays, and so do its push keys: `push_renew` is back to what it
/// was, in memory and on disk — the push key is not refused (503
/// `push_renewing`) for a renewal not taken.
#[tokio::test]
async fn a_renewed_key_not_put_in_place_leaves_the_push_keys_alone() {
    let mut a = Account::new();
    let new = a.stage([77; 32], [7; 16]);
    let answer = a.staged_answer(&new, [7; 16]);
    a.renew(&answer, [7; 16], Assurance::Auto);
    let restarts = Arc::new(AtomicUsize::new(0));
    let shared = a.shared("ws://127.0.0.1:9", &restarts);
    // After the save, the staged seed's file is moved away: the rename fails.
    let staging = FileSeedStore::for_instance(&a.paths).staging_path(&[7; 16]);
    let aside = staging.with_extension("aside");
    let (from, to) = (staging.clone(), aside.clone());
    rotation::BETWEEN_SAVE_AND_SWAP.with(|b| *b.borrow_mut() = Some(Box::new(move || std::fs::rename(&from, &to).unwrap())));
    let promoted = rotation::promote(&shared, &new);
    assert!(promoted.is_err(), "nothing to put in place");
    assert_eq!(a.seed_in_use().sign_public(), machine().sign_public(), "the key in use stays");
    assert_eq!(restarts.load(Ordering::SeqCst), 0);
    assert!(
        !shared.state.lock().unwrap().rotation.push_renew,
        "the push keys are refused as being renewed for a renewal not taken"
    );
    assert!(!AgentState::load(&a.paths).unwrap().unwrap().rotation.push_renew, "nor at the next start");
    // Put back, it is taken, and the push keys follow it.
    std::fs::rename(&aside, &staging).unwrap();
    rotation::promote(&shared, &new).unwrap();
    assert_eq!(a.seed_in_use().sign_public(), new.sign_public());
    assert!(AgentState::load(&a.paths).unwrap().unwrap().rotation.push_renew);
}

/// R6 re-review: the device role never signs in once this computer's own
/// chain names other keys for it (a renewal not taken yet, or made
/// elsewhere) — even with no staged seed at all — nor once a renewal is
/// committed while the connection with the key in use still looks up.
#[tokio::test]
async fn the_device_role_never_signs_in_with_a_key_renewed_away_or_committed() {
    // Renewed elsewhere, nothing staged.
    let mut a = Account::new();
    let hub = FakeHub::start(Script::rotating(vec![machine().sign_public()]), a.entries.clone()).await;
    let shared = a.shared(&hub.url, &Arc::default());
    assert!(rotation::device_role_may_sign_in(&shared), "nothing renewed, nothing staged");
    let entries = a.renew_to(&thief());
    assert!(shared.state.lock().unwrap().absorb(&entries, now_ms()).error.is_none());
    assert!(a.staged().is_none());
    assert!(!rotation::device_role_may_sign_in(&shared), "renewed away: closed");
    drop(hub);
    // Committed (the hub closed with 4005) while the connection with the key
    // in use still looks up.
    let b = Account::new();
    b.stage([77; 32], [7; 16]);
    let hub = FakeHub::start(Script::rotating(vec![machine().sign_public()]), b.entries.clone()).await;
    let shared = b.shared(&hub.url, &Arc::default());
    let task = signed_in(&shared).await;
    assert!(rotation::device_role_may_sign_in(&shared), "the link vouches for the key in use");
    shared.rotation.renewed_by_hub();
    assert!(!rotation::device_role_may_sign_in(&shared), "committed: the key in use is retired");
    shared.cancel.cancel();
    let _ = task.await;
}

/// R6 re-review: with the account's renewal policy on
/// (`SetRenewalPolicy { always_assured: true }`), a renewal that would not
/// be confirmed on the device is refused before anything is staged (409
/// `assurance_required`) — the chain would never take it; one that would be
/// is staged.
#[tokio::test]
async fn with_the_renewal_policy_on_only_a_renewal_confirmed_on_the_device_is_staged() {
    let mut a = Account::new();
    a.push(Action::SetRenewalPolicy { always_assured: true });
    let hub = FakeHub::start(Script::rotating(vec![machine().sign_public()]), a.entries.clone()).await;
    let shared = a.shared(&hub.url, &Arc::default());
    let task = signed_in(&shared).await;
    let eph = DhKeypair::from_secret([5; 32]);
    let (status, body) = rotate(&shared, &a.request([7; 16], &eph, 0)).await;
    assert_eq!((status, body["error"].as_str()), (409, Some("assurance_required")), "{body}");
    assert!(a.staged().is_none() && hub.heard("rotation_stage").is_empty(), "nothing staged");
    let (status, body) = rotate(&shared, &a.request([8; 16], &eph, 1)).await;
    assert_eq!(status, 200, "{body}");
    assert!(a.staged().is_some());
    shared.cancel.cancel();
    let _ = task.await;
}

/// A 4005 alone (the entry not seen on this connection) also
/// makes the staged key sign in at once — and promote once the chain it
/// then reads from the hub names it.
#[tokio::test]
async fn a_close_4005_signs_in_with_the_staged_key_and_promotes_on_the_chain() {
    let mut a = Account::new();
    let old = machine();
    let new = a.stage([77; 32], [7; 16]);
    let answer = a.staged_answer(&new, [7; 16]);
    let hub = FakeHub::start(Script::rotating(vec![old.sign_public()]), a.entries.clone()).await;
    let restarts = Arc::new(AtomicUsize::new(0));
    let shared = a.shared(&hub.url, &restarts);
    // At start the staged key is tried first and denied: the key in use signs in.
    let task = signed_in(&shared).await;
    // The hub commits the renewal: it serves it from now on, takes only the
    // new key, and closes the old key's connection with 4005.
    let entries = a.renew(&answer, [7; 16], Assurance::Auto);
    hub.chain.lock().unwrap().extend(entries);
    {
        let mut script = hub.script.lock().unwrap();
        script.accept = vec![new.sign_public()];
        script.rotated = vec![old.sign_public()];
    }
    hub.send(Message::Close(Some(CloseFrame {
        code: 4005.into(),
        reason: "rotated".into(),
    })));
    until("the restart", || restarts.load(Ordering::SeqCst) == 1).await;
    tokio::time::timeout(Duration::from_secs(5), task).await.expect("the link ends").unwrap();
    assert_eq!(a.seed_in_use().sign_public(), new.sign_public());
    assert!(a.staged().is_none());
    let logins = hub.logins();
    assert_eq!(logins.len(), 3, "{logins:?}");
    assert!(logins[0].by(&new, &hub.url) && !logins[0].welcomed);
    assert!(logins[1].by(&old, &hub.url) && logins[1].welcomed);
    assert!(logins[2].by(&new, &hub.url) && logins[2].welcomed);
}

/// The hub phase's constraint (§3.8): once the renewal is committed — the
/// chain names the staged key, or the hub closed with 4005 — the key in use
/// never signs in again, even while the hub turns the new one away: a
/// sign-in with a retired key is the hub's evidence against this very device.
#[tokio::test]
async fn after_a_committed_renewal_the_old_key_never_signs_in_again() {
    // Committed on the chain.
    let mut a = Account::new();
    let old = machine();
    let hub = FakeHub::start(Script::rotating(vec![old.sign_public()]), a.entries.clone()).await;
    let shared = a.shared(&hub.url, &Arc::default());
    let task = signed_in(&shared).await;
    let eph = DhKeypair::from_secret([5; 32]);
    let (status, answer) = rotate(&shared, &a.request([7; 16], &eph, 0)).await;
    assert_eq!(status, 200);
    let new = a.staged().unwrap().identity();
    {
        let mut script = hub.script.lock().unwrap();
        script.accept.clear();
        script.rotated = vec![old.sign_public()];
    }
    let before = hub.logins().len();
    let entries = a.renew(&answer, [7; 16], Assurance::Auto);
    hub.send_entries(&entries);
    until("two more sign-ins", || hub.logins().len() >= before + 2).await;
    let after: Vec<Login> = hub.logins()[before..].to_vec();
    assert!(after.iter().all(|l| l.by(&new, &hub.url)), "only the renewed key signs in: {after:?}");
    assert!(!shared.cancel.is_cancelled(), "it keeps trying");
    assert_eq!(a.seed_in_use().sign_public(), old.sign_public(), "nothing promoted without signing in");
    assert!(a.staged().is_some());
    shared.cancel.cancel();
    let _ = task.await;

    // Committed by a 4005 alone (the entry not seen).
    let b = Account::new();
    let new = b.stage([77; 32], [7; 16]);
    let hub = FakeHub::start(Script::rotating(vec![old.sign_public()]), b.entries.clone()).await;
    let shared = b.shared(&hub.url, &Arc::default());
    let task = signed_in(&shared).await;
    {
        let mut script = hub.script.lock().unwrap();
        script.accept.clear();
        script.rotated = vec![old.sign_public()];
    }
    let before = hub.logins().len();
    hub.send(Message::Close(Some(CloseFrame {
        code: 4005.into(),
        reason: "rotated".into(),
    })));
    until("two more sign-ins", || hub.logins().len() >= before + 2).await;
    let after: Vec<Login> = hub.logins()[before..].to_vec();
    assert!(after.iter().all(|l| l.by(&new, &hub.url)), "only the renewed key signs in: {after:?}");
    assert_eq!(b.seed_in_use().sign_public(), old.sign_public());
    shared.cancel.cancel();
    let _ = task.await;
}

/// The hub phase's review: a page whose answer was lost asks again with a
/// new nonce; the hub, still holding the FIRST staging, commits it. The
/// first staged seed is still here: the renewal is this device's own —
/// promoted, the second forgotten, never "replaced elsewhere".
#[tokio::test]
async fn of_two_stagings_the_hub_took_the_first_one_committed_is_promoted() {
    let mut a = Account::new();
    let old = machine();
    let hub = FakeHub::start(Script::rotating(vec![old.sign_public()]), a.entries.clone()).await;
    let restarts = Arc::new(AtomicUsize::new(0));
    let shared = a.shared(&hub.url, &restarts);
    let task = signed_in(&shared).await;
    let eph = DhKeypair::from_secret([5; 32]);
    let (status, first) = rotate(&shared, &a.request([7; 16], &eph, 0)).await;
    assert_eq!(status, 200);
    let (status, second) = rotate(&shared, &a.request([8; 16], &eph, 0)).await;
    assert_eq!(status, 200);
    assert_eq!(a.nonces(), vec![[7; 16], [8; 16]], "both kept");
    let first_key = unhex32(first["signKey"].as_str().unwrap()).unwrap();
    assert_ne!(first["signKey"], second["signKey"]);
    {
        let mut script = hub.script.lock().unwrap();
        script.accept = vec![first_key];
        script.rotated = vec![old.sign_public()];
    }
    let entries = a.renew(&first, [7; 16], Assurance::Auto);
    hub.send_entries(&entries);
    until("the restart", || restarts.load(Ordering::SeqCst) == 1).await;
    tokio::time::timeout(Duration::from_secs(5), task).await.expect("the link ends").unwrap();
    assert_eq!(a.seed_in_use().sign_public(), first_key, "the first staging, promoted");
    assert!(a.nonces().is_empty(), "the other staging goes with the promotion");
    let st = AgentState::load(&a.paths).unwrap().unwrap();
    assert!(st.rotation.foreign_ms.is_none() && st.alarms.is_empty(), "not replaced elsewhere");
    let logins = hub.logins();
    assert!(logins[0].by(&old, &hub.url));
    assert!(logins[1..].iter().all(|l| !l.by(&old, &hub.url)), "the old key never again: {logins:?}");
}

/// A crash with several staged seeds: at start the one the chain names signs
/// in first and is taken; with none named, the newest first, then the
/// older, then the key in use.
#[tokio::test]
async fn at_start_several_staged_seeds_sign_in_named_first_then_newest() {
    // None named: newest, older, then the key in use.
    let a = Account::new();
    let old = machine();
    let older = a.stage([77; 32], [7; 16]);
    std::thread::sleep(Duration::from_millis(5));
    let newer = a.stage([78; 32], [8; 16]);
    let hub = FakeHub::start(Script::rotating(vec![old.sign_public()]), a.entries.clone()).await;
    let shared = a.shared(&hub.url, &Arc::default());
    let task = signed_in(&shared).await;
    let logins = hub.logins();
    assert_eq!(logins.len(), 3, "{logins:?}");
    assert!(logins[0].by(&newer, &hub.url) && logins[1].by(&older, &hub.url) && logins[2].by(&old, &hub.url));
    assert_eq!(a.nonces().len(), 2, "both still staged");
    shared.cancel.cancel();
    let _ = task.await;

    // The chain names the older one (committed while this device was away).
    let mut b = Account::new();
    let older = b.stage([77; 32], [7; 16]);
    let answer = b.staged_answer(&older, [7; 16]);
    std::thread::sleep(Duration::from_millis(5));
    b.stage([78; 32], [8; 16]);
    b.renew(&answer, [7; 16], Assurance::Auto);
    let mut script = Script::rotating(vec![older.sign_public()]);
    script.rotated = vec![old.sign_public()];
    let hub = FakeHub::start(script, b.entries.clone()).await;
    let restarts = Arc::new(AtomicUsize::new(0));
    let shared = b.shared(&hub.url, &restarts);
    let task = tokio::spawn(link::run(shared.clone()));
    until("the restart", || restarts.load(Ordering::SeqCst) == 1).await;
    tokio::time::timeout(Duration::from_secs(5), task).await.expect("the link ends").unwrap();
    let logins = hub.logins();
    assert_eq!(logins.len(), 1, "the named one first: {logins:?}");
    assert!(logins[0].by(&older, &hub.url));
    assert_eq!(b.seed_in_use().sign_public(), older.sign_public());
    assert!(b.nonces().is_empty());
}

/// A crash after staging, before any commit: at start the staged key signs
/// in first; the hub does not take it (`denied`) and the key in use signs in
/// — the seed in use untouched, the staged one kept for a commit to come.
#[tokio::test]
async fn at_start_a_staged_key_signs_in_first_and_the_key_in_use_after_a_denial() {
    let a = Account::new();
    let old = machine();
    let new = a.stage([77; 32], [7; 16]);
    let hub = FakeHub::start(Script::rotating(vec![old.sign_public()]), a.entries.clone()).await;
    let shared = a.shared(&hub.url, &Arc::default());
    let task = signed_in(&shared).await;
    let logins = hub.logins();
    assert_eq!(logins.len(), 2, "{logins:?}");
    assert!(logins[0].by(&new, &hub.url) && !logins[0].welcomed, "the staged key first");
    assert!(logins[1].by(&old, &hub.url) && logins[1].welcomed, "then the key in use");
    assert_eq!(a.seed_in_use().sign_public(), old.sign_public());
    assert!(a.staged().is_some(), "still staged");
    assert!(!shared.cancel.is_cancelled());
    shared.cancel.cancel();
    let _ = task.await;
}

/// A crash after the hub committed, before the rename: the saved chain
/// names the staged key; at start it signs in with it and takes it.
#[tokio::test]
async fn at_start_a_staged_key_the_chain_names_is_taken() {
    let mut a = Account::new();
    let old = machine();
    let new = a.stage([77; 32], [7; 16]);
    let answer = a.staged_answer(&new, [7; 16]);
    a.renew(&answer, [7; 16], Assurance::Auto);
    let mut script = Script::rotating(vec![new.sign_public()]);
    script.rotated = vec![old.sign_public()];
    let hub = FakeHub::start(script, a.entries.clone()).await;
    let restarts = Arc::new(AtomicUsize::new(0));
    let shared = a.shared(&hub.url, &restarts);
    assert_eq!(
        rotation::verdict(&shared.state.lock().unwrap(), &old, std::slice::from_ref(&new), false),
        Verdict::Renewed,
        "the saved chain names the staged key"
    );
    let task = tokio::spawn(link::run(shared.clone()));
    until("the restart", || restarts.load(Ordering::SeqCst) == 1).await;
    tokio::time::timeout(Duration::from_secs(5), task).await.expect("the link ends").unwrap();
    assert_eq!(a.seed_in_use().sign_public(), new.sign_public());
    assert!(a.staged().is_none());
    let logins = hub.logins();
    assert_eq!(logins.len(), 1);
    assert!(logins[0].by(&new, &hub.url));
}

/// Promotion only on a verified entry: a hub that takes the staged key but
/// serves no renewal of this device gets nothing promoted — and since it
/// holds a renewal to that key, the key in use is never tried: the staged
/// key signs in (and reads the chain) again, and the seed in use stays.
#[tokio::test]
async fn a_hub_taking_the_staged_key_without_the_entry_promotes_nothing() {
    let a = Account::new();
    let old = machine();
    let new = a.stage([77; 32], [7; 16]);
    let hub = FakeHub::start(Script::rotating(vec![old.sign_public(), new.sign_public()]), a.entries.clone()).await;
    let restarts = Arc::new(AtomicUsize::new(0));
    let shared = a.shared(&hub.url, &restarts);
    let task = tokio::spawn(link::run(shared.clone()));
    until("the staged key signing in again", || hub.logins().len() >= 2).await;
    let logins = hub.logins();
    assert!(logins.iter().all(|l| l.by(&new, &hub.url) && l.welcomed), "never the key in use: {logins:?}");
    assert_eq!(restarts.load(Ordering::SeqCst), 0);
    assert_eq!(a.seed_in_use().sign_public(), old.sign_public(), "no promotion on the hub's word");
    assert!(a.staged().is_some());
    assert!(!rotation::device_role_may_sign_in(&shared), "the device role waits too");
    shared.cancel.cancel();
    let _ = task.await;
}

/// §3.5 「不拿旧钥误伤自己」: a host asleep through the commit of its own
/// renewal comes back with a stale chain. EVERY sign-in — not only the first
/// after start — tries the staged keys first, so its first sign-in after
/// waking is the staged key's, never the retired one's; and the device role
/// waits for the link meanwhile.
#[tokio::test]
async fn a_host_asleep_through_its_renewal_never_signs_in_with_the_retired_key() {
    let mut a = Account::new();
    let old = machine();
    let new = a.stage([77; 32], [7; 16]);
    let answer = a.staged_answer(&new, [7; 16]);
    let hub = FakeHub::start(Script::rotating(vec![old.sign_public()]), a.entries.clone()).await;
    let restarts = Arc::new(AtomicUsize::new(0));
    let shared = a.shared(&hub.url, &restarts);
    // Awake: the staged key is not known yet (denied), the key in use signs in.
    let task = signed_in(&shared).await;
    assert!(rotation::device_role_may_sign_in(&shared), "the link showed the key in use is current");
    let awake = hub.logins().len();
    assert_eq!(awake, 2);
    // Asleep: the connection goes; the hub commits the renewal meanwhile — its
    // chain serves it, it takes only the new key — and this device heard nothing.
    let entries = a.renew(&answer, [7; 16], Assurance::Auto);
    hub.chain.lock().unwrap().extend(entries);
    {
        let mut script = hub.script.lock().unwrap();
        script.accept = vec![new.sign_public()];
        script.rotated = vec![old.sign_public()];
    }
    hub.send(Message::Close(Some(CloseFrame {
        code: 1012.into(),
        reason: "restart".into(),
    })));
    until("the restart with the renewed key", || restarts.load(Ordering::SeqCst) == 1).await;
    tokio::time::timeout(Duration::from_secs(5), task).await.expect("the link ends").unwrap();
    let after: Vec<Login> = hub.logins()[awake..].to_vec();
    assert!(!after.is_empty() && after.iter().all(|l| l.by(&new, &hub.url)), "never the retired key: {after:?}");
    assert_eq!(a.seed_in_use().sign_public(), new.sign_public());
}

/// The device role's gate (§3.5): open while no staged seed is kept; while
/// one is, only once the link is signed in with the key in use.
#[tokio::test]
async fn the_device_role_waits_for_the_link_while_a_staged_seed_is_kept() {
    let a = Account::new();
    let hub = FakeHub::start(Script::rotating(vec![machine().sign_public()]), a.entries.clone()).await;
    let shared = a.shared(&hub.url, &Arc::default());
    assert!(rotation::device_role_may_sign_in(&shared), "nothing staged");
    a.stage([77; 32], [7; 16]);
    assert!(!rotation::device_role_may_sign_in(&shared), "staged, and the link has not signed in");
    let task = signed_in(&shared).await;
    assert!(rotation::device_role_may_sign_in(&shared), "the hub took the key in use");
    // A laptop asleep: the wall clock jumps, the dead socket still looks open
    // and nothing was heard since — the connection vouches for nothing.
    let woke = now_ms() + rotation::LIVE_MS + 60_000;
    assert!(shared.rotation.session().is_some(), "the connection still looks up");
    assert!(!rotation::device_role_may_sign_in_at(&shared, woke), "stale: no device-role sign-in");
    // Heard from the hub again (its answer to the link's ping): it vouches again.
    shared.rotation.heard();
    assert!(rotation::device_role_may_sign_in(&shared));
    shared.cancel.cancel();
    let _ = task.await;
    assert!(!rotation::device_role_may_sign_in(&shared), "the connection went");
}

/// The cross-review's residual: the hub answers a staging after the agent
/// stopped waiting (and the page's withdrawal is lost), so it holds a
/// staging of this device. The seed is kept — unshown, no code — for the
/// attempt's ten minutes: a sign-in asked to confirm it can (no false
/// `rotation_unknown`), and its late `rotation_staged` makes it one the hub
/// took; a late refusal forgets it, and so does the end of the attempt.
#[tokio::test]
async fn a_staging_answered_late_is_kept_and_taken_and_one_never_answered_goes_with_its_attempt() {
    let a = Account::new();
    let mut script = Script::rotating(vec![machine().sign_public()]);
    script.stage = StageAnswer::Silent;
    let hub = FakeHub::start(script, a.entries.clone()).await;
    let shared = a.shared(&hub.url, &Arc::default());
    let task = signed_in(&shared).await;
    let eph = DhKeypair::from_secret([5; 32]);
    let (status, _) = rotate(&shared, &a.request([7; 16], &eph, 1)).await;
    assert_eq!(status, 504);
    let kept = a.staged().expect("kept for the attempt");
    assert!(kept.accepted_ms.is_none());
    let st = AgentState::load(&a.paths).unwrap().unwrap();
    let shown = rotation::status(&a.paths, Some(&st), now_ms());
    assert_eq!((shown.state.as_str(), shown.code), ("idle", None), "not shown, no code");
    // Asked to confirm it on this connection: confirmed, not unknown.
    let new = kept.identity();
    hub.send_json(json!({
        "t": "rotation_pending",
        "sign_key": hex(&new.sign_public()),
        "dh_key": hex(&new.dh_public()),
        "nonce_hash": hex(&Sha256::digest([7u8; 16])),
    }));
    until("the ack", || !hub.heard("rotation_ack").is_empty()).await;
    assert!(hub.heard("rotation_unknown").is_empty());
    // The late answer: one the hub took now, shown with its code.
    hub.send_json(json!({"t": "rotation_staged", "nonce_hash": hex(&Sha256::digest([7u8; 16])), "ready_ms": 5}));
    until("the late answer", || a.staged().is_some_and(|s| s.accepted_ms.is_some())).await;
    let shown = rotation::status(&a.paths, Some(&st), now_ms());
    assert_eq!(shown.state, "staged");
    assert!(shown.code.is_some(), "high assurance: its code now");
    // A late refusal forgets one never answered.
    let (status, _) = rotate(&shared, &a.request([8; 16], &eph, 0)).await;
    assert_eq!(status, 504);
    assert_eq!(a.nonces(), vec![[7; 16], [8; 16]]);
    hub.send_json(json!({"t": "rotation_refused", "nonce_hash": hex(&Sha256::digest([8u8; 16])), "reason": "no_attempt"}));
    until("the late refusal", || a.nonces() == vec![[7; 16]]).await;
    // One never answered goes when its attempt is over; the taken one stays.
    let (status, _) = rotate(&shared, &a.request([9; 16], &eph, 0)).await;
    assert_eq!(status, 504);
    let made = FileSeedStore::for_instance(&a.paths)
        .stagings()
        .unwrap()
        .into_iter()
        .find(|s| s.nonce == [9; 16])
        .unwrap()
        .created_ms;
    let named = rotation::named_keys(&shared.state.lock().unwrap());
    rotation::set_aside_expired(&a.paths, named, &machine(), made + rotation::ATTEMPT_MS + 1);
    assert_eq!(a.nonces(), vec![[7; 16]]);
    let aside: Vec<[u8; 16]> = FileSeedStore::for_instance(&a.paths).expired().iter().map(|s| s.nonce).collect();
    assert_eq!(aside, vec![[9; 16]], "set aside, never deleted (R6 review)");
    shared.cancel.cancel();
    let _ = task.await;
}

/// §3.5 / §3.8: an entry renewing this device to keys it never staged —
/// whoever else held its old key did it: `foreign_rotation` alarm, the link
/// stops, the old seed stays (and the staged one is not taken),
/// `link.rotation` says `foreign` / `renewed_elsewhere`.
#[tokio::test]
async fn a_renewal_to_keys_this_device_does_not_hold_is_alarmed_and_stops_the_link() {
    let mut a = Account::new();
    let old = machine();
    let hub = FakeHub::start(Script::rotating(vec![old.sign_public()]), a.entries.clone()).await;
    let restarts = Arc::new(AtomicUsize::new(0));
    let shared = a.shared(&hub.url, &restarts);
    let task = signed_in(&shared).await;
    // It staged a key of its own, too.
    let eph = DhKeypair::from_secret([5; 32]);
    assert_eq!(rotate(&shared, &a.request([7; 16], &eph, 0)).await.0, 200);
    // The hub committed the thief's renewal: the old key is retired there.
    {
        let mut script = hub.script.lock().unwrap();
        script.accept = vec![thief().sign_public()];
        script.rotated = vec![old.sign_public()];
    }
    // Its own trouble twice first: the statement waits on disk meanwhile.
    hub.script.lock().unwrap().dispute_answers = vec![503, 503];
    let before = hub.logins().len();
    let entries = a.renew_to(&thief());
    hub.send_entries(&entries);
    tokio::time::timeout(Duration::from_secs(10), task).await.expect("the link stops").unwrap();
    assert!(shared.cancel.is_cancelled());
    assert_eq!(a.seed_in_use().sign_public(), old.sign_public(), "the old seed stays");
    assert_eq!(restarts.load(Ordering::SeqCst), 0, "nothing promoted");
    // The evidence is the retired key's dispute statement
    // over that renewal, kept and delivered to the hub — no sign-in with the
    // retired key is made for it.
    until("the dispute statement", || !hub.disputes().is_empty()).await;
    let file = super::dispute::path(&a.paths, "a-1", "m-1", entries[0].seq(), &hex(&a.hub.hash_at(entries[0].seq()).unwrap()));
    let name = file.file_name().unwrap().to_str().unwrap().to_string();
    assert!(name.starts_with(&format!("rotation-dispute-{}-", entries[0].seq())) && name.ends_with(".json"), "{name}");
    assert!(file.parent().unwrap().ends_with("agent"));
    assert!(file.exists(), "kept until the hub has it");
    until("the hub's 200", || hub.disputes().len() == 3).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(hub.logins().len(), before, "no sign-in with the retired key on purpose");
    let disputes = hub.disputes();
    assert!(disputes.iter().all(|d| d == &disputes[0]), "the same statement each time");
    let (path, body) = &disputes[0];
    assert_eq!(path, "/v1/rotations/m-1/disputes");
    let seq = entries[0].seq();
    assert_eq!(body["seq"].as_u64(), Some(seq));
    assert_eq!(body["entry_hash"].as_str(), Some(hex(&a.hub.hash_at(seq).unwrap()).as_str()));
    let sig = unhex64(body["sig"].as_str().unwrap());
    assert!(a.hub.verify_rotate_dispute("m-1", seq, &sig), "it verifies with rh-proto on the chain");
    until("the hub's 200 noted", || {
        AgentState::load(&a.paths).unwrap().unwrap().rotation.evidence_ms.is_some()
    })
    .await;
    until("the file gone once delivered", || !file.exists()).await;
    let st = AgentState::load(&a.paths).unwrap().unwrap();
    assert!(st.alarms.iter().any(|al| al.what.contains("replaced elsewhere")), "{:?}", st.alarms);
    let shown = rotation::status(&a.paths, Some(&st), now_ms());
    assert_eq!((shown.state.as_str(), shown.reason.as_deref()), ("foreign", Some(rotation::FOREIGN_RENEWED_ELSEWHERE)));
    assert_eq!(
        rotation::verdict(&st, &old, &[], false),
        Verdict::Foreign {
            seq: entries[0].seq(),
            fingerprint: rh_proto::identity::device_fingerprint_text(&thief().sign_public(), &thief().dh_public()),
        }
    );
    // Started again, it does not connect, and the statement the hub took is
    // not delivered again.
    let again = link::test_shared(&a.paths, st, old, super::streams::Handle::detached(), rotation::no_restart());
    let before = hub.logins().len();
    let task = tokio::spawn(link::run(again.clone()));
    tokio::time::timeout(Duration::from_secs(5), task).await.expect("it stops at once").unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(hub.logins().len(), before, "no sign-in with a key the chain retired");
    assert_eq!(hub.disputes().len(), 3);
}

/// The undelivered statement survives "disconnect": a real
/// host told "replaced elsewhere" whose user disconnects instead of adding
/// it again would otherwise delete the only evidence before the hub has it.
/// Everything else in `agent/` goes; the statement is delivered after the
/// unenroll — also after a restart — and only then deleted.
#[tokio::test]
async fn an_undelivered_dispute_statement_survives_disconnecting() {
    let mut a = Account::new();
    let old = machine();
    let mut script = Script::rotating(vec![old.sign_public()]);
    script.dispute_answers = vec![503; 10_000];
    let hub = FakeHub::start(script, a.entries.clone()).await;
    let shared = a.shared(&hub.url, &Arc::default());
    let task = signed_in(&shared).await;
    {
        let mut script = hub.script.lock().unwrap();
        script.accept = vec![thief().sign_public()];
        script.rotated = vec![old.sign_public()];
    }
    let entries = a.renew_to(&thief());
    hub.send_entries(&entries);
    tokio::time::timeout(Duration::from_secs(10), task).await.expect("the link stops").unwrap();
    let seq = entries[0].seq();
    let file = super::dispute::path(&a.paths, "a-1", "m-1", seq, &hex(&a.hub.hash_at(seq).unwrap()));
    until("a first delivery", || !hub.disputes().is_empty()).await;
    assert!(file.exists());
    // Disconnect before the hub has it.
    super::control::Control::new(a.paths.clone()).unenroll().await.unwrap();
    assert!(file.exists(), "the statement survives");
    let left: Vec<std::ffi::OsString> = std::fs::read_dir(file.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(left, vec![file.file_name().unwrap().to_os_string()], "everything else in agent/ is gone");
    assert!(!super::state::exists(&a.paths));
    // Still delivered, unenrolled…
    let tried = hub.disputes().len();
    until("a delivery after the unenroll", || hub.disputes().len() > tried).await;
    // …and after a restart (the process ends; the next start delivers it).
    super::dispute::stop_delivery(&a.paths);
    until("the process gone", || !super::dispute::delivering(&a.paths)).await;
    assert!(file.exists());
    hub.script.lock().unwrap().dispute_answers.clear();
    super::dispute::deliver(&a.paths);
    until("delivered, and only then deleted", || !file.exists()).await;
    let sig = unhex64(hub.disputes().last().unwrap().1["sig"].as_str().unwrap());
    assert!(a.hub.verify_rotate_dispute("m-1", seq, &sig));
    // Done: a disconnect now leaves nothing at all.
    super::control::unenroll_files(&a.paths).unwrap();
    assert!(!file.parent().unwrap().exists());
}

/// A hub refusing the key in use as `rotated` is not
/// evidence. The agent asks for the chain on that socket; a hostile hub that
/// shows no valid renewal gets `refused` — retried with backoff, the seed in
/// use kept, no fresh seed, no alarm, nothing archived — and an honest one
/// serving the verified renewal of this id to keys it does not hold gets
/// `foreign` (`renewed_elsewhere`), the link stopped.
#[tokio::test]
async fn a_hub_saying_rotated_is_believed_only_with_a_verified_renewal() {
    // A hostile hub: `rotated`, and no renewal on the chain it serves.
    let a = Account::new();
    let old = machine();
    let mut script = Script::rotating(Vec::new());
    script.rotated = vec![old.sign_public()];
    let hub = FakeHub::start(script, a.entries.clone()).await;
    let shared = a.shared(&hub.url, &Arc::default());
    let task = tokio::spawn(link::run(shared.clone()));
    until("a second try", || hub.logins().len() >= 2).await;
    assert!(!shared.cancel.is_cancelled(), "the link keeps trying");
    assert!(!hub.heard("chain_sub").is_empty(), "it asked for the chain on the refused socket");
    {
        let st = shared.state.lock().unwrap();
        assert!(st.rotation.refused_ms.is_some() && st.rotation.foreign_ms.is_none());
        assert!(st.alarms.is_empty(), "no alarm on the hub's word");
        assert_eq!(rotation::status(&a.paths, Some(&st), now_ms()).state, "refused");
    }
    assert_eq!(a.seed_in_use().sign_public(), old.sign_public(), "no fresh seed");
    assert!(!rotation::archived(&a.paths, &old));
    assert!(!rotation::seed_retired(&a.paths, &old));
    assert!(shared.health.lock().unwrap().refused.is_some());
    shared.cancel.cancel();
    let _ = task.await;

    // An honest hub: the renewal that retired the key is on the chain it serves.
    let mut b = Account::new();
    let served = b.entries.clone();
    let renewal = b.renew_to(&thief());
    let mut script = Script::rotating(vec![thief().sign_public()]);
    script.rotated = vec![old.sign_public()];
    let hub = FakeHub::start(script, served.iter().cloned().chain(renewal.iter().cloned()).collect()).await;
    let restarts = Arc::new(AtomicUsize::new(0));
    // This machine's chain is from before the renewal.
    let mut st = AgentState::enrolling(&hub.url, "a-1", "m-1", fingerprint(&account_key().sign_public()), now_ms());
    assert!(st.absorb(&served, now_ms()).error.is_none());
    st.save(&b.paths).unwrap();
    let count = restarts.clone();
    let shared = link::test_shared(
        &b.paths,
        st,
        machine(),
        super::streams::Handle::detached(),
        Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
        }),
    );
    let task = tokio::spawn(link::run(shared.clone()));
    tokio::time::timeout(Duration::from_secs(10), task).await.expect("the link stops").unwrap();
    let st = shared.state.lock().unwrap();
    assert_eq!(st.rotation.foreign_reason.as_deref(), Some(rotation::FOREIGN_RENEWED_ELSEWHERE));
    assert!(st.alarms.iter().any(|al| al.what.contains("replaced elsewhere") && al.what.contains("backup")), "{:?}", st.alarms);
    drop(st);
    // That refused sign-in is no evidence; the retired key's dispute
    // statement is, delivered on its own — no other sign-in.
    until("the dispute statement", || !hub.disputes().is_empty()).await;
    let (_, body) = hub.disputes()[0].clone();
    assert!(b.hub.verify_rotate_dispute("m-1", renewal[0].seq(), &unhex64(body["sig"].as_str().unwrap())));
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(hub.logins().len(), 1);
    assert_eq!(b.seed_in_use().sign_public(), old.sign_public(), "the seed stays until a person adds it again");
    assert_eq!(restarts.load(Ordering::SeqCst), 0);
    // Only now is the seed retired, and the next sign-up uses a fresh one.
    assert!(rotation::seed_retired(&b.paths, &old));
    let fresh = rotation::identity_for_enrolling(&b.paths).unwrap();
    assert_ne!(fresh.sign_public(), old.sign_public());
}

/// §8 item 8: a channel opened with a device's old static key closes once
/// the chain renews that device's identity; the device opens it again with
/// its new key.
#[tokio::test]
async fn a_channel_opened_with_a_renewed_key_is_closed() {
    let mut a = Account::new();
    let h2_old = MachineIdentity::from_seed(&[20; 32]);
    let h2_new = MachineIdentity::from_seed(&[21; 32]);
    // h-2 runs AI too, and m-1 accepts its control (an aggregator).
    a.push(Action::AddDevice3(DeviceV3 {
        id: "h-2".into(),
        sign_key: h2_old.sign_public(),
        dh_key: h2_old.dh_public(),
        flags: Flags { host: true, admin: false },
    }));
    a.push(key_epoch("h-2", 0));
    a.push(Action::SetAccept {
        host: "m-1".into(),
        from: rh_proto::chain::Accept::Devices(vec!["h-2".into()]),
    });
    let shared = a.shared("ws://127.0.0.1:9", &Arc::default());
    let (tx, rx) = mpsc::unbounded_channel();
    std::mem::forget(rx);
    let mut routes: link::Routes = std::collections::HashMap::from([(1, ("h-2".to_string(), h2_old.dh_public(), tx.clone())), (2, ("d-1".to_string(), admin().dh_public(), tx))]);
    let (out, mut closed) = mpsc::unbounded_channel();
    link::drop_removed(&shared, &mut routes, &out);
    assert_eq!(routes.len(), 2, "nothing to close yet");
    let mut r = HostRotation {
        host: "h-2".into(),
        sign_key: h2_new.sign_public(),
        dh_key: h2_new.dh_public(),
        staged_seq: a.hub.seq(),
        staged_head: a.hub.head(),
        nonce: [3; 16],
        assurance: Assurance::Auto,
        old_sig: [0; 64],
        new_sig: [0; 64],
    };
    let body = r.body("a-1", &h2_old.sign_public(), &h2_old.dh_public());
    r.old_sig = rh_proto::sig::sign(h2_old.signing_key(), rh_proto::sig::HOST_ROTATE, &body);
    r.new_sig = rh_proto::sig::sign(h2_new.signing_key(), rh_proto::sig::HOST_ROTATE, &body);
    let entries = vec![a.push(Action::RotateHostKey(r)), a.push(key_epoch("h-2", 1))];
    assert!(shared.state.lock().unwrap().absorb(&entries, now_ms()).error.is_none());
    link::drop_removed(&shared, &mut routes, &out);
    assert_eq!(routes.keys().copied().collect::<Vec<_>>(), vec![2], "the admin's channel stays");
    match closed.try_recv() {
        Ok(Frame::Close { ch, code, reason }) => {
            assert_eq!((ch, code), (1, 4403));
            assert!(reason.contains("renewed"), "{reason}");
        }
        other => panic!("{other:?}"),
    }
}

/// R6 review: a staging set aside as expired — judged on the hub's word
/// (it took the key in use after the staging's day was over) — is never
/// tried at sign-in; but when this device's own verified chain names it
/// after all (a hub that committed it late), it is taken back, signed in
/// with and promoted: never "replaced elsewhere".
#[tokio::test]
async fn a_staging_set_aside_as_expired_is_taken_back_when_the_chain_names_it() {
    let mut a = Account::new();
    let old = machine();
    let new = a.stage([77; 32], [7; 16]);
    let answer = a.staged_answer(&new, [7; 16]);
    let store = FileSeedStore::for_instance(&a.paths);
    store.expire(&[7; 16]).unwrap();
    let hub = FakeHub::start(Script::rotating(vec![old.sign_public()]), a.entries.clone()).await;
    let restarts = Arc::new(AtomicUsize::new(0));
    let shared = a.shared(&hub.url, &restarts);
    let task = signed_in(&shared).await;
    let logins = hub.logins();
    assert_eq!(logins.len(), 1, "set aside: never tried at sign-in: {logins:?}");
    assert!(logins[0].by(&old, &hub.url));
    assert!(a.nonces().is_empty());
    assert_eq!(store.expired().len(), 1, "kept");
    // The hub commits it late: the chain names it.
    {
        let mut script = hub.script.lock().unwrap();
        script.accept = vec![new.sign_public()];
        script.rotated = vec![old.sign_public()];
    }
    let entries = a.renew(&answer, [7; 16], Assurance::Auto);
    hub.send_entries(&entries);
    until("the restart", || restarts.load(Ordering::SeqCst) == 1).await;
    tokio::time::timeout(Duration::from_secs(5), task).await.expect("the link ends").unwrap();
    assert_eq!(a.seed_in_use().sign_public(), new.sign_public(), "promoted from the staging set aside");
    assert!(store.expired().is_empty() && a.nonces().is_empty());
    let st = AgentState::load(&a.paths).unwrap().unwrap();
    assert!(st.rotation.foreign_ms.is_none() && st.alarms.is_empty(), "never replaced elsewhere");
    let logins = hub.logins();
    assert!(logins[1..].iter().all(|l| l.by(&new, &hub.url)), "{logins:?}");
}

/// R6 review: the hub's own trouble — `error
/// unavailable`, close 1011 — is transient: the staged key is tried again
/// after the back-off, and the key in use never signs in on it. Only the
/// hub's `denied` (it does not know the staged key) lets the key in use try.
#[tokio::test]
async fn a_hub_in_trouble_never_makes_the_key_in_use_sign_in() {
    let a = Account::new();
    let old = machine();
    let new = a.stage([77; 32], [7; 16]);
    let mut script = Script::rotating(vec![old.sign_public()]);
    script.unavailable = vec![new.sign_public()];
    let hub = FakeHub::start(script, a.entries.clone()).await;
    let shared = a.shared(&hub.url, &Arc::default());
    let task = tokio::spawn(link::run(shared.clone()));
    until("a second try", || hub.logins().len() >= 2).await;
    let logins = hub.logins();
    assert!(logins.iter().all(|l| l.by(&new, &hub.url) && !l.welcomed), "never the key in use: {logins:?}");
    assert!(shared.rotation.session().is_none());
    assert!(a.staged().is_some(), "still staged");
    // The hub is well again and does not know the staged key: `denied`, then the key in use.
    hub.script.lock().unwrap().unavailable.clear();
    until("the key in use", || shared.rotation.session().is_some_and(|s| !s.staged_key)).await;
    let logins = hub.logins();
    let (denied, taken) = (&logins[logins.len() - 2], &logins[logins.len() - 1]);
    assert!(denied.by(&new, &hub.url) && !denied.welcomed, "the staged key first: {logins:?}");
    assert!(taken.by(&old, &hub.url) && taken.welcomed, "then the key in use, on a denial");
    shared.cancel.cancel();
    let _ = task.await;
}

/// R6 review: the chain renews this device to keys it holds in no
/// staging that reads, while a staged file does not read. Not "replaced
/// elsewhere" — no such alarm, nothing offered, no sign-in on purpose with
/// the old key — but "staged key unreadable": alarmed, and the link stops
/// without signing in at all.
#[tokio::test]
async fn an_unreadable_staged_key_stops_the_link_and_is_not_taken_for_a_replacement() {
    let mut a = Account::new();
    let old = machine();
    let new = a.stage([77; 32], [7; 16]);
    let answer = a.staged_answer(&new, [7; 16]);
    a.renew(&answer, [7; 16], Assurance::Auto);
    let store = FileSeedStore::for_instance(&a.paths);
    super::write_private(&store.staging_path(&[7; 16]), b"not a key file").unwrap();
    let mut script = Script::rotating(vec![new.sign_public()]);
    script.rotated = vec![old.sign_public()];
    let hub = FakeHub::start(script, a.entries.clone()).await;
    let restarts = Arc::new(AtomicUsize::new(0));
    let shared = a.shared(&hub.url, &restarts);
    let task = tokio::spawn(link::run(shared.clone()));
    tokio::time::timeout(Duration::from_secs(5), task).await.expect("the link stops").unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(hub.logins().is_empty(), "no sign-in at all");
    let st = AgentState::load(&a.paths).unwrap().unwrap();
    assert!(st.rotation.unreadable_ms.is_some());
    assert!(st.rotation.foreign_ms.is_none() && st.rotation.evidence_ms.is_none());
    assert!(st.alarms.iter().any(|al| al.what.contains("Staged key unreadable")), "{:?}", st.alarms);
    assert!(!st.alarms.iter().any(|al| al.what.contains("replaced elsewhere")));
    assert_eq!(rotation::status(&a.paths, Some(&st), now_ms()).state, "unreadable");
    assert!(!rotation::seed_retired(&a.paths, &old));
    assert_eq!(a.seed_in_use().sign_public(), old.sign_public());
    assert_eq!(restarts.load(Ordering::SeqCst), 0);
}
