//! The application: configuration, state, routes, and a serve loop that can be
//! stopped.
//!
//! [`GotchaApp::run`] is overridden rather than used, because the stock one
//! serves bare (see [`crate::shutdown`]). Everything above it —
//! `config` / `state` / `routes` / `build_router` — is the framework's, so the
//! router (and therefore `/openapi.json`, `/redoc`, `/scalar`) is assembled
//! exactly as gotcha assembles it.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use gotcha::axum::extract::FromRef;
use gotcha::{ConfigWrapper, GotchaApp, GotchaContext, GotchaError, GotchaResult, GotchaRouter};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::accounts::login::claude::ClaudeCodeLogin;
use crate::accounts::login::codex::CodexLogin;
use crate::accounts::login::Logins;
use crate::api::AppState;
use droppen_http::config::AppConfig;
use crate::feed::global::GlobalFeed;
use crate::feed::FeedRegistry;
use crate::provider::{InjectionRegistry, ProviderRegistry};
use droppen_instance::instance::{InstanceDirs, InstanceLock};
use droppen_instance::paths::Paths;
use droppen_sessions::account::{RotationOutcome, RotationTrigger};
use droppen_sessions::{DriverStates, InjectionRunState, Source, TranscriptSources};
use crate::shutdown::{self, Cleanups};
use droppen_base::time::iso8601;

/// How long a shutdown waits for in-flight requests before giving up.
const DRAIN_GRACE: Duration = Duration::from_secs(5);

/// The Droppen server.
#[derive(Debug, Default)]
pub struct RemoteHandApp {
    /// Overrides `home_dir` from the config file (used by tests).
    pub home_override: Option<std::path::PathBuf>,
    /// Overrides `[server].port` (used by `export-openapi`, which wants port 0).
    pub port_override: Option<u16>,
    /// Overrides `instance` (used by tests, which cannot share one process
    /// environment between servers that must not see each other).
    pub instance_override: Option<String>,
    /// Skip the instance lock. `export-openapi` sets this: it serves a throwaway
    /// router to read one document back and must not refuse to run because a
    /// real server happens to own the instance.
    pub skip_lock: bool,
    /// Serve without the authentication guard. `export-openapi` sets this for
    /// the same reason it sets [`Self::skip_lock`]: it reads `/openapi.json` off
    /// a throwaway in-process server, and on a developer's own machine — where
    /// accounts exist — that read would otherwise be a `401` and `pnpm gen:api`
    /// would fail. Never set by anything that serves a port.
    pub auth_disabled: bool,
    /// Overrides `auth.enabled` from the configuration (used by tests, which
    /// cannot set `APP_AUTH__ENABLED` for one server without setting it for
    /// every server in the process). `Some(false)` is the explicit open mode
    /// most integration tests run in, so only the ones about authentication
    /// have to log in; `None` leaves the configuration alone.
    pub auth_enabled_override: Option<bool>,
    /// Overrides whether the desktop app runs this server (`DROPPEN_DESKTOP_APP`,
    /// [`crate::desktop_app`]) — for tests, which cannot set the variable for
    /// one server without setting it for every server in the process.
    pub desktop_app_override: Option<bool>,
}

impl GotchaApp for RemoteHandApp {
    type State = AppState;
    type Config = AppConfig;

    fn config(&self) -> impl std::future::Future<Output = GotchaResult<ConfigWrapper<Self::Config>>> + Send {
        let home_override = self.home_override.clone();
        let port_override = self.port_override;
        let instance_override = self.instance_override.clone();
        let auth_enabled_override = self.auth_enabled_override;
        async move {
            let mut config = droppen_http::config::load(std::env::var("GOTCHA_ACTIVE_PROFILE").ok())?;
            if let Some(home) = home_override {
                config.app.home_dir = Some(home.to_string_lossy().into_owned());
            }
            if let Some(port) = port_override {
                config.server.port = port;
            }
            if let Some(instance) = instance_override {
                config.app.instance = Some(instance);
            }
            if let Some(enabled) = auth_enabled_override {
                config.app.auth.enabled = Some(enabled);
            }
            Ok(config)
        }
    }

    fn routes(&self, router: GotchaRouter<GotchaContext<Self::State, Self::Config>>) -> GotchaRouter<GotchaContext<Self::State, Self::Config>> {
        crate::api::routes(router)
    }

