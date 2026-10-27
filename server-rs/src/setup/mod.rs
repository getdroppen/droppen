//! The first-run guide's server half: which AI tools this device has.
//!
//! The desktop app's first window walks a new user from "nothing installed"
//! to a first session. Its first step needs one answer the web app cannot get
//! for itself — *are Claude Code and Codex ready on this device?* — and that
//! answer is assembled here, behind `GET /api/setup/tools`, out of what the
//! CLIs say about themselves:
//!
//! * [`locate`] — where each CLI is: on the child `PATH`, or in one of the
//!   places installers and version managers put things;
//! * [`detect`] — its version, and its own login status;
//! * [`shell`] — what the user's login shell says the `PATH`, the proxies and
//!   the two API keys are (presence only);
//! * [`runner`] — the seam every one of those programs is run through, so the
//!   tests run none of them;
//! * [`commands`] / [`tasks`] — the whitelisted installs, updates and
//!   sign-ins, and the terminals they run in, for the desktop app only
//!   ([`app_caller`]);
//! * [`progress`] — where the guide stands, kept here rather than in the
//!   page, whose origin moves with the port;
//! * [`folders`] — step 03's folders: recent ones, and a practice folder.
//!
//! # "Ready"
//!
//! A tool is ready when it runs, is at least [`detect::MIN_CLAUDE`] /
//! [`detect::MIN_CODEX`], and either its own login works (and, as far as can
//! be told, its plan includes it) or a Droppen account for it is stored —
//! accounts are verified when they are saved, so a stored one is a working one.
//!
//! # The `PATH`
//!
//! A CLI installed while the server runs must be found without a restart. The
//! common installs land in directories the server's `PATH` has from the start
//! (`~/.local/bin`, `/opt/homebrew/bin`); for the rest, a re-read of the login
//! shell (`POST /api/setup/tools/rescan`) rebuilds the `PATH` every child gets
//! ([`crate::process_env::set_child_path`]) the way the desktop app builds it at
//! launch, plus the directories the user adopted, which persist in this
//! instance's `kv` table and are applied again at boot.
//!
//! # What is never touched
//!
//! No credential file, no keychain item, no `/usage` probe — see [`detect`].
//! The login shell's key variables are tested for being set *inside* the shell;
//! their values never reach this process.

pub mod commands;
pub mod detect;
pub mod folders;
pub mod locate;
pub mod progress;
pub mod runner;
pub mod shell;
pub mod tasks;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gotcha::axum::extract::FromRequestParts;
use gotcha::axum::http::request::Parts;
use gotcha::axum::http::{header, StatusCode};
use gotcha::{api, Json, Schematic, State};
use serde::{Deserialize, Serialize};

use crate::api::created::Created;
use crate::api::error::ApiError;
use crate::api::extract::{ApiJson, ApiPath};
use crate::api::{ApiRouter, AppState};
use crate::auth::AuthIdentity;
use crate::db::scope;
use crate::openapi_ext::{ExtraResponses, Nullable, Optional};
use crate::sessions::Paths;
use detect::{Detection, Probe};
use runner::{Runner, SystemRunner};
use shell::ShellSnapshot;

/// The variable the desktop app reports its own login-shell read in:
/// `ok`, `timeout` or `failed`. Server-only: never passed to a child
/// ([`crate::process_env`]).
pub const SHELL_READ_ENV: &str = "DROPPEN_DESKTOP_SHELL_READ";

/// The `kv` key of the adopted directories, under [`scope::SETUP`].
const OVERLAY_KEY: &str = "pathOverlay";

/// How long one detection is reused. Long enough that a page asking twice on
/// load spawns one round of CLIs, short enough that "I just installed it" is
/// never answered from before.
const DETECTION_REUSE: Duration = Duration::from_secs(2);

// ----------------------------------------------------------------- wire shapes

/// How the most recent read of the login shell went.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum PathRead {
    /// The shell answered in time.
    Ok,
    /// The shell's start-up files took longer than the read was allowed.
    Timeout,
    /// The shell would not start, or exited without answering.
    Failed,
    /// Nobody has read it: not run by the desktop app, and no rescan yet.
    #[default]
    Unknown,
}

impl PathRead {
    /// The desktop app's spelling, from `DROPPEN_DESKTOP_SHELL_READ`.
    pub fn from_app(value: Option<&str>) -> Option<Self> {
        match value.map(|value| value.trim().to_ascii_lowercase()).as_deref() {
            Some("ok") => Some(PathRead::Ok),
            Some("timeout") => Some(PathRead::Timeout),
            Some("failed") => Some(PathRead::Failed),
            _ => None,
        }
    }
}

/// One of the two AI CLIs the guide sets up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum ToolId {
    /// Anthropic's Claude Code (`claude`).
    Claude,
    /// OpenAI's Codex CLI (`codex`).
    Codex,
}

impl ToolId {
    /// Both, in the order the report lists them.
    pub const ALL: [ToolId; 2] = [ToolId::Claude, ToolId::Codex];

    /// The executable's name.
    pub fn binary(self) -> &'static str {
        match self {
            ToolId::Claude => "claude",
            ToolId::Codex => "codex",
        }
    }

    /// The Droppen accounts that drive this CLI.
    pub fn provider(self) -> crate::accounts::store::AccountProvider {
        match self {
            ToolId::Claude => crate::accounts::store::AccountProvider::Claude,
            ToolId::Codex => crate::accounts::store::AccountProvider::Codex,
        }
    }

    /// `claude` / `codex` from a path segment.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "claude" => Some(ToolId::Claude),
            "codex" => Some(ToolId::Codex),
            _ => None,
        }
    }
}

/// How a CLI was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum FoundVia {
    /// On the `PATH` the server's children get.
    Path,
    /// Not on the `PATH`, but in a place installers and version managers use;
    /// the guide offers to adopt its directory.
    Fallback,
    /// In a directory the user adopted.
    Picked,
}

/// How a CLI was installed, read off where its binary really lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum InstallMethod {
    /// The vendor's own installer (`~/.local/share/claude/versions/`,
    /// `~/.codex/packages/standalone/`).
    Native,
    /// Homebrew (a Cellar or Caskroom).
    Brew,
    /// npm (a `node_modules`).
    Npm,
    /// None of those.
    Unknown,
}

/// What the CLI's own status command says about its login.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "kebab-case")]
pub enum LoginState {
    /// It says it is signed in.
    SignedIn,
    /// It says it is not.
    SignedOut,
    /// It could not be asked, or its answer was not understood.
    Unknown,
}

/// Whether git is there to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum GitState {
    /// Installed. On a Mac: `xcode-select -p` succeeded.
    Ready,
    /// Not installed. On a Mac the first `git` a session runs would open
    /// Apple's install dialog in the middle of it.
    Missing,
    /// Could not be told.
    Unknown,
}

/// Where a CLI is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
pub struct ToolFound {
    /// The executable, as found (not resolved through symlinks).
    pub path: String,
    /// How it was found.
    pub via: FoundVia,
    /// How it was installed.
    pub method: InstallMethod,
}

/// Who a CLI is signed in as, by its own account. The address is shown on
/// this device only and never logged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
pub struct ToolLogin {
    /// Signed in, signed out, or unknown.
    pub state: LoginState,
    /// How: claude's `authMethod` (`claude.ai`, `console`, `oauth_token`,
    /// `api_key`, `api_key_helper`, `third_party`) or codex's `chatgpt` /
    /// `apiKey`.
    pub method: Nullable<String>,
    /// The plan, as a label only (`Max 20x`, `Pro`, `plus`).
    pub plan: Nullable<String>,
    /// The account's address.
    pub email: Nullable<String>,
    /// `false` when the plan does not include the tool (best effort: claude's
    /// `subscriptionType: "free"`); `null` when that cannot be told.
    pub usable: Nullable<bool>,
}

