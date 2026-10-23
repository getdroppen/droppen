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
//! (`crate::auth::setup`). Almost no test here is about that, so the ordinary
//! starts — [`TestServer::start`], [`TestServer::start_with`],
//! [`TestServer::start_at`] — switch authentication **off** explicitly, the
//! `auth.enabled = false` escape hatch, rather than every test logging in. The
//! tests that are about authentication use [`TestServer::start_guarded`] and
//! [`TestServer::start_guarded_at`], which leave the default alone.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use remote_hand_server::app::RemoteHandApp;
use remote_hand_server::sessions::Paths;
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
    global: std::sync::Arc<remote_hand_server::feed::global::GlobalFeed>,
    terminals: std::sync::Arc<remote_hand_server::terminals::Registry>,
    cancel: CancellationToken,
    /// An `Option` so `stop` can take it: this struct has a `Drop`, and a type
    /// with one cannot have a field moved out of it.
    server: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
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
        let global = prepared.global();
        let terminals = prepared.cleanups().terminals.clone();
        let server = tokio::spawn(prepared.serve_task());
        TestServer {
            addr,
            home: home.to_path_buf(),
            // The same instance the server was given: the Claude root it owns
            // hangs off it, so a `Paths` built for `default` here would point a
            // sibling instance's test at the wrong directory.
            paths: Paths::for_instance(home.to_path_buf(), instance.unwrap_or(remote_hand_server::instance::DEFAULT_INSTANCE)),
            global,
            terminals,
            cancel,
            server: Some(server),
            _home: None,
        }
    }

    /// Whether the global feed's watcher is running — i.e. a client is attached.
    pub fn feed_is_running(&self) -> bool {
        self.global.is_running()
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
        let mut stream = tokio::net::TcpStream::connect(self.addr).await.expect("connect");
        // The server's own address, unless the case names another `Host`.
        let mut request = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\n");
        if !headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("host")) {
            request.push_str(&format!("Host: {}\r\n", self.addr));
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
    /// Kill whatever terminals the test started.
    ///
    /// A web terminal now belongs to a detached `pty-shim` process that is
    /// *designed* to outlive the server — which is right in production and
    /// wrong in a test binary, where it would leave stray shells behind on the
    /// developer's machine. `kill_all` is the deliberate escape hatch for
    /// exactly this, and it signals rather than sending frames so it works from
    /// a `Drop` with no runtime to await on.
    fn drop(&mut self) {
        self.terminals.kill_all();
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
fn split_response(raw: &[u8]) -> (u16, String, String) {
    let text = String::from_utf8_lossy(raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").expect("malformed HTTP response");
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("status line");
    let chunked = head.to_lowercase().contains("transfer-encoding: chunked");
    (status, head.to_string(), if chunked { dechunk(body) } else { body.to_string() })
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
