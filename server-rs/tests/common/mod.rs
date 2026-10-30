//! A server on a throwaway home directory, shared by the integration tests.
//!
//! Nothing here touches the real home directory: every test points the server at
//! a tempdir through `RemoteHandApp::home_override`, and [`Paths`] derives every
//! other location from it — so a test can seed `~/.claude`, `~/.codex` or
//! `~/.config/remote-hand` without depending on (or writing to) a user's own.
//!
//! Deliberately *not* a `Paths` environment override: two of these run
//! concurrently in one test binary, and a process-wide variable cannot say two
//! different things at once.
//!
//! # Authentication
//!
//! A server with no account is in its setup state and refuses every route
//! (`droppen_auth::setup`). Almost no test here is about that, so the ordinary
//! starts — [`TestServer::start`], [`TestServer::start_with`],
//! [`TestServer::start_at`] — switch authentication **off** explicitly, the
//! `auth.enabled = false` escape hatch, rather than every test logging in. The
//! tests that are about authentication use [`TestServer::start_guarded`] and
//! [`TestServer::start_guarded_at`], which leave the default alone.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use remote_hand_server::app::RemoteHandApp;
use droppen_workbench::apps::Registry;
use droppen_drive::feed::global::GlobalFeed;
use droppen_drive::feed::FeedRegistry;
use droppen_instance::instance::InstanceDirs;
use droppen_instance::paths::Paths;
use remote_hand_server::shutdown::Cleanups;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

/// A running server plus everything needed to talk to it and stop it.
pub struct TestServer {
    /// The address the listener actually bound.
    pub addr: SocketAddr,
    /// Where the server thinks `$HOME` is.
    pub home: PathBuf,
    /// The same path set the server's handlers see.
    pub paths: Paths,
    global: Arc<GlobalFeed>,
    terminals: Arc<droppen_workbench::terminals::Registry>,
    apps: Arc<Registry>,
    feeds: Arc<FeedRegistry>,
    cleanups: Cleanups,
    cancel: CancellationToken,
    /// An `Option` so `stop` can take it: this struct has a `Drop`, and a type
    /// with one cannot have a field moved out of it.
    server: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
    /// Ends the terminals' shims and the CLIs this server started if the test
    /// process dies before this is dropped: its own shims are this test
    /// binary's and go with it ([`shims_end_with_this_process`]), but a
    /// terminal's are the server binary's.
    _reaper: Reaper,
    /// `None` when the caller owns the home directory — see
    /// [`TestServer::start_at`], where the whole point is that it outlives any
    /// one server.
    _home: Option<tempfile::TempDir>,
}

impl TestServer {
    /// Start a server on a fresh empty home directory.
    pub async fn start() -> TestServer {
        Self::start_with(|_| {}).await
    }

    /// Start a server on a home directory `seed` has filled in first.
    pub async fn start_with(seed: impl FnOnce(&Path)) -> TestServer {
        let home = tempfile::tempdir().expect("tempdir");
        seed(home.path());
        let mut server = Self::start_at(home.path(), None).await;
        server._home = Some(home);
        server
    }

    /// Start a server on a home directory the *caller* owns, optionally under a
    /// named instance.
    ///
    /// Two things need this. A restart is two servers on one home, one after
    /// the other, so the directory cannot belong to either of them; and two
    /// instances are two servers on one home at the same time, which is what
    /// `APP_INSTANCE` exists for and the only way to test what they must not do
    /// to each other.
    pub async fn start_at(home: &Path, instance: Option<&str>) -> TestServer {
        Self::boot(home, instance, Some(false)).await
    }

    /// Start a server on a fresh empty home directory with authentication as
    /// it is by default: on, so with no account it is in its setup state.
    pub async fn start_guarded() -> TestServer {
        let home = tempfile::tempdir().expect("tempdir");
        let mut server = Self::start_guarded_at(home.path(), None).await;
        server._home = Some(home);
        server
    }

    /// [`TestServer::start_at`] with authentication as it is by default.
    pub async fn start_guarded_at(home: &Path, instance: Option<&str>) -> TestServer {
        Self::boot(home, instance, None).await
    }

    /// [`TestServer::start_at`] with `auth.enabled = true`: a login everywhere.
    pub async fn start_forced_at(home: &Path) -> TestServer {
        Self::boot(home, None, Some(true)).await
    }

    /// A server the desktop app runs (`DROPPEN_DESKTOP_APP`), authentication as it
    /// is by default, on a home directory the caller owns, under `instance`.
    pub async fn start_desktop_app_at(home: &Path, instance: &str) -> TestServer {
        Self::boot_with(home, Some(instance), None, Some(true)).await
    }

