//! Installs, updates and sign-ins, run where the user can watch them.
//!
//! Each task is a whitelisted command ([`super::commands`]) in an ordinary
//! server terminal ([`crate::terminals`]): the guide shows it in its embedded
//! xterm, the user can type into it (a password prompt, the code a CLI asks
//! for), and because terminals live in a detached shim, an install keeps going
//! when the window closes and is there to reattach to when it opens again.
//! What this module adds around the terminal is bookkeeping:
//!
//! * a bounded tail of the output (`on_data`), so a failure can be shown in
//!   plain words — no network, no permission, no disk — with the last lines
//!   underneath, secrets redacted;
//! * the exit (`on_exit`): its code, the record persisted in this instance's
//!   `kv` table (`setup` / `tasks`), and a fresh detection — with a re-read of
//!   the login shell when the tool is *still* not found, which is what finds a
//!   CLI an installer put somewhere only the shell knows about.
//!
//! # Failed
//!
//! A task failed when it exited non-zero, and also when an install exited 0
//! and the tool is still nowhere to be found (an update: still too old) — an
//! installer can end well without having installed anything. Which of the two
//! is only known once detection has looked again, so a task that exited 0
//! reads as running until then; a failed one keeps its tail and the cause.
//!
//! # Two environments
//!
//! An install runs as `$SHELL -l -c '<command>'` with the user's *login shell*
//! environment — its `PATH` and its proxies, as read by [`super::shell`] — and
//! not with the `PATH` Droppen gives its children. Two reasons, both found on
//! real machines: Codex's installer decides whether to add `~/.local/bin` to
//! `~/.zprofile` by looking at `PATH`, and Droppen's always has it, so the
//! user's own terminal would never find codex; and a company proxy set in
//! `.zshrc` is invisible to an app launchd started.
//!
//! A sign-in is the opposite: it runs the very binary the guide found, with
//! the children's environment, because the login being made is the one
//! Droppen's sessions will use. The CLIs open the system browser themselves;
//! nothing here stops them (the setup-token wizard's no-op openers are for a
//! browser on another device, which this is not).
//!
//! # The API key
//!
//! `codex login --with-api-key` reads the key from standard input. It is
//! written there and nowhere else: not on the command line (`ps` shows argv to
//! every user on the machine), not in a log, not in the task's tail, and the
//! request body that carried it is never echoed.
//!
//! It has no terminal: it runs to its end in a task of its own, which the
//! request only waits for, so a client that goes away mid sign-in neither
//! kills the CLI nor leaves the tool's task running for ever (every later one
//! would be `409 busy` until a restart).
//!
//! # Adopting a directory
//!
//! A directory the user points the guide at joins every child's `PATH`, now
//! and after every restart, so it has to stay the user's ([`trusted_folder`]):
//! it is kept as its real path, links resolved, and it, every folder above it
//! up to `/`, and the CLI in it must belong to this user or root and be
//! writable by nobody else — when adopted, and again each time it is put on
//! the `PATH`. A CLI that is a link is followed one link at a time, and every
//! link on the way, and the folder holding it, must pass the same test: a link
//! that somebody else can re-point is somebody else's CLI.
//!
//! One allowance: on a Mac the `admin` group may write (Homebrew's folders on
//! an Intel Mac are `775 root:admin`), since its members can become root
//! anyway. None for a sticky folder open to all, like `/tmp`: what is in one
//! can be cleaned away and made again by anybody, between two checks, and no
//! real install lives there.
//!
//! A real path with a `:` in it is refused outright: it would split into two
//! entries of the `PATH` (macOS shows a folder named "Tools 24/25" with a `:`
//! on disk), the second one relative and so resolved in each session's own
//! working directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::commands::{self, CommandTool, Entry};
use super::detect::Detection;
use super::runner::Invocation;
use super::shell::{self, ShellSnapshot};
use super::{Setup, SetupTask, TaskFailure, TaskKind, ToolId, ToolLogin};
use crate::api::error::ApiError;
use crate::api::AppState;
use crate::db::scope;
use crate::openapi_ext::Nullable;
use crate::process_env::{self, Env};
use crate::terminals::{CreateSpec, Registry};

/// The `kv` key, under [`scope::SETUP`].
const TASKS_KEY: &str = "tasks";
/// Every task terminal's size: fixed, like the setup-token wizard's, so no
/// attached pane can narrow it and wrap what the installer prints.
pub const TASK_SIZE: (u16, u16) = (100, 30);
/// How many lines of a failed task's output are kept.
pub const TAIL_LINES: usize = 20;
/// How much output the tail buffer holds while a task runs.
const TAIL_BYTES: usize = 32 * 1024;
/// How long `codex login --with-api-key` may take.
const API_KEY_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a sign-in poll's answer is reused.
const LOGIN_REUSE: Duration = Duration::from_millis(1500);
/// The exit code a terminal reports when it was stopped rather than exited.
pub const STOPPED: i32 = -1;

/// One task, as it is kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TaskRecord {
    /// Which run this is, so a late exit hook cannot finish a newer task.
    pub id: String,
    pub tool: ToolId,
    pub kind: TaskKind,
    pub method: String,
    pub command: String,
    pub terminal_id: Option<String>,
    pub started_at: String,
    /// Exited, and what that means is settled (module docs, "Failed").
    pub finished: bool,
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub failed: bool,
    pub failure: Option<TaskFailure>,
    /// The last lines, kept from the exit until the outcome is settled, and
    /// after that only when it failed.
    pub tail: Vec<String>,
}

impl TaskRecord {
    fn new(tool: ToolId, entry: &Entry) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            tool,
            kind: entry.kind,
            method: entry.method.to_string(),
            command: entry.command.to_string(),
            terminal_id: None,
            started_at: crate::sessions::iso8601(std::time::SystemTime::now()),
            finished: false,
            exit_code: None,
            failed: false,
            failure: None,
            tail: Vec::new(),
        }
    }

    /// Exited, but not yet settled: detection is still looking.
    fn settling(&self) -> bool {
        !self.finished && self.exit_code.is_some()
    }

    fn view(&self) -> SetupTask {
        SetupTask {
            tool: self.tool,
            kind: self.kind,
            method: self.method.clone(),
            command: self.command.clone(),
            terminal_id: Nullable(self.terminal_id.clone()),
            started_at: self.started_at.clone(),
            running: !self.finished,
            exit_code: Nullable(self.exit_code),
            failed: self.finished && self.failed,
            failure: Nullable(self.failure),
            tail: if self.finished && self.failed { self.tail.clone() } else { Vec::new() },
        }
    }
}

/// The task state a [`Setup`] keeps: loaded from the `kv` table on first use.
#[derive(Debug, Default)]
pub(crate) struct Tasks {
    loaded: bool,
    records: HashMap<ToolId, TaskRecord>,
}

/// The last answers to `GET /api/setup/tools/{tool}/login`.
pub(crate) type LoginCache = HashMap<ToolId, (Instant, ToolLogin)>;

// ---------------------------------------------------------------- the output

/// A task's recent output, bounded.
#[derive(Debug, Default)]
pub struct Tail {
    text: String,
}

impl Tail {
    /// Add a chunk, dropping the oldest output beyond the cap.
    pub fn push(&mut self, chunk: &str) {
        self.text.push_str(chunk);
        if self.text.len() > TAIL_BYTES {
            let mut cut = self.text.len() - TAIL_BYTES;
            while !self.text.is_char_boundary(cut) {
                cut += 1;
            }
            self.text.drain(..cut);
        }
    }

    /// The last [`TAIL_LINES`] lines as a terminal would show them: escapes
    /// gone, and of a line a progress bar redrew with `\r`, the last drawing.
    pub fn lines(&self) -> Vec<String> {
        last_lines(&self.text)
    }
}

/// The last [`TAIL_LINES`] non-empty lines of `text`, as displayed.
pub fn last_lines(text: &str) -> Vec<String> {
    let clean = crate::text::strip_ansi(text);
    let lines: Vec<String> = clean
        .split('\n')
        .filter_map(|line| line.split('\r').rev().find(|part| !part.trim().is_empty()))
        .map(|line| line.trim_end().to_string())
        .collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].to_vec()
}

