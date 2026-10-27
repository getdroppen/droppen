//! End-to-end proof of the apps subsystem: the three REST routes, and the
//! `/embed/{id}/*` reverse proxy for both HTTP and websockets.
//!
//! No `code-server` is involved. The proxy only ever asks the registry "which
//! loopback port does this id mean", so the upstream here is a small axum server
//! the test starts itself, registered through
//! [`Registry::register_upstream`](remote_hand_server::apps::Registry::register_upstream).
//! It answers every path with a page that echoes the request target back — which
//! is what makes the path rewrite observable — under a deliberately hostile
//! `Content-Security-Policy`, and speaks a websocket echo on `/socket`.
//!
//! The restart-survival tests at the bottom need more than a port: they need a
//! *process* that survives a server, which means a real pid, a real process
//! group and a command line that names `code-server`. That is [`Fake`] — a
//! `code-server` script that runs this very test binary's [`fake_code_server`],
//! so the fake needs no interpreter, no fixture and nothing installed.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use gotcha::axum::http::{header, HeaderValue};
use remote_hand_server::apps::record::{self, AppRecord};
use remote_hand_server::apps::AppType;
use remote_hand_server::instance::{InstanceDirs, DEFAULT_INSTANCE};
use remote_hand_server::sessions::Paths;
use serde_json::Value;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

mod common;

// ------------------------------------------------------------------ the server

/// A running remote-hand server plus the apps registry its handlers share.
type Probe = common::TestServer;

/// `POST`/`GET` a JSON route, asserting the status so the happy path is one line.
async fn json(probe: &Probe, method: &str, path: &str, body: Option<&str>) -> Value {
    let (status, text) = match body {
        Some(body) => probe.request(method, path, &[("Content-Type", "application/json")], body.as_bytes()).await,
        None => probe.request(method, path, &[], &[]).await,
    };
    assert_eq!(status, 200, "{method} {path} -> {status}: {text}");
    serde_json::from_str(&text).expect("JSON body")
}

/// The path that names this server's app record.
///
/// Still `apps.json`, and it is a name rather than a file: `record::save`
/// and `record::load` derive the instance's `state.db` from it, and the file
/// itself is neither read nor written.
fn record_file(probe: &Probe) -> PathBuf {
    instance_dirs(&probe.home).apps_file()
}

/// The instance directory a home directory's default instance uses.
fn instance_dirs(home: &Path) -> InstanceDirs {
    InstanceDirs::new(&Paths::new(home.to_path_buf()).data_dir(), DEFAULT_INSTANCE).expect("instance name")
}

/// One HTTP response, split far enough for the assertions to reach the headers.
struct Exchange {
    status: u16,
    head: String,
    body: String,
}

impl Exchange {
    /// The value of a response header, or `None`.
    fn header(&self, name: &str) -> Option<String> {
        common::header(&self.head, name)
    }
}

/// A one-shot HTTP/1.1 request. A full client would be a large dependency for
/// plaintext requests to a loopback port the test just opened.
async fn get(probe: &Probe, path: &str) -> Exchange {
    request(probe.addr, "GET", path, None).await
}

async fn request(addr: SocketAddr, method: &str, path: &str, body: Option<&str>) -> Exchange {
    let headers = if body.is_some() { vec![("Content-Type", "application/json")] } else { vec![] };
    let (status, head, body) = common::http(addr, method, path, &headers, body.map(str::as_bytes).unwrap_or(&[])).await;
    Exchange { status, head, body: String::from_utf8_lossy(&body).into_owned() }
}

// ---------------------------------------------------------------- the upstream

/// A stand-in for `code-server`: it echoes the request target and forbids being
/// framed, so both halves of what the proxy is supposed to rewrite are visible.
struct Upstream {
    addr: SocketAddr,
    cancel: CancellationToken,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Upstream {
    async fn start() -> Upstream {
        use gotcha::axum::routing::any;

        let router = gotcha::axum::Router::new()
            .route("/socket", any(socket))
            .route("/bulk", any(bulk))
            .fallback(page);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind upstream");
        let addr = listener.local_addr().expect("upstream addr");
        let cancel = CancellationToken::new();
        let server = tokio::spawn({
            let cancel = cancel.clone();
            async move {
                gotcha::axum::serve(listener, router)
                    .with_graceful_shutdown(async move { cancel.cancelled().await })
                    .await
            }
        });
        Upstream { addr, cancel, server }
    }

    async fn stop(self) {
        self.cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.server).await;
    }
}

