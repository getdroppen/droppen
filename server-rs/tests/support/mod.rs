//! A server pointed at a synthetic home, with fake `claude` / `codex`
//! executables on `PATH`.
//!
//! The CLIs are replaced by scripts that record the argv and the environment
//! they were started with, so the account plumbing — which is exactly "what
//! ends up in a child process" — can be asserted on without a real login.
//!
//! Two rules keep the tests independent of each other despite `PATH` being
//! process-global:
//!
//! * the scripts are written once per test binary and are stateless; everything
//!   they record goes into their **working directory**, which the server derives
//!   from per-test state (a session's `cwd`, or a probe's config dir);
//! * every server is pointed at its own `tempdir` through
//!   `RemoteHandApp::home_override`, so config, data, both Claude roots and the
//!   Codex tree are all inside it.
//!
//! The server itself and the HTTP transport come from [`common::TestServer`],
//! which every other integration test uses; everything below is the part that is
//! only about spawned CLIs.

#![allow(dead_code)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;

#[path = "../common/mod.rs"]
pub mod common;

/// The fake `claude`: a resident that takes messages on stdin, like the real one.
///
/// remote-hand no longer runs `claude --print` once per message. It starts one
/// `claude -p --input-format stream-json --output-format stream-json --verbose`
/// per session and keeps it, so this script is a *process* too: it announces
/// itself with a `system:init` line, then reads one JSON message per turn and
/// answers each with a `result`. The two events the old fake conflated — "the
/// turn is over" and "the CLI is gone" — are now separate here as well.
///
/// Behaviour is driven entirely by files in its working directory, so two tests
/// can never configure each other's runs:
///
/// * `argv.bin` / `env.txt` — what it was started with (overwritten per spawn);
/// * `calls.txt` — one line per *spawn*, which under a resident is one line per
///   process rather than one per message: that difference is itself assertable;
/// * `stdin.jsonl` — every line it was handed, so the prompt can be read back
///   from where it now travels;
/// * `announce.txt` — the session id to answer under, for a CLI that resumes
///   into a session id of its own choosing;
/// * `fork.txt` / `fork.body` — a transcript to write on startup, for the
///   neighbouring-session cases;
/// * `mute.txt` — take this long to say `system:init`, which is the window a
///   cold CLI spends being started and unable to be written to;
/// * `drip.txt` — `<lines> <seconds>`: an assistant line this many times,
///   waiting this long between them, for asserting on what counts as progress;
/// * `hold` / `go` — hold the turn open until a `go` file appears, which is how
///   a test catches a session mid-turn without racing a clock;
/// * `wake.txt` / `wake` / `woken` — run a turn nothing on stdin asked for, as a
///   finished background task makes a real CLI do;
/// * `sleep.txt` — hold the turn open this long instead;
/// * `stderr.txt` — last words, for the tail a failed turn carries;
/// * `exit.txt` — die with this code mid-turn instead of finishing it;
/// * `usage.out` / `auth.out` — replies for the two read-only subcommands.
///
/// It also takes a directory lock under the test's home (found by walking up to
/// `.rh-test-root`) and records any overlap, which is how "probes are
/// serialized" is asserted without a clock.
///
/// # Why the marker is checked before anything is written
///
/// A resident outlives the server that started it — that is the entire point of
/// the shim — so a test can finish, drop its `tempdir`, and leave this script
/// running in a directory that no longer exists. `getcwd` then fails, and a
/// shell falls back to the inherited `PWD`, which is the *server's* working
/// directory: the checkout. Refusing to write outside a test's own home is what
/// keeps that from dropping `argv.bin` into the source tree.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
dir=$(/bin/pwd 2>/dev/null) || dir="$PWD"
root="$dir"
while [ "$root" != "/" ] && [ ! -f "$root/.rh-test-root" ]; do root=$(dirname "$root"); done
[ -f "$root/.rh-test-root" ] || exit 97