/// Why a task failed, in plain words, when its last lines say so: the most
/// recent line that names a cause wins.
pub fn classify(lines: &[String]) -> Option<TaskFailure> {
    lines.iter().rev().find_map(|line| {
        let lower = line.to_ascii_lowercase();
        if lower.contains("could not resolve host") || lower.contains("enotfound") || lower.contains("getaddrinfo") {
            Some(TaskFailure::Network)
        } else if lower.contains("permission denied") || lower.contains("eacces") {
            Some(TaskFailure::Permission)
        } else if lower.contains("no space left") || lower.contains("enospc") {
            Some(TaskFailure::Disk)
        } else {
            None
        }
    })
}

/// Every line through both redactions: the credentials this server stores,
/// by value, then anything shaped like a key.
fn redact(lines: Vec<String>, accounts: &crate::accounts::store::AccountsStore) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| process_env::redact_secrets(&accounts.redact_known_secrets(&line)))
        .collect()
}

// --------------------------------------------------------------- the specs

/// The environment an install runs with: the children's, minus the `PATH` and
/// the proxies, which come from the login shell's own read instead — or, with
/// no read to go by, launchd's bare `PATH`, from which `$SHELL -l` builds the
/// user's own.
pub fn install_env(clean: &Env, read: Option<&ShellSnapshot>) -> Env {
    let mut env = clean.clone();
    for name in shell::PROXY_VARIABLES {
        env.remove(name);
    }
    let path = read
        .filter(|read| read.status == super::PathRead::Ok)
        .and_then(|read| read.path.clone())
        .unwrap_or_else(|| shell::MINIMAL_PATH.to_string());
    env.insert("PATH".to_string(), path);
    for (name, value) in read.map(|read| read.proxies.as_slice()).unwrap_or_default() {
        env.insert(name.clone(), value.clone());
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    env
}

/// How the user's shell is asked to run one line as a login shell: `-l -c`,
/// except csh and tcsh, which take `-l` only as their sole flag and so get
/// `-c` alone (they still read `.cshrc`). The line itself is plain words in
/// every shell ([`super::commands`]).
pub fn shell_args(shell: &str, line: &str) -> Vec<String> {
    let name = Path::new(shell).file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let login = !matches!(name, "csh" | "tcsh");
    let mut args = Vec::with_capacity(3);
    if login {
        args.push("-l".to_string());
    }
    args.push("-c".to_string());
    args.push(line.to_string());
    args
}

/// The terminal an install or update runs in: `$SHELL -l -c '<command>'`.
pub fn install_spec(shell: &str, command: &str, home: &Path, env: Env, title: String) -> CreateSpec {
    let mut spec = CreateSpec::new(shell, shell_args(shell, command));
    spec.env = Some(env);
    spec.cwd = Some(home.to_string_lossy().into_owned());
    spec.title = Some(title);
    spec.size = Some(TASK_SIZE);
    spec
}

/// The terminal a sign-in runs in: the CLI the guide found, run directly.
pub fn login_spec(program: &Path, args: Vec<String>, home: &Path, env: Env, title: String) -> CreateSpec {
    let mut spec = CreateSpec::new(program.to_string_lossy(), args);
    spec.env = Some(env);
    spec.cwd = Some(home.to_string_lossy().into_owned());
    spec.title = Some(title);
    spec.size = Some(TASK_SIZE);
    spec
}

/// "Install Claude Code", "Sign in to Codex".
fn title(tool: ToolId, kind: TaskKind) -> String {
    let name = match tool {
        ToolId::Claude => "Claude Code",
        ToolId::Codex => "Codex",
    };
    match kind {
        TaskKind::Install => format!("Install {name}"),
        TaskKind::Update => format!("Update {name}"),
        TaskKind::Login => format!("Sign in to {name}"),
    }
}

// --------------------------------------------------------------- the errors

fn busy() -> ApiError {
    ApiError::new(gotcha::axum::http::StatusCode::CONFLICT, "busy")
}

/// `400 {"error":"unavailable","reason":…}`.
pub fn unavailable(reason: &str) -> ApiError {
    ApiError::with_body(
        gotcha::axum::http::StatusCode::BAD_REQUEST,
        serde_json::json!({ "error": "unavailable", "reason": reason }),
    )
}

// ---------------------------------------------------------------- the state

impl Setup {
    fn tasks(&self) -> std::sync::MutexGuard<'_, Tasks> {
        let mut tasks = self.tasks.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !tasks.loaded {
            tasks.loaded = true;
            tasks.records = self
                .db()
                .kv_get(scope::SETUP, TASKS_KEY)
                .and_then(|raw| serde_json::from_str::<HashMap<ToolId, TaskRecord>>(&raw).ok())
                .unwrap_or_default();
            // A sign-in by API key runs inside this process, and a task that
            // exited is settled by it; either left unfinished on disk is one
            // whose server stopped before it was done. Nothing will finish
            // them now, so they are over, outcome unknown.
            for record in tasks.records.values_mut() {
                if !record.finished && (record.terminal_id.is_none() || record.exit_code.is_some()) {
                    record.finished = true;
                }
            }
        }
        tasks
    }

    fn persist_tasks(&self) {
        let raw = serde_json::to_string(&self.tasks().records).unwrap_or_else(|_| "{}".to_string());
        self.db().kv_set(scope::SETUP, TASKS_KEY, &raw);
    }

    /// The task for `tool` as the report shows it: running while its terminal
    /// lives (and, once it exited, until detection has settled what that
    /// means), and finished with no exit code when the terminal is gone
    /// without this server having seen it go (a restart in between).
    pub fn task_view(&self, tool: ToolId, terminals: &Registry) -> Option<SetupTask> {
        let live: Vec<String> = terminals.list().into_iter().map(|info| info.id).collect();
        let mut tasks = self.tasks();
        let record = tasks.records.get_mut(&tool)?;
        let gone = !record.finished && !record.settling() && record.terminal_id.as_ref().is_some_and(|id| !live.contains(id));
        if gone {
            record.finished = true;
        }
        let view = record.view();
        drop(tasks);
        if gone {
            self.persist_tasks();
        }
        Some(view)
    }

    /// Claim `tool` for a new task, unless one is still running. One that has
    /// exited and is only being settled no longer holds it.
    fn begin(&self, record: TaskRecord, terminals: &Registry) -> Result<Option<TaskRecord>, ApiError> {
        let live: Vec<String> = terminals.list().into_iter().map(|info| info.id).collect();
        let mut tasks = self.tasks();
        if let Some(current) = tasks.records.get(&record.tool) {
            let running = !current.finished && !current.settling() && current.terminal_id.as_ref().is_none_or(|id| live.contains(id));
            if running {
                return Err(busy());
            }
        }
        Ok(tasks.records.insert(record.tool, record))
    }

    /// Put back what `begin` replaced, when the task never started.
    fn abandon(&self, tool: ToolId, id: &str, previous: Option<TaskRecord>) {
        let mut tasks = self.tasks();
        if tasks.records.get(&tool).is_some_and(|record| record.id == id) {
            match previous {
                Some(previous) => tasks.records.insert(tool, previous),
                None => tasks.records.remove(&tool),
            };
        }
    }

    fn set_terminal(&self, tool: ToolId, id: &str, terminal_id: String) {
        if let Some(record) = self.tasks().records.get_mut(&tool).filter(|record| record.id == id) {
            record.terminal_id = Some(terminal_id);
        }
        self.persist_tasks();
    }

    /// Record how task `id` ended. Synchronous, so a report asked the moment
    /// after already says so; the slow half is [`Setup::after_exit`]. A
    /// non-zero exit (or none) is a failure at once; an exit of 0 is settled
    /// by `after_exit`, once detection has looked.
    pub(crate) fn record_exit(&self, tool: ToolId, id: &str, exit_code: Option<i32>, lines: Vec<String>) {
        {
            let mut tasks = self.tasks();
            let Some(record) = tasks
                .records
                .get_mut(&tool)
                .filter(|record| record.id == id && !record.finished && !record.settling())
            else {
                return;
            };
            record.exit_code = exit_code;
            record.tail = lines;
            if exit_code != Some(0) {
                record.finished = true;
                record.failed = true;
                record.failure = classify(&record.tail);
            }
        }
        self.persist_tasks();
        self.forget_detection();
        self.logins.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&tool);
    }

    /// After task `id`: look again, and when the tool is still nowhere to be
    /// found, read the login shell again and adopt its `PATH` — the installer
    /// may have put it somewhere only the shell knows about. Then settle a task
    /// that exited 0: an install whose tool is still not found, or an update
    /// whose tool is still too old, failed after all.
    pub(crate) async fn after_exit(&self, tool: ToolId, id: &str) {
        let mut detection = self.detection(true).await;
        let found = |detection: &Detection| detection.tools.iter().any(|probe| probe.id == tool && probe.found.is_some());
        if !found(&detection) {
            self.read_shell(false, true).await;
            detection = self.detection(true).await;
        }
        let probe = detection.tools.iter().find(|probe| probe.id == tool);
        {
            let mut tasks = self.tasks();
            let Some(record) = tasks.records.get_mut(&tool).filter(|record| record.id == id && record.settling()) else {
                return;
            };
            let did_its_job = match record.kind {
                TaskKind::Install => probe.is_some_and(|probe| probe.found.is_some()),
                TaskKind::Update => probe.is_some_and(|probe| probe.supported),
                // Whether a sign-in worked is the tool's own status to say.
                TaskKind::Login => true,
            };
            record.finished = true;
            if did_its_job {
                record.tail.clear();
            } else {
                record.failed = true;
                record.failure = classify(&record.tail);
            }
        }
        self.persist_tasks();
    }

    /// One cheap answer to "is it signed in yet?" for a sign-in in progress:
    /// `claude auth status` / `codex login status` only, reused for 1.5 s.
    pub async fn poll_login(&self, tool: ToolId) -> ToolLogin {
        if let Some((at, login)) = self.logins.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(&tool) {
            if at.elapsed() < LOGIN_REUSE {
                return login.clone();
            }
        }
        let probe = (self.probe)(&self.paths, self.overlay());
        let login = match super::locate::locate(tool, &probe.search()) {
            None => super::detect::unknown_login(),
            Some(found) => {
                let path = PathBuf::from(found.path);
                match tool {
                    ToolId::Claude => super::detect::login(&self.runner, &probe, tool, &path).await,
                    ToolId::Codex => super::detect::codex_status(&self.runner, &probe, &path).await,
                }
            }
        };
        self.logins
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(tool, (Instant::now(), login.clone()));
        login
    }

    /// Adopt the directory of a CLI the user picked (or a fallback the guide
    /// found): it must be an executable regular file named for the tool, in a
    /// folder that is [`trusted_folder`]. The folder is kept as its real path.
    pub fn adopt(&self, tool: ToolId, path: &str) -> Result<PathBuf, ApiError> {
        let path = Path::new(path);
        let named = path.file_name().is_some_and(|name| name == tool.binary());
        if !path.is_absolute() || !named || resolve_executable(path).is_none() {
            return Err(ApiError::with_body(
                gotcha::axum::http::StatusCode::BAD_REQUEST,
                serde_json::json!({ "error": "not_the_tool", "hint": format!("an executable file named {}", tool.binary()) }),
            ));
        }
        // The folder that goes on the PATH is the real one, links in it
        // resolved (each checked on the way); the CLI keeps its name in it (it
        // may be a link itself, followed and checked the same way). Then the
        // spelling the disk has: on a Mac's case-insensitive disk `/users/ada`
        // finds the same folders as `/Users/ada`, and it is the latter that is
        // kept (and checked again later).
        let dir = path
            .parent()
            .and_then(resolve_trusted)
            .and_then(|dir| std::fs::canonicalize(dir).ok());
        if dir.as_deref().is_some_and(|dir| dir.to_string_lossy().contains(':')) {
            return Err(ApiError::with_body(
                gotcha::axum::http::StatusCode::BAD_REQUEST,
                serde_json::json!({
                    "error": "colon_in_path",
                    "hint": "a folder whose path has a ':' cannot go on a PATH; move the CLI or link it from another folder",
                }),
            ));
        }
        let others_can_change_it = !dir.as_deref().is_some_and(trusted_folder);
        if others_can_change_it {
            return Err(ApiError::with_body(
                gotcha::axum::http::StatusCode::BAD_REQUEST,
                serde_json::json!({
                    "error": "not_yours",
                    "hint": "the file and its folder must belong to you and be writable by nobody else",
                }),
            ));
        }
        let dir = dir.unwrap_or_default();
        let mut overlay = self.overlay();
        if !overlay.contains(&dir) {
            overlay.push(dir.clone());
            let raw: Vec<String> = overlay.iter().map(|dir| dir.to_string_lossy().into_owned()).collect();
            self.db()
                .kv_set(scope::SETUP, super::OVERLAY_KEY, &serde_json::to_string(&raw).unwrap_or_default());
        }
        process_env::set_child_path(Some(super::locate::with_overlay(&process_env::child_path(), &overlay)));
        self.forget_detection();
        Ok(dir)
    }
}

