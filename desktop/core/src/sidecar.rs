//! The server process: its environment, its lifecycle, and its pid.
//!
//! # Why the app supervises `serve` itself rather than running `up`
//!
//! The server has a supervisor mode, `remote-hand-server up -- serve`, and it
//! is the right thing for a container or a launchd agent. It is the wrong one
//! here, for one reason that decides it: `up` runs `<data>/bin/remote-hand-server`
//! whenever that file exists (the self-upgrade store), and only falls back to
//! its own binary when it does not. That store is shared by every instance on
//! the device. On a Mac that also has a command-line install, the app would
//! therefore run *that* binary instead of the one it ships; on a Mac without
//! one, it would seed the store with its own copy for others to pick up. An
//! app has to run the server it was built and tested with, and its updates come
//! from the app's own updater (`src-tauri/src/updater.rs`), not from a shared
//! directory.
//!
//! So [`Supervisor`] does the part of `up` the app needs: start `serve`, and
//! when it dies, start it again after a back-off that doubles from one second
//! to thirty and resets after a minute of healthy running. Stopping is
//! `SIGTERM` to that one pid — the server shuts down gracefully on it — then
//! `SIGKILL` if it has not gone after a grace period. Never a process group
//! and never a name: the pty and session shims the server starts live in groups
//! of their own precisely so a server restart does not take them down.
//!
//! The child is started in a process group of its own, so a `^C` in a terminal
//! that launched the app reaches the app (which then stops the server) and not
//! the server directly as well.
//!
//! # A server a crashed app left behind
//!
//! If the app is killed outright its server keeps running, holding the
//! instance lock, and the next launch's server would refuse to start. The pid
//! of every server the app starts is therefore written to `server.pid` with the
//! path of its executable, and [`stop_leftover`] stops it on the next launch —
//! but only if the process with that pid is still running that same executable,
//! so a recycled pid is never signalled.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Environment variables never passed from the app's own environment to the
/// server, by prefix: the server's configuration (`APP_*`, `GOTCHA_*`), its
/// own switches and paths (`RH_*`, `REMOTE_HAND_*`, `DROPPEN_*`), and the
/// variables that mark a shell as running inside a Claude Code session. An app
/// started from a terminal inside another Droppen session would otherwise hand
/// that session's instance, port and data directory to this server.
pub const SCRUBBED_PREFIXES: [&str; 6] = ["APP_", "GOTCHA_", "RH_", "REMOTE_HAND_", "DROPPEN_", "CLAUDE_CODE_"];

/// Exact names scrubbed for the same reason: the generic `PORT`/`HOST` the
/// server also reads, and Claude Code's nesting marker.
pub const SCRUBBED_NAMES: [&str; 3] = ["PORT", "HOST", "CLAUDECODE"];

/// Whether `name` is kept from the app's environment.
pub fn passes_through(name: &str) -> bool {
    !SCRUBBED_NAMES.contains(&name) && !SCRUBBED_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

/// The server's whole environment: the app's, scrubbed ([`passes_through`]),
/// with `PATH` replaced and the instance, host and port set.
pub fn server_env(inherited: impl IntoIterator<Item = (String, String)>, path: &str, port: u16) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = inherited.into_iter().filter(|(name, _)| name != "PATH" && passes_through(name)).collect();
    env.push(("PATH".into(), path.into()));
    env.push(("APP_INSTANCE".into(), crate::INSTANCE.into()));
    env.push(("APP_SERVER__HOST".into(), "127.0.0.1".into()));
    env.push(("APP_SERVER__PORT".into(), port.to_string()));
    // The server is run by the app: no self-upgrade (it would replace the
    // shared `<data>/bin`, not the app), and the web app hides what would be a
    // dead end for a login nobody types (`server-rs/src/desktop_app.rs`).
    // Under the name since the rename to Droppen and the one before it, which
    // a server from before the rename reads instead (`server-rs/src/env.rs`).
    for name in DESKTOP_APP_ENV {
        env.push((name.into(), "1".into()));
    }
    env.sort();
    env
}

/// What tells the server the app runs it, under its name since the rename to
/// Droppen and its old one (`server-rs/src/desktop_app.rs`).
pub const DESKTOP_APP_ENV: [&str; 2] = ["DROPPEN_DESKTOP_APP", "RH_DESKTOP_APP"];

/// What `serve` exits with when another server already holds its instance
/// (`EX_TEMPFAIL`; `server-rs/src/supervisor.rs`, `INSTANCE_HELD_EXIT`).
pub const INSTANCE_HELD_EXIT: i32 = 75;