/// The Droppen accounts stored for one CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct DroppenAccounts {
    /// How many. Each was verified when it was saved.
    pub total: u32,
    /// The label of the one new sessions follow, if one is the default.
    pub default_label: Nullable<String>,
}

/// One AI CLI, as far as this device is concerned.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    /// Which CLI.
    pub id: ToolId,
    /// Where it is; `null` when it was found nowhere.
    pub found: Nullable<ToolFound>,
    /// What `--version` said (`2.1.211`, `0.163.0`).
    pub version: Nullable<String>,
    /// The oldest version this server works with.
    pub min_version: String,
    /// Found, its version read, and at least `minVersion`.
    pub supported: bool,
    /// Its own login.
    pub login: ToolLogin,
    /// The login shell sets `ANTHROPIC_API_KEY` (claude) / `OPENAI_API_KEY`
    /// (codex) — presence only; the value is never read.
    pub shell_key: bool,
    /// Droppen accounts stored for it.
    pub droppen_accounts: DroppenAccounts,
    /// `supported`, and signed in with a plan that includes it, or with at
    /// least one Droppen account.
    pub ready: bool,
    /// The install, update or sign-in running for it, or the last one that
    /// finished.
    pub task: Nullable<SetupTask>,
}

/// What a task does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    /// Install the tool.
    Install,
    /// Update it to the newest version.
    Update,
    /// Sign it in.
    Login,
}

/// Why a finished task failed, when its output said so plainly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum TaskFailure {
    /// It could not reach the network (`Could not resolve host`).
    Network,
    /// It was not allowed to write somewhere (`Permission denied`).
    Permission,
    /// The disk is full (`No space left`).
    Disk,
}

/// An install, update or sign-in the guide started for one tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SetupTask {
    /// Which tool.
    pub tool: ToolId,
    /// What it does.
    pub kind: TaskKind,
    /// The whitelisted method (`native`, `brew`, `npm`, `claudeai`, …).
    pub method: String,
    /// Exactly what runs, as the guide showed it.
    pub command: String,
    /// The terminal it runs in; `null` for the API-key sign-in, which has none.
    pub terminal_id: Nullable<String>,
    /// When it started, ISO-8601.
    pub started_at: String,
    /// Still running.
    pub running: bool,
    /// How it exited: `-1` when it was stopped; `null` while running, and for
    /// a task whose terminal was gone when the server came back.
    pub exit_code: Nullable<i32>,
    /// It finished without doing its job: it exited non-zero (or could not
    /// run), or an install exited 0 and the tool is still not found (an
    /// update: still older than `minVersion`).
    pub failed: bool,
    /// Why it failed, when the output said so plainly.
    pub failure: Nullable<TaskFailure>,
    /// The last lines of its output (at most 20, secrets redacted), only once
    /// it has finished and failed.
    pub tail: Vec<String>,
}

/// The operating system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
pub struct SetupOs {
    /// Node's spelling: `darwin`, `linux`, …
    pub platform: String,
    /// The macOS version (`15.3`); `null` off a Mac.
    pub macos: Nullable<String>,
    /// `arm64` or `x86_64`.
    pub arch: String,
}

/// What the install commands need.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
pub struct SetupHelpers {
    /// `brew` is on the `PATH`.
    pub brew: bool,
    /// Node's version (`22.3.0`), when `node` is on the `PATH`.
    pub node: Nullable<String>,
    /// git.
    pub git: GitState,
}

/// `GET /api/setup/tools`: which AI tools this device has, and how ready each is.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ToolsReport {
    /// How the read of the login shell whose `PATH` the server's children use
    /// went: the desktop app's at launch, or the server's own after a rescan.
    pub path_read: PathRead,
    /// The operating system.
    pub os: SetupOs,
    /// Homebrew, Node and git.
    pub helpers: SetupHelpers,
    /// Whether this caller may install, sign in and look again from here: only
    /// the desktop app's own window, signed in, on this device, and not on
    /// behalf of a device linked through Droppen ([`app_caller`]).
    pub can_install: bool,
    /// Always two: claude, then codex.
    pub tools: Vec<Tool>,
}

/// `POST /api/setup/tools/rescan` body.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Schematic)]
pub struct RescanRequest {
    /// Give the login shell 20 s instead of 5 ("look again, slowly").
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub patient: Optional<bool>,
}

/// `POST /api/setup/tools/{tool}/install` and `…/update` body.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct ToolMethodRequest {
    /// `native`, `brew` or `npm` (see `GET /api/setup/commands`).
    pub method: String,
}

/// `POST /api/setup/tools/{tool}/login` body.
#[derive(Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ToolLoginRequest {
    /// claude: `claudeai`, `console` or `sso`; codex: `chatgpt`, `device` or
    /// `apikey`.
    pub mode: String,
    /// The OpenAI API key, for codex's `apikey` mode only. Handed to the CLI on
    /// standard input; never stored, logged or returned.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub api_key: Optional<String>,
}

/// Never the key.
impl std::fmt::Debug for ToolLoginRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolLoginRequest")
            .field("mode", &self.mode)
            .field("api_key", &self.api_key.0.as_ref().map(|_| "…"))
            .finish()
    }
}

/// `POST /api/setup/tools/{tool}/use` body.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct ToolUseRequest {
    /// The executable to adopt: absolute, and named `claude` / `codex`.
    pub path: String,
}

/// `POST /api/setup/git/install` response.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct GitInstallStarted {
    /// Apple's installer window was asked for. It runs on its own from there;
    /// `GET /api/setup/tools` says when git is ready.
    pub started: bool,
}

// --------------------------------------------------------------------- state

/// How detection is pointed at a machine: the live one, or a test's.
type ProbeFn = Arc<dyn Fn(&Paths, Vec<PathBuf>) -> Probe + Send + Sync>;

/// What the server knows about its own read of the login shell.
#[derive(Debug, Default)]
struct ShellState {
    /// The most recent read, of either kind.
    last: Option<ShellSnapshot>,
    /// How the most recent *adopting* read (a rescan, or one after a task)
    /// went: the read whose `PATH` the children now use, or tried to.
    adopted: Option<PathRead>,
}

/// The guide's state: one per server, on [`AppState::setup`].
pub struct Setup {
    paths: Arc<Paths>,
    runner: Arc<dyn Runner>,
    probe: ProbeFn,
    /// The login shell's path.
    shell_program: String,
    /// What the desktop app said about its own read at launch.
    app_read: Option<PathRead>,
    shell: Mutex<ShellState>,
    /// One read of the login shell at a time.
    shell_gate: tokio::sync::Mutex<()>,
    /// The last detection, when it ran, and the generation it belongs to.
    /// Held across a detection, which is what makes concurrent requests share
    /// one round of CLIs.
    detection: tokio::sync::Mutex<Option<(Instant, u64, Arc<Detection>)>>,
    /// Bumped by anything that makes the last detection wrong (a task ended,
    /// a directory was adopted) — from places that cannot wait for the lock.
    generation: AtomicU64,
    /// Each tool's last task.
    tasks: Mutex<tasks::Tasks>,
    /// The last sign-in poll per tool.
    logins: Mutex<tasks::LoginCache>,
    /// The port this server listens on, once it does: what a request from the
    /// app's own window names as its `Host` and `Origin` ([`app_caller`]).
    listen_port: OnceLock<u16>,
}

impl std::fmt::Debug for Setup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Setup")
            .field("shell_program", &self.shell_program)
            .field("app_read", &self.app_read)
            .finish_non_exhaustive()
    }
}

