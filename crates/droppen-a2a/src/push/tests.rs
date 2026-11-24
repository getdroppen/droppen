//! Push delivery end to end: the real store, engine and client, the fake
//! backend, a resolver the tests answer for, and webhooks of the tests' own:
//! HTTPS under a root made when the test runs, or plain http for the TCK's
//! exception.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::rustls;
use tokio_util::sync::CancellationToken;

use droppen_base::time::now_ms;
use droppen_instance::db::Db;

use super::guard::Lookup;
use super::{PushSettings, Pusher, Sleep};
use crate::backend::{event_channel, Question, QuestionItem, QuestionOption};
use crate::engine::{A2a, Caller, Sent, Settings};
use crate::store::Store;
use crate::testing::{Call, FakeBackend};
use crate::wire::TaskState;

/* ── the webhook ─────────────────────────────────────────────────────── */

/// A test root and a webhook certificate it signed, for `hooks.test`,
/// `localhost`, `127.0.0.1` and `::1`.
struct Pki {
    root: Vec<u8>,
    server: Arc<rustls::ServerConfig>,
}

fn pki() -> Pki {
    use rcgen::{BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose, SanType};
    let root_key = KeyPair::generate().expect("a root key");
    let mut root = CertificateParams::new(Vec::<String>::new()).expect("root parameters");
    root.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    root.distinguished_name.push(DnType::CommonName, "Droppen push test root");
    root.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
    let root_cert = root.self_signed(&root_key).expect("the root");
    let issuer = Issuer::new(root, root_key);
    let leaf_key = KeyPair::generate().expect("a webhook key");
    let mut leaf = CertificateParams::new(vec!["hooks.test".to_string(), "localhost".to_string()]).expect("webhook parameters");
    leaf.subject_alt_names.push(SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    leaf.subject_alt_names.push(SanType::IpAddress(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let leaf_cert = leaf.signed_by(&leaf_key, &issuer).expect("the webhook's certificate");
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let server = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_no_client_auth()
        .with_single_cert(
            vec![leaf_cert.der().clone()],
            rustls::pki_types::PrivateKeyDer::Pkcs8(leaf_key.serialize_der().into()),
        )
        .expect("the webhook's TLS");
    Pki {
        root: root_cert.der().to_vec(),
        server: Arc::new(server),
    }
}

/// One request a webhook got.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    target: String,
    headers: HashMap<String, String>,
    body: String,
}

/// How a webhook answers.
#[derive(Debug, Clone)]
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    close: bool,
}

impl Reply {
    fn status(status: u16) -> Self {
        Reply {
            status,
            headers: Vec::new(),
            close: false,
        }
    }
}

#[derive(Default)]
struct HookState {
    seen: Mutex<Vec<Seen>>,
    replies: Mutex<VecDeque<Reply>>,
    always: Mutex<Option<Reply>>,
    connections: AtomicUsize,
}

impl HookState {
    fn reply(&self) -> Reply {
        let next = self.replies.lock().unwrap().pop_front();
        next.or_else(|| self.always.lock().unwrap().clone()).unwrap_or(Reply::status(200))
    }
}

/// A webhook: writes down every request and answers each as scripted (200
/// when nothing is).
#[derive(Clone)]
struct Hook {
    addr: SocketAddr,
    tls: bool,
    state: Arc<HookState>,
}

impl Hook {
    async fn https(pki: &Pki) -> Hook {
        Self::bind(SocketAddr::from(([127, 0, 0, 1], 0)), Some(pki)).await.expect("a webhook")
    }

    async fn plain() -> Hook {
        Self::bind(SocketAddr::from(([127, 0, 0, 1], 0)), None).await.expect("a webhook")
    }

    async fn bind(addr: SocketAddr, pki: Option<&Pki>) -> std::io::Result<Hook> {
        let listener = TcpListener::bind(addr).await?;
        let hook = Hook {
            addr: listener.local_addr()?,
            tls: pki.is_some(),
            state: Arc::new(HookState::default()),
        };
        let acceptor = pki.map(|pki| tokio_rustls::TlsAcceptor::from(pki.server.clone()));
        let state = hook.state.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                state.connections.fetch_add(1, Ordering::SeqCst);
                let state = state.clone();
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    match acceptor {
                        Some(acceptor) => {
                            if let Ok(stream) = acceptor.accept(stream).await {
                                serve(stream, state).await;
                            }
                        }
                        None => serve(stream, state).await,
                    }
                });
            }
        });
        Ok(hook)
    }

    fn port(&self) -> u16 {
        self.addr.port()
    }

    fn origin(&self) -> String {
        let scheme = if self.tls { "https" } else { "http" };
        match self.addr.ip() {
            IpAddr::V4(ip) => format!("{scheme}://{ip}:{}", self.port()),
            IpAddr::V6(ip) => format!("{scheme}://[{ip}]:{}", self.port()),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.origin())
    }

    fn then(&self, reply: Reply) {
        self.state.replies.lock().unwrap().push_back(reply);
    }

    fn always(&self, reply: Reply) {
        *self.state.always.lock().unwrap() = Some(reply);
    }

    fn seen(&self) -> Vec<Seen> {
        self.state.seen.lock().unwrap().clone()
    }

    fn bodies(&self) -> Vec<String> {
        self.seen().into_iter().map(|seen| seen.body).collect()
    }

    fn connections(&self) -> usize {
        self.state.connections.load(Ordering::SeqCst)
    }

    /// Wait until it got `n` requests; fails the test after ten seconds.
    async fn wait_for(&self, n: usize) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while self.seen().len() < n {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the webhook got {:?}, not {n} requests",
                self.seen()
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

/// HTTP/1.1, as much of it as a push needs: requests with a
/// `content-length`, kept alive until a reply says otherwise.
async fn serve<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S, state: Arc<HookState>) {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let head_end = loop {
            if let Some(at) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
                break at;
            }
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
        buf.drain(..head_end + 4);
        let mut lines = head.split("\r\n");
        let mut request_line = lines.next().unwrap_or_default().split(' ');
        let method = request_line.next().unwrap_or_default().to_string();
        let target = request_line.next().unwrap_or_default().to_string();
        let headers: HashMap<String, String> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
            .collect();
        let length: usize = headers.get("content-length").and_then(|value| value.parse().ok()).unwrap_or(0);
        while buf.len() < length {
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        let body: Vec<u8> = buf.drain(..length).collect();
        let reply = state.reply();
        state.seen.lock().unwrap().push(Seen {
            method,
            target,
            headers,
            body: String::from_utf8_lossy(&body).into_owned(),
        });
        let mut out = format!("HTTP/1.1 {} Test\r\ncontent-length: 2\r\n", reply.status);
        for (name, value) in &reply.headers {
            out.push_str(&format!("{name}: {value}\r\n"));
        }
        if reply.close {
            out.push_str("connection: close\r\n");
        }
        out.push_str("\r\nok");
        if stream.write_all(out.as_bytes()).await.is_err() || stream.flush().await.is_err() {
            return;
        }
        if reply.close {
            let _ = stream.shutdown().await;
            return;
        }
    }
}

