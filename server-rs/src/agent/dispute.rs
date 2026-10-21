//! The retired key's dispute statement (`docs/host-identity-rotation.md`
//! §3.8 「证据：退役钥签的争议声明」, contract §12 #6).
//!
//! When this device's own verified chain renewed it to keys it does not hold
//! ([`super::rotation::Verdict::Foreign`]), whoever else held its key won. A
//! sign-in with the retired key is no evidence of it (two wall clocks cannot
//! order it against the renewal, and the hub only records such a sign-in).
//! The evidence is a statement the retired key signs over the hash of the
//! disputed `RotateHostKey` — signable only once that entry was seen
//! ([`rh_proto::chain::sign_rotate_dispute`]):
//!
//! - **Made** from the chain: the entry that retired the key this device
//!   holds, its hash and the sign key it installed, signed with that key
//!   ([`statement`]).
//! - **Kept** before anything is sent: `agent/rotation-dispute-<seq>.json`,
//!   written whole and flushed ([`keep`]). It needs no seed, so it outlives
//!   "add this device again with a new identity".
//! - **Delivered** to `POST /v1/rotations/{host}/disputes` `{seq, entry_hash,
//!   sig}` — no session: the signature is the credential — until the hub
//!   says 200, or 410 `host_removed` (then it stops). Anything else is tried
//!   again 30 s later, doubling to 10 minutes, and again at every start
//!   ([`deliver`]). The hub's 200 is noted on the file and as
//!   `RotationNote.evidence_ms`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use rh_proto::chain;
use rh_proto::identity::MachineIdentity;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::hub::{hex, now_ms};
use super::state::AgentState;
use crate::sessions::Paths;

/// The first wait after a delivery that failed…
#[cfg(not(test))]
const RETRY_FIRST: Duration = Duration::from_secs(30);
#[cfg(test)]
const RETRY_FIRST: Duration = Duration::from_millis(100);
/// …doubling up to this.
#[cfg(not(test))]
const RETRY_MAX: Duration = Duration::from_secs(10 * 60);
#[cfg(test)]
const RETRY_MAX: Duration = Duration::from_millis(400);
/// How long one delivery may take.
#[cfg(not(test))]
const POST_WAIT: Duration = Duration::from_secs(20);
#[cfg(test)]
const POST_WAIT: Duration = Duration::from_millis(700);

/// One statement as kept on disk (camelCase).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Statement {
    /// 1.
    pub version: u32,
    /// The hub it goes to (`wss://…`, the enrollment's).
    pub hub: String,
    /// The account…
    pub account: String,
    /// …and this device's id there (the host whose renewal it disputes).
    pub host: String,
    /// The disputed `RotateHostKey` entry.
    pub seq: u64,
    /// Its chain hash (hex, 32 bytes).
    pub entry_hash: String,
    /// The sign key it installed (hex, 32 bytes).
    pub new_sign: String,
    /// The retired key's signature (hex, 64 bytes).
    pub sig: String,
    /// When it was made (this device's clock).
    pub created_ms: u64,
    /// When the hub took it (200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acked_ms: Option<u64>,
    /// Why delivering it stopped without a 200 (`host_removed`: 410).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<String>,
}

impl Statement {
    /// Whether it still waits to be delivered.
    pub fn pending(&self) -> bool {
        self.acked_ms.is_none() && self.stopped.is_none()
    }
}

/// `<instance dir>/agent/rotation-dispute-<seq>.json`.
pub fn path(paths: &Paths, seq: u64) -> PathBuf {
    super::key_path(paths).with_file_name(format!("rotation-dispute-{seq}.json"))
}