impl Setup {
    /// The guide for a server: real programs, the live environment, and the
    /// desktop app's report from `DROPPEN_DESKTOP_SHELL_READ`.
    ///
    /// Touches nothing — no process, no database — until a request asks.
    pub fn new(paths: Arc<Paths>) -> Arc<Self> {
        Self::with(
            paths,
            Arc::new(SystemRunner),
            Arc::new(|paths: &Paths, overlay| Probe::live(paths.home.clone(), overlay)),
            shell::login_shell(),
            PathRead::from_app(std::env::var(SHELL_READ_ENV).ok().as_deref()),
        )
    }

    /// The guide with every input named.
    fn with(paths: Arc<Paths>, runner: Arc<dyn Runner>, probe: ProbeFn, shell_program: String, app_read: Option<PathRead>) -> Arc<Self> {
        Arc::new(Self {
            paths,
            runner,
            probe,
            shell_program,
            app_read,
            shell: Mutex::new(ShellState::default()),
            shell_gate: tokio::sync::Mutex::new(()),
            detection: tokio::sync::Mutex::new(None),
            generation: AtomicU64::new(0),
            tasks: Mutex::new(tasks::Tasks::default()),
            logins: Mutex::new(tasks::LoginCache::new()),
            listen_port: OnceLock::new(),
        })
    }

    fn db(&self) -> Arc<crate::db::Db> {
        crate::db::at(self.paths.instance_db())
    }

    /// Record the port this server listens on, once it is bound. Until then —
    /// and in a server that never binds — nothing counts as the app's window.
    pub fn set_listen_port(&self, port: u16) {
        let _ = self.listen_port.set(port);
    }

    /// The port this server listens on, if it has been recorded.
    pub fn listen_port(&self) -> Option<u16> {
        self.listen_port.get().copied()
    }

    /// The directories the user adopted, in the order they were adopted, as
    /// stored.
    fn stored_overlay(&self) -> Vec<PathBuf> {
        self.db()
            .kv_get(scope::SETUP, OVERLAY_KEY)
            .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
            .unwrap_or_default()
            .into_iter()
            .map(PathBuf::from)
            .collect()
    }

    /// The adopted directories that are still only this user's (or root's)
    /// to change ([`tasks::trusted_folder`]): the ones searched and put on the `PATH`.
    pub fn overlay(&self) -> Vec<PathBuf> {
        self.stored_overlay().into_iter().filter(|dir| tasks::trusted_folder(dir)).collect()
    }

    /// Put the adopted directories on the children's `PATH` — at boot, by the
    /// instance's owner only, since the `PATH` is the whole process's.
    ///
    /// Behind the instance lock in [`crate::app::RemoteHandApp::prepare`]: a
    /// throwaway in-process server (`export-openapi`) must not rewrite the
    /// `PATH` of a process it does not own the instance for.
    ///
    /// Each directory is checked again, as when it was adopted
    /// ([`tasks::trusted_folder`]): a directory gone since and made again by somebody
    /// else — another user's `/tmp/x` after a reboot — is left off.
    pub fn apply_overlay(&self) {
        let overlay = self.overlay();
        let left_off = self.stored_overlay().len().saturating_sub(overlay.len());
        if left_off > 0 {
            tracing::warn!(left_off, "adopted tool directories no longer only this user's are left off the PATH");
        }
        if overlay.is_empty() {
            return;
        }
        let path = locate::with_overlay(&crate::process_env::child_path(), &overlay);
        tracing::info!(dirs = overlay.len(), "adopted tool directories are on the children's PATH");
        crate::process_env::set_child_path(Some(path));
    }

    /// The last read of the login shell, if any.
    fn shell_snapshot(&self) -> Option<ShellSnapshot> {
        self.shell.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).last.clone()
    }

    /// Read the login shell, and — when `adopt` — put what it says on the
    /// children's `PATH`.
    ///
    /// The new `PATH` is the one the process inherited, in its own order,
    /// then whatever the desktop app's recipe adds to it ([`locate::build`]:
    /// the shell's directories, then the base ones), then the adopted ones —
    /// so a re-read only ever appends. Never the recipe first: on a server
    /// started with a `PATH` of its own (a Linux box whose `node` comes from
    /// nvm), putting `/usr/bin` ahead would hand every new session the
    /// system's older `node`. A read that did not answer changes nothing.
    pub async fn read_shell(&self, patient: bool, adopt: bool) -> ShellSnapshot {
        let _one_at_a_time = self.shell_gate.lock().await;
        let invocation = shell::invocation(&self.shell_program, &self.paths.home, &crate::process_env::clean_env_bare(), patient);
        let snapshot = shell::parse(&self.runner.run(invocation).await);
        tracing::info!(status = ?snapshot.status, adopt, "read the login shell");
        if adopt {
            if let (PathRead::Ok, Some(shell_path)) = (snapshot.status, snapshot.path.as_deref()) {
                let built = locate::build(&self.paths.home, Some(shell_path));
                let kept = locate::merge(&crate::process_env::inherited_path(), &built);
                let path = locate::with_overlay(&kept, &self.overlay());
                crate::process_env::set_child_path(Some(path));
            }
        }
        let mut state = self.shell.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state.last = Some(snapshot.clone());
        if adopt {
            state.adopted = Some(snapshot.status);
        }
        snapshot
    }

    /// Read the login shell once, for the key and proxy questions, if nothing
    /// has yet. Only ever for the desktop app's own server.
    async fn ensure_shell_read(&self) {
        if self.shell_snapshot().is_some() {
            return;
        }
        let _one_at_a_time = self.shell_gate.lock().await;
        if self.shell_snapshot().is_some() {
            return;
        }
        drop(_one_at_a_time);
        self.read_shell(false, false).await;
    }

    /// The `pathRead` the report carries: the server's own adopting read when
    /// there has been one, else the desktop app's, else the server's own
    /// non-adopting read, else unknown.
    fn path_read(&self) -> PathRead {
        let state = self.shell.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .adopted
            .or(self.app_read)
            .or(state.last.as_ref().map(|snapshot| snapshot.status))
            .unwrap_or_default()
    }

    /// The current detection: reused for [`DETECTION_REUSE`], unless `fresh`
    /// or something has made it wrong since ([`Setup::forget_detection`]).
    pub async fn detection(&self, fresh: bool) -> Arc<Detection> {
        let mut cached = self.detection.lock().await;
        let generation = self.generation.load(Ordering::SeqCst);
        if let Some((at, made_in, detection)) = cached.as_ref() {
            if !fresh && *made_in == generation && at.elapsed() < DETECTION_REUSE {
                return detection.clone();
            }
        }
        let probe = (self.probe)(&self.paths, self.overlay());
        let detection = Arc::new(detect::detect(&self.runner, &probe).await);
        *cached = Some((Instant::now(), generation, detection.clone()));
        detection
    }

    /// The next report must look again.
    pub fn forget_detection(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// The whole report for one request.
    pub async fn report(&self, state: &AppState, can_install: bool) -> ToolsReport {
        let report = self.report_without_tasks(state, can_install).await;
        ToolsReport {
            tools: report
                .tools
                .into_iter()
                .map(|tool| Tool {
                    task: Nullable(self.task_view(tool.id, &state.terminals)),
                    ..tool
                })
                .collect(),
            ..report
        }
    }

    async fn report_without_tasks(&self, state: &AppState, can_install: bool) -> ToolsReport {
        if state.desktop_app {
            self.ensure_shell_read().await;
        }
        let detection = self.detection(false).await;
        let shell = self.shell_snapshot().unwrap_or_default();
        let accounts = state.drive.accounts.list().await;
        let mut tools = Vec::with_capacity(detection.tools.len());
        for probe in &detection.tools {
            let provider = probe.id.provider();
            let mine: Vec<_> = accounts.iter().filter(|account| account.provider == provider).collect();
            let default_id = state.drive.accounts.default_account_id(provider).await;
            let default_label = default_id.and_then(|id| mine.iter().find(|account| account.id == id).map(|account| account.label.clone()));
            let droppen_accounts = DroppenAccounts {
                total: mine.len() as u32,
                default_label: Nullable(default_label),
            };
            let signed_in = probe.login.state == LoginState::SignedIn && probe.login.usable.0 != Some(false);
            tools.push(Tool {
                id: probe.id,
                found: Nullable(probe.found.clone()),
                version: Nullable(probe.version.clone()),
                min_version: probe.id.min_version().to_string(),
                supported: probe.supported,
                login: probe.login.clone(),
                shell_key: match probe.id {
                    ToolId::Claude => shell.anthropic_key,
                    ToolId::Codex => shell.openai_key,
                },
                ready: probe.supported && (signed_in || droppen_accounts.total > 0),
                droppen_accounts,
                task: Nullable(None),
            });
        }
        ToolsReport {
            path_read: self.path_read(),
            os: detection.os.clone(),
            helpers: detection.helpers.clone(),
            can_install,
            tools,
        }
    }
}