/* ── the resolver and the clock ──────────────────────────────────────── */

/// Host names as a test says they resolve: each name's answers in turn, the
/// last one again after that. A name with no answer does not resolve.
#[derive(Default)]
struct Names {
    answers: Mutex<HashMap<String, VecDeque<Vec<IpAddr>>>>,
    asked: Mutex<Vec<String>>,
}

impl Names {
    fn answer(&self, host: &str, answers: &[&[IpAddr]]) {
        self.answers
            .lock()
            .unwrap()
            .insert(host.to_string(), answers.iter().map(|answer| answer.to_vec()).collect());
    }

    fn asked(&self, host: &str) -> usize {
        self.asked.lock().unwrap().iter().filter(|asked| *asked == host).count()
    }
}

impl Lookup for Names {
    fn lookup(&self, host: String) -> BoxFuture<'static, std::io::Result<Vec<IpAddr>>> {
        self.asked.lock().unwrap().push(host.clone());
        let mut answers = self.answers.lock().unwrap();
        let answer = match answers.get_mut(&host) {
            Some(answers) if answers.len() > 1 => answers.pop_front(),
            Some(answers) => answers.front().cloned(),
            None => None,
        };
        drop(answers);
        Box::pin(async move { answer.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no such test host")) })
    }
}

/// What a test does as a pause starts.
type Then = Box<dyn Fn() + Send>;

/// The sender's pauses: written down, and taken at once unless held (a
/// pause of zero never is). `then` runs as each pause starts.
#[derive(Clone)]
struct Pauses {
    taken: Arc<Mutex<Vec<Duration>>>,
    held: Arc<tokio::sync::watch::Sender<bool>>,
    then: Arc<Mutex<Option<Then>>>,
}

impl Pauses {
    fn new() -> Self {
        Pauses {
            taken: Arc::default(),
            held: Arc::new(tokio::sync::watch::channel(false).0),
            then: Arc::default(),
        }
    }