/// How many bytes [`bulk`] streams back, in chunks of [`BULK_CHUNK`].
const BULK_CHUNKS: usize = 64;
const BULK_CHUNK: usize = 8192;

/// A response with no length known up front, so the proxy has to forward a
/// chunked body rather than one it could have counted.
async fn bulk() -> gotcha::axum::response::Response {
    let stream =
        futures_util::stream::iter((0..BULK_CHUNKS).map(|_| Ok::<_, std::io::Error>(gotcha::axum::body::Bytes::from_static(&[b'x'; BULK_CHUNK]))));
    gotcha::axum::response::Response::new(gotcha::axum::body::Body::from_stream(stream))
}

/// Every non-socket path: a page naming the target it arrived on, and how many
/// bytes of request body came with it.
async fn page(request: gotcha::axum::extract::Request) -> gotcha::axum::response::Response {
    let target = request.uri().path_and_query().map(|target| target.as_str()).unwrap_or("").to_string();
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let sent = gotcha::axum::body::to_bytes(request.into_body(), usize::MAX)
        .await
        .map(|body| body.len())
        .unwrap_or(0);
    let body = format!("<!doctype html><title>app</title><p>target={target}<p>host={host}<p>bytes={sent}");

    let mut response = gotcha::axum::response::Response::new(gotcha::axum::body::Body::from(body));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'self'; frame-ancestors 'none'"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    response
}

/// A websocket echo that only accepts the `chat` subprotocol, so the proxy can
/// be shown to carry the negotiation through in both directions.
async fn socket(upgrade: gotcha::axum::extract::ws::WebSocketUpgrade) -> gotcha::axum::response::Response {
    use gotcha::axum::extract::ws::Message as ServerMessage;

    upgrade.protocols(["chat"]).on_upgrade(|mut socket| async move {
        while let Some(Ok(message)) = socket.next().await {
            match message {
                ServerMessage::Text(text) => {
                    if socket.send(ServerMessage::Text(text)).await.is_err() {
                        break;
                    }
                }
                ServerMessage::Close(_) => break,
                _ => {}
            }
        }
    })
}

// -------------------------------------------------------------------- the tests

#[tokio::test]
async fn the_proxy_rewrites_the_framing_policy_and_the_path() {
    let upstream = Upstream::start().await;
    let probe = Probe::start().await;
    let app = probe.apps().register_upstream(upstream.addr.port());

    let response = get(&probe, &format!("/embed/{}/stable/index.html?folder=/tmp", app.id)).await;

    assert_eq!(response.status, 200);
    assert!(
        response.body.contains("target=/stable/index.html?folder=/tmp"),
        "the `/embed/{{id}}` prefix is stripped and the query rides along: {}",
        response.body
    );
    assert_eq!(
        response.header("content-security-policy").as_deref(),
        Some("frame-ancestors 'self'"),
        "the app's own policy is replaced, not merged"
    );
    assert_eq!(response.header("x-frame-options"), None, "it would out-vote the CSP");
    assert_eq!(
        response.header("content-type").as_deref(),
        Some("text/html; charset=utf-8"),
        "every other header is the app's"
    );
    assert!(
        response.body.contains(&format!("host={}", probe.addr)),
        "the client's Host survives, so the app builds URLs against the outer origin: {}",
        response.body
    );

    probe.stop().await;
    upstream.stop().await;
}

#[tokio::test]
async fn a_bare_app_id_proxies_to_the_apps_root() {
    let upstream = Upstream::start().await;
    let probe = Probe::start().await;
    let app = probe.apps().register_upstream(upstream.addr.port());

    for path in [format!("/embed/{}", app.id), format!("/embed/{}/", app.id)] {
        let response = get(&probe, &path).await;
        assert_eq!(response.status, 200, "{path}");
        assert!(
            response.body.contains("target=/<"),
            "{path} should reach the app's root: {}",
            response.body
        );
    }

    probe.stop().await;
    upstream.stop().await;
}