# The reader polls for `spawn.done`, so argv and env are complete before it
# appears: a half-written `env.txt` would otherwise read as a missing variable.
rm -f "$dir/spawn.done"
: > "$dir/argv.tmp"
for arg in "$@"; do printf '%s\0' "$arg" >> "$dir/argv.tmp"; done
env > "$dir/env.tmp"
mv "$dir/argv.tmp" "$dir/argv.bin"
mv "$dir/env.tmp" "$dir/env.txt"
echo call >> "$dir/calls.txt"
: > "$dir/spawn.done"

if [ "$1" = "auth" ]; then
  if [ -f "$dir/auth.out" ]; then cat "$dir/auth.out"; exit 0; fi
  exit 1
fi

if [ "$1" = "-p" ] && [ "$2" = "/usage" ]; then
  if mkdir "$root/probe.lock" 2>/dev/null; then
    sleep 0.3
    rmdir "$root/probe.lock"
  else
    echo overlap >> "$root/overlap.txt"
  fi
  if [ -f "$dir/usage.out" ]; then cat "$dir/usage.out"; fi
  exit 0
fi

# Which id to answer under: the one the selector asked for, unless a test wants
# a CLI that resumes into a session of its own choosing.
session=""
previous=""
for arg in "$@"; do
  case "$previous" in
    --resume|--session-id) session="$arg" ;;
  esac
  previous="$arg"
done
if [ -f "$dir/announce.txt" ]; then read -r session < "$dir/announce.txt"; fi

if [ -f "$dir/fork.txt" ]; then
  read -r forked < "$dir/fork.txt"
  mkdir -p "$(dirname "$forked")"
  cat "$dir/fork.body" > "$forked"
fi

if [ -f "$dir/stderr.txt" ]; then cat "$dir/stderr.txt" >&2; fi

# A CLI that is slow to come up. Nothing may be written to it until it says it
# is listening, so this is the window in which a message has been accepted and
# no turn can have started yet.
if [ -f "$dir/mute.txt" ]; then sleep "$(cat "$dir/mute.txt")"; fi

# The handshake. `hook_started` really does come before `init`, and lines that
# are not JSON really do turn up, so both are here: a reader that took the first
# line as the handshake, or refused the junk, would already be wrong.
printf '%s\n' '{"type":"system","subtype":"hook_started"}'
printf '%s\n' 'this line is not json'
printf '{"type":"system","subtype":"init","session_id":"%s"}\n' "$session"

# The turn claude opens for itself. A background task that finishes while the
# session is between turns wakes the main agent, and the CLI runs a whole turn
# with nothing on stdin to have started it — so this is a subshell, which is
# exactly what makes it the case remote-hand used to miss.
#
# The child line first, because a subagent's forwarded stream is what an idle
# session with background work out looks like; the line after it is this thread,
# and is the one that means a turn is running. Both waits are bounded so a test
# that fails halfway leaves nothing polling a directory that has gone.
if [ -f "$dir/wake.txt" ]; then
  (
    n=0; while [ ! -f "$dir/wake" ] && [ "$n" -lt 500 ]; do [ -d "$dir" ] || exit 97; sleep 0.02; n=$((n + 1)); done
    printf '%s\n' '{"type":"assistant","parent_tool_use_id":"toolu_child","message":{}}'
    printf '%s\n' '{"type":"assistant","message":{}}'
    n=0; while [ ! -f "$dir/woken" ] && [ "$n" -lt 500 ]; do [ -d "$dir" ] || exit 97; sleep 0.02; n=$((n + 1)); done
    printf '%s\n' '{"type":"result","subtype":"success","is_error":false}'
  ) &
fi