    fn sleep(&self) -> Sleep {
        let pauses = self.clone();
        Arc::new(move |pause| {
            pauses.taken.lock().unwrap().push(pause);
            let then = pauses.then.lock().unwrap();
            if let Some(then) = then.as_ref() {
                then();
            }
            drop(then);
            let mut held = pauses.held.subscribe();
            Box::pin(async move {
                if !pause.is_zero() {
                    let _ = held.wait_for(|held| !*held).await;
                }
            })
        })
    }

    fn taken(&self) -> Vec<Duration> {
        self.taken.lock().unwrap().clone()
    }

    fn hold(&self, held: bool) {
        self.held.send_replace(held);
    }
}

/* ── the server ──────────────────────────────────────────────────────── */

const TEN: Duration = Duration::from_secs(10);
const SIXTY: Duration = Duration::from_secs(60);

/// A server over the database in a directory, with a pusher whose
/// resolver, clock and roots are the test's.
struct World {
    a2a: Arc<A2a>,
    fake: Arc<FakeBackend>,
    pusher: Arc<Pusher>,
    cancel: CancellationToken,
    caller: Caller,
    token: String,
}

/// What every server of one test shares.
struct Rig {
    dir: tempfile::TempDir,
    pki: Pki,
    names: Arc<Names>,
    pauses: Pauses,
    /// The token's secret, once the first start made it.
    token: Mutex<Option<String>>,
}

impl Rig {
    fn new() -> Rig {
        Rig {
            dir: tempfile::tempdir().expect("tempdir"),
            pki: pki(),
            names: Arc::new(Names::default()),
            pauses: Pauses::new(),
            token: Mutex::new(None),
        }
    }

    fn settings(&self) -> PushSettings {
        PushSettings {
            timeout: Duration::from_secs(5),
            retries: super::RETRIES.to_vec(),
            lookup: self.names.clone(),
            roots: vec![reqwest::Certificate::from_der(&self.pki.root).expect("the test root")],
            sleep: self.pauses.sleep(),
        }
    }

    /// A server over the rig's database (`tck`: `DROPPEN_A2A_TCK=1`), with an
    /// entry made from `entry` when it has none of that id yet, and a token
    /// for it.
    async fn start(&self, entry: Value, tck: bool) -> World {
        let db = Arc::new(Db::open(self.dir.path().join("state.db")));
        let (events, inbox) = event_channel();
        let fake = FakeBackend::new(events);
        let cancel = CancellationToken::new();
        let settings = Settings {
            version: "0.0.0-test".into(),
            public_base: None,
            tck,
            tck_streaming_timeout: Duration::from_millis(300),
        };
        let a2a = A2a::new(Store::new(db), fake.clone(), inbox, settings, cancel.clone());
        let pusher = Pusher::attach_with(&a2a, self.settings());
        let id = entry["id"].as_str().expect("an entry id").to_string();
        if a2a.entry(&id).is_none() {
            a2a.create_entry(serde_json::from_value(entry).expect("an entry body"))
                .await
                .expect("the entry is made");
        }
        let made = self.token.lock().unwrap().clone();
        let token = match made {
            Some(secret) => secret,
            None => {
                let secret = a2a
                    .create_token(serde_json::from_value(json!({"name": "dont-do", "entries": [id]})).unwrap())
                    .expect("a token")
                    .secret;
                *self.token.lock().unwrap() = Some(secret.clone());
                secret
            }
        };
        let caller = a2a
            .authorize(Some(&format!("Bearer {token}")), Some(IpAddr::V4(Ipv4Addr::LOCALHOST)), &id)
            .expect("the token reaches the entry");
        World {
            a2a,
            fake,
            pusher,
            cancel,
            caller,
            token,
        }
    }
}

fn claude_entry(push_origins: &[String]) -> Value {
    json!({"id": "main", "name": "Main", "source": "claude", "cwd": "/work", "enabled": true, "isolation": "none", "pushOrigins": push_origins})
}

fn message(id: &str, text: &str) -> Value {
    json!({"messageId": id, "role": "ROLE_USER", "parts": [{"text": text, "mediaType": "text/plain"}]})
}

fn question() -> Question {
    Question {
        request_id: "req-1".into(),
        questions: vec![QuestionItem {
            question: "Which branch?".into(),
            header: "Branch".into(),
            multi_select: false,
            options: vec![QuestionOption {
                label: "main".into(),
                description: "the default".into(),
            }],
        }],
    }
}

/// What a pushed body is: `status <state>` or `artifact <id>`.
fn what(body: &str) -> String {
    let value: Value = serde_json::from_str(body).unwrap_or_else(|_| panic!("a push is JSON: {body}"));
    if let Some(state) = value["statusUpdate"]["status"]["state"].as_str() {
        return format!("status {state}");
    }
    if let Some(id) = value["artifactUpdate"]["artifact"]["artifactId"].as_str() {
        return format!("artifact {id}");
    }
    panic!("a push is a status or an artifact update: {body}")
}

impl World {
    async fn send(&self, body: Value) -> String {
        let request = serde_json::from_value(body).expect("a send request");
        match self.a2a.send(&self.caller, request, false).await {
            Ok(Sent::Task { follow, .. }) => follow.task_id().to_string(),
            Ok(Sent::Message(_)) => panic!("a message, not a task"),
            Err(error) => panic!("the send was refused: {error}"),
        }
    }

