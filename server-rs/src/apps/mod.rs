//! Embedded app instances: one VS Code server per app, framed by the web UI.
//!
//! Port of the former TypeScript server's `apps.ts` (the supervision) and the
//! three `/api/apps` routes in the former TypeScript server's `app.ts`. The
//! transport half — the `/embed/{id}/*` reverse proxy the iframe actually talks
//! to — is [`embed`], and is deliberately not part of the API surface: the
//! TypeScript mounts it on the bare node server rather than inside hono, so it
//! appears in no spec.
//!
//! The shape here is one supervisor task per instance, owning the
//! [`tokio::process::Child`], plus a [`std::sync::Mutex`]-guarded list that is
//! the whole of the shared state. The list doubles as the port reservation: a
//! port is claimed under the same lock that scans for a free one, which closes
//! a race the TypeScript still has (`findFreePort` awaits per-port probes, so
//! two overlapping creates there can pick the same port).
//!
//! # Surviving the server
//!
//! An app is a *detached* process: its own process group, `kill_on_drop(false)`,
//! stdio to a log file rather than to a pipe this server owns. So a restart —
//! deploy, crash, `SIGKILL` — leaves the user's editor running, with its unsaved
//! buffers and its language servers, and the next boot picks it back up. Which
//! makes three things load-bearing:
//!
//! * **[`record`]** — the `apps` table in the instance database is the only
//!   thing that says an app was ours. Nothing else distinguishes a `code-server`
//!   we started from one the user started by hand.
//! * **[`Registry::readopt_all`]** — the boot's probe-and-adopt: alive *and*
//!   answering means adopt, and every other combination is cleaned up rather
//!   than guessed at.
//! * **[`crate::procs`]** — the guard in front of every signal. A recorded pid
//!   may have been recycled by the kernel, and killing a stranger is far worse
//!   than leaking an editor.
//!
//! Shutdown therefore *detaches* rather than kills ([`Registry::shutdown_all`]),
//! exactly as the web terminals do one layer down.

pub mod embed;
pub mod record;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use gotcha::axum::body::Body;
use gotcha::axum::http::{Request, StatusCode};
use gotcha::{api, Json, Schematic, State};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::oneshot;

use crate::api::error::ApiError;
use crate::api::extract::{ApiJson, ApiPath};
use crate::api::{ApiRouter, AppState, SendResult};
use crate::instance::InstanceDirs;
use crate::openapi_ext::ExtraResponses;
use crate::process_env::{self, Env};
use crate::procs;
use crate::sessions::{iso8601, Paths};

use record::AppRecord;

/// The loopback ports an app may be given, inclusive.
const PORT_RANGE: std::ops::RangeInclusive<u16> = 8710..=8899;
/// How long a fresh instance has to answer `/healthz`.
const READY_TIMEOUT: Duration = Duration::from_secs(25);
/// How long each readiness probe waits before being abandoned.
const READY_PROBE_TIMEOUT: Duration = Duration::from_millis(1500);
/// How long the readiness loop sleeps between probes.
const READY_POLL: Duration = Duration::from_millis(400);
/// How long an exited instance stays listed, so a client that is mid-poll sees
/// it disappear rather than blink out under a request in flight.
const REAP_AFTER: Duration = Duration::from_secs(5);
/// How often an *adopted* app's pid is checked; see [`watch`].
const WATCH_POLL: Duration = Duration::from_secs(5);
/// How much of a failing instance's log is read back.
const LOG_CAP: usize = 4096;
/// How much of it is quoted in the error.
const LOG_TAIL: usize = 300;
/// The most of a `/healthz` answer that is read before judging it.
const HEALTH_CAP: usize = 64 * 1024;
/// The path both probes ask on.
const HEALTH: &str = "/healthz";
/// The program an app of type `vscode` runs.
const COMMAND: &str = "code-server";
/// The `error` of the `409` for a `vscode` app when [`COMMAND`] is not on the
/// `PATH` it would be started with.
pub const CODE_SERVER_MISSING: &str = "code_server_missing";

/// One running app, as the API describes it.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    /// Opaque id; also the `/embed/{id}/` path segment.
    pub id: String,
    /// Which kind of app this is.
    #[serde(rename = "type")]
    pub kind: AppType,
    /// Display name.
    pub title: String,
    /// When it was started, ISO-8601.
    pub created_at: String,
}

/// The kinds of app that can be started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum AppType {
    /// A `code-server` instance.
    Vscode,
}

impl AppType {
    /// The program this kind of app runs, and so the name its command line must
    /// carry before anything of ours signals its pid.
    fn program(self) -> &'static str {
        match self {
            AppType::Vscode => COMMAND,
        }
    }

    /// The `error` a `409` carries when [`Self::program`] is not installed.
    fn missing(self) -> &'static str {
        match self {
            AppType::Vscode => CODE_SERVER_MISSING,
        }
    }
}

/// The `GET /api/apps` response.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct AppList {
    /// Every instance whose process is still running.
    pub apps: Vec<AppInfo>,
}

/// The `POST /api/apps` body.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Schematic)]
pub struct AppCreate {
    /// Which kind of app to start.
    #[serde(rename = "type")]
    pub kind: AppType,
}

/// Why an app could not be started.
///
/// For every failure but one the message is the contract: the route answers
/// `409` with it verbatim, exactly as the TypeScript's
/// `err instanceof AppError ? err.message : …` does. The one is a program that
/// is not installed, which is also the commonest — `code-server` is an optional
/// prerequisite nothing installs for the user — and the only one a person fixes
/// on the device rather than by trying again; see [`AppError::into_api`].
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// The program an app of this type runs is not on the `PATH` it would be
    /// started with.
    #[error("{} is not installed on this device, or is not on the server's PATH", .0.program())]
    Missing(AppType),
    /// Anything else, said as it is.
    #[error("{0}")]
    Failed(String),
}

impl AppError {
    fn new(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
    }

    /// What the route says when the failure was not one this module named —
    /// the TypeScript's fall-through for a non-`AppError` throw.
    fn opaque() -> Self {
        Self::new("cannot start app")
    }

