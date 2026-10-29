//! The device role's controller (`droppen_aggregator::controller`) run under
//! the agent state it is enrolled with, which the aggregator crate cannot name.
//! These tests stay here until the agent is a crate of its own.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use droppen_aggregator::controller::*;
use droppen_aggregator::hub::{HubError, Mail};
use droppen_instance::paths::Paths;
use rh_proto::chain::{Accept, Action, ChainClock, ChainState, Flags, KeyCommit, KeyCommits, Signer};
use rh_proto::identity::MachineIdentity;
use rh_proto::keys::DhKeypair;
use rh_proto::sig::{HubLogin, LoginRole};
use rh_proto::wrap::{self, encode_host_key_mail, seal_host_key, HostKeyContext};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::state::{AgentState, LinkEnrollment};

/// An account: admin `d-a`; hosts `m-me` (this computer, not an admin),
/// `m-2` (its key epoch 0 committed as `[0x22; 32]`) and `m-3` (epoch 0
/// as `[0x33; 32]`); `m-2` accepts every device.
struct Account {
    admin: (ed25519_dalek::SigningKey, DhKeypair),
    me: MachineIdentity,
    /// Every entry, for an agent state to absorb.
    entries: Vec<rh_proto::chain::Entry>,
    /// The account key's fingerprint (what the agent pins).
    pin: [u8; 16],
}

fn sk(b: u8) -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[b; 32])
}

fn commit(host: &str, epoch: u32, key: &[u8; 32]) -> Action {
    Action::KeyEpochs(KeyCommits {
        user: None,
        machines: vec![(
            host.into(),
            KeyCommit {
                epoch,
                hash: rh_proto::chain::machine_key_commit("a-1", host, epoch, key),
            },
        )],
    })
}

fn account() -> Account {
    let ak = sk(1);
    let admin = (sk(2), DhKeypair::from_secret([3; 32]));
    let device = |id: &str, sign: &ed25519_dalek::SigningKey, dh: [u8; 32], host: bool, admin: bool| rh_proto::chain::DeviceV3 {
        id: id.into(),
        sign_key: sign.verifying_key().to_bytes(),
        dh_key: dh,
        flags: Flags { host, admin },
    };
    let genesis = rh_proto::chain::genesis3_draft(
        "a-1",
        ak.verifying_key().to_bytes(),
        device("d-a", &admin.0, admin.1.public(), false, true),
        None,
        1,
    )
    .sign(&ak);
    let mut clock = ChainClock::history();
    let mut state = ChainState::genesis(&genesis, None).unwrap();
    let mut entries = vec![genesis];
    let me = MachineIdentity::from_seed(&[9; 32]);
    let m2 = MachineIdentity::from_seed(&[10; 32]);
    let m3 = MachineIdentity::from_seed(&[11; 32]);
    let mut push = |state: &mut ChainState, action: Action| {
        let e = state.draft(Signer::Device("d-a".into()), action, 2 + state.seq()).sign(&admin.0);
        state.apply(&e, &mut clock).unwrap();
        entries.push(e);
    };
    for (id, m, key) in [("m-me", &me, [0x11; 32]), ("m-2", &m2, [0x22; 32]), ("m-3", &m3, [0x33; 32])] {
        push(&mut state, Action::AddDevice3(device(id, m.signing_key(), m.dh_public(), true, false)));
        push(&mut state, commit(id, 0, &key));
    }
    push(
        &mut state,
        Action::SetAccept {
            host: "m-2".into(),
            from: Accept::All,
        },
    );
    let pin = rh_proto::identity::fingerprint(&ak.verifying_key().to_bytes());
    Account { admin, me, entries, pin }
}

fn mail(a: &Account, sender: &str, sender_dh: &DhKeypair, host: &str, epoch: u32, key: &[u8; 32]) -> Mail {
    let ctx = HostKeyContext {
        account: "a-1".into(),
        sender: sender.into(),
        recipient: "m-me".into(),
        host: host.into(),
        epoch,
    };
    let sealed = seal_host_key(sender_dh, &a.me.dh_public(), &ctx, key, &mut rand::rngs::OsRng).unwrap();
    Mail {
        id: 1,
        sender: sender.into(),
        purpose: wrap::MAIL_HOST_KEY,
        epoch,
        sealed: encode_host_key_mail(host, &sealed),
    }
}