/// The statement this device makes against the renewal that retired
/// `current` (the key it holds) on `st`'s verified chain — the
/// `RotateHostKey` at the end of the span `current` was this device's key
/// (`keys_at(host, seq − 1)`), however many renewals came after it. `None`
/// when the chain retired no key of this device that it holds.
pub fn statement(st: &AgentState, current: &MachineIdentity, now: u64) -> Option<Statement> {
    let chain = st.chain.as_ref()?;
    let record = chain.device_record(&st.machine)?;
    let seq = record.retired.iter().find(|r| r.sign_key == current.sign_public())?.until_seq;
    if chain.keys_at(&st.machine, seq.checked_sub(1)?)?.sign_key != current.sign_public() {
        return None;
    }
    let entry_hash = chain.hash_at(seq)?;
    let new_sign = chain.keys_at(&st.machine, seq)?.sign_key;
    let sig = chain::sign_rotate_dispute(current.signing_key(), &st.account, &st.machine, seq, &entry_hash, &new_sign);
    // Checked as every device will check it, before it is kept.
    if !chain.verify_rotate_dispute(&st.machine, seq, &sig) {
        tracing::error!(seq, "the dispute statement this device made does not verify on its own chain: not kept");
        return None;
    }
    Some(Statement {
        version: 1,
        hub: st.hub.clone(),
        account: st.account.clone(),
        host: st.machine.clone(),
        seq,
        entry_hash: hex(&entry_hash),
        new_sign: hex(&new_sign),
        sig: hex(&sig),
        created_ms: now,
        acked_ms: None,
        stopped: None,
    })
}

/// Keep `s` (0600, written whole, the directory flushed) unless one for its
/// entry is kept already: `true` when it was written now.
pub fn keep(paths: &Paths, s: &Statement) -> Result<bool, String> {
    let path = path(paths, s.seq);
    if path.exists() {
        return Ok(false);
    }
    write(&path, s)?;
    Ok(true)
}

fn write(path: &std::path::Path, s: &Statement) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?;
    super::write_private(path, &bytes)?;
    super::sync_dir(path);
    Ok(())
}

/// Every statement this instance keeps, by entry.
pub fn kept(paths: &Paths) -> Vec<(PathBuf, Statement)> {
    let key = super::key_path(paths);
    let Some(Ok(entries)) = key.parent().map(std::fs::read_dir) else {
        return Vec::new();
    };
    let mut out: Vec<(PathBuf, Statement)> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_str().is_some_and(|n| n.starts_with("rotation-dispute-") && n.ends_with(".json")))
        .filter_map(|e| {
            let read = std::fs::read(e.path()).ok().and_then(|b| serde_json::from_slice::<Statement>(&b).ok());
            if read.is_none() {
                tracing::warn!(path = %e.path().display(), "a dispute statement that does not read: not delivered");
            }
            read.map(|s| (e.path(), s))
        })
        .collect();
    out.sort_by_key(|(_, s)| s.seq);
    out
}

/// What the hub did with one delivery.
#[derive(Debug, PartialEq, Eq)]
enum Delivery {
    /// 200: it holds the evidence.
    Acked,
    /// 410 `host_removed`: nobody needs it any more.
    Removed,
    /// Anything else (its trouble, a timeout, the network): again later.
    Again(String),
}

/// A host id as one path segment (ids are `[A-Za-z0-9_-]`; anything else escaped).
fn segment(id: &str) -> String {
    id.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

async fn post(http: &reqwest::Client, s: &Statement) -> Delivery {
    let url = format!(
        "{}/v1/rotations/{}/disputes",
        crate::aggregator::hub::http_base(&s.hub).trim_end_matches('/'),
        segment(&s.host)
    );
    let body = json!({"seq": s.seq, "entry_hash": s.entry_hash, "sig": s.sig});
    let sent = tokio::time::timeout(POST_WAIT, async {
        let response = http.post(&url).json(&body).send().await.map_err(|e| e.without_url().to_string())?;
        let status = response.status().as_u16();
        let answer: Value = response.json().await.unwrap_or(Value::Null);
        Ok::<_, String>((status, answer))
    })
    .await;
    match sent {
        Ok(Ok((200, _))) => Delivery::Acked,
        Ok(Ok((410, _))) => Delivery::Removed,
        Ok(Ok((status, answer))) => Delivery::Again(format!("{status} {}", answer["error"].as_str().unwrap_or_default())),
        Ok(Err(error)) => Delivery::Again(error),
        Err(_) => Delivery::Again("the hub did not answer in time".into()),
    }
}

/// The deliveries running, by agent directory: a generation and the task.
static RUNNING: LazyLock<Mutex<HashMap<PathBuf, (u64, tokio::task::AbortHandle)>>> = LazyLock::new(Default::default);
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Removes its delivery from [`RUNNING`] however it ends (aborted included).
struct Running(PathBuf, u64);

impl Drop for Running {
    fn drop(&mut self) {
        let mut running = RUNNING.lock().expect("dispute deliveries");
        if running.get(&self.0).is_some_and(|(g, _)| *g == self.1) {
            running.remove(&self.0);
        }
    }
}

fn agent_dir(paths: &Paths) -> PathBuf {
    super::key_path(paths).parent().map(std::path::Path::to_path_buf).unwrap_or_default()
}

/// Deliver every statement this instance keeps that the hub has not taken —
/// at start, and whenever one is made. One delivery task per instance; none
/// when nothing waits (or outside a runtime).
pub fn deliver(paths: &Paths) {
    if !kept(paths).iter().any(|(_, s)| s.pending()) {
        return;
    }
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        tracing::warn!("no runtime to deliver this device's dispute statement now: at the next start");
        return;
    };
    let dir = agent_dir(paths);
    let mut running = RUNNING.lock().expect("dispute deliveries");
    if running.contains_key(&dir) {
        return;
    }
    let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let guard = Running(dir.clone(), generation);
    let task = runtime.spawn(deliveries(paths.clone(), guard));
    running.insert(dir, (generation, task.abort_handle()));
}