// -------------------------------------------------------------------- routes

/// Who sent a request and what it says about where it was made: what the
/// gate on the app-only routes reads ([`app_caller`]).
///
/// Annotate the parameter `#[api(skip)]`: it contributes nothing to the
/// OpenAPI document.
#[derive(Debug, Clone, Default)]
pub struct Caller {
    /// The peer's address, when the listener recorded one (`crate::app`
    /// serves with connect info; a router driven without a socket has none).
    pub peer: Option<SocketAddr>,
    /// Who it is signed in as, if anyone.
    pub identity: Option<AuthIdentity>,
    /// Its `Origin` header.
    pub origin: Option<String>,
    /// Its `Host` header (or the request target's authority).
    pub host: Option<String>,
}

impl<S: Send + Sync> FromRequestParts<S> for Caller {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let text = |name| parts.headers.get(name).and_then(|value: &header::HeaderValue| value.to_str().ok()).map(str::to_string);
        Ok(Caller {
            peer: parts
                .extensions
                .get::<gotcha::axum::extract::ConnectInfo<SocketAddr>>()
                .map(|info| info.0),
            identity: parts.extensions.get::<AuthIdentity>().cloned(),
            origin: text(header::ORIGIN),
            host: text(header::HOST).or_else(|| parts.uri.authority().map(|authority| authority.to_string())),
        })
    }
}

/// Whether a request is the desktop app's own window talking to its own
/// server. Every one of these must hold:
///
/// * the app runs this server;
/// * the peer is this device (loopback);
/// * the caller is signed in — the app's window always is, with the login the
///   app owns — and is not a device linked through the Droppen bridge
///   (`cloud:<device>`, which also arrives over loopback);
/// * it was addressed to `127.0.0.1:<this server's port>` (`Host`) and, when it
///   says where it was made (`Origin`), was made by a page at exactly
///   `http://127.0.0.1:<port>`, the address the app opens its window at.
///
/// The last one is what loopback alone cannot tell: a page on another site
/// whose name was re-pointed at 127.0.0.1 (DNS rebinding) names its own host,
/// and so does a browser elsewhere reaching this port through a tunnel or a
/// reverse proxy (`tailscale serve`, `cloudflared`, `ssh -R`), whose requests
/// arrive over loopback too. `require_origin` is for the routes that act: a
/// browser always sends `Origin` on a `POST` from a page.
fn from_app_window(state: &AppState, caller: &Caller, require_origin: bool) -> bool {
    let Some(port) = state.setup.listen_port() else {
        return false;
    };
    let local = caller.peer.is_some_and(|peer| match peer.ip() {
        std::net::IpAddr::V4(ip) => ip.is_loopback(),
        std::net::IpAddr::V6(ip) => ip.to_ipv4_mapped().map_or(ip.is_loopback(), |ip| ip.is_loopback()),
    });
    let signed_in = caller
        .identity
        .as_ref()
        .is_some_and(|identity| !identity.username.starts_with("cloud:"));
    let own = format!("127.0.0.1:{port}");
    let addressed_here = caller.host.as_deref() == Some(own.as_str());
    let made_here = match caller.origin.as_deref() {
        Some(origin) => origin == format!("http://{own}"),
        None => !require_origin,
    };
    state.desktop_app && local && signed_in && addressed_here && made_here
}

/// Whether this caller may install and sign in from here — what the tools
/// report says as `canInstall`. A page's own `GET` carries no `Origin`, so
/// one is not required here; the routes that act require it ([`app_caller`]).
pub fn local_app_request(state: &AppState, caller: &Caller) -> bool {
    from_app_window(state, caller, false)
}

/// The gate on every route that installs, updates, signs in or rewrites the
/// `PATH`: `403 {"error":"app_only"}` unless the request is the app's own
/// window, `Origin` included ([`from_app_window`]).
///
/// A browser on another device — a phone, a laptop on the LAN, a device linked
/// through Droppen, a page reaching this port through a tunnel — must not be
/// able to start an installer on this one. What this cannot yet tell apart is
/// the app's window from another browser on the same Mac signed in to the same
/// server at the same address. The local web access work adds the
/// app-internal account (`user add --app`, `droppen-app`) the app signs in
/// with, and once it exists this check also requires that identity.
pub fn app_caller(state: &AppState, caller: &Caller) -> Result<(), ApiError> {
    if from_app_window(state, caller, true) {
        Ok(())
    } else {
        Err(ApiError::new(StatusCode::FORBIDDEN, "app_only"))
    }
}

/// The tool a path segment names, or `400 {"error":"unknown_tool"}`.
fn tool_of(raw: &str) -> Result<ToolId, ApiError> {
    ToolId::parse(raw).ok_or_else(|| ApiError::bad_request("unknown_tool"))
}

/// Which AI tools this device has, and whether each is ready.
#[api(id = "setup_tools", group = "setup", summary = "Report which AI tools are ready")]
pub async fn tools_report(State(state): State<AppState>, #[api(skip)] caller: Caller) -> Json<ToolsReport> {
    let can_install = local_app_request(&state, &caller);
    Json(state.setup.report(&state, can_install).await)
}

/// Read the login shell again, adopt its `PATH`, and look for the tools afresh.
///
/// The desktop app's only ([`app_caller`]): it rewrites the `PATH` every
/// child of this server gets from now on, which is no request for anything
/// but the app's own window to make.
#[api(id = "setup_rescan_tools", group = "setup", summary = "Look for the AI tools again")]
pub async fn rescan_tools(
    State(state): State<AppState>,
    #[api(skip)] caller: Caller,
    ApiJson(body): ApiJson<RescanRequest>,
) -> Result<Json<ToolsReport>, ApiError> {
    app_caller(&state, &caller)?;
    state.setup.read_shell(body.patient.0.unwrap_or(false), true).await;
    state.setup.detection(true).await;
    Ok(Json(state.setup.report(&state, true).await))
}

/// Every command the guide can run, exactly as it would run it.
#[api(id = "setup_commands", group = "setup", summary = "List the install and sign-in commands")]
pub async fn list_commands(State(state): State<AppState>) -> Json<commands::SetupCommandList> {
    Json(commands::list(&*state.setup.detection(false).await))
}

/// Start a whitelisted install or update in a terminal.
async fn start_method(state: &AppState, tool: &str, kind: TaskKind, method: &str) -> Result<Created<SetupTask>, ApiError> {
    let tool = tool_of(tool)?;
    let entry = commands::find(tool.into(), kind, method).ok_or_else(|| ApiError::bad_request("unknown_method"))?;
    commands::availability(entry, &*state.setup.detection(false).await).map_err(tasks::unavailable)?;
    tasks::start_in_terminal(state, tool, entry).await.map(Created)
}

