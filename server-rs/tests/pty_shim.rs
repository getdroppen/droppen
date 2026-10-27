//! Restart survival, end to end.
//!
//! Every test here is about one question: **is the shell still there?** So the
//! restarts are real ones. Most are a full `RemoteHandApp` stopped and started
//! again over the same home directory — which releases and retakes the instance
//! lock, rescans the instance directory and reattaches, exactly as a service
//! restart does. One is realer still: the actual binary, killed with `SIGKILL`
//! through its whole process group, and a second invocation that adopts what it
//! left behind.
//!
//! The terminals run deterministic `/bin/sh -c …` scripts rather than a login
//! shell, so every assertion is about bytes the test itself put on the wire.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use remote_hand_server::app::RemoteHandApp;
use remote_hand_server::instance::{InstanceDirs, InstanceLock, DEFAULT_INSTANCE};
use remote_hand_server::pty_shim::proto::{self, Frame, Hello2, Meta};
use remote_hand_server::pty_shim::sidecar::Sidecars;
use remote_hand_server::sessions::Paths;
use remote_hand_server::shutdown::Cleanups;
use remote_hand_server::terminals::{Chunk, CreateSpec, Registry};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

// ------------------------------------------------------------------- harness

/// One running server over a home directory the test owns.
struct Boot {
    addr: SocketAddr,
    terminals: std::sync::Arc<Registry>,
    /// This instance's `pty-shims` directory, so `halt` can find the sockets of
    /// the terminals it is about to let go of.
    shims: std::path::PathBuf,
    cleanups: Cleanups,
    cancel: CancellationToken,
    /// An `Option` so `halt` can take it past this struct's `Drop`.
    task: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
    /// Whether this server was stopped deliberately.
    ///
    /// A terminal is *meant* to outlive its server, so a test that panics
    /// halfway through would otherwise leave a shell — and, if it was running
    /// `vi`, a swap file — behind to confuse the next run. `Drop` kills what a
    /// finished test did not.
    stopped: bool,
}

impl Drop for Boot {
    fn drop(&mut self) {
        if !self.stopped {
            self.terminals.kill_all();
            self.cancel.cancel();
        }
    }
}

impl Boot {
    /// Start a server on `home`, owning `instance`.
    async fn start(home: &Path, instance: &str) -> Boot {
        let app = RemoteHandApp {
            home_override: Some(home.to_path_buf()),
            port_override: Some(0),
            instance_override: Some(instance.to_string()),
            // No login: these tests are not about authentication (`tests/auth.rs` is).
            auth_enabled_override: Some(false),
            ..Default::default()
        };
        let prepared = app.prepare().await.expect("prepare");
        let addr = prepared.addr;
        let cleanups = prepared.cleanups();
        let terminals = cleanups.terminals.clone();
        let cancel = prepared.cancel();
        let task = tokio::spawn(prepared.serve_task());
        Boot {
            addr,
            terminals,
            shims: shim_dir(home, instance),
            cleanups,
            cancel,
            task: Some(task),
            stopped: false,
        }
    }

    /// Stop the server the way `SIGTERM` does: cancel, then run the cleanups —
    /// which detach the terminals rather than killing them.
    ///
    /// Awaiting the serve task matters for more than tidiness: the instance lock
    /// lives inside it, so this is also what releases the instance for the next
    /// boot.
    async fn halt(mut self) {
        // Read before the stop, because stopping drains the registry.
        let sockets: Vec<std::path::PathBuf> = self
            .terminals
            .list()
            .iter()
            .map(|info| Sidecars::new(&self.shims, &info.id).socket())
            .collect();
        self.stop().await;
        // The link tasks return on the same cancellation as the serve task but
        // outside it, so they let their sockets go a moment after `stop` comes
        // back. That moment used to be a flat 50 ms, which is a guess about a
        // machine rather than a fact about the server — too long on an idle one
        // and, on a loaded one, quietly too short. What the successor needs on
        // the other side of it is a shim that answers when dialled, so wait for
        // that: as long as it takes, and loudly if it never comes.
        wait_for(
            || sockets.iter().all(|socket| shim_answers(socket)),
            "the shims to answer a fresh dial",
            20,
        )
        .await;
    }

    /// Stop the server *and* kill everything it started.
    async fn halt_and_kill(mut self) {
        self.terminals.kill_all();
        self.stop().await;
    }