    /// The `409` this failure is answered with.
    ///
    /// A missing program is a token beside a sentence,
    /// `{"error":"code_server_missing","hint":…}` — the shape of every refusal
    /// a client is meant to act on (`attachment_missing`, `provider_mismatch`)
    /// — so the web app can say what to install, in the reader's language and
    /// with a link, instead of quoting an `os error 2`. Everything else is the
    /// sentence alone, as it always was.
    fn into_api(self) -> ApiError {
        let message = self.to_string();
        match self {
            AppError::Missing(kind) => ApiError::with_body(
                StatusCode::CONFLICT,
                serde_json::json!({ "error": kind.missing(), "hint": message }),
            ),
            AppError::Failed(_) => ApiError::conflict(message),
        }
    }
}

/// What a spawn of `kind`'s program that failed with `error` says.
///
/// `NotFound` is the program missing after all: [`Registry::create_app`] looked
/// on the same `PATH` a moment earlier, but it can be uninstalled in between,
/// and an `exec` whose `#!` interpreter is gone fails the same way — a broken
/// install, which is close enough to none. Anything else (a permission, a limit
/// on processes) is said as it is.
fn spawn_error(kind: AppType, error: &std::io::Error) -> AppError {
    if error.kind() == std::io::ErrorKind::NotFound {
        return AppError::Missing(kind);
    }
    AppError::new(format!("cannot launch {}: {error}", kind.program()))
}

/// Per-app state (settings DB, "last opened folder", UI layout) and logs, under
/// the directory this server exclusively owns.
///
/// Without a directory of its own every `code-server` shares one state directory
/// and instances hijack each other's folder on reload. Hanging the root off the
/// *instance* rather than off `$HOME` settles the same question one level up: two
/// servers on one home are two instances by construction, holding two different
/// locks over two disjoint trees, so neither can be handed a path the other has
/// an editor sitting on. This belongs beside the other per-instance paths in
/// [`InstanceDirs`] and is derived from its root only because that file is not
/// this change's to edit.
fn app_data_root(instance: &InstanceDirs) -> PathBuf {
    instance.root().join("apps")
}

/// Extensions stay in `code-server`'s default directory, so they are installed
/// once and appear in every instance.
fn extensions_dir(home: &Path) -> PathBuf {
    home.join(".local").join("share").join("code-server").join("extensions")
}

/// The environment an app is spawned with.
///
/// `code-server` lets `PORT`/`HOST` override `--bind-addr`, so ours must not
/// leak in — the rest is [`process_env::clean_env`]'s scrubbed environment.
fn app_env() -> Env {
    let mut env = process_env::clean_env_bare();
    env.remove("PORT");
    env.remove("HOST");
    env
}

/// How an instance came to be in the list, and so whether its pid may be
/// signalled on sight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// This process spawned it and still holds its [`tokio::process::Child`].
    /// The pid cannot have been recycled: it is unreaped until the supervisor
    /// says otherwise, and the supervisor sets `exited` the instant it does.
    Spawned,
    /// Read out of the instance database at boot. The pid is a claim, not a
    /// handle.
    Adopted,
}

/// One live (or just-exited) instance.
struct Instance {
    info: AppInfo,
    port: u16,
    /// Set by the supervisor (or [`watch`]) the moment the process is gone.
    exited: Arc<AtomicBool>,
    /// `None` for an upstream registered by [`Registry::register_upstream`],
    /// which this process did not start and must not signal.
    pid: Option<u32>,
    /// What `pid` meant when it was learned, so a later boot can tell the app
    /// apart from whatever the kernel handed that number next. `None` for an
    /// instance with no process, and for one adopted from a record written
    /// before the guard existed.
    pid_token: Option<String>,
    /// The folder the app was opened on, carried so a later boot can put the
    /// record back the way it found it.
    cwd: Option<String>,
    origin: Origin,
}

impl Instance {
    /// What the record should say about this instance, if anything.
    ///
    /// An instance with no pid is a registered upstream — a test seam with no
    /// process behind it — and an exited one is a pid a later boot must not be
    /// handed. Neither belongs in a record whose whole purpose is to be acted
    /// on.
    fn record(&self) -> Option<AppRecord> {
        let pid = self.pid?;
        if self.exited.load(Ordering::SeqCst) {
            return None;
        }
        Some(AppRecord {
            id: self.info.id.clone(),
            kind: self.info.kind,
            pid,
            port: self.port,
            pid_token: self.pid_token.clone(),
            cwd: self.cwd.clone(),
            started_at: self.info.created_at.clone(),
        })
    }
}

/// What is answering on a recorded port when a boot goes looking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Occupant {
    /// Something that answers `/healthz` the way `code-server` does.
    Ours,
    /// Something is listening, but it is not an app of ours. It may be an
    /// unrelated service that took the port while we were gone.
    Foreign,
    /// Nothing is listening at all.
    Nobody,
}

/// What became of a record between one boot and the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Survivor {
    /// Pid alive, port answering: the app is still there.
    Alive,
    /// Pid alive, port dead: a half-dead app serving nobody.
    Stranded,
    /// The port belongs to something that is not this app. Hands off both.
    Usurped,
    /// Nothing is left of it.
    Dead,
}

/// Every app this server started or adopted, and the ports they listen on.
pub struct Registry {
    home: PathBuf,
    /// The instance directory this server owns; `apps.json` and the per-app
    /// logs live in it.
    instance: Arc<InstanceDirs>,
    instances: Mutex<Vec<Instance>>,
    /// Set by [`Registry::shutdown_all`]. Once this server has let go of its
    /// apps, nothing it does afterwards may rewrite the file the next boot
    /// adopts from — a late supervisor callback would otherwise erase a record
    /// for an app that is still running.
    detached: AtomicBool,
    counter: AtomicU64,
    client: embed::HttpClient,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry").field("home", &self.home).finish_non_exhaustive()
    }
}