    async fn boot(home: &Path, instance: Option<&str>, auth_enabled: Option<bool>) -> TestServer {
        Self::boot_with(home, instance, auth_enabled, None).await
    }

    async fn boot_with(home: &Path, instance: Option<&str>, auth_enabled: Option<bool>, desktop_app: Option<bool>) -> TestServer {
        test_home(home);
        shims_end_with_this_process();
        let app = || RemoteHandApp {
            home_override: Some(home.to_path_buf()),
            port_override: Some(0),
            instance_override: instance.map(str::to_owned),
            auth_enabled_override: auth_enabled,
            desktop_app_override: desktop_app,
            ..Default::default()
        };
        // A server stopped a moment ago on this home can still be letting go
        // of its instance: a task it spawned holds the claim until the runtime
        // next polls it and it sees the cancellation, which a test that
        // restarts straight away can beat. So a refusal is tried again, for a
        // couple of seconds, before it is a failure.
        let mut attempts = 0;
        let prepared = loop {
            match app().prepare().await {
                Ok(prepared) => break prepared,
                Err(error) if attempts < 40 => {
                    attempts += 1;
                    let _ = error;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => panic!("prepare: {error:?}"),
            }
        };
        let addr = prepared.addr;
        let cancel = prepared.cancel();
        let cleanups = prepared.cleanups();
        let global = cleanups.global.clone();
        let terminals = cleanups.terminals.clone();
        let apps = cleanups.apps.clone();
        let feeds = cleanups.feeds.clone();
        let server = tokio::spawn(prepared.serve_task());
        TestServer {
            addr,
            home: home.to_path_buf(),
            // The same instance the server was given: the Claude root it owns
            // hangs off it, so a `Paths` built for `default` here would point a
            // sibling instance's test at the wrong directory.
            paths: Paths::for_instance(home.to_path_buf(), instance.unwrap_or(droppen_instance::instance::DEFAULT_INSTANCE)),
            global,
            terminals,
            apps,
            feeds,
            cleanups,
            cancel,
            server: Some(server),
            _reaper: Reaper::watch(None, home, instance.unwrap_or(droppen_instance::instance::DEFAULT_INSTANCE)),
            _home: None,
        }
    }

    /// Whether the global feed's watcher is running — i.e. a client is attached.
    pub fn feed_is_running(&self) -> bool {
        self.global.is_running()
    }

    /// The apps registry these handlers share.
    pub fn apps(&self) -> Arc<Registry> {
        self.apps.clone()
    }

    /// The terminal registry these handlers share.
    pub fn terminals(&self) -> Arc<droppen_workbench::terminals::Registry> {
        self.terminals.clone()
    }

    /// The live session feeds, so a caller driving the server itself can assert
    /// they were torn down.
    pub fn feeds(&self) -> Arc<FeedRegistry> {
        self.feeds.clone()
    }

    /// The token that stops this server, for a caller driving the shutdown.
    pub fn cancel(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Everything `shutdown::supervise` tears down on SIGTERM.
    pub fn cleanups(&self) -> Cleanups {
        self.cleanups.clone()
    }

    /// Stop the way `SIGTERM` does: cancel the token and run the registered
    /// cleanups, which detach terminals and apps rather than killing them.
    ///
    /// The serve task is handed back so the caller can wait for the listener to
    /// go — and, with it, the instance lock the task holds.
    pub fn shutdown_like_sigterm(&mut self) -> Option<tokio::task::JoinHandle<std::io::Result<()>>> {
        self.cancel.cancel();
        self.cleanups.run();
        self.server.take()
    }

    /// Take the serve task without cancelling anything, for a test that stops
    /// the server by its own route.
    pub fn take_task(&mut self) -> Option<tokio::task::JoinHandle<std::io::Result<()>>> {
        self.server.take()
    }

    /// `GET path`, returning `(status, body)`.
    pub async fn get(&self, path: &str) -> (u16, String) {
        self.request("GET", path, &[], &[]).await
    }

    /// `GET path`, asserting a `200` and parsing the body as JSON.
    pub async fn get_json(&self, path: &str) -> Value {
        let (status, body) = self.get(path).await;
        assert_eq!(status, 200, "GET {path} -> {status}: {body}");
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("GET {path} returned non-JSON ({e}): {body}"))
    }

    /// A request with a JSON body, returning `(status, body)`.
    pub async fn json(&self, method: &str, path: &str, body: &Value) -> (u16, String) {
        let encoded = serde_json::to_vec(body).expect("encode body");
        self.request(method, path, &[("Content-Type", "application/json")], &encoded).await
    }

    /// A one-shot HTTP/1.1 request. `headers` are sent verbatim.
    pub async fn request(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> (u16, String) {
        let (status, _, body) = self.request_full(method, path, headers, body).await;
        (status, body)
    }

    /// The same, with the response head, for tests that assert on headers.
    ///
    /// A full client (reqwest and its TLS stack) would be a large dependency for
    /// plaintext requests to a loopback port the test just opened.
    pub async fn request_full(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> (u16, String, String) {
        let (status, head, body) = http(self.addr, method, path, headers, body).await;
        (status, head, String::from_utf8_lossy(&body).into_owned())
    }

    /// The websocket URL of a path on this server.
    pub fn ws_url(&self, path: &str) -> String {
        format!("ws://{}{path}", self.addr)
    }

    /// Stop the server and wait for the listener to go away.
    pub async fn stop(mut self) {
        self.cancel.cancel();
        if let Some(server) = self.server.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
        }
    }
}

impl Drop for TestServer {
    /// Kill whatever terminals and apps the test started.
    ///
    /// A web terminal now belongs to a detached `pty-shim` process that is
    /// *designed* to outlive the server — which is right in production and
    /// wrong in a test binary, where it would leave stray shells behind on the
    /// developer's machine. An embedded app is detached for the same reason.
    /// `kill_all` is the deliberate escape hatch for exactly this, and it
    /// signals rather than sending frames so it works from a `Drop` with no
    /// runtime to await on.
    fn drop(&mut self) {
        self.terminals.kill_all();
        self.apps.kill_all();
    }
}

/* ── talking to a server ─────────────────────────────────────────────── */

/// A one-shot HTTP/1.1 request to `addr`.
///
/// A full client (reqwest and its TLS stack) would be a large dependency for
/// plaintext requests to a loopback port the test just opened. `headers` are
/// sent verbatim; the `Host` is `addr` unless the caller names its own, and a
/// non-empty body carries its `Content-Length`. The response is split into
/// `(status, head, body)`, with a chunked body joined.
pub async fn http(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> (u16, String, Vec<u8>) {
    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let mut request = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\n");
    if !headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("host")) {
        request.push_str(&format!("Host: {addr}\r\n"));
    }
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    if !body.is_empty() {
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");

    let mut raw = request.into_bytes();
    raw.extend_from_slice(body);
    stream.write_all(&raw).await.expect("write");

    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.expect("read");
    split_response(&response)
}

/// The (trimmed) value of a response header, or `None`. Case-insensitive.
pub fn header(head: &str, name: &str) -> Option<String> {
    head.lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.trim().eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim().to_string())
}

/* ── waiting for a condition ─────────────────────────────────────────── */

/// Poll `condition` every 20 ms until it holds, or fail with `what`.
pub async fn wait_for(what: &str, budget: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + budget;
    while !condition() {
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The same, for a condition that has to be awaited, returning its first `Some`.
pub async fn wait_for_async<T, F, Fut>(what: &str, budget: Duration, mut probe: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = std::time::Instant::now() + budget;
    loop {
        if let Some(value) = probe().await {
            return value;
        }
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/* ── seeding transcripts ─────────────────────────────────────────────── */

/// Write JSONL entries, one per line, creating the parent directory.
pub fn write_jsonl(path: &Path, entries: &[Value]) {
    std::fs::create_dir_all(path.parent().expect("a parent directory")).expect("transcript dir");
    let body: String = entries.iter().map(|entry| format!("{entry}\n")).collect();
    std::fs::write(path, body).expect("seed transcript");
}

/// Write a transcript at `<root>/projects/<project>/<id>.jsonl` and return its
/// path.
pub fn seed_transcript(root: &Path, project: &str, id: &str, entries: &[Value]) -> PathBuf {
    let dir = root.join("projects").join(project);
    std::fs::create_dir_all(&dir).expect("project dir");
    let path = dir.join(format!("{id}.jsonl"));
    write_jsonl(&path, entries);
    path
}

/// Give a transcript a definite mtime, since the session list is ordered by it
/// and two files written in the same instant would otherwise tie.
pub fn set_mtime(path: &Path, seconds_since_epoch: u64) {
    let when = std::time::UNIX_EPOCH + Duration::from_secs(seconds_since_epoch);
    let file = std::fs::OpenOptions::new().write(true).open(path).expect("open for times");
    file.set_times(std::fs::FileTimes::new().set_accessed(when).set_modified(when))
        .expect("set mtime");
}

/* ── accounts ────────────────────────────────────────────────────────── */

/// Create an oauth-token account over HTTP, asserting the `201` and returning
/// the body.
pub async fn create_account(server: &TestServer, label: &str, secret: &str) -> Value {
    let (status, raw) = server
        .json("POST", "/api/accounts", &serde_json::json!({"label": label, "kind": "oauth_token", "secret": secret}))
        .await;
    assert_eq!(status, 201, "POST /api/accounts -> {status}: {raw}");
    serde_json::from_str(&raw).unwrap_or_else(|error| panic!("POST /api/accounts returned non-JSON ({error}): {raw}"))
}

/// The same, returning the new account's id.
pub async fn create_account_id(server: &TestServer, label: &str, secret: &str) -> String {
    create_account(server, label, secret).await["id"]
        .as_str()
        .expect("account id")
        .to_string()
}

/* ── the real binary ─────────────────────────────────────────────────── */

/// A server started from the real binary, on a home directory the test owns.
///
/// A test may kill it whenever it likes: a group signal to `pid` reaches the
/// server and everything it did not detach. Dropped, it does the same, if
/// `pid` is still the process it started, and then kills every CLI its run
/// journal names and every server-binary process naming its home, which a
/// server detaches on purpose so that they outlive it. In that order, so that
/// a server still running starts nothing after they were looked for. And if
/// the test process dies before this is dropped, its [`Reaper`] does it all.
pub struct Spawned {
    pub pid: u32,
    pub addr: SocketAddr,
    /// When `pid` started, so a pid the system has handed to another process
    /// since is never signalled.
    token: Option<String>,
    /// The home directory it runs on.
    home: PathBuf,
    /// The instance it runs.
    instance: String,
    _reaper: Reaper,
}

impl Spawned {
    /// Kill its process group now, if its leader is still the server that
    /// was started ([`kill_group_of`]).
    pub fn kill(&self) {
        kill_group_of(self.pid, self.token.as_deref());
    }
}

impl Drop for Spawned {
    fn drop(&mut self) {
        kill_group_of(self.pid, self.token.as_deref());
        kill_journal_runs(&self.home, &self.instance);
        kill_shims_serving(&self.home);
    }
}

/// Kill the process group `pid` leads, if a start token was recorded for it
/// and `pid` is still the process that token was taken from. Without a token,
/// or with one that cannot be read for `pid` now, nothing is signalled: that
/// `pid` is alive cannot tell the process that was started from one the
/// system has given its pid to since.
pub fn kill_group_of(pid: u32, token: Option<&str>) {
    let Some(token) = token else {
        return;
    };
    if droppen_base::procs::start_token(pid).as_deref() == Some(token) {
        // SAFETY: a signal to the process group of a process this test
        // started, checked to be that process still.
        unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
    }
}

/// What a test runs out of process outlives the test process if it dies
/// before its guards are dropped: killed by a timeout or an interrupted run,
/// it leaves a server that goes on starting sessions on its schedules, and the
/// shims and CLIs it started. So a guard that starts one also starts this:
/// the test binary run as its helper entry ([`session_shim`], which then
/// [`reap`]s), in a process group of its own so that a signal to the test's
/// group does not take it too. It reads one line from a pipe from this
/// process. A guard dropped in time has done the work itself, and says so on
/// that line; a pipe that closes without it was closed by this process dying,
/// and then the reaper does what the guard would have.
///
/// What it ends, it picks by process group and by the test's own home
/// directory, never by a name alone: the group the guard started, if its
/// leader is still the process that was started; the CLIs the run journal
/// under that home names, each if it is still the process its row records;
/// and the server binary's processes whose command line names a path under
/// that home ([`kill_shims_serving`]). Every path is built from the home
/// itself, which must be a directory of the test's own ([`test_home`]).
pub struct Reaper {
    child: Option<std::process::Child>,
}

/// The reaper's task, in its environment, one line each: the server's process
/// group (`-` for a server in this process), when its leader started (`-` if
/// that cannot be read), the home it serves and the instance.
const REAPER_TASK: &str = "DROPPEN_TEST_REAPER";

impl Reaper {
    /// Watch a server on `home` for the instance `instance`: out of process,
    /// the process group `pgid`; in this one, `None`.
    pub fn watch(pgid: Option<u32>, home: &Path, instance: &str) -> Reaper {
        use std::os::unix::process::CommandExt;
        let home = test_home(home);
        let token = pgid.and_then(droppen_base::procs::start_token);
        // libtest names a test by its path inside the binary.
        let (_binary, module) = module_path!().split_once("::").expect("a module inside a test binary");
        let task = [
            pgid.map_or("-".to_string(), |pgid| pgid.to_string()),
            token.unwrap_or_else(|| "-".to_string()),
            home.display().to_string(),
            instance.to_string(),
        ];
        let child = std::process::Command::new(std::env::current_exe().expect("this test binary"))
            .args(["--ignored", "--exact", "--quiet", &format!("{module}::session_shim")])
            .env(REAPER_TASK, task.join("\n"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .expect("start the reaper");
        Reaper { child: Some(child) }
    }
}

/// The line a guard says down the pipe when it lets its reaper go.
const REAPER_RELEASE: &str = "released";

impl Drop for Reaper {
    /// Tell the reaper it has nothing to do, close the pipe, and wait for it
    /// to go.
    fn drop(&mut self) {
        use std::io::Write;
        if let Some(mut child) = self.child.take() {
            if let Some(mut pipe) = child.stdin.take() {
                let _ = writeln!(pipe, "{REAPER_RELEASE}");
            }
            let _ = child.wait();
        }
    }
}

/// A guard's [`Reaper`], run as this binary's helper entry: wait for the
/// guard's word, or for the test process to go, and then do the guard's work.
fn reap() {
    let Ok(task) = std::env::var(REAPER_TASK) else {
        return;
    };
    let mut task = task.lines();
    let pgid: Option<u32> = task.next().and_then(|pgid| pgid.parse().ok());
    let token = task.next().filter(|token| *token != "-").map(str::to_owned);
    let home = PathBuf::from(task.next().expect("the home"));
    let instance = task.next().expect("the instance").to_owned();
    // One line: the guard's word, or nothing once the test process is gone.
    // Only the line, not the pipe's end: a child the test started meanwhile
    // can have inherited its write end, and would hold it open.
    let mut said = String::new();
    let _ = std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut said);
    if said.trim_end() == REAPER_RELEASE {
        return;
    }
    if let Some(pgid) = pgid {
        kill_group_of(pgid, token.as_deref());
    }
    // A server a supervisor had just started may come up a moment after its
    // supervisor is gone, and a shim the dead process had just forked may not
    // have been the server binary yet: looked for again, for five seconds.
    for _ in 0..25 {
        kill_shims_serving(&home);
        kill_journal_runs(&home, &instance);
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Start the real server binary on `home` and wait until it is listening.
///
/// The flags are the ones every test needs: no login, an ephemeral port, and a
/// loopback host. `extra_env` is applied on top (the fake-CLI-on-`PATH` seam,
/// for instance). The child inherits this process's environment and leads a
/// process group of its own, so the test can kill the group without killing
/// itself.
pub async fn spawn_server(home: &Path, instance: &str, extra_env: &[(&str, &str)]) -> Spawned {
    test_home(home);
    let port_file = InstanceDirs::new(&Paths::new(home.to_path_buf()).data_dir(), instance)
        .expect("instance name")
        .port_file();
    // So a stale port from a previous server is never mistaken for this one's.
    let _ = std::fs::remove_file(&port_file);

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-hand-server"));
    command
        .arg("serve")
        .env("APP_HOME_DIR", home)
        // No login: not a test of authentication (`tests/auth.rs` is).
        .env("APP_AUTH__ENABLED", "false")
        .env("APP_SERVER__PORT", "0")
        .env("APP_SERVER__HOST", "127.0.0.1")
        // A group of its own, so the test can kill the group without killing
        // itself — and so a child that takes a group of its own again is
        // demonstrably outside it.
        .process_group(0)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(false);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let mut child = command.spawn().expect("spawn the server binary");
    let pid = child.id().expect("the server has a pid");
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    // Armed before the wait below, so a server that never comes up is still
    // ended with the test.
    let mut spawned = Spawned {
        pid,
        addr: ([127, 0, 0, 1], 0).into(),
        token: droppen_base::procs::start_token(pid),
        _reaper: Reaper::watch(Some(pid), home, instance),
        home: home.to_path_buf(),
        instance: instance.to_string(),
    };
    spawned.addr = wait_for_listener(home, instance, Duration::from_secs(30)).await;
    spawned
}

/// Wait until the server rooted at `home` has written its port and answers on
/// it.
pub async fn wait_for_listener(home: &Path, instance: &str, budget: Duration) -> SocketAddr {
    let port_file = InstanceDirs::new(&Paths::new(home.to_path_buf()).data_dir(), instance)
        .expect("instance name")
        .port_file();
    let deadline = std::time::Instant::now() + budget;
    loop {
        if let Some(port) = std::fs::read_to_string(&port_file).ok().and_then(|text| text.trim().parse::<u16>().ok()) {
            let candidate: SocketAddr = ([127, 0, 0, 1], port).into();
            if tokio::net::TcpStream::connect(candidate).await.is_ok() {
                return candidate;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the server never recorded a port it was listening on"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/* ── what the server actually persisted ──────────────────────────────── */

/// One text column of a query against a state database.
///
/// Every store the server keeps is SQLite now — `<config>/state.db` for what is
/// machine-wide, `<instance>/state.db` for what belongs to one server — and none
/// of them writes the JSON file it is still named after. So an assertion that
/// used to read `runs.json` or `push.json` to see what was *persisted*, as
/// against what a handler happened to answer, comes through here. Where a route
/// shows the same fact it is the better window and these tests use it; this is
/// for the facts no route exposes.
///
/// Read-only, and that flag is load-bearing rather than tidy: opening
/// read-write would *create* the database, so a mistyped path would answer every
/// "is it in the table?" with a serene empty list instead of failing. Panicking
/// rather than returning an error is the same instinct — a query that will not
/// run is a broken test, never an empty store.
///
/// `busy_timeout` because the server under test is holding the same file open.
/// WAL lets this connection read straight through its writes; a checkpoint is
/// the one moment that can still make it wait.
pub fn rows(db: &Path, query: &str, params: impl rusqlite::Params) -> Vec<String> {
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI;
    let conn = rusqlite::Connection::open_with_flags(db, flags).unwrap_or_else(|error| panic!("cannot open {} read-only: {error}", db.display()));
    conn.busy_timeout(Duration::from_secs(5)).expect("set a busy timeout");
    let mut statement = conn
        .prepare(query)
        .unwrap_or_else(|error| panic!("cannot prepare `{query}` against {}: {error}", db.display()));
    let found = statement
        .query_map(params, |row| row.get::<_, String>(0))
        .and_then(|mapped| mapped.collect::<rusqlite::Result<Vec<String>>>())
        .unwrap_or_else(|error| panic!("cannot run `{query}` against {}: {error}", db.display()));
    found
}

/// The same, with every row parsed as JSON.
///
/// The shape of every `payload` column: the record's own `serde` encoding, which
/// the schema deliberately keeps opaque so that exactly one piece of code
/// decides what a run or a subscription means. A payload that will not parse
/// fails the test rather than being skipped — here it can only mean the encoding
/// moved.
pub fn json_rows(db: &Path, query: &str, params: impl rusqlite::Params) -> Vec<Value> {
    rows(db, query, params)
        .iter()
        .map(|raw| serde_json::from_str(raw).unwrap_or_else(|error| panic!("a stored payload is not JSON ({error}): {raw}")))
        .collect()
}

/// Edit a row underneath a running server, and say how many rows changed.
///
/// The one deliberate exception to the paragraph above, and it is worth naming
/// why it has to exist: absolute session expiry cannot be observed by waiting a
/// week, and no route can move a login's expiry backwards. Reaching into the row
/// is the only way left to ask "does the check read the store, or something it
/// cached at login?".
///
/// The database must already be there. `open` would otherwise create an empty
/// one beside the real path and the `UPDATE` would cheerfully change nothing.
pub fn edit(db: &Path, statement: &str, params: impl rusqlite::Params) -> usize {
    assert!(db.is_file(), "{} does not exist; opening it would create an empty one", db.display());
    let conn = rusqlite::Connection::open(db).unwrap_or_else(|error| panic!("cannot open {}: {error}", db.display()));
    conn.busy_timeout(Duration::from_secs(5)).expect("set a busy timeout");
    conn.execute(statement, params)
        .unwrap_or_else(|error| panic!("cannot run `{statement}` against {}: {error}", db.display()))
}

/// Split a raw HTTP response into `(status, head, body)`, decoding a chunked body.
fn split_response(raw: &[u8]) -> (u16, String, Vec<u8>) {
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("malformed HTTP response: {}", String::from_utf8_lossy(raw)));
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("status line");
    let chunked = head.to_lowercase().contains("transfer-encoding: chunked");
    let body = if chunked {
        dechunk(&String::from_utf8_lossy(&raw[split + 4..])).into_bytes()
    } else {
        raw[split + 4..].to_vec()
    };
    (status, head, body)
}

/// Join a chunked body. Streaming responses (file downloads) arrive this way.
fn dechunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some((size, remainder)) = rest.split_once("\r\n") {
        let size = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if size == 0 || remainder.len() < size {
            break;
        }
        out.push_str(&remainder[..size]);
        rest = remainder[size..].strip_prefix("\r\n").unwrap_or("");
    }
    out
}

/// Run every session shim a server in this process starts as this test
/// binary's own [`session_shim`] entry, rather than as the server binary's
/// `session-shim` ([`droppen_provider::testing::shims_run_as`]).
///
/// The server's shim outlives whoever started it, which is what a server
/// needs: in a test it kept a shim and its CLI running for good behind every
/// test process that ended before its guards could run, killed by a timeout
/// or an interrupted run. The test binary's shim ends with the test process
/// that started it, and its CLI reads end of file and goes too.
pub fn shims_end_with_this_process() {
    droppen_provider::testing::shims_run_as(module_path!());
}

/// Not a test: what this binary runs as when a test starts it as one of its
/// helpers. Either the session shim of a server this binary runs in-process
/// ([`droppen_provider::testing::session_shim`], which only
/// [`shims_run_as`](droppen_provider::testing::shims_run_as) can name and
/// which names this entry), or a guard's [`Reaper`]; each does nothing unless
/// it was started as such.
#[test]
#[ignore = "the session shim and the reaper a test runs this binary as, not a test"]
fn session_shim() {
    droppen_provider::testing::session_shim();
    reap();
}

/// Where an instance on `home` keeps its run journal, built from `home`
/// alone. Never through [`Paths`]: that reads `DROPPEN_DATA_DIR`, which a
/// Droppen server sets in its shells when it is a second install, and would
/// point a guard at that server's own journal.
pub fn state_db_of(home: &Path, instance: &str) -> PathBuf {
    home.join(".local/share/remote-hand/instances").join(instance).join("state.db")
}

/// `home`, once it is known to be a directory of the test's own: absolute and
/// under the system's temporary directory, which every test home is made in.
/// And with neither `DROPPEN_DATA_DIR` nor `DROPPEN_CONFIG_DIR` set in this
/// process: a server under test would then read and write the directory they
/// name rather than its home, and a guard must never be pointed at another
/// install's files. Fails rather than kill anything outside.
pub fn test_home(home: &Path) -> &Path {
    for name in ["DROPPEN_DATA_DIR", "DROPPEN_CONFIG_DIR"] {
        assert!(
            std::env::var_os(name).is_none(),
            "{name} is set in this test process, so a server under test would use another install's files: unset it to run these tests"
        );
    }
    let plain = home
        .components()
        .all(|part| matches!(part, std::path::Component::RootDir | std::path::Component::Normal(_)));
    assert!(
        plain && home.is_absolute() && home.starts_with(std::env::temp_dir()) && home != std::env::temp_dir(),
        "{} is not a directory of this test's own under {}",
        home.display(),
        std::env::temp_dir().display()
    );
    home
}

/// Kill every CLI the run journal of `instance` under `home` names, each if it
/// is still the process its row records (`pid_token`): the residents and app
/// servers a server started, each in a process group of its own so that it
/// outlives the server, which is why no shutdown ends one. Its session shim
/// follows it out. Read-only, and every failure but a home that is not the
/// test's own is a silent return: the journal may not exist yet, its server
/// may be gone, and a guard that panicked would fail the test it is cleaning
/// up after.
pub fn kill_journal_runs(home: &Path, instance: &str) {
    let state_db = state_db_of(test_home(home), instance);
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI;
    let Ok(conn) = rusqlite::Connection::open_with_flags(&state_db, flags) else {
        return;
    };
    let Ok(mut statement) = conn.prepare("SELECT payload FROM runs") else {
        return;
    };
    let Ok(rows) = statement.query_map([], |row| row.get::<_, String>(0)) else {
        return;
    };
    for payload in rows.filter_map(Result::ok) {
        let Ok(record) = serde_json::from_str::<Value>(&payload) else { continue };
        let Some(pid) = record["cli_pid"].as_u64().and_then(|pid| u32::try_from(pid).ok()) else { continue };
        kill_group_of(pid, record["pid_token"].as_str());
    }
}

/// One process of the server binary that serves a test's home, as
/// [`server_processes_under`] found it.
pub struct ServerProcess {
    pub pid: u32,
    /// When it started, as `ps` read it in the same listing as its pid, so it
    /// is never signalled once its pid is another process's.
    started: String,
    /// Its arguments, its program first.
    pub args: Vec<String>,
}

impl ServerProcess {
    /// Whether `pid` is still this process: its start time, read again, is
    /// the one listed with it.
    fn is_still_running(&self) -> bool {
        let Ok(output) = std::process::Command::new("/bin/ps")
            .args(["-o", "lstart=", "-p", &self.pid.to_string()])
            .env("LC_ALL", "C")
            .output()
        else {
            return false;
        };
        same_start(&String::from_utf8_lossy(&output.stdout), &self.started)
    }

    /// Kill it, if it is still the process that was found.
    pub fn kill(&self) {
        if self.is_still_running() {
            // SAFETY: a signal to a process this test's server started,
            // checked to be the process that was listed.
            unsafe { libc::kill(self.pid as libc::pid_t, libc::SIGKILL) };
        }
    }

    /// Kill its process group, if it is still the process that was found.
    pub fn kill_group(&self) {
        if self.is_still_running() {
            // SAFETY: a signal to the group of a process this test's server
            // started, checked to be the process that was listed.
            unsafe { libc::kill(-(self.pid as libc::pid_t), libc::SIGKILL) };
        }
    }
}

/// Whether two `lstart` readings name the same moment, their spacing aside.
fn same_start(read: &str, listed: &str) -> bool {
    let words = |text: &str| text.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
    !listed.is_empty() && words(read) == words(listed)
}

/// Every process of the server binary (its program's file name is
/// `remote-hand-server`) whose command line names a path under `home`: the
/// `session-shim`s (their stderr file lives under it), the `pty-shim`s (their
/// socket does) and a server run from a copy installed there. Matched on
/// `<home>/` at the start of an argument or of an `--option=` value, read
/// with `ps -ww` together with each process's start time, so another test's
/// processes, running at the same time, are never among them.
pub fn server_processes_under(home: &Path) -> Vec<ServerProcess> {
    let home = test_home(home);
    // `lstart` in the C locale, where it is five words: in others (zh_CN, ja_JP)
    // it is four, and the program's path would be read as part of it.
    let Ok(output) = std::process::Command::new("/bin/ps")
        .args(["-A", "-ww", "-o", "pid=,lstart=,command="])
        .env("LC_ALL", "C")
        .output()
    else {
        return Vec::new();
    };
    let under = format!("{}/", home.display());
    let mut found = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        // `lstart` is five words: weekday, month, day, time and year.
        let mut words = line.split_whitespace();
        let Some(pid) = words.next().and_then(|pid| pid.parse::<u32>().ok()) else { continue };
        let started: Vec<&str> = words.by_ref().take(5).collect();
        let args: Vec<String> = words.map(str::to_owned).collect();
        if started.len() < 5 {
            continue;
        }
        let program = args.first().and_then(|program| Path::new(program).file_name());
        let names_home = args
            .iter()
            .any(|arg| arg.starts_with(&under) || arg.split_once('=').is_some_and(|(_, value)| value.starts_with(&under)));
        if program != Some(std::ffi::OsStr::new("remote-hand-server")) || !names_home {
            continue;
        }
        found.push(ServerProcess { pid, started: started.join(" "), args });
    }
    found
}

/// Kill every process of the server binary serving `home`
/// ([`server_processes_under`]), each if it is still the process that was
/// found: a session shim's CLI reads end of file once its shim is gone, and a
/// terminal's shell hangs up with its shim.
pub fn kill_shims_serving(home: &Path) {
    for process in server_processes_under(home) {
        process.kill();
    }
}

/// Kills a process group when it goes out of scope, if its leader is still
/// the process its start token was taken from ([`kill_group_of`]).
///
/// Both the server and the CLI it spawns lead groups of their own — the server
/// because [`spawn_server`] says so, the CLI because that is what lets it
/// outlive the server — so a group signal is what reaches every child they
/// forked.
pub struct KillGroupOnDrop {
    pid: u32,
    token: Option<String>,
}

impl KillGroupOnDrop {
    /// The group a run's CLI leads: its `cli_pid` and `pid_token`, as the run
    /// journal records them.
    pub fn run(record: &Value) -> KillGroupOnDrop {
        KillGroupOnDrop {
            pid: record["cli_pid"].as_u64().and_then(|pid| u32::try_from(pid).ok()).expect("cli_pid"),
            token: record["pid_token"].as_str().map(str::to_owned),
        }
    }
}

impl Drop for KillGroupOnDrop {
    fn drop(&mut self) {
        kill_group_of(self.pid, self.token.as_deref());
    }
}

/// A terminal running `script` under `/bin/sh`.
pub fn sh(script: &str) -> droppen_workbench::terminals::CreateSpec {
    droppen_workbench::terminals::CreateSpec::new("/bin/sh", vec!["-c".to_string(), script.to_string()])
}