# One turn per line of stdin. A `control_request` is answered and nothing more:
# the real CLI only reads its stdin between turns, so an interrupt sent while a
# turn is in flight is answered by the signal ladder rather than by this.
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$dir/stdin.jsonl"
  case "$line" in
    *control_request*)
      printf '%s\n' '{"type":"control_response","response":{"subtype":"success"}}'
      continue ;;
  esac
  if [ -f "$dir/drip.txt" ]; then
    read -r ticks gap < "$dir/drip.txt"
    tick=0
    while [ "$tick" -lt "$ticks" ]; do
      tick=$((tick + 1))
      printf '{"type":"assistant","message":{"content":[{"type":"text","text":"working %s"}]}}\n' "$tick"
      sleep "$gap"
    done
  fi
  if [ -f "$dir/hold" ]; then
    # Bounded and anchored, for the same reason the waits above are: a test
    # that fails while the turn is held drops its `tempdir`, `go` can then
    # never arrive, and an unbounded loop here would poll a path that no longer
    # exists at fifty spawns a second until the machine is rebooted. Giving up
    # exits — falling through would print the `result` below and turn a hang
    # into a pass — with a code no test expects.
    n=0
    while [ ! -f "$dir/go" ]; do
      [ -d "$dir" ] || exit 97
      # Leave time for upgrade tests to copy and health-check a large debug binary.
      [ "$n" -lt 6000 ] || exit 97
      sleep 0.02
      n=$((n + 1))
    done
    rm -f "$dir/go"
  fi
  if [ -f "$dir/sleep.txt" ]; then sleep "$(cat "$dir/sleep.txt")"; fi
  if [ -f "$dir/exit.txt" ]; then exit "$(cat "$dir/exit.txt")"; fi
  printf '%s\n' '{"type":"result","subtype":"success","is_error":false}'
done
exit 0
"#;

/// The fake Codex implements the bidirectional App Server protocol.
const FAKE_CODEX: &str = include_str!("fake_codex.py");

/// The one directory a binary got, and the arguments it was built for.
static FAKE_BIN: OnceLock<(String, Vec<(String, String)>, PathBuf)> = OnceLock::new();

/// Put fake executables on `PATH`, once per test binary, and return their
/// directory.
///
/// The directory is deliberately leaked: the scripts have to outlive every test
/// in the binary, and there is no later point at which "every test is done" is
/// observable from inside one.
///
/// `PATH` is process-global and the harness runs tests in threads, so the write
/// below is the one piece of shared mutable state in this fixture. The
/// [`OnceLock`] makes it happen exactly once and before any test in the binary
/// spawns a CLI — every entry point calls this first. It is not, strictly, free
/// of a concurrent `getenv`: a test thread already inside `clean_env` when the
/// first one initializes could observe the write mid-flight. Removing the
/// window entirely would mean an environment snapshot taken before `main`, which
/// Rust's test harness gives no hook for.
///
/// Only one directory exists per binary, so a second call with a different
/// `prefix` or script set panics rather than silently handing back the first
/// caller's directory.
pub fn fake_bin(prefix: &str, scripts: &[(&str, &str)]) -> &'static Path {
    let (built_for, built_scripts, dir) = FAKE_BIN.get_or_init(|| {
        let dir = tempfile::Builder::new().prefix(prefix).tempdir().expect("fake bin dir").keep();
        for (name, body) in scripts {
            let path = dir.join(name);
            std::fs::write(&path, body).expect("write fake cli");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
            }
        }
        // Prepended, so the fakes win over anything really installed, while
        // `sh`, `env` and friends stay reachable.
        let existing = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{existing}", dir.display()));
        let scripts = scripts.iter().map(|(name, body)| (name.to_string(), body.to_string())).collect();
        (prefix.to_string(), scripts, dir)
    });
    assert!(
        built_for.as_str() == prefix,
        "fake_bin was already built for prefix {built_for:?}, so a call for {prefix:?} would silently reuse the wrong directory"
    );
    let wanted: Vec<(String, String)> = scripts.iter().map(|(name, body)| (name.to_string(), body.to_string())).collect();
    assert!(
        *built_scripts == wanted,
        "fake_bin was already built with a different script set, so this call would silently reuse the wrong directory"
    );
    dir
}

fn ensure_fake_clis_on_path() -> &'static Path {
    fake_bin("remote-hand-fake-bin-", &[("claude", FAKE_CLAUDE), ("codex", FAKE_CODEX)])
}

/// The Claude root the `default` instance owns, under a home with no server yet.
///
/// A restart fixture has to write a transcript *before* any server exists, and
/// the sessions it seeds must be the next server's own — an external one is
/// browse-only and could not be sent to.
pub fn owned_claude_root(home: &Path) -> PathBuf {
    remote_hand_server::sessions::Paths::new(home.to_path_buf()).claude_root(remote_hand_server::claude_roots::ClaudeRoot::Managed)
}

