//! The dogfood test: remote-hand upgrades remote-hand without dropping anything.
//!
//! Everything else in the restart-survival phases proves one subsystem survives
//! one restart. This proves the thing they were all for. A real
//! `remote-hand-server up` supervises a real server; a web terminal is running a
//! loop that prints a numbered line every fifth of a second; an injected CLI run
//! is in flight. The server is then handed a copy of its own binary over
//! `POST /api/server/self-upgrade`, and the assertions are the promises the
//! feature makes:
//!
//! * the **server** process is a different one afterwards — the swap really
//!   restarted something;
//! * the **shim** and the **shell** are the same processes they were — the
//!   restart went underneath them, not through them;
//! * the tick sequence the shell produced has **no gap** across the swap, and
//!   the numbering does not restart — the shell never stopped and was never
//!   re-run;
//! * the new server **adopts** the run that was in flight, so the composer is
//!   still blocked on it;
//! * a fresh websocket against the new server delivers **live** output, so the
//!   terminal is not merely alive but usable.
//!
//! Nothing here is in-process. The server is the built binary, started the way a
//! container starts it, and the upgrade is an HTTP request with a multipart body
//! written by hand — because the point of this file is that the mechanism works,
//! not that the code agrees with itself.

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use support::{plan_held_cli, release_turn, Probe};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

const SESSION: &str = "eeeeeeee-5555-4555-8555-eeeeeeeeeeee";

/// The loop the terminal runs. Every fifth of a second, a numbered line.
///
/// `sh` rather than the user's shell, and `printf` rather than `echo`, so the
/// bytes on the pty are exactly these and nothing platform-dependent.
const TICKER: &str = "i=0; while :; do i=$((i+1)); printf 'tick %d\\n' \"$i\"; sleep 0.2; done";

/// How long anything in this test may take before it is a failure.
///
/// Generous: the upgrade copies and health-checks a sixty-megabyte debug binary,
/// and this test runs alongside every other one in the suite.
const BUDGET: Duration = Duration::from_secs(90);

// ---------------------------------------------------------------- the test