    fn state(&self, config: &ConfigWrapper<Self::Config>) -> impl std::future::Future<Output = GotchaResult<Self::State>> + Send {
        let home = config.app.home();
        let instance = config.app.instance();
        let auth_settings = config.app.auth.clone();
        let plugins_config = config.app.plugins.clone();
        let auth_disabled = self.auth_disabled;
        async move {
            // The instance is part of the path set: the Claude root this server
            // owns lives inside its instance directory, so a `Paths` that did not
            // know the name would hand every server the same one.
            let paths = Arc::new(Paths::for_instance(home, &instance));
            // Derived here rather than handed in, so the framework's own `state`
            // path and `prepare`'s agree by construction: both read the same two
            // config fields.
            let instance = Arc::new(InstanceDirs::new(&paths.data_dir(), &instance).map_err(GotchaError::message)?);
            // A pending aggregator key from before chain v3 is deleted once at
            // boot, whatever the link and cloud state: nothing in this build
            // writes it, and while it exists the status route would keep
            // reporting `pending` for ever.
            droppen_aggregator::sweep_legacy_key(paths.as_ref());
            // Attachments uploaded but never sent accumulate; sweep the stale
            // ones once at boot, off the startup path so a slow disk cannot
            // delay the listener.
            tokio::spawn({
                let roots = droppen_files::FileRoots::of(&paths);
                async move { roots.sweep_uploads().await }
            });
            let auth = droppen_auth::Auth::new(&paths, &instance, &auth_settings, auth_disabled, Arc::new(NotifyTokens));
            let logins = logins();
            let drive = crate::drive::Drive::new(paths.clone(), &instance, auth, transcript_sources(), register_providers, logins.clone());
            let feeds = FeedRegistry::new();
            let global = GlobalFeed::new();
            // A session's driver changes things no file shows — an automatic
            // account rotation above all — and has to be able to say so to the
            // two feeds that would otherwise only notice on their next poll.
            let refresh = droppen_http::refresh::Refresh::new(global.clone());
            drive.injections.attach_listeners(feeds.clone(), refresh.clone());
            drive.injections.attach_alerts(Arc::new(PushAlerts {
                transcripts: drive.transcripts.clone(),
            }));
            // Found once, here: an entry that registers no ACP provider fails
            // to start rather than on the first request for an agent.
            let acp = drive
                .providers
                .provider::<crate::provider::acp::Acp>()
                .ok_or_else(|| GotchaError::message("no ACP provider is registered"))?
                .runs()
                .clone();
            Ok(AppState {
                acp,
                cloud: droppen_cloud::Cloud::new(paths.as_ref().clone()),
                // Touches nothing yet: the installed packages are read by the
                // first status, the instance database by the first switch or
                // store, and `boot` below.
                plugins: droppen_plugins::Plugins::load(&paths, &plugins_config, Some(refresh.clone())),
                chats: crate::chats::Chats::new(paths.clone()),
                drive,
                terminals: droppen_workbench::terminals::Registry::new(paths.clone(), instance.clone()),
                apps: droppen_workbench::apps::Registry::new(paths.clone(), instance.clone()),
                setup: crate::setup::Setup::new(paths.clone(), &logins),
                listen_port: Arc::new(std::sync::OnceLock::new()),
                paths,
                instance,
                started_at: Arc::from(iso8601(std::time::SystemTime::now()).as_str()),
                feeds,
                global,
                refresh,
                cancel: CancellationToken::new(),
                lock: Arc::new(std::sync::OnceLock::new()),
                desktop_app: self.desktop_app_override.unwrap_or_else(crate::desktop_app::from_env),
                send_limit: droppen_http::SEND_LIMIT,
            })
        }
    }

    fn run(self) -> impl std::future::Future<Output = GotchaResult<()>> + Send {
        self.run_server()
    }
}

/// The instance claims [`Prepared::claim_for_process`] keeps. A static is never
/// dropped, so these are released by the process exiting and by nothing else.
static PROCESS_CLAIMS: std::sync::Mutex<Vec<Arc<InstanceLock>>> = std::sync::Mutex::new(Vec::new());

/// A fully assembled application: bound listener, router, and the token that
/// stops both.
pub struct Prepared {
    /// The address the listener actually bound (resolved, so port 0 is usable).
    pub addr: SocketAddr,
    listener: TcpListener,
    router: gotcha::axum::Router,
    cancel: CancellationToken,
    cleanups: Cleanups,
    /// Held so a setup code can die with the server that minted it.
    auth: Arc<droppen_auth::Auth>,
    /// This server's claim on its instance directory. Held for the whole of
    /// `serve`; dropping it would let a second server adopt what this one owns.
    /// Shared, weakly, with [`crate::api::AppState`] so the self-upgrade handler
    /// can ask whether the claim still means anything before replacing the
    /// binary. The serve task holds it, and a production server also keeps it
    /// for the rest of its process: see [`Prepared::claim_for_process`].
    _lock: Option<Arc<InstanceLock>>,
}

impl RemoteHandApp {
    /// Bind, serve, and stop on the first SIGINT/SIGTERM.
    pub async fn run_server(self) -> GotchaResult<()> {
        self.prepare().await?.serve_until_signalled().await
    }