/// A running server plus its synthetic home.
pub struct Probe {
    /// Where the server thinks `$HOME` is.
    pub home: PathBuf,
    /// Which instance this server owns.
    ///
    /// Carried rather than assumed: everything that survives a restart lives
    /// under `instances/<name>/`, so a helper that hard-coded `default` handed a
    /// sibling probe the *other* server's state — and an assertion that reads
    /// the wrong instance's database passes or fails for reasons that have
    /// nothing to do with the test.
    instance: String,
    server: common::TestServer,
    /// Kills the sessions this server started, before the home they run in goes
    /// away. Declared above `_home` because that is the order it has to happen
    /// in, and field order is what decides it.
    residents: ResidentReaper,
    /// Owned by the probe rather than the server, so a home directory can
    /// outlive the server that was pointed at it — which is what a restart and
    /// a second instance both need.
    _home: Option<tempfile::TempDir>,
}

/// Kills every resident the instance's journal names when it is dropped.
///
/// A resident `claude` deliberately outlives the server that spawned it — that
/// is what the session shim is for — so no shutdown path ends one, and a test
/// that sends a single message would otherwise leave a shim and a CLI running
/// for ever, holding a deleted `tempdir` open. claude leads a process group of
/// its own, so one signal per record does it and the shim follows its child out.
///
/// Its own type rather than a `Drop` on [`Probe`] so that `Probe::stop` can
/// still move the server out of itself, and so that the order relative to the
/// `tempdir` is stated where it is decided.
struct ResidentReaper {
    state_db: PathBuf,
}

impl ResidentReaper {
    /// Signal every resident the journal currently names.
    ///
    /// Run twice — once before the server is stopped and once when the probe
    /// goes — because a respawn rewrites the journal, and a sweep that landed in
    /// the gap between the outgoing record being retired and the incoming one
    /// being written would find neither. Killing an already-dead process group
    /// is a no-op, so doing it twice costs nothing.
    fn sweep(&self) {
        // Read-only, and every failure is a silent return: the journal is a
        // database now, the server that owns it may already be gone, and a
        // reaper that panicked would fail the test it was cleaning up after.
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI;
        let Ok(conn) = rusqlite::Connection::open_with_flags(&self.state_db, flags) else {
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
            let Some(pid) = record["cli_pid"].as_u64() else { continue };
            // SAFETY: `kill` delivers a signal to a process group and cannot
            // corrupt this process' own state.
            unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
        }
    }
}

impl Drop for ResidentReaper {
    fn drop(&mut self) {
        self.sweep();
    }
}

/// Where one instance keeps its state, the journal included.
fn state_db_of(home: &Path, instance: &str) -> PathBuf {
    home.join(".local/share/remote-hand/instances").join(instance).join("state.db")
}

impl Probe {
    /// Boot a server on an ephemeral port, rooted at a fresh home directory.
    pub async fn start() -> Probe {
        Probe::start_named(None).await
    }

    /// The same, under an explicit instance name.
    pub async fn start_named(instance: Option<&str>) -> Probe {
        ensure_fake_clis_on_path();
        let home = tempfile::tempdir().expect("tempdir");
        // The marker the fake `claude` walks up to find the test's own root.
        std::fs::write(home.path().join(".rh-test-root"), "").expect("marker");
        let server = common::TestServer::start_at(home.path(), instance).await;
        let instance = instance.unwrap_or(remote_hand_server::instance::DEFAULT_INSTANCE).to_string();
        Probe {
            home: home.path().to_path_buf(),
            residents: ResidentReaper {
                state_db: state_db_of(home.path(), &instance),
            },
            instance,
            server,
            _home: Some(home),
        }
    }

    /// Boot a server on a home directory the *caller* owns and keeps alive.
    ///
    /// What the restart tests need: the home has to survive the server, either
    /// because it was seeded before one existed or because a second server is
    /// about to be pointed at it.
    pub async fn start_at(home: &Path) -> Probe {
        Probe::start_named_at(home, remote_hand_server::instance::DEFAULT_INSTANCE).await
    }