#[tokio::test]
async fn the_server_replaces_its_own_binary_without_dropping_a_terminal_or_a_run() {
    // The probe only lays the home directory out the way the real binary expects
    // — fake CLIs on `PATH`, a seeded session — and then gets out of the way.
    let home = tempfile::tempdir().expect("tempdir");
    let probe = Probe::start_at(home.path()).await;
    let workspace = probe.workspace();
    probe.seed_session(&probe.managed_root(), SESSION, &workspace);
    let journal = probe.instance_db();
    probe.stop().await;

    // Held, not slow: the turn must still be running on the far side of the
    // upgrade, and a timed sleep loses that race whenever copying and
    // health-checking a sixty-megabyte debug binary takes longer than it — at
    // which point the turn ends *legitimately*, the adopted run is withdrawn
    // exactly as designed, and the assertion below reads an absence as a bug.
    plan_held_cli(&workspace);

    let mut up = Supervisor::start(home.path()).await;
    let first_server = up.server_pid();

    // ---- a terminal with something running in it -------------------------

    let (status, created) = up.json("POST", "/api/terminals", Some("{}")).await;
    assert_eq!(status, 200, "{created}");
    let terminal = created["id"].as_str().expect("terminal id").to_string();

    let mut socket = up.attach(&terminal).await;
    socket.send_input(&format!("{TICKER}\n")).await;
    // Wait for the loop to be demonstrably running before anything is upgraded,
    // or the test could prove nothing more than that a swap is fast.
    let before = socket.read_ticks_until(3).await;
    assert!(before >= 3, "the ticker never started; saw up to {before}");

    let shims = home.path().join(".local/share/remote-hand/instances/default/pty-shims");
    let first_shim = one_shim_pid(home.path());
    let first_shell = shell_pid(&shims, &terminal).await;

    // ---- a CLI run in flight ---------------------------------------------

    let (status, body) = up
        .json(
            "POST",
            &format!("/api/sessions/claude/{SESSION}/message"),
            Some(r#"{"text":"keep going"}"#),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let record = wait_for_record(&journal).await;
    let cli_pid = record["cli_pid"].as_u64().expect("cli_pid") as u32;
    let _cli_guard = KillGroupOnDrop(cli_pid);

    // ---- the upgrade -------------------------------------------------------

    let binary = PathBuf::from(env!("CARGO_BIN_EXE_remote-hand-server"));
    let bytes = std::fs::read(&binary).expect("read this test's own server binary");
    let digest = sha256_hex(&bytes);
    let (status, answer) = up.upload_binary(&bytes, Some(&digest)).await;
    assert_eq!(status, 200, "the upgrade was refused: {answer}");
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["restart"], "restarting", "a server under `up` restarts itself: {answer}");
    assert!(
        answer["version"].as_str().is_some_and(|v| !v.is_empty()),
        "the health check reports the version it read: {answer}"
    );

    // The uploaded binary is installed, and the one it replaced is one step back.
    let bin = home.path().join(".local/share/remote-hand/bin");
    assert!(bin.join("remote-hand-server").is_file());
    assert!(bin.join("remote-hand-server.prev").is_file(), "the incumbent is kept");
    assert!(
        std::fs::read_dir(&bin)
            .expect("bin dir")
            .filter_map(|e| e.ok())
            .all(|e| !e.file_name().to_string_lossy().starts_with(".staging-")),
        "no staging file is left behind"
    );

    // ---- what survived -----------------------------------------------------

    let second_server = up.wait_for_new_server(first_server).await;
    assert_ne!(second_server, first_server, "the server process really was replaced");

    assert_eq!(
        one_shim_pid(home.path()),
        first_shim,
        "the pty shim is the same process: the restart went underneath it"
    );
    assert_eq!(shell_pid(&shims, &terminal).await, first_shell, "and so is the shell it is minding");

    // The old socket died with the old server. A reattach is what the web does,
    // and it is what proves the new server can serve this terminal.
    let mut socket = up.attach(&terminal).await;
    let after = socket.read_ticks_until(before + 5).await;
    assert!(
        after >= before + 5,
        "the new server delivers live output: saw up to {after}, wanted {}",
        before + 5
    );

    // The strongest statement available about continuity, and it comes from the
    // shim's own ring rather than from either server: every tick the shell ever
    // printed, in one contiguous run. A shell that had been restarted would
    // start again at 1; one that had been paused would leave a hole.
    let ticks = ticks_in_ring(&shims, &terminal, after).await;
    assert_contiguous(&ticks);
    let highest = *ticks.last().expect("at least one tick");
    assert!(highest >= after, "the ring holds everything the socket saw: {highest} < {after}");
    assert!(
        ticks.first().copied().expect("a first tick") <= before,
        "the run spans the upgrade: it starts at {:?}, and {before} was reached before it",
        ticks.first()
    );

    // The turn that was in flight is the new server's now. Not a new run against
    // the same session — the same `claude`, reached back through the shim socket
    // it was left holding, still inside the turn nobody interrupted.
    let detail = up.get_json(&format!("/api/sessions/claude/{SESSION}")).await;
    assert_eq!(detail["injection"]["state"], "running", "the run was adopted: {detail}");
    assert_eq!(
        detail["canSend"], true,
        "a resident takes the next message while it works; it queues rather than being refused: {detail}"
    );
    assert!(support::call_count(&workspace) <= 1, "no second CLI was ever spawned");
    // And the prompt that started it was read once, by that one process: an
    // upgrade that had lost the turn and re-sent it would show two.
    assert_eq!(
        support::prompts(&workspace),
        vec!["keep going".to_string()],
        "the turn spans the upgrade rather than being started again on the far side"
    );

    // Released at last, the held turn ends normally — and it is the *new*
    // server that hears the `result` and closes the run, which is the whole
    // continuity claim in one line.
    release_turn(&workspace);
    let deadline = tokio::time::Instant::now() + BUDGET;
    loop {
        let detail = up.get_json(&format!("/api/sessions/claude/{SESSION}")).await;
        if detail["injection"]["state"] == "ok" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the released turn never finished under the new server: {detail}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    up.stop().await;
}

// ------------------------------------------------------------------ harness

/// A real `remote-hand-server up`, and the server it is supervising.
struct Supervisor {
    /// The supervisor's own pid; also its process group, so a stray test can be
    /// cleaned up in one signal.
    pid: u32,
    addr: std::net::SocketAddr,
    home: PathBuf,
    stopped: bool,
}

impl Drop for Supervisor {
    /// A supervisor that outlived its test would keep restarting a server, on
    /// the developer's machine, forever. That is a much worse way to learn an
    /// assertion moved than a red test.
    fn drop(&mut self) {
        if !self.stopped {
            // SAFETY: a negative pid names a process group; signalling one that
            // has already gone is an ignored `ESRCH`.
            unsafe { libc::kill(-(self.pid as libc::pid_t), libc::SIGKILL) };
        }
    }
}

/// Forget the port an earlier server on `home` recorded.
///
/// [`wait_for_listener`] trusts whatever port the file names and answers, so
/// a file left by a server that has since stopped (the in-process probe
/// several tests start with) could send it to that port while the new server
/// was still on its way up. Once the port is reused by another test's server,
/// the requests that follow reach a server that is not this test's.
fn forget_port(home: &Path) {
    let _ = std::fs::remove_file(home.join(".local/share/remote-hand/instances/default/server.port"));
}

impl Supervisor {
    async fn start(home: &Path) -> Supervisor {
        forget_port(home);
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-hand-server"));
        command
            .args(["up", "--", "serve"])
            .env("APP_HOME_DIR", home)
            // No login: not a test of authentication (`tests/auth.rs` is).
            .env("APP_AUTH__ENABLED", "false")
            .env("APP_SERVER__PORT", "0")
            .env("APP_SERVER__HOST", "127.0.0.1")
            .env("RUST_LOG", "remote_hand_server=debug")
            // The fake `claude` lives on this process's `PATH`; the supervisor
            // hands it down to the server, which hands it down to the CLI.
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .process_group(0)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(
                std::fs::File::create(home.join("serve.log")).expect("serve.log"),
            ))
            // Kept rather than discarded: when this test fails it is almost
            // always the supervisor that knows why, and "no new server ever
            // appeared" is not a diagnosis.
            .stderr(std::process::Stdio::from(std::fs::File::create(home.join("up.log")).expect("up.log")))
            .kill_on_drop(false);
        let mut child = command.spawn().expect("spawn the supervisor");
        let pid = child.id().expect("the supervisor has a pid");
        tokio::spawn(async move {
            let _ = child.wait().await;
        });

        // Armed before the wait below, so a server that never comes up still
        // takes its supervisor down with the test rather than leaving one
        // running.
        let mut supervisor = Supervisor {
            pid,
            addr: ([127, 0, 0, 1], 0).into(),
            home: home.to_path_buf(),
            stopped: false,
        };
        supervisor.addr = wait_for_listener(home).await;
        supervisor
    }
    /// The pid of the server currently serving.
    ///
    /// Read out of the instance lock file rather than by walking the process
    /// tree: the holder writes its own pid there, so this is the server's own
    /// statement about who it is, and it changes exactly when the process does.
    fn server_pid(&self) -> u32 {
        let lock = self.home.join(".local/share/remote-hand/instances/default/lock");
        std::fs::read_to_string(&lock)
            .ok()
            .and_then(|raw| raw.trim().parse().ok())
            .unwrap_or_else(|| panic!("no server pid in {}", lock.display()))
    }

    /// Wait until a *different* server is serving, and answering.
    async fn wait_for_new_server(&mut self, previous: u32) -> u32 {
        let deadline = std::time::Instant::now() + BUDGET;
        loop {
            let current = std::fs::read_to_string(self.home.join(".local/share/remote-hand/instances/default/lock"))
                .ok()
                .and_then(|raw| raw.trim().parse::<u32>().ok());
            if let Some(pid) = current.filter(|pid| *pid != previous) {
                // The port is rebound, so the address may have moved.
                self.addr = wait_for_listener(&self.home).await;
                if let (200, _) = self.json("GET", "/api/settings", None).await {
                    return pid;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no new server ever took the instance lock (still {current:?}). Supervisor log:\n{}",
                self.log()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// What the supervisor said, for a failure message.
    fn log(&self) -> String {
        std::fs::read_to_string(self.home.join("up.log")).unwrap_or_else(|error| format!("(no up.log: {error})"))
    }

    async fn json(&self, method: &str, path: &str, body: Option<&str>) -> (u16, Value) {
        let (status, raw) = request(self.addr, method, path, "application/json", body.map(str::as_bytes)).await;
        let parsed = serde_json::from_slice(&raw).unwrap_or(Value::Null);
        (status, parsed)
    }

    async fn get_json(&self, path: &str) -> Value {
        self.json("GET", path, None).await.1
    }

    /// `POST /api/server/self-upgrade`, with a multipart body written by hand.
    ///
    /// Built here rather than with a client library because the body's exact
    /// shape *is* the contract this route publishes, and a test that generated
    /// it with the same understanding the handler parses it with would not be
    /// checking anything.
    async fn upload_binary(&self, bytes: &[u8], sha256: Option<&str>) -> (u16, Value) {
        const BOUNDARY: &str = "----remote-hand-self-upgrade-test";
        let mut body = Vec::new();
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"binary\"; filename=\"remote-hand-server\"\r\n\
                 Content-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
        if let Some(digest) = sha256 {
            body.extend_from_slice(format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"sha256\"\r\n\r\n{digest}\r\n").as_bytes());
        }
        body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());

        let (status, raw) = request(
            self.addr,
            "POST",
            "/api/server/self-upgrade",
            &format!("multipart/form-data; boundary={BOUNDARY}"),
            Some(&body),
        )
        .await;
        (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
    }

    async fn attach(&self, id: &str) -> Socket {
        let url = format!("ws://{}/api/terminals/{id}/ws", self.addr);
        let deadline = std::time::Instant::now() + BUDGET;
        loop {
            match tokio_tungstenite::connect_async(&url).await {
                Ok((socket, _)) => return Socket(socket),
                Err(error) => {
                    assert!(std::time::Instant::now() < deadline, "cannot attach to {id}: {error}");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }

    /// Stop the whole thing the way `docker stop` would: one SIGTERM to the
    /// supervisor, which forwards it to the server and then exits.
    async fn stop(mut self) {
        self.stopped = true;
        // SAFETY: a signal to one pid this test started.
        unsafe { libc::kill(self.pid as libc::pid_t, libc::SIGTERM) };
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while alive(self.pid) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // Whatever the supervisor chose to leave running — the shims and the
        // CLI, by design — is this test's to clean up.
        // SAFETY: a negative pid names a process group.
        unsafe { libc::kill(-(self.pid as libc::pid_t), libc::SIGKILL) };
        for pid in shim_pids(&self.home) {
            // SAFETY: as above, for each shim this test's server started.
            unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
        }
    }
}

/// Where a server rooted at `home` is listening, once it is.
async fn wait_for_listener(home: &Path) -> std::net::SocketAddr {
    let port_file = home.join(".local/share/remote-hand/instances/default/server.port");
    let deadline = std::time::Instant::now() + BUDGET;
    loop {
        if let Some(port) = std::fs::read_to_string(&port_file).ok().and_then(|t| t.trim().parse::<u16>().ok()) {
            let candidate: std::net::SocketAddr = ([127, 0, 0, 1], port).into();
            if tokio::net::TcpStream::connect(candidate).await.is_ok() {
                return candidate;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the supervised server never recorded a port it was listening on"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A connected attach socket that counts ticks.
struct Socket(tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>);

impl Socket {
    /// Type into the terminal.
    async fn send_input(&mut self, text: &str) {
        let frame = serde_json::json!({"type": "input", "data": text}).to_string();
        self.0.send(Message::Text(frame.into())).await.expect("send input");
    }

    /// Read frames until a tick numbered at least `target` arrives, and return
    /// the highest tick seen.
    async fn read_ticks_until(&mut self, target: u64) -> u64 {
        let deadline = std::time::Instant::now() + BUDGET;
        let mut highest = 0;
        let mut seen = String::new();
        while highest < target {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(!remaining.is_zero(), "no tick reached {target}; highest was {highest}");
            let message = match tokio::time::timeout(remaining, self.0.next()).await {
                Ok(Some(Ok(message))) => message,
                Ok(Some(Err(error))) => panic!("websocket error waiting for tick {target}: {error}"),
                Ok(None) => panic!("the socket closed before tick {target}; highest was {highest}"),
                Err(_) => panic!("no tick reached {target}; highest was {highest}"),
            };
            if let Message::Text(text) = message {
                seen.push_str(&text);
                if let Some(max) = ticks(&seen).last() {
                    highest = *max;
                }
            }
        }
        highest
    }
}

// ---------------------------------------------------------------- utilities

/// Every `tick N` in `text`, in the order it appears.
///
/// Deliberately not a regex: the pattern is two literal tokens, and a hand
/// scanner cannot accidentally match the command line the shell echoed back
/// (which contains `tick %d`, not `tick 7`).
fn ticks(text: &str) -> Vec<u64> {
    let mut found = Vec::new();
    for part in text.split("tick ").skip(1) {
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        // A truncated frame can end mid-number; the next read will carry the
        // whole line, so a partial one is skipped rather than mis-parsed.
        if !digits.is_empty() && part.len() > digits.len() {
            if let Ok(value) = digits.parse() {
                found.push(value);
            }
        }
    }
    found
}

/// Fail unless `ticks` counts up by exactly one, with nothing missing.
fn assert_contiguous(ticks: &[u64]) {
    assert!(!ticks.is_empty(), "the shell produced no ticks at all");
    for pair in ticks.windows(2) {
        assert_eq!(
            pair[1],
            pair[0] + 1,
            "the tick sequence jumps from {} to {}: the shell was interrupted or restarted. Full sequence: {ticks:?}",
            pair[0],
            pair[1]
        );
    }
}

/// Every tick the shim has buffered for a terminal, from the start of its ring
/// up to and including `until`.
///
/// Read straight from the shim, bypassing both servers: what this returns is
/// what the *shell* produced, which is the question continuity is really about.
///
/// `until` is what makes this terminate. The shell under test is an infinite
/// loop, so "read to the end of the output" is not a thing that happens and a
/// quiet-for-a-moment heuristic would simply never fire — the drain has to stop
/// at a tick the caller already knows arrived.
async fn ticks_in_ring(shims: &Path, id: &str, until: u64) -> Vec<u64> {
    let sidecars = remote_hand_server::pty_shim::sidecar::Sidecars::new(shims, id);
    let mut link = remote_hand_server::pty_shim::client::connect(&sidecars.socket(), 0)
        .await
        .expect("connect to the shim");
    let deadline = std::time::Instant::now() + BUDGET;
    let mut text = String::new();
    let mut found = Vec::new();
    while found.last().copied().unwrap_or(0) < until {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let frame = match tokio::time::timeout(remaining, link.recv()).await {
            Ok(Some(frame)) => frame,
            Ok(None) => panic!("the shim closed before tick {until}; saw {found:?}"),
            Err(_) => panic!("the shim never replayed as far as tick {until}; saw {found:?}"),
        };
        if frame.kind == remote_hand_server::pty_shim::proto::OUTPUT {
            text.push_str(&String::from_utf8_lossy(&frame.payload));
            found = ticks(&text);
        }
    }
    found
}

/// Which shell a shim is minding, asked without disturbing the server's link.
///
/// `u64::MAX` as the cursor is the peek idiom the terminal tests use: the ring
/// clamps it to the head, so this connection learns the pid and is handed no
/// output.
async fn shell_pid(shims: &Path, id: &str) -> u32 {
    let sidecars = remote_hand_server::pty_shim::sidecar::Sidecars::new(shims, id);
    remote_hand_server::pty_shim::client::connect(&sidecars.socket(), u64::MAX)
        .await
        .expect("connect to the shim")
        .shell_pid
}

/// The pids of every `pty-shim` serving this test's home directory.
///
/// Matched on the home path as well as the subcommand, so a shim belonging to
/// another test running concurrently is never counted.
fn shim_pids(home: &Path) -> Vec<u32> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-A", "-o", "pid=,command="])
        .output()
        .expect("ps");
    let needle = home.to_string_lossy().into_owned();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains("pty-shim") && line.contains(&needle))
        .filter_map(|line| line.split_whitespace().next()?.parse().ok())
        .collect()
}

/// The single shim this test created, or a failure naming what was found.
fn one_shim_pid(home: &Path) -> u32 {
    let pids = shim_pids(home);
    assert_eq!(pids.len(), 1, "expected exactly one pty shim for this test, found {pids:?}");
    pids[0]
}

/// The single record in the instance's run journal, once one appears.
///
/// The `runs` table of `state.db`, read from outside the process that writes it:
/// the server under test here is the real binary under a supervisor, and what
/// this test is about is that the row survives the binary being replaced
/// underneath it.
async fn wait_for_record(db: &Path) -> Value {
    let deadline = std::time::Instant::now() + BUDGET;
    loop {
        if let Some(first) = support::common::json_rows(db, "SELECT payload FROM runs ORDER BY ordinal", ())
            .into_iter()
            .next()
        {
            return first;
        }
        assert!(std::time::Instant::now() < deadline, "no run was ever recorded in {}", db.display());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn alive(pid: u32) -> bool {
    remote_hand_server::pty_shim::client::process_alive(pid)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Kills a process group when it goes out of scope.
struct KillGroupOnDrop(u32);

impl Drop for KillGroupOnDrop {
    fn drop(&mut self) {
        // SAFETY: a negative pid names a process group; killing one that has
        // already gone is an ignored `ESRCH`.
        unsafe { libc::kill(-(self.0 as libc::pid_t), libc::SIGKILL) };
    }
}

/// A one-shot HTTP/1.1 request against a server in another process.
async fn request(addr: std::net::SocketAddr, method: &str, path: &str, content_type: &str, body: Option<&[u8]>) -> (u16, Vec<u8>) {
    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if let Some(body) = body {
        head.push_str(&format!("Content-Type: {content_type}\r\nContent-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await.expect("write head");
    if let Some(body) = body {
        stream.write_all(body).await.expect("write body");
    }

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read");
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("malformed HTTP response: {:?}", String::from_utf8_lossy(&raw)));
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("status line");
    (status, raw[split + 4..].to_vec())
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn ticks_are_read_from_the_stream_and_not_from_the_echoed_command() {
        // The shell echoes the command line first; it contains the word `tick`
        // followed by a format specifier, which must not parse as a number.
        let stream = "i=0; while :; do printf 'tick %d\\n' \"$i\"; done\r\ntick 1\r\ntick 2\r\ntick 3\r\n";
        assert_eq!(ticks(stream), vec![1, 2, 3]);
        assert_contiguous(&ticks(stream));
    }

    #[test]
    fn a_frame_that_ends_mid_number_is_not_mis_parsed() {
        // The pty coalesces on a timer, so a frame can end anywhere — including
        // between the digits of a number. Reading `tick 1` out of `tick 12`
        // would invent a gap that never happened.
        assert_eq!(ticks("tick 9\r\ntick 1"), vec![9]);
        assert_eq!(ticks("tick 9\r\ntick 10\r\n"), vec![9, 10]);
    }

    #[test]
    #[should_panic(expected = "jumps from 4 to 6")]
    fn a_gap_in_the_ticks_is_a_failure() {
        assert_contiguous(&[3, 4, 6]);
    }
}

// ---------------------------------------------------- the ways it can go wrong
//
// Every one of these is about the same promise: an upgrade that does not happen
// must leave nothing behind. A staged binary is the size of a server, so
// "nothing" is not a tidiness point — four abandoned uploads is half a gigabyte
// of a data volume, and the volume is usually the thing that is short.

/// A server started *without* a supervisor, on a home directory the test owns.
struct Unsupervised {
    pid: u32,
    addr: std::net::SocketAddr,
    home: PathBuf,
    stopped: bool,
}

impl Drop for Unsupervised {
    fn drop(&mut self) {
        if !self.stopped {
            // SAFETY: a negative pid names a process group this test started.
            unsafe { libc::kill(-(self.pid as libc::pid_t), libc::SIGKILL) };
        }
    }
}

impl Unsupervised {
    /// Start from the test's own build, which is not under `<data>/bin`.
    async fn start(home: &Path) -> Unsupervised {
        Self::start_from(home, Path::new(env!("CARGO_BIN_EXE_remote-hand-server"))).await
    }

    /// Start the *installed* binary by hand, which is what an operator does when
    /// debugging — and what once fooled `Restart::detect` into answering
    /// `restarting` with nothing there to restart it.
    async fn start_installed(home: &Path) -> Unsupervised {
        let bin = home.join(".local/share/remote-hand/bin");
        std::fs::create_dir_all(&bin).expect("bin dir");
        let installed = bin.join("remote-hand-server");
        std::fs::copy(env!("CARGO_BIN_EXE_remote-hand-server"), &installed).expect("install");
        std::fs::set_permissions(&installed, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        Self::start_from(home, &installed).await
    }

    async fn start_from(home: &Path, exe: &Path) -> Unsupervised {
        forget_port(home);
        let mut command = tokio::process::Command::new(exe);
        command
            .arg("serve")
            .env("APP_HOME_DIR", home)
            // No login: not a test of authentication (`tests/auth.rs` is).
            .env("APP_AUTH__ENABLED", "false")
            .env("APP_SERVER__PORT", "0")
            .env("APP_SERVER__HOST", "127.0.0.1")
            // Deliberately *not* set, under either name: this is the whole
            // point of the harness.
            .env_remove("DROPPEN_SUPERVISED")
            .env_remove("RH_SUPERVISED")
            .process_group(0)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(false);
        let mut child = command.spawn().expect("spawn the server");
        let pid = child.id().expect("the server has a pid");
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
        let mut server = Unsupervised {
            pid,
            addr: ([127, 0, 0, 1], 0).into(),
            home: home.to_path_buf(),
            stopped: false,
        };
        server.addr = wait_for_listener(home).await;
        server
    }

    async fn stop(mut self) {
        self.stopped = true;
        // SAFETY: a negative pid names a process group this test started.
        unsafe { libc::kill(-(self.pid as libc::pid_t), libc::SIGKILL) };
        for pid in shim_pids(&self.home) {
            // SAFETY: as above, for each shim this server started.
            unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
        }
    }
}

/// Every `.staging-*` in a home directory's binary store.
fn staging_files(home: &Path) -> Vec<PathBuf> {
    let bin = home.join(".local/share/remote-hand/bin");
    let Ok(entries) = std::fs::read_dir(&bin) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".staging-"))
        })
        .collect()
}

/// Poll until the store is clean, so an assertion cannot race the cleanup.
async fn wait_for_a_clean_store(home: &Path) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let left = staging_files(home);
        if left.is_empty() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "an abandoned upload left {} behind in <data>/bin: {left:?}",
            left.len()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// One multipart body, so the failure tests all send the same shape.
fn multipart(boundary: &str, bytes: &[u8], sha256: Option<&str>) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"binary\"; filename=\"remote-hand-server\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n");
    if let Some(digest) = sha256 {
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"sha256\"\r\n\r\n{digest}\r\n").as_bytes());
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

async fn post_upgrade(addr: std::net::SocketAddr, bytes: &[u8], sha256: Option<&str>) -> (u16, Value) {
    const BOUNDARY: &str = "----remote-hand-self-upgrade-test";
    let body = multipart(BOUNDARY, bytes, sha256);
    let (status, raw) = request(
        addr,
        "POST",
        "/api/server/self-upgrade",
        &format!("multipart/form-data; boundary={BOUNDARY}"),
        Some(&body),
    )
    .await;
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

#[tokio::test]
async fn an_upload_that_is_refused_leaves_no_staged_binary_behind() {
    let home = tempfile::tempdir().expect("tempdir");
    let server = Unsupervised::start(home.path()).await;

    // A hash that does not match. The refusal happens after the whole body has
    // been written to disk, which is exactly the window a guard built after the
    // upload finished would have missed.
    let payload = b"#!/bin/sh\nexit 0\n";
    let (status, body) = post_upgrade(server.addr, payload, Some(&"0".repeat(64))).await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body["error"].as_str().is_some_and(|error| error.contains("hashes to")),
        "the refusal names both digests: {body}"
    );
    wait_for_a_clean_store(home.path()).await;

    // A binary that arrives intact and is then refused by the health check,
    // which is the other late failure.
    let (status, body) = post_upgrade(server.addr, payload, None).await;
    assert_eq!(status, 400, "{body}");
    wait_for_a_clean_store(home.path()).await;

    // A body with no `binary` part at all never creates a file to begin with.
    let (status, _) = request(
        server.addr,
        "POST",
        "/api/server/self-upgrade",
        "multipart/form-data; boundary=----x",
        Some(b"------x\r\nContent-Disposition: form-data; name=\"sha256\"\r\n\r\nabc\r\n------x--\r\n"),
    )
    .await;
    assert_eq!(status, 400);
    wait_for_a_clean_store(home.path()).await;

    // And the incumbent was never touched by any of it.
    assert!(!home.path().join(".local/share/remote-hand/bin/remote-hand-server.prev").exists());

    server.stop().await;
}

#[tokio::test]
async fn a_staged_binary_left_by_a_killed_server_is_swept_at_the_next_boot() {
    // The case nothing else covers. An upload guards itself, and a client that
    // hangs up cancels the handler, whose locals drop — but a process that is
    // killed outright between creating the file and finishing it runs no
    // destructors at all. Each of those leaves a server-sized file, so without a
    // sweep the cost is one per crash, forever.
    let home = tempfile::tempdir().expect("tempdir");
    let bin = home.path().join(".local/share/remote-hand/bin");
    std::fs::create_dir_all(&bin).expect("bin dir");
    let abandoned: Vec<PathBuf> = ["staging-101-aaa", "staging-102-bbb"]
        .iter()
        .map(|name| {
            let path = bin.join(format!(".{name}"));
            std::fs::write(&path, vec![b'x'; 4096]).expect("write");
            path
        })
        .collect();
    // Something that is not a staging file, to prove the sweep is not a `rm -rf`.
    let bystander = bin.join("remote-hand-server.prev");
    std::fs::write(&bystander, "a previous binary").expect("write");

    let server = Unsupervised::start(home.path()).await;
    wait_for_a_clean_store(home.path()).await;
    for path in &abandoned {
        assert!(!path.exists(), "{} survived the boot sweep", path.display());
    }
    assert!(bystander.exists(), "the sweep must only take staging files");

    server.stop().await;
}

#[tokio::test]
async fn an_upgrade_also_clears_what_an_earlier_crash_left() {
    // The other half: a server that is never restarted would otherwise carry a
    // crashed attempt's file until it was.
    let home = tempfile::tempdir().expect("tempdir");
    let server = Unsupervised::start(home.path()).await;

    // A server that has never been asked to upgrade has no `<data>/bin` yet;
    // only `up` creates one at boot. The crashed attempt this is standing in for
    // would have created it, so the test does too.
    let bin = home.path().join(".local/share/remote-hand/bin");
    std::fs::create_dir_all(&bin).expect("bin dir");
    let abandoned = bin.join(".staging-999-old");
    std::fs::write(&abandoned, vec![b'x'; 4096]).expect("write");

    // Any upgrade attempt will do, including one that is refused.
    let (status, _) = post_upgrade(server.addr, b"#!/bin/sh\nexit 0\n", None).await;
    assert_eq!(status, 400);
    assert!(!abandoned.exists(), "the attempt swept what the last crash left");

    server.stop().await;
}

#[tokio::test]
async fn a_client_that_hangs_up_mid_upload_leaves_no_staged_binary_behind() {
    let home = tempfile::tempdir().expect("tempdir");
    let server = Unsupervised::start(home.path()).await;

    // Big enough that the server is certainly still reading when the socket
    // goes away, which is the case a real interrupted `curl` produces.
    let payload = vec![b'x'; 8 * 1024 * 1024];
    const BOUNDARY: &str = "----remote-hand-abort";
    let body = multipart(BOUNDARY, &payload, None);

    let mut stream = tokio::net::TcpStream::connect(server.addr).await.expect("connect");
    let head = format!(
        "POST /api/server/self-upgrade HTTP/1.1\r\nHost: {}\r\n\
         Content-Type: multipart/form-data; boundary={BOUNDARY}\r\nContent-Length: {}\r\n\r\n",
        server.addr,
        body.len()
    );
    stream.write_all(head.as_bytes()).await.expect("write head");
    // Promise the whole body, deliver a tenth of it, then hang up.
    stream.write_all(&body[..body.len() / 10]).await.expect("write part");
    stream.flush().await.expect("flush");
    drop(stream);

    wait_for_a_clean_store(home.path()).await;
    // The server is still there: an abandoned upload is not a crash.
    assert!(alive(server.pid), "the server survived a client hanging up");
    let (status, _) = request(server.addr, "GET", "/api/settings", "application/json", None).await;
    assert_eq!(status, 200, "and is still answering");

    server.stop().await;
}

#[tokio::test]
async fn an_unsupervised_server_installs_the_binary_and_stays_up() {
    // The regression this pins: `Restart::detect` once treated "my executable
    // lives under <data>/bin" as proof of a supervisor. An operator running the
    // installed binary by hand was told `restarting`, and then the process
    // exited with nothing to bring it back.
    //
    // So this server has to be started *from the installed path* — a server run
    // out of `target/debug` never trips the old rule, and a test that used one
    // would pass against the bug.
    let home = tempfile::tempdir().expect("tempdir");
    let server = Unsupervised::start_installed(home.path()).await;
    let bytes = std::fs::read(env!("CARGO_BIN_EXE_remote-hand-server")).expect("read this test's own binary");

    let (status, answer) = post_upgrade(server.addr, &bytes, Some(&sha256_hex(&bytes))).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["restart"], "manual", "nobody is watching this server: {answer}");
    assert!(
        answer["detail"].as_str().is_some_and(|detail| detail.contains("restart it")),
        "and it says what the operator has to do: {answer}"
    );

    // The swap still happened — that half is unconditional.
    assert!(home.path().join(".local/share/remote-hand/bin/remote-hand-server").is_file());

    // The point: it is still running. Given a moment, because the bug was a
    // delayed exit and an immediate check would not have seen it either.
    tokio::time::sleep(Duration::from_millis(2000)).await;
    assert!(alive(server.pid), "an unsupervised server must not stop itself");
    let (status, _) = request(server.addr, "GET", "/api/settings", "application/json", None).await;
    assert_eq!(status, 200, "and must still be serving");

    server.stop().await;
}

/// A deleted lock file means the flock this server holds is on an unlinked
/// inode and keeps nobody out; replacing the shared binary under that claim
/// could race a sibling that now owns the instance. The ownership check
/// answers before a byte of the upload is read: garbage that would otherwise
/// be a 400 from the health check comes back 409 instead, and nothing is
/// staged or swapped.
#[tokio::test]
async fn a_lost_instance_lock_refuses_the_upgrade_before_a_byte_is_staged() {
    let home = tempfile::tempdir().expect("tempdir");
    let probe = Probe::start_at(home.path()).await;
    probe.stop().await;

    let up = Supervisor::start(home.path()).await;
    let lock = home.path().join(".local/share/remote-hand/instances/default/lock");
    std::fs::remove_file(&lock).expect("remove the lock underneath the server");

    let (status, body) = up.upload_binary(b"not a binary at all", None).await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"], "instance_lock_lost", "{body}");

    // Nothing staged, nothing swapped, and no restart happened: a restarted
    // server would have re-acquired the lock and recreated the file.
    let bin_dir = home.path().join(".local/share/remote-hand/bin");
    let staged = std::fs::read_dir(&bin_dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|e| e.file_name().to_string_lossy().starts_with(".staging-"))
                .count()
        })
        .unwrap_or(0);
    assert_eq!(staged, 0, "an upload refused for ownership must stage nothing");
    assert!(!bin_dir.join("remote-hand-server.prev").exists(), ".prev written under a lost lock");
    assert!(!lock.exists(), "the server restarted on a refused upgrade");
    let (status, _) = up.json("GET", "/api/terminals", None).await;
    assert_eq!(status, 200, "the refusing server must keep serving");

    up.stop().await;
}
