//! End-to-end proof of the terminal subsystem: the four REST routes, the attach
//! websocket, and the internal API the account wizards are built on.
//!
//! Every terminal here runs a deterministic `/bin/sh -c …` rather than a login
//! shell, so the assertions are about bytes the test itself put on the wire. The
//! one exception is the REST create route, which exists precisely to start the
//! user's own shell.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use remote_hand_server::app::RemoteHandApp;
use remote_hand_server::instance::{InstanceDirs, DEFAULT_INSTANCE};
use remote_hand_server::pty_shim::proto::Meta;
use remote_hand_server::pty_shim::sidecar::{self, Sidecars};
use remote_hand_server::sessions::Paths;
use remote_hand_server::shutdown::Cleanups;
use remote_hand_server::terminals::{CreateSpec, Registry, MAX_TERMINALS};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

/// A running server plus the terminal registry its handlers share.
struct Probe {
    addr: SocketAddr,
    terminals: Arc<Registry>,
    cleanups: Cleanups,
    cancel: tokio_util::sync::CancellationToken,
    /// An `Option` so `stop` can take it out past this struct's `Drop`.
    server: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
    home: tempfile::TempDir,
}

impl Probe {
    async fn start() -> Probe {
        let home = tempfile::tempdir().expect("tempdir");
        let app = RemoteHandApp {
            home_override: Some(home.path().to_path_buf()),
            port_override: Some(0),
            // No login: these tests are not about authentication (`tests/auth.rs` is).
            auth_enabled_override: Some(false),
            ..Default::default()
        };
        let prepared = app.prepare().await.expect("prepare");
        let addr = prepared.addr;
        let cleanups = prepared.cleanups();
        let terminals = cleanups.terminals.clone();
        let cancel = prepared.cancel();
        let server = tokio::spawn(prepared.serve_task());
        Probe {
            addr,
            terminals,
            cleanups,
            cancel,
            server: Some(server),
            home,
        }
    }

    /// Where this probe's shims keep their sidecars.
    fn shim_dir(&self) -> std::path::PathBuf {
        shims_of(self.home.path())
    }

    async fn request(&self, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
        http_request(self.addr, method, path, body).await
    }

    async fn json(&self, method: &str, path: &str, body: Option<&str>) -> Value {
        let (status, text) = self.request(method, path, body).await;
        assert_eq!(status, 200, "{method} {path} -> {status}: {text}");
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{method} {path} returned non-JSON ({e}): {text}"))
    }

    /// Connect to a terminal's attach socket.
    async fn attach(&self, id: &str) -> Socket {
        let url = format!("ws://{}/api/terminals/{id}/ws", self.addr);
        let (socket, _) = tokio_tungstenite::connect_async(&url).await.expect("websocket connect");
        Socket(socket)
    }

    /// Stop the server, and kill the shims the test started.
    ///
    /// A terminal now outlives its server on purpose, so a test that only
    /// cancelled would leave shells running on the developer's machine.
    async fn stop(mut self) {
        self.terminals.kill_all();
        self.cancel.cancel();
        if let Some(server) = self.server.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
        }
    }
}

impl Drop for Probe {
    /// The same safety net for a test that panics before it reaches `stop`.
    fn drop(&mut self) {
        self.terminals.kill_all();
    }
}

/// A connected attach socket, with the read/write helpers the assertions need.
struct Socket(tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>);

impl Socket {
    async fn send(&mut self, frame: &str) {
        self.0.send(Message::Text(frame.into())).await.expect("send control frame");
    }

    /// The next text frame, or `None` once the server closed the socket.
    async fn next_text(&mut self) -> Option<String> {
        loop {
            let message = tokio::time::timeout(Duration::from_secs(10), self.0.next())
                .await
                .expect("timed out waiting for a frame")?
                .expect("websocket error");
            match message {
                Message::Text(text) => return Some(text.to_string()),
                Message::Close(_) => return None,
                _ => continue,
            }
        }
    }