/// The macOS `admin` group (gid 80). Its members can become root anyway, and
/// Homebrew's folders on an Intel Mac are writable by it.
#[cfg(target_os = "macos")]
const ADMIN_GID: Option<u32> = Some(80);
#[cfg(not(target_os = "macos"))]
const ADMIN_GID: Option<u32> = None;

/// Whether only this server's user (or root) can change the entry at `path`
/// itself, read as it is on disk (a link is not followed): owned by one of
/// them, writable by no group but `admin` (macOS) and by nobody else. A
/// link's own mode means nothing; only its owner counts.
fn owned_safely(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    // SAFETY: `geteuid` reads an attribute of the calling process, takes no
    // arguments and is documented as always succeeding.
    let me = unsafe { libc::geteuid() };
    let owner = meta.uid() == me || meta.uid() == 0;
    if meta.file_type().is_symlink() {
        return owner;
    }
    let mode = meta.mode();
    let group = mode & 0o020 == 0 || ADMIN_GID == Some(meta.gid());
    let others = mode & 0o002 == 0;
    owner && group && others
}

// The unit tests work in scratch folders whose parents are the machine's
// (a container's `/work` may be open to all); a test that checks trust
// names its scratch root here, and the walk stops there. Thread-local, like
// the child `PATH` override, so it is that test's alone.
#[cfg(test)]
thread_local! {
    pub(crate) static TRUST_ROOT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Whether only this user (or root) can change `path`, or any folder leading
/// to it: `path` and every folder above it up to `/`, each as it is on disk
/// ([`owned_safely`]). Give it a canonical path: a link anywhere in it is
/// checked as a link, not followed.
pub(crate) fn trusted(path: &Path) -> bool {
    owned_safely(path) && path.ancestors().skip(1).all(owned_safely_above)
}

/// [`owned_safely`] for a folder the walk passes through. In the unit tests,
/// a folder above the test's scratch root ([`TRUST_ROOT`]) is the machine's,
/// not the test's, and is taken as it is.
fn owned_safely_above(dir: &Path) -> bool {
    #[cfg(test)]
    {
        let top = TRUST_ROOT.with(|root| root.borrow().clone());
        if top.as_deref().is_some_and(|top| !dir.starts_with(top)) {
            return true;
        }
    }
    owned_safely(dir)
}

/// How many links a resolution follows before it gives up (the kernel's own
/// limit is about this).
const MAX_LINKS: u32 = 40;

/// `path` resolved the way the kernel does it, one component and one link at
/// a time, with every step checked: each folder it enters must be owned by
/// this user or root and writable by nobody else ([`owned_safely`]) — also one
/// it only passes through and leaves again by `..`, which somebody else
/// could later replace with a link — and each link, the same, in a folder
/// that is [`trusted`]. So nobody else can re-point any step. The real path
/// it ends at, with no link left in it; `None` when any step fails, or the
/// path is not absolute or not there.
pub(crate) fn resolve_trusted(path: &Path) -> Option<PathBuf> {
    use std::collections::VecDeque;
    use std::path::Component;
    if !path.is_absolute() {
        return None;
    }
    let mut pending: VecDeque<std::ffi::OsString> = VecDeque::new();
    let push_front = |pending: &mut VecDeque<std::ffi::OsString>, path: &Path| {
        for component in path.components().rev() {
            match component {
                Component::RootDir => pending.push_front("/".into()),
                Component::CurDir => {}
                Component::ParentDir => pending.push_front("..".into()),
                Component::Normal(name) => pending.push_front(name.to_os_string()),
                Component::Prefix(_) => {}
            }
        }
    };
    push_front(&mut pending, path);
    let mut resolved = PathBuf::from("/");
    let mut links = 0;
    while let Some(component) = pending.pop_front() {
        if component == "/" {
            resolved = PathBuf::from("/");
            continue;
        }
        if component == ".." {
            resolved.pop();
            continue;
        }
        let next = resolved.join(&component);
        let meta = std::fs::symlink_metadata(&next).ok()?;
        if !meta.file_type().is_symlink() {
            if !owned_safely_above(&next) {
                return None;
            }
            resolved = next;
            continue;
        }
        links += 1;
        if links > MAX_LINKS || !owned_safely(&next) || !trusted(&resolved) {
            return None;
        }
        let target = std::fs::read_link(&next).ok()?;
        // An absolute target starts again at `/`; a relative one goes on from
        // the folder the link is in, which is where `resolved` stands.
        push_front(&mut pending, &target);
    }
    Some(resolved)
}

/// Whether an adopted `folder` is still only the user's: it is canonical (no
/// link anywhere in it — one appearing since is somebody re-pointing it), its
/// path has no `:` (it would split on the `PATH`), it and every folder above
/// it are [`trusted`], and each CLI in it is too, where it leads as well
/// ([`resolve_trusted`]).
pub(crate) fn trusted_folder(folder: &Path) -> bool {
    if !folder.is_absolute()
        || folder.to_string_lossy().contains(':')
        || std::fs::canonicalize(folder).ok().as_deref() != Some(folder)
        || !trusted(folder)
    {
        return false;
    }
    [ToolId::Claude, ToolId::Codex].into_iter().all(|tool| {
        let entry = folder.join(tool.binary());
        if std::fs::symlink_metadata(&entry).is_err() {
            return true;
        }
        resolve_trusted(&entry).is_some_and(|real| trusted(&real))
    })
}

/// `path` if it is an executable regular file (through symlinks).
fn resolve_executable(path: &Path) -> Option<&Path> {
    let env: Env = Env::new();
    process_env::resolve_on_path(&path.to_string_lossy(), &env).map(|_| path)
}

// ---------------------------------------------------------------- starting

/// The CLI to run a sign-in with, or why there is none.
fn found_cli(state: &AppState, tool: ToolId) -> Result<PathBuf, ApiError> {
    let probe = (state.setup.probe)(&state.setup.paths, state.setup.overlay());
    super::locate::locate(tool, &probe.search())
        .map(|found| PathBuf::from(found.path))
        .ok_or_else(|| unavailable("not_installed"))
}

/// Start an install, update or terminal sign-in for `tool` and return the
/// running task.
pub async fn start_in_terminal(state: &AppState, tool: ToolId, entry: &'static Entry) -> Result<SetupTask, ApiError> {
    let setup = state.setup.clone();
    let home = setup.paths.home.clone();
    let mut spec = match entry.kind {
        TaskKind::Install | TaskKind::Update => {
            // The proxies and the shell's own PATH; read now if nothing has.
            setup.ensure_shell_read().await;
            let env = install_env(&process_env::clean_env_bare(), setup.shell_snapshot().as_ref());
            install_spec(&setup.shell_program, entry.command, &home, env, title(tool, entry.kind))
        }
        TaskKind::Login => {
            let program = found_cli(state, tool)?;
            let env = match tool {
                ToolId::Claude => process_env::claude_env([("TERM", "xterm-256color")]),
                ToolId::Codex => process_env::clean_env([("TERM", "xterm-256color")]),
            };
            login_spec(&program, commands::login_args(entry), &home, env, title(tool, entry.kind))
        }
    };

    let record = TaskRecord::new(tool, entry);
    let id = record.id.clone();
    let previous = setup.begin(record, &state.terminals)?;

    let tail = Arc::new(Mutex::new(Tail::default()));
    spec.on_data = Some(Box::new({
        let tail = tail.clone();
        move |chunk: &str| tail.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(chunk)
    }));
    let (exited, on_exit) = tokio::sync::oneshot::channel::<()>();
    spec.on_exit = Some(Box::new({
        let (setup, id, accounts) = (setup.clone(), id.clone(), state.drive.accounts.clone());
        move |code: i32| {
            let lines = redact(tail.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).lines(), &accounts);
            setup.record_exit(tool, &id, Some(code), lines);
            let _ = exited.send(());
        }
    }));

    // From here the tool is claimed, so the terminal is opened in a task of
    // its own that the request only waits for (as the API-key sign-in is): a
    // request dropped meanwhile can neither leave the claim with no terminal
    // (the tool `busy` until a restart) nor leave an installer running that
    // no task knows about. Should the opening itself be dropped or panic,
    // the guard gives the claim back.
    let opened = tokio::spawn({
        let (terminals, setup, id) = (state.terminals.clone(), setup.clone(), id.clone());
        async move {
            let mut guard = StartGuard {
                setup: setup.clone(),
                tool,
                id: id.clone(),
                previous: Some(previous),
            };
            match terminals.create_terminal_with(spec).await {
                Ok(info) => {
                    // Opened: the claim stands, and the guard has nothing to undo.
                    guard.previous = None;
                    setup.set_terminal(tool, &id, info.id);
                    tracing::info!(?tool, kind = ?entry.kind, method = entry.method, "setup task started");
                    tokio::spawn(async move {
                        if on_exit.await.is_ok() {
                            setup.after_exit(tool, &id).await;
                        }
                    });
                    Ok(())
                }
                Err(error) => {
                    tracing::error!(%error, ?tool, kind = ?entry.kind, "a setup task could not open a terminal");
                    // The claim goes back before the request hears about it.
                    drop(guard);
                    Err(ApiError::internal("cannot start the task's terminal"))
                }
            }
        }
    });
    match opened.await {
        Ok(result) => result?,
        Err(error) => {
            tracing::error!(%error, ?tool, "opening a setup task's terminal stopped");
            return Err(ApiError::internal("cannot start the task's terminal"));
        }
    }
    setup
        .task_view(tool, &state.terminals)
        .ok_or_else(|| ApiError::internal("the task vanished"))
}