impl Registry {
    /// A registry rooted at `paths`' home directory, owning `instance`.
    ///
    /// Nothing is read or swept here. Both are [`Registry::readopt_all`]'s job,
    /// which the application calls only for a server that holds the instance
    /// lock — the state of a running app is not a throwaway server's to touch.
    pub fn new(paths: Arc<Paths>, instance: Arc<InstanceDirs>) -> Arc<Self> {
        Arc::new(Self {
            home: paths.home.clone(),
            instance,
            instances: Mutex::new(Vec::new()),
            detached: AtomicBool::new(false),
            counter: AtomicU64::new(0),
            client: embed::client(),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Instance>> {
        self.instances.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Where an app's stdout and stderr go.
    ///
    /// A file rather than `/dev/null`, because a `code-server` that will not
    /// start says why exactly once and this is the only place it can say it: the
    /// process is detached, so there is no pipe of ours for it to talk into, and
    /// it may fail long after the request that created it has been answered.
    fn log_path(&self, id: &str) -> PathBuf {
        app_data_root(&self.instance).join(format!("{id}.log"))
    }

    /// The `--user-data-dir` an app is given, beside its log.
    fn app_dir(&self, id: &str) -> PathBuf {
        app_data_root(&self.instance).join(id)
    }

    /// Rewrite `apps.json` from `instances`.
    ///
    /// Called with the list's lock held, which is what makes the rewrite
    /// serialisable: two concurrent changes cannot interleave into a file that
    /// describes neither. The write is blocking, and deliberately so — it is a
    /// few hundred bytes, and doing it anywhere else would open a window in
    /// which the file disagrees with the list.
    fn persist(&self, instances: &[Instance]) {
        if self.detached.load(Ordering::SeqCst) {
            return;
        }
        let records: Vec<AppRecord> = instances.iter().filter_map(Instance::record).collect();
        record::save(&self.instance.apps_file(), &records);
    }

    /// Rewrite `apps.json` from the current list.
    fn refresh(&self) {
        let instances = self.lock();
        self.persist(&instances);
    }

    /// Every instance whose process is still running.
    pub fn list(&self) -> Vec<AppInfo> {
        self.lock()
            .iter()
            .filter(|instance| !instance.exited.load(Ordering::SeqCst))
            .map(|instance| instance.info.clone())
            .collect()
    }

    /// The loopback port `id` is reachable on, or `None` if it is unknown or
    /// its process is gone. The one question [`embed`] asks.
    pub fn port(&self, id: &str) -> Option<u16> {
        self.lock()
            .iter()
            .find(|instance| instance.info.id == id && !instance.exited.load(Ordering::SeqCst))
            .map(|instance| instance.port)
    }

    /// Whether `id` is still listed at all, exited or not.
    fn holds(&self, id: &str) -> bool {
        self.lock().iter().any(|instance| instance.info.id == id)
    }

    /// Claim a free port and list the instance against it, under one lock.
    ///
    /// Listing before the spawn is what makes the reservation real: a second
    /// `create_app` racing this one sees the port taken. Adopted instances are
    /// in the same list, so an app that survived a restart keeps its port too.
    fn reserve(&self, kind: AppType) -> Result<(AppInfo, u16, Arc<AtomicBool>), AppError> {
        let mut instances = self.lock();
        let used: HashSet<u16> = instances.iter().map(|instance| instance.port).collect();
        let port = PORT_RANGE
            .clone()
            .find(|port| !used.contains(port) && std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok())
            .ok_or_else(|| AppError::new("no free port for a new app"))?;

        let counter = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        let info = AppInfo {
            id: uuid::Uuid::new_v4().to_string(),
            kind,
            title: format!("vscode {counter}"),
            created_at: iso8601(std::time::SystemTime::now()),
        };
        let exited = Arc::new(AtomicBool::new(false));
        instances.push(Instance {
            info: info.clone(),
            port,
            exited: exited.clone(),
            pid: None,
            pid_token: None,
            cwd: Some(self.home.to_string_lossy().into_owned()),
            origin: Origin::Spawned,
        });
        Ok((info, port, exited))
    }

    /// Attach the spawned child's pid to a reserved instance, and write it down.
    ///
    /// The record lands here — after the spawn, before the app is ready —
    /// because this is the first moment there is a pid to record and the last
    /// moment before a crash could leave one running with nothing pointing at
    /// it. An app that never becomes ready is removed again by `create_app`.
    fn attach_pid(&self, id: &str, pid: Option<u32>) {
        let mut instances = self.lock();
        if let Some(instance) = instances.iter_mut().find(|instance| instance.info.id == id) {
            instance.pid = pid;
            // Taken here rather than before the spawn because there is no pid to
            // take it for until now. The window in between is a child that
            // exited and had its number recycled between `spawn` and this line,
            // which is not reachable in practice.
            instance.pid_token = pid.and_then(procs::start_token);
        }
        self.persist(&instances);
    }

    /// Take an instance out of the list, and out of `apps.json` with it.
    fn take(&self, id: &str) -> Option<Instance> {
        let mut instances = self.lock();
        let position = instances.iter().position(|instance| instance.info.id == id)?;
        let instance = instances.remove(position);
        self.persist(&instances);
        Some(instance)
    }

    /// Drop an instance from the list without signalling anything.
    fn forget(&self, id: &str) {
        let mut instances = self.lock();
        instances.retain(|instance| instance.info.id != id);
        self.persist(&instances);
    }

    /// Start an app and wait for it to answer.
    pub async fn create_app(self: &Arc<Self>, kind: AppType) -> Result<AppInfo, AppError> {
        // One arm today; `match` rather than `let` so a second `AppType` cannot
        // be added without this deciding what to run for it.
        let AppType::Vscode = kind;
        // Looked up before a port is reserved or a directory made, on the very
        // `PATH` the spawn below is handed: a program that is not there is the
        // usual way for this to fail, and this is what lets the answer say so
        // rather than "No such file or directory". Asked on every create, not
        // once at boot, so an editor installed while the server runs works on
        // the next click with no restart.
        let env = app_env();
        if process_env::resolve_on_path(kind.program(), &env).is_none() {
            tracing::warn!(
                program = kind.program(),
                path = env.get("PATH").map_or("", String::as_str),
                "cannot start an app: its program is not on the PATH apps are started with"
            );
            return Err(AppError::Missing(kind));
        }
        let (info, port, exited) = self.reserve(kind)?;
        match self.launch(&info, port, exited, &env).await {
            Ok(()) => Ok(info),
            Err(error) => {
                // A launch that failed *after* the spawn may still have a
                // process attached — a `code-server` that came up too slowly is
                // the everyday case — so this is the full stop, not a forget.
                self.close_app(&info.id).await;
                Err(error)
            }
        }
    }

    /// Spawn the child with `env` and wait for `/healthz`, leaving the instance
    /// listed.
    async fn launch(self: &Arc<Self>, info: &AppInfo, port: u16, exited: Arc<AtomicBool>, env: &Env) -> Result<(), AppError> {
        let user_data_dir = self.app_dir(&info.id);
        tokio::fs::create_dir_all(&user_data_dir).await.map_err(|error| {
            tracing::warn!(%error, dir = %user_data_dir.display(), "cannot create the app's state directory");
            AppError::opaque()
        })?;

        let mut command = Command::new(COMMAND);
        command.args([
            "--bind-addr".into(),
            format!("127.0.0.1:{port}"),
            "--auth".into(),
            "none".into(),
            "--disable-telemetry".into(),
            "--disable-update-check".into(),
            "--disable-workspace-trust".into(),
            "--user-data-dir".into(),
            user_data_dir.to_string_lossy().into_owned(),
            "--extensions-dir".into(),
            extensions_dir(&self.home).to_string_lossy().into_owned(),
            self.home.to_string_lossy().into_owned(),
        ]);
        process_env::apply_env(&mut command, env);

        // Every flag below is what makes the app outlive this server.
        // `process_group(0)` takes it out of the server's group, so a `kill`
        // aimed at the server — or a shell signalling the whole group on
        // Ctrl-C — does not take the user's editor with it. `kill_on_drop`
        // keeps tokio from tidying it away when the handle goes. And the stdio
        // is a file, so nothing the app writes depends on a pipe this process
        // holds open: the TypeScript piped stdout and never read it, which
        // fills the pipe and stalls a chatty `code-server`.
        let (out, err) = self.open_log(&info.id);
        command.stdin(std::process::Stdio::null());
        command.stdout(out);
        command.stderr(err);
        command.kill_on_drop(false);
        command.process_group(0);

        let child = command.spawn().map_err(|error| spawn_error(info.kind, &error))?;
        self.attach_pid(&info.id, child.id());

        let (exit_tx, exit_rx) = oneshot::channel::<Option<i32>>();
        tokio::spawn(supervise(child, exited, exit_tx, Arc::clone(self), info.id.clone(), user_data_dir));

        let log = self.log_path(&info.id);
        tokio::select! {
            ready = self.wait_ready(port) => ready,
            code = exit_rx => {
                let code = code.ok().flatten().map_or("null".to_string(), |code| code.to_string());
                Err(AppError::new(format!("{COMMAND} exited early (code {code}): {}", log_tail(&log))))
            }
        }
    }

    /// The two stdio handles an app's log file is opened as.
    ///
    /// Both ends of the same freshly truncated file — the id is a uuid, so
    /// there is never anything there worth keeping — falling back to `/dev/null`
    /// rather than failing the launch, because a missing log costs debugging
    /// information and a refused launch costs the user their editor.
    fn open_log(&self, id: &str) -> (std::process::Stdio, std::process::Stdio) {
        let path = self.log_path(id);
        let opened = path
            .parent()
            .ok_or_else(|| std::io::Error::other("no parent"))
            .and_then(std::fs::create_dir_all)
            .and_then(|()| std::fs::File::create(&path))
            .and_then(|file| Ok((file.try_clone()?, file)));
        match opened {
            Ok((out, err)) => (out.into(), err.into()),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "cannot open the app log; its output goes nowhere");
                (std::process::Stdio::null(), std::process::Stdio::null())
            }
        }
    }

    /// Poll `/healthz` until it answers something that is not a server error.
    ///
    /// Deliberately laxer than [`Registry::occupant`], which asks about a port
    /// nobody vouches for. Here the process is this server's own child and is
    /// watched for exit at the same time, so the only open question is *when* it
    /// is up — not *whether* the thing on the port is the thing we started.
    async fn wait_ready(&self, port: u16) -> Result<(), AppError> {
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
        let uri = format!("http://127.0.0.1:{port}{HEALTH}");
        while tokio::time::Instant::now() < deadline {
            let request = Request::builder().uri(&uri).body(Body::empty());
            if let Ok(request) = request {
                if let Ok(Ok(response)) = tokio::time::timeout(READY_PROBE_TIMEOUT, self.client.request(request)).await {
                    if response.status().as_u16() < 500 {
                        return Ok(());
                    }
                }
            }
            tokio::time::sleep(READY_POLL).await;
        }
        Err(AppError::new("app did not become ready in time"))
    }

    /// What is listening on a loopback port a record points at.
    ///
    /// `code-server` answers `/healthz` with a JSON object
    /// (`{"status":"alive","lastHeartbeat":…}`). Requiring *an object* rather
    /// than those exact keys is the deliberate middle: it tells an editor apart
    /// from the unrelated services that plausibly hold a port in this range —
    /// which answer HTML, a `404`, or nothing at all — without making adoption
    /// break the day `code-server` renames a field. And when it is wrong it is
    /// wrong in the safe direction: an unrecognised answer means "leave it
    /// alone", never "kill it".
    async fn occupant(&self, port: u16) -> Occupant {
        let uri = format!("http://127.0.0.1:{port}{HEALTH}");
        let Ok(request) = Request::builder().uri(&uri).body(Body::empty()) else {
            return Occupant::Foreign;
        };
        let response = match tokio::time::timeout(READY_PROBE_TIMEOUT, self.client.request(request)).await {
            Ok(Ok(response)) => response,
            // A refused connection is the one unambiguous answer: nothing holds
            // the port, so there is nothing to mistake for the app.
            Ok(Err(error)) if error.is_connect() => return Occupant::Nobody,
            // Anything else — a timeout, a socket that accepts and says
            // nothing, a protocol error — is *something*, and something that
            // will not answer is not an app of ours.
            _ => return Occupant::Foreign,
        };
        if response.status() != StatusCode::OK {
            return Occupant::Foreign;
        }
        let Ok(body) = gotcha::axum::body::to_bytes(Body::new(response.into_body()), HEALTH_CAP).await else {
            return Occupant::Foreign;
        };
        match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(serde_json::Value::Object(_)) => Occupant::Ours,
            _ => Occupant::Foreign,
        }
    }

    /// What became of one record: the pid and the port, asked separately.
    ///
    /// Separately because the interesting failures are the ones where the two
    /// disagree, and a single "is it up" would collapse them into each other.
    ///
    /// The pid question is *identity*, not liveness: a recycled number is alive
    /// and is not the app, and adopting on liveness alone would put a stranger's
    /// process in the list and eventually signal it. A record written before the
    /// token existed carries none and falls back to liveness, which is the same
    /// trade [`crate::inject::runs`] makes — a guard that cannot tell must not
    /// refuse, or every restart would throw away a live editor.
    async fn survivor(&self, record: &AppRecord) -> Survivor {
        match (
            procs::is_same_process(record.pid, record.pid_token.as_deref()),
            self.occupant(record.port).await,
        ) {
            (true, Occupant::Ours) => Survivor::Alive,
            (true, Occupant::Nobody) => Survivor::Stranded,
            // Either the pid was recycled and the port is a stranger's, or the
            // pid is gone and something else — even another `code-server` —
            // has the port. Nothing here is ours to adopt or to signal.
            (_, Occupant::Foreign) | (false, Occupant::Ours) => Survivor::Usurped,
            (false, Occupant::Nobody) => Survivor::Dead,
        }
    }

    /// Pick up every app in `apps.json` that is still there, and tidy the rest.
    ///
    /// Runs at boot, after the instance lock and before the listener exists: a
    /// `GET /api/apps` must never answer from a half-read record, and a server
    /// that does not own the instance must not touch another server's apps.
    ///
    /// The four outcomes, and why each does what it does:
    ///
    /// 1. **alive** — the pid is still the process the record was written for,
    ///    and the port answers like `code-server`. Adopt it; the `/embed` proxy
    ///    works again the moment it is in the list.
    /// 2. **stranded** — pid there, nothing on the port. The app is serving
    ///    nobody and holds a state directory; stop it, but only once
    ///    [`still_ours`] has agreed twice over that the pid is what it says.
    /// 3. **usurped** — something else holds the port. Drop the record and say
    ///    so loudly, but signal nothing: after a pid has been recycled, the
    ///    process at that number may be anything at all, and killing a
    ///    stranger's service is the worst thing this code could do.
    /// 4. **dead** — neither. Drop the record.
    pub async fn readopt_all(self: &Arc<Self>) -> usize {
        let mut records = record::load(&self.instance.apps_file());
        // Oldest first, so the titles come back in the order the user made them
        // — the counter is all an adopted app has to be named from.
        records.sort_by(|left, right| left.started_at.cmp(&right.started_at));

        let mut adopted: Vec<(AppRecord, Arc<AtomicBool>)> = Vec::new();
        for record in records {
            match self.survivor(&record).await {
                Survivor::Alive => {
                    let exited = Arc::new(AtomicBool::new(false));
                    let counter = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
                    let info = AppInfo {
                        id: record.id.clone(),
                        kind: record.kind,
                        title: format!("vscode {counter}"),
                        created_at: record.started_at.clone(),
                    };
                    tracing::info!(app = %record.id, pid = record.pid, port = record.port, "adopted a surviving app");
                    self.lock().push(Instance {
                        info,
                        port: record.port,
                        exited: exited.clone(),
                        pid: Some(record.pid),
                        pid_token: record.pid_token.clone(),
                        cwd: record.cwd.clone(),
                        origin: Origin::Adopted,
                    });
                    adopted.push((record, exited));
                }
                Survivor::Stranded => {
                    if still_ours(record.pid, record.pid_token.as_deref(), record.kind.program()).await {
                        tracing::info!(
                            app = %record.id,
                            pid = record.pid,
                            port = record.port,
                            "an app survived without its port; stopping it"
                        );
                        procs::terminate_group(record.pid);
                    } else {
                        tracing::warn!(
                            app = %record.id,
                            pid = record.pid,
                            "an app's pid is now something else entirely; leaving it alone"
                        );
                    }
                }
                Survivor::Usurped => tracing::warn!(
                    app = %record.id,
                    pid = record.pid,
                    port = record.port,
                    "the recorded port is held by something that is not this app; dropping the record and signalling nothing"
                ),
                Survivor::Dead => tracing::info!(app = %record.id, pid = record.pid, "an app did not survive the restart"),
            }
        }

        // One rewrite for the whole boot: whatever is in the list now is what
        // the *next* boot should find, and everything else is gone from it.
        self.refresh();
        let count = adopted.len();
        for (record, exited) in adopted {
            tokio::spawn(watch(Arc::downgrade(self), record, exited));
        }
        self.sweep_state().await;
        count
    }

    /// Delete the per-app state and logs of every app that is not running.
    ///
    /// This replaces the blanket wipe the constructor used to do at boot. With
    /// adoption, a live app's settings directory is not dead weight — it is the
    /// open folder, the layout and the undo history of an editor the user still
    /// has in front of them, and deleting it pulls that out from under them. The
    /// log of an app that is gone is kept until the next boot on purpose: it is
    /// where a `code-server` that refused to start said why.
    async fn sweep_state(&self) {
        let keep: HashSet<String> = self.lock().iter().map(|instance| instance.info.id.clone()).collect();

        // Everything under the instance directory was put there by a server
        // holding this instance's lock — this one, or the one before it — so an
        // entry that is not a live app is this instance's own litter. Sweeping
        // by what is *there* rather than by what the journal still remembers is
        // what stops an app whose record was dropped two boots ago leaking its
        // state directory for ever.
        let root = app_data_root(&self.instance);
        if let Ok(mut entries) = tokio::fs::read_dir(&root).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name();
                // `<id>` is an app's state; `<id>.log` is what it said on its
                // way out. Both belong to the app the id names, and go together.
                let Some(name) = name.to_str() else { continue };
                if keep.contains(name.strip_suffix(".log").unwrap_or(name)) {
                    continue;
                }
                let path = entry.path();
                let _ = match entry.file_type().await {
                    Ok(kind) if kind.is_dir() => tokio::fs::remove_dir_all(&path).await,
                    _ => tokio::fs::remove_file(&path).await,
                };
            }
        }
    }

    /// Stop an app. `false` means there was no such instance.
    pub async fn close_app(&self, id: &str) -> bool {
        let Some(instance) = self.take(id) else { return false };
        stop(&instance).await;
        true
    }

    /// Let go of every app, leaving every one of them running.
    ///
    /// Called from the shutdown supervisor, and the inversion is the point of
    /// the whole phase: this used to `SIGTERM` every `code-server`, because an
    /// orphan holding a loopback port was a leak. Now the app is *supposed* to
    /// outlive the server — that is what a restart-survivable editor means —
    /// and killing it would throw away exactly what the user restarted to keep.
    ///
    /// `apps.json` is deliberately not rewritten, and the `detached` flag makes
    /// sure nothing else rewrites it either: it is the record the next boot
    /// adopts from, and it is already correct.
    pub fn shutdown_all(&self) -> usize {
        self.detached.store(true, Ordering::SeqCst);
        let mut instances = self.lock();
        let count = instances.len();
        instances.clear();
        count
    }

    /// Kill every app, sweep its record, and return how many were signalled.
    ///
    /// Not part of shutdown, and deliberately not reachable from the API: it
    /// exists so a test binary leaves no `code-server` processes behind, which
    /// detaching by design would otherwise guarantee it did. `SIGKILL` and no
    /// grace period, because this may be the last thing the process does and
    /// there is nobody left to escalate.
    ///
    /// The start-time token is the only guard available here — there is no
    /// runtime left to await a `ps` on — but it is the exact one, and an
    /// unguarded `SIGKILL` at a recorded pid is precisely how a stranger's
    /// process gets killed by a test tidying up after itself.
    pub fn kill_all(&self) -> usize {
        let mut instances = self.lock();
        let mut killed = 0;
        for instance in instances.drain(..) {
            let Some(pid) = instance.pid.filter(|_| !instance.exited.load(Ordering::SeqCst)) else {
                continue;
            };
            if instance.origin == Origin::Adopted && !procs::is_same_process(pid, instance.pid_token.as_deref()) {
                tracing::warn!(app = %instance.info.id, pid, "refusing to kill an adopted pid that is no longer the app it was recorded as");
                continue;
            }
            procs::signal_group(pid, libc::SIGKILL);
            killed += 1;
        }
        // Straight past the `detached` guard: a test that killed everything
        // must not leave a record of it behind for the next server on this
        // instance directory.
        record::save(&self.instance.apps_file(), &[]);
        killed
    }

    /// Register an already-listening loopback upstream as an app.
    ///
    /// The `/embed` proxy only ever asks "which port does this id mean", so
    /// this is the seam a test uses to put a real HTTP+websocket server behind
    /// an app id without a `code-server` install. Nothing in the server itself
    /// calls it; the returned instance has no process, so it is never recorded
    /// in `apps.json`, and [`close_app`] and [`kill_all`] drop it from the list
    /// and signal nothing.
    ///
    /// [`close_app`]: Registry::close_app
    /// [`kill_all`]: Registry::kill_all
    pub fn register_upstream(&self, port: u16) -> AppInfo {
        let counter = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        let info = AppInfo {
            id: uuid::Uuid::new_v4().to_string(),
            kind: AppType::Vscode,
            title: format!("vscode {counter}"),
            created_at: iso8601(std::time::SystemTime::now()),
        };
        self.lock().push(Instance {
            info: info.clone(),
            port,
            exited: Arc::new(AtomicBool::new(false)),
            pid: None,
            pid_token: None,
            cwd: None,
            origin: Origin::Spawned,
        });
        info
    }
}

