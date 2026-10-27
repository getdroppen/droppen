//! Starting, keeping and stopping Droppen on this device.
//!
//! The order of a start, and why:
//!
//! 1. stop a server a crashed earlier run left behind (it would hold the
//!    instance lock) — only if it provably is one ([`sidecar::stop_leftover`]);
//! 2. make sure the instance is the app's own: created by the app (or new) and
//!    held by no other server ([`instance`]); otherwise refuse, touching
//!    nothing;
//! 3. build the `PATH` and look for the AI CLIs;
//! 4. pick the port (last launch's, if free);
//! 5. make sure the app's own account, `droppen-app`, exists, app-internal
//!    (`user add --app`) — **before** the server starts, so the window can sign
//!    in as soon as it answers; a stored login of another name (installs that
//!    named the account after the macOS login name) is replaced by a
//!    `droppen-app` one ([`accounts::ready_login`]); until the instance's
//!    marker records it done, finish removing the accounts an older build's
//!    first start may have inherited from the device, so only the app's can
//!    log in;
//! 6. start the server under the [`Supervisor`];
//! 7. [`Shell::connect`]: wait for it, prove the listener is the child, log in,
//!    check the instance, and hand the login to the windows; once signed in,
//!    remove the replaced login's account, if any ([`Cli::retire`]).
//!
//! The login is defended against a local process that keeps it failing (wrong
//! passwords lock the account for five minutes): at most one server restart
//! (which clears the in-memory lockout) and then at most one new password per
//! app run or per *Try again*, and a bounded number of re-logins; past that the
//! app shows its own page with the reason — never the web app's login page.
//!
//! Every step is logged to `~/Library/Logs/Droppen/desktop.log`, none of them
//! with a secret in it.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use droppen_desktop_core::accounts::{self, Cli, Ensured, Retired, APP_ACCOUNT};
use droppen_desktop_core::credentials::{generate_password, Credential, LayeredStore};
use droppen_desktop_core::dirs::ShellDirs;
use droppen_desktop_core::env_path::{self, AiClis};
use droppen_desktop_core::instance::{self, LockState, Ownership};
use droppen_desktop_core::server::{self, ApiError, Login};
use droppen_desktop_core::sidecar::{self, Backoff, Event, PidRecord, Spec, Supervisor};
use droppen_desktop_core::state::ShellState;
use droppen_desktop_core::{browser, port, web, INSTANCE};

use crate::window::MAIN;

/// How long a stop waits for the server after `SIGTERM` before `SIGKILL`.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// How long a start may take, all retries included, before it is a failure.
const START_DEADLINE: Duration = Duration::from_secs(120);

/// At most this many re-logins in [`RELOGIN_WINDOW`]; past it the app shows its
/// own error page instead of logging in again.
const RELOGINS: usize = 5;
const RELOGIN_WINDOW: Duration = Duration::from_secs(600);

/// Where the start is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Starting (or retrying a start).
    Starting,
    /// Up, logged in, shown.
    Running,
    /// The server died and is being restarted.
    Restarting,
    /// It failed; the message says why, in words for the person using the app.
    Failed(String),
    /// Stopped at quit.
    Stopped,
}

impl Phase {
    /// The name the start-up page reads.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Restarting => "restarting",
            Self::Failed(_) => "failed",
            Self::Stopped => "stopped",
        }
    }
}

/// What the shell needs from the windows and the menu bar.
pub trait Ui: Send + Sync {
    /// The phase changed.
    fn phase(&self, phase: &Phase);
    /// Hand a fresh login to the app's windows ([`web::Handoff`]: the window
    /// that asked goes where it wanted, every other one back to its page). It
    /// carries the token: never log it.
    fn hand_off(&self, login: web::Handoff);
    /// Put the main window back on the app's own start-up page, which shows a
    /// failure with *Try again* — never the web app's login page.
    fn show_start_page(&self);
}

/// The recoveries a login may still spend: reset per app run and per *Try
/// again*, so a local process making logins fail cannot make the app rotate
/// the password (which also revokes every login) or restart the server in a
/// loop.
#[derive(Debug, Clone, Copy)]
struct Budget {
    password_reset: bool,
    lockout_restart: bool,
}

impl Budget {
    const FULL: Self = Self {
        password_reset: true,
        lockout_restart: true,
    };
}

struct Inner {
    phase: Phase,
    supervisor: Option<Supervisor>,
    credential: Option<Credential>,
    token: Option<String>,
    path: String,
    booting: bool,
    /// Set by [`Shell::shutdown`]: a start still in progress must not leave a
    /// server running behind a quitting app.
    quitting: bool,
    /// The port the window was last pointed at.
    shown_port: Option<u16>,
    /// When a login was last handed to the windows.
    handed_off: Option<Instant>,
    budget: Budget,
    /// Set when the server exited because another server holds the instance:
    /// the pid that holder wrote, if any.
    held_by: Option<Option<u32>>,
    /// The account an earlier login of the app used, to remove once the
    /// `droppen-app` login has signed in (kept in the app state too).
    replaced_login: Option<String>,
}

/// Why a login could not be had.
enum LoginFailure {
    /// Worth trying again in a moment (the server is restarting, say).
    Transient(String),
    /// Not without a person: shown on the app's own page.
    Fatal(String),
}