#[tokio::test]
async fn bodies_stream_through_in_both_directions() {
    let upstream = Upstream::start().await;
    let probe = Probe::start().await;
    let app = probe.apps().register_upstream(upstream.addr.port());

    // Up: a request body reaches the app whole.
    let sent = "{\"folder\":\"".to_string() + &"a".repeat(20_000) + "\"}";
    let response = request(probe.addr, "POST", &format!("/embed/{}/save", app.id), Some(&sent)).await;
    assert_eq!(response.status, 200);
    assert!(
        response.body.contains(&format!("bytes={}", sent.len())),
        "the whole request body has to reach the app: {}",
        response.body
    );

    // Down: a response the app streams (no length known up front) is forwarded
    // as a stream, and arrives intact.
    let response = get(&probe, &format!("/embed/{}/bulk", app.id)).await;
    assert_eq!(response.status, 200);
    assert_eq!(
        response.header("transfer-encoding").as_deref(),
        Some("chunked"),
        "an unsized body must not be buffered into a Content-Length"
    );
    assert_eq!(response.header("content-length"), None);
    assert_eq!(response.body.len(), BULK_CHUNKS * BULK_CHUNK);
    assert!(response.body.bytes().all(|byte| byte == b'x'));

    probe.stop().await;
    upstream.stop().await;
}

#[tokio::test]
async fn a_websocket_round_trips_through_the_proxy() {
    let upstream = Upstream::start().await;
    let probe = Probe::start().await;
    let app = probe.apps().register_upstream(upstream.addr.port());

    let mut request = format!("ws://{}/embed/{}/socket", probe.addr, app.id)
        .into_client_request()
        .expect("client request");
    request.headers_mut().insert("sec-websocket-protocol", HeaderValue::from_static("chat"));

    let (mut socket, handshake) = tokio_tungstenite::connect_async(request).await.expect("websocket connect");
    assert_eq!(
        handshake.headers().get("sec-websocket-protocol").and_then(|value| value.to_str().ok()),
        Some("chat"),
        "the subprotocol the app picked has to reach the browser"
    );

    socket.send(Message::Text("ping".into())).await.expect("send");
    let echoed = tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await
        .expect("timed out")
        .expect("stream ended")
        .expect("websocket error");
    assert_eq!(echoed, Message::Text("ping".into()));

    // A second round trip, so the splice is shown to survive the first frame.
    socket.send(Message::Text("pong".into())).await.expect("send");
    let echoed = tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await
        .expect("timed out")
        .expect("stream ended")
        .expect("websocket error");
    assert_eq!(echoed, Message::Text("pong".into()));

    socket.close(None).await.expect("close");
    probe.stop().await;
    upstream.stop().await;
}

#[tokio::test]
async fn an_unknown_app_id_is_a_plain_text_404() {
    let probe = Probe::start().await;

    let response = get(&probe, "/embed/00000000-0000-4000-8000-000000000000/index.html").await;
    assert_eq!(response.status, 404);
    assert_eq!(response.body, "app not found");
    assert_eq!(response.header("content-type").as_deref(), Some("text/plain"));

    let url = format!("ws://{}/embed/nope/socket", probe.addr);
    assert!(
        tokio_tungstenite::connect_async(&url).await.is_err(),
        "an upgrade to an unknown app cannot succeed either"
    );

    probe.stop().await;
}

#[tokio::test]
async fn a_path_that_is_not_an_embed_still_belongs_to_the_spa() {
    let probe = Probe::start().await;

    // Wrapping the fallback must not have taken it away from `static_files`,
    // whose contract is that an unmatched `/api/*` is a JSON 404, not the shell.
    let response = get(&probe, "/api/definitely-not-a-route").await;
    assert_eq!(response.status, 404);
    assert_eq!(response.header("content-type").as_deref(), Some("application/json"));
    assert_eq!(serde_json::from_str::<Value>(&response.body).expect("JSON body")["error"], "not found");

    // `/embed/{id}` with no slash before the query is not an embed at all — the
    // TypeScript's regex does not match it, so the SPA gets it.
    let response = get(&probe, "/embed/abc?folder=/tmp").await;
    assert_ne!(response.body, "app not found", "this must never reach the proxy");

    probe.stop().await;
}