/// Gives a claimed tool back when its terminal never opened — the opening
/// failed, was dropped or panicked — putting back the task it replaced.
struct StartGuard {
    setup: Arc<Setup>,
    tool: ToolId,
    id: String,
    previous: Option<Option<TaskRecord>>,
}

impl Drop for StartGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            self.setup.abandon(self.tool, &self.id, previous);
        }
    }
}

/// Ends the API-key sign-in's task however its run ends: when the run is
/// dropped before it recorded an exit (cancelled, or it panicked), the task is
/// over, outcome unknown, rather than holding the tool for ever.
struct SignInGuard {
    setup: Arc<Setup>,
    id: String,
    done: bool,
}

impl Drop for SignInGuard {
    fn drop(&mut self) {
        if !self.done {
            self.setup.record_exit(ToolId::Codex, &self.id, None, Vec::new());
        }
    }
}

/// `codex login --with-api-key`, the key on standard input and nowhere else.
///
/// Run in a task of its own that the request only waits for (module docs):
/// dropping the request drops the wait, not the sign-in.
pub async fn login_with_api_key(state: &AppState, key: &str) -> Result<SetupTask, ApiError> {
    let key = key.trim().to_string();
    if key.is_empty() || key.chars().count() > 512 || key.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ApiError::bad_request("that does not look like an API key"));
    }
    let entry = commands::find(CommandTool::Codex, TaskKind::Login, "apikey").expect("whitelisted");
    let program = found_cli(state, ToolId::Codex)?;
    let setup = state.setup.clone();
    let record = TaskRecord::new(ToolId::Codex, entry);
    let id = record.id.clone();
    setup.begin(record, &state.terminals)?;

    let accounts = state.drive.accounts.clone();
    let run = tokio::spawn({
        let mut guard = SignInGuard {
            setup: setup.clone(),
            id: id.clone(),
            done: false,
        };
        async move {
            let setup = guard.setup.clone();
            let mut invocation = Invocation::new(program.to_string_lossy(), &["login", "--with-api-key"], API_KEY_TIMEOUT);
            invocation.env = process_env::clean_env_bare();
            invocation.cwd = Some(setup.paths.home.clone());
            invocation.stdin = Some(format!("{key}\n").into_bytes());
            let output = setup.runner.run(invocation).await;

            // Whatever the CLI said, with the key cut out by value before
            // anything else sees it — a CLI that quotes a rejected key back
            // must not have it quoted again here.
            let said = format!("{}\n{}", output.stdout, output.stderr).replace(&key, "[redacted]");
            drop(key);
            let lines = redact(last_lines(&said), &accounts);
            let code = if output.spawn_failed || output.timed_out { None } else { output.code };
            setup.record_exit(ToolId::Codex, &guard.id, code, lines);
            guard.done = true;
            tracing::info!(succeeded = code == Some(0), "codex sign-in with an API key finished");
            setup.after_exit(ToolId::Codex, &guard.id).await;
        }
    });
    if let Err(error) = run.await {
        tracing::error!(%error, "the codex sign-in with an API key stopped");
    }
    setup
        .task_view(ToolId::Codex, &state.terminals)
        .ok_or_else(|| ApiError::internal("the task vanished"))
}