/// The app's hold on its server.
pub struct Shell {
    dirs: ShellDirs,
    sidecar: PathBuf,
    instance_dir: PathBuf,
    store: LayeredStore,
    inherited: Vec<(String, String)>,
    port: Arc<AtomicU16>,
    nonce: String,
    translocated: bool,
    inner: Mutex<Inner>,
    ui: Box<dyn Ui>,
    /// Held for every login and hand-off, so a restart and a re-login never
    /// race each other into two logins.
    connecting: Mutex<()>,
    relogins: Mutex<VecDeque<Instant>>,
    me: Weak<Shell>,
}

/// What *Open in Browser* did ([`Shell::open_in_browser`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opening {
    /// The browser has the page, signed in (or, in the end-to-end build, the
    /// log says it would).
    Done,
    /// The instance has a user account: open its plain page, the login page,
    /// on this port.
    PlainPage(u16),
}

/// The server's environment ([`sidecar::server_env`]). In an end-to-end test
/// build, also the hub its sign-ins go to, from `DROPPEN_DESKTOP_TEST_HUB` (a
/// local hub, the server's `DROPPEN_HUB_DEFAULT`, which the scrub would drop),
/// so signing in to Droppen can be tried end to end without droppen.app.
fn server_env(inherited: &[(String, String)], path: &str, port: u16) -> Vec<(String, String)> {
    #[allow(unused_mut)]
    let mut env = sidecar::server_env(inherited.to_vec(), path, port);
    #[cfg(feature = "e2e")]
    if let Some((_, hub)) = inherited.iter().find(|(name, _)| name == "DROPPEN_DESKTOP_TEST_HUB") {
        // Also what desktop/build.sh finds to tell a test build from a release.
        log::info!("e2e build: the server signs in to the test hub {hub} (DROPPEN_DESKTOP_TEST_HUB)");
        env.push(("DROPPEN_HUB_DEFAULT".into(), hub.clone()));
        env.sort();
    }
    env
}