/// Tests: the delivery task ends as the process would.
#[cfg(test)]
pub fn stop_delivery(paths: &Paths) {
    if let Some((_, task)) = RUNNING.lock().expect("dispute deliveries").get(&agent_dir(paths)) {
        task.abort();
    }
}

/// Tests: whether a delivery task runs for `paths`.
#[cfg(test)]
pub fn delivering(paths: &Paths) -> bool {
    RUNNING.lock().expect("dispute deliveries").contains_key(&agent_dir(paths))
}

async fn deliveries(paths: Paths, guard: Running) {
    crate::agent::traffic::install_client_tap();
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default();
    let mut wait = RETRY_FIRST;
    loop {
        let pending: Vec<(PathBuf, Statement)> = {
            // Checked and given up under the lock, so a statement kept right
            // after is never left without a delivery ([`deliver`]).
            let mut running = RUNNING.lock().expect("dispute deliveries");
            let pending: Vec<_> = kept(&paths).into_iter().filter(|(_, s)| s.pending()).collect();
            if pending.is_empty() {
                if running.get(&guard.0).is_some_and(|(g, _)| *g == guard.1) {
                    running.remove(&guard.0);
                }
                return;
            }
            pending
        };
        let mut again = false;
        for (path, mut s) in pending {
            match post(&http, &s).await {
                Delivery::Acked => {
                    let at = now_ms();
                    tracing::warn!(seq = s.seq, "the hub took this device's dispute statement against the renewal that replaced its key");
                    s.acked_ms = Some(at);
                    if let Err(error) = write(&path, &s) {
                        tracing::error!(%error, "cannot note that the hub took the dispute statement");
                    }
                    note_evidence(&paths, &s, at);
                }
                Delivery::Removed => {
                    tracing::info!(seq = s.seq, "the hub says this device was removed: its dispute statement is not delivered any more");
                    s.stopped = Some("host_removed".into());
                    if let Err(error) = write(&path, &s) {
                        tracing::error!(%error, "cannot note that delivering the dispute statement stopped");
                    }
                }
                Delivery::Again(why) => {
                    tracing::warn!(seq = s.seq, %why, "cannot deliver this device's dispute statement yet; trying again in {wait:?}");
                    again = true;
                }
            }
        }
        if again {
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(RETRY_MAX);
        }
    }
}