    /// Read frames until everything in `needles` has been seen, returning the
    /// concatenation. The pty coalesces on a 16 ms timer, so which bytes land in
    /// which frame is deliberately not asserted — only the stream they form.
    async fn read_until(&mut self, needles: &[&str]) -> String {
        let mut seen = String::new();
        while !needles.iter().all(|needle| seen.contains(needle)) {
            match self.next_text().await {
                Some(text) => seen.push_str(&text),
                None => panic!("socket closed before {needles:?} arrived; saw: {seen:?}"),
            }
        }
        seen
    }
}

/// A one-shot HTTP/1.1 request, returning `(status, body)`.
async fn http_request(addr: SocketAddr, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if let Some(body) = body {
        request.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    if let Some(body) = body {
        request.push_str(body);
    }
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read");
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").expect("malformed HTTP response");
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("status line");
    (status, body.to_string())
}

/// A terminal running `script` under `/bin/sh`.
fn sh(script: &str) -> CreateSpec {
    CreateSpec::new("/bin/sh", vec!["-c".to_string(), script.to_string()])
}

/// Ask a shim who it is minding, without disturbing the server's own link.
///
/// `u64::MAX` as the cursor is the "peek" idiom: the ring clamps it to the head,
/// so this connection is told the shell's pid and then handed no output at all.
async fn read_shell_pid(shims: &std::path::Path, id: &str) -> u32 {
    let link = remote_hand_server::pty_shim::client::connect(&sidecars(shims, id).socket(), u64::MAX)
        .await
        .expect("connect to the shim");
    link.shell_pid
}

fn sidecars(shims: &Path, id: &str) -> Sidecars {
    Sidecars::new(shims, id)
}

/// The default instance of a home directory, and where its shims keep their
/// sidecars.
fn instance_of(home: &Path) -> InstanceDirs {
    InstanceDirs::new(&Paths::new(home.to_path_buf()).data_dir(), DEFAULT_INSTANCE).expect("the default instance name is valid")
}

fn shims_of(home: &Path) -> std::path::PathBuf {
    instance_of(home).pty_shims()
}

/// A registry over `home`'s default instance, with no server around it.
///
/// Adoption needs two registries over one home directory, one after the other,
/// which a [`Probe`] cannot give: it owns its tempdir, and the whole point of a
/// surviving terminal is that it outlives whatever started it.
fn registry_at(home: &Path) -> Arc<Registry> {
    let dirs = instance_of(home);
    dirs.create().expect("create the instance directory");
    Registry::new(Arc::new(Paths::new(home.to_path_buf())), Arc::new(dirs))
}

/// A uuid-shaped id whose first sixteen hex digits are `n`'s.
///
/// The socket is named after those digits (`pty_shim::sidecar`), so ids that
/// only differ further along would all try to bind the same address.
fn seeded_id(n: usize) -> String {
    format!("{n:08x}-1111-4000-8000-000000000000")
}

/// Write the identity of a terminal whose server is gone. `created_seq` is what
/// a restart puts the user's tabs back in order by.
async fn seed_identity(shims: &Path, n: usize) -> Sidecars {
    let id = seeded_id(n);
    let sidecars = sidecars(shims, &id);
    sidecars
        .write_meta(&Meta {
            id,
            title: format!("seeded {n}"),
            cwd: "/tmp".to_string(),
            created_seq: n as u64,
            shell: "/bin/sh".to_string(),
            created_at: "2026-01-01T00:00:00.000Z".to_string(),
        })
        .await
        .expect("write meta");
    sidecars
}

/// An identity whose socket is bound and answers nothing at all.
///
/// This is what a wedged shim looks like to adoption: the connection is
/// accepted, so it is not cheaply refused, and the handshake then runs out —
/// which is the cost that used to be paid one terminal at a time. The listener
/// is returned because dropping it would unbind the socket.
async fn seed_mute_shim(shims: &Path, n: usize) -> tokio::net::UnixListener {
    let sidecars = seed_identity(shims, n).await;
    sidecars.ensure_socket_dir().expect("the shim socket directory");
    tokio::net::UnixListener::bind(sidecars.socket()).expect("bind a mute shim socket")
}

fn alive(pid: u32) -> bool {
    remote_hand_server::procs::alive(pid)
}

fn kill_pid(pid: u32) {
    remote_hand_server::pty_shim::child::kill_shell(pid, libc::SIGKILL);
}

#[tokio::test]
async fn the_rest_routes_list_create_rename_and_kill() {
    let probe = Probe::start().await;

    assert_eq!(
        probe.json("GET", "/api/terminals", None).await["terminals"]
            .as_array()
            .expect("array")
            .len(),
        0
    );

    // The web sends `{}`; the shell then starts in the home directory.
    let created = probe.json("POST", "/api/terminals", Some("{}")).await;
    let id = created["id"].as_str().expect("id").to_string();
    assert_eq!(created["title"], "terminal 1", "the default title counts terminals");
    assert_eq!(created["cwd"], probe.home.path().to_string_lossy().as_ref());
    assert!(created["createdAt"].as_str().expect("createdAt").contains('T'));

    let listed = probe.json("GET", "/api/terminals", None).await;
    assert_eq!(listed["terminals"].as_array().expect("array").len(), 1);
    assert_eq!(listed["terminals"][0]["id"], id.as_str());

    let renamed = probe
        .json("PATCH", &format!("/api/terminals/{id}"), Some(r#"{"title":"  build  "}"#))
        .await;
    assert_eq!(renamed["title"], "build", "the title is trimmed");

    let killed = probe.json("DELETE", &format!("/api/terminals/{id}"), None).await;
    assert_eq!(killed["accepted"], true);

    // Killing removes the entry outright, so a second attempt is a 404 with the
    // body the web client reads.
    let (status, body) = probe.request("DELETE", &format!("/api/terminals/{id}"), None).await;
    assert_eq!(status, 404);
    assert_eq!(serde_json::from_str::<Value>(&body).expect("json")["error"], "terminal not found");

    let (status, body) = probe.request("PATCH", &format!("/api/terminals/{id}"), Some(r#"{"title":"gone"}"#)).await;
    assert_eq!(status, 404);
    assert_eq!(serde_json::from_str::<Value>(&body).expect("json")["error"], "terminal not found");

    probe.stop().await;
}

#[tokio::test]
async fn the_list_keeps_creation_order() {
    let probe = Probe::start().await;

    // The web renders `terminals` unsorted, so a reshuffle between polls would
    // move the user's tabs under them.
    let mut created = Vec::new();
    for _ in 0..8 {
        created.push(probe.terminals.create_terminal_with(sh("sleep 30")).await.expect("create terminal").id);
    }

    let listed: Vec<String> = probe.json("GET", "/api/terminals", None).await["terminals"]
        .as_array()
        .expect("array")
        .iter()
        .map(|t| t["id"].as_str().expect("id").to_string())
        .collect();
    assert_eq!(listed, created);

    // Removing one from the middle must not disturb the rest.
    assert!(probe.terminals.kill_terminal(&created[3]));
    created.remove(3);
    let listed: Vec<String> = probe.json("GET", "/api/terminals", None).await["terminals"]
        .as_array()
        .expect("array")
        .iter()
        .map(|t| t["id"].as_str().expect("id").to_string())
        .collect();
    assert_eq!(listed, created);

    probe.stop().await;
}

#[tokio::test]
async fn a_working_directory_outside_home_is_refused() {
    let probe = Probe::start().await;

    let (status, body) = probe.request("POST", "/api/terminals", Some(r#"{"cwd":"/etc"}"#)).await;
    assert_eq!(status, 400, "the multi-status response the OpenAPI registry documents");
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("json")["error"],
        "path is outside the home directory"
    );

    // A path inside home is accepted, `..` and all.
    let inside = format!("{}/./sub/..", probe.home.path().to_string_lossy());
    let created = probe
        .json("POST", "/api/terminals", Some(&serde_json::json!({ "cwd": inside }).to_string()))
        .await;
    assert_eq!(created["cwd"], probe.home.path().to_string_lossy().as_ref(), "the path is normalized");

    probe.stop().await;
}

#[tokio::test]
async fn output_reaches_the_socket_and_the_exit_is_announced() {
    let probe = Probe::start().await;
    // The second line waits on the test rather than on the clock. What this
    // needs is one line before the attach and one after it, and a `sleep` in the
    // script is a guess at when the attach happens; a `read` is the attach
    // itself saying so.
    let info = probe
        .terminals
        .create_terminal_with(sh("echo hello; read _; echo bye"))
        .await
        .expect("create terminal");

    let mut socket = probe.attach(&info.id).await;
    // The replay covers whatever landed before the attach; later frames carry
    // the rest. Both are bare text — no envelope, exactly what `term.write` eats.
    // The line's end can come a frame after its text, so wait for both.
    let before = socket.read_until(&["hello", "\n"]).await;
    assert!(
        before.contains("hello\r\n"),
        "the pty's own CRLF translation is passed through: {before:?}"
    );

    socket.send(r#"{"type":"input","data":"\r"}"#).await;
    let after = socket.read_until(&["bye", "[process exited]"]).await;
    assert!(after.contains("bye\r\n"), "output produced after the attach arrives too: {after:?}");
    assert!(
        after.ends_with("\r\n[process exited]\r\n"),
        "the exit note is the last thing on the wire: {after:?}"
    );

    // An exited terminal drops out of the list but stays attachable for a while,
    // so a client that reconnects still repaints the note.
    let listed = probe.json("GET", "/api/terminals", None).await;
    assert_eq!(listed["terminals"].as_array().expect("array").len(), 0);

    probe.stop().await;
}

#[tokio::test]
async fn a_resize_frame_reaches_the_kernel_and_junk_is_ignored() {
    let probe = Probe::start().await;
    // `stty size` prints "<rows> <cols>" as the kernel knows them. The shell
    // polls for the resize instead of sleeping a fixed beat: on a loaded CI
    // runner the resize frame can take well over half a second to land, and a
    // fixed sleep loses that race. A junk resize that wrongly applied leaves
    // the size unmatched, so the loop runs out and prints the wrong size for
    // the assertion to catch.
    let info = probe
        .terminals
        .create_terminal_with(sh(
            "i=0; while [ \"$(stty size)\" != \"30 100\" ] && [ $i -lt 100 ]; do i=$((i+1)); sleep 0.1; done; stty size",
        ))
        .await
        .expect("create terminal");

    let mut socket = probe.attach(&info.id).await;
    socket.send(r#"{"type":"resize","cols":100,"rows":30}"#).await;
    // Everything below must be dropped in silence rather than closing the socket
    // or reaching `pty.resize`: out of range, wrong types, unknown type, junk.
    socket.send(r#"{"type":"resize","cols":5000,"rows":30}"#).await;
    socket.send(r#"{"type":"resize","cols":0,"rows":0}"#).await;
    socket.send(r#"{"type":"resize","cols":"80","rows":"24"}"#).await;
    socket.send(r#"{"type":"nonsense"}"#).await;
    socket.send("not json at all").await;

    let seen = socket.read_until(&["[process exited]"]).await;
    assert!(seen.contains("30 100"), "the 100x30 resize is the only one that should apply: {seen:?}");

    probe.stop().await;
}

#[tokio::test]
async fn input_frames_reach_the_process() {
    let probe = Probe::start().await;
    let info = probe
        .terminals
        .create_terminal_with(sh("read line; echo \"got:$line\""))
        .await
        .expect("create terminal");

    let mut socket = probe.attach(&info.id).await;
    socket.send(r#"{"type":"input","data":"ping\r"}"#).await;

    let seen = socket.read_until(&["got:ping"]).await;
    assert!(seen.contains("got:ping"), "{seen:?}");

    probe.stop().await;
}

#[tokio::test]
async fn every_attached_client_sees_the_same_stream() {
    let probe = Probe::start().await;
    // The line waits on the test, not on the clock. `sleep 0.3` was a guess
    // at when both clients would be attached; on a loaded machine the shell
    // had printed and exited first, and an exited terminal refuses an attach.
    let info = probe
        .terminals
        .create_terminal_with(sh("read _; echo shared"))
        .await
        .expect("create terminal");

    let mut first = probe.attach(&info.id).await;
    let mut second = probe.attach(&info.id).await;
    // Each attach opens with the replay frame, which is the server saying it
    // has registered that client. Only then is the line asked for.
    first.next_text().await.expect("the first client's replay");
    second.next_text().await.expect("the second client's replay");
    first.send(r#"{"type":"input","data":"\r"}"#).await;

    assert!(first.read_until(&["shared"]).await.contains("shared"));
    assert!(second.read_until(&["shared"]).await.contains("shared"));

    probe.stop().await;
}

#[tokio::test]
async fn an_unknown_terminal_gets_a_note_and_a_close() {
    let probe = Probe::start().await;
    let mut socket = probe.attach("11111111-2222-3333-4444-555555555555").await;

    assert_eq!(socket.next_text().await.as_deref(), Some("\r\n[terminal not found]\r\n"));
    assert_eq!(socket.next_text().await, None, "the server closes the socket after the note");

    probe.stop().await;
}

#[tokio::test]
async fn the_exit_hook_fires_once_on_a_natural_exit() {
    let probe = Probe::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let codes: Arc<Mutex<Vec<i32>>> = Arc::new(Mutex::new(Vec::new()));

    let mut spec = sh("exit 7");
    spec.on_exit = Some({
        let calls = calls.clone();
        let codes = codes.clone();
        Box::new(move |code| {
            calls.fetch_add(1, Ordering::SeqCst);
            codes.lock().expect("codes poisoned").push(code);
        })
    });
    probe.terminals.create_terminal_with(spec).await.expect("create terminal");

    wait_for(|| calls.load(Ordering::SeqCst) == 1, "the exit hook to fire").await;
    // The list drops a terminal the moment the lifecycle task notes its exit,
    // which is past every point at which it could have fired the hook again —
    // a condition to wait on rather than a guess at how long that takes.
    wait_for(|| probe.terminals.list().is_empty(), "the terminal to be marked exited").await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the hook must fire exactly once");
    assert_eq!(*codes.lock().expect("codes poisoned"), vec![7], "the process's own status is reported");

    probe.stop().await;
}

#[tokio::test]
async fn the_exit_hook_fires_once_on_a_kill() {
    let probe = Probe::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let codes: Arc<Mutex<Vec<i32>>> = Arc::new(Mutex::new(Vec::new()));

    // Long-lived on purpose: the hook must come from the kill, not from an exit.
    let mut spec = sh("sleep 30");
    spec.on_exit = Some({
        let calls = calls.clone();
        let codes = codes.clone();
        Box::new(move |code| {
            calls.fetch_add(1, Ordering::SeqCst);
            codes.lock().expect("codes poisoned").push(code);
        })
    });
    let info = probe.terminals.create_terminal_with(spec).await.expect("create terminal");
    let mut socket = probe.attach(&info.id).await;
    // The server attaches after the upgrade response the client has already
    // read, and its first frame (the replay) is what says it has. A kill that
    // arrived before that would earn `[terminal not found]` instead.
    socket.next_text().await.expect("the replay frame");

    assert!(probe.terminals.kill_terminal(&info.id));
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the kill fires the hook synchronously");
    assert_eq!(
        *codes.lock().expect("codes poisoned"),
        vec![-1],
        "a killed process has no status to report"
    );

    // The process really does die, and its own exit must not fire the hook
    // again. `[process exited]` is the lifecycle task's own last act before it
    // stands down, so seeing it means every path that could fire has now run.
    socket.read_until(&["[process exited]"]).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the hook must fire exactly once");
    assert!(!probe.terminals.kill_terminal(&info.id), "the entry is gone");

    probe.stop().await;
}

#[tokio::test]
async fn the_raw_output_hook_sees_the_stream_before_coalescing() {
    let probe = Probe::start().await;
    let seen: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));

    let mut spec = sh("echo scanned");
    spec.on_data = Some({
        let seen = seen.clone();
        Box::new(move |chunk| seen.lock().expect("seen poisoned").push_str(chunk))
    });
    probe.terminals.create_terminal_with(spec).await.expect("create terminal");

    wait_for(|| seen.lock().expect("seen poisoned").contains("scanned"), "the scanner to see output").await;

    probe.stop().await;
}

#[tokio::test]
async fn clearing_the_replay_drops_the_scrollback() {
    let probe = Probe::start().await;
    let info = probe
        .terminals
        .create_terminal_with(sh("echo sk-ant-pretend-token; sleep 30"))
        .await
        .expect("create terminal");

    wait_for(
        || {
            probe
                .terminals
                .attach(&info.id)
                .is_some_and(|attachment| attachment.replay.contains("sk-ant-pretend-token"))
        },
        "the credential to reach the replay buffer",
    )
    .await;

    assert!(probe.terminals.clear_terminal_replay(&info.id));

    let attachment = probe.terminals.attach(&info.id).expect("still attachable");
    assert_eq!(attachment.replay, "\r\n[scrollback cleared: it held a credential]\r\n");
    assert!(!probe.terminals.clear_terminal_replay("no-such-terminal"));

    probe.stop().await;
}

/// Shutdown *detaches*: the sockets close and the registry empties, but the
/// shell keeps running under its shim, which is the whole point of the feature.
#[tokio::test]
async fn shutdown_detaches_every_terminal_and_leaves_its_shell_alive() {
    let probe = Probe::start().await;
    let calls = Arc::new(AtomicUsize::new(0));

    let mut spec = sh("sleep 30");
    spec.on_exit = Some({
        let calls = calls.clone();
        Box::new(move |_| {
            calls.fetch_add(1, Ordering::SeqCst);
        })
    });
    let info = probe.terminals.create_terminal_with(spec).await.expect("create terminal");
    let mut socket = probe.attach(&info.id).await;
    // Every attach opens with one frame — empty here, the shell having printed
    // nothing — so take it now and leave the assertion below about the close.
    assert_eq!(socket.next_text().await.as_deref(), Some(""), "the opening frame of a silent terminal");
    let shims = probe.shim_dir();
    let shell_pid = read_shell_pid(&shims, &info.id).await;

    let mut probe = probe;
    let terminals = probe.terminals.clone();
    let addr = probe.addr;
    let server = probe.server.take().expect("the server task");
    // Exactly what `shutdown::supervise` does on SIGTERM: cancel, then run the
    // registered cleanups.
    probe.cancel.cancel();
    probe.cleanups.run();

    // The browser socket closes with the listener, as it always did.
    assert_eq!(socket.next_text().await, None, "a live attach socket must be closed on shutdown");
    let stopped = tokio::time::timeout(Duration::from_secs(5), server).await;
    assert!(stopped.is_ok(), "the server task must finish after cancellation");
    assert!(terminals.is_empty(), "the registry lets go of every terminal");
    assert_eq!(calls.load(Ordering::SeqCst), 0, "a detached terminal has not exited, so no hook fires");

    // What is different from every previous version of this subsystem: the
    // shell, its shim and its identity are all still there to be adopted.
    assert!(alive(shell_pid), "the shell must survive the server that started it");
    assert!(shims.join(format!("{}.meta.json", info.id)).exists(), "the identity survives too");
    assert!(sidecars(&shims, &info.id).socket().exists(), "and so does the socket");

    wait_for_async(
        || async move { tokio::net::TcpStream::connect(addr).await.is_err() },
        "the port to be released",
    )
    .await;

    // Left running by shutdown, so this test cleans up after itself.
    terminals.kill_all();
    kill_pid(shell_pid);
}

/// Poll `condition` until it holds, or fail after five seconds.
async fn wait_for(mut condition: impl FnMut() -> bool, what: &str) {
    for _ in 0..250 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

/// The same, for a condition that has to be awaited.
async fn wait_for_async<F, Fut>(mut condition: F, what: &str)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..250 {
        if condition().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

/// A browser tab that stops draining its socket must not be able to hold the pty
/// reader — or every other client — hostage.
///
/// The fan-out cannot drop chunks the way the session feed's can (a terminal
/// stream is not re-derivable from a later frame), so the only bound available
/// is on the client: past the per-listener queue cap it is told why and closed.
#[tokio::test]
async fn a_client_that_stops_reading_is_disconnected_rather_than_waited_for() {
    let probe = Probe::start().await;
    // The cheapest unbounded producer there is.
    let info = probe
        .terminals
        .create_terminal_with(sh("yes remote-hand"))
        .await
        .expect("create terminal");

    let mut slow = probe.attach(&info.id).await;
    let mut fast = probe.attach(&info.id).await;
    // Where a failure got to, printed with the test's output only when it fails.
    let started = std::time::Instant::now();

    // `slow` is never polled, so its queue grows past the cap; `fast` drains as
    // fast as it can, which is what proves the reader was never held up.
    let mut drained = 0usize;
    while drained < 24 * 1024 * 1024 {
        match fast.next_text().await {
            Some(text) => drained += text.len(),
            None => panic!("the fast client was closed after {drained} bytes; only the slow one should be"),
        }
    }
    eprintln!("{:?}: the fast client has read {drained} bytes", started.elapsed());

    // Now read the slow client out: its backlog, then the note, then the close.
    // Bounded, so an unbounded queue fails the test instead of hanging it.
    //
    // It reads at an unhurried pace and says something after every frame, the
    // way a browser answers pings and sends keystrokes. That keeps the server's
    // buffer full, so sending the note waits, and whatever the client said
    // during that wait is still unread when the server is done. A server that
    // dropped the socket straight after its close frame then got a reset out
    // of the kernel, which threw away everything not yet transmitted, the note
    // included. Answering the client's close first is what keeps the tail.
    let mut told_why = false;
    let mut backlog = 0usize;
    while let Some(text) = slow.next_text().await {
        told_why |= text.contains(remote_hand_server::terminals::SLOW_NOTE.trim());
        backlog += text.len();
        assert!(backlog < 32 * 1024 * 1024, "the slow client's queue is not bounded at all");
        // Unchecked: whether the server is still there to read it is the
        // question the read loop answers.
        let _ = slow.0.send(Message::Text(r#"{"type":"still-here"}"#.into())).await;
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    eprintln!("{:?}: the slow client read {backlog} bytes and was closed", started.elapsed());
    assert!(told_why, "a disconnected client must be told why");

    // The fast client is untouched by its neighbour's disconnection.
    assert!(fast.next_text().await.is_some(), "the other client keeps its stream");

    assert!(probe.terminals.kill_terminal(&info.id));
    probe.stop().await;
}

/// Each terminal costs a pty, three threads and a replay buffer. A client
/// looping on the create route should meet a refusal, not the descriptor limit.
#[tokio::test]
async fn a_full_registry_answers_429_rather_than_opening_another_pty() {
    let probe = Probe::start().await;

    // `true` exits at once, but an exited terminal stays registered until it is
    // reaped — so it still counts against the cap, which is the point.
    let mut created = 0;
    loop {
        let (status, body) = probe.request("POST", "/api/terminals", Some("{}")).await;
        if status == 429 {
            assert_eq!(serde_json::from_str::<Value>(&body).expect("json")["error"], "too many terminals");
            break;
        }
        assert_eq!(status, 200, "create #{created} -> {status}: {body}");
        created += 1;
        assert!(created <= 256, "the registry accepted {created} terminals without a cap");
    }
    assert!(created >= 8, "the cap must leave room for real use, got {created}");

    probe.stop().await;
}

/// The client arms a screen reset when it re-attaches and disarms it on the
/// first frame it receives (`web/src/features/terminals/xterm-pane.tsx`). A
/// terminal with nothing buffered used to send no frame at all, so the reset
/// stayed armed — and fired on whatever came next, which could be a keystroke
/// minutes later wiping the screen the user was reading.
#[tokio::test]
async fn an_attach_opens_with_a_frame_even_when_nothing_is_buffered() {
    let probe = Probe::start().await;
    // Prints nothing, ever: the replay is empty and stays empty.
    let info = probe.terminals.create_terminal_with(sh("sleep 30")).await.expect("create terminal");
    assert_eq!(
        probe.terminals.attach(&info.id).expect("attachable").replay,
        "",
        "this terminal has produced no output at all"
    );

    let mut socket = probe.attach(&info.id).await;
    assert_eq!(
        socket.next_text().await.as_deref(),
        Some(""),
        "an attach is announced by an empty frame, not by silence"
    );

    // And by *one* frame: the opening frame is not a stream of empty ones.
    let next = tokio::time::timeout(Duration::from_millis(500), socket.next_text()).await;
    assert!(next.is_err(), "a silent terminal says nothing more after its opening frame, got {next:?}");

    probe.stop().await;
}

/// Adoption used to be serial, and it runs before the listener is bound: every
/// shim that would not answer cost a whole handshake timeout before the next
/// terminal was even tried. A user with ten dead shims waited minutes for a
/// server that had not started listening.
#[tokio::test]
async fn a_directory_of_dead_shims_does_not_hold_the_live_ones_up() {
    let home = tempfile::tempdir().expect("tempdir");
    let first = registry_at(home.path());
    let mut created = Vec::new();
    for _ in 0..3 {
        created.push(first.create_terminal_with(sh("sleep 30")).await.expect("create terminal").id);
    }
    // Detached, not killed: the shims stay behind, which is what a restart finds.
    first.shutdown_all();

    let shims = shims_of(home.path());
    // Seeded after the live ones, so their own order is not what is asserted.
    let mut mute = Vec::new();
    for n in 0..6 {
        mute.push(seed_mute_shim(&shims, 100 + n).await);
    }

    let second = registry_at(home.path());
    let started = std::time::Instant::now();
    let adopted = second.readopt_all().await;
    let elapsed = started.elapsed();

    assert_eq!(adopted, 3, "every shim with a live shell behind it is adopted");
    let listed: Vec<String> = second.list().into_iter().map(|info| info.id).collect();
    assert_eq!(listed, created, "adopted tabs come back in the order the user made them");
    // Generous on purpose: concurrently this is one handshake timeout for all
    // six, and serially it was six of them end to end.
    assert!(
        elapsed < Duration::from_secs(20),
        "six mute shims took {elapsed:?}, which is the shape of one timeout after another"
    );

    second.kill_all();
    drop(mute);
}

/// A crash-prone instance directory can accumulate more identities than the
/// registry is allowed to hold terminals. Adoption honours the ceiling the
/// create path enforces, or a stuffed directory would be the way around it.
#[tokio::test]
async fn adoption_honours_the_cap_the_create_path_enforces() {
    let home = tempfile::tempdir().expect("tempdir");
    let registry = registry_at(home.path());
    let shims = shims_of(home.path());

    const OVER: usize = 8;
    // No sockets, so none of these is adoptable — which is exactly what makes
    // the ones that were *considered* visible: adoption sweeps an identity whose
    // shim is gone, and never touches the ones past the cap.
    for n in 0..MAX_TERMINALS + OVER {
        seed_identity(&shims, n).await;
    }

    assert_eq!(registry.readopt_all().await, 0, "an identity with no shim is debris, not a terminal");
    let left = sidecar::scan(&shims).await;
    let newest: Vec<String> = (MAX_TERMINALS..MAX_TERMINALS + OVER).map(seeded_id).collect();
    assert_eq!(
        left, newest,
        "the cap's worth of oldest tabs was considered; the rest keep their identities for a boot with room"
    );
}