/// Install a tool: the whitelisted command for `method`, in a terminal.
#[api(id = "setup_install_tool", group = "setup", summary = "Install an AI tool")]
pub async fn install_tool(
    State(state): State<AppState>,
    #[api(skip)] caller: Caller,
    ApiPath(tool): ApiPath<String>,
    ApiJson(body): ApiJson<ToolMethodRequest>,
) -> Result<Created<SetupTask>, ApiError> {
    app_caller(&state, &caller)?;
    start_method(&state, &tool, TaskKind::Install, &body.method).await
}

/// Update a tool: the whitelisted command for `method`, in a terminal.
#[api(id = "setup_update_tool", group = "setup", summary = "Update an AI tool")]
pub async fn update_tool(
    State(state): State<AppState>,
    #[api(skip)] caller: Caller,
    ApiPath(tool): ApiPath<String>,
    ApiJson(body): ApiJson<ToolMethodRequest>,
) -> Result<Created<SetupTask>, ApiError> {
    app_caller(&state, &caller)?;
    start_method(&state, &tool, TaskKind::Update, &body.method).await
}

/// Sign a tool in: its own login command in a terminal, or — codex's
/// `apikey` mode — with the key on its standard input.
#[api(id = "setup_login_tool", group = "setup", summary = "Sign an AI tool in")]
pub async fn login_tool(
    State(state): State<AppState>,
    #[api(skip)] caller: Caller,
    ApiPath(tool): ApiPath<String>,
    ApiJson(body): ApiJson<ToolLoginRequest>,
) -> Result<Created<SetupTask>, ApiError> {
    app_caller(&state, &caller)?;
    let tool = tool_of(&tool)?;
    let entry = commands::find(tool.into(), TaskKind::Login, &body.mode).ok_or_else(|| ApiError::bad_request("unknown_method"))?;
    if entry.method == "apikey" {
        let Some(key) = body.api_key.0.as_deref() else {
            return Err(ApiError::bad_request("api_key_required"));
        };
        return tasks::login_with_api_key(&state, key).await.map(Created);
    }
    tasks::start_in_terminal(&state, tool, entry).await.map(Created)
}

/// Is it signed in yet? One cheap status check, for polling during a sign-in.
#[api(id = "setup_tool_login", group = "setup", summary = "Check an AI tool's sign-in")]
pub async fn tool_login(State(state): State<AppState>, ApiPath(tool): ApiPath<String>) -> Result<Json<ToolLogin>, ApiError> {
    Ok(Json(state.setup.poll_login(tool_of(&tool)?).await))
}

/// Ask macOS for its command line tools (git): Apple's own installer window.
#[api(id = "setup_install_git", group = "setup", summary = "Install git (Apple's command line tools)")]
pub async fn install_git(
    State(state): State<AppState>,
    #[api(skip)] caller: Caller,
) -> Result<Created<GitInstallStarted>, ApiError> {
    app_caller(&state, &caller)?;
    let entry = commands::find(commands::CommandTool::Git, TaskKind::Install, "apple").expect("whitelisted");
    commands::availability(entry, &*state.setup.detection(false).await).map_err(tasks::unavailable)?;
    Ok(Created(GitInstallStarted {
        started: tasks::install_git(&state).await,
    }))
}

/// Adopt a CLI the user pointed at (or the guide found off the `PATH`): its
/// directory joins every child's `PATH`, now and after a restart.
#[api(id = "setup_use_tool", group = "setup", summary = "Use an AI tool found elsewhere")]
pub async fn use_tool(
    State(state): State<AppState>,
    #[api(skip)] caller: Caller,
    ApiPath(tool): ApiPath<String>,
    ApiJson(body): ApiJson<ToolUseRequest>,
) -> Result<Json<ToolsReport>, ApiError> {
    app_caller(&state, &caller)?;
    state.setup.adopt(tool_of(&tool)?, &body.path)?;
    state.setup.detection(true).await;
    Ok(Json(state.setup.report(&state, true).await))
}

/// Register the guide's routes.
pub fn routes(router: ApiRouter) -> ApiRouter {
    router
        .get("/api/setup/tools", tools_report)
        .post("/api/setup/tools/rescan", rescan_tools)
        .get("/api/setup/commands", list_commands)
        .post("/api/setup/tools/{tool}/install", install_tool)
        .post("/api/setup/tools/{tool}/update", update_tool)
        .post("/api/setup/tools/{tool}/login", login_tool)
        .get("/api/setup/tools/{tool}/login", tool_login)
        .post("/api/setup/tools/{tool}/use", use_tool)
        .post("/api/setup/git/install", install_git)
        .get("/api/setup/progress", progress::get_progress)
        .put("/api/setup/progress", progress::put_progress)
        .get("/api/setup/projects/recent", folders::recent_folders)
        .post("/api/setup/practice-folder", folders::practice_folder)
}