    /// Load configuration, build state and routes, and bind the listener.
    ///
    /// Split out of `run` so `export-openapi` and the integration tests can take
    /// the address the server actually got — which the framework's own `run`
    /// keeps to itself.
    pub async fn prepare(self) -> GotchaResult<Prepared> {
        self.logger()?;
        for line in droppen_base::env::unread_dirs() {
            tracing::warn!("{line}");
        }
        let config = self.config().await?;

        // Ownership first, before anything reads — let alone adopts — a shim
        // socket, a cursor file or a run record. A second server on the same
        // instance stops here with a message naming the holder — and, because
        // the error stays typed rather than becoming a message, with the exit
        // code `up` reads as "wait for the holder" instead of "bad binary".
        let paths = Paths::for_instance(config.app.home(), &config.app.instance());
        let dirs = InstanceDirs::new(&paths.data_dir(), &config.app.instance()).map_err(GotchaError::message)?;
        let lock = if self.skip_lock {
            None
        } else {
            let lock = InstanceLock::acquire(&dirs).map_err(droppen_supervisor::supervisor::typed_instance_error)?;
            tracing::info!(instance = dirs.name(), lock = %lock.path().display(), "instance locked");
            Some(Arc::new(lock))
        };
        // `skip_lock` still needs the tree to exist: `state` builds a terminal
        // registry rooted in it either way.
        dirs.create().map_err(GotchaError::message)?;

        let state = self.state(&config).await?;
        if let Some(lock) = &lock {
            // The handler that replaces the binary needs to ask whether this
            // claim still holds; a state built without a lock (export-openapi,
            // tests) leaves the cell empty and that handler refuses.
            let _ = state.lock.set(Arc::downgrade(lock));
        }
        // Adoption is the first thing the *owner* does, and it happens before
        // the listener exists: `GET /api/terminals` must never answer from a
        // half-scanned instance directory.
        //
        // "Before the listener" is bounded, though, and deliberately so.
        // [`droppen_workbench::terminals::Registry::readopt_all`] gives the whole instance
        // directory one budget and lets whatever is still handshaking outlive
        // it; a shim that has wedged costs seconds, and a boot that waited for
        // all of them is a server that never starts. The terminals that land
        // late register themselves and simply show up a moment after the first
        // list — which is the trade this makes: a tab that appears late, rather
        // than a server that does not appear at all.
        //
        // It is also gated on the lock, and that gate is load-bearing rather
        // than tidy. `export-openapi` runs a throwaway server in-process, often
        // on a developer's machine while the real one is up; without the gate it
        // would attach to that server's live shims, advance their cursors and
        // sweep the sidecars of any it could not reach. Adoption without
        // ownership is exactly the thing this whole module exists to prevent.
        if let Some(held) = &lock {
            // The flock is on the inode; a deleted or replaced lock file means
            // the claim no longer keeps anyone else out. Adopting under a lost
            // claim is how two servers end up owning one shim, so a lost lock
            // skips adoption entirely — the server still serves, it just
            // touches nothing that needs ownership.
            if let Err(lost) = held.verify_still_held() {
                tracing::error!(%lost, "instance lock lost before adoption; skipping all adoption");
            } else {
                // Also gated on the lock, and for the same reason everything below
                // is: a throwaway server sweeping `<data>/bin` would delete the
                // staging file of an upgrade the real server has in flight. At
                // boot as well as before each upgrade, because the two catch
                // different things: this one clears a file left by a process that
                // was killed outright, and the per-upgrade one keeps a server that
                // is never restarted from accumulating them.
                droppen_supervisor::selfupdate::store::BinStore::new(&paths).sweep_staging();
                // The Claude root remote-hand owns may still be at the path
                // every instance shared before roots were per-instance; the
                // `default` instance claims it here, before anything lists a
                // session out of either place. Renaming a directory two servers
                // could be reading is an owner's move, so it sits behind the
                // same lock every adoption below does.
                crate::claude_roots::migrate_legacy_root(&paths);
                // After the move, so a legacy root's leftovers are swept too:
                // an adoption by an earlier build, interrupted between its copy
                // and its rename, leaves a full transcript nothing will ever
                // list.
                crate::sessions::claude::adoption_leftovers::sweep_partial_adoptions(&paths);
                // Who this server is, minted once and kept for ever after.
                // Under the lock like everything else here: it writes to the
                // instance's database, and a throwaway in-process server must
                // not mint an identity into the real one's file. Nothing reads
                // the uuid yet — see [`droppen_instance::identity`] for why it is written
                // before anything does.
                droppen_instance::identity::ensure(&paths);
                // The directories the user adopted for a CLI found off the
                // PATH, back on every child's PATH. Under the lock because the
                // PATH is the whole process's: a throwaway in-process server
                // must not rewrite it.
                state.setup.apply_overlay();
                // The registry is the one piece of ownership state that cannot
                // be rebuilt from the filesystem, so the owner tidies it here
                // rather than on every list refresh.
                crate::sessions::claude::ClaudeCodeSessions.prune_owned(&paths).await;
                // A codex thread this instance made before claims existed has a
                // registry entry and no claim, which is exactly a thread the
                // sibling instances still list. Mirroring the registry out here
                // means each instance's own boot is what withdraws its threads
                // from everybody else's list. Pruning follows, so the shared
                // directory does not keep a file per thread the user deleted;
                // only this instance's own claims are ever dropped.
                crate::sessions::codex_claims::migrate(&paths).await;
                crate::sessions::codex_claims::prune(&paths).await;
                let adopted = state.terminals.readopt_all().await;
                if adopted > 0 {
                    tracing::info!(adopted, "re-adopted surviving terminals");
                }
                let runs = state.drive.providers.adopt_all().await;
                if !runs.is_empty() {
                    tracing::info!(
                        recovered = runs.recovered,
                        finished = runs.finished,
                        deferred = runs.deferred,
                        "re-adopted surviving agent runs"
                    );
                }
                let adopted = state.apps.readopt_all().await;
                if adopted > 0 {
                    tracing::info!(adopted, "re-adopted surviving apps");
                }
                // Inside the lock for the same reason every adoption above is,
                // and it is the sharpest case of the rule: this loop *starts
                // CLIs*. `export-openapi` builds a throwaway in-process server on
                // a developer's machine, and every integration test builds one
                // per case; a runner outside this gate would have each of them
                // fire the user's real nine-o'clock task. Acting on a schedule
                // without owning the instance is exactly the thing this block
                // exists to prevent.
                crate::schedule::runner::spawn(&crate::schedule::ScheduleState::from_ref(&state));
                // The quota fallback probe starts CLIs as well — against the
                // usage logins every instance on this machine shares — so it
                // sits behind the same gate. It only probes a login nothing has
                // read for a while, and never one another process is probing.
                crate::accounts::quota::spawn_fallback_ticker(state.drive.quota.clone(), state.drive.accounts.clone(), state.cancel.clone());
            }
        }
        let cancel = state.cancel.clone();
        let desktop_app = state.desktop_app;
        let auth = state.drive.auth.clone();
        let host = config.server.host.clone();
        let cloud = state.cloud.control.clone();
        let link_settings = state.cloud.clone();
        let drive_for_link = state.drive.clone();
        let aggregator_switch = state.cloud.aggregator_switch();
        let listen_port = state.listen_port.clone();
        let cleanups = Cleanups {
            feeds: state.feeds.clone(),
            global: state.global.clone(),
            terminals: state.terminals.clone(),
            apps: state.apps.clone(),
        };

        let context = GotchaContext {
            config: config.clone(),
            state,
        };
        let router = self.build_router(context).await?;
        // The `/embed/{id}/*` proxy is composed here rather than in
        // [`crate::api::routes`] for two reasons: it needs the apps registry,
        // which does not exist yet when the framework asks for routes, and it
        // needs the raw request — websocket upgrades included — which only a
        // tower `Service` sees. Re-registering the fallback puts it in front of
        // the SPA service, which it delegates every non-embed path to, so the
        // static-file behaviour is exactly what `api::routes` set up.
        let router = router.fallback_service(droppen_workbench::apps::embed::fallback(cleanups.apps.clone(), crate::static_files::service()));
        // The authentication guard goes on **here**, after the last
        // `fallback_service`, and that ordering is the whole point.
        // `axum::Router::layer` wraps the routes and the fallback *as they stand
        // when it is called*; mounting the guard inside `crate::api::routes` —
        // where `droppen_http::origin`'s sits — would cover the documented routes and
        // leave `/openapi.json`, `/redoc`, `/scalar`, the SPA fallback and the
        // `/embed/{id}/*` proxy outside it. Those are the first paths anyone
        // without a token would try.
        //
        // Being the outermost layer also puts it in front of the origin guard,
        // which is why it hands `OPTIONS` straight through: a browser sends no
        // credentials on a preflight, and a `401` there would break every
        // cross-origin request before the real one was made.
        let auth_for_link = auth.clone();
        let router = router.layer(droppen_auth::layer(auth));
        // Outside even that: a request addressed to a name this server does
        // not answer to (`droppen_http::origin`, "Which names the server answers
        // to") reaches nothing, the exempt sign-in routes included.
        let router = router.layer(droppen_http::origin::host_layer(std::sync::Arc::new(droppen_http::origin::HostPolicy::from_env(
            &config.server.host,
        ))));

        let bind = format!("{}:{}", config.server.host, config.server.port);
        let listener = TcpListener::bind(&bind)
            .await
            .map_err(|source| GotchaError::Bind { addr: bind.clone(), source })?;
        let addr = listener.local_addr().map_err(GotchaError::Io)?;
        // The address the desktop app's window is at, `127.0.0.1:<port>`, is
        // what the first-run guide's install routes check a request against
        // (`http::app_window`). Recorded once, here; before this, nothing
        // counts as the app's window.
        listen_port
            .set(addr.port())
            .expect("the listen port is recorded once, by the boot that bound it");
        // Same rule: the port file says where *the owner* is listening, and a
        // throwaway server writing its own would send every reader somewhere
        // that is about to stop existing.
        if lock.is_some() {
            dirs.write_port(addr.port());
            // Who may use the page, said where the person starting the server
            // reads: a loud warning when nobody is asked, and — when there is
            // no account yet — the one-time setup code for the first one.
            // Only the owner: a throwaway server must not mint a code into the
            // real one's file, nor shout about a port nobody will use.
            let url = droppen_auth::setup::local_url(&host, addr.port());
            if auth_for_link.is_off() {
                warn_auth_off(&url);
            } else {
                // Not on the desktop app's instance, which never leaves its
                // setup state (its account is app-internal): a code on file
                // there would let any process of that user make an account. Its
                // first account is made on a page the app opens as the owner
                // (`droppen_auth::local_open`, "Not on the desktop app's
                // instance").
                if desktop_app {
                    droppen_auth::setup::remove_code_file(&droppen_auth::setup::code_file(&paths));
                } else {
                    auth_for_link.setup.arm_for(&paths, url);
                }
                auth_for_link.in_setup().await;
                // The local capability: `remote-hand-server open` asks this
                // socket — which only the OS user running this server can
                // reach — for a one-time token that opens the page signed in
                // as this device's owner (`droppen_auth::local_open`).
                let page = droppen_auth::local_open::page_url(addr);
                if let Err(error) = droppen_auth::local_open::ControlSocket::of(&paths).serve(auth_for_link.clone(), page, desktop_app, cancel.clone()) {
                    tracing::error!(%error, "cannot listen on the control socket; `remote-hand-server open` will not work");
                }
                // And again every few seconds, so that an account made or
                // removed with the CLI while this server runs ends — or
                // begins — the setup state's code without any request having
                // to change state to notice. Leaving the setup state (and
                // booting outside it) is also when the device's owner stops
                // being one: its sessions go — so that removing every account
                // later cannot bring an old one back — and a Droppen link it
                // signed in passes to the first account.
                tokio::spawn({
                    let auth = auth_for_link.clone();
                    let link_settings = link_settings.clone();
                    let cancel = cancel.clone();
                    async move {
                        let mut tick = tokio::time::interval(droppen_auth::SETUP_TICK);
                        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                        let mut was_setup = None;
                        loop {
                            tokio::select! {
                                () = cancel.cancelled() => break,
                                _ = tick.tick() => {
                                    let setup = auth.in_setup().await;
                                    if !setup && was_setup != Some(false) {
                                        let ended = auth.sessions.revoke_user(droppen_auth::LOCAL_OWNER, None).await;
                                        if ended > 0 {
                                            tracing::info!(ended, "this device has an account: the owner's open pages are signed out");
                                        }
                                        link_settings.hand_over_from_local_owner(&auth.users).await;
                                    }
                                    was_setup = Some(setup);
                                }
                            }
                        }
                    }
                });
            }
            // The cloud agent's link to its hub — only by the owner (never a
            // throwaway in-process server), and at boot only for an enrolled
            // instance: one that was never enrolled starts nothing here. From
            // now on the local settings can enroll it, and start or stop the
            // link, without a restart.
            cloud.attach(droppen_agent::link::LinkEnv {
                addr,
                credential: auth_for_link.internal_credential().to_string(),
                streams: Arc::new(crate::stream_source::SessionStreams::new(
                    drive_for_link.paths.clone(),
                    drive_for_link.transcripts.clone(),
                    drive_for_link,
                )),
                host: Arc::new(PushLinkHost),
                cancel: cancel.clone(),
                aggregator_switch,
            });
            // A device linked to Droppen reaches other devices from its page;
            // with authentication off that page answers anyone, so the link
            // stays down until it is back on.
            if auth_for_link.is_off() {
                tracing::warn!("authentication is off: not connecting this device's Droppen link");
            } else {
                cloud.start_if_enrolled().await;
            }
        }
        Ok(Prepared {
            addr,
            listener,
            router,
            cancel,
            cleanups,
            auth: auth_for_link,
            _lock: lock,
        })
    }
}