    /// A new task, sent without waiting, with `push` as its push
    /// configuration if given.
    async fn new_task(&self, id: &str, push: Option<Value>) -> String {
        let mut configuration = json!({"returnImmediately": true});
        if let Some(push) = push {
            configuration["taskPushNotificationConfig"] = push;
        }
        self.send(json!({"message": message(id, "fix the bug"), "configuration": configuration}))
            .await
    }

    fn add_push(&self, task_id: &str, mut push: Value) {
        push["taskId"] = json!(task_id);
        self.a2a
            .create_push(&self.caller, task_id, serde_json::from_value(push).unwrap())
            .expect("the push configuration is kept");
    }

    fn state(&self, task_id: &str) -> TaskState {
        self.a2a.store().task(task_id).expect("the task").state
    }

    async fn wait_state(&self, task_id: &str, state: TaskState) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while self.state(task_id) != state {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the task never became {state:?}; it is {:?}",
                self.state(task_id)
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// The task, handed to the backend and taken into its turn.
    async fn working(&self, task_id: &str) {
        let id = task_id.to_string();
        self.fake
            .wait_for(move |call| matches!(call, Call::Start(start) if start.task.task_id == id))
            .await;
        self.fake.turn_started(task_id);
        self.wait_state(task_id, TaskState::Working).await;
    }

    async fn completed(&self, task_id: &str) {
        self.fake.complete(task_id, "Fixed it.");
        self.wait_state(task_id, TaskState::Completed).await;
    }

    /// The task's events a push carries, in order: all but the progress.
    fn pushable(&self, task_id: &str) -> Vec<String> {
        self.a2a
            .store()
            .events_after(task_id, 0)
            .into_iter()
            .filter(|event| event.kind != "progress")
            .map(|event| event.payload)
            .collect()
    }

    fn cursor(&self, task_id: &str, config_id: &str) -> Option<i64> {
        self.a2a.store().push_cursor(task_id, config_id)
    }

    fn token_id(&self) -> String {
        crate::token::parse(&self.token).expect("a token").0.to_string()
    }
}

fn push_to(id: &str, url: &str) -> Value {
    json!({"id": id, "url": url})
}

/* ── what goes, in what order ────────────────────────────────────────── */

#[tokio::test]
async fn a_whole_task_is_pushed_in_order_with_its_headers() {
    let rig = Rig::new();
    let hook = Hook::https(&rig.pki).await;
    let w = rig.start(claude_entry(&[hook.origin()]), false).await;
    let push = json!({
        "id": "hook-1",
        "url": hook.url("/a2a/push?entry=main"),
        "token": "notify-secret",
        "authentication": {"scheme": "Bearer", "credentials": "hook-credentials"},
    });
    let task = w.new_task("m-1", Some(push)).await;
    w.working(&task).await;
    w.fake.progress(&task, "reading the code");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !w.a2a.store().events_after(&task, 0).iter().any(|event| event.kind == "progress") {
        assert!(tokio::time::Instant::now() < deadline, "the progress was never written");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    w.fake.ask(&task, question());
    w.wait_state(&task, TaskState::InputRequired).await;
    let answer = json!({"messageId": "m-2", "role": "ROLE_USER", "taskId": task, "parts": [{"text": "main"}]});
    w.send(json!({"message": answer})).await;
    w.wait_state(&task, TaskState::Working).await;
    w.completed(&task).await;
    w.pusher.settled().await;

    let bodies = hook.bodies();
    assert_eq!(bodies, w.pushable(&task), "every event but the progress, as kept, in order");
    assert_eq!(
        bodies.iter().map(|body| what(body)).collect::<Vec<_>>(),
        [
            "status TASK_STATE_SUBMITTED",
            "status TASK_STATE_WORKING",
            "status TASK_STATE_INPUT_REQUIRED",
            "status TASK_STATE_WORKING",
            "artifact summary",
            "artifact result",
            "status TASK_STATE_COMPLETED",
        ]
    );
    for seen in hook.seen() {
        assert_eq!(seen.method, "POST");
        assert_eq!(seen.target, "/a2a/push?entry=main");
        assert_eq!(seen.headers["content-type"], "application/a2a+json");
        assert_eq!(seen.headers["authorization"], "Bearer hook-credentials");
        assert_eq!(seen.headers["a2a-notification-token"], "notify-secret");
        assert_eq!(seen.headers["x-a2a-notification-token"], "notify-secret");
        assert_eq!(seen.headers["user-agent"], "droppen/0.0.0-test");
    }
    let last = w.a2a.store().task(&task).unwrap().seq;
    assert_eq!(w.cursor(&task, "hook-1"), Some(last), "the cursor stands at the last event");
    assert!(rig.pauses.taken().is_empty(), "nothing was tried twice");
}

#[tokio::test]
async fn a_configuration_added_later_is_told_what_happens_from_then_on() {
    let rig = Rig::new();
    let hook = Hook::https(&rig.pki).await;
    let w = rig.start(claude_entry(&[hook.origin()]), false).await;
    let task = w.new_task("m-1", None).await;
    w.working(&task).await;
    w.add_push(&task, json!({"id": "late", "url": hook.url("/late")}));
    w.completed(&task).await;
    w.pusher.settled().await;
    assert_eq!(
        hook.bodies().iter().map(|body| what(body)).collect::<Vec<_>>(),
        ["artifact summary", "artifact result", "status TASK_STATE_COMPLETED"]
    );
    assert!(hook.seen().iter().all(|seen| !seen.headers.contains_key("authorization")
        && !seen.headers.contains_key("a2a-notification-token")
        && !seen.headers.contains_key("x-a2a-notification-token")));
}

/* ── retries ─────────────────────────────────────────────────────────── */

#[tokio::test]
async fn a_failed_push_is_tried_again_at_once_then_after_ten_and_sixty_seconds() {
    let rig = Rig::new();
    let hook = Hook::https(&rig.pki).await;
    for status in [500, 503, 502] {
        hook.then(Reply::status(status));
    }
    let w = rig.start(claude_entry(&[hook.origin()]), false).await;
    let task = w.new_task("m-1", Some(push_to("hook-1", &hook.url("/a2a")))).await;
    hook.wait_for(4).await;
    w.pusher.settled().await;
    let first = w.pushable(&task)[0].clone();
    assert_eq!(hook.bodies(), vec![first; 4], "the same event, four times");
    assert_eq!(rig.pauses.taken(), [Duration::ZERO, TEN, SIXTY]);
    assert_eq!(w.cursor(&task, "hook-1"), Some(1), "delivered on the last retry");

    w.working(&task).await;
    w.pusher.settled().await;
    assert_eq!(hook.seen().len(), 5, "the next event goes at its first attempt");
    assert_eq!(rig.pauses.taken().len(), 3);
}

#[tokio::test]
async fn after_the_last_retry_the_event_is_passed_over_and_the_next_one_goes() {
    let rig = Rig::new();
    let hook = Hook::https(&rig.pki).await;
    for _ in 0..4 {
        hook.then(Reply::status(500));
    }
    let w = rig.start(claude_entry(&[hook.origin()]), false).await;
    let task = w.new_task("m-1", Some(push_to("hook-1", &hook.url("/a2a")))).await;
    hook.wait_for(4).await;
    w.pusher.settled().await;
    assert_eq!(w.cursor(&task, "hook-1"), Some(1), "given up on, and passed over");
    w.working(&task).await;
    w.pusher.settled().await;
    let pushable = w.pushable(&task);
    let mut expected = vec![pushable[0].clone(); 4];
    expected.push(pushable[1].clone());
    assert_eq!(hook.bodies(), expected);
    assert_eq!(w.cursor(&task, "hook-1"), Some(2));
}

#[tokio::test]
async fn a_dead_webhook_holds_up_no_other_configuration() {
    let rig = Rig::new();
    let dead = Hook::https(&rig.pki).await;
    dead.always(Reply::status(503));
    let live = Hook::https(&rig.pki).await;
    let w = rig.start(claude_entry(&[dead.origin(), live.origin()]), false).await;
    rig.pauses.hold(true);
    let task = w.new_task("m-1", None).await;
    w.add_push(&task, push_to("dead", &dead.url("/a2a")));
    w.add_push(&task, push_to("live", &live.url("/a2a")));
    w.working(&task).await;
    w.completed(&task).await;
    live.wait_for(4).await;
    dead.wait_for(2).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(dead.seen().len(), 2, "the dead one waits out its ten seconds");
    assert_eq!(
        live.bodies().iter().map(|body| what(body)).collect::<Vec<_>>(),
        [
            "status TASK_STATE_WORKING",
            "artifact summary",
            "artifact result",
            "status TASK_STATE_COMPLETED"
        ],
        "while the other is told everything"
    );

    rig.pauses.hold(false);
    w.pusher.settled().await;
    let expected: Vec<String> = w.pushable(&task)[1..].iter().flat_map(|body| vec![body.clone(); 4]).collect();
    assert_eq!(dead.bodies(), expected, "each event tried four times, in order");
    assert_eq!(w.cursor(&task, "dead"), w.cursor(&task, "live"));
}

/* ── a restart ───────────────────────────────────────────────────────── */

/// A sink that hears nothing: the pusher taken off a server.
struct Deaf;

impl crate::TaskEventSink for Deaf {
    fn task_event(&self, _event: &crate::TaskEvent) {}
}

#[tokio::test]
async fn a_restart_delivers_what_was_left_from_the_cursor() {
    let rig = Rig::new();
    let hook = Hook::https(&rig.pki).await;
    let entry = claude_entry(&[hook.origin()]);
    let first = rig.start(entry.clone(), false).await;
    let task = first.new_task("m-1", Some(push_to("hook-1", &hook.url("/a2a")))).await;
    first.working(&task).await;
    first.pusher.settled().await;
    assert_eq!(hook.seen().len(), 2);
    assert_eq!(first.cursor(&task, "hook-1"), Some(2));

    // The server stops delivering, and the task goes on without it.
    first.a2a.set_event_sink(Arc::new(Deaf));
    first.completed(&task).await;
    assert_eq!(hook.seen().len(), 2);
    first.cancel.cancel();

    let second = rig.start(entry.clone(), false).await;
    assert_eq!(hook.seen().len(), 2, "nothing goes before the server takes its instance over");
    second.a2a.resume().await;
    second.pusher.settled().await;
    assert_eq!(hook.bodies(), second.pushable(&task), "the rest, from the cursor on, and nothing twice");
    assert_eq!(second.cursor(&task, "hook-1"), Some(5));
    second.cancel.cancel();

    let third = rig.start(entry, false).await;
    third.a2a.resume().await;
    third.pusher.settled().await;
    assert_eq!(hook.seen().len(), 5, "a delivered event does not go again");
}

/* ── who is told ─────────────────────────────────────────────────────── */

#[tokio::test]
async fn a_caller_whose_token_is_revoked_is_told_nothing_more() {
    let rig = Rig::new();
    let hook = Hook::https(&rig.pki).await;
    let w = rig.start(claude_entry(&[hook.origin()]), false).await;
    let task = w.new_task("m-1", Some(push_to("hook-1", &hook.url("/a2a")))).await;
    w.pusher.settled().await;
    assert_eq!(hook.seen().len(), 1);

    // Revoked in the table only: the configuration stays, and it is the
    // check before each attempt that stops the push.
    w.a2a.store().revoke_token(&w.token_id(), now_ms()).unwrap();
    w.working(&task).await;
    w.completed(&task).await;
    w.pusher.settled().await;
    assert_eq!(hook.seen().len(), 1, "nothing after the revocation");
    assert!(w.a2a.store().push(&task, "hook-1").is_some());
    assert_eq!(w.cursor(&task, "hook-1"), Some(1), "and nothing passed over either");
}

#[tokio::test]
async fn revoking_a_token_pushes_nothing_of_the_cancellation() {
    let rig = Rig::new();
    let hook = Hook::https(&rig.pki).await;
    let w = rig.start(claude_entry(&[hook.origin()]), false).await;
    let task = w.new_task("m-1", Some(push_to("hook-1", &hook.url("/a2a")))).await;
    w.working(&task).await;
    w.pusher.settled().await;
    assert_eq!(hook.seen().len(), 2);

    w.a2a.revoke_token(&w.token_id()).await.unwrap();
    w.wait_state(&task, TaskState::Canceled).await;
    w.pusher.settled().await;
    assert_eq!(hook.seen().len(), 2, "the revoked caller is not told of its task's end");
    assert!(w.a2a.store().pushes(&task).is_empty());
}

#[tokio::test]
async fn a_deleted_configuration_gets_nothing_more() {
    let rig = Rig::new();
    let hook = Hook::https(&rig.pki).await;
    let w = rig.start(claude_entry(&[hook.origin()]), false).await;
    let task = w.new_task("m-1", Some(push_to("hook-1", &hook.url("/a2a")))).await;
    w.pusher.settled().await;
    assert_eq!(hook.seen().len(), 1);
    w.a2a.delete_push(&w.caller, &task, "hook-1").unwrap();
    w.working(&task).await;
    w.completed(&task).await;
    w.pusher.settled().await;
    assert_eq!(hook.seen().len(), 1);
}

#[tokio::test]
async fn a_configuration_deleted_between_attempts_is_not_tried_again() {
    let rig = Rig::new();
    let hook = Hook::https(&rig.pki).await;
    hook.always(Reply::status(500));
    let w = rig.start(claude_entry(&[hook.origin()]), false).await;
    let store = w.a2a.store().clone();
    *rig.pauses.then.lock().unwrap() = Some(Box::new(move || {
        for task in store.open_tasks() {
            store.delete_pushes(&task.id).unwrap();
        }
    }));
    w.new_task("m-1", Some(push_to("hook-1", &hook.url("/a2a")))).await;
    hook.wait_for(1).await;
    w.pusher.settled().await;
    assert_eq!(hook.seen().len(), 1, "one attempt, and none after the deletion");
    assert_eq!(rig.pauses.taken(), [Duration::ZERO]);
}

/* ── where a push may go ─────────────────────────────────────────────── */

#[tokio::test]
async fn a_name_that_resolves_to_a_private_address_is_never_connected_to() {
    let rig = Rig::new();
    let hook = Hook::https(&rig.pki).await;
    rig.names.answer("hooks.test", &[&[IpAddr::V4(Ipv4Addr::LOCALHOST)]]);
    let origin = format!("https://hooks.test:{}", hook.port());
    let w = rig.start(claude_entry(std::slice::from_ref(&origin)), false).await;
    let task = w.new_task("m-1", Some(push_to("hook-1", &format!("{origin}/a2a")))).await;
    w.pusher.settled().await;
    assert_eq!(hook.connections(), 0, "the webhook listens there, and is never reached");
    assert_eq!(rig.names.asked("hooks.test"), 4, "every attempt looked the name up again");
    assert_eq!(w.cursor(&task, "hook-1"), Some(1), "given up on");
}

/// A listener at `port` on another loopback address than 127.0.0.1: `::1`,
/// or `127.0.0.2` where there is no IPv6.
async fn second_loopback(pki: &Pki, port: u16) -> Option<Hook> {
    for ip in [IpAddr::V6(Ipv6Addr::LOCALHOST), IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2))] {
        if let Ok(hook) = Hook::bind(SocketAddr::new(ip, port), Some(pki)).await {
            return Some(hook);
        }
    }
    None
}

#[tokio::test]
async fn a_name_rebound_to_a_refused_address_is_refused_on_the_next_connection() {
    let rig = Rig::new();
    let hook = Hook::https(&rig.pki).await;
    hook.always(Reply {
        close: true,
        ..Reply::status(200)
    });
    let trap = second_loopback(&rig.pki, hook.port()).await;
    let rebound = trap.as_ref().map_or(IpAddr::V6(Ipv6Addr::LOCALHOST), |trap| trap.addr.ip());
    // The first answer is an address the entry names, standing in for a
    // public one; the next is a loopback address it does not name.
    rig.names.answer("hooks.test", &[&[IpAddr::V4(Ipv4Addr::LOCALHOST)], &[rebound]]);
    let origin = format!("https://hooks.test:{}", hook.port());
    let w = rig.start(claude_entry(&[origin.clone(), hook.origin()]), false).await;
    let task = w.new_task("m-1", Some(push_to("hook-1", &format!("{origin}/a2a")))).await;
    w.pusher.settled().await;
    assert_eq!(hook.seen().len(), 1, "the first answer was let through");

    w.working(&task).await;
    w.pusher.settled().await;
    assert_eq!(hook.seen().len(), 1);
    assert_eq!(rig.names.asked("hooks.test"), 5, "a new connection, a new lookup, each checked");
    match trap {
        Some(trap) => assert_eq!(trap.connections(), 0, "the rebound address was never connected to"),
        None => eprintln!("no second loopback address here: the rebound address was unreachable anyway"),
    }
}

#[tokio::test]
async fn a_redirect_is_not_followed() {
    let rig = Rig::new();
    let hook = Hook::https(&rig.pki).await;
    let elsewhere = Hook::https(&rig.pki).await;
    hook.always(Reply {
        headers: vec![("location".into(), elsewhere.url("/stolen"))],
        ..Reply::status(307)
    });
    let w = rig.start(claude_entry(&[hook.origin(), elsewhere.origin()]), false).await;
    let task = w.new_task("m-1", Some(push_to("hook-1", &hook.url("/a2a")))).await;
    w.pusher.settled().await;
    assert_eq!(hook.seen().len(), 4, "a redirect is a failed attempt");
    assert_eq!(elsewhere.connections(), 0, "and where it points is never asked");
    assert_eq!(w.cursor(&task, "hook-1"), Some(1));
}

#[tokio::test]
async fn the_proxy_environment_is_ignored() {
    let rig = Rig::new();
    let proxy = Hook::plain().await;
    for name in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"] {
        std::env::set_var(name, proxy.origin());
    }
    for name in ["NO_PROXY", "no_proxy"] {
        std::env::remove_var(name);
    }
    let hook = Hook::https(&rig.pki).await;
    rig.names.answer("hooks.test", &[&[IpAddr::V4(Ipv4Addr::LOCALHOST)]]);
    let origin = format!("https://hooks.test:{}", hook.port());
    let w = rig.start(claude_entry(&[origin.clone(), hook.origin()]), false).await;
    w.new_task("m-1", Some(push_to("hook-1", &format!("{origin}/a2a")))).await;
    w.pusher.settled().await;
    assert_eq!(hook.seen().len(), 1, "straight to the webhook");
    assert_eq!(proxy.connections(), 0, "never through the proxy");
}