impl Shell {
    /// A shell for `sidecar` under `dirs`, keeping its credential in `store`.
    pub fn new(dirs: ShellDirs, sidecar: PathBuf, store: LayeredStore, translocated: bool, ui: Box<dyn Ui>) -> Arc<Self> {
        let instance_dir = instance::dir_under(&dirs.home, INSTANCE);
        Arc::new_cyclic(|me| Self {
            dirs,
            sidecar,
            instance_dir,
            store,
            inherited: std::env::vars().collect(),
            port: Arc::new(AtomicU16::new(0)),
            nonce: web::new_nonce(),
            translocated,
            inner: Mutex::new(Inner {
                phase: Phase::Starting,
                supervisor: None,
                credential: None,
                token: None,
                path: String::new(),
                booting: false,
                quitting: false,
                shown_port: None,
                handed_off: None,
                budget: Budget::FULL,
                held_by: None,
                replaced_login: None,
            }),
            ui,
            connecting: Mutex::new(()),
            relogins: Mutex::new(VecDeque::new()),
            me: me.clone(),
        })
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// This launch's nonce, which the window's init script is built with.
    pub fn nonce(&self) -> &str {
        &self.nonce
    }

    /// Whether macOS runs the app from an App Translocation copy.
    pub fn translocated(&self) -> bool {
        self.translocated
    }

    /// The window's login, once there is one (the sign-in watch asks the
    /// server with it). Never log it.
    pub fn login_token(&self) -> Option<String> {
        self.inner().token.clone()
    }

    /// The server's port, once it has one.
    pub fn port(&self) -> Option<u16> {
        Some(self.port.load(Ordering::SeqCst)).filter(|port| *port != 0)
    }

    /// A window finished loading the web app on `port`: if that is the
    /// server's port, keep it for the next launch (`state.json`). Only now —
    /// the page on the new origin has the login — is a moved port worth the
    /// old origin's local storage.
    pub fn loaded_on(&self, port: u16) {
        if self.port() != Some(port) {
            return;
        }
        match ShellState::remember_port(&self.dirs.state_file(), port) {
            Ok(true) => log::info!("port {port} kept for the next launch"),
            Ok(false) => {}
            Err(error) => log::warn!("cannot save the app state: {error}"),
        }
    }

    /// The phase right now.
    pub fn phase(&self) -> Phase {
        self.inner().phase.clone()
    }

    /// The running server's pid.
    pub fn server_pid(&self) -> Option<u32> {
        self.inner().supervisor.as_ref().and_then(Supervisor::pid)
    }

    /// The app's log directory.
    pub fn logs_dir(&self) -> PathBuf {
        self.dirs.logs.clone()
    }

    fn set_phase(&self, phase: Phase) {
        {
            let mut inner = self.inner();
            if inner.phase == phase {
                return;
            }
            inner.phase = phase.clone();
        }
        log::info!("phase: {}", phase.name());
        self.ui.phase(&phase);
    }

    fn fail(&self, message: String) {
        log::error!("{message}");
        self.set_phase(Phase::Failed(message));
        self.ui.show_start_page();
    }

    /// Start, or start again after *Try again*, on a thread of its own. Gives
    /// the login back its recoveries. A second call while one runs does nothing.
    pub fn boot(&self) {
        {
            let mut inner = self.inner();
            if inner.booting || inner.quitting {
                return;
            }
            inner.booting = true;
            inner.budget = Budget::FULL;
            inner.held_by = None;
        }
        let Some(shell) = self.me.upgrade() else { return };
        std::thread::Builder::new()
            .name("droppen-boot".into())
            .spawn(move || {
                shell.set_phase(Phase::Starting);
                let outcome = shell.try_boot();
                shell.inner().booting = false;
                match outcome {
                    Ok(()) => shell.set_phase(Phase::Running),
                    Err(error) => shell.fail(error),
                }
            })
            .expect("spawn the boot thread");
    }

    fn try_boot(&self) -> Result<(), String> {
        self.dirs
            .create()
            .map_err(|error| format!("Droppen could not create its folders: {error}"))?;
        if !self.sidecar.is_file() {
            return Err(format!(
                "The Droppen server is missing from the app ({}). Reinstall Droppen.",
                self.sidecar.display()
            ));
        }
        // A retry first lets go of what the failed attempt started.
        if let Some(supervisor) = self.inner().supervisor.take() {
            let pid = supervisor.pid();
            log::info!("stopping the previous attempt's server: {:?}", supervisor.stop(STOP_GRACE));
            if let Some(pid) = pid {
                PidRecord::clear(&self.dirs.pid_file(), pid);
            }
        }
        match sidecar::stop_leftover(&self.dirs.pid_file(), &self.instance_dir, STOP_GRACE) {
            sidecar::Leftover::None => {}
            other => log::warn!("a server from an earlier run: {other:?}"),
        }

        // The instance must be the app's own before anything touches it.
        let fresh = match instance::ownership(&self.instance_dir) {
            Ownership::NotOurs => {
                return Err(format!(
                    "This device already has a Droppen server instance called \"{INSTANCE}\" that the Droppen app did not create ({}). \
                     The app leaves it alone. Rename or remove that folder if it is not needed, then try again.",
                    self.instance_dir.display()
                ))
            }
            Ownership::Ours => false,
            Ownership::New => {
                instance::claim_new(&self.instance_dir).map_err(|error| format!("Droppen could not create its server folder: {error}"))?;
                log::info!("created instance {INSTANCE} at {}", self.instance_dir.display());
                true
            }
        };
        match instance::lock_state(&self.instance_dir) {
            Ok(LockState::Free) => {}
            Ok(LockState::Held(holder)) => return Err(held_message(holder)),
            Err(error) => return Err(format!("Droppen could not check its server's lock: {error}")),
        }

        // PATH, once per start: the login shell's is read here, not per session.
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|shell| shell.starts_with('/'))
            .unwrap_or_else(|| "/bin/zsh".into());
        let login_path = env_path::login_shell_path(std::path::Path::new(&shell), &self.dirs.home, Duration::from_secs(5));
        if login_path.is_none() {
            log::warn!("could not read the login shell's PATH ({shell}); using the standard directories only");
        }
        let path = env_path::build(&self.dirs.home, login_path.as_deref());
        let clis = AiClis::on(&path);
        log::info!(
            "PATH has {} directories; claude: {}; codex: {}",
            path.split(':').count(),
            clis.claude
                .as_deref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "not found".into()),
            clis.codex
                .as_deref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "not found".into()),
        );

        // The port: last launch's while it is free (asked again for a few
        // seconds if it is taken). A new one is kept only once the window has
        // loaded on it ([`Shell::loaded_on`]): until then the old port's origin
        // holds the login and the device keys, and a start that fails must go
        // back to it next time.
        let saved = ShellState::load(&self.dirs.state_file()).port;
        let port = port::pick(saved, &self.dirs.support).map_err(|error| format!("Droppen found no free port for its server: {error}"))?;
        if saved != Some(port) {
            log::info!("port {port} (was {saved:?}; kept once the window loads on it)");
        } else {
            log::info!("port {port}");
        }
        self.port.store(port, Ordering::SeqCst);

        // What the app keeps across launches: an earlier login still to be
        // removed is recorded there (the port only once the window loaded).
        let state_file = self.dirs.state_file();
        let mut state = ShellState::load(&state_file);

        // The account, before the server is ever up.
        let cli = self.cli(&path, port);
        let (stored, found) = self.store.load();
        match &stored {
            Some(credential) => log::info!("using the stored login for {} ({found:?})", credential.username),
            None => log::info!("no stored login ({found:?}): making the app's account"),
        }
        // An earlier build's login (named after the macOS login name) is
        // recorded before the new login replaces it, so its account is removed
        // even if this start is cut short after that — and if the record cannot
        // be saved, this run still removes it once signed in (it is kept in
        // memory too), while the account is app-internal from here on and so
        // never holds the instance behind a login nobody can pass.
        let login_name = accounts::legacy_username(std::env::var("USER").ok().as_deref());
        let record = |old: &str| {
            log::info!("the app's earlier login is {old}, from before its account was {APP_ACCOUNT}: moving to {APP_ACCOUNT}");
            state.replaced_login = Some(old.to_string());
            if let Err(error) = state.save(&state_file) {
                log::warn!("cannot save the app state (the old login is removed this run all the same): {error}");
            }
        };
        let ready = accounts::ready_login(&cli, stored, &login_name, record, |credential| {
            self.store
                .save(credential)
                .map_err(|error| format!("Droppen could not keep its login: {error}"))
        })?;
        if let Ensured::Migrated { password_reset } = ready.ensured {
            log_migrated(APP_ACCOUNT, password_reset);
        }
        match ready.kept_in {
            Some(kind) => log::info!("account {APP_ACCOUNT} {:?}, password kept in the {kind}", ready.ensured),
            None if ready.ensured != Ensured::Existed => log::info!("account {APP_ACCOUNT}: {:?}", ready.ensured),
            None => {}
        }
        let credential = ready.credential;
        // Until the marker records it, every start finishes the cleanup an
        // older build's first start may have left: that build inherited the
        // machine-wide accounts, and a start cut short before removing them
        // must not leave them for good. A new instance has none.
        if fresh || instance::needs_pruning(&self.instance_dir) {
            let removed = cli.prune_others(&credential.username)?;
            if !removed.is_empty() {
                log::info!("new instance: removed {} inherited account(s): {}", removed.len(), removed.join(", "));
            }
            instance::mark_pruned(&self.instance_dir).map_err(|error| format!("Droppen could not record its server folder's state: {error}"))?;
            log::info!("instance {INSTANCE} has only the app's account ({})", credential.username);
        }

        {
            let mut inner = self.inner();
            inner.path = path.clone();
            inner.credential = Some(credential);
            inner.replaced_login = state.replaced_login.clone();
        }
        // Whatever an earlier server wrote must not vouch for this one.
        instance::forget_port(&self.instance_dir);
        let supervisor = self.start_supervisor(path);
        {
            let mut inner = self.inner();
            if inner.quitting {
                drop(inner);
                log::info!("quitting during the start: {:?}", supervisor.stop(STOP_GRACE));
                return Err("Droppen is quitting.".into());
            }
            inner.supervisor = Some(supervisor);
        }
        self.connect("/", MAIN)
    }

    fn cli(&self, path: &str, port: u16) -> Cli {
        Cli {
            program: self.sidecar.clone(),
            env: server_env(&self.inherited, path, port),
            cwd: self.dirs.home.clone(),
            instance: INSTANCE.into(),
            log: Some(self.dirs.server_log()),
        }
    }

    fn start_supervisor(&self, path: String) -> Supervisor {
        let sidecar_path = self.sidecar.clone();
        let inherited = self.inherited.clone();
        let home = self.dirs.home.clone();
        let log = self.dirs.server_log();
        let instance_dir = self.instance_dir.clone();
        let support = self.dirs.support.clone();
        let port = Arc::clone(&self.port);
        let next_spec = move |attempt: u32| {
            let mut current = port.load(Ordering::SeqCst);
            // A restart keeps the port unless something else has taken it.
            if attempt > 0 && !port::is_free(current) {
                match port::choose(&support) {
                    Ok(fresh) => {
                        // Kept once the window loads on it (`Shell::loaded_on`).
                        log::warn!("port {current} is taken; moving the server to {fresh}");
                        current = fresh;
                        port.store(fresh, Ordering::SeqCst);
                    }
                    Err(error) => log::error!("no free port: {error}"),
                }
            }
            instance::forget_port(&instance_dir);
            Spec {
                program: sidecar_path.clone(),
                args: vec!["serve".into()],
                env: server_env(&inherited, &path, current),
                cwd: home.clone(),
                log: Some(log.clone()),
            }
        };
        let me = self.me.clone();
        Supervisor::start(next_spec, Backoff::default(), move |event| {
            if let Some(shell) = me.upgrade() {
                shell.on_event(event);
            }
        })
    }

    fn on_event(&self, event: Event) {
        match event {
            Event::Started { pid, attempt } => {
                let port = self.port().unwrap_or_default();
                log::info!("server started: pid {pid}, port {port}, start #{}", attempt + 1);
                let record = PidRecord {
                    pid,
                    exe: self.sidecar.clone(),
                    port,
                };
                if let Err(error) = record.save(&self.dirs.pid_file()) {
                    log::warn!("cannot record the server pid: {error}");
                }
                if attempt > 0 {
                    self.after_restart(port);
                }
            }
            Event::Exited {
                pid,
                code,
                signal,
                requested,
            } => {
                PidRecord::clear(&self.dirs.pid_file(), pid);
                if code == Some(sidecar::INSTANCE_HELD_EXIT) {
                    let holder = instance::recorded_holder(&self.instance_dir);
                    log::warn!("server {pid} found instance {INSTANCE} held by another server (pid {holder:?})");
                    self.inner().held_by = Some(holder);
                } else if requested {
                    log::info!("server stopped: pid {pid}, code {code:?}, signal {signal:?}");
                } else {
                    log::warn!("server exited: pid {pid}, code {code:?}, signal {signal:?}");
                }
                if !requested && self.phase() == Phase::Running {
                    self.set_phase(Phase::Restarting);
                }
            }
            Event::SpawnFailed { error } => log::error!("cannot start the server: {error}"),
            Event::Restarting { after } => log::info!("restarting the server in {after:?}"),
        }
    }

    /// A restarted server keeps its accounts and logins (they are in its
    /// database), so the window's token still works and the web app reconnects
    /// by itself — unless the window has nothing to show yet (the start failed,
    /// or never got a login) or the port, and with it the web app's origin,
    /// moved. Then the shell connects again.
    fn after_restart(&self, port: u16) {
        let Some(shell) = self.me.upgrade() else { return };
        std::thread::spawn(move || {
            let healthy = server::wait_healthy(port, Instant::now() + Duration::from_secs(60), || shell.server_running());
            if let Err(error) = healthy {
                log::warn!("restarted server is not answering yet: {error}");
                return;
            }
            // A start or a re-login in progress handles this server itself.
            let Ok(guard) = shell.connecting.try_lock() else { return };
            let (needs_login, booting) = {
                let inner = shell.inner();
                let failed = matches!(inner.phase, Phase::Failed(_));
                (needs_login(inner.token.is_some(), failed, inner.shown_port, port), inner.booting)
            };
            if booting {
                return;
            }
            if !needs_login {
                log::info!("server back on port {port}");
                shell.inner().held_by = None;
                shell.set_phase(Phase::Running);
                return;
            }
            log::info!("server back on port {port}; logging in again");
            match shell.connect_locked(&guard, "/", MAIN) {
                Ok(()) => shell.set_phase(Phase::Running),
                Err(error) => shell.fail(error),
            }
        });
    }

    fn server_running(&self) -> bool {
        self.inner().supervisor.as_ref().is_some_and(Supervisor::is_running)
    }

    /// Log in and hand the login to the windows: `asking` (a window's label) at
    /// `next`, every other window at the page it shows.
    fn connect(&self, next: &str, asking: &str) -> Result<(), String> {
        let guard = self.connecting.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.connect_locked(&guard, next, asking)
    }

    fn connect_locked(&self, _guard: &MutexGuard<'_, ()>, next: &str, asking: &str) -> Result<(), String> {
        let deadline = Instant::now() + START_DEADLINE;
        let mut last = String::from("the server did not start");
        let (port, login) = loop {
            if Instant::now() >= deadline {
                return Err(format!("Droppen did not come up within {}s: {last}", START_DEADLINE.as_secs()));
            }
            {
                let inner = self.inner();
                if inner.quitting {
                    return Err("Droppen is quitting.".into());
                }
                if let Some(holder) = inner.held_by {
                    return Err(held_message(holder));
                }
            }
            let Some(port) = self.port() else {
                return Err("Droppen has no port for its server.".into());
            };
            let attempt_deadline = (Instant::now() + Duration::from_secs(30)).min(deadline);
            if let Err(error) = server::wait_healthy(port, attempt_deadline, || self.server_running()) {
                last = error.to_string();
                // The supervisor restarts a server that died; give it the time.
                std::thread::sleep(Duration::from_millis(300));
                continue;
            }
            if !self.wait_until_ours(port, Duration::from_secs(5)) {
                last = format!("the server on port {port} could not be shown to be the app's own");
                log::warn!("{last}; not sending it the password");
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
            match self.login_recovering(port) {
                Ok(login) => break (port, login),
                Err(LoginFailure::Fatal(message)) => return Err(message),
                Err(LoginFailure::Transient(why)) => {
                    last = why;
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        };
        // The port can move under a restart; ask the one the login was made on.
        match server::instance(port, &login.token) {
            Ok(instance) if instance == INSTANCE => {}
            other => {
                let _ = server::logout(port, &login.token);
                return Err(match other {
                    Ok(instance) => format!("The server on port {port} is instance \"{instance}\", not the app's. The app logged out of it."),
                    Err(error) => format!("Droppen could not confirm its server: {error}"),
                });
            }
        }
        log::info!("logged in as {} on port {port}; token expires {}", login.username, login.expires_at);
        let (superseded, entry) = {
            let mut inner = self.inner();
            let superseded = inner.token.replace(login.token.clone()).filter(|old| *old != login.token);
            inner.shown_port = Some(port);
            inner.held_by = None;
            let entry = web::Entry {
                nonce: self.nonce.clone(),
                translocated: self.translocated,
            };
            (superseded, entry)
        };
        if let Some(old) = superseded {
            // The login this one replaces is of no use to anyone now.
            match server::logout(port, &old) {
                Ok(()) => log::info!("revoked the login this one replaces"),
                Err(error) => log::warn!("could not revoke the replaced login: {error}"),
            }
        }
        self.ui.hand_off(web::Handoff::new(port, &login.token, entry, asking, next));
        self.inner().handed_off = Some(Instant::now());
        self.retire_replaced_login(port);
        Ok(())
    }

    /// Once the `droppen-app` login has signed in, remove the account the
    /// login it replaced used — that one account, named by the old stored
    /// login and nothing else, and only while it is app-internal: a user
    /// account of that name may be a person's own by now, and is left alone
    /// ([`accounts::Retired::Kept`]). A failure is logged and tried again at
    /// the next sign-in.
    fn retire_replaced_login(&self, port: u16) {
        let (old, signed_in_as, path) = {
            let inner = self.inner();
            let signed_in_as = inner.credential.as_ref().map(|credential| credential.username.clone());
            (inner.replaced_login.clone(), signed_in_as, inner.path.clone())
        };
        let Some(old) = old else { return };
        if signed_in_as.as_deref() != Some(APP_ACCOUNT) || old == APP_ACCOUNT {
            return;
        }
        match self.cli(&path, port).retire(&old) {
            Ok(retired) => {
                match retired {
                    Retired::Removed => log::info!("removed {old}, the app's earlier login account: the app signs in as {APP_ACCOUNT}"),
                    Retired::Gone => log::info!("{old}, the app's earlier login account, is already gone"),
                    Retired::Kept => log::warn!("{old} is a user account now, not the app's earlier login: left alone"),
                }
                self.inner().replaced_login = None;
                let state_file = self.dirs.state_file();
                let mut state = ShellState::load(&state_file);
                state.replaced_login = None;
                if let Err(error) = state.save(&state_file) {
                    log::warn!("cannot save the app state: {error}");
                }
            }
            Err(error) => log::warn!("could not remove {old}, the app's earlier login account (tried again at the next sign-in): {error}"),
        }
    }

    /// *Open in Browser*: the page in this device's default browser, signed in
    /// with a one-time token while the instance has no user account
    /// ([`Opening::Done`]), or the plain page — its login page — once it has one
    /// ([`Opening::PlainPage`]). The token is asked for with the window's own
    /// login (`POST /api/auth/local-open/grant`, [`server::local_open_grant`]):
    /// the server's control socket, which any process of this user could ask,
    /// opens nothing on the app's instance. It goes to the browser on
    /// `osascript`'s standard input ([`browser::open_secret`]) — on no command
    /// line, in no log. Blocking: call it off the main thread. On failure,
    /// why, and the port for opening the plain page instead.
    pub fn open_in_browser(&self) -> Result<Opening, (String, Option<u16>)> {
        let Some(port) = self.port() else {
            return Err(("the server has no port yet".into(), None));
        };
        if !self.server_running() {
            return Err(("the server is not running".into(), Some(port)));
        }
        let Some(login) = self.login_token() else {
            return Err(("the window is not signed in yet".into(), Some(port)));
        };
        // The end-to-end build opens no browser: it says what it would have done.
        let e2e = cfg!(feature = "e2e");
        // A window login the server no longer takes (expired, revoked) is made
        // again — every window back to the page it shows — and asked once more.
        let grant = server::grant_signing_in_again(&login, |token| server::local_open_grant(port, token), || {
            self.sign_in_again()?;
            self.login_token().ok_or_else(|| "the window is not signed in".to_string())
        });
        match grant {
            Ok(Some(token)) => {
                if e2e {
                    log::info!("would open the page in the browser with a one-time sign-in (e2e build: not opened)");
                    return Ok(Opening::Done);
                }
                browser::open_secret(&server::signed_in_page(port, &token)).map_err(|why| (why, Some(port)))?;
                log::info!("opened the page in the browser, signed in as this device's owner");
                Ok(Opening::Done)
            }
            Ok(None) => {
                if e2e {
                    log::info!("would open the page in the browser at its login page (e2e build: not opened)");
                    return Ok(Opening::Done);
                }
                log::info!("the instance has an account: opening its login page in the browser");
                Ok(Opening::PlainPage(port))
            }
            Err(why) => Err((why, Some(port))),
        }
    }

    /// Sign the windows in again, each back to the page it shows, for
    /// something the app asks its server with the window's login and the
    /// server no longer takes. Counted with the windows' own re-logins
    /// ([`RELOGINS`]).
    fn sign_in_again(&self) -> Result<(), String> {
        let guard = self.connecting.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !self.count_relogin() {
            return Err(format!("signed in again {RELOGINS} times in {} minutes already", RELOGIN_WINDOW.as_secs() / 60));
        }
        // No window is the asking one: each goes back to the page it shows.
        self.connect_locked(&guard, "/", "")?;
        log::info!("logged in again (for Open in Browser)");
        self.set_phase(Phase::Running);
        Ok(())
    }

    /// Whether the server on `port` is provably the supervised child: the
    /// instance's lock holder and recorded port say so ([`instance::serves_on`]).
    /// Waits a moment, since the port file is written just after the bind.
    fn wait_until_ours(&self, port: u16, wait: Duration) -> bool {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(pid) = self.server_pid() {
                if instance::serves_on(&self.instance_dir, pid, port) {
                    return true;
                }
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Log in with the stored password, spending the recoveries a refusal
    /// allows: one server restart (which clears a lockout, kept in memory by
    /// the server), then one new password.
    fn login_recovering(&self, port: u16) -> Result<Login, LoginFailure> {
        let credential = self
            .inner()
            .credential
            .clone()
            .ok_or_else(|| LoginFailure::Fatal("Droppen has no login for its server yet.".into()))?;
        match server::login(port, &credential.username, &credential.password) {
            Ok(login) => return Ok(login),
            Err(ApiError::Refused) => log::warn!("the server refused the stored login"),
            Err(error) => return Err(LoginFailure::Transient(error.to_string())),
        }

        // The harmless recovery first: a restart clears a lockout (the
        // server keeps it in memory) and changes nothing else. Only if the
        // password itself is wrong does the app make a new one — which also
        // revokes every login of the account.
        if self.spend(|budget| &mut budget.lockout_restart) {
            log::warn!("restarting the server to clear a lockout (once per run)");
            let Some(port) = self.restart_server(Duration::from_secs(60)) else {
                return Err(LoginFailure::Transient("the server did not come back after the restart".into()));
            };
            match server::login(port, &credential.username, &credential.password) {
                Ok(login) => return Ok(login),
                Err(ApiError::Refused) => log::warn!("refused after a restart too: the stored password is probably not the account's"),
                Err(error) => return Err(LoginFailure::Transient(error.to_string())),
            }
        }

        if self.spend(|budget| &mut budget.password_reset) {
            log::warn!("setting a new password for {} (once per run)", credential.username);
            let path = self.inner().path.clone();
            let port = self.port().unwrap_or(port);
            let credential = self
                .reset_password(&credential.username, &self.cli(&path, port))
                .map_err(LoginFailure::Fatal)?;
            match server::login(port, &credential.username, &credential.password) {
                Ok(login) => return Ok(login),
                Err(ApiError::Refused) => log::error!("the new password was refused too"),
                Err(error) => return Err(LoginFailure::Transient(error.to_string())),
            }
        }

        Err(LoginFailure::Fatal(
            "Droppen could not sign in to its own server on this device: it refused the app's login, even after a restart and \
             a new password. Something on this device may be trying passwords. Try again; if it keeps happening, quit Droppen \
             and open it again."
                .into(),
        ))
    }

    /// Take one recovery out of the budget, if it is still there.
    fn spend(&self, which: impl Fn(&mut Budget) -> &mut bool) -> bool {
        let mut inner = self.inner();
        let left = which(&mut inner.budget);
        std::mem::replace(left, false)
    }

    /// Restart the server and wait until a new, provably own child answers.
    /// The port it came back on, or `None`.
    fn restart_server(&self, wait: Duration) -> Option<u16> {
        let old = self.server_pid();
        if let Some(supervisor) = self.inner().supervisor.as_ref() {
            supervisor.restart();
        }
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline {
            if let (Some(pid), Some(port)) = (self.server_pid(), self.port()) {
                if Some(pid) != old
                    && server::wait_healthy(port, (Instant::now() + Duration::from_secs(5)).min(deadline), || self.server_running()).is_ok()
                    && self.wait_until_ours(port, Duration::from_secs(5))
                {
                    return Some(port);
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        None
    }

    fn reset_password(&self, username: &str, cli: &Cli) -> Result<Credential, String> {
        let credential = Credential {
            username: username.to_string(),
            password: generate_password().map_err(|error| format!("Droppen could not make a password: {error}"))?,
        };
        if let Ensured::Migrated { password_reset } = cli.ensure(&credential, true)? {
            log_migrated(username, password_reset);
        }
        let kind = self
            .store
            .save(&credential)
            .map_err(|error| format!("Droppen could not keep its login: {error}"))?;
        log::info!("new password for {username} kept in the {kind}");
        self.inner().credential = Some(credential.clone());
        Ok(credential)
    }

    /// The web app in the window labelled `asking` went to its login page
    /// (logged out, its token expired or was revoked): log in again, send that
    /// window where it wanted to go, and every other window back to its page
    /// with the same login — they all had the one just lost. The login page
    /// itself is never shown; past [`RELOGINS`] in [`RELOGIN_WINDOW`] the app's
    /// own page explains instead.
    ///
    /// Windows that find the same dead login at once all ask; the first is
    /// answered with a login for every window, and the ones asked for before it
    /// was handed out are left at that.
    pub fn relogin(&self, next: String, asking: String) {
        let asked = Instant::now();
        let Some(shell) = self.me.upgrade() else { return };
        std::thread::spawn(move || {
            let guard = shell.connecting.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let handed_off = shell.inner().handed_off;
            if handed_off_since(handed_off, asked) {
                log::info!("window {asking} asked for a login that reached every window since; not logging in again");
                return;
            }
            if !shell.count_relogin() {
                shell.fail(format!(
                    "Droppen keeps losing its sign-in on this device ({RELOGINS} times in {} minutes), so it stopped signing in \
                     again by itself. Try again; if it keeps happening, quit Droppen and open it again.",
                    RELOGIN_WINDOW.as_secs() / 60
                ));
                return;
            }
            match shell.connect_locked(&guard, &next, &asking) {
                Ok(()) => {
                    log::info!("logged in again (asked by window {asking})");
                    shell.set_phase(Phase::Running);
                }
                Err(error) => shell.fail(error),
            }
        });
    }

    /// Count one re-login; `false` once [`RELOGINS`] were made in
    /// [`RELOGIN_WINDOW`].
    fn count_relogin(&self) -> bool {
        let mut recent = self.relogins.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        while recent.front().is_some_and(|at| now.duration_since(*at) > RELOGIN_WINDOW) {
            recent.pop_front();
        }
        if recent.len() >= RELOGINS {
            return false;
        }
        recent.push_back(now);
        true
    }

    /// *Try again* on the app's page.
    pub fn retry(&self) {
        if matches!(self.phase(), Phase::Failed(_)) {
            log::info!("trying again");
            self.relogins.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clear();
            self.boot();
        }
    }

    /// Stop everything, at quit: revoke the app's login, stop the server
    /// (`SIGTERM`, then `SIGKILL` after the grace period), forget its pid.
    pub fn shutdown(&self) {
        let (supervisor, token) = {
            let mut inner = self.inner();
            inner.quitting = true;
            (inner.supervisor.take(), inner.token.take())
        };
        if let (Some(token), Some(port)) = (token, self.port()) {
            match server::logout(port, &token) {
                Ok(()) => log::info!("logged out"),
                Err(error) => log::warn!("logout at quit: {error}"),
            }
        }
        if let Some(supervisor) = supervisor {
            let pid = supervisor.pid();
            let stopped = supervisor.stop(STOP_GRACE);
            log::info!("server stopped at quit: {stopped:?} (pid {pid:?})");
            if let Some(pid) = pid {
                PidRecord::clear(&self.dirs.pid_file(), pid);
            }
        }
        self.set_phase(Phase::Stopped);
    }
}

/// Whether a server that came back on `port` needs a fresh login and hand-off:
/// the window has no login (`has_token` false), the start failed, or the
/// window shows another port (another origin) than the server's.
fn needs_login(has_token: bool, failed: bool, shown_port: Option<u16>, port: u16) -> bool {
    !has_token || failed || shown_port != Some(port)
}

/// Whether a login was handed to every window after a window asked for one
/// at `asked` — which then has it already.
fn handed_off_since(handed_off: Option<Instant>, asked: Instant) -> bool {
    handed_off.is_some_and(|at| at > asked)
}

/// Log that the app's account was a user account and is now app-internal: an
/// account made before the app's login was app-internal can carry the app's
/// name.
fn log_migrated(username: &str, password_reset: bool) {
    log::info!(
        "account {username} was a user account; it is now the app's app-internal one (user mark-app), {}",
        if password_reset { "with a new password" } else { "password kept" }
    );
}

/// What to tell the person when another server holds the app's instance.
fn held_message(holder: Option<u32>) -> String {
    let who = holder.map(|pid| format!(" (pid {pid})")).unwrap_or_default();
    format!("Another Droppen server{who} is already running the app's instance \"{INSTANCE}\" on this device. Quit it, then try again.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inherited_with_test_hub() -> Vec<(String, String)> {
        vec![
            ("HOME".into(), "/Users/ada".into()),
            ("DROPPEN_DESKTOP_TEST_HUB".into(), "http://127.0.0.1:53139".into()),
            ("DROPPEN_HUB_DEFAULT".into(), "https://elsewhere.example".into()),
        ]
    }

    fn value<'a>(env: &'a [(String, String)], name: &str) -> Vec<&'a str> {
        env.iter().filter(|(n, _)| n == name).map(|(_, v)| v.as_str()).collect()
    }

    #[cfg(feature = "e2e")]
    #[test]
    fn an_end_to_end_build_signs_its_server_in_to_the_test_hub() {
        let env = server_env(&inherited_with_test_hub(), "/usr/bin:/bin", 18_700);
        // The test hub, and only it: an inherited DROPPEN_HUB_DEFAULT is scrubbed like every DROPPEN_*.
        assert_eq!(value(&env, "DROPPEN_HUB_DEFAULT"), ["http://127.0.0.1:53139"]);
        assert!(value(&env, "DROPPEN_DESKTOP_TEST_HUB").is_empty(), "the hook itself stays with the app");
        assert_eq!(value(&env, "HOME"), ["/Users/ada"]);
        let mut sorted = env.clone();
        sorted.sort();
        assert_eq!(env, sorted, "still in order");
        // No test hub: nothing added.
        let without = server_env(&[("HOME".into(), "/Users/ada".into())], "/usr/bin:/bin", 18_700);
        assert!(value(&without, "DROPPEN_HUB_DEFAULT").is_empty());
        assert_eq!(without, sidecar::server_env(vec![("HOME".into(), "/Users/ada".into())], "/usr/bin:/bin", 18_700));
    }

    #[cfg(not(feature = "e2e"))]
    #[test]
    fn a_release_build_gives_its_server_no_test_hub() {
        let inherited = inherited_with_test_hub();
        let env = server_env(&inherited, "/usr/bin:/bin", 18_700);
        assert!(value(&env, "DROPPEN_HUB_DEFAULT").is_empty(), "droppen.app, whatever the environment says");
        assert!(value(&env, "DROPPEN_DESKTOP_TEST_HUB").is_empty());
        assert_eq!(env, sidecar::server_env(inherited, "/usr/bin:/bin", 18_700), "exactly the sidecar's environment");
    }

    #[test]
    fn a_restarted_server_is_logged_into_again_only_when_the_window_needs_it() {
        // Same port, a login, all well: the web app reconnects by itself.
        assert!(!needs_login(true, false, Some(51_000), 51_000));
        // The start had failed before the server came up healthy.
        assert!(needs_login(true, true, Some(51_000), 51_000));
        assert!(needs_login(false, true, None, 51_000));
        // Never logged in.
        assert!(needs_login(false, false, None, 51_000));
        // The server moved to another port: another origin to open.
        assert!(needs_login(true, false, Some(51_000), 52_000));
    }

    #[test]
    fn windows_asking_at_once_are_answered_by_one_login() {
        let asked = Instant::now();
        let later = asked + Duration::from_millis(50);
        // Nothing handed out yet, or only before this window asked: log in.
        assert!(!handed_off_since(None, asked));
        assert!(!handed_off_since(Some(asked - Duration::from_millis(1)), asked));
        // A login went to every window after it asked: it has one.
        assert!(handed_off_since(Some(later), asked));
    }

    #[test]
    fn each_recovery_is_spent_once() {
        let mut budget = Budget::FULL;
        assert!(std::mem::replace(&mut budget.password_reset, false));
        assert!(!std::mem::replace(&mut budget.password_reset, false), "a second reset is refused");
        assert!(budget.lockout_restart, "the restart is a budget of its own");
    }

    #[test]
    fn the_held_message_names_the_holder_when_it_can() {
        assert!(held_message(Some(4242)).contains("(pid 4242)"));
        assert!(!held_message(None).contains("pid"));
        assert!(held_message(None).contains(INSTANCE));
    }
}