/// Whether `pid` is still the app it was recorded as, by both of the tests this
/// server has for it.
///
/// The start-time token is the exact one: two processes that shared a pid
/// necessarily started at different moments, so a token that disagrees is proof
/// of reuse. It is also the one that can abstain — a record written before the
/// guard existed carries none, and a platform may have none to give — and on its
/// own it would then wave a recycled pid straight through. The command line is
/// the weaker test but never abstains. So both are asked, and both must agree,
/// before anything of ours signals a process it did not spawn.
///
/// Asked in that order on purpose: a token that already disagrees settles it
/// without paying for a `ps`.
async fn still_ours(pid: u32, token: Option<&str>, program: &str) -> bool {
    procs::is_same_process(pid, token) && procs::is_running(pid, program).await
}

/// Ask an instance's process group to stop, if it is still ours to ask.
///
/// The guard is only for an *adopted* pid: one this process spawned is still an
/// unreaped child of ours, and a pid cannot be recycled before it is reaped.
async fn stop(instance: &Instance) {
    let Some(pid) = instance.pid else { return };
    if instance.exited.load(Ordering::SeqCst) {
        return;
    }
    if instance.origin == Origin::Adopted && !still_ours(pid, instance.pid_token.as_deref(), instance.info.kind.program()).await {
        tracing::warn!(
            app = %instance.info.id,
            pid,
            "refusing to signal an adopted pid that is no longer the app it was recorded as"
        );
        return;
    }
    procs::terminate_group(pid);
}