#[tokio::test]
async fn the_rest_routes_list_and_close() {
    let upstream = Upstream::start().await;
    let probe = Probe::start().await;

    assert_eq!(json(&probe, "GET", "/api/apps", None).await["apps"], serde_json::json!([]));

    let app = probe.apps().register_upstream(upstream.addr.port());
    let listed = json(&probe, "GET", "/api/apps", None).await;
    assert_eq!(listed["apps"][0]["id"], app.id);
    assert_eq!(listed["apps"][0]["type"], "vscode");
    assert_eq!(listed["apps"][0]["title"], "vscode 1");
    assert!(listed["apps"][0]["createdAt"].as_str().expect("createdAt").ends_with('Z'));

    assert_eq!(json(&probe, "DELETE", &format!("/api/apps/{}", app.id), None).await["accepted"], true);
    assert_eq!(json(&probe, "GET", "/api/apps", None).await["apps"], serde_json::json!([]));

    let gone = get(&probe, &format!("/embed/{}/index.html", app.id)).await;
    assert_eq!(gone.status, 404, "the proxy stops answering for a closed app");

    let again = request(probe.addr, "DELETE", &format!("/api/apps/{}", app.id), None).await;
    assert_eq!(again.status, 404);
    assert_eq!(serde_json::from_str::<Value>(&again.body).expect("JSON body")["error"], "app not found");

    probe.stop().await;
    upstream.stop().await;
}

#[tokio::test]
async fn shutdown_lets_go_of_every_app_without_signalling_it() {
    let upstream = Upstream::start().await;
    let probe = Probe::start().await;
    probe.apps().register_upstream(upstream.addr.port());
    assert_eq!(probe.apps().list().len(), 1);

    // Shutdown *detaches*: the app leaves the list — which is what the proxy
    // reads — and keeps running. A registered upstream has no process of ours
    // to signal either way, which is why the survival of a real one is asserted
    // against a real process further down.
    assert_eq!(probe.apps().shutdown_all(), 1);
    assert!(probe.apps().list().is_empty());

    probe.stop().await;
    upstream.stop().await;
}