/// A hub for one device role, in memory: device sessions minted for
/// `m-me`'s signature only, its mailbox, the acknowledgements and the key
/// requests it got, and which agents declared `policy/1`.
#[derive(Default)]
struct FakeHub {
    origin: String,
    sign_key: [u8; 32],
    mail: Vec<(i64, String, u8, u32, Vec<u8>)>,
    acked: Vec<i64>,
    asked: Vec<(String, u8, u64)>,
    sessions: usize,
    /// Every `POST /v1/session`, taken or not.
    attempts: usize,
    /// How long a key request takes to be answered (ms).
    ask_delay_ms: u64,
    /// Answer sign-ins `403 device_removed`.
    removed: bool,
    /// Answer sign-ins `403 rotation_pending` (the device role suspended).
    suspended: bool,
    /// How many times the mailbox was read, and at which read each mail was acknowledged.
    reads: usize,
    acked_at: Vec<(i64, usize)>,
}

type Hub = Arc<Mutex<FakeHub>>;

async fn fake_hub(sign_key: [u8; 32]) -> (Hub, String) {
    use axum::extract::{Path as UrlPath, Query, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{delete, get, post};
    use axum::Json;
    use serde_json::{json, Value};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("ws://{}", listener.local_addr().unwrap());
    let hub: Hub = Arc::new(Mutex::new(FakeHub {
        origin: origin.clone(),
        sign_key,
        ..FakeHub::default()
    }));
    let authed = |headers: &HeaderMap| headers.get("authorization").and_then(|v| v.to_str().ok()) == Some("Bearer tok");
    let app = axum::Router::new()
        .route(
            "/v1/session/challenge",
            post(|| async { Json(json!({"nonce": droppen_base::text::hex(&[7; 32])})) }),
        )
        .route(
            "/v1/session",
            post(|State(hub): State<Hub>, Json(body): Json<Value>| async move {
                let mut h = hub.lock().unwrap();
                h.attempts += 1;
                if h.removed {
                    h.sessions += 1;
                    return (StatusCode::FORBIDDEN, Json(json!({"error": "device_removed"})));
                }
                if h.suspended {
                    return (StatusCode::FORBIDDEN, Json(json!({"error": "rotation_pending"})));
                }
                let text = body["signature"].as_str().unwrap_or_default();
                let bytes: Vec<u8> = (0..text.len() / 2)
                    .filter_map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok())
                    .collect();
                let signature: [u8; 64] = bytes.try_into().unwrap_or([0; 64]);
                let login = HubLogin {
                    role: LoginRole::DeviceSession,
                    account: body["account"].as_str().unwrap_or_default().into(),
                    id: body["device"].as_str().unwrap_or_default().into(),
                    hub: h.origin.clone(),
                    nonce: [7; 32],
                    time_ms: body["time_ms"].as_u64().unwrap_or_default(),
                };
                if body["device"] != "m-me" || !login.verify(&h.sign_key, &signature) {
                    return (StatusCode::UNAUTHORIZED, Json(json!({"error": "denied"})));
                }
                h.sessions += 1;
                (StatusCode::OK, Json(json!({"token": "tok", "expires_ms": droppen_base::time::now_ms_u64() + 3_600_000})))
            }),
        )
        .route(
            "/v1/mailbox",
            get(
                move |State(hub): State<Hub>, headers: HeaderMap, Query(q): Query<std::collections::HashMap<String, String>>| async move {
                    if !authed(&headers) {
                        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "unauthorized"})));
                    }
                    let after: i64 = q.get("after").and_then(|a| a.parse().ok()).unwrap_or(0);
                    let mut h = hub.lock().unwrap();
                    if after == 0 {
                        h.reads += 1;
                    }
                    let mail: Vec<Value> = h
                        .mail
                        .iter()
                        .filter(|m| m.0 > after)
                        .map(|m| json!({"id": m.0, "sender": m.1, "purpose": m.2, "epoch": m.3, "sealed": droppen_base::text::hex(&m.4)}))
                        .collect();
                    (StatusCode::OK, Json(json!({ "mail": mail })))
                },
            )
            .post(move |State(hub): State<Hub>, headers: HeaderMap, Json(body): Json<Value>| async move {
                if !authed(&headers) {
                    return (StatusCode::UNAUTHORIZED, Json(json!({"error": "unauthorized"})));
                }
                let (id, delay) = {
                    let mut h = hub.lock().unwrap();
                    h.asked.push((
                        body["recipient"].as_str().unwrap_or_default().into(),
                        body["purpose"].as_u64().unwrap_or_default() as u8,
                        droppen_base::time::now_ms_u64(),
                    ));
                    (1000 + h.asked.len(), h.ask_delay_ms)
                };
                tokio::time::sleep(Duration::from_millis(delay)).await;
                (StatusCode::OK, Json(json!({ "id": id })))
            }),
        )
        .route(
            "/v1/mailbox/{id}",
            delete(move |State(hub): State<Hub>, headers: HeaderMap, UrlPath(id): UrlPath<i64>| async move {
                if !authed(&headers) {
                    return StatusCode::UNAUTHORIZED;
                }
                let mut h = hub.lock().unwrap();
                h.mail.retain(|m| m.0 != id);
                h.acked.push(id);
                let reads = h.reads;
                h.acked_at.push((id, reads));
                StatusCode::NO_CONTENT
            }),
        )
        .route(
            "/v1/me",
            get(move |headers: HeaderMap| async move {
                if !authed(&headers) {
                    return (StatusCode::UNAUTHORIZED, Json(json!({"error": "unauthorized"})));
                }
                (
                    StatusCode::OK,
                    Json(json!({"machines": [{"id": "m-2", "reads_v3": true}, {"id": "m-3", "reads_v3": false}]})),
                )
            }),
        )
        .with_state(hub.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (hub, origin)
}