    /// The same, under an explicit instance name.
    ///
    /// What the migration tests need: which instance boots onto a home directory
    /// first is the whole question there, and `default` is only one of the
    /// answers.
    pub async fn start_named_at(home: &Path, instance: &str) -> Probe {
        ensure_fake_clis_on_path();
        let marker = home.join(".rh-test-root");
        if !marker.exists() {
            std::fs::write(&marker, "").expect("marker");
        }
        Probe {
            home: home.to_path_buf(),
            instance: instance.to_string(),
            server: common::TestServer::start_at(home, Some(instance)).await,
            residents: ResidentReaper {
                state_db: state_db_of(home, instance),
            },
            _home: None,
        }
    }

    /// A second server on this one's home, under a different instance name.
    ///
    /// The supported way to run two remote-hands side by side: disjoint
    /// instance directories, one shared set of sessions. The returned probe
    /// borrows the home directory and must be stopped before this one is
    /// dropped.
    pub async fn sibling(&self, instance: &str) -> Probe {
        Probe {
            home: self.home.clone(),
            instance: instance.to_string(),
            server: common::TestServer::start_at(&self.home, Some(instance)).await,
            residents: ResidentReaper {
                state_db: state_db_of(&self.home, instance),
            },
            _home: None,
        }
    }

    pub async fn stop(self) {
        // Before the server goes, so a driver that is mid-respawn is ended by
        // the sweep it can still be seen by rather than by the one in `Drop`,
        // which may read the journal in the gap between the two records.
        self.residents.sweep();
        self.server.stop().await;
    }

    /// The directory this probe's server exclusively owns.
    pub fn instance_dir(&self) -> PathBuf {
        self.data_dir().join("instances").join(&self.instance)
    }

    /// Everything this instance alone owns: the journal of runs in flight, the
    /// layouts, the login sessions, the registry of sessions it made, and the
    /// VAPID key and subscriptions.
    ///
    /// Instance-scoped for the reason the sidecars it absorbed were made
    /// instance-scoped one at a time: a subscription belongs to the one origin
    /// the browser granted it to, and a run belongs to the server that spawned
    /// it. Stopping this instance must not disturb its sibling.
    pub fn instance_db(&self) -> PathBuf {
        self.instance_dir().join("state.db")
    }

    /// Everything that is the machine's rather than one server's: the accounts
    /// and which session runs under which, the local user list, and the flat
    /// maps — preferences, titles, archived, seen, the composer's settings.
    ///
    /// Shared on purpose. An account is a person and a session marked read is
    /// read on every server on this machine; the tokens a phone logged in with
    /// are not, which is why those live next door in [`Probe::instance_db`].
    pub fn config_db(&self) -> PathBuf {
        self.config_dir().join("state.db")
    }

    /// Where the per-session claims live — shared by every instance.
    pub fn lock_dir(&self) -> PathBuf {
        self.data_dir().join("inject-locks")
    }

    /// Which instance made which codex thread — shared, for the same reason
    /// [`Probe::lock_dir`] is: the sibling that did not write it is the reader
    /// it exists for.
    pub fn claims_dir(&self) -> PathBuf {
        self.data_dir().join("codex-claims")
    }

    // ---------------------------------------------------------------- requests

    pub async fn get(&self, path: &str) -> (u16, String) {
        self.server.get(path).await
    }

    pub async fn get_json(&self, path: &str) -> Value {
        self.server.get_json(path).await
    }

    pub async fn send(&self, method: &str, path: &str, body: Value) -> (u16, Value) {
        let (status, raw) = self.server.json(method, path, &body).await;
        (status, parse(&raw))
    }

    /// The same, asserting the status so the happy path stays one line.
    pub async fn send_ok(&self, method: &str, path: &str, body: Value, expect: u16) -> Value {
        let (status, value) = self.send(method, path, body).await;
        assert_eq!(status, expect, "{method} {path} -> {status}: {value}");
        value
    }

    // ------------------------------------------------------------------ layout

    pub fn data_dir(&self) -> PathBuf {
        self.home.join(".local/share/remote-hand")
    }