/// Everything gotcha cannot say about the guide's routes.
pub fn describe(reg: &mut ExtraResponses) {
    reg.describe(
        "GET",
        "/api/setup/tools",
        200,
        "Claude Code and Codex on this device, and whether each is ready",
    )
    .describe(
        "POST",
        "/api/setup/tools/rescan",
        200,
        "The login shell was read again, its PATH adopted, and the tools looked for afresh",
    )
    .error("POST", "/api/setup/tools/rescan", 400, "Malformed body")
    .error(
        "POST",
        "/api/setup/tools/rescan",
        403,
        "Not the desktop app's own window on this device (`app_only`)",
    )
    .describe(
        "GET",
        "/api/setup/commands",
        200,
        "Every command the guide can run, verbatim, and whether it can run here",
    );
    for (path, what) in [
        ("/api/setup/tools/{tool}/install", "Install running in a terminal"),
        ("/api/setup/tools/{tool}/update", "Update running in a terminal"),
    ] {
        reg.restatus("POST", path, 200, 201, what)
            .error(
                "POST",
                path,
                400,
                "Unknown tool, unknown method (`unknown_method`), or one this device cannot run (`unavailable`, with a `reason`)",
            )
            .error("POST", path, 403, "Not the desktop app's own window on this device (`app_only`)")
            .error("POST", path, 409, "A task for this tool is still running (`busy`)")
            .error("POST", path, 500, "The terminal could not be started");
    }
    reg.restatus(
        "POST",
        "/api/setup/tools/{tool}/login",
        200,
        201,
        "Sign-in running in a terminal; for `apikey`, finished",
    )
    .error(
        "POST",
        "/api/setup/tools/{tool}/login",
        400,
        "Unknown tool or mode, no key for `apikey`, or the tool is not installed (`unavailable`)",
    )
    .error(
        "POST",
        "/api/setup/tools/{tool}/login",
        403,
        "Not the desktop app's own window on this device (`app_only`)",
    )
    .error(
        "POST",
        "/api/setup/tools/{tool}/login",
        409,
        "A task for this tool is still running (`busy`)",
    )
    .describe(
        "GET",
        "/api/setup/tools/{tool}/login",
        200,
        "What the tool's own status command says right now",
    )
    .error("GET", "/api/setup/tools/{tool}/login", 400, "Unknown tool")
    .describe("POST", "/api/setup/tools/{tool}/use", 200, "Adopted; the tools as they are now")
    .error(
        "POST",
        "/api/setup/tools/{tool}/use",
        400,
        "Unknown tool, not an executable named for it (`not_the_tool`), in a folder whose real path has a `:` (`colon_in_path`), or one somebody else can change: the folder, a folder above it, the CLI or a link on the way to it is not owned by this user or root, or others may write to it (`not_yours`)",
    )
    .error(
        "POST",
        "/api/setup/tools/{tool}/use",
        403,
        "Not the desktop app's own window on this device (`app_only`)",
    )
    .restatus("POST", "/api/setup/git/install", 200, 201, "Apple's installer window was asked for")
    .error(
        "POST",
        "/api/setup/git/install",
        400,
        "Not a Mac, or git is already there (`unavailable`)",
    )
    .error("POST", "/api/setup/git/install", 403, "Not the desktop app's own window on this device (`app_only`)")
    .describe(
        "GET",
        "/api/setup/progress",
        200,
        "How far the first-run guide got, and whether it opens by itself",
    )
    .body::<progress::SetupProgress>("PUT", "/api/setup/progress", 200, "Stored; the progress as it now reads")
    .error("PUT", "/api/setup/progress", 400, "Malformed body")
    .describe(
        "GET",
        "/api/setup/projects/recent",
        200,
        "The folders sessions ran in on this device, the most recent first",
    )
    .restatus(
        "POST",
        "/api/setup/practice-folder",
        200,
        201,
        "An empty practice folder: the one made last while it is still empty, else a new one",
    )
    .error("POST", "/api/setup/practice-folder", 500, "The folder could not be made");
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::process_env::Env;
    use runner::fixture::Fixture;
    use runner::RunOutput;
    use std::os::unix::fs::PermissionsExt;

    /// A guide on a scratch home whose detection only ever looks there.
    pub(crate) fn guide(home: &std::path::Path, fixture: Arc<Fixture>, app_read: Option<PathRead>) -> Arc<Setup> {
        guide_in_shell(home, fixture, app_read, "/bin/zsh")
    }

    /// [`guide`], with `shell` as the user's login shell — what an install's
    /// terminal runs.
    pub(crate) fn guide_in_shell(home: &std::path::Path, fixture: Arc<Fixture>, app_read: Option<PathRead>, shell: &str) -> Arc<Setup> {
        let scratch = home.to_path_buf();
        let probe: ProbeFn = Arc::new(move |_paths: &Paths, overlay| {
            let bin = scratch.join("bin");
            let path = std::iter::once(bin.to_string_lossy().into_owned())
                .chain(overlay.iter().map(|dir: &PathBuf| dir.to_string_lossy().into_owned()))
                .collect::<Vec<_>>()
                .join(":");
            let env: Env = [("PATH".to_string(), path)].into_iter().collect();
            Probe {
                home: scratch.clone(),
                env: env.clone(),
                claude_env: env,
                inherited_path: bin.to_string_lossy().into_owned(),
                overlay,
                system_dirs: Vec::new(),
                macos: true,
            }
        });
        Setup::with(Arc::new(Paths::new(home.to_path_buf())), fixture, probe, shell.to_string(), app_read)
    }

    pub(crate) fn marker(path: &std::path::Path) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, "not a program; never executed\n").expect("write");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    /// What a login shell prints, fenced.
    pub(crate) fn shell_answer(body: &str) -> RunOutput {
        RunOutput::exited(0, format!("hello from .zshrc\n{}\n{body}\n{}\n", shell::BEGIN, shell::END), "")
    }

    /// A test that sets the child-`PATH` override takes this, and gets back
    /// what was there when it ends. In the test build the override is the
    /// test thread's own (`process_env`), so no other test sees it meanwhile;
    /// this keeps a thread the harness reuses from inheriting it.
    pub(crate) struct ChildPathGuard {
        before: Option<String>,
    }

    impl ChildPathGuard {
        pub(crate) fn take() -> Self {
            Self {
                before: crate::process_env::child_path_override(),
            }
        }
    }

    impl Drop for ChildPathGuard {
        fn drop(&mut self) {
            crate::process_env::set_child_path(self.before.take());
        }
    }

    /// The scratch folder a trust test works in, as the top of the walk
    /// (`tasks::TRUST_ROOT`): the folders above it are the machine's, not the
    /// test's. Returns it canonical; the walk stops again when this is dropped.
    pub(crate) struct TrustRoot(pub(crate) PathBuf);

    impl TrustRoot {
        pub(crate) fn at(scratch: &std::path::Path) -> Self {
            let root = std::fs::canonicalize(scratch).expect("canonical scratch");
            tasks::TRUST_ROOT.with(|slot| *slot.borrow_mut() = Some(root.clone()));
            Self(root)
        }
    }

    impl Drop for TrustRoot {
        fn drop(&mut self) {
            tasks::TRUST_ROOT.with(|slot| *slot.borrow_mut() = None);
        }
    }

    #[test]
    fn the_app_s_report_is_read_in_its_own_spelling() {
        assert_eq!(PathRead::from_app(Some("ok")), Some(PathRead::Ok));
        assert_eq!(PathRead::from_app(Some(" TIMEOUT ")), Some(PathRead::Timeout));
        assert_eq!(PathRead::from_app(Some("failed")), Some(PathRead::Failed));
        assert_eq!(PathRead::from_app(Some("maybe")), None);
        assert_eq!(PathRead::from_app(None), None);
    }

    #[tokio::test]
    async fn the_report_is_two_tools_ready_by_login_or_by_account() {
        let home = tempfile::tempdir().expect("tempdir");
        marker(&home.path().join("bin/claude"));
        marker(&home.path().join("bin/codex"));
        let fixture = Fixture::new();
        fixture
            .answer("sw_vers -productVersion", RunOutput::exited(0, "15.3\n", ""))
            .answer("xcode-select -p", RunOutput::exited(0, "/Library/Developer/CommandLineTools\n", ""))
            .answer("claude --version", RunOutput::exited(0, "2.1.300 (Claude Code)\n", ""))
            .answer("claude auth status --json", RunOutput::exited(1, r#"{"loggedIn":false}"#, ""))
            .answer("codex --version", RunOutput::exited(0, "codex-cli 0.163.0\n", ""))
            .answer("codex login status", RunOutput::exited(0, "", "Logged in using ChatGPT\n"))
            .answer_rpc(
                "account/read",
                serde_json::json!({"account": {"type": "chatgpt", "email": "ada@example.com", "planType": "plus"}}),
            )
            .answer("zsh -ilc", shell_answer("PATH=/usr/bin\nANTHROPIC_API_KEY=set\nOPENAI_API_KEY=unset"));

        let mut state = crate::api::test_support::state(home.path()).await;
        state.setup = guide(home.path(), fixture.clone(), Some(PathRead::Timeout));
        let report = state.setup.report(&state, false).await;
        assert_eq!(
            report.tools.iter().map(|tool| tool.id).collect::<Vec<_>>(),
            [ToolId::Claude, ToolId::Codex]
        );
        assert_eq!(
            report.path_read,
            PathRead::Timeout,
            "the desktop app's own read, when the server has not adopted one"
        );
        assert!(!report.can_install);
        let claude = &report.tools[0];
        assert_eq!(claude.login.state, LoginState::SignedOut);
        assert!(claude.supported && !claude.ready, "signed out and no account");
        assert!(!claude.shell_key, "an ordinary server reads no shell on a GET");
        let codex = &report.tools[1];
        assert!(codex.ready);
        assert_eq!(codex.login.plan.0.as_deref(), Some("plus"));
        assert!(!fixture.lines().iter().any(|line| line.starts_with("zsh")), "{:?}", fixture.lines());

        // A stored account makes claude ready; the desktop app's server
        // also reads the shell for the key question, once.
        let account = state
            .drive
            .accounts
            .create(
                "work gateway",
                crate::accounts::store::AccountProvider::Claude,
                crate::accounts::store::Credential::Oauth("sk-ant-oat01-test-only".into()),
            )
            .await
            .expect("account");
        state
            .drive
            .accounts
            .set_default(Some(&account.id), crate::accounts::store::AccountProvider::Claude)
            .await
            .expect("default");
        state.desktop_app = true;
        let report = state.setup.report(&state, false).await;
        let claude = &report.tools[0];
        assert!(claude.ready, "a verified Droppen account is a way to be ready");
        assert_eq!(claude.droppen_accounts.total, 1);
        assert_eq!(claude.droppen_accounts.default_label.0.as_deref(), Some("work gateway"));
        assert!(claude.shell_key, "ANTHROPIC_API_KEY is set in the login shell");
        assert!(!report.tools[1].shell_key);
        let _ = state.setup.report(&state, false).await;
        assert_eq!(
            fixture.lines().iter().filter(|line| line.starts_with("zsh")).count(),
            1,
            "read once, then cached"
        );

        let serialized = serde_json::to_string(&report).expect("json");
        assert!(!serialized.contains("sk-"), "no credential fragment in the report: {serialized}");
    }

    #[tokio::test]
    async fn a_rescan_puts_the_shell_s_path_on_every_child_and_finds_what_it_adds() {
        let _guard = ChildPathGuard::take();
        let home = tempfile::tempdir().expect("tempdir");
        let nvm = home.path().join(".nvm/versions/node/v22.3.0/bin");
        std::fs::create_dir_all(&nvm).expect("nvm");
        let fixture = Fixture::new();
        fixture.answer("zsh -ilc", shell_answer(&format!("PATH={}:/usr/bin", nvm.display())));
        let setup = guide(home.path(), fixture.clone(), Some(PathRead::Timeout));

        let before = crate::process_env::inherited_path();
        let read = setup.read_shell(true, true).await;
        assert_eq!(read.status, PathRead::Ok);
        let after = crate::process_env::child_path();
        let dirs: Vec<&str> = after.split(':').collect();
        assert!(
            dirs.contains(&nvm.to_string_lossy().as_ref()),
            "the shell's directory is on the PATH: {after}"
        );
        assert!(dirs.contains(&home.path().join(".local/bin").to_string_lossy().as_ref()), "and the base");
        // The inherited PATH comes first, whole and in its own order; the
        // read only appends. A server whose `node` comes from nvm must not
        // find the system's `/usr/bin/node` first after a rescan.
        let inherited: Vec<&str> = before.split(':').filter(|dir| !dir.is_empty()).collect();
        assert_eq!(dirs[..inherited.len()], inherited[..], "a re-read keeps the inherited PATH as it was: {after}");
        assert!(dirs.len() > inherited.len(), "and adds after it: {after}");
        // Every child now starts with it: a session's CLI, a probe, a terminal.
        assert_eq!(crate::process_env::clean_env_bare().get("PATH"), Some(&after));
        assert_eq!(crate::process_env::claude_env_bare().get("PATH"), Some(&after));
        assert_eq!(setup.path_read(), PathRead::Ok, "the adopted read is the one reported");
        let asked = fixture.asked();
        assert_eq!(asked.len(), 1);
        assert_eq!(
            asked[0].path.as_deref(),
            Some(shell::MINIMAL_PATH),
            "the read itself starts from launchd's PATH"
        );
    }

    #[tokio::test]
    async fn a_rescan_the_shell_did_not_answer_changes_nothing_and_says_why() {
        let _guard = ChildPathGuard::take();
        let home = tempfile::tempdir().expect("tempdir");
        let fixture = Fixture::new();
        fixture.answer("zsh -ilc", RunOutput::timeout());
        let setup = guide(home.path(), fixture, Some(PathRead::Ok));
        let before = crate::process_env::child_path_override();
        assert_eq!(setup.read_shell(false, true).await.status, PathRead::Timeout);
        assert_eq!(crate::process_env::child_path_override(), before);
        assert_eq!(setup.path_read(), PathRead::Timeout, "the rescan's own outcome, not the app's older one");
    }

    #[tokio::test]
    async fn the_adopted_directories_come_back_at_boot() {
        let _guard = ChildPathGuard::take();
        let home = tempfile::tempdir().expect("tempdir");
        let scratch = TrustRoot::at(home.path());
        let picked = scratch.0.join("tools/bin");
        std::fs::create_dir_all(&picked).expect("dir");
        let setup = guide(home.path(), Fixture::new(), None);
        setup.apply_overlay();
        assert_eq!(
            crate::process_env::child_path_override(),
            _guard.before,
            "nothing adopted, nothing changed"
        );
        // Adopted once; since then, another folder of the same name was made
        // by somebody who lets everyone write to it (another user's `/tmp/x`
        // after a reboot, say), and one adopted folder now leads elsewhere
        // through a link. Only the one still the user's comes back.
        let reclaimed = scratch.0.join("tmp/x");
        std::fs::create_dir_all(&reclaimed).expect("dir");
        std::fs::set_permissions(&reclaimed, std::fs::Permissions::from_mode(0o777)).expect("chmod");
        let elsewhere = scratch.0.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).expect("dir");
        let repointed = scratch.0.join("repointed");
        std::os::unix::fs::symlink(&elsewhere, &repointed).expect("link");
        setup.db().kv_set(
            scope::SETUP,
            OVERLAY_KEY,
            &serde_json::to_string(&[reclaimed.to_string_lossy(), repointed.to_string_lossy(), picked.to_string_lossy()]).unwrap(),
        );
        let before = crate::process_env::child_path();
        setup.apply_overlay();
        let after = crate::process_env::child_path();
        assert_eq!(after, format!("{before}:{}", picked.display()));
        assert_eq!(setup.overlay(), [picked]);
    }

    /// What the app's own window sends: from loopback, signed in, to and from
    /// `127.0.0.1:<port>`.
    fn window(port: u16) -> Caller {
        Caller {
            peer: Some("127.0.0.1:50123".parse().expect("address")),
            identity: Some(AuthIdentity {
                username: "ada".to_string(),
                token_sha256: None,
            }),
            origin: Some(format!("http://127.0.0.1:{port}")),
            host: Some(format!("127.0.0.1:{port}")),
        }
    }

    #[tokio::test]
    async fn only_the_app_s_own_window_counts_as_the_app() {
        let home = tempfile::tempdir().expect("tempdir");
        let mut state = crate::api::test_support::state(home.path()).await;
        let port = 58123;
        assert!(app_caller(&state, &window(port)).is_err(), "not run by the desktop app");
        state.desktop_app = true;
        assert!(app_caller(&state, &window(port)).is_err(), "before the server listens, nothing is its window");
        state.setup.set_listen_port(port);

        assert!(app_caller(&state, &window(port)).is_ok());
        for peer in ["[::1]:50123", "[::ffff:127.0.0.1]:50123"] {
            let caller = Caller {
                peer: Some(peer.parse().expect("address")),
                ..window(port)
            };
            assert!(app_caller(&state, &caller).is_ok(), "{peer}");
        }

        let refused = |caller: Caller, why: &str| {
            let error = app_caller(&state, &caller).expect_err(why);
            assert_eq!(error.status(), StatusCode::FORBIDDEN, "{why}");
            assert_eq!(error.body()["error"], "app_only", "{why}");
        };
        refused(
            Caller {
                peer: Some("192.168.1.20:50123".parse().unwrap()),
                ..window(port)
            },
            "a browser on the LAN",
        );
        refused(Caller { peer: None, ..window(port) }, "no peer is not a local peer");
        refused(Caller { identity: None, ..window(port) }, "the app's window is always signed in");
        refused(
            Caller {
                identity: Some(AuthIdentity {
                    username: "cloud:d-abc".to_string(),
                    token_sha256: None,
                }),
                ..window(port)
            },
            "a linked device's request arrives over loopback and is still not this device",
        );
        refused(
            Caller {
                origin: Some("http://evil.example:58123".into()),
                host: Some("evil.example:58123".into()),
                ..window(port)
            },
            "DNS rebinding: a page elsewhere whose name now points at 127.0.0.1",
        );
        refused(
            Caller {
                origin: Some("http://127.0.0.1:9000".into()),
                host: Some("127.0.0.1:9000".into()),
                ..window(port)
            },
            "a browser through `ssh -R 9000:127.0.0.1:58123`, arriving over loopback",
        );
        refused(
            Caller {
                origin: Some("https://mac.tail1234.ts.net".into()),
                host: Some("mac.tail1234.ts.net".into()),
                ..window(port)
            },
            "a browser through `tailscale serve`",
        );
        refused(
            Caller {
                origin: Some("http://localhost:58123".into()),
                ..window(port)
            },
            "made by a page somewhere else",
        );
        refused(Caller { origin: None, ..window(port) }, "an action without its Origin");

        // The report's `canInstall`: a page's own GET says no Origin, and
        // needs none; everything else still holds.
        assert!(local_app_request(&state, &Caller { origin: None, ..window(port) }));
        assert!(!local_app_request(
            &state,
            &Caller {
                origin: None,
                host: Some("evil.example:58123".into()),
                ..window(port)
            }
        ));
        assert!(!local_app_request(&state, &Caller { identity: None, ..window(port) }));
    }

    /// Through the real router and its guard, from a real loopback socket.
    /// Nothing here reaches a command: every request is refused before one.
    #[tokio::test]
    async fn the_install_routes_refuse_everything_but_the_app_and_the_whitelist() {
        let client = reqwest::Client::new();
        // `token`: the bearer the app's window holds; the window's page sends
        // its own address as `Origin`, and reqwest sends `Host` from the URL.
        let send = |addr: SocketAddr, path: &'static str, body: serde_json::Value, token: Option<String>, origin: bool| {
            let client = client.clone();
            async move {
                let mut request = client.post(format!("http://{addr}{path}")).json(&body);
                if let Some(token) = token {
                    request = request.bearer_auth(token);
                }
                if origin {
                    request = request.header("Origin", format!("http://{addr}"));
                }
                let response = request.send().await.expect("answered");
                let status = response.status().as_u16();
                (status, response.json::<serde_json::Value>().await.unwrap_or_default())
            }
        };
        let routes = [
            ("/api/setup/tools/claude/install", serde_json::json!({"method": "native"})),
            ("/api/setup/tools/codex/update", serde_json::json!({"method": "npm"})),
            (
                "/api/setup/tools/codex/login",
                serde_json::json!({"mode": "apikey", "apiKey": "sk-proj-never-used-0123456789"}),
            ),
            ("/api/setup/tools/claude/use", serde_json::json!({"path": "/usr/bin/true"})),
            ("/api/setup/git/install", serde_json::json!({})),
            ("/api/setup/tools/rescan", serde_json::json!({"patient": true})),
        ];

        let ordinary = tempfile::tempdir().expect("tempdir");
        let (state, addr) = crate::api::test_support::served(ordinary.path()).await;
        state.drive.auth.users.add("ada", "correct horse battery staple").await.expect("user");
        let token = state.drive.auth.sessions.issue("ada", None).await.expect("session").token;
        for (path, body) in routes.clone() {
            let (status, body) = send(addr, path, body, Some(token.clone()), true).await;
            assert_eq!(status, 403, "{path}: a server the app does not run installs nothing, nor rewrites its PATH");
            assert_eq!(body["error"], "app_only", "{path}");
        }

        let app = tempfile::tempdir().expect("tempdir");
        let (state, addr) = crate::api::test_support::served_with(app.path(), |state| state.desktop_app = true).await;
        state.drive.auth.users.add("ada", "correct horse battery staple").await.expect("user");
        let token = state.drive.auth.sessions.issue("ada", None).await.expect("session").token;
        for (path, body) in routes {
            let (status, answer) = send(addr, path, body, Some(token.clone()), false).await;
            assert_eq!(
                (status, answer["error"].as_str()),
                (403, Some("app_only")),
                "{path}: an action has to say the app's page made it"
            );
        }
        let post = |addr: SocketAddr, path: &'static str, body: serde_json::Value| send(addr, path, body, Some(token.clone()), true);
        let (status, body) = post(
            addr,
            "/api/setup/tools/claude/install",
            serde_json::json!({"method": "curl https://example.com/x.sh | sh"}),
        )
        .await;
        assert_eq!((status, body["error"].as_str()), (400, Some("unknown_method")), "only the whitelist runs");
        let (status, body) = post(addr, "/api/setup/tools/claude/update", serde_json::json!({"method": "apple"})).await;
        assert_eq!((status, body["error"].as_str()), (400, Some("unknown_method")));
        let (status, body) = post(addr, "/api/setup/tools/gemini/install", serde_json::json!({"method": "native"})).await;
        assert_eq!((status, body["error"].as_str()), (400, Some("unknown_tool")));
        let (status, body) = post(addr, "/api/setup/tools/codex/login", serde_json::json!({"mode": "password"})).await;
        assert_eq!((status, body["error"].as_str()), (400, Some("unknown_method")));
        let (status, body) = post(addr, "/api/setup/tools/codex/login", serde_json::json!({"mode": "apikey"})).await;
        assert_eq!((status, body["error"].as_str()), (400, Some("api_key_required")));
    }

    /// The credential paths the guide must never name outside a comment.
    const CREDENTIAL_FRAGMENTS: [&str; 6] = [
        ".credentials.json",
        ".claude.json",
        ".codex/auth.json",
        "auth.json",
        "find-generic-password",
        "Claude Code-credentials",
    ];

    /// Every non-comment line of `code` that names a credential path.
    fn credential_mentions(code: &str) -> Vec<String> {
        code.lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .filter(|line| CREDENTIAL_FRAGMENTS.iter().any(|fragment| line.contains(fragment)))
            .map(str::to_string)
            .collect()
    }

    /// The source of `fn name` in `source`: from its signature to the brace
    /// that closes its body.
    fn function_source<'a>(source: &'a str, name: &str) -> &'a str {
        let start = [format!("fn {name}("), format!("fn {name}<")]
            .iter()
            .find_map(|signature| source.find(signature.as_str()))
            .unwrap_or_else(|| panic!("fn {name} is where the scan expects it"));
        let open = start + source[start..].find('{').expect("a body");
        let mut depth = 0usize;
        for (offset, c) in source[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &source[start..open + offset + 1];
                    }
                }
                _ => {}
            }
        }
        panic!("fn {name} ends")
    }

    /// The module promises it never opens a credential file. Words in comments
    /// are how the promise is written down; anywhere else, a fragment of one of
    /// these paths is a read waiting to happen. The promise covers what the
    /// module calls into, too: the `claude auth status` parser, the plan label,
    /// the codex app-server conversation and the terminal-text cleaner.
    #[test]
    fn no_source_the_module_runs_names_a_credential_path() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut checked = 0;
        for entry in std::fs::read_dir(src.join("setup")).expect("src/setup") {
            let path = entry.expect("entry").path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("source");
            // The tests below spell the fragments out to check for them; the
            // scan stops where a file's tests begin.
            let code = source.split("#[cfg(test)]").next().unwrap_or_default();
            assert_eq!(credential_mentions(code), Vec::<String>::new(), "{}", path.display());
            checked += 1;
        }
        assert!(checked >= 6, "the scan found the module's files");

        for (file, functions) in [
            ("accounts/mod.rs", &["parse_auth_status"][..]),
            ("text.rs", &["strip_ansi"][..]),
            ("quota/mod.rs", &["claude_plan_label"][..]),
            ("quota/codex.rs", &["app_server_call", "ask", "send", "reply"][..]),
            ("process_env.rs", &["apply_env"][..]),
        ] {
            let source = std::fs::read_to_string(src.join(file)).expect(file);
            for name in functions {
                let body = function_source(&source, name);
                assert_eq!(credential_mentions(body), Vec::<String>::new(), "{file}: fn {name}");
            }
        }
        assert_eq!(
            credential_mentions("let path = home.join(\".codex/auth.json\");"),
            ["let path = home.join(\".codex/auth.json\");"],
            "the scan would see one"
        );
    }
}