impl Prepared {
    /// The token that stops this server; also lives in every handler's state.
    pub fn cancel(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// The live session feeds, so a caller driving the server itself can assert
    /// they were torn down.
    pub fn feeds(&self) -> std::sync::Arc<FeedRegistry> {
        self.cleanups.feeds.clone()
    }

    /// The shared session-list feed, for the same reason.
    pub fn global(&self) -> std::sync::Arc<GlobalFeed> {
        self.cleanups.global.clone()
    }

    /// Everything the shutdown supervisor would tear down. A caller driving the
    /// server itself (`export-openapi`, the integration tests) reaches the
    /// subsystem registries through this, and can run the teardown by hand.
    pub fn cleanups(&self) -> Cleanups {
        self.cleanups.clone()
    }

    /// Serve in a task, and stop on the first SIGINT/SIGTERM.
    ///
    /// The instance stays claimed until this process exits, not merely until
    /// the serve task ends; see [`Prepared::claim_for_process`].
    pub async fn serve_until_signalled(self) -> GotchaResult<()> {
        self.claim_for_process();
        tracing::info!(addr = %self.addr, "Droppen server listening");
        let cancel = self.cancel.clone();
        let cleanups = self.cleanups.clone();
        let mut server = tokio::spawn(self.serve_task());

        tokio::select! {
            // The server stopped on its own (bind lost, accept loop failed).
            result = &mut server => return join(result),
            () = shutdown::supervise(cancel, cleanups) => {}
        }

        // Cancelled: let in-flight requests finish, but never hang on one.
        match tokio::time::timeout(DRAIN_GRACE, server).await {
            Ok(result) => join(result),
            Err(_) => {
                tracing::warn!("drain timed out; exiting anyway");
                Ok(())
            }
        }
    }

    /// Keep this server's instance claim until the process exits.
    ///
    /// The serve task lets go of its claim when it returns. That suits a server
    /// stopped inside a process that goes on running, which is what the
    /// integration tests do between a halt and the next boot, after
    /// cancelling it and running its cleanups. It does not suit the production
    /// server, whose process can go on for a while after the serve task has
    /// returned. When the serve task fails on its own, nothing has cancelled
    /// the background tasks, and the scheduler's `fire` writes the instance
    /// directory. Meanwhile `up` (`droppen_supervisor::supervisor`) starts a successor as
    /// soon as the flock is free, because it counts on the flock going only
    /// when its holder dies. Held here, it does.
    ///
    /// A static rather than a handle for `main` to keep: both callers of
    /// [`RemoteHandApp::run_server`] (`serve` and `agent run`) return straight
    /// into the end of `main`, so "until the process exits" is the lifetime
    /// needed either way, and a static cannot be dropped early by a caller.
    fn claim_for_process(&self) {
        if let Some(lock) = &self._lock {
            PROCESS_CLAIMS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(lock.clone());
        }
    }

    /// Serve until the token is cancelled, without installing a signal handler.
    /// Used by `export-openapi` and by the integration tests.
    pub fn serve_task(self) -> impl std::future::Future<Output = std::io::Result<()>> + Send {
        let Prepared {
            listener,
            router,
            cancel,
            auth,
            _lock,
            ..
        } = self;
        async move {
            // `_lock` is moved in deliberately: the instance stays claimed for
            // as long as this server is serving. A production server keeps it
            // past that, until its process exits (`claim_for_process`).
            let _lock = _lock;
            // With the peer address, which `auth::guard` needs to accept the
            // cloud agent's bridge only from loopback.
            let served = gotcha::axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
                .with_graceful_shutdown(async move { cancel.cancelled().await })
                .await;
            // A deletion of the other devices' copies still running (the
            // aggregator turned off just before) gets a moment to finish.
            droppen_aggregator::finish_deletions(std::time::Duration::from_secs(5)).await;
            // A setup code is only good while its server runs; the file must
            // not outlive it and be read back as if it still worked.
            auth.setup.disarm();
            served
        }
    }
}

/// The warning a server with authentication switched off starts with: on
/// standard error, where the person who started it looks, and in the log.
fn warn_auth_off(url: &str) {
    let rule = "!".repeat(72);
    eprintln!(
        "\n{rule}\n  Droppen: LOCAL AUTHENTICATION IS OFF (auth.enabled = false).\n\n  Anyone who can reach {url}\n  has full access to this device: its files, its shells and the AI\n  CLIs it runs, with no login. This is for development and tests only.\n  Remove auth.enabled / APP_AUTH__ENABLED to require a login.\n{rule}\n"
    );
    tracing::warn!(
        url,
        "local authentication is off (auth.enabled = false): every route answers anyone who can reach the port"
    );
}

fn join(result: Result<std::io::Result<()>, tokio::task::JoinError>) -> GotchaResult<()> {
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(GotchaError::Io(error)),
        Err(error) => Err(GotchaError::message(error)),
    }
}