#[tokio::test]
async fn plain_http_goes_to_the_tcks_webhook_and_no_one_elses() {
    let rig = Rig::new();
    let hook = Hook::plain().await;
    rig.names.answer("localhost", &[&[IpAddr::V4(Ipv4Addr::LOCALHOST)]]);
    let w = rig
        .start(json!({"id": "tck", "name": "TCK", "source": "tck", "enabled": true}), true)
        .await;
    let task = w.send(json!({"message": message("tck-input-required-1", "ask me")})).await;
    w.wait_state(&task, TaskState::InputRequired).await;
    let auth = json!({"scheme": "Bearer", "credentials": "tck-test-token"});
    for (id, url) in [
        ("literal", format!("http://127.0.0.1:{}/webhook", hook.port())),
        ("name", format!("http://localhost:{}/webhook", hook.port())),
    ] {
        w.a2a
            .add_push(
                &task,
                serde_json::from_value(json!({"id": id, "url": url, "authentication": auth})).unwrap(),
            )
            .unwrap();
    }
    w.pusher.settled().await;
    let seen = hook.seen();
    assert_eq!(seen.len(), 4, "each configuration told the task from its start: {seen:?}");
    assert!(seen.iter().all(|seen| seen.headers["authorization"] == "Bearer tck-test-token"));
    assert_eq!(
        hook.bodies().iter().map(|body| what(body)).collect::<std::collections::BTreeSet<_>>(),
        ["status TASK_STATE_SUBMITTED".to_string(), "status TASK_STATE_INPUT_REQUIRED".to_string()].into()
    );
}