    async fn stop(&mut self) {
        self.stopped = true;
        self.cancel.cancel();
        self.cleanups.run();
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
        }
    }

    async fn get_json(&self, path: &str) -> Value {
        let (status, body) = http_request(self.addr, "GET", path, None).await;
        assert_eq!(status, 200, "GET {path} -> {status}: {body}");
        serde_json::from_str(&body).expect("json")
    }

    async fn attach_ws(&self, id: &str) -> Socket {
        let url = format!("ws://{}/api/terminals/{id}/ws", self.addr);
        let (socket, _) = tokio_tungstenite::connect_async(&url).await.expect("websocket connect");
        Socket(socket)
    }
}

/// The instance directory a home directory's default instance uses.
fn shim_dir(home: &Path, instance: &str) -> std::path::PathBuf {
    dirs(home, instance).pty_shims()
}

fn dirs(home: &Path, instance: &str) -> InstanceDirs {
    InstanceDirs::new(&Paths::new(home.to_path_buf()).data_dir(), instance).expect("instance name")
}

/// A terminal running `script` under `/bin/sh`.
fn sh(script: &str) -> CreateSpec {
    CreateSpec::new("/bin/sh", vec!["-c".to_string(), script.to_string()])
}

/// Everything one attached client would see: the replay, then every later chunk.
///
/// This is the browser's stream — `terminals::routes` sends `replay` and then
/// each chunk verbatim as a text frame — without the websocket in the way.
struct Watcher(remote_hand_server::terminals::Attachment);

impl Watcher {
    fn attach(registry: &Registry, id: &str) -> Watcher {
        Watcher(registry.attach(id).expect("attach"))
    }

    fn replay(&self) -> &str {
        &self.0.replay
    }

    /// Read until every needle has been seen, returning the whole stream.
    // `result_large_err` fires on newer clippy: tungstenite's Error is large
    // on Linux, and a test helper unwrapping it is not worth restructuring.
    #[allow(clippy::result_large_err)]
    async fn read_until(&mut self, needles: &[&str], what: &str) -> String {
        let mut seen = self.0.replay.clone();
        while !needles.iter().all(|needle| seen.contains(needle)) {
            let next = tokio::time::timeout(Duration::from_secs(20), self.0.next_chunk())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for {what}; saw {seen:?}"));
            match next {
                Some(Chunk::Data(chunk)) => seen.push_str(&chunk),
                Some(Chunk::TooSlow) => panic!("the watcher was disconnected as too slow"),
                None => panic!("the terminal finished before {what}; saw {seen:?}"),
            }
        }
        seen
    }
}

/// A connected attach socket.
struct Socket(tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>);

impl Socket {
    async fn send(&mut self, frame: &str) {
        self.0.send(Message::Text(frame.into())).await.expect("send control frame");
    }