/// Every CLI's transcripts, for whatever lists or reads sessions. Each reader
/// names its own [`Source`]; this is the one place that names all of them.
pub fn transcript_sources() -> TranscriptSources {
    let mut sources = TranscriptSources::default();
    sources.register(Arc::new(crate::sessions::claude::ClaudeCodeSessions));
    sources.register(Arc::new(crate::sessions::codex::CodexSessions));
    sources.register(Arc::new(crate::sessions::acp_transcript::AcpSessions));
    sources
}

/// Every CLI this server drives, over the runs they read their state from.
/// Each provider names its own [`Source`]; this is the one place that names
/// all of them.
pub fn register_providers(providers: &mut ProviderRegistry, registry: &Arc<InjectionRegistry>) {
    providers.register(Arc::new(crate::provider::resident::ClaudeCode::new(registry.clone())));
    providers.register(Arc::new(crate::provider::app_server::Codex::new(registry.clone())));
    providers.register(Arc::new(crate::provider::acp::Acp::new(registry.clone())));
}

/// Every CLI's login, as the accounts are handed them: the one place that
/// names each provider's own implementation.
pub fn logins() -> Logins {
    Logins {
        claude: Arc::new(ClaudeCodeLogin),
        codex: Arc::new(CodexLogin),
    }
}

/// The drivers' alerts, sent by [`droppen_notify::push`]: Web Push to every subscribed
/// browser, and the Bark endpoint too for a question or a rotation. Each push
/// function spawns its sending and returns, as [`crate::provider::Alerts`] asks.
struct PushAlerts {
    /// What the alerts read the session's row through.
    transcripts: TranscriptSources,
}