    pub fn config_dir(&self) -> PathBuf {
        self.home.join(".config/remote-hand")
    }

    pub fn local_root(&self) -> PathBuf {
        self.home.join(".claude")
    }

    /// The Claude root this probe's server owns — its instance's own.
    pub fn managed_root(&self) -> PathBuf {
        self.instance_dir().join("claude")
    }

    /// A workspace the fake CLI can dump into: it is a session's `cwd`.
    pub fn workspace(&self) -> PathBuf {
        let dir = self.home.join("workspace");
        std::fs::create_dir_all(&dir).expect("workspace");
        dir
    }

    /// Write a one-entry transcript into one of the roots and return its path.
    pub fn seed_session(&self, root: &Path, id: &str, cwd: &Path) -> PathBuf {
        common::seed_transcript(
            root,
            "-workspace",
            id,
            &[serde_json::json!({
                "type": "user",
                "cwd": cwd.to_string_lossy(),
                "timestamp": "2024-05-01T10:00:00.000Z",
                "message": {"content": [{"type": "text", "text": "hello"}]},
            })],
        )
    }

    /// Create an oauth-token account over HTTP, asserting the `201` and
    /// returning the body.
    pub async fn create_account(&self, label: &str, secret: &str) -> Value {
        common::create_account(&self.server, label, secret).await
    }

    /// The same, returning the new account's id.
    pub async fn create_account_id(&self, label: &str, secret: &str) -> String {
        common::create_account_id(&self.server, label, secret).await
    }

    /// Wait until the run remote-hand started for this session has finished.
    pub async fn wait_for_injection(&self, source: &str, id: &str) -> Value {
        self.wait_for_injection_within(source, id, Duration::from_secs(10)).await
    }