#[tokio::test]
async fn a_failing_launch_is_a_409_that_says_why() {
    let env = remote_hand_server::process_env::clean_env_bare();
    if remote_hand_server::process_env::resolve_on_path("code-server", &env).is_some() {
        // Refusing to start a real editor from a test; the failure path is what
        // is under test and it is unreachable when the binary exists.
        return;
    }

    let probe = Probe::start().await;
    let response = request(probe.addr, "POST", "/api/apps", Some(r#"{"type":"vscode"}"#)).await;

    assert_eq!(response.status, 409);
    let body = serde_json::from_str::<Value>(&response.body).expect("JSON body");
    // A token the web app acts on, not an `os error 2` to quote.
    assert_eq!(body["error"], "code_server_missing", "{body}");
    let hint = body["hint"].as_str().expect("a sentence beside the token");
    assert!(hint.starts_with("code-server is not installed"), "the reason has to reach the user: {hint}");
    assert!(probe.apps().list().is_empty(), "a failed launch leaves nothing listed");

    probe.stop().await;
}

/// A stand-in for a real `code-server`, spawned by the tests below through a
/// script of that name.
///
/// Not a test: it is `--ignored` so the suite never runs it, and it is in this
/// binary so the fake needs no interpreter, no network and no second crate. It
/// serves the same router [`Upstream`] does, plus the `/healthz` an adoption
/// probe insists on, and it never returns — the process is meant to be killed.
#[test]
#[ignore = "spawned as a fake code-server; not a test"]
fn fake_code_server() {
    let Ok(addr) = std::env::var("RH_FAKE_ADDR") else {
        panic!("RH_FAKE_ADDR must name the address to bind");
    };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    runtime.block_on(async move {
        use gotcha::axum::routing::any;

        let router = gotcha::axum::Router::new()
            .route("/healthz", any(healthz))
            .route("/socket", any(socket))
            .route("/bulk", any(bulk))
            .fallback(page);
        let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind the fake code-server");
        gotcha::axum::serve(listener, router).await.expect("serve");
    });
}

/// What `code-server` answers `/healthz` with, and so what adoption looks for.
async fn healthz() -> gotcha::axum::response::Response {
    let body = r#"{"status":"alive","lastHeartbeat":1767225600000}"#;
    let mut response = gotcha::axum::response::Response::new(gotcha::axum::body::Body::from(body));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

#[tokio::test]
async fn an_unknown_app_type_is_refused_before_anything_is_spawned() {
    let probe = Probe::start().await;
    let response = request(probe.addr, "POST", "/api/apps", Some(r#"{"type":"emacs"}"#)).await;
    assert_eq!(response.status, 400, "the body does not validate");
    probe.stop().await;
}

// ------------------------------------------------- apps that outlive a server

/// When every staged record claims to have been started.
const STARTED_AT: &str = "2026-01-01T00:00:00.000Z";

/// Write a `code-server` that is really this test binary, and return its path.
///
/// `--bind-addr` is the one argument it reads; a real `code-server` is handed a
/// dozen more, and ignoring them is exactly what a stand-in should do. The last
/// line is deliberately *not* an `exec`: the shell stays alive as the process
/// whose pid the server records, which is what keeps the name `code-server` in
/// the command line the pid-reuse guard reads — and the shell and the server it
/// runs are both in the process group the spawn created, so one signal reaches
/// the pair of them exactly as it would `code-server` and its node children.
fn code_server_script(dir: &Path) -> PathBuf {
    let path = dir.join("code-server");
    let binary = std::env::current_exe().expect("current exe");
    let script = format!(
        r#"#!/bin/sh
addr=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --bind-addr) addr="$2"; shift 2 ;;
    *) shift ;;
  esac
done
RH_FAKE_ADDR="$addr" "{binary}" fake_code_server --exact --ignored --nocapture
"#,
        binary = binary.display()
    );
    std::fs::write(&path, script).expect("write the fake code-server");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("make it executable");
    path
}

/// A running fake `code-server`, detached from the test that started it.
struct Fake {
    pid: u32,
    port: u16,
    _dir: tempfile::TempDir,
}

impl Fake {
    /// Spawn one in a process group of its own and wait for it to answer.
    async fn start() -> Fake {
        let dir = tempfile::tempdir().expect("tempdir");
        let script = code_server_script(dir.path());
        let port = free_port();

        let mut command = tokio::process::Command::new(&script);
        command
            .arg("--bind-addr")
            .arg(format!("127.0.0.1:{port}"))
            .process_group(0)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(false);
        let mut child = command.spawn().expect("spawn the fake code-server");
        let pid = child.id().expect("the fake has a pid");
        // Reaped, so a killed fake becomes properly dead rather than a zombie
        // that still answers `kill(pid, 0)`.
        tokio::spawn(async move {
            let _ = child.wait().await;
        });

        let addr: SocketAddr = ([127, 0, 0, 1], port).into();
        for _ in 0..500 {
            if tokio::net::TcpStream::connect(addr).await.is_ok() && request(addr, "GET", "/healthz", None).await.status == 200 {
                return Fake { pid, port, _dir: dir };
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        kill_group(pid);
        panic!("the fake code-server never came up on {port}");
    }
}

impl Drop for Fake {
    /// A fake is detached by construction, so nothing else would ever stop it.
    fn drop(&mut self) {
        kill_group(self.pid);
    }
}

/// A port nothing is listening on.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    port
}

fn alive(pid: u32) -> bool {
    remote_hand_server::procs::alive(pid)
}

/// `SIGKILL` a whole process group, for a test's own cleanup.
fn kill_group(pid: u32) {
    if pid <= 1 {
        return;
    }
    // SAFETY: a signal to a process group this test spawned with
    // `process_group(0)`, guarded above against the two wildcard values.
    unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
}

/// Put records where a server started on `home` will adopt them from.
///
/// Through `record::save`, which is the production writer, so what is staged is
/// a row in that instance's database rather than the `apps.json` this used to
/// write — the same store the boot will read, reached the same way.
fn stage(home: &Path, records: &[AppRecord]) {
    let dirs = instance_dirs(home);
    dirs.create().expect("instance directory");
    record::save(&dirs.apps_file(), records);
}

/// One staged record. The id is a path segment the `/embed` proxy will match.
///
/// No `pid_token`, so every test that uses it as it stands is also a test of the
/// records written before the guard existed: they must still be adopted on
/// liveness rather than refused for having nothing to compare.
fn record_of(id: &str, pid: u32, port: u16) -> AppRecord {
    AppRecord {
        id: id.to_string(),
        kind: AppType::Vscode,
        pid,
        port,
        cwd: None,
        started_at: STARTED_AT.to_string(),
        pid_token: None,
    }
}

/// A start-time token from some other moment, which is what a record left by a
/// process whose pid has since been recycled amounts to.
///
/// Not any real process's: macOS tokens are seconds and microseconds since the
/// epoch and Linux's are clock ticks since boot, so nothing alive can be said to
/// have started here.
const OTHER_MOMENT: &str = "0.000000";

/// A live process that is not an app of this server's, whose command line says
/// `name`.
///
/// `sh -c` is given a third word because that becomes `$0`, and so the argument
/// vector `ps` reports; and a compound command because a shell handed a single
/// simple one `exec`s straight out of the process whose pid the test is about to
/// use, taking the name with it.
async fn bystander(name: &str) -> u32 {
    let mut command = tokio::process::Command::new("/bin/sh");
    command
        .args(["-c", "sleep 30; exit 0", name])
        .process_group(0)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(false);
    let mut child = command.spawn().expect("spawn a bystander");
    let pid = child.id().expect("the bystander has a pid");
    // Reaped, so a killed bystander becomes properly dead rather than a zombie
    // that still answers `kill(pid, 0)`.
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    pid
}

#[tokio::test]
async fn an_app_that_outlived_its_server_is_adopted_and_still_proxies() {
    let fake = Fake::start().await;
    let id = "11111111-1111-4111-8111-111111111111";
    let probe = Probe::start_with(|home| stage(home, &[record_of(id, fake.pid, fake.port)])).await;

    // The list is rebuilt from the file, and an adopted app does not claim to
    // have been created at the moment the server restarted.
    let listed = json(&probe, "GET", "/api/apps", None).await;
    assert_eq!(listed["apps"].as_array().expect("array").len(), 1, "{listed}");
    assert_eq!(listed["apps"][0]["id"], id);
    assert_eq!(listed["apps"][0]["createdAt"], STARTED_AT);

    // And it is a working app rather than a row in a list: the proxy reaches
    // the process that survived, through the id the record named.
    let response = get(&probe, &format!("/embed/{id}/stable/index.html?folder=/tmp")).await;
    assert_eq!(response.status, 200);
    assert!(
        response.body.contains("target=/stable/index.html?folder=/tmp"),
        "the adopted app is behind the proxy: {}",
        response.body
    );

    // Closing it stops a process group this server never spawned.
    assert_eq!(json(&probe, "DELETE", &format!("/api/apps/{id}"), None).await["accepted"], true);
    common::wait_for("the adopted app to be killed", Duration::from_secs(15), || !alive(fake.pid)).await;
    assert!(
        record::load(&record_file(&probe)).is_empty(),
        "and takes its record with it, so the next boot has nothing to adopt"
    );

    probe.stop().await;
}

#[tokio::test]
async fn a_shutdown_leaves_a_real_app_running_with_its_record_intact() {
    let fake = Fake::start().await;
    let id = "22222222-2222-4222-8222-222222222222";
    let probe = Probe::start_with(|home| stage(home, &[record_of(id, fake.pid, fake.port)])).await;
    assert_eq!(probe.apps().list().len(), 1, "the app was adopted");

    // This is the inversion the phase exists for: the app is *let go of*, not
    // killed, and the record it will be found by is left exactly as it was.
    assert_eq!(probe.apps().shutdown_all(), 1);
    assert!(probe.apps().list().is_empty(), "it leaves the list the proxy reads");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(alive(fake.pid), "shutting the server down must not touch the user's editor");
    let records = record::load(&record_file(&probe));
    assert_eq!(records.len(), 1, "the next boot still has something to adopt: {records:?}");
    assert_eq!(records[0].pid, fake.pid);
    assert_eq!(records[0].port, fake.port);

    probe.stop().await;
}

#[tokio::test]
async fn a_recorded_pid_that_is_now_a_stranger_is_never_signalled() {
    // Two bystanders, in groups of their own so nothing aimed at this test
    // binary sweeps them up, and between them the two ways a pid this server no
    // longer owns is told apart from one it does.
    //
    // The first is *convincing*: its command line names `code-server` exactly as
    // a real editor's does, which is the shape of the hazard — a pid recycled
    // onto another `code-server`, the user's own or another instance's. Only the
    // start-time token recorded beside the pid can tell it apart.
    //
    // The second stands in for a record written before that token existed. There
    // is nothing to compare, the guard degrades to liveness rather than refusing
    // to adopt anything at all, and the command line is what is left to notice
    // this was never an editor.
    let convincing = bystander("code-server").await;
    let obvious = bystander("something-else").await;
    assert!(
        remote_hand_server::procs::is_running(convincing, "code-server").await,
        "the first bystander has to be genuinely convincing, or the token is never what saves it"
    );
    assert!(!remote_hand_server::procs::is_running(obvious, "code-server").await);

    // Alive pid, dead port: the shape of an app that died and left its number to
    // be handed to somebody else. The boot wants to stop both — and must not.
    let recycled = "33333333-3333-4333-8333-333333333333";
    let ancient = "77777777-7777-4777-8777-777777777777";
    let probe = Probe::start_with(|home| {
        let mut staged = record_of(recycled, convincing, free_port());
        staged.pid_token = Some(OTHER_MOMENT.to_string());
        stage(home, &[staged, record_of(ancient, obvious, free_port())]);
    })
    .await;

    let listed = json(&probe, "GET", "/api/apps", None).await;
    assert_eq!(listed["apps"], serde_json::json!([]), "neither record is adopted");
    assert!(record::load(&record_file(&probe)).is_empty(), "and both are dropped");

    // The assertion the whole guard exists for. A `SIGTERM` would already have
    // been delivered; the escalation is three seconds behind it.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(alive(convincing), "a `code-server` that merely inherited the pid is not ours");
    assert!(alive(obvious), "and neither is anything else that did");

    probe.stop().await;
    kill_group(convincing);
    kill_group(obvious);
}

#[tokio::test]
async fn an_app_whose_pid_was_recycled_is_not_adopted_however_right_the_port_looks() {
    // Everything a boot can see says yes: the pid is alive, and the port answers
    // `/healthz` the way an editor does. It is still not this record's app —
    // somebody else's `code-server`, on a number the kernel handed on — and the
    // start-time token is the only thing that knows.
    let fake = Fake::start().await;
    let id = "66666666-6666-4666-8666-666666666666";
    let probe = Probe::start_with(|home| {
        let mut staged = record_of(id, fake.pid, fake.port);
        staged.pid_token = Some(OTHER_MOMENT.to_string());
        stage(home, &[staged]);
    })
    .await;

    let listed = json(&probe, "GET", "/api/apps", None).await;
    assert_eq!(
        listed["apps"],
        serde_json::json!([]),
        "a port that looks right is not an identity: {listed}"
    );
    assert!(record::load(&record_file(&probe)).is_empty(), "and the record is dropped");

    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(alive(fake.pid), "nor is it signalled: this is somebody's editor, just not ours");

    probe.stop().await;
}

#[tokio::test]
async fn a_port_held_by_something_that_is_not_the_app_is_left_alone() {
    // Two shapes of the same hazard. The plain upstream answers `/healthz` with
    // a page rather than the JSON an editor answers with: an unrelated service
    // that took the port while the server was down. The fake answers exactly
    // right — but the pid recorded beside it is gone, so it is somebody else's
    // `code-server`, not the one this record was written for.
    let upstream = Upstream::start().await;
    let fake = Fake::start().await;
    // Above every platform's `pid_max`, so the record cannot name a process
    // this test binary happens to have spawned.
    let dead = u32::MAX - 1;
    let stranger = "44444444-4444-4444-8444-444444444444";
    let orphan = "55555555-5555-4555-8555-555555555555";
    let probe = Probe::start_with(|home| {
        stage(
            home,
            &[record_of(stranger, dead, upstream.addr.port()), record_of(orphan, dead, fake.port)],
        )
    })
    .await;

    let listed = json(&probe, "GET", "/api/apps", None).await;
    assert_eq!(listed["apps"], serde_json::json!([]), "neither record is adopted");
    assert!(record::load(&record_file(&probe)).is_empty(), "and both are dropped");

    // Both are still serving: adoption looked, did not recognise them, and left.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let answered = request(upstream.addr, "GET", "/healthz", None).await;
    assert_eq!(answered.status, 200, "the unrelated service must still be there");
    assert!(answered.body.contains("target=/healthz"), "{}", answered.body);
    assert!(alive(fake.pid), "and so must a code-server this server never started");

    probe.stop().await;
    upstream.stop().await;
}

// ------------------------------------------------------- across two processes

/// The real thing: the actual binary, killed through its whole process group
/// with `SIGKILL`, and a second invocation that picks the app back up.
///
/// Everything else here stages the record by hand. This one does not: the
/// server spawns the app itself, down the production path, and writes the record
/// itself — the only seam is a `code-server` on its `PATH` that is really
/// [`fake_code_server`]. What it proves is that the mechanism works, rather than
/// that the code agrees with itself.
#[tokio::test]
async fn an_app_survives_its_server_being_sigkilled_and_the_next_one_adopts_it() {
    let home = tempfile::tempdir().expect("tempdir");
    let bin = tempfile::tempdir().expect("tempdir");
    code_server_script(bin.path());

    let first = spawn_server(home.path(), bin.path()).await;
    let created = request(first.addr, "POST", "/api/apps", Some(r#"{"type":"vscode"}"#)).await;
    assert_eq!(created.status, 200, "create -> {}: {}", created.status, created.body);
    let id = serde_json::from_str::<Value>(&created.body).expect("json")["id"]
        .as_str()
        .expect("id")
        .to_string();

    // The record is written by the server that spawned it, and is the only
    // thing that will say this app was ours.
    let records = record::load(&instance_dirs(home.path()).apps_file());
    assert_eq!(records.len(), 1, "the running app is written down: {records:?}");
    assert_eq!(records[0].id, id);
    assert!(records[0].pid_token.is_some(), "the pid-reuse guard is recorded with it: {records:?}");
    let app = records[0].pid;
    assert!(alive(app), "the app the server started is running");

    // The whole process group, so nothing survives by being merely a child.
    unsafe { libc::kill(-(first.pid as libc::pid_t), libc::SIGKILL) };
    common::wait_for("the server to die", Duration::from_secs(10), || !alive(first.pid)).await;
    assert!(alive(app), "the app must survive a SIGKILL aimed at the server's whole process group");

    let second = spawn_server(home.path(), bin.path()).await;
    let listed: Value = serde_json::from_str(&request(second.addr, "GET", "/api/apps", None).await.body).expect("json");
    assert_eq!(
        listed["apps"].as_array().expect("array").len(),
        1,
        "the second process adopts it: {listed}"
    );
    assert_eq!(listed["apps"][0]["id"], id.as_str());

    // Still a working app, through a proxy in a process that never spawned it.
    let response = request(second.addr, "GET", &format!("/embed/{id}/stable/index.html"), None).await;
    assert_eq!(response.status, 200);
    assert!(response.body.contains("target=/stable/index.html"), "{}", response.body);

    // And the second server can close what the first one started.
    let closed = request(second.addr, "DELETE", &format!("/api/apps/{id}"), None).await;
    assert_eq!(closed.status, 200, "{}", closed.body);
    common::wait_for("the adopted app to be killed with its group", Duration::from_secs(15), || !alive(app)).await;

    unsafe { libc::kill(-(second.pid as libc::pid_t), libc::SIGKILL) };
    common::wait_for("the second server to die", Duration::from_secs(10), || !alive(second.pid)).await;
    kill_group(app);
}

/// Start one, with `bin` in front of its `PATH`.
///
/// The `PATH` is the whole of the seam, and it is deliberately the only one: the
/// server resolves `code-server` by name exactly as it does in production, spawns
/// it exactly as it does in production, and never learns that the thing it
/// started is a test.
async fn spawn_server(home: &Path, bin: &Path) -> common::Spawned {
    let path = match std::env::var("PATH") {
        Ok(inherited) => format!("{}:{inherited}", bin.display()),
        Err(_) => bin.display().to_string(),
    };
    common::spawn_server(home, DEFAULT_INSTANCE, &[("PATH", &path)]).await
}