impl crate::provider::Alerts for PushAlerts {
    fn turn(&self, paths: Arc<Paths>, source: Source, id: String, outcome: InjectionRunState, error: Option<String>, driver: DriverStates) {
        droppen_notify::push::notify_turn(paths, self.transcripts.clone(), source, id, outcome, error, driver);
    }

    fn question(&self, paths: Arc<Paths>, source: Source, id: String, question: String) {
        droppen_notify::push::notify_question(paths, self.transcripts.clone(), source, id, question);
    }

    fn rotation(&self, paths: Arc<Paths>, source: Source, id: String, outcome: RotationOutcome, trigger: RotationTrigger, text: String) {
        droppen_notify::push::notify_rotation(paths, self.transcripts.clone(), source, id, outcome, trigger, text);
    }
}

/// The link's push channels, as [`droppen_notify::push`] runs them: the VAPID renewal
/// in the instance's state database, and the account alerts to Web Push and
/// Bark. `notify_account_change` spawns its sending and returns, as
/// [`droppen_agent::link::LinkHost::account_change`] asks.
pub(crate) struct PushLinkHost;

impl droppen_agent::link::LinkHost for PushLinkHost {
    fn renew_push_keys<'a>(&'a self, paths: &'a Paths) -> futures_util::future::BoxFuture<'a, std::io::Result<usize>> {
        Box::pin(droppen_notify::push::store::renew_vapid(paths))
    }

    fn account_change(&self, paths: Arc<Paths>, title: String, body: String) {
        droppen_notify::push::notify_account_change(paths, title, body);
    }
}

/// The plugin host's status, for the session-list socket's opening `settings`
/// frame: the same reading `/api/settings` takes.
pub(crate) struct PluginHost(pub(crate) Arc<droppen_plugins::Plugins>);

impl crate::feed::global::PluginStatuses for PluginHost {
    fn status(&self) -> futures_util::future::BoxFuture<'_, Vec<droppen_plugin_sdk::PluginStatus>> {
        Box::pin(self.0.status_async())
    }
}

/// `POST /api/notify`'s capability as [`droppen_notify::notify`] checks it: the token
/// it mints per session, verified against its stored secret, and the
/// [`droppen_notify::notify::NotifyCaller`] the notify handler reads back out of the
/// request's [`droppen_http::identity::NotifyGrant`].
pub(crate) struct NotifyTokens;

impl droppen_auth::NotifyCredential for NotifyTokens {
    fn is_notify_request(&self, method: &gotcha::axum::http::Method, path: &str) -> bool {
        droppen_notify::notify::is_notify_request(method, path)
    }

    fn caller<'a>(
        &'a self,
        paths: &'a Paths,
        instance: &'a str,
        headers: &'a gotcha::axum::http::HeaderMap,
    ) -> futures_util::future::BoxFuture<'a, Option<droppen_http::identity::NotifyGrant>> {
        Box::pin(async move { droppen_notify::notify::caller(paths, instance, headers).await.map(droppen_http::identity::NotifyGrant::new) })
    }
}

/// The link's streams read the account bindings, the drivers and the drafts
/// from the drive, as the session list does.
impl crate::stream_source::LiveSessions for crate::drive::Drive {
    fn bindings(&self) -> futures_util::future::BoxFuture<'_, std::collections::BTreeMap<String, String>> {
        Box::pin(self.accounts.session_account_map())
    }

    fn driver_states(&self) -> DriverStates {
        self.providers.driver_states()
    }

    fn drafts(&self, source: Source, id: &str) -> Option<Box<dyn droppen_agent::streams::source::DraftFeed>> {
        self.providers
            .draft_watch(source, id)
            .map(|rx| Box::new(DraftWatch(rx)) as Box<dyn droppen_agent::streams::source::DraftFeed>)
    }
}

/// A claude session's draft, as its driver publishes it.
struct DraftWatch(tokio::sync::watch::Receiver<crate::provider::drafts::Draft>);