    /// Wait for a run to leave `running`, giving it `budget` to do it in.
    ///
    /// A test whose run is *meant* to take longer than the default ten seconds
    /// says so here rather than racing the default: the budget is a guard
    /// against a hung server, not a statement about how long the run should be.
    pub async fn wait_for_injection_within(&self, source: &str, id: &str, budget: Duration) -> Value {
        let deadline = std::time::Instant::now() + budget;
        loop {
            let detail = self.get_json(&format!("/api/sessions/{source}/{id}")).await;
            let injection = detail["injection"].clone();
            if injection["state"].as_str().is_some_and(|state| state != "running") {
                return injection;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "injection {source}/{id} never finished: {injection}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Wait for the fake CLI to have dumped its argv into `dir`.
    pub async fn wait_for_spawn(&self, dir: &Path) -> Spawn {
        self.wait_for_spawn_number(dir, 1).await
    }

    /// The same, for the `nth` spawn in `dir` rather than the first.
    ///
    /// A resident serves message after message, so a second send usually starts
    /// no second process and the dumps are still the *first* one's. Where a test
    /// is about the respawn — a message whose settings the running process
    /// cannot satisfy — sampling `argv.bin` would read the outgoing process'
    /// command line and pass for the wrong reason.
    ///
    /// The budget is generous because a respawn is two waits back to back: the
    /// outgoing process is given `resident::CLOSE_GRACE` to leave when its stdin
    /// closes, and the incoming shim is given `shim::SPAWN_PATIENCE` to answer.
    /// A fixture whose patience is shorter than the code's is a flake waiting
    /// for a loaded machine.
    pub async fn wait_for_spawn_number(&self, dir: &Path, nth: usize) -> Spawn {
        let deadline = std::time::Instant::now() + Duration::from_secs(40);
        loop {
            if call_count(dir) >= nth {
                if let Some(spawn) = Spawn::read(dir) {
                    return spawn;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "spawn {nth} was never recorded in {} (saw {})",
                dir.display(),
                call_count(dir)
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

fn parse(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|error| panic!("expected JSON ({error}): {body}"))
}

/// What a fake CLI was started with.
pub struct Spawn {
    pub argv: Vec<String>,
    pub env: std::collections::HashMap<String, String>,
}

impl Spawn {
    /// What the fake `codex` recorded, which it writes under its own names.
    pub fn read_codex(dir: &Path) -> Option<Spawn> {
        Spawn::from_dumps(&dir.join("codex-argv.bin"), &dir.join("codex-env.txt"))
    }

    fn from_dumps(argv_path: &Path, env_path: &Path) -> Option<Spawn> {
        let raw = std::fs::read(argv_path).ok()?;
        let env_raw = std::fs::read_to_string(env_path).ok()?;
        if raw.is_empty() {
            return None;
        }
        let argv = raw
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect();
        let env = env_raw
            .lines()
            .filter_map(|line| line.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
            .collect();
        Some(Spawn { argv, env })
    }

    fn read(dir: &Path) -> Option<Spawn> {
        // The marker is written last, so both dumps are whole by the time it exists.
        if !dir.join("spawn.done").exists() {
            return None;
        }
        Spawn::from_dumps(&dir.join("argv.bin"), &dir.join("env.txt"))
    }

    pub fn env(&self, key: &str) -> Option<&str> {
        self.env.get(key).map(String::as_str)
    }

    /// The value that follows `flag` in the argv, if the flag is there at all.
    pub fn flag(&self, flag: &str) -> Option<&str> {
        let index = self.argv.iter().position(|arg| arg == flag)?;
        self.argv.get(index + 1).map(String::as_str)
    }

    /// Whether the argv is the resident stream-json command line.
    ///
    /// The whole of what replaced `--print`: one process per session, fed
    /// message after message, rather than one child per message with the prompt
    /// behind a `--` guard. `--verbose` is not decoration — the CLI refuses
    /// `--output-format stream-json` without it — so it is asserted with the
    /// rest.
    pub fn is_resident(&self) -> bool {
        let flags: Vec<&str> = self.argv.iter().map(String::as_str).collect();
        flags.starts_with(&["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"])
    }
}

/// Make the fake `claude` die in the middle of a turn.
///
/// The resident-era shape of "the CLI failed": a real one writes its complaint
/// to stderr and exits, which is what the failed turn's reason is built out of.
/// The turn's own `result` never arrives, so the driver reports the death.
pub fn plan_dying_cli(dir: &Path, stderr: &str, exit_code: i32) {
    std::fs::create_dir_all(dir).expect("plan dir");
    std::fs::write(dir.join("stderr.txt"), stderr).expect("stderr plan");
    std::fs::write(dir.join("exit.txt"), format!("{exit_code}\n")).expect("exit plan");
}

/// Make the fake `claude` emit a line every 100 ms, `lines` times.
///
/// A run that keeps producing output is a run that is making progress, however
/// long it takes overall — which is the distinction the idle watchdog exists to
/// draw and the one a wall-clock cap would get wrong.
pub fn plan_dripping_cli(dir: &Path, lines: u32, gap: Duration) {
    std::fs::create_dir_all(dir).expect("plan dir");
    let gap = gap.as_secs_f32();
    std::fs::write(dir.join("drip.txt"), format!("{lines} {gap}\n")).expect("drip plan");
}

/// Hold every turn open, so a session can be caught mid-flight.
///
/// Better than a sleep for anything that has to *end* on cue: the turn lasts
/// exactly until [`release_turn`], so a test asserting on a queue behind a
/// running turn is not racing a clock.
pub fn plan_held_cli(dir: &Path) {
    std::fs::create_dir_all(dir).expect("plan dir");
    std::fs::write(dir.join("hold"), "").expect("hold plan");
}

/// Let the fake `claude` finish the turn it is holding.
pub fn release_turn(dir: &Path) {
    std::fs::write(dir.join("go"), "").expect("go");
}

/// Arm the fake `claude` to run one turn of its own, unasked.
///
/// What a real CLI does when a background task finishes while the session is
/// between turns: the notification wakes the main agent and a whole turn goes by
/// with nothing on stdin to have started it. Armed at spawn and started by
/// [`wake_cli`], so a test decides *when* it happens rather than racing it.
pub fn plan_waking_cli(dir: &Path) {
    std::fs::create_dir_all(dir).expect("plan dir");
    std::fs::write(dir.join("wake.txt"), "").expect("wake plan");
}

/// Let that turn start: a subagent's forwarded line, then the main thread's own.
pub fn wake_cli(dir: &Path) {
    std::fs::write(dir.join("wake"), "").expect("wake");
}

/// Let that turn end, with the `result` that closes it.
pub fn finish_woken_turn(dir: &Path) {
    std::fs::write(dir.join("woken"), "").expect("woken");
}

/// Make the fake `claude` take this long to say it is listening.
///
/// The window between "the send was accepted" and `system:init`, which a real
/// cold CLI spends a second or two in and nothing can be written into.
pub fn plan_mute_cli(dir: &Path, seconds: f32) {
    std::fs::create_dir_all(dir).expect("plan dir");
    std::fs::write(dir.join("mute.txt"), format!("{seconds}\n")).expect("mute plan");
}

/// Keep either fake CLI's turn open for this long, so a run can be caught
/// mid-flight where nothing is in a position to release it.
pub fn plan_slow_cli(dir: &Path, seconds: f32) {
    std::fs::create_dir_all(dir).expect("plan dir");
    std::fs::write(dir.join("sleep.txt"), format!("{seconds}\n")).expect("sleep plan");
}

/// Make the fake `claude` announce a session id other than the one it was asked
/// to resume — which is what a CLI that answers `--resume` by forking does.
///
/// The id is authoritative: it is the `system:init` line, and the driver follows
/// it rather than inferring a fork from what appeared on disk.
pub fn plan_announced_session(dir: &Path, id: &str) {
    std::fs::create_dir_all(dir).expect("plan dir");
    std::fs::write(dir.join("announce.txt"), format!("{id}\n")).expect("announce plan");
}

/// Make the fake `claude` write a transcript at `target` on startup.
///
/// A session that appears in the project directory while a turn is running: the
/// false positive every "did this resume fork?" heuristic has to survive.
pub fn plan_forked_transcript(dir: &Path, target: &Path, body: &str) {
    std::fs::create_dir_all(dir).expect("plan dir");
    std::fs::write(dir.join("fork.txt"), format!("{}\n", target.display())).expect("fork plan");
    std::fs::write(dir.join("fork.body"), body).expect("fork body");
}

/// The prompts the fake `claude` has been handed, in the order it read them.
///
/// The prompt no longer travels in argv: a resident is given one stream-json
/// `user` message per turn on its stdin, and this reads them back out of the log
/// the fake keeps. Lines that are not user messages — the `control_request` an
/// interrupt sends — are skipped, because they are not what anybody asked for.
pub fn prompts(dir: &Path) -> Vec<String> {
    let Ok(raw) = std::fs::read_to_string(dir.join("stdin.jsonl")) else {
        return vec![];
    };
    raw.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line.get("type").and_then(Value::as_str) == Some("user"))
        .filter_map(|line| line["message"]["content"][0]["text"].as_str().map(str::to_owned))
        .collect()
}

/// Wait until the fake `claude` has read `count` prompts, and return them.
pub async fn wait_for_prompts(dir: &Path, count: usize) -> Vec<String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let seen = prompts(dir);
        if seen.len() >= count {
            return seen;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the CLI in {} was handed {} prompt(s), not {count}: {seen:?}",
            dir.display(),
            seen.len()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Give the fake `claude` a `/usage` reply to print from `dir`.
pub fn plan_usage(dir: &Path, body: &str) {
    std::fs::create_dir_all(dir).expect("usage dir");
    std::fs::write(dir.join("usage.out"), body).expect("usage plan");
}

/// Give the fake `claude` an `auth status --json` reply to print from `dir`.
pub fn plan_auth(dir: &Path, body: &str) {
    std::fs::create_dir_all(dir).expect("auth dir");
    std::fs::write(dir.join("auth.out"), body).expect("auth plan");
}

/// How many times the fake `claude` ran in `dir`.
pub fn call_count(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("calls.txt")).map(|raw| raw.lines().count()).unwrap_or(0)
}

/// Append a line to a file, creating it if need be.
pub fn append_line(path: &Path, line: &str) {
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).expect("append");
    writeln!(file, "{line}").expect("write");
}