/// `xcode-select --install`: Apple's own installer window, no terminal.
pub async fn install_git(state: &AppState) -> bool {
    let invocation = Invocation::new("/usr/bin/xcode-select", &["--install"], super::detect::STEP_TIMEOUT);
    let output = state.setup.runner.run(invocation).await;
    state.setup.forget_detection();
    output.succeeded()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::runner::fixture::Fixture;
    use crate::setup::runner::RunOutput;
    use crate::setup::tests::{guide, marker};
    use crate::setup::{LoginState, PathRead};

    #[test]
    fn the_tail_is_what_a_terminal_would_show() {
        let mut tail = Tail::default();
        tail.push("\x1b[32m==>\x1b[0m Downloading\r\n");
        tail.push("####      40%\r########  80%\r########## 100%\r\n");
        for n in 0..30 {
            tail.push(&format!("line {n}\n"));
        }
        tail.push("curl: (6) Could not resolve host: claude.ai\n\n");
        let lines = tail.lines();
        assert_eq!(lines.len(), TAIL_LINES);
        assert_eq!(lines.last().map(String::as_str), Some("curl: (6) Could not resolve host: claude.ai"));
        assert_eq!(lines[0], "line 11");

        let mut short = Tail::default();
        short.push("\x1b[32m==>\x1b[0m Downloading\r\n####      40%\r########## 100%\r\n");
        assert_eq!(
            short.lines(),
            [" ==>  Downloading", "########## 100%"],
            "escapes become spaces; the last redraw wins"
        );

        let mut big = Tail::default();
        big.push(&"é".repeat(TAIL_BYTES));
        assert!(big.text.len() <= TAIL_BYTES, "bounded, and cut on a character boundary");
    }

    #[test]
    fn a_failure_is_named_by_the_last_line_that_names_one() {
        let lines = |text: &[&str]| text.iter().map(|line| line.to_string()).collect::<Vec<_>>();
        assert_eq!(
            classify(&lines(&["curl: (6) Could not resolve host: claude.ai"])),
            Some(TaskFailure::Network)
        );
        assert_eq!(classify(&lines(&["npm ERR! code EACCES"])), Some(TaskFailure::Permission));
        assert_eq!(
            classify(&lines(&["mkdir: /opt/homebrew/Cellar: Permission denied"])),
            Some(TaskFailure::Permission)
        );
        assert_eq!(classify(&lines(&["write error: No space left on device"])), Some(TaskFailure::Disk));
        assert_eq!(
            classify(&lines(&["Could not resolve host: x", "then: No space left on device"])),
            Some(TaskFailure::Disk),
            "the most recent cause"
        );
        assert_eq!(classify(&lines(&["something else went wrong"])), None);
    }

    #[test]
    fn an_install_runs_in_the_login_shell_s_own_environment() {
        let clean: Env = [
            ("PATH", "/Users/ada/.local/bin:/opt/homebrew/bin:/usr/bin"),
            ("HOME", "/Users/ada"),
            ("https_proxy", "http://from-the-app:1"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let read = ShellSnapshot {
            status: PathRead::Ok,
            path: Some("/usr/local/bin:/usr/bin:/bin".to_string()),
            proxies: vec![("HTTPS_PROXY".to_string(), "http://proxy.corp:3128".to_string())],
            ..ShellSnapshot::default()
        };
        let env = install_env(&clean, Some(&read));
        assert_eq!(env["PATH"], "/usr/local/bin:/usr/bin:/bin", "the shell's PATH, not Droppen's (H5)");
        assert_eq!(env["HTTPS_PROXY"], "http://proxy.corp:3128", "the shell's proxy (H32)");
        assert!(!env.contains_key("https_proxy"), "and only the shell's");
        assert_eq!(env["HOME"], "/Users/ada");
        assert_eq!(env["TERM"], "xterm-256color");

        let unread = install_env(&clean, None);
        assert_eq!(unread["PATH"], shell::MINIMAL_PATH, "no read: launchd's PATH, which `-l` builds on");
        let timed_out = ShellSnapshot {
            status: PathRead::Timeout,
            ..ShellSnapshot::default()
        };
        assert_eq!(install_env(&clean, Some(&timed_out))["PATH"], shell::MINIMAL_PATH);

        let entry = commands::find(CommandTool::Codex, TaskKind::Install, "native").unwrap();
        let spec = install_spec("/bin/zsh", entry.command, Path::new("/Users/ada"), env, title(ToolId::Codex, entry.kind));
        assert_eq!(spec.command, "/bin/zsh");
        assert_eq!(spec.args, ["-l", "-c", entry.command], "the whitelisted line, verbatim");
        assert!(entry.command.starts_with("/bin/sh -c '"), "POSIX under /bin/sh, whatever the login shell");
        assert_eq!(
            install_spec("/usr/local/bin/fish", entry.command, Path::new("/Users/ada"), Env::new(), String::new()).args,
            ["-l", "-c", entry.command]
        );
        assert_eq!(
            install_spec("/bin/tcsh", entry.command, Path::new("/Users/ada"), Env::new(), String::new()).args,
            ["-c", entry.command],
            "tcsh takes -l only alone"
        );
        assert_eq!(spec.size, Some(TASK_SIZE));
        assert_eq!(spec.title.as_deref(), Some("Install Codex"));
        assert_eq!(spec.cwd.as_deref(), Some("/Users/ada"));
    }

    #[test]
    fn a_sign_in_runs_the_cli_the_guide_found() {
        let entry = commands::find(CommandTool::Claude, TaskKind::Login, "console").unwrap();
        let env: Env = [("PATH".to_string(), "/children/path".to_string())].into_iter().collect();
        let spec = login_spec(
            Path::new("/Users/ada/.local/bin/claude"),
            commands::login_args(entry),
            Path::new("/Users/ada"),
            env,
            title(ToolId::Claude, TaskKind::Login),
        );
        assert_eq!(spec.command, "/Users/ada/.local/bin/claude");
        assert_eq!(spec.args, ["auth", "login", "--console"]);
        assert_eq!(
            spec.env.as_ref().and_then(|env| env.get("PATH")).map(String::as_str),
            Some("/children/path")
        );
        assert!(
            !spec.env.as_ref().unwrap().contains_key("BROWSER"),
            "nothing stops the CLI opening this device's browser"
        );
        assert_eq!(spec.title.as_deref(), Some("Sign in to Claude Code"));
    }

    /// A state whose guide only ever looks in a scratch home.
    async fn scene(fixture: Arc<Fixture>) -> (tempfile::TempDir, AppState) {
        let home = tempfile::tempdir().expect("tempdir");
        let mut state = crate::api::test_support::state(home.path()).await;
        state.setup = guide(home.path(), fixture, Some(PathRead::Ok));
        (home, state)
    }

    fn running(tool: ToolId, kind: TaskKind, method: &str) -> TaskRecord {
        let entry = commands::find(tool.into(), kind, method).expect("whitelisted");
        TaskRecord::new(tool, entry)
    }

    /// The exit is what updates detection: an install that put `claude` on the
    /// PATH reads as found the moment it finishes.
    #[tokio::test]
    async fn an_exit_code_updates_the_task_and_detection() {
        let fixture = Fixture::new();
        fixture
            .answer("claude --version", RunOutput::exited(0, "2.1.300 (Claude Code)\n", ""))
            .answer("claude auth status --json", RunOutput::exited(1, "{}", ""));
        let (home, state) = scene(fixture.clone()).await;
        let setup = state.setup.clone();
        assert!(setup.report(&state, false).await.tools[0].found.0.is_none());

        let record = running(ToolId::Claude, TaskKind::Install, "native");
        let id = record.id.clone();
        setup.begin(record, &state.terminals).expect("free");
        let view = setup.task_view(ToolId::Claude, &state.terminals).expect("task");
        assert!(view.running);

        // The installer ran (here: a marker file appears where it would put
        // the binary) and exited 0.
        marker(&home.path().join("bin/claude"));
        setup.record_exit(ToolId::Claude, &id, Some(0), vec!["installed".to_string()]);
        let settling = setup.task_view(ToolId::Claude, &state.terminals).expect("task");
        assert!(settling.running && !settling.failed, "an exit of 0 is not yet a success");
        setup.after_exit(ToolId::Claude, &id).await;
        let report = setup.report(&state, false).await;
        let claude = &report.tools[0];
        assert!(claude.found.0.is_some(), "found after the install");
        assert_eq!(claude.version.0.as_deref(), Some("2.1.300"));
        let task = claude.task.0.as_ref().expect("the last task");
        assert!(!task.running);
        assert!(!task.failed);
        assert_eq!(task.exit_code.0, Some(0));
        assert!(task.tail.is_empty(), "a success keeps no tail");
    }

    /// An installer can end well without installing anything: exit 0 with the
    /// tool still nowhere is a failure, its last lines kept. So is an update
    /// that exited 0 and left the tool as old as it was.
    #[tokio::test]
    async fn an_exit_of_0_that_installed_nothing_failed() {
        let _guard = crate::setup::tests::ChildPathGuard::take();
        let fixture = Fixture::new();
        fixture
            .answer("zsh -ilc", RunOutput::timeout())
            .answer("claude --version", RunOutput::exited(0, "2.1.1 (Claude Code)\n", ""))
            .answer("claude auth status --json", RunOutput::exited(1, "{}", ""));
        let (home, state) = scene(fixture.clone()).await;
        let setup = state.setup.clone();

        let record = running(ToolId::Codex, TaskKind::Install, "native");
        let id = record.id.clone();
        setup.begin(record, &state.terminals).expect("free");
        setup.record_exit(ToolId::Codex, &id, Some(0), vec!["Installation complete!".to_string()]);
        setup.after_exit(ToolId::Codex, &id).await;
        let task = setup.task_view(ToolId::Codex, &state.terminals).expect("task");
        assert!(!task.running && task.failed, "{task:?}");
        assert_eq!(task.exit_code.0, Some(0));
        assert_eq!(task.failure.0, None, "no cause in its words");
        assert_eq!(task.tail, ["Installation complete!"]);
        assert!(
            fixture.lines().iter().any(|line| line.starts_with("zsh -ilc")),
            "the login shell was asked where it went first"
        );

        marker(&home.path().join("bin/claude"));
        let record = running(ToolId::Claude, TaskKind::Update, "native");
        let id = record.id.clone();
        setup.begin(record, &state.terminals).expect("free");
        setup.record_exit(ToolId::Claude, &id, Some(0), vec!["Already up to date".to_string()]);
        setup.after_exit(ToolId::Claude, &id).await;
        let task = setup.task_view(ToolId::Claude, &state.terminals).expect("task");
        assert!(task.failed, "2.1.1 is still older than {}", crate::setup::detect::MIN_CLAUDE);
    }

    #[tokio::test]
    async fn a_failed_task_says_why_and_keeps_its_tail_redacted() {
        let _guard = crate::setup::tests::ChildPathGuard::take();
        let fixture = Fixture::new();
        fixture.answer("zsh -ilc", RunOutput::timeout());
        let (_home, state) = scene(fixture.clone()).await;
        let setup = state.setup.clone();
        let record = running(ToolId::Codex, TaskKind::Install, "native");
        let id = record.id.clone();
        setup.begin(record, &state.terminals).expect("free");

        // curl's own exit code: the installer is downloaded before it runs, so
        // a download that fails is the line's status (6: no such host).
        let mut tail = Tail::default();
        tail.push("Downloading codex…\r\nAuthorization: sk-ant-oat01-AbCdEfGhIjKlMnOp\r\ncurl: (6) Could not resolve host: chatgpt.com\r\n");
        setup.record_exit(ToolId::Codex, &id, Some(6), redact(tail.lines(), &state.drive.accounts));
        let task = setup.task_view(ToolId::Codex, &state.terminals).expect("task");
        assert_eq!(task.exit_code.0, Some(6));
        assert!(task.failed && !task.running);
        assert_eq!(task.failure.0, Some(TaskFailure::Network));
        assert!(task.tail.iter().any(|line| line.contains("sk-ant-\u{2026}[redacted]")), "{:?}", task.tail);
        assert!(
            !serde_json::to_string(&task).unwrap().contains("AbCdEf"),
            "the token never reaches the report"
        );

        // Still not found, so the login shell is read again (and adopted).
        setup.after_exit(ToolId::Codex, &id).await;
        assert!(fixture.lines().iter().any(|line| line.starts_with("zsh -ilc")), "{:?}", fixture.lines());
        assert_eq!(setup.path_read(), PathRead::Timeout);

        // A later exit hook for the same run changes nothing.
        setup.record_exit(ToolId::Codex, &id, Some(0), Vec::new());
        assert_eq!(setup.task_view(ToolId::Codex, &state.terminals).unwrap().exit_code.0, Some(6));
    }

    #[tokio::test]
    async fn a_running_task_holds_its_tool_and_a_vanished_terminal_lets_go() {
        let (_home, state) = scene(Fixture::new()).await;
        let setup = state.setup.clone();
        let mut record = running(ToolId::Claude, TaskKind::Login, "claudeai");
        record.terminal_id = Some("no-such-terminal".to_string());
        setup.begin(record, &state.terminals).expect("free");
        // The terminal is not in the registry — the server restarted and the
        // shim is gone — so the task reads as finished, exit unknown.
        let view = setup.task_view(ToolId::Claude, &state.terminals).expect("task");
        assert!(!view.running);
        assert_eq!(view.exit_code.0, None);
        assert!(setup
            .begin(running(ToolId::Claude, TaskKind::Install, "native"), &state.terminals)
            .is_ok());
        // The new one has no terminal yet (it is being started): busy.
        let refused = setup
            .begin(running(ToolId::Claude, TaskKind::Install, "brew"), &state.terminals)
            .expect_err("busy");
        assert_eq!(refused.status(), gotcha::axum::http::StatusCode::CONFLICT);
        assert_eq!(refused.body()["error"], "busy");
        assert!(
            setup.begin(running(ToolId::Codex, TaskKind::Install, "native"), &state.terminals).is_ok(),
            "per tool"
        );
    }

    #[tokio::test]
    async fn the_api_key_goes_to_stdin_and_nowhere_else() {
        let key = "sk-proj-TESTKEY0123456789abcdefghijklmnop";
        let fixture = Fixture::new();
        fixture
            .answer(
                "codex login --with-api-key",
                RunOutput::exited(1, "", format!("Error: the key {key} was refused\n")),
            )
            .answer("codex --version", RunOutput::exited(0, "codex-cli 0.163.0\n", ""))
            .answer("codex login status", RunOutput::exited(1, "", "Not logged in\n"));
        let (home, state) = scene(fixture.clone()).await;
        marker(&home.path().join("bin/codex"));

        let task = login_with_api_key(&state, &format!("  {key}  ")).await.expect("ran");
        let login = fixture
            .asked()
            .into_iter()
            .find(|asked| asked.line().starts_with("codex login --with-api-key"))
            .expect("the sign-in ran");
        assert_eq!(login.args, ["login", "--with-api-key"], "never on the command line");
        assert_eq!(login.stdin.as_deref(), Some(format!("{key}\n").as_bytes()), "only on stdin");
        for asked in fixture.asked() {
            assert!(!asked.args.iter().any(|arg| arg.contains("TESTKEY")), "{}", asked.line());
        }
        assert_eq!(task.terminal_id.0, None, "no terminal for this one");
        assert!(!task.running && task.failed);
        assert_eq!(task.exit_code.0, Some(1));
        let serialized = serde_json::to_string(&task).unwrap();
        assert!(!serialized.contains("TESTKEY"), "the key is never echoed: {serialized}");
        assert!(task.tail.iter().any(|line| line.contains("[redacted]")), "{:?}", task.tail);
        assert!(login_with_api_key(&state, "has a space").await.is_err());
        assert!(login_with_api_key(&state, "").await.is_err());
    }

    /// The request that carried the key goes away (the window closed, the
    /// connection dropped) while codex is still signing in: the sign-in runs
    /// to its end anyway, and codex is free for the next task after it.
    #[tokio::test]
    async fn a_sign_in_whose_request_went_away_still_ends() {
        let fixture = Fixture::new();
        fixture
            .answer("codex login --with-api-key", RunOutput::exited(0, "Successfully logged in\n", ""))
            .answer("codex --version", RunOutput::exited(0, "codex-cli 0.163.0\n", ""))
            .answer("codex login status", RunOutput::exited(0, "", "Logged in using an API key - sk-proj-***\n"));
        let still_running = fixture.hold("codex login --with-api-key");
        let (home, state) = scene(fixture.clone()).await;
        marker(&home.path().join("bin/codex"));

        let request = tokio::spawn({
            let state = state.clone();
            async move { login_with_api_key(&state, "sk-proj-TESTKEY0123456789").await }
        });
        for _ in 0..200 {
            if fixture.lines().iter().any(|line| line.starts_with("codex login --with-api-key")) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        request.abort();
        assert!(request.await.is_err(), "the request was dropped mid sign-in");
        assert!(state.setup.task_view(ToolId::Codex, &state.terminals).expect("task").running);

        still_running.notify_one();
        for _ in 0..200 {
            if !state.setup.task_view(ToolId::Codex, &state.terminals).expect("task").running {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let task = state.setup.task_view(ToolId::Codex, &state.terminals).expect("task");
        assert!(!task.running && !task.failed, "{task:?}");
        assert_eq!(task.exit_code.0, Some(0));
        assert!(
            state
                .setup
                .begin(running(ToolId::Codex, TaskKind::Install, "native"), &state.terminals)
                .is_ok(),
            "not busy for ever"
        );
    }

    /// A run that never got to say how it ended (cancelled, or it panicked)
    /// still ends its task.
    #[tokio::test]
    async fn a_sign_in_dropped_before_its_exit_is_over() {
        let (_home, state) = scene(Fixture::new()).await;
        let record = running(ToolId::Codex, TaskKind::Login, "apikey");
        let id = record.id.clone();
        state.setup.begin(record, &state.terminals).expect("free");
        drop(SignInGuard {
            setup: state.setup.clone(),
            id,
            done: false,
        });
        let task = state.setup.task_view(ToolId::Codex, &state.terminals).expect("task");
        assert!(!task.running && task.failed);
        assert_eq!(task.exit_code.0, None);
    }

    #[tokio::test]
    async fn a_sign_in_poll_is_one_cheap_question_reused_briefly() {
        let fixture = Fixture::new();
        fixture.answer("codex login status", RunOutput::exited(0, "", "Logged in using ChatGPT\n"));
        let (home, state) = scene(fixture.clone()).await;
        assert_eq!(
            state.setup.poll_login(ToolId::Codex).await.state,
            LoginState::Unknown,
            "nothing to ask yet"
        );
        marker(&home.path().join("bin/codex"));
        state.setup.logins.lock().unwrap().clear();
        assert_eq!(state.setup.poll_login(ToolId::Codex).await.state, LoginState::SignedIn);
        assert_eq!(state.setup.poll_login(ToolId::Codex).await.state, LoginState::SignedIn);
        let asked = fixture.lines();
        assert_eq!(asked, ["codex login status"], "once, and no app server for a poll: {asked:?}");
    }

    #[tokio::test]
    async fn only_an_executable_named_for_the_tool_is_adopted() {
        let _guard = crate::setup::tests::ChildPathGuard::take();
        let (home, state) = scene(Fixture::new()).await;
        let scratch = crate::setup::tests::TrustRoot::at(home.path());
        let root = scratch.0.clone();
        let setup = state.setup.clone();
        let picked = root.join("opt/claude-somewhere/bin/claude");
        marker(&picked);
        std::fs::write(root.join("opt/plain"), "x").unwrap();
        for bad in [
            "relative/claude".to_string(),
            root.join("opt/plain").to_string_lossy().into_owned(),
            root.join("opt/claude-somewhere/bin").to_string_lossy().into_owned(),
            picked.to_string_lossy().replace("/claude", "/codex"),
        ] {
            assert_eq!(
                setup.adopt(ToolId::Claude, &bad).expect_err(&bad).body()["error"],
                "not_the_tool",
                "{bad}"
            );
        }
        assert!(setup.adopt(ToolId::Codex, &picked.to_string_lossy()).is_err(), "claude is not codex");

        // A folder others may write to (a shared one, another user's) could
        // have its `claude` swapped for anything; so could a link into one, a
        // CLI others may write to, and a folder anywhere above it open to all.
        use std::os::unix::fs::PermissionsExt;
        let chmod = |path: &Path, mode: u32| std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
        let shared = root.join("shared/bin/claude");
        marker(&shared);
        chmod(shared.parent().unwrap(), 0o777);
        let link = root.join("links/claude");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&shared, &link).unwrap();
        let writable = root.join("loose/bin/claude");
        marker(&writable);
        chmod(&writable, 0o775);
        let under_open = root.join("open/deep/bin/claude");
        marker(&under_open);
        chmod(&root.join("open"), 0o777);
        for theirs in [&shared, &link, &writable, &under_open] {
            assert_eq!(
                setup.adopt(ToolId::Claude, &theirs.to_string_lossy()).expect_err("not adopted").body()["error"],
                "not_yours",
                "{}",
                theirs.display()
            );
        }
        assert!(setup.overlay().is_empty());

        // Under a sticky folder open to all (`/tmp`) is not the user's either:
        // what is there can be cleaned away and made again by anybody.
        let in_sticky = root.join("sticky/mine/bin/claude");
        marker(&in_sticky);
        chmod(&root.join("sticky"), 0o1777);
        assert_eq!(
            setup.adopt(ToolId::Claude, &in_sticky.to_string_lossy()).expect_err("sticky").body()["error"],
            "not_yours",
            "under a sticky folder"
        );

        // A folder whose real path has a `:` ("Tools 24/25" in the Finder)
        // would split on the PATH into half a folder and a relative `25/bin`.
        let colon = root.join("Tools 24:25/bin/claude");
        marker(&colon);
        assert_eq!(
            setup.adopt(ToolId::Claude, &colon.to_string_lossy()).expect_err("colon").body()["error"],
            "colon_in_path"
        );
        assert!(setup.overlay().is_empty());

        // A CLI that is a link is followed one link at a time: a link on the
        // way that somebody else could re-point (here, in a folder open to
        // all) makes it theirs, even though the chain ends at the user's own.
        let hop = root.join("hop");
        std::fs::create_dir_all(&hop).unwrap();
        std::os::unix::fs::symlink(&picked, hop.join("claude")).unwrap();
        let chained = root.join("chain/bin/claude");
        std::fs::create_dir_all(chained.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(hop.join("claude"), &chained).unwrap();
        chmod(&hop, 0o777);
        assert_eq!(
            setup.adopt(ToolId::Claude, &chained.to_string_lossy()).expect_err("a link others can re-point").body()["error"],
            "not_yours"
        );
        chmod(&hop, 0o755);
        let first = setup.adopt(ToolId::Claude, &chained.to_string_lossy()).expect("every link the user's");
        assert_eq!(first, chained.parent().unwrap(), "the folder the link is in, not where it leads");

        // A link whose target only passes through somebody else's folder and
        // leaves it again by `..` (`…/bob/x/../../opt/…`): x could be swapped
        // for a link of bob's at any time, and `x/..` would lead elsewhere.
        let bob = root.join("bob");
        std::fs::create_dir_all(bob.join("x")).unwrap();
        chmod(&bob, 0o777);
        let through = root.join("through/bin/claude");
        std::fs::create_dir_all(through.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(bob.join("x/../../opt/claude-somewhere/bin/claude"), &through).unwrap();
        assert!(through.exists(), "the link does lead to the user's claude");
        assert_eq!(
            setup.adopt(ToolId::Claude, &through.to_string_lossy()).expect_err("through bob's folder").body()["error"],
            "not_yours"
        );

        // Reached through a link to its folder, the folder kept is the real one.
        let via = root.join("via");
        std::os::unix::fs::symlink(picked.parent().unwrap(), &via).unwrap();
        let dir = setup.adopt(ToolId::Claude, &via.join("claude").to_string_lossy()).expect("adopted");
        assert_eq!(dir, picked.parent().unwrap(), "the real folder, not the link");
        assert_eq!(setup.overlay(), [first, dir.clone()]);
        assert!(process_env::child_path().split(':').any(|entry| Path::new(entry) == dir));
        setup.adopt(ToolId::Claude, &picked.to_string_lossy()).expect("again");
        assert_eq!(setup.overlay().len(), 2, "adopted once");
        // The folder a link on the way sits in is opened to others afterwards:
        // the chain drops off the stored overlay the next time it is read (a
        // rescan, a restart, the next search). The PATH children already have
        // keeps it until then; and only this user or root could have made
        // that change, since every step was theirs when it was adopted.
        chmod(&hop, 0o777);
        assert_eq!(setup.overlay(), [dir.clone()], "the chain is no longer the user's alone");
        chmod(&hop, 0o755);

        // Typed in another case on a disk that ignores case (a Mac's, by
        // default): the folder is kept as the disk spells it, so the later
        // checks, which compare with the disk's own spelling, still pass.
        let mixed = root.join("Mixed/bin/claude");
        marker(&mixed);
        let typed = root.join("mixed/bin/claude");
        if typed.exists() {
            let kept = setup.adopt(ToolId::Claude, &typed.to_string_lossy()).expect("the same folder, typed in lower case");
            assert_eq!(kept, mixed.parent().unwrap(), "the disk's spelling");
            assert!(setup.overlay().contains(&kept));
        } else {
            eprintln!("this disk tells case apart: the other-case spelling was not tried");
        }
        let claude = &setup.report(&state, false).await.tools[0];
        assert_eq!(claude.found.0.as_ref().map(|found| found.via), Some(super::super::FoundVia::Picked));
    }

    /// The request that started an install goes away while its terminal is
    /// being opened: the terminal opens all the same, its exit is settled,
    /// and the tool is free after. A real terminal (the server's own pty shim)
    /// runs a stand-in login shell that takes `-l -c <line>` and exits at
    /// once: nothing is installed, nothing is downloaded.
    #[tokio::test]
    async fn an_install_whose_request_went_away_while_its_terminal_opened_still_ends() {
        let exe = std::env::current_exe().unwrap_or_default();
        if crate::pty_shim::shim_program() == exe {
            eprintln!("no server binary beside the tests to run the pty shim (a `cargo test --lib` on its own): not run");
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().expect("tempdir");
        let shell = home.path().join("stand-in-shell");
        std::fs::write(&shell, "#!/bin/sh\nsleep 1\nexit 0\n").expect("write");
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let mut state = crate::api::test_support::state(home.path()).await;
        // What the instance lock's owner makes at boot, and this lock-less
        // state does not: where terminals keep their identity and socket.
        std::fs::create_dir_all(state.instance.pty_shims()).expect("the shims' folder");
        state.setup = crate::setup::tests::guide_in_shell(home.path(), Fixture::new(), Some(PathRead::Ok), &shell.to_string_lossy());
        let entry = commands::find(CommandTool::Claude, TaskKind::Install, "native").expect("whitelisted");

        let request = tokio::spawn({
            let state = state.clone();
            async move { start_in_terminal(&state, ToolId::Claude, entry).await }
        });
        // Once the tool is claimed, the opening is under way: drop the request.
        for _ in 0..2000 {
            if state.setup.task_view(ToolId::Claude, &state.terminals).is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        request.abort();
        let _ = request.await;

        let mut view = None;
        for _ in 0..1000 {
            let now = state.setup.task_view(ToolId::Claude, &state.terminals).expect("the task");
            if !now.running {
                view = Some(now);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let view = view.expect("the task ended");
        assert!(view.terminal_id.0.is_some(), "the terminal opened although nobody waited for it: {view:?}");
        assert_eq!(view.exit_code.0, Some(0));
        assert!(view.failed, "exit 0 and still no claude: settled as failed, not left running");
        assert!(
            state
                .setup
                .begin(running(ToolId::Claude, TaskKind::Install, "brew"), &state.terminals)
                .is_ok(),
            "and the tool is free"
        );
    }

    /// A terminal that never opened (the opening failed, was dropped or
    /// panicked) gives the tool back, with the task it replaced.
    #[tokio::test]
    async fn a_task_whose_terminal_never_opened_lets_go_of_its_tool() {
        let (_home, state) = scene(Fixture::new()).await;
        let setup = state.setup.clone();
        let mut done = running(ToolId::Claude, TaskKind::Install, "native");
        done.finished = true;
        done.exit_code = Some(0);
        setup.begin(done, &state.terminals).expect("free");
        let next = running(ToolId::Claude, TaskKind::Update, "native");
        let id = next.id.clone();
        let previous = setup.begin(next, &state.terminals).expect("free");
        assert!(
            setup.begin(running(ToolId::Claude, TaskKind::Install, "brew"), &state.terminals).is_err(),
            "claimed"
        );
        drop(StartGuard {
            setup: setup.clone(),
            tool: ToolId::Claude,
            id,
            previous: Some(previous),
        });
        let view = setup.task_view(ToolId::Claude, &state.terminals).expect("task");
        assert_eq!((view.kind, view.exit_code.0), (TaskKind::Install, Some(0)), "the earlier task is back");
        assert!(
            setup.begin(running(ToolId::Claude, TaskKind::Install, "brew"), &state.terminals).is_ok(),
            "and the tool is free"
        );
    }
}