/// How to start one server process.
#[derive(Debug, Clone)]
pub struct Spec {
    /// The executable.
    pub program: PathBuf,
    /// Its arguments, e.g. `["serve"]`.
    pub args: Vec<String>,
    /// Its complete environment; nothing else is inherited.
    pub env: Vec<(String, String)>,
    /// Its working directory.
    pub cwd: PathBuf,
    /// Where its standard output and error go (appended), or nowhere.
    pub log: Option<PathBuf>,
}

impl Spec {
    /// Start it: own process group, no standard input, output to the log.
    pub fn spawn(&self) -> io::Result<Child> {
        use std::os::unix::process::CommandExt;

        let (stdout, stderr) = match &self.log {
            Some(path) => {
                let file = crate::logging::open_append(path)?;
                (Stdio::from(file.try_clone()?), Stdio::from(file))
            }
            None => (Stdio::null(), Stdio::null()),
        };
        Command::new(&self.program)
            .args(&self.args)
            .env_clear()
            .envs(self.env.iter().map(|(name, value)| (name, value)))
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .process_group(0)
            .spawn()
    }
}

/// The restart back-off: doubles from `min` to `max`, back to `min` after a
/// run of at least `healthy_after`.
#[derive(Debug, Clone)]
pub struct Backoff {
    /// The first wait.
    pub min: Duration,
    /// The longest wait.
    pub max: Duration,
    /// A run this long counts as healthy and resets the wait.
    pub healthy_after: Duration,
    next: Duration,
}

impl Backoff {
    /// A back-off with these bounds.
    pub fn new(min: Duration, max: Duration, healthy_after: Duration) -> Self {
        Self {
            min,
            max,
            healthy_after,
            next: min,
        }
    }

    /// How long to wait after a run that lasted `ran`.
    pub fn after(&mut self, ran: Duration) -> Duration {
        if ran >= self.healthy_after {
            self.next = self.min;
        }
        let wait = self.next;
        self.next = (self.next * 2).min(self.max);
        wait
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new(Duration::from_secs(1), Duration::from_secs(30), Duration::from_secs(60))
    }
}

/// What the supervisor reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A server process started.
    Started {
        /// Its pid.
        pid: u32,
        /// Which start this is, from 0.
        attempt: u32,
    },
    /// It could not be started at all.
    SpawnFailed {
        /// Why.
        error: String,
    },
    /// It exited.
    Exited {
        /// Its pid.
        pid: u32,
        /// Its exit code, if it exited.
        code: Option<i32>,
        /// The signal that ended it, if one did.
        signal: Option<i32>,
        /// Whether the app asked for it.
        requested: bool,
    },
    /// It will be started again after `after`.
    Restarting {
        /// The back-off wait.
        after: Duration,
    },
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

#[derive(Default)]
struct State {
    child: Option<Child>,
    stopping: bool,
}

/// Keeps one server running until [`Supervisor::stop`].
///
/// Every signal it sends goes to the pid of a [`Child`] it has not yet reaped,
/// under the same lock the reaping takes — so a pid it signals is always its
/// own process (alive, or a zombie whose pid cannot have been reused).
pub struct Supervisor {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    done: std::sync::mpsc::Receiver<()>,
}

/// How a stop went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// Nothing was running.
    NotRunning,
    /// It exited within the grace period after `SIGTERM`.
    Gracefully,
    /// It had to be killed.
    Killed,
}