/// Own the child until it exits, then announce it and tidy up after it.
async fn supervise(
    mut child: tokio::process::Child,
    exited: Arc<AtomicBool>,
    exit: oneshot::Sender<Option<i32>>,
    registry: Arc<Registry>,
    id: String,
    user_data_dir: PathBuf,
) {
    let code = child.wait().await.ok().and_then(|status| status.code());
    exited.store(true, Ordering::SeqCst);
    // The record goes the moment the process does: a pid that has been reaped
    // is a pid the kernel may hand to somebody else.
    registry.refresh();
    // Nobody is listening once the instance is up; the failure path is.
    let _ = exit.send(code);
    tracing::info!(app = %id, ?code, "app exited");
    let _ = tokio::fs::remove_dir_all(&user_data_dir).await;
    tokio::time::sleep(REAP_AFTER).await;
    registry.forget(&id);
}

/// Notice when an *adopted* app exits.
///
/// A spawned app has [`supervise`] holding its `Child` and is told the instant
/// it dies. An adopted one is a number in a list: nothing wakes anybody when it
/// goes, so `GET /api/apps` would advertise a dead editor until the next restart
/// unless something looks. A `Weak` so a shut-down registry is not kept alive by
/// its own watchers.
async fn watch(registry: Weak<Registry>, record: AppRecord, exited: Arc<AtomicBool>) {
    let AppRecord { id, pid, ref pid_token, .. } = record;
    loop {
        tokio::time::sleep(WATCH_POLL).await;
        let Some(registry) = registry.upgrade() else { return };
        // Closed, or the server let go of it: either way there is nothing here
        // left to watch.
        if !registry.holds(&id) || exited.load(Ordering::SeqCst) {
            return;
        }
        // Identity rather than liveness, for the same reason adoption asks it:
        // a pid the kernel has handed on is not this app still running, and
        // reporting it as one would advertise a dead editor for ever.
        if procs::is_same_process(pid, pid_token.as_deref()) {
            continue;
        }
        exited.store(true, Ordering::SeqCst);
        registry.refresh();
        tracing::info!(app = %id, pid, "an adopted app exited");
        tokio::time::sleep(REAP_AFTER).await;
        registry.forget(&id);
        return;
    }
}