async fn until(within: Duration, what: &str, f: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + within;
    while !f() {
        assert!(std::time::Instant::now() < deadline, "never: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The controller's rounds against a hub: it signs in as a device with
/// its machine key, keeps the host key an admin sealed it, drops (and
/// acknowledges) every other mail, says which hosts declared `policy/1`;
/// a key nobody seals it is asked for once it has been missing a while —
/// once, from the admins only — and taken when it comes; turned off, it
/// deletes nothing more and writes nothing more.
#[tokio::test]
async fn the_device_role_takes_its_keys_asks_for_a_missing_one_and_stops_when_turned_off() {
    let a = account();
    let (hub, origin) = fake_hub(a.me.sign_public()).await;
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::for_instance(dir.path().to_path_buf(), "control");
    std::fs::create_dir_all(paths.instance_dir()).unwrap();
    let mut st = AgentState::enrolling(&origin, "a-1", "m-me", a.pin, droppen_base::time::now_ms_u64());
    assert!(st.absorb(&a.entries, droppen_base::time::now_ms_u64()).error.is_none());
    assert!(st.enrolled());
    let state = Arc::new(Mutex::new(st));
    let (bump, chain) = watch::channel(0u64);
    let status = Arc::new(Mutex::new(Status::default()));
    let cancel = CancellationToken::new();
    // Mail waiting: m-2's key (kept), junk of another purpose and m-3's key (m-3 does not accept it): dropped.
    {
        let mut h = hub.lock().unwrap();
        let keep = mail(&a, "d-a", &a.admin.1, "m-2", 0, &[0x22; 32]);
        h.mail.push((1, keep.sender, keep.purpose, keep.epoch, keep.sealed));
        h.mail.push((2, "d-a".into(), wrap::MAIL_USER_KEY, 0, vec![1, 2, 3]));
        let other = mail(&a, "d-a", &a.admin.1, "m-3", 0, &[0x33; 32]);
        h.mail.push((3, other.sender, other.purpose, other.epoch, other.sealed));
    }
    let timing = Timing {
        gap_every: Duration::from_millis(50),
        idle_every: Duration::from_millis(50),
        ask_after: Duration::from_millis(300),
        ask_backoff: [Duration::from_secs(600); 3],
    };
    let (keys_out, ring) = watch::channel(None);
    let switch = droppen_aggregator::TestSwitch::on();
    let task = tokio::spawn(run(Env {
        paths: paths.clone(),
        identity: Arc::new(MachineIdentity::from_seed(&[9; 32])),
        enrollment: Arc::new(LinkEnrollment(state.clone())),
        switch: switch.clone(),
        chain,
        cancel: cancel.clone(),
        status: status.clone(),
        timing,
        keys_out,
        nudge: Arc::default(),
        may_sign_in: None,
    }));
    let covers = |epochs: &[u32]| {
        let s = status.lock().unwrap();
        s.signed_in && s.hosts.iter().any(|h| h.id == "m-2" && h.epochs == epochs && h.policy == Some(true))
    };
    until(Duration::from_secs(10), "m-2's epoch 0 held", || covers(&[0])).await;
    // S1-5: the copies task is handed the same keys, in memory.
    {
        let held = ring.borrow().clone().expect("keys handed over");
        assert_eq!((held.account.as_str(), held.device.as_str()), ("a-1", "m-me"));
        assert_eq!(held.hosts.get("m-2").map(|k| k.0.clone()), Some(vec![(0, [0x22; 32])]));
    }
    assert_eq!(hub.lock().unwrap().acked, vec![1, 2, 3]);
    assert_eq!(hub.lock().unwrap().sessions, 1);
    let keys = droppen_aggregator::keys::HostKeys::load(&paths, "a-1", "m-me").unwrap();
    assert_eq!(*keys.key("m-2", 0).unwrap(), [0x22; 32]);
    assert!(!keys.has("m-3", 0), "a host that does not accept it");
    assert!(hub.lock().unwrap().asked.is_empty(), "nothing missing: nothing asked");
    assert_eq!(status.lock().unwrap().hosts.len(), 1, "only m-2 accepts it");

    // m-2's key turns over; nobody seals it the new one: it asks the admin, once.
    let committed = droppen_base::time::now_ms_u64();
    {
        let mut st = state.lock().unwrap();
        let chain = st.chain.clone().unwrap();
        let e = chain
            .draft(Signer::Device("d-a".into()), commit("m-2", 1, &[0x23; 32]), 100)
            .sign(&a.admin.0);
        assert!(st.absorb(&[e], droppen_base::time::now_ms_u64()).error.is_none());
    }
    bump.send_modify(|v| *v += 1);
    until(Duration::from_secs(10), "a key request", || !hub.lock().unwrap().asked.is_empty()).await;
    let asked = hub.lock().unwrap().asked.clone();
    assert_eq!(asked.len(), 1);
    assert_eq!((asked[0].0.as_str(), asked[0].1), ("d-a", wrap::MAIL_KEY_REQUEST), "the admin, purpose 4");
    assert!(asked[0].2 - committed >= 300, "after the grace for the admin's own delivery");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(hub.lock().unwrap().asked.len(), 1, "once per backoff step");
    assert!(status.lock().unwrap().asked_ms.is_some());
    // The admin's answer.
    {
        let answer = mail(&a, "d-a", &a.admin.1, "m-2", 1, &[0x23; 32]);
        hub.lock()
            .unwrap()
            .mail
            .push((4, answer.sender, answer.purpose, answer.epoch, answer.sealed));
    }
    until(Duration::from_secs(10), "m-2's epoch 1 held", || covers(&[0, 1])).await;
    // The gap closed: the next one starts its backoff again.
    until(Duration::from_secs(10), "the request record reset", || {
        droppen_aggregator::keys::HostKeys::load(&paths, "a-1", "m-me").is_ok_and(|k| k.asked().1.is_empty() && k.asked().2 == 0)
    })
    .await;

    // Turned off: the directory goes, and nothing writes it again.
    switch.set(false);
    droppen_aggregator::forget_all(&paths).unwrap();
    until(Duration::from_secs(10), "idle", || !status.lock().unwrap().running).await;
    {
        let again = mail(&a, "d-a", &a.admin.1, "m-2", 1, &[0x23; 32]);
        hub.lock().unwrap().mail.push((5, again.sender, again.purpose, again.epoch, again.sealed));
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!droppen_aggregator::dir(&paths).exists(), "nothing written once off");
    cancel.cancel();
    task.await.unwrap();
}

/// A controller on `a`'s chain against `hub`, with short rounds: the
/// agent state, the chain bump, its status, its token and its task.
struct Running {
    paths: Paths,
    state: Arc<Mutex<AgentState>>,
    bump: watch::Sender<u64>,
    status: Arc<Mutex<Status>>,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
    /// The owner's switch it runs under.
    switch: Arc<droppen_aggregator::TestSwitch>,
    /// Wakes it as pushed mail would.
    nudge: Arc<tokio::sync::Notify>,
    _dir: tempfile::TempDir,
}

fn start(a: &Account, origin: &str) -> Running {
    start_as(a, origin, MachineIdentity::from_seed(&[9; 32]))
}

/// [`start`], the device role running with `identity`.
fn start_as(a: &Account, origin: &str, identity: MachineIdentity) -> Running {
    start_gated(a, origin, identity, None)
}

/// [`start_as`], its sign-ins gated by `may_sign_in`.
fn start_gated(a: &Account, origin: &str, identity: MachineIdentity, may_sign_in: Option<droppen_aggregator::SignInGate>) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::for_instance(dir.path().to_path_buf(), "control");
    std::fs::create_dir_all(paths.instance_dir()).unwrap();
    let mut st = AgentState::enrolling(origin, "a-1", "m-me", a.pin, droppen_base::time::now_ms_u64());
    assert!(st.absorb(&a.entries, droppen_base::time::now_ms_u64()).error.is_none());
    let state = Arc::new(Mutex::new(st));
    let (bump, chain) = watch::channel(0u64);
    let status = Arc::new(Mutex::new(Status::default()));
    let cancel = CancellationToken::new();
    let nudge: Arc<tokio::sync::Notify> = Arc::default();
    let switch = droppen_aggregator::TestSwitch::on();
    let task = tokio::spawn(run(Env {
        paths: paths.clone(),
        identity: Arc::new(identity),
        enrollment: Arc::new(LinkEnrollment(state.clone())),
        switch: switch.clone(),
        chain,
        cancel: cancel.clone(),
        status: status.clone(),
        timing: Timing {
            gap_every: Duration::from_millis(50),
            idle_every: Duration::from_millis(50),
            ask_after: Duration::from_millis(100),
            ask_backoff: [Duration::from_secs(600); 3],
        },
        keys_out: watch::channel(None).0,
        nudge: nudge.clone(),
        may_sign_in,
    }));
    Running {
        paths,
        state,
        bump,
        status,
        cancel,
        task,
        switch,
        nudge,
        _dir: dir,
    }
}

fn push_mail(hub: &Hub, id: i64, m: Mail) {
    hub.lock().unwrap().mail.push((id, m.sender, m.purpose, m.epoch, m.sealed));
}

/// The admin signs `actions` onto the agent's chain, and the chain bumps.
fn sign_onto(r: &Running, a: &Account, actions: Vec<Action>) {
    {
        let mut st = r.state.lock().unwrap();
        let mut chain = st.chain.clone().unwrap();
        let mut entries = Vec::new();
        for action in actions {
            let e = chain.draft(Signer::Device("d-a".into()), action, 200 + chain.seq()).sign(&a.admin.0);
            chain.apply(&e, &mut ChainClock::history()).unwrap();
            entries.push(e);
        }
        assert!(st.absorb(&entries, droppen_base::time::now_ms_u64()).error.is_none());
    }
    r.bump.send_modify(|v| *v += 1);
}

fn held(r: &Running) -> Vec<String> {
    droppen_aggregator::keys::HostKeys::load(&r.paths, "a-1", "m-me")
        .map(|k| k.hosts())
        .unwrap_or_default()
}

/// Security review of S1-4, S5 (and D2): a host that stops accepting this
/// device mid-run — its accept list narrowed to admins, its key turned
/// over — has its keys dropped here, from the disk and the status.
#[tokio::test]
async fn a_host_that_narrows_its_accept_list_mid_run_loses_its_keys_here() {
    let a = account();
    let (hub, origin) = fake_hub(a.me.sign_public()).await;
    push_mail(&hub, 1, mail(&a, "d-a", &a.admin.1, "m-2", 0, &[0x22; 32]));
    let r = start(&a, &origin);
    until(Duration::from_secs(10), "m-2 held", || held(&r) == ["m-2"]).await;
    sign_onto(
        &r,
        &a,
        vec![
            Action::SetAccept {
                host: "m-2".into(),
                from: Accept::Admins,
            },
            commit("m-2", 1, &[0x24; 32]),
        ],
    );
    until(Duration::from_secs(10), "m-2's keys dropped", || held(&r).is_empty()).await;
    until(Duration::from_secs(10), "status without m-2", || {
        let s = r.status.lock().unwrap();
        s.running && s.hosts.is_empty()
    })
    .await;
    r.cancel.cancel();
    r.task.await.unwrap();
}

/// Security review of S1-4, S1: turned off while a key request is on its
/// way (the window between reading the mailbox and noting the request),
/// the directory deleted meanwhile stays deleted — nothing held in memory
/// writes it back; and a key that arrives after is not taken either.
#[tokio::test]
async fn turned_off_mid_round_the_deleted_keys_stay_deleted() {
    let a = account();
    let (hub, origin) = fake_hub(a.me.sign_public()).await;
    hub.lock().unwrap().ask_delay_ms = 700;
    push_mail(&hub, 1, mail(&a, "d-a", &a.admin.1, "m-2", 0, &[0x22; 32]));
    let r = start(&a, &origin);
    until(Duration::from_secs(10), "m-2 held", || held(&r) == ["m-2"]).await;
    // A key it lacks, so it asks — slowly.
    sign_onto(&r, &a, vec![commit("m-2", 1, &[0x23; 32])]);
    until(Duration::from_secs(10), "a key request on its way", || {
        !hub.lock().unwrap().asked.is_empty()
    })
    .await;
    // The owner turns it off meanwhile (the switch first, the directory a moment later, as the local API does).
    r.switch.set(false);
    push_mail(&hub, 2, mail(&a, "d-a", &a.admin.1, "m-2", 1, &[0x23; 32]));
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let disk = droppen_aggregator::keys::HostKeys::load(&r.paths, "a-1", "m-me").unwrap();
    assert_eq!(disk.asked_ms(), None, "the request is not noted once it is off");
    assert_eq!(disk.epochs("m-2"), vec![0], "no key taken once it is off");
    droppen_aggregator::forget_all(&r.paths).unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!droppen_aggregator::dir(&r.paths).exists(), "nothing wrote the keys back");
    assert!(!r.status.lock().unwrap().running);
    // On again: it starts from the disk (nothing), and takes what the mailbox holds.
    r.switch.set(true);
    r.bump.send_modify(|v| *v += 1);
    until(Duration::from_secs(10), "m-2's epoch 1 taken again", || {
        droppen_aggregator::keys::HostKeys::load(&r.paths, "a-1", "m-me").is_ok_and(|k| k.epochs("m-2") == [1])
    })
    .await;
    r.cancel.cancel();
    r.task.await.unwrap();
}

/// S1: stopped (its link's token) while a key request is on its way —
/// the link stopping for a disconnection or a removal — it writes nothing
/// after: the request is not noted, the directory not recreated.
#[tokio::test]
async fn stopped_mid_round_it_writes_nothing_after() {
    let a = account();
    let (hub, origin) = fake_hub(a.me.sign_public()).await;
    hub.lock().unwrap().ask_delay_ms = 700;
    push_mail(&hub, 1, mail(&a, "d-a", &a.admin.1, "m-2", 0, &[0x22; 32]));
    let r = start(&a, &origin);
    until(Duration::from_secs(10), "m-2 held", || held(&r) == ["m-2"]).await;
    sign_onto(&r, &a, vec![commit("m-2", 1, &[0x23; 32])]);
    until(Duration::from_secs(10), "a key request on its way", || {
        !hub.lock().unwrap().asked.is_empty()
    })
    .await;
    r.cancel.cancel();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let disk = droppen_aggregator::keys::HostKeys::load(&r.paths, "a-1", "m-me").unwrap();
    assert_eq!(disk.asked_ms(), None, "nothing noted after the stop");
    r.task.await.unwrap();
}

/// Security review of S1-4, S2: the hub says this device was removed (it
/// was removed while this computer was off, so its chain never showed
/// it): the other devices' keys here go at once, and it does not keep
/// signing in.
#[tokio::test]
async fn the_hub_saying_removed_deletes_the_keys_and_stops_asking() {
    let a = account();
    let (hub, origin) = fake_hub(a.me.sign_public()).await;
    hub.lock().unwrap().removed = true;
    let r = {
        let r = start(&a, &origin);
        // What an earlier run kept.
        let mut keys = droppen_aggregator::keys::HostKeys::load(&r.paths, "a-1", "m-me").unwrap();
        keys.put("m-2", 0, &[0x22; 32]).unwrap();
        r
    };
    until(Duration::from_secs(10), "keys deleted", || !droppen_aggregator::dir(&r.paths).exists()).await;
    let error = r.status.lock().unwrap().error.clone();
    assert_eq!(error.as_deref(), Some(HubError::Removed.to_string().as_str()));
    let tries = hub.lock().unwrap().sessions;
    tokio::time::sleep(Duration::from_millis(600)).await;
    r.bump.send_modify(|v| *v += 1);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(hub.lock().unwrap().sessions, tries, "no more sign-ins (rounds every 50 ms otherwise)");
    assert!(!droppen_aggregator::dir(&r.paths).exists());
    r.cancel.cancel();
    r.task.await.unwrap();
}

/// Host identity renewal (`docs/host-identity-rotation.md` §3.8; the
/// hub phase's constraint): once the chain names other keys for this
/// computer than the ones it runs with — a renewal its link has not taken
/// yet — the device role signs in with nothing: the hub would keep a
/// sign-in with the retired key as evidence against this very computer.
#[tokio::test]
async fn a_device_role_never_signs_in_with_a_key_the_chain_does_not_name() {
    let a = account();
    let (hub, origin) = fake_hub(a.me.sign_public()).await;
    let r = start_as(&a, &origin, MachineIdentity::from_seed(&[12; 32]));
    tokio::time::sleep(Duration::from_millis(600)).await;
    r.bump.send_modify(|v| *v += 1);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(hub.lock().unwrap().attempts, 0, "no sign-in at all (rounds every 50 ms)");
    assert!(!r.status.lock().unwrap().signed_in);
    r.cancel.cancel();
    r.task.await.unwrap();
    // The keys the chain names, but a staged identity key is kept and the
    // link has not signed in with the key in use (§3.5 「不拿旧钥误伤自己」):
    // the chain here may be stale — still no sign-in.
    let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let gate = open.clone();
    let r = start_gated(
        &a,
        &origin,
        MachineIdentity::from_seed(&[9; 32]),
        Some(Arc::new(move || gate.load(std::sync::atomic::Ordering::SeqCst))),
    );
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(hub.lock().unwrap().attempts, 0, "no session minted with a key that may be retired");
    // The link signed in with the key in use: it is current.
    open.store(true, std::sync::atomic::Ordering::SeqCst);
    until(Duration::from_secs(10), "a sign-in", || hub.lock().unwrap().sessions > 0).await;
    r.cancel.cancel();
    r.task.await.unwrap();
}

/// R6 review: while this computer's identity renewal is pending the
/// hub suspends its device role (`403 rotation_pending`). The controller
/// parks — pushed mail does not wake it into another sign-in — until the
/// chain moves (the renewal written), then signs in again.
#[tokio::test]
async fn a_suspended_device_role_is_parked_until_the_chain_moves() {
    let a = account();
    let (hub, origin) = fake_hub(a.me.sign_public()).await;
    hub.lock().unwrap().suspended = true;
    let r = start(&a, &origin);
    until(Duration::from_secs(10), "the first sign-in", || hub.lock().unwrap().attempts == 1).await;
    until(Duration::from_secs(10), "the refusal said", || {
        r.status.lock().unwrap().error.as_deref().is_some_and(|e| e.contains("rotation_pending"))
    })
    .await;
    // Mail pushed meanwhile wakes it, and it stays parked.
    for _ in 0..5 {
        r.nudge.notify_one();
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    assert_eq!(hub.lock().unwrap().attempts, 1, "parked: no sign-in on a wake-up");
    // The chain moved: it tries again (and parks again while suspended)…
    r.bump.send_modify(|v| *v += 1);
    until(Duration::from_secs(10), "a sign-in once the chain moved", || hub.lock().unwrap().attempts == 2).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(hub.lock().unwrap().attempts, 2);
    // …and signs in once the hub lets it.
    hub.lock().unwrap().suspended = false;
    r.bump.send_modify(|v| *v += 1);
    until(Duration::from_secs(10), "a session", || hub.lock().unwrap().sessions > 0).await;
    r.cancel.cancel();
    r.task.await.unwrap();
}

/// Security re-review of S1-4, 4: a host key that cannot be kept never
/// sits in the mailbox for good — another key held for its epoch is
/// definitive (acknowledged at once); the disk failing is tried a few
/// rounds, then acknowledged.
#[tokio::test]
async fn a_key_that_cannot_be_kept_does_not_stick_in_the_mailbox() {
    let a = account();
    let (hub, origin) = fake_hub(a.me.sign_public()).await;
    let r = start(&a, &origin);
    // Another key for m-2's epoch 0 already in the file (not the committed one).
    droppen_aggregator::keys::HostKeys::load(&r.paths, "a-1", "m-me")
        .unwrap()
        .put("m-2", 0, &[0x99; 32])
        .unwrap();
    push_mail(&hub, 1, mail(&a, "d-a", &a.admin.1, "m-2", 0, &[0x22; 32]));
    until(Duration::from_secs(10), "the conflicting key's mail acknowledged", || {
        hub.lock().unwrap().acked.contains(&1)
    })
    .await;
    let at = hub.lock().unwrap().acked_at.iter().find(|(id, _)| *id == 1).map(|(_, read)| *read);
    assert_eq!(at, Some(1), "at the first read: a conflict is not tried again");
    let kept = droppen_aggregator::keys::HostKeys::load(&r.paths, "a-1", "m-me").unwrap();
    assert_eq!(*kept.key("m-2", 0).unwrap(), [0x99; 32], "the file is not changed by it");
    // The disk refuses writes: a few rounds, then acknowledged.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        sign_onto(&r, &a, vec![commit("m-2", 1, &[0x23; 32])]);
        let dir = droppen_aggregator::dir(&r.paths);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let first = hub.lock().unwrap().reads;
        push_mail(&hub, 2, mail(&a, "d-a", &a.admin.1, "m-2", 1, &[0x23; 32]));
        until(Duration::from_secs(10), "the unstorable key's mail acknowledged", || {
            hub.lock().unwrap().acked.contains(&2)
        })
        .await;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let at = hub
            .lock()
            .unwrap()
            .acked_at
            .iter()
            .find(|(id, _)| *id == 2)
            .map(|(_, read)| *read)
            .unwrap();
        assert!(
            at - first >= MAX_PUT_TRIES as usize,
            "tried {MAX_PUT_TRIES} rounds first ({} reads)",
            at - first
        );
        assert!(!droppen_aggregator::keys::HostKeys::load(&r.paths, "a-1", "m-me").unwrap().has("m-2", 1));
    }
    r.cancel.cancel();
    r.task.await.unwrap();
}