impl Supervisor {
    /// Start supervising. `next_spec` is asked for the spec before every start
    /// (the attempt number is its argument), so a restart can move to a new
    /// port; `on_event` hears everything, from the supervisor's own thread.
    pub fn start<S, E>(mut next_spec: S, mut backoff: Backoff, on_event: E) -> Self
    where
        S: FnMut(u32) -> Spec + Send + 'static,
        E: Fn(Event) + Send + Sync + 'static,
    {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        });
        let (finished, done) = std::sync::mpsc::channel();
        let thread_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("droppen-server-supervisor".into())
            .spawn(move || {
                let shared = thread_shared;
                let mut attempt = 0u32;
                loop {
                    if shared.state.lock().map(|state| state.stopping).unwrap_or(true) {
                        break;
                    }
                    let spec = next_spec(attempt);
                    let started = Instant::now();
                    let pid = {
                        let mut state = match shared.state.lock() {
                            Ok(state) => state,
                            Err(_) => break,
                        };
                        if state.stopping {
                            break;
                        }
                        match spec.spawn() {
                            Ok(child) => {
                                let pid = child.id();
                                state.child = Some(child);
                                Some(pid)
                            }
                            Err(error) => {
                                drop(state);
                                on_event(Event::SpawnFailed { error: error.to_string() });
                                None
                            }
                        }
                    };
                    if let Some(pid) = pid {
                        on_event(Event::Started { pid, attempt });
                        let (status, requested) = wait_for_exit(&shared);
                        on_event(Event::Exited {
                            pid,
                            code: status.and_then(|status| status.code()),
                            signal: status.and_then(|status| std::os::unix::process::ExitStatusExt::signal(&status)),
                            requested,
                        });
                    }
                    attempt = attempt.saturating_add(1);
                    if shared.state.lock().map(|state| state.stopping).unwrap_or(true) {
                        break;
                    }
                    let wait = backoff.after(started.elapsed());
                    on_event(Event::Restarting { after: wait });
                    if sleep_unless_stopped(&shared, wait) {
                        break;
                    }
                }
                let _ = finished.send(());
            })
            .expect("spawn the supervisor thread");
        Self {
            shared,
            thread: Some(thread),
            done,
        }
    }

    /// The running server's pid, if one is running.
    pub fn pid(&self) -> Option<u32> {
        self.shared.state.lock().ok()?.child.as_ref().map(Child::id)
    }

    /// Whether a server process is running right now.
    pub fn is_running(&self) -> bool {
        self.pid().is_some()
    }

    /// Send `SIGTERM` to the running server (if any) without stopping
    /// supervision: it will be restarted after the back-off. For a server that
    /// answers on a port it should not (see the shell's port check).
    pub fn restart(&self) {
        if let Ok(state) = self.shared.state.lock() {
            if let Some(child) = &state.child {
                signal(child.id(), libc::SIGTERM);
            }
        }
    }

    /// Stop supervising and stop the server: `SIGTERM`, up to `grace` for it to
    /// exit, then `SIGKILL`. Returns once it has been reaped.
    pub fn stop(mut self, grace: Duration) -> Stopped {
        let had_child = {
            let mut state = match self.shared.state.lock() {
                Ok(state) => state,
                Err(poisoned) => poisoned.into_inner(),
            };
            state.stopping = true;
            let had = state.child.is_some();
            if let Some(child) = &state.child {
                signal(child.id(), libc::SIGTERM);
            }
            self.shared.wake.notify_all();
            had
        };
        let outcome = if self.done.recv_timeout(grace).is_ok() {
            if had_child {
                Stopped::Gracefully
            } else {
                Stopped::NotRunning
            }
        } else {
            if let Ok(state) = self.shared.state.lock() {
                if let Some(child) = &state.child {
                    signal(child.id(), libc::SIGKILL);
                }
            }
            let _ = self.done.recv_timeout(Duration::from_secs(2));
            Stopped::Killed
        };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        outcome
    }
}

/// Wait for the current child to exit, reaping it under the lock. Returns its
/// status (`None` if waiting failed) and whether a stop had been asked for.
fn wait_for_exit(shared: &Shared) -> (Option<std::process::ExitStatus>, bool) {
    let mut state = match shared.state.lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    };
    loop {
        let Some(child) = state.child.as_mut() else {
            return (None, state.stopping);
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                state.child = None;
                return (Some(status), state.stopping);
            }
            Ok(None) => {}
            Err(_) => {
                state.child = None;
                return (None, state.stopping);
            }
        }
        state = match shared.wake.wait_timeout(state, Duration::from_millis(100)) {
            Ok((state, _)) => state,
            Err(poisoned) => poisoned.into_inner().0,
        };
    }
}

/// Sleep for `wait`, or until a stop. Returns whether a stop was asked for.
fn sleep_unless_stopped(shared: &Shared, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    let mut state = match shared.state.lock() {
        Ok(state) => state,
        Err(_) => return true,
    };
    while !state.stopping {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        state = match shared.wake.wait_timeout(state, left) {
            Ok((state, _)) => state,
            Err(_) => return true,
        };
    }
    true
}