/// The last [`LOG_TAIL`] characters of an app's log, for the error that says why
/// it would not start.
fn log_tail(path: &Path) -> String {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let length = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    // Only the end of it: a `code-server` that logged for an hour before falling
    // over must not be read into memory to quote three hundred characters.
    if file.seek(SeekFrom::Start(length.saturating_sub(LOG_CAP as u64))).is_err() {
        return String::new();
    }
    let mut raw = Vec::new();
    if file.read_to_end(&mut raw).is_err() {
        return String::new();
    }
    // Lossy, and the seek above may well have landed mid-character: this is a
    // diagnostic quote, not a transcript.
    let text = String::from_utf8_lossy(&raw);
    let characters: Vec<char> = text.trim_end().chars().collect();
    characters[characters.len().saturating_sub(LOG_TAIL)..].iter().collect()
}

/// Running embedded app instances.
#[api(id = "list_apps", group = "apps", summary = "List apps")]
pub async fn list_apps(State(state): State<AppState>) -> Json<AppList> {
    Json(AppList { apps: state.apps.list() })
}

/// Start an app instance and wait for it to come up.
#[api(id = "create_app", group = "apps", summary = "Create an app")]
pub async fn create_app(State(state): State<AppState>, ApiJson(body): ApiJson<AppCreate>) -> Result<Json<AppInfo>, ApiError> {
    state
        .apps
        .create_app(body.kind)
        .await
        .map(Json)
        .map_err(AppError::into_api)
}