impl droppen_agent::streams::source::DraftFeed for DraftWatch {
    fn changed(&mut self) -> futures_util::future::BoxFuture<'_, bool> {
        Box::pin(async move { tokio::sync::watch::Receiver::changed(&mut self.0).await.is_ok() })
    }

    fn seen(&mut self) -> droppen_agent::streams::source::Draft {
        let draft = self.0.borrow_and_update().clone();
        droppen_agent::streams::source::Draft {
            v: draft.v,
            body: serde_json::to_value(&draft).unwrap_or(serde_json::Value::Null),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The server's link host renews the push (VAPID) keypair, and a renewal
    /// the state database refuses fails and leaves the keypair as it was:
    /// what the link's retry counts on
    /// (`agent::link::tests::the_push_keys_are_renewed_again_after_a_failure_without_a_restart`).
    #[tokio::test]
    async fn the_push_link_host_renews_the_vapid_keys_and_fails_when_refused() {
        use droppen_agent::link::LinkHost;
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "agentlink");
        let old = droppen_notify::push::store::vapid_public(&paths).await.unwrap();
        let db = droppen_instance::db::at(paths.instance_db());
        db.write(|tx| tx.execute_batch("CREATE TRIGGER refuse_push BEFORE UPDATE ON kv WHEN NEW.scope = 'push' BEGIN SELECT RAISE(ABORT, 'refused (test)'); END;"))
            .unwrap();
        assert!(PushLinkHost.renew_push_keys(&paths).await.is_err(), "refused");
        assert_eq!(droppen_notify::push::store::vapid_public(&paths).await.unwrap(), old);
        db.write(|tx| tx.execute_batch("DROP TRIGGER refuse_push")).unwrap();
        PushLinkHost.renew_push_keys(&paths).await.expect("renewed");
        assert_ne!(droppen_notify::push::store::vapid_public(&paths).await.unwrap(), old, "a new push key");
    }

    /// Each CLI is registered twice, for its transcripts and for its runs, in
    /// two functions here. A CLI in one and not the other would list sessions
    /// nothing can drive, or drive sessions nothing lists, and both lists are
    /// in [`Source`]'s order.
    #[test]
    fn every_provider_is_registered_with_its_transcripts() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Arc::new(Paths::new(home.path().to_path_buf()));
        let dirs = InstanceDirs::new(&paths.data_dir(), "providers-test").expect("valid name");
        dirs.create().expect("instance dirs");
        let accounts = Arc::new(crate::accounts::store::AccountsStore::new(paths.clone(), logins()));
        let quota = Arc::new(crate::accounts::quota::QuotaCache::new(paths.clone(), crate::app::logins()));
        let decoration = crate::drive::NotifyDecoration::new(paths.clone(), dirs.clone());
        let providers = ProviderRegistry::new(InjectionRegistry::new(paths, &dirs, accounts, quota, decoration), register_providers);

        let transcripts: Vec<Source> = transcript_sources().iter().map(|reader| reader.source()).collect();
        assert_eq!(providers.sources().collect::<Vec<_>>(), transcripts);
        assert_eq!(transcripts, [Source::Claude, Source::Codex, Source::Acp], "every source has both");
    }

    /// An ownership row is written with its own CLI's step after it: the
    /// writer takes the row's source from the reader whose step it runs, so a
    /// Codex row always comes with the thread's claim, and no other CLI's row
    /// writes one. Each reader's own `record_owned` is the same writer.
    #[tokio::test]
    async fn every_cli_records_ownership_with_its_own_step() {
        use droppen_sessions::owned_sessions::{self, OwnedSessions};
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        let claimed = |id: &str| paths.codex_claims_dir().join(id).exists();

        for reader in transcript_sources().iter() {
            let id = format!("{}-a", reader.source().as_str());
            OwnedSessions::record(&paths, reader, &id).await.expect("record");
            assert_eq!(claimed(&id), reader.source() == Source::Codex, "{} runs its own step", reader.source().as_str());
        }
        crate::sessions::claude::ClaudeCodeSessions.record_owned(&paths, "claude-b").await.expect("record");
        crate::sessions::codex::CodexSessions.record_owned(&paths, "codex-b").await.expect("record");

        let owned = owned_sessions::read(&paths).await;
        assert_eq!(owned.ids(Source::Codex).collect::<Vec<_>>(), ["codex-a", "codex-b"]);
        assert!(owned.ids(Source::Codex).all(claimed), "every codex row has its claim");
        assert_eq!(owned.ids(Source::Claude).collect::<Vec<_>>(), ["claude-a", "claude-b"]);
        assert!(!owned.ids(Source::Claude).any(claimed), "a claude row claims nothing");
        assert!(!owned.contains(Source::Acp, "acp-a"), "no ACP row is read back");
    }

    /// What each CLI's sessions are offered for: only Claude Code stops to ask
    /// questions, Claude Code runs its instant chats with every tool off, so
    /// they take no attachments, and an ACP agent runs neither instant chats
    /// nor scheduled tasks.
    #[test]
    fn every_provider_says_what_its_sessions_are_offered_for() {
        use crate::provider::Capabilities;
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Arc::new(Paths::new(home.path().to_path_buf()));
        let dirs = InstanceDirs::new(&paths.data_dir(), "capabilities-test").expect("valid name");
        dirs.create().expect("instance dirs");
        let accounts = Arc::new(crate::accounts::store::AccountsStore::new(paths.clone(), logins()));
        let quota = Arc::new(crate::accounts::quota::QuotaCache::new(paths.clone(), crate::app::logins()));
        let decoration = crate::drive::NotifyDecoration::new(paths.clone(), dirs.clone());
        let providers = ProviderRegistry::new(InjectionRegistry::new(paths, &dirs, accounts, quota, decoration), register_providers);

        let all = Capabilities {
            questions: true,
            chats: true,
            chat_attachments: true,
            schedules: true,
        };
        assert_eq!(
            providers.capabilities(Source::Claude),
            Capabilities {
                chat_attachments: false,
                ..all
            }
        );
        assert_eq!(providers.capabilities(Source::Codex), Capabilities { questions: false, ..all });
        assert_eq!(providers.capabilities(Source::Acp), Capabilities::default());
    }

    /// The instance is free as soon as the serve task ends, whatever still holds
    /// the server's state.
    ///
    /// The state is cloned into every handler and background task, and some of
    /// them end after the serve task does. A state that held the lock strongly
    /// would keep the instance claimed until the last of them ended, and a
    /// server restarted in the same process (which is what the restart tests
    /// do) would be refused on a busy machine.
    #[tokio::test]
    async fn the_instance_is_released_when_serving_ends_while_the_state_lives_on() {
        let home = tempfile::tempdir().expect("tempdir");
        let prepared = RemoteHandApp {
            home_override: Some(home.path().to_path_buf()),
            port_override: Some(0),
            auth_enabled_override: Some(false),
            ..Default::default()
        }
        .prepare()
        .await
        .expect("prepare");
        // The router carries the state, as every handler does. Kept past the
        // serve task, it stands in for whichever holder happens to end last.
        let lingering = prepared.router.clone();
        let cancel = prepared.cancel();
        let cleanups = prepared.cleanups();
        let serving = tokio::spawn(prepared.serve_task());
        cancel.cancel();
        cleanups.run();
        serving.await.expect("the serve task").expect("serve");

        let dirs = InstanceDirs::new(&Paths::new(home.path().to_path_buf()).data_dir(), droppen_instance::instance::DEFAULT_INSTANCE)
            .expect("the default instance name is valid");
        InstanceLock::acquire(&dirs).expect("the instance is free once its server has stopped serving");
        drop(lingering);
    }

    /// A production server's claim outlives its serve task, so whatever its
    /// process still runs before it exits never shares the instance with a
    /// successor. `serve_until_signalled` takes this claim first thing; it is
    /// taken directly here so the test does not install the process's signal
    /// handlers.
    #[tokio::test]
    async fn a_production_claim_lasts_past_the_serve_task() {
        let home = tempfile::tempdir().expect("tempdir");
        let prepared = RemoteHandApp {
            home_override: Some(home.path().to_path_buf()),
            port_override: Some(0),
            auth_enabled_override: Some(false),
            ..Default::default()
        }
        .prepare()
        .await
        .expect("prepare");
        prepared.claim_for_process();
        let cancel = prepared.cancel();
        let cleanups = prepared.cleanups();
        let serving = tokio::spawn(prepared.serve_task());
        cancel.cancel();
        cleanups.run();
        serving.await.expect("the serve task").expect("serve");

        let dirs = InstanceDirs::new(&Paths::new(home.path().to_path_buf()).data_dir(), droppen_instance::instance::DEFAULT_INSTANCE)
            .expect("the default instance name is valid");
        let refused = InstanceLock::acquire(&dirs).expect_err("the instance is still claimed after serving");
        assert!(refused.to_string().contains(&format!("pid {}", std::process::id())), "by this process: {refused}");
    }

    /// Whether a session can be sent to, through the readers registered here:
    /// each CLI's refusals are its own reader's ([`TranscriptSources::sendability`]).
    mod sendability {
        use super::*;
        use droppen_sessions::{SessionScope, SessionStatus};

        #[test]
        fn a_live_claude_session_cannot_be_sent_to() {
            let sources = crate::app::transcript_sources();
            let owned = SessionScope::Owned;
            assert!(sources.sendability(Source::Claude, owned, SessionStatus::Idle, false, false).0);
            assert!(!sources.sendability(Source::Claude, owned, SessionStatus::Waiting, false, false).0);
            assert!(sources.sendability(Source::Codex, owned, SessionStatus::Waiting, false, false).0);
            assert!(!sources.sendability(Source::Codex, owned, SessionStatus::Running, false, false).0);
        }

        #[test]
        fn an_in_flight_injection_outranks_every_other_verdict() {
            let sources = crate::app::transcript_sources();
            let (can_send, reason) = sources.sendability(Source::Codex, SessionScope::Owned, SessionStatus::Idle, true, false);
            assert!(!can_send);
            assert_eq!(reason.as_deref(), Some("a message from Droppen is still running"));
        }

        /// The two arms a resident lifts, and the one it does not.
        ///
        /// Both of the block arms below would fire for a healthy resident, and each
        /// would be wrong in the same way: the busy CLI they are describing is this
        /// server's own, taking messages in order. If either survived, a session
        /// would answer its first message and refuse every one after it.
        #[test]
        fn a_resident_session_takes_messages_while_it_is_busy() {
            let sources = crate::app::transcript_sources();
            let owned = SessionScope::Owned;
            assert!(
                sources.sendability(Source::Claude, owned, SessionStatus::Idle, true, true).0,
                "a turn in flight is queued behind, not refused"
            );
            assert!(
                sources.sendability(Source::Claude, owned, SessionStatus::Waiting, false, true).0,
                "the live claude the probe found is the resident this server started"
            );
            // Ownership is the one thing it does not lift.
            assert!(!sources.sendability(Source::Claude, SessionScope::External, SessionStatus::Idle, false, true).0);
        }

        /// An idle, unlocked, perfectly healthy session this server did not make is
        /// still not this server's to drive.
        #[test]
        fn somebody_elses_session_is_never_sendable() {
            let sources = crate::app::transcript_sources();
            for source in [Source::Claude, Source::Codex] {
                let (can_send, reason) = sources.sendability(source, SessionScope::External, SessionStatus::Idle, false, false);
                assert!(!can_send, "{source} external sessions are browse-only");
                assert!(reason.is_some_and(|reason| !reason.is_empty()));
            }
            // And ownership is checked before the injection state, which cannot
            // meaningfully exist for a session this server never started.
            assert!(!sources.sendability(Source::Claude, SessionScope::External, SessionStatus::Idle, true, false).0);
        }

        /// Each CLI's refusals, word for word: the composer shows them as they are.
        /// Every registered reader is checked, and the `match` makes a new source
        /// say its words here before this compiles.
        #[test]
        fn every_source_refuses_in_its_own_words() {
            type Live = fn(SessionStatus) -> Option<&'static str>;
            let sources = crate::app::transcript_sources();
            for reader in sources.iter() {
                let source = reader.source();
                let (external, live): (&str, Live) = match source {
                    Source::Claude => (
                        "this session was not started by this instance; continue it where it was started",
                        |status| (status != SessionStatus::Idle).then_some("attached to a live local Claude Code process"),
                    ),
                    Source::Codex => (
                        "this codex session was started elsewhere; continue it where it was started",
                        |status| (status == SessionStatus::Running).then_some("session is currently running"),
                    ),
                    Source::Acp => ("this ACP session was started elsewhere; continue it where it was started", |_| None),
                };
                assert_eq!(
                    sources.sendability(source, SessionScope::External, SessionStatus::Idle, false, false),
                    (false, Some(external.to_string())),
                    "{source}"
                );
                for status in [SessionStatus::Idle, SessionStatus::Running, SessionStatus::Waiting] {
                    let expected = live(status).map_or((true, None), |reason| (false, Some(reason.to_string())));
                    assert_eq!(sources.sendability(source, SessionScope::Owned, status, false, false), expected, "{source} {status:?}");
                }
            }
            // A source with no reader lists no sessions, but its fallback wording
            // still says what happened.
            assert_eq!(
                TranscriptSources::default().sendability(Source::Claude, SessionScope::External, SessionStatus::Idle, false, false),
                (false, Some("this claude session was started elsewhere; continue it where it was started".to_string()))
            );
        }
    }

    /// What the first-run guide runs never names a credential path: a scan of
    /// the guide's files and of every function they call into, in whichever
    /// crate it lives. Here, at the entry, because the entry is the one place
    /// that sees them all by path.
    mod credential_paths {
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
                ("../crates/droppen-base/src/text.rs", &["strip_ansi"][..]),
                ("accounts/login/claude.rs", &["plan_label"][..]),
                ("accounts/quota/codex.rs", &["app_server_call", "ask", "send", "reply"][..]),
                ("accounts/login/codex.rs", &["app_server_call"][..]),
                ("../crates/droppen-base/src/process_env.rs", &["apply_env"][..]),
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
}