fn signal(pid: u32, signal: libc::c_int) {
    if let Ok(pid) = libc::pid_t::try_from(pid) {
        if pid > 0 {
            // SAFETY: a signal to one positive pid; never 0 or a negative group.
            unsafe {
                libc::kill(pid, signal);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// server.pid
// ---------------------------------------------------------------------------

/// What `server.pid` records about a server the app started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PidRecord {
    /// Its pid.
    pub pid: u32,
    /// The executable it was started from.
    pub exe: PathBuf,
    /// The port it was told to use.
    pub port: u16,
}

impl PidRecord {
    /// Write the record (0600).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let body = serde_json::to_vec(self).map_err(io::Error::other)?;
        crate::state::write_atomically(path, &body, 0o600)
    }

    /// Read the record, if there is a readable one.
    pub fn load(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Remove the record, if it still names `pid`.
    pub fn clear(path: &Path, pid: u32) {
        if Self::load(path).is_some_and(|record| record.pid == pid) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The executable a running process was started from, or `None` if there is
/// no such process (or it cannot be asked).
pub fn exe_of(pid: u32) -> Option<PathBuf> {
    exe_of_impl(pid)
}

#[cfg(target_os = "macos")]
fn exe_of_impl(pid: u32) -> Option<PathBuf> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is as large as the call is told it is.
    let len = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if len <= 0 {
        return None;
    }
    buffer.truncate(len as usize);
    Some(PathBuf::from(<std::ffi::OsString as std::os::unix::ffi::OsStringExt>::from_vec(buffer)))
}

#[cfg(not(target_os = "macos"))]
fn exe_of_impl(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

fn alive(pid: u32) -> bool {
    match libc::pid_t::try_from(pid) {
        // SAFETY: signal 0 only checks that the pid exists and may be signalled.
        Ok(pid) if pid > 0 => unsafe { libc::kill(pid, 0) == 0 },
        _ => false,
    }
}

/// What [`stop_leftover`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Leftover {
    /// No record, or the process is gone.
    None,
    /// The pid is now some other program's: left alone, record removed.
    Recycled,
    /// A server the app had started was still running, and was stopped.
    Stopped {
        /// Its pid.
        pid: u32,
        /// Whether it needed `SIGKILL`.
        killed: bool,
    },
}

/// Stop the server a previous run of the app left behind.
///
/// `server.pid` names a pid; that process is stopped only if it still is a
/// Droppen app's server: its executable is the sidecar ([`crate::SIDECAR_NAME`]) and
/// either lives in an app bundle with the app's bundle id (wherever the app has
/// been moved since) or holds `instance_dir`'s lock under that pid. A pid that
/// has been reused by anything else is left alone.
pub fn stop_leftover(pid_file: &Path, instance_dir: &Path, grace: Duration) -> Leftover {
    let Some(record) = PidRecord::load(pid_file) else {
        let _ = std::fs::remove_file(pid_file);
        return Leftover::None;
    };
    let ours = |pid: u32| {
        exe_of(pid).is_some_and(|exe| {
            let holder = match crate::instance::lock_state(instance_dir) {
                Ok(crate::instance::LockState::Held(holder)) => holder,
                _ => None,
            };
            is_leftover_server(&exe, pid, holder)
        })
    };
    let outcome = if exe_of(record.pid).is_none() {
        Leftover::None
    } else if !ours(record.pid) {
        Leftover::Recycled
    } else {
        signal(record.pid, libc::SIGTERM);
        let deadline = Instant::now() + grace;
        while alive(record.pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let killed = alive(record.pid);
        if killed && ours(record.pid) {
            signal(record.pid, libc::SIGKILL);
            let deadline = Instant::now() + Duration::from_secs(2);
            while alive(record.pid) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        Leftover::Stopped { pid: record.pid, killed }
    };
    let _ = std::fs::remove_file(pid_file);
    outcome
}

/// Whether the process `pid`, running `exe`, is a Droppen app's server: see
/// [`stop_leftover`]. `lock_holder` is the pid the instance's held lock names.
pub fn is_leftover_server(exe: &Path, pid: u32, lock_holder: Option<u32>) -> bool {
    exe.file_name().is_some_and(|name| name == crate::SIDECAR_NAME) && (lock_holder == Some(pid) || in_app_bundle(exe))
}

/// Whether `exe` sits in `<X>.app/Contents/MacOS/` of an app whose
/// `Info.plist` names [`crate::BUNDLE_ID`].
fn in_app_bundle(exe: &Path) -> bool {
    exe.parent()
        .filter(|dir| dir.file_name().is_some_and(|name| name == "MacOS"))
        .and_then(Path::parent)
        .and_then(|contents| std::fs::read_to_string(contents.join("Info.plist")).ok())
        .is_some_and(|plist| plist_bundle_id(&plist).as_deref() == Some(crate::BUNDLE_ID))
}

/// `CFBundleIdentifier` from an XML property list.
pub fn plist_bundle_id(plist: &str) -> Option<String> {
    let after = &plist[plist.find("<key>CFBundleIdentifier</key>")? + "<key>CFBundleIdentifier</key>".len()..];
    let start = after.find("<string>")? + "<string>".len();
    let end = after[start..].find("</string>")?;
    Some(after[start..start + end].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc;

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    fn spec(program: &Path, dir: &Path) -> Spec {
        Spec {
            program: program.to_path_buf(),
            args: vec!["serve".into()],
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            cwd: dir.to_path_buf(),
            log: Some(dir.join("server.log")),
        }
    }

    fn fast() -> Backoff {
        Backoff::new(Duration::from_millis(50), Duration::from_millis(200), Duration::from_secs(60))
    }

    trait GroupZero {
        fn process_group_zero(&mut self) -> &mut Self;
    }

    impl GroupZero for Command {
        fn process_group_zero(&mut self) -> &mut Self {
            std::os::unix::process::CommandExt::process_group(self, 0)
        }
    }

    fn wait_until(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + timeout;
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn the_environment_is_scrubbed_and_pinned_to_this_instance() {
        let inherited = vec![
            ("HOME".to_string(), "/Users/ada".to_string()),
            ("USER".to_string(), "ada".to_string()),
            ("LANG".to_string(), "zh_CN.UTF-8".to_string()),
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("APP_INSTANCE".to_string(), "someone-else".to_string()),
            ("APP_SERVER__PORT".to_string(), "8700".to_string()),
            ("APP_HOME_DIR".to_string(), "/elsewhere".to_string()),
            ("RH_SUPERVISED".to_string(), "1".to_string()),
            ("RH_DESKTOP_APP".to_string(), "0".to_string()),
            ("DROPPEN_DESKTOP_APP".to_string(), "0".to_string()),
            ("RH_NOTIFY_TOKEN".to_string(), "secret".to_string()),
            ("DROPPEN_NOTIFY_TOKEN".to_string(), "secret".to_string()),
            ("REMOTE_HAND_DATA_DIR".to_string(), "/elsewhere".to_string()),
            ("DROPPEN_DATA_DIR".to_string(), "/elsewhere".to_string()),
            ("DROPPEN_SUPERVISED".to_string(), "1".to_string()),
            ("DROPPEN_DESKTOP_KEYCHAIN".to_string(), "0".to_string()),
            ("GOTCHA_ACTIVE_PROFILE".to_string(), "dev".to_string()),
            ("PORT".to_string(), "3000".to_string()),
            ("HOST".to_string(), "0.0.0.0".to_string()),
            ("CLAUDECODE".to_string(), "1".to_string()),
            ("CLAUDE_CODE_ENTRYPOINT".to_string(), "cli".to_string()),
        ];
        let env = server_env(inherited, "/Users/ada/.local/bin:/usr/bin", 51_234);
        let get = |name: &str| env.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str());
        assert_eq!(get("HOME"), Some("/Users/ada"));
        assert_eq!(get("USER"), Some("ada"));
        assert_eq!(get("LANG"), Some("zh_CN.UTF-8"));
        assert_eq!(get("PATH"), Some("/Users/ada/.local/bin:/usr/bin"));
        assert_eq!(get("APP_INSTANCE"), Some(crate::INSTANCE));
        assert_eq!(get("DROPPEN_DESKTOP_APP"), Some("1"));
        assert_eq!(get("RH_DESKTOP_APP"), Some("1"), "and under its old name, for a server from before the rename");
        assert_eq!(get("APP_SERVER__HOST"), Some("127.0.0.1"));
        assert_eq!(get("APP_SERVER__PORT"), Some("51234"));
        for gone in [
            "APP_HOME_DIR",
            "RH_SUPERVISED",
            "DROPPEN_SUPERVISED",
            "RH_NOTIFY_TOKEN",
            "DROPPEN_NOTIFY_TOKEN",
            "REMOTE_HAND_DATA_DIR",
            "DROPPEN_DATA_DIR",
            "DROPPEN_DESKTOP_KEYCHAIN",
            "GOTCHA_ACTIVE_PROFILE",
            "PORT",
            "HOST",
            "CLAUDECODE",
            "CLAUDE_CODE_ENTRYPOINT",
        ] {
            assert_eq!(get(gone), None, "{gone} leaked through");
        }
        assert_eq!(env.iter().filter(|(key, _)| key == "PATH").count(), 1);
        assert_eq!(env.iter().filter(|(key, _)| key == "APP_INSTANCE").count(), 1);
        for name in DESKTOP_APP_ENV {
            assert_eq!(
                env.iter().filter(|(key, _)| key == name).count(),
                1,
                "{name}: the app's own value, not the inherited one"
            );
        }
    }

    #[test]
    fn the_backoff_doubles_to_a_ceiling_and_resets_after_a_healthy_run() {
        let mut backoff = Backoff::new(Duration::from_secs(1), Duration::from_secs(30), Duration::from_secs(60));
        let quick = Duration::from_secs(1);
        let waits: Vec<u64> = (0..7).map(|_| backoff.after(quick).as_secs()).collect();
        assert_eq!(waits, vec![1, 2, 4, 8, 16, 30, 30]);
        assert_eq!(backoff.after(Duration::from_secs(61)), Duration::from_secs(1));
        assert_eq!(backoff.after(quick), Duration::from_secs(2));
    }

    #[test]
    fn the_child_gets_exactly_the_environment_it_was_given() {
        let dir = tempfile::tempdir().expect("tempdir");
        let program = script(dir.path(), "server", "env > \"$PWD/env.txt\"");
        let mut spec = spec(&program, dir.path());
        spec.env.push(("APP_INSTANCE".into(), "desktop".into()));
        let mut child = spec.spawn().expect("spawn");
        assert!(child.wait().expect("wait").success());
        let env = std::fs::read_to_string(dir.path().join("env.txt")).expect("env");
        assert!(env.contains("APP_INSTANCE=desktop"));
        assert!(!env.contains("HOME="), "nothing is inherited: {env}");
    }

    #[test]
    fn a_crashed_server_is_restarted_with_backoff_and_a_stop_is_graceful() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Crashes on its first two starts, then stays up until SIGTERM, which
        // it answers by exiting cleanly — as the real server does.
        let program = script(
            dir.path(),
            "server",
            "n=$(cat starts 2>/dev/null || echo 0); n=$((n+1)); echo $n > starts\n\
             if [ $n -le 2 ]; then exit 3; fi\n\
             trap 'echo stopped > stopped; exit 0' TERM\n\
             touch ready\n\
             while :; do sleep 0.05; done",
        );
        let (send, events) = mpsc::channel();
        let dir_path = dir.path().to_path_buf();
        let supervisor = Supervisor::start(
            move |_| spec(&program, &dir_path),
            fast(),
            move |event| {
                let _ = send.send(event);
            },
        );
        wait_until("the third start", Duration::from_secs(10), || {
            std::fs::read_to_string(dir.path().join("starts"))
                .map(|s| s.trim() == "3")
                .unwrap_or(false)
        });
        wait_until("the server to be ready", Duration::from_secs(5), || dir.path().join("ready").exists());
        let pid = supervisor.pid().expect("pid");
        assert!(alive(pid));
        assert_eq!(supervisor.stop(Duration::from_secs(5)), Stopped::Gracefully);
        assert!(!alive(pid), "the server is gone after stop");
        assert_eq!(std::fs::read_to_string(dir.path().join("stopped")).expect("trap ran").trim(), "stopped");

        let events: Vec<Event> = events.try_iter().collect();
        let crashes = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    Event::Exited {
                        code: Some(3),
                        requested: false,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(crashes, 2, "{events:?}");
        let restarts: Vec<Duration> = events
            .iter()
            .filter_map(|event| match event {
                Event::Restarting { after } => Some(*after),
                _ => None,
            })
            .collect();
        assert_eq!(restarts, vec![Duration::from_millis(50), Duration::from_millis(100)]);
        assert!(
            matches!(
                events.last(),
                Some(Event::Exited {
                    requested: true,
                    code: Some(0),
                    ..
                })
            ),
            "{events:?}"
        );
    }

    #[test]
    fn a_server_that_ignores_sigterm_is_killed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let program = script(dir.path(), "server", "trap '' TERM\ntouch ready\nwhile :; do sleep 0.05; done");
        let dir_path = dir.path().to_path_buf();
        let supervisor = Supervisor::start(move |_| spec(&program, &dir_path), fast(), |_| {});
        wait_until("the server to ignore SIGTERM", Duration::from_secs(5), || {
            dir.path().join("ready").exists()
        });
        let pid = supervisor.pid().expect("pid");
        assert_eq!(supervisor.stop(Duration::from_millis(300)), Stopped::Killed);
        assert!(!alive(pid));
    }

    #[test]
    fn stopping_during_the_backoff_does_not_start_another() {
        let dir = tempfile::tempdir().expect("tempdir");
        let program = script(dir.path(), "server", "echo x >> starts; exit 1");
        let dir_path = dir.path().to_path_buf();
        let slow = Backoff::new(Duration::from_secs(30), Duration::from_secs(30), Duration::from_secs(60));
        let supervisor = Supervisor::start(move |_| spec(&program, &dir_path), slow, |_| {});
        wait_until("the first start", Duration::from_secs(5), || dir.path().join("starts").exists());
        let started = Instant::now();
        // Either the one start was still being reaped or it was already gone.
        let stopped = supervisor.stop(Duration::from_secs(5));
        assert!(matches!(stopped, Stopped::NotRunning | Stopped::Gracefully), "{stopped:?}");
        assert!(started.elapsed() < Duration::from_secs(5), "the stop cut the 30s back-off short");
        let starts = std::fs::read_to_string(dir.path().join("starts")).expect("starts");
        assert_eq!(starts.lines().count(), 1);
    }

    #[test]
    fn each_start_asks_for_a_fresh_spec() {
        let dir = tempfile::tempdir().expect("tempdir");
        let program = script(dir.path(), "server", "echo \"$1\" >> args; exit 1");
        let dir_path = dir.path().to_path_buf();
        let supervisor = Supervisor::start(
            move |attempt| {
                let mut spec = spec(&program, &dir_path);
                spec.args = vec![format!("attempt-{attempt}")];
                spec
            },
            fast(),
            |_| {},
        );
        wait_until("three starts", Duration::from_secs(10), || {
            std::fs::read_to_string(dir.path().join("args"))
                .map(|s| s.lines().count() >= 3)
                .unwrap_or(false)
        });
        supervisor.stop(Duration::from_secs(5));
        let args = std::fs::read_to_string(dir.path().join("args")).expect("args");
        assert!(args.starts_with("attempt-0\nattempt-1\nattempt-2\n"), "{args}");
    }

    #[test]
    fn a_missing_program_is_reported_and_retried_not_fatal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (send, events) = mpsc::channel();
        let dir_path = dir.path().to_path_buf();
        let supervisor = Supervisor::start(
            move |_| spec(&dir_path.join("missing"), &dir_path),
            fast(),
            move |event| {
                let _ = send.send(event);
            },
        );
        let first = events.recv_timeout(Duration::from_secs(5)).expect("an event");
        assert!(matches!(first, Event::SpawnFailed { .. }), "{first:?}");
        supervisor.stop(Duration::from_secs(5));
    }

    #[test]
    fn the_server_output_goes_to_the_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let program = script(dir.path(), "server", "echo out; echo err >&2");
        let mut child = spec(&program, dir.path()).spawn().expect("spawn");
        child.wait().expect("wait");
        let log = std::fs::read_to_string(dir.path().join("server.log")).expect("log");
        assert!(log.contains("out") && log.contains("err"), "{log}");
    }

    #[test]
    fn a_leftover_server_is_stopped_by_pid_only_if_it_is_still_ours() {
        use std::os::fd::AsRawFd;

        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("server.pid");
        let instance = dir.path().join("instance");
        std::fs::create_dir_all(&instance).expect("instance dir");
        // No record: nothing to do.
        assert_eq!(stop_leftover(&pid_file, &instance, Duration::from_secs(1)), Leftover::None);

        // A server from a "previous run" that is still going: a copy of this
        // test binary named like the server, running the sleeping stand-in
        // below. (A copy of a system binary would not do: macOS kills a
        // relocated platform binary on sight.)
        let program = dir.path().join("remote-hand-server");
        std::fs::copy(std::env::current_exe().expect("test binary"), &program).expect("copy");
        let mut leftover = Command::new(&program)
            .args(["--ignored", "--exact", "sidecar::tests::stand_in_server", "--test-threads=1", "-q"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group_zero()
            .spawn()
            .expect("spawn");
        let pid = leftover.id();
        let canonical = std::fs::canonicalize(&program).expect("canonical");
        wait_until("the stand-in to run", Duration::from_secs(10), || {
            exe_of(pid).and_then(|exe| std::fs::canonicalize(exe).ok()) == Some(canonical.clone())
        });
        let record = PidRecord {
            pid,
            exe: PathBuf::from("/Applications/Somewhere/Else.app/Contents/MacOS/remote-hand-server"),
            port: 50_000,
        };
        record.save(&pid_file).expect("record");

        // Not in an app bundle and not holding the instance: not provably ours.
        assert_eq!(stop_leftover(&pid_file, &instance, Duration::from_secs(1)), Leftover::Recycled);
        assert!(alive(pid), "left alone");

        // Holding the instance's lock under its pid (the test holds the flock
        // for it, as the server would): ours, wherever it was started from.
        let lock = std::fs::File::create(instance.join("lock")).expect("lock");
        // SAFETY: flock on a descriptor the test owns.
        assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0);
        std::fs::write(instance.join("lock"), format!("{pid}\n")).expect("pid");
        record.save(&pid_file).expect("record");
        let reaper = std::thread::spawn(move || leftover.wait());
        assert_eq!(
            stop_leftover(&pid_file, &instance, Duration::from_secs(5)),
            Leftover::Stopped { pid, killed: false }
        );
        let status = reaper.join().expect("join").expect("wait");
        assert_eq!(std::os::unix::process::ExitStatusExt::signal(&status), Some(libc::SIGTERM));
        assert!(!pid_file.exists());
        drop(lock);

        // A record whose pid now runs something else is left alone.
        let mut other = Command::new("/bin/sleep").arg("5").spawn().expect("sleep");
        PidRecord {
            pid: other.id(),
            exe: program.clone(),
            port: 50_000,
        }
        .save(&pid_file)
        .expect("record");
        assert_eq!(stop_leftover(&pid_file, &instance, Duration::from_secs(1)), Leftover::Recycled);
        assert!(alive(other.id()), "an unrelated process was not signalled");
        other.kill().expect("kill our own sleep");
        other.wait().expect("reap");
        assert!(!pid_file.exists());
    }

    /// The stand-in leftover server: just runs until it is stopped.
    #[test]
    #[ignore = "run by a_leftover_server_is_stopped_by_pid_only_if_it_is_still_ours, as a child"]
    fn stand_in_server() {
        std::thread::sleep(Duration::from_secs(30));
    }

    #[test]
    fn an_app_bundle_is_recognised_by_its_bundle_id() {
        let root = tempfile::tempdir().expect("tempdir");
        let bundle = |name: &str, id: &str| {
            let contents = root.path().join(format!("{name}.app/Contents"));
            std::fs::create_dir_all(contents.join("MacOS")).expect("dirs");
            std::fs::write(
                contents.join("Info.plist"),
                format!("<?xml version=\"1.0\"?>\n<plist><dict>\n<key>CFBundleName</key>\n<string>{name}</string>\n<key>CFBundleIdentifier</key>\n  <string>{id}</string>\n</dict></plist>"),
            )
            .expect("plist");
            contents.join("MacOS/remote-hand-server")
        };
        let moved = bundle("Droppen copy", crate::BUNDLE_ID);
        let other = bundle("Other", "com.example.other");
        assert!(is_leftover_server(&moved, 10, None), "ours wherever the app was moved");
        assert!(!is_leftover_server(&other, 10, None), "another app's server");
        // The release and an end-to-end test build never take each other's
        // server for their own, should a recorded pid be reused by it.
        let other_build = if cfg!(feature = "e2e") {
            crate::identity::RELEASE.bundle_id
        } else {
            crate::identity::E2E.bundle_id
        };
        assert!(!is_leftover_server(&bundle("Droppen other build", other_build), 10, None));
        assert!(is_leftover_server(&other, 10, Some(10)), "but holding our instance under that pid, ours");
        assert!(
            !is_leftover_server(&moved.with_file_name("Droppen"), 10, Some(10)),
            "only the server binary"
        );
        assert!(!is_leftover_server(Path::new("/bin/sleep"), 10, Some(10)));
        assert_eq!(
            plist_bundle_id("<key>CFBundleIdentifier</key><string>a.b</string>").as_deref(),
            Some("a.b")
        );
        assert_eq!(plist_bundle_id("<key>CFBundleName</key><string>a.b</string>"), None);
    }

    #[test]
    fn a_pid_record_is_cleared_only_for_its_own_pid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("server.pid");
        let record = PidRecord {
            pid: 4242,
            exe: PathBuf::from("/Applications/Droppen.app/Contents/MacOS/remote-hand-server"),
            port: 51_000,
        };
        record.save(&path).expect("save");
        assert_eq!(PidRecord::load(&path), Some(record));
        PidRecord::clear(&path, 1);
        assert!(path.exists());
        PidRecord::clear(&path, 4242);
        assert!(!path.exists());
    }
}