/// Stop an app instance.
#[api(id = "close_app", group = "apps", summary = "Close an app")]
pub async fn close_app(State(state): State<AppState>, ApiPath(id): ApiPath<String>) -> Result<Json<SendResult>, ApiError> {
    if !state.apps.close_app(&id).await {
        return Err(ApiError::not_found("app not found"));
    }
    Ok(Json(SendResult { accepted: true }))
}

/// Everything gotcha cannot derive about these operations.
pub fn describe(reg: &mut ExtraResponses) {
    reg.describe("GET", "/api/apps", 200, "Running embedded app instances")
        .describe("POST", "/api/apps", 200, "App started; embed via /embed/{id}/")
        .error(
            "POST",
            "/api/apps",
            409,
            "App could not be started; `code_server_missing` (with a `hint`) when code-server is not on the server's PATH",
        )
        .describe("DELETE", "/api/apps/{id}", 200, "App closed")
        .error("DELETE", "/api/apps/{id}", 404, "App not found");
}

/// Register every app route.
pub fn routes(router: ApiRouter) -> ApiRouter {
    router
        .get("/api/apps", list_apps)
        .post("/api/apps", create_app)
        .delete("/api/apps/{id}", close_app)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A registry on a throwaway home and instance.
    fn registry(home: &Path, instance: &str) -> Arc<Registry> {
        let paths = Arc::new(Paths::new(home.to_path_buf()));
        let dirs = InstanceDirs::new(&paths.data_dir(), instance).expect("instance name");
        dirs.create().expect("create instance dirs");
        Registry::new(paths, Arc::new(dirs))
    }

    /// A record for an app that is not there any more: the pid is above every
    /// platform's `pid_max`, so no test can be unlucky enough to have it.
    fn record_of(id: &str, port: u16) -> AppRecord {
        AppRecord {
            id: id.to_string(),
            kind: AppType::Vscode,
            pid: u32::MAX - 1,
            port,
            cwd: None,
            started_at: "2026-01-01T00:00:00.000Z".to_string(),
            pid_token: None,
        }
    }

    #[test]
    fn the_state_directories_hang_off_the_instance_that_owns_them() {
        let dirs = InstanceDirs::new(Path::new("/tmp/data"), "ci-1").expect("instance name");
        assert_eq!(app_data_root(&dirs), dirs.root().join("apps"));
        assert!(
            app_data_root(&dirs).starts_with(dirs.root()),
            "an app's state must be somewhere no other instance can reach"
        );

        let home = Path::new("/tmp/home");
        assert!(extensions_dir(home).ends_with(".local/share/code-server/extensions"));
    }

    #[tokio::test]
    async fn one_instances_sweep_never_deletes_another_instances_app_data() {
        let home = tempfile::tempdir().expect("tempdir");

        // Instance A: an editor the user has open right now, with its state in
        // A's own instance directory.
        let live = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        let a = registry(home.path(), "a");
        let a_state = a.app_dir(live);
        std::fs::create_dir_all(&a_state).expect("A's state directory");

        // Instance B, on the same home, with a journal of its own. The one app
        // it remembers is long dead, so B's own leftovers are litter.
        let mine = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
        let b = registry(home.path(), "b");
        let b_state = b.app_dir(mine);
        std::fs::create_dir_all(&b_state).expect("B's state directory");
        let b_log = b.log_path(mine);
        std::fs::create_dir_all(b_log.parent().expect("the log has a directory")).expect("B's log directory");
        std::fs::write(&b_log, b"listen EADDRINUSE").expect("B's log");
        record::save(&b.instance.apps_file(), &[record_of(mine, 8710)]);

        assert_eq!(b.readopt_all().await, 0, "B has nothing left to adopt");

        assert!(a_state.is_dir(), "B must not sweep the instance directory A is using");

        // The sweep still does its job on what is demonstrably this instance's.
        assert!(!b_state.exists(), "B's own dead app leaves no state behind");
        assert!(!b_log.exists(), "nor a log");
    }

    #[test]
    fn the_spawn_environment_drops_the_servers_own_port_and_host() {
        // `clean_env` reads the live process environment, so assert on what
        // this function can guarantee no matter what was inherited.
        let env = app_env();
        assert!(!env.contains_key("PORT"), "PORT would override --bind-addr");
        assert!(!env.contains_key("HOST"), "HOST would override --bind-addr");
        assert!(!env.keys().any(|key| key.starts_with("CLAUDE_")), "the harness strip still applies");
    }

    #[test]
    fn the_log_tail_is_the_last_three_hundred_characters_of_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("app.log");
        assert_eq!(log_tail(&path), "", "a log that was never opened quotes nothing");

        std::fs::write(&path, "listen EADDRINUSE\n").expect("write");
        assert_eq!(log_tail(&path), "listen EADDRINUSE");

        // A long log is read from the end, not from the start.
        let mut long = "x".repeat(LOG_CAP * 3);
        long.push_str("the last thing it said");
        std::fs::write(&path, &long).expect("write");
        let tail = log_tail(&path);
        assert_eq!(tail.chars().count(), LOG_TAIL);
        assert!(tail.ends_with("the last thing it said"), "{tail}");
    }

    #[test]
    fn every_app_type_names_the_program_its_pid_must_be_running() {
        assert_eq!(AppType::Vscode.program(), COMMAND);
        assert_eq!(AppType::Vscode.missing(), CODE_SERVER_MISSING);
    }

    #[test]
    fn a_program_that_is_not_installed_is_a_409_with_a_token_beside_the_sentence() {
        let missing = AppError::Missing(AppType::Vscode).into_api();
        assert_eq!(missing.status(), StatusCode::CONFLICT, "still a 409, as it always was");
        assert_eq!(missing.body()["error"], CODE_SERVER_MISSING, "the token the web app acts on");
        let hint = missing.body()["hint"].as_str().expect("a sentence beside it");
        assert_eq!(hint, "code-server is not installed on this device, or is not on the server's PATH");

        // Every other failure keeps its sentence, and nothing else.
        let other = AppError::new("app did not become ready in time").into_api();
        assert_eq!(other.status(), StatusCode::CONFLICT);
        assert_eq!(other.body(), &serde_json::json!({ "error": "app did not become ready in time" }));
    }

    #[test]
    fn a_spawn_that_finds_no_program_is_the_program_missing() {
        let not_found = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert!(matches!(spawn_error(AppType::Vscode, &not_found), AppError::Missing(AppType::Vscode)));

        // Not every spawn failure is a missing program, and the others say what
        // they were rather than guessing at an install.
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let error = spawn_error(AppType::Vscode, &denied);
        assert!(
            matches!(&error, AppError::Failed(message) if message.starts_with("cannot launch code-server: ")),
            "{error}"
        );
    }

    #[tokio::test]
    async fn an_app_whose_program_is_not_on_the_path_is_refused_before_anything_is_reserved() {
        // The override is this test thread's own (`process_env`), so the empty
        // `PATH` is seen by this create and by no test running beside it.
        let _guard = crate::setup::tests::ChildPathGuard::take();
        let empty = tempfile::tempdir().expect("a PATH directory with nothing in it");
        process_env::set_child_path(Some(empty.path().to_string_lossy().into_owned()));

        let home = tempfile::tempdir().expect("tempdir");
        let registry = registry(home.path(), "missing");
        let result = registry.create_app(AppType::Vscode).await;

        assert!(matches!(result, Err(AppError::Missing(AppType::Vscode))), "{result:?}");
        assert!(registry.list().is_empty(), "nothing is listed");
        assert!(
            !app_data_root(&registry.instance).exists(),
            "and nothing was made for an app that never had a program to run"
        );
    }

    #[test]
    fn a_recorded_instance_is_one_with_a_process_that_has_not_exited() {
        let mut instance = Instance {
            info: AppInfo {
                id: "abc".into(),
                kind: AppType::Vscode,
                title: "vscode 1".into(),
                created_at: "2026-01-01T00:00:00.000Z".into(),
            },
            port: 8710,
            exited: Arc::new(AtomicBool::new(false)),
            pid: None,
            pid_token: None,
            cwd: Some("/home/user".into()),
            origin: Origin::Spawned,
        };
        assert!(instance.record().is_none(), "a registered upstream has no process to record");

        instance.pid = Some(4242);
        let record = instance.record().expect("a spawned app is recorded");
        assert_eq!(record.pid, 4242);
        assert_eq!(record.port, 8710);
        assert_eq!(record.id, "abc");
        assert_eq!(record.started_at, "2026-01-01T00:00:00.000Z", "the record carries the original start");

        instance.exited.store(true, Ordering::SeqCst);
        assert!(instance.record().is_none(), "a pid that has been reaped is never written down");
    }

    #[test]
    fn the_wire_names_match_the_zod_contract() {
        let info = AppInfo {
            id: "abc".into(),
            kind: AppType::Vscode,
            title: "vscode 1".into(),
            created_at: "2026-01-01T00:00:00.000Z".into(),
        };
        let encoded = serde_json::to_value(&info).expect("encode");
        assert_eq!(encoded["type"], "vscode", "the field is `type`, not `kind`");
        assert_eq!(encoded["createdAt"], "2026-01-01T00:00:00.000Z");

        let create: AppCreate = serde_json::from_str(r#"{"type":"vscode"}"#).expect("decode");
        assert_eq!(create.kind, AppType::Vscode);
        assert!(serde_json::from_str::<AppCreate>(r#"{"type":"emacs"}"#).is_err(), "one kind only");
    }
}