#[tokio::test]
async fn plain_http_is_refused_for_an_entry_that_is_not_the_tcks() {
    let rig = Rig::new();
    let hook = Hook::plain().await;
    let w = rig.start(claude_entry(&[]), true).await;
    let task = w.new_task("m-1", None).await;
    w.a2a
        .add_push(
            &task,
            serde_json::from_value(push_to("hook-1", &format!("http://127.0.0.1:{}/webhook", hook.port()))).unwrap(),
        )
        .unwrap();
    w.working(&task).await;
    w.pusher.settled().await;
    assert_eq!(hook.connections(), 0);
    assert!(rig.pauses.taken().is_empty(), "a refused url is not tried again");
    assert_eq!(w.cursor(&task, "hook-1"), Some(2), "its events are passed over");
}

#[tokio::test]
async fn an_origin_taken_out_of_the_entry_stops_its_pushes() {
    let rig = Rig::new();
    let hook = Hook::https(&rig.pki).await;
    let w = rig.start(claude_entry(&[hook.origin()]), false).await;
    let task = w.new_task("m-1", Some(push_to("hook-1", &hook.url("/a2a")))).await;
    w.pusher.settled().await;
    assert_eq!(hook.seen().len(), 1);
    w.a2a
        .update_entry("main", serde_json::from_value(json!({"pushOrigins": []})).unwrap())
        .await
        .expect("the entry changes");
    w.working(&task).await;
    w.pusher.settled().await;
    assert_eq!(hook.seen().len(), 1, "the url is held to the entry as it stands");
}