/// The hub took `s`: noted for the local settings — in the state of the
/// enrollment it disputes, when that is still the one here (it stopped on
/// the verdict: nothing else writes it meanwhile).
fn note_evidence(paths: &Paths, s: &Statement, at: u64) {
    let Ok(Some(mut st)) = AgentState::load(paths) else { return };
    if st.account != s.account || st.machine != s.host || st.rotation.foreign_ms.is_none() {
        return;
    }
    st.rotation.evidence_ms.get_or_insert(at);
    if let Err(error) = st.save(paths) {
        tracing::error!(%error, "cannot note the delivered dispute statement");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use rh_proto::chain::{Action, Assurance, ChainClock, ChainState, DeviceInfo, DeviceV3, Flags, HostRotation, KeyCommit, KeyCommits, Signer};

    use super::*;
    use crate::agent::{FileSeedStore, SeedStore, StoredSeed};

    /// m-1's state: a chain that added it with `keys[0]` and renewed it to
    /// each of the others in turn (whoever held the key before signing).
    fn renewed(keys: &[&MachineIdentity], hub: &str) -> AgentState {
        let ak = MachineIdentity::from_seed(&[1; 32]);
        let d1 = MachineIdentity::from_seed(&[2; 32]);
        let genesis = chain::genesis_draft(
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
        let mut chain = ChainState::verify(std::slice::from_ref(&genesis), None, &mut ChainClock::history()).unwrap();
        let mut entries = vec![genesis];
        let mut push = |chain: &mut ChainState, action: Action| {
            let e = chain.draft(Signer::Device("d-1".into()), action, 2_000 + chain.seq()).sign(d1.signing_key());
            chain.apply(&e, &mut ChainClock::history()).unwrap();
            entries.push(e);
        };
        let epoch = |e: u32| {
            Action::KeyEpochs(KeyCommits {
                user: None,
                machines: vec![("m-1".into(), KeyCommit { epoch: e, hash: [e as u8; 32] })],
            })
        };
        push(
            &mut chain,
            Action::AddDevice3(DeviceV3 {
                id: "m-1".into(),
                sign_key: keys[0].sign_public(),
                dh_key: keys[0].dh_public(),
                flags: Flags { host: true, admin: false },
            }),
        );
        push(&mut chain, epoch(0));
        for (n, pair) in keys.windows(2).enumerate() {
            let (from, to) = (pair[0], pair[1]);
            let mut r = HostRotation {
                host: "m-1".into(),
                sign_key: to.sign_public(),
                dh_key: to.dh_public(),
                staged_seq: chain.seq(),
                staged_head: chain.head(),
                nonce: [n as u8 + 1; 16],
                assurance: Assurance::Auto,
                old_sig: [0; 64],
                new_sig: [0; 64],
            };
            let body = r.body("a-1", &from.sign_public(), &from.dh_public());
            r.old_sig = rh_proto::sig::sign(from.signing_key(), rh_proto::sig::HOST_ROTATE, &body);
            r.new_sig = rh_proto::sig::sign(to.signing_key(), rh_proto::sig::HOST_ROTATE, &body);
            push(&mut chain, Action::RotateHostKey(r));
            push(&mut chain, epoch(n as u32 + 1));
        }
        let mut st = AgentState::enrolling(hub, "a-1", "m-1", rh_proto::identity::fingerprint(&ak.sign_public()), 5);
        assert!(st.absorb(&entries, now_ms()).error.is_none());
        st.rotation.foreign_ms = Some(6);
        st
    }

    /// The instance, holding `held`'s seed; the state of `st` saved.
    fn instance(name: &str, held: [u8; 32], st: &AgentState) -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), name);
        let uuid = crate::identity::ensure(&paths).unwrap().uuid;
        FileSeedStore::for_instance(&paths)
            .create(&StoredSeed {
                uuid,
                seed: zeroize::Zeroizing::new(held),
            })
            .unwrap();
        st.save(&paths).unwrap();
        (dir, paths)
    }

    /// How the fake hub answers a statement.
    #[derive(Clone, Copy, Debug)]
    enum Answer {
        Status(u16),
        /// Longer than [`POST_WAIT`].
        Hang,
    }

    #[derive(Default)]
    struct Fake {
        answers: Vec<Answer>,
        posts: Vec<(String, Value, Instant)>,
    }

    /// A hub answering `POST /v1/rotations/{host}/disputes` in turn (then 200).
    async fn hub(answers: Vec<Answer>) -> (Arc<Mutex<Fake>>, String) {
        use axum::extract::{Path as UrlPath, State};
        use axum::http::StatusCode;
        use axum::Json;
        let fake = Arc::new(Mutex::new(Fake { answers, ..Fake::default() }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("ws://{}", listener.local_addr().unwrap());
        let app = axum::Router::new()
            .route(
                "/v1/rotations/{host}/disputes",
                axum::routing::post(
                    |State(fake): State<Arc<Mutex<Fake>>>, UrlPath(host): UrlPath<String>, Json(body): Json<Value>| async move {
                        let answer = {
                            let mut f = fake.lock().unwrap();
                            f.posts.push((host, body, Instant::now()));
                            if f.answers.is_empty() {
                                Answer::Status(200)
                            } else {
                                f.answers.remove(0)
                            }
                        };
                        let status = match answer {
                            Answer::Status(s) => s,
                            Answer::Hang => {
                                tokio::time::sleep(POST_WAIT * 3).await;
                                200
                            }
                        };
                        let body = if status == 200 { json!({"ok": true}) } else { json!({"error": "unavailable"}) };
                        (StatusCode::from_u16(status).unwrap(), Json(body))
                    },
                ),
            )
            .with_state(fake.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (fake, origin)
    }

    async fn until(what: &str, f: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !f() {
            assert!(Instant::now() < deadline, "never: {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// The statement disputes the renewal that retired the key held — however
    /// many came after — and verifies with rh-proto on the chain; none for a
    /// key the chain did not retire.
    #[test]
    fn the_statement_disputes_the_renewal_that_retired_the_key_held() {
        let (k0, k1, k2) = (MachineIdentity::from_seed(&[4; 32]), MachineIdentity::from_seed(&[5; 32]), MachineIdentity::from_seed(&[6; 32]));
        let st = renewed(&[&k0, &k1, &k2], "wss://hub.example");
        let chain = st.chain.as_ref().unwrap();
        let retired = &chain.device_record("m-1").unwrap().retired;
        let (first, later) = (retired[0].until_seq, retired[1].until_seq);
        let s = statement(&st, &k0, 77).expect("K0 was retired");
        assert_eq!((s.seq, s.created_ms, s.host.as_str(), s.account.as_str()), (first, 77, "m-1", "a-1"));
        assert_eq!(s.entry_hash, hex(&chain.hash_at(first).unwrap()));
        assert_eq!(s.new_sign, hex(&k1.sign_public()), "the key that renewal installed, not the latest");
        let sig: [u8; 64] = crate::agent::link::unhex(&s.sig).unwrap().try_into().unwrap();
        assert!(chain.verify_rotate_dispute("m-1", first, &sig), "it verifies with rh-proto on the chain");
        assert!(!chain.verify_rotate_dispute("m-1", later, &sig));
        // The holder of K1 disputes the second renewal.
        let second = statement(&st, &k1, 77).unwrap();
        assert_eq!((second.seq, second.new_sign.as_str()), (later, hex(&k2.sign_public()).as_str()));
        // The key now, or one never on the chain: nothing to dispute.
        assert!(statement(&st, &k2, 77).is_none());
        assert!(statement(&st, &MachineIdentity::from_seed(&[9; 32]), 77).is_none());
        assert!(statement(&AgentState::enrolling("wss://hub.example", "a-1", "m-1", [0; 16], 5), &k0, 77).is_none());
    }

    /// Kept before anything is sent, once per renewal; delivered — no session
    /// — again after the hub's 5xx and after a timeout, 30 s doubling (here
    /// shortened), until it says 200; then noted on the file and as
    /// `evidence_ms`, and not delivered again.
    #[tokio::test]
    async fn a_statement_is_delivered_again_until_the_hub_takes_it() {
        let (k0, k1) = (MachineIdentity::from_seed(&[4; 32]), MachineIdentity::from_seed(&[5; 32]));
        let (fake, origin) = hub(vec![Answer::Status(503), Answer::Hang, Answer::Status(500)]).await;
        let st = renewed(&[&k0, &k1], &origin);
        let (_dir, paths) = instance("disputeretry", [4; 32], &st);
        let s = statement(&st, &k0, now_ms()).unwrap();
        let before = crate::agent::dir_syncs();
        assert!(keep(&paths, &s).unwrap());
        assert!(crate::agent::dir_syncs() > before, "flushed before it is sent");
        assert!(!keep(&paths, &s).unwrap(), "one per renewal");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(path(&paths, s.seq)).unwrap().permissions().mode() & 0o777, 0o600);
        }
        deliver(&paths);
        until("taken", || kept(&paths).first().is_some_and(|(_, s)| s.acked_ms.is_some())).await;
        let posts: Vec<(String, Value, Instant)> = fake.lock().unwrap().posts.clone();
        assert_eq!(posts.len(), 4, "503, a timeout, 500, then 200");
        for (host, body, _) in &posts {
            assert_eq!(host, "m-1");
            assert_eq!(body, &json!({"seq": s.seq, "entry_hash": s.entry_hash, "sig": s.sig}));
        }
        // Waited between tries, longer each time.
        let gaps: Vec<Duration> = posts.windows(2).map(|w| w[1].2 - w[0].2).collect();
        assert!(gaps[0] >= RETRY_FIRST, "{gaps:?}");
        assert!(gaps[2] >= RETRY_FIRST * 4, "{gaps:?}");
        assert!(AgentState::load(&paths).unwrap().unwrap().rotation.evidence_ms.is_some());
        until("the delivery ends", || !delivering(&paths)).await;
        deliver(&paths);
        tokio::time::sleep(RETRY_MAX).await;
        assert_eq!(fake.lock().unwrap().posts.len(), 4, "taken: not sent again");
    }

    /// A process that ends before the hub took it: delivered again at the next
    /// start (what `Control::attach` calls), from the file alone.
    #[tokio::test]
    async fn a_statement_is_delivered_again_after_a_restart() {
        let (k0, k1) = (MachineIdentity::from_seed(&[4; 32]), MachineIdentity::from_seed(&[5; 32]));
        let (fake, origin) = hub(vec![Answer::Status(503); 50]).await;
        let st = renewed(&[&k0, &k1], &origin);
        let (_dir, paths) = instance("disputerestart", [4; 32], &st);
        keep(&paths, &statement(&st, &k0, now_ms()).unwrap()).unwrap();
        deliver(&paths);
        until("a first try", || !fake.lock().unwrap().posts.is_empty()).await;
        stop_delivery(&paths);
        until("the process gone", || !delivering(&paths)).await;
        assert!(kept(&paths)[0].1.pending());
        // The hub is well again; the next start delivers it.
        fake.lock().unwrap().answers.clear();
        deliver(&paths);
        until("taken after the restart", || kept(&paths)[0].1.acked_ms.is_some()).await;
    }

    /// 410 `host_removed`: nobody needs it any more — delivering stops, noted
    /// on the file, never sent again, no evidence noted.
    #[tokio::test]
    async fn a_removed_host_stops_the_delivery() {
        let (k0, k1) = (MachineIdentity::from_seed(&[4; 32]), MachineIdentity::from_seed(&[5; 32]));
        let (fake, origin) = hub(vec![Answer::Status(503), Answer::Status(410)]).await;
        let st = renewed(&[&k0, &k1], &origin);
        let (_dir, paths) = instance("disputegone", [4; 32], &st);
        keep(&paths, &statement(&st, &k0, now_ms()).unwrap()).unwrap();
        deliver(&paths);
        until("stopped", || kept(&paths)[0].1.stopped.as_deref() == Some("host_removed")).await;
        until("the delivery ends", || !delivering(&paths)).await;
        deliver(&paths);
        tokio::time::sleep(RETRY_MAX * 2).await;
        assert_eq!(fake.lock().unwrap().posts.len(), 2);
        assert!(kept(&paths)[0].1.acked_ms.is_none());
        assert!(AgentState::load(&paths).unwrap().unwrap().rotation.evidence_ms.is_none());
    }

    /// The statement needs no seed: "add this device again with a new
    /// identity" keeps it, and it is still delivered.
    #[tokio::test]
    async fn a_statement_outlives_adding_the_device_again() {
        let (k0, k1) = (MachineIdentity::from_seed(&[4; 32]), MachineIdentity::from_seed(&[5; 32]));
        let (fake, origin) = hub(vec![Answer::Status(503); 50]).await;
        let st = renewed(&[&k0, &k1], &origin);
        let (_dir, paths) = instance("disputefresh", [4; 32], &st);
        let s = statement(&st, &k0, now_ms()).unwrap();
        keep(&paths, &s).unwrap();
        let fresh = crate::agent::rotation::re_enroll_fresh(&paths, 99).unwrap();
        assert_ne!(fresh.sign_public(), k0.sign_public());
        assert!(!crate::agent::state::exists(&paths), "the enrollment is forgotten");
        assert_eq!(kept(&paths), vec![(path(&paths, s.seq), s.clone())], "the statement is not");
        fake.lock().unwrap().answers.clear();
        deliver(&paths);
        until("taken", || kept(&paths)[0].1.acked_ms.is_some()).await;
        assert_eq!(fake.lock().unwrap().posts[0].1["sig"], json!(s.sig));
    }
}