    // `result_large_err` fires on newer clippy: tungstenite's Error is large
    // on Linux, and a test helper unwrapping it is not worth restructuring.
    #[allow(clippy::result_large_err)]
    async fn read_until(&mut self, needles: &[&str], what: &str) -> String {
        let mut seen = String::new();
        while !needles.iter().all(|needle| seen.contains(needle)) {
            let message = tokio::time::timeout(Duration::from_secs(20), self.0.next())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for {what}; saw {seen:?}"))
                .unwrap_or_else(|| panic!("the socket closed before {what}; saw {seen:?}"))
                .expect("websocket error");
            if let Message::Text(text) = message {
                seen.push_str(&text);
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

/// Ask a shim who it is minding, without disturbing anyone else's link.
///
/// `u64::MAX` as the cursor is the "peek" idiom: the ring clamps it to the head,
/// so this connection is told the shell's pid and handed no output at all. It is
/// also told, quite correctly, that a cursor that far out cannot be true; the
/// gap frame that says so is dropped with the link a line later.
async fn peek_shell_pid(shims: &Path, id: &str) -> u32 {
    let link = remote_hand_server::pty_shim::client::connect(&Sidecars::new(shims, id).socket(), u64::MAX)
        .await
        .expect("connect to the shim");
    link.shell_pid
}

fn alive(pid: u32) -> bool {
    remote_hand_server::procs::alive(pid)
}

/// Whether a shim is ready for whoever dials it next: it accepts a connection,
/// or it is not there at all — a shell that exited takes its socket with it, and
/// a socket nobody is waiting on is nothing to wait for.
///
/// A blocking connect, because a unix socket has no network behind it: it either
/// finds a listener in the kernel or it does not, and it does so before the next
/// line of this function runs.
fn shim_answers(socket: &Path) -> bool {
    !socket.exists() || std::os::unix::net::UnixStream::connect(socket).is_ok()
}

/// Poll `condition` until it holds, or fail after `seconds`.
async fn wait_for(mut condition: impl FnMut() -> bool, what: &str, seconds: u64) {
    for _ in 0..(seconds * 50) {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

// --------------------------------------------------------- adoption, in process

/// The headline: a terminal, and the shell inside it, come back after the server
/// that started them has gone away and a new one has taken its place.
#[tokio::test]
async fn a_terminal_is_adopted_across_a_restart_with_the_same_shell() {
    let home = tempfile::tempdir().expect("tempdir");
    let shims = shim_dir(home.path(), DEFAULT_INSTANCE);

    let first = Boot::start(home.path(), DEFAULT_INSTANCE).await;
    let info = first
        .terminals
        .create_terminal_with(sh(
            "echo before-the-restart; while :; do read line || sleep 1; echo \"echoed:$line\"; done",
        ))
        .await
        .expect("create terminal");
    let mut watcher = Watcher::attach(&first.terminals, &info.id);
    watcher.read_until(&["before-the-restart"], "the first line of output").await;
    let shell_pid = peek_shell_pid(&shims, &info.id).await;
    assert!(alive(shell_pid));

    first.halt().await;
    // The whole point: the server is gone and the shell is not.
    assert!(alive(shell_pid), "the shell must outlive the server that started it");

    let second = Boot::start(home.path(), DEFAULT_INSTANCE).await;

    // It is in the list, with everything the first server recorded about it.
    let listed = second.get_json("/api/terminals").await;
    let terminals = listed["terminals"].as_array().expect("array");
    assert_eq!(terminals.len(), 1, "the adopted terminal is listed: {listed}");
    assert_eq!(terminals[0]["id"], info.id.as_str());
    assert_eq!(terminals[0]["title"], info.title.as_str(), "the title survives");
    assert_eq!(terminals[0]["createdAt"], info.created_at.as_str(), "and so does its age");

    // And it is the same shell, not a new one wearing its name.
    assert_eq!(peek_shell_pid(&shims, &info.id).await, shell_pid, "adoption must not restart the shell");

    // A browser attaching gets a repaint of the screen as it stands, and then
    // live output from the shell that was already running.
    let mut socket = second.attach_ws(&info.id).await;
    let repaint = socket.read_until(&["before-the-restart"], "the repaint").await;
    assert!(repaint.contains("\x1b[H\x1b[J"), "the repaint clears before it paints: {repaint:?}");
    assert!(!repaint.contains("output gap"), "nothing was lost: {repaint:?}");

    socket.send(r#"{"type":"input","data":"still-here\r"}"#).await;
    socket.read_until(&["echoed:still-here"], "the adopted shell to answer").await;

    second.halt_and_kill().await;
    wait_for(|| !alive(shell_pid), "the shell to be killed with the test", 10).await;
}

/// The golden test: a terminal that is producing output the whole time a
/// server is replaced must not lose any of it.
#[tokio::test]
async fn every_line_of_a_running_loop_survives_a_restart_in_the_middle_of_it() {
    const TICKS: usize = 25;
    let home = tempfile::tempdir().expect("tempdir");

    let first = Boot::start(home.path(), DEFAULT_INSTANCE).await;
    // The cadence is 0.2 s rather than 1 s
    // so the suite does not spend half a minute here; what is being tested is
    // that the restart lands *between* ticks, which it does either way.
    let info = first
        .terminals
        .create_terminal_with(sh(&format!(
            "i=1; while [ $i -le {TICKS} ]; do echo tick-$i; i=$((i+1)); sleep 0.2; done; echo all-done"
        )))
        .await
        .expect("create terminal");

    let mut before = Watcher::attach(&first.terminals, &info.id);
    let seen_before = before.read_until(&["tick-3"], "the loop to get going").await;
    drop(before);
    first.halt().await;

    let second = Boot::start(home.path(), DEFAULT_INSTANCE).await;
    let mut after = Watcher::attach(&second.terminals, &info.id);
    let seen_after = after.read_until(&["all-done"], "the loop to finish").await;

    // The browser's stream is what it saw before the restart plus what it saw
    // after; every tick has to be in one of them.
    let stream = format!("{seen_before}{seen_after}");
    let missing: Vec<String> = (1..=TICKS).map(|i| format!("tick-{i}")).filter(|tick| !stream.contains(tick)).collect();
    assert!(missing.is_empty(), "the restart lost {missing:?}");

    // An 8 MiB ring against a few hundred bytes of ticks: a gap here would mean
    // the cursor, not the ring, lost them.
    assert!(!stream.contains("output gap"), "nothing should have fallen out of the ring: {stream:?}");

    second.halt_and_kill().await;
}

/// The cursor is a number in a file, and the thing that kills a server can
/// truncate or garble the file on its way past. A cursor beyond anything the
/// shell ever wrote is one of those, and believing it means silently skipping
/// every byte between the shim's head and whatever the file said.
#[tokio::test]
async fn a_cursor_from_beyond_the_end_of_the_output_is_clamped_and_said_out_loud() {
    let home = tempfile::tempdir().expect("tempdir");
    let shims = shim_dir(home.path(), DEFAULT_INSTANCE);

    let first = Boot::start(home.path(), DEFAULT_INSTANCE).await;
    let info = first
        .terminals
        .create_terminal_with(sh(
            "echo before-the-restart; while :; do read line || sleep 1; echo \"echoed:$line\"; done",
        ))
        .await
        .expect("create terminal");
    let mut watcher = Watcher::attach(&first.terminals, &info.id);
    watcher.read_until(&["before-the-restart"], "the first line of output").await;
    drop(watcher);
    first.halt().await;

    // A few dozen bytes of output, and a cursor claiming a terabyte of it.
    let sidecars = Sidecars::new(&shims, &info.id);
    std::fs::write(sidecars.cursor(), b"999999999999\n").expect("corrupt the cursor");

    let second = Boot::start(home.path(), DEFAULT_INSTANCE).await;
    let mut watcher = Watcher::attach(&second.terminals, &info.id);
    watcher
        .read_until(&["output gap"], "the note that says the stream is not continuous")
        .await;

    // Clamped, not merely reported: the shell's next output arrives instead of
    // being swallowed until it catches up with a number out of a broken file.
    watcher.0.write("still-here\r");
    watcher
        .read_until(&["echoed:still-here"], "the shell to answer past the bad cursor")
        .await;
    drop(watcher);

    second.halt_and_kill().await;
}

/// A full-screen program cannot be restored by replaying its output — it has to
/// be restored from the screen that output produced.
#[tokio::test]
async fn a_full_screen_program_is_repainted_from_the_shims_screen_model() {
    let home = tempfile::tempdir().expect("tempdir");
    let first = Boot::start(home.path(), DEFAULT_INSTANCE).await;

    // `vi` when the machine has one, and the escape sequences `vi` would emit
    // when it does not, so the assertion is about the shim either way.
    let vi = ["/usr/bin/vi", "/bin/vi", "/usr/bin/vim"]
        .into_iter()
        .find(|path| Path::new(path).exists());
    // The file it edits lives in this test's own home directory, so the swap
    // file `vi` writes beside it goes with the tempdir instead of greeting the
    // next run with a recovery prompt where the empty buffer should be.
    let scratch = home.path().join("golden.txt");
    let spec = match vi {
        Some(vi) => sh(&format!("{vi} {}", scratch.display())),
        None => sh("printf '\\033[?1049h\\033[?1h\\033=\\033[H~ SYNTHETIC-VI\\r\\n'; sleep 30"),
    };
    let info = first.terminals.create_terminal_with(spec).await.expect("create terminal");

    let mut watcher = Watcher::attach(&first.terminals, &info.id);
    // Both `vi` and the stand-in draw `~` down the left margin of an alternate
    // screen, which is the one thing they are guaranteed to have in common.
    watcher.read_until(&["\x1b[?1049h", "~"], "the alternate screen to be drawn").await;
    drop(watcher);

    first.halt().await;
    let second = Boot::start(home.path(), DEFAULT_INSTANCE).await;

    let watcher = Watcher::attach(&second.terminals, &info.id);
    let repaint = watcher.replay().to_string();
    assert!(
        repaint.contains("\x1b[?1049h"),
        "the repaint has to put the client back on the alternate screen: {repaint:?}"
    );
    assert!(repaint.contains('~'), "and repaint what was on it: {repaint:?}");
    assert!(
        repaint.contains("\x1b[H\x1b[J"),
        "clearing first is what makes a repaint idempotent: {repaint:?}"
    );
    drop(watcher);

    second.halt_and_kill().await;
}

// ------------------------------------------------------------- corpse guards

/// A shim minding a shell that has already exited is not a terminal, and must
/// not be listed as one.
#[tokio::test]
async fn a_tombstoned_terminal_is_reported_and_swept_rather_than_adopted() {
    let home = tempfile::tempdir().expect("tempdir");
    let shims = shim_dir(home.path(), DEFAULT_INSTANCE);

    let first = Boot::start(home.path(), DEFAULT_INSTANCE).await;
    let info = first.terminals.create_terminal_with(sh("sleep 30")).await.expect("create terminal");
    let shell_pid = peek_shell_pid(&shims, &info.id).await;
    first.halt().await;

    // The shell dies while nobody is watching, which is exactly the case the
    // tombstone exists for.
    remote_hand_server::pty_shim::child::kill_shell(shell_pid, libc::SIGKILL);
    let sidecars = Sidecars::new(&shims, &info.id);
    wait_for(|| sidecars.tombstoned(), "the shim to record the exit", 10).await;

    let second = Boot::start(home.path(), DEFAULT_INSTANCE).await;
    assert!(
        second.get_json("/api/terminals").await["terminals"].as_array().expect("array").is_empty(),
        "a terminal whose shell exited must not come back"
    );
    assert!(!sidecars.meta().exists(), "and its sidecars go with it");
    assert!(!sidecars.exit().exists());
    second.halt_and_kill().await;
}

/// A socket with nothing behind it is not a terminal either.
#[tokio::test]
async fn a_socket_whose_shim_was_killed_is_swept_rather_than_adopted() {
    let home = tempfile::tempdir().expect("tempdir");
    let shims = shim_dir(home.path(), DEFAULT_INSTANCE);

    let first = Boot::start(home.path(), DEFAULT_INSTANCE).await;
    let info = first.terminals.create_terminal_with(sh("sleep 30")).await.expect("create terminal");
    let shell_pid = peek_shell_pid(&shims, &info.id).await;
    first.halt().await;

    // `SIGKILL` gives the shim no chance to write a tombstone or unlink its
    // socket: the socket file is left behind with nothing listening on it.
    let shim_pid = shim_pid_for(&shims, &info.id);
    unsafe { libc::kill(shim_pid as libc::pid_t, libc::SIGKILL) };
    remote_hand_server::pty_shim::child::kill_shell(shell_pid, libc::SIGKILL);
    let sidecars = Sidecars::new(&shims, &info.id);
    wait_for(|| !alive(shim_pid), "the shim to die", 10).await;
    assert!(sidecars.socket().exists(), "the point of this test is the socket that was left behind");
    assert!(!sidecars.tombstoned(), "and that there is no tombstone to explain it");

    let second = Boot::start(home.path(), DEFAULT_INSTANCE).await;
    assert!(
        second.get_json("/api/terminals").await["terminals"].as_array().expect("array").is_empty(),
        "a shim that died without saying so must not be adopted"
    );
    assert!(!sidecars.socket().exists(), "the stale socket is cleaned up");
    assert!(!sidecars.meta().exists());
    second.halt_and_kill().await;
}

/// The guard that needs the handshake: a shim that answers, and names a shell
/// that is not there.
///
/// Built by hand rather than found in the wild, because the window it covers —
/// a shim still serving after its shell was reaped — is milliseconds wide in a
/// real one. A stub that speaks the protocol reproduces it exactly.
#[tokio::test]
async fn a_shim_that_names_a_dead_shell_is_refused_at_the_handshake() {
    let home = tempfile::tempdir().expect("tempdir");
    let shims = shim_dir(home.path(), DEFAULT_INSTANCE);
    dirs(home.path(), DEFAULT_INSTANCE).create().expect("create instance dirs");

    let id = "3f2504e0-4f89-11d3-9a0c-0305e82c3301";
    let sidecars = Sidecars::new(&shims, id);
    sidecars.ensure_socket_dir().expect("socket dir");
    sidecars
        .write_meta(&Meta {
            id: id.to_string(),
            title: "terminal 1".into(),
            cwd: home.path().to_string_lossy().into_owned(),
            created_seq: 0,
            shell: "/bin/sh".into(),
            created_at: "2026-01-01T00:00:00.000Z".into(),
        })
        .await
        .expect("write meta");

    // A stub shim: it completes the handshake, and the pid it names is one that
    // cannot exist.
    let listener = tokio::net::UnixListener::bind(sidecars.socket()).expect("bind");
    let stub = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut rd, mut wr) = stream.into_split();
                while let Ok(Some(frame)) = proto::read_frame(&mut rd).await {
                    if frame.kind == proto::HELLO {
                        let hello = Hello2 {
                            shell_pid: u32::MAX - 1,
                            total: 0,
                        };
                        let _ = proto::write_frame(&mut wr, &Frame::json(proto::HELLO, &hello)).await;
                    }
                }
            });
        }
    });

    let server = Boot::start(home.path(), DEFAULT_INSTANCE).await;
    assert!(
        server.get_json("/api/terminals").await["terminals"].as_array().expect("array").is_empty(),
        "a shim minding a pid that is not there must not be adopted"
    );
    assert!(!sidecars.meta().exists(), "and it is swept, not left to be retried forever");

    stub.abort();
    server.halt_and_kill().await;
}

/// Debris from a crash between spawning a shim and recording what it was.
#[tokio::test]
async fn a_socket_with_no_identity_is_swept_at_boot() {
    let home = tempfile::tempdir().expect("tempdir");
    let shims = shim_dir(home.path(), DEFAULT_INSTANCE);
    dirs(home.path(), DEFAULT_INSTANCE).create().expect("create instance dirs");
    let orphan = shims.join("deadbeefdeadbeef.sock");
    std::fs::write(&orphan, b"").expect("write");

    let server = Boot::start(home.path(), DEFAULT_INSTANCE).await;
    assert!(!orphan.exists(), "a socket nothing claims is debris");
    server.halt_and_kill().await;
}

// ---------------------------------------------------------------- ownership

/// Two instances on one machine are two worlds. Neither can see, adopt or kill
/// the other's terminals.
#[tokio::test]
async fn two_instances_do_not_see_each_others_terminals() {
    let home = tempfile::tempdir().expect("tempdir");

    let alpha = Boot::start(home.path(), "alpha").await;
    let beta = Boot::start(home.path(), "beta").await;

    let mine = alpha.terminals.create_terminal_with(sh("sleep 30")).await.expect("create terminal");
    assert_eq!(alpha.get_json("/api/terminals").await["terminals"].as_array().expect("array").len(), 1);
    assert!(
        beta.get_json("/api/terminals").await["terminals"].as_array().expect("array").is_empty(),
        "beta must not see alpha's terminal"
    );

    // And a restart of beta neither adopts alpha's shim nor sweeps its sidecars.
    beta.halt().await;
    let beta = Boot::start(home.path(), "beta").await;
    assert!(beta.get_json("/api/terminals").await["terminals"].as_array().expect("array").is_empty());
    assert!(
        Sidecars::new(shim_dir(home.path(), "alpha"), &mine.id).meta().exists(),
        "alpha's identity file is none of beta's business"
    );
    assert_eq!(alpha.get_json("/api/terminals").await["terminals"].as_array().expect("array").len(), 1);

    beta.halt_and_kill().await;
    alpha.halt_and_kill().await;
}

/// Two servers on *one* instance is the case the lock exists to refuse.
#[tokio::test]
async fn a_second_server_on_one_instance_is_refused_and_told_the_pid() {
    let home = tempfile::tempdir().expect("tempdir");
    let running = Boot::start(home.path(), DEFAULT_INSTANCE).await;

    let second = RemoteHandApp {
        home_override: Some(home.path().to_path_buf()),
        port_override: Some(0),
        instance_override: Some(DEFAULT_INSTANCE.to_string()),
        // No login: these tests are not about authentication (`tests/auth.rs` is).
        auth_enabled_override: Some(false),
        ..Default::default()
    }
    .prepare()
    .await;
    let error = second.err().expect("a second server on one instance must be refused").to_string();
    assert!(error.contains(&format!("pid {}", std::process::id())), "the holder is named: {error}");
    assert!(error.contains("APP_INSTANCE"), "and so is the way out: {error}");

    // A different name is free, which is what the message is pointing at.
    let elsewhere = Boot::start(home.path(), "staging").await;
    elsewhere.halt_and_kill().await;

    running.halt_and_kill().await;
    // Released with the server that held it: the serve task holds the only
    // strong reference to the lock, so it is free once that task has ended,
    // however long the server's other tasks take to wind down.
    let _lock = InstanceLock::acquire(&dirs(home.path(), DEFAULT_INSTANCE)).expect("the lock is free again");
}

/// A server that skipped the lock has no business in the instance directory.
///
/// `export-openapi` is that server: it stands a throwaway router up in-process
/// to read one document back out of it, and it is routinely run on a machine
/// where the real server is up. Without the gate it would attach to that
/// server's live shims, advance their cursors and sweep the sidecars of any it
/// could not reach — and then exit.
#[tokio::test]
async fn a_server_that_skips_the_lock_neither_adopts_nor_claims_the_port() {
    let home = tempfile::tempdir().expect("tempdir");
    let owner = Boot::start(home.path(), DEFAULT_INSTANCE).await;
    let info = owner.terminals.create_terminal_with(sh("sleep 30")).await.expect("create terminal");

    let port_file = dirs(home.path(), DEFAULT_INSTANCE).port_file();
    let owners_port = std::fs::read_to_string(&port_file).expect("the owner recorded its port");
    assert_eq!(owners_port.trim().parse::<u16>().expect("a port"), owner.addr.port());

    let unowned = RemoteHandApp {
        home_override: Some(home.path().to_path_buf()),
        port_override: Some(0),
        skip_lock: true,
        // No login: these tests are not about authentication (`tests/auth.rs` is).
        auth_enabled_override: Some(false),
        ..Default::default()
    }
    .prepare()
    .await
    .expect("a lock-skipping server starts even though the instance is taken");

    assert!(
        unowned.cleanups().terminals.is_empty(),
        "a server that does not own the instance must adopt nothing from it"
    );
    assert_eq!(
        std::fs::read_to_string(&port_file).expect("port file"),
        owners_port,
        "and it must not redirect readers to a port it is about to close"
    );

    // The owner's terminal is untouched: still listed, still on disk.
    assert_eq!(owner.get_json("/api/terminals").await["terminals"].as_array().expect("array").len(), 1);
    assert!(Sidecars::new(shim_dir(home.path(), DEFAULT_INSTANCE), &info.id).meta().exists());

    drop(unowned);
    owner.halt_and_kill().await;
}

/// An instance name that is not `[a-z0-9-]{1,32}` never becomes a directory.
#[tokio::test]
async fn an_illegal_instance_name_refuses_to_start() {
    let home = tempfile::tempdir().expect("tempdir");
    for bad in ["../escape", "Upper", "with space", ""] {
        let error = RemoteHandApp {
            home_override: Some(home.path().to_path_buf()),
            port_override: Some(0),
            instance_override: Some(bad.to_string()),
            // No login: these tests are not about authentication (`tests/auth.rs` is).
            auth_enabled_override: Some(false),
            ..Default::default()
        }
        .prepare()
        .await
        .err();
        // An empty name means "unset", and is the default rather than a refusal.
        if bad.is_empty() {
            let started = error.is_none();
            assert!(started, "an empty instance name falls back to the default");
            continue;
        }
        let message = error.expect("an illegal name must be refused").to_string();
        assert!(message.contains("a-z"), "the refusal says what a name may be: {message}");
    }
}

// -------------------------------------------------------- across two processes

/// The real thing: the actual binary, killed through its whole process group
/// with `SIGKILL`, and a second invocation that picks up where it left off.
///
/// This is the test that says the mechanism works rather than that the code
/// agrees with itself. `process_group(0)` on the server means `kill(-pid, …)`
/// reaches the server and everything it did *not* detach — which is precisely
/// the `sh -c 'server & kill -9 $!'` shape, and precisely what a shim spawned
/// into a process group of its own has to survive.
#[tokio::test]
async fn a_shim_survives_its_server_being_sigkilled_and_is_adopted_by_the_next_one() {
    let home = tempfile::tempdir().expect("tempdir");
    let shims = shim_dir(home.path(), DEFAULT_INSTANCE);

    let first = spawn_server(home.path()).await;
    let created = http_request(first.addr, "POST", "/api/terminals", Some(r#"{"title":"across processes"}"#)).await;
    assert_eq!(created.0, 200, "create -> {}: {}", created.0, created.1);
    let id = serde_json::from_str::<Value>(&created.1).expect("json")["id"]
        .as_str()
        .expect("id")
        .to_string();
    let shell_pid = peek_shell_pid(&shims, &id).await;

    // The whole process group, so nothing survives by being merely a child.
    unsafe { libc::kill(-(first.pid as libc::pid_t), libc::SIGKILL) };
    wait_for(|| !alive(first.pid), "the server to die", 10).await;
    assert!(
        alive(shell_pid),
        "the shell must survive a SIGKILL aimed at the server's whole process group"
    );

    let second = spawn_server(home.path()).await;
    let listed: Value = serde_json::from_str(&http_request(second.addr, "GET", "/api/terminals", None).await.1).expect("json");
    let terminals = listed["terminals"].as_array().expect("array");
    assert_eq!(terminals.len(), 1, "the second process adopts what the first left: {listed}");
    assert_eq!(terminals[0]["id"], id.as_str());
    assert_eq!(terminals[0]["title"], "across processes");
    assert_eq!(peek_shell_pid(&shims, &id).await, shell_pid, "and it is the same shell");

    // Still a working terminal, not just a row in a list.
    let url = format!("ws://{}/api/terminals/{id}/ws", second.addr);
    let (socket, _) = tokio_tungstenite::connect_async(&url).await.expect("websocket connect");
    let mut socket = Socket(socket);
    socket.send(r#"{"type":"input","data":"echo across-$((6*7))\r"}"#).await;
    socket.read_until(&["across-42"], "the adopted shell to answer").await;

    // Found before anything is killed: a shim whose shell is gone announces the
    // exit and leaves on its own, so a lookup after the shell's death can miss it.
    let shim_pid = shim_pid_for(&shims, &id);
    unsafe { libc::kill(-(second.pid as libc::pid_t), libc::SIGKILL) };
    remote_hand_server::pty_shim::child::kill_shell(shell_pid, libc::SIGKILL);
    unsafe { libc::kill(shim_pid as libc::pid_t, libc::SIGKILL) };
    wait_for(|| !alive(shell_pid), "the shell to be cleaned up", 10).await;
    // Nothing here was given the chance to tidy up after itself, which is the
    // point; the socket is outside the tempdir, so the test removes it.
    let socket = Sidecars::new(&shims, &id).socket();
    let _ = std::fs::remove_file(&socket);
    if let Some(parent) = socket.parent().filter(|parent| parent.starts_with("/tmp/remote-hand-")) {
        let _ = std::fs::remove_dir(parent);
    }
}

/// A server started from the real binary, on a home directory the test owns.
struct Spawned {
    pid: u32,
    addr: SocketAddr,
}

async fn spawn_server(home: &Path) -> Spawned {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-hand-server"));
    command
        .arg("serve")
        .env("APP_HOME_DIR", home)
        // No login: not a test of authentication (`tests/auth.rs` is).
        .env("APP_AUTH__ENABLED", "false")
        .env("APP_SERVER__PORT", "0")
        .env("APP_SERVER__HOST", "127.0.0.1")
        // A group of its own, so the test can kill the group without killing
        // itself — and so a shim, which takes a group of its own again, is
        // demonstrably outside it.
        .process_group(0)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(false);
    let mut child = command.spawn().expect("spawn the server binary");
    let pid = child.id().expect("the server has a pid");
    tokio::spawn(async move {
        let _ = child.wait().await;
    });

    // The bound port is written into the instance directory precisely so
    // something other than the server can find it.
    let port_file = dirs(home, DEFAULT_INSTANCE).port_file();
    let mut addr = None;
    for _ in 0..500 {
        if let Some(port) = std::fs::read_to_string(&port_file).ok().and_then(|text| text.trim().parse::<u16>().ok()) {
            let candidate: SocketAddr = ([127, 0, 0, 1], port).into();
            if tokio::net::TcpStream::connect(candidate).await.is_ok() {
                addr = Some(candidate);
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Spawned {
        pid,
        addr: addr.expect("the server never recorded a port it was listening on"),
    }
}

/// The pid of the shim minding `id`, read from the process table.
///
/// The server does not publish it — nothing needs it in production, where a shim
/// is reached through its socket — but a test that has to prove a process died
/// needs something to point at.
fn shim_pid_for(shims: &Path, id: &str) -> u32 {
    let socket = Sidecars::new(shims, id).socket();
    let needle = socket.to_string_lossy().into_owned();
    let output = std::process::Command::new("/bin/ps").args(["-Ao", "pid=,command="]).output().expect("ps");
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        if line.contains(&needle) && line.contains("pty-shim") {
            if let Some(pid) = line.split_whitespace().next().and_then(|pid| pid.parse().ok()) {
                return pid;
            }
        }
    }
    panic!("no pty-shim process is serving {needle}");
}
