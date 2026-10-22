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

use gotcha::{ConfigWrapper, GotchaApp, GotchaContext, GotchaError, GotchaResult, GotchaRouter};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::api::AppState;
use crate::config::AppConfig;
use crate::feed::global::GlobalFeed;
use crate::feed::FeedRegistry;
use crate::instance::{InstanceDirs, InstanceLock};
use crate::sessions::{iso8601, Paths};
use crate::shutdown::{self, Cleanups};

/// How long a shutdown waits for in-flight requests before giving up.
const DRAIN_GRACE: Duration = Duration::from_secs(5);

/// The remote-hand server.
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
            let mut config = crate::config::load(std::env::var("GOTCHA_ACTIVE_PROFILE").ok())?;
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
            let instance = Arc::new(instance_dirs(&paths, &instance)?);
            // Attachments uploaded but never sent accumulate; sweep the stale
            // ones once at boot, off the startup path so a slow disk cannot
            // delay the listener (the former TypeScript server's `files.ts`
            // does it at import).
            tokio::spawn({
                let paths = paths.clone();
                async move { crate::files::sweep_uploads(paths.as_ref()).await }
            });
            let auth = crate::auth::Auth::new(&paths, &instance, &auth_settings, auth_disabled);
            let drive = crate::drive::Drive::new(paths.clone(), &instance, auth);
            let feeds = FeedRegistry::new();
            let global = GlobalFeed::new();
            // A session's driver changes things no file shows — an automatic
            // account rotation above all — and has to be able to say so to the
            // two feeds that would otherwise only notice on their next poll.
            drive.injections.attach_listeners(feeds.clone(), global.clone());
            Ok(AppState {
                cloud: crate::api::cloud::Cloud::new(paths.as_ref().clone()),
                // Reads only what is compiled in; the disk is touched by the
                // first request that needs a store, and by `boot` below.
                plugins: crate::plugins::Plugins::load(&paths, &plugins_config, Some(global.clone())),
                chats: crate::chats::Chats::new(paths.clone()),
                drive,
                terminals: crate::terminals::Registry::new(paths.clone(), instance.clone()),
                apps: crate::apps::Registry::new(paths.clone(), instance.clone()),
                setup: crate::setup::Setup::new(paths.clone()),
                paths,
                instance,
                started_at: Arc::from(iso8601(std::time::SystemTime::now()).as_str()),
                feeds,
                global,
                cancel: CancellationToken::new(),
                lock: Arc::new(std::sync::OnceLock::new()),
                desktop_app: self.desktop_app_override.unwrap_or_else(crate::desktop_app::from_env),
            })
        }
    }

    fn run(self) -> impl std::future::Future<Output = GotchaResult<()>> + Send {
        self.run_server()
    }
}

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
    auth: Arc<crate::auth::Auth>,
    /// This server's claim on its instance directory. Held for the whole of
    /// `serve`; dropping it would let a second server adopt what this one owns.
    /// Shared with [`crate::api::AppState`] so the self-upgrade handler can ask
    /// whether the claim still means anything before replacing the binary.
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
        let config = self.config().await?;

        // Ownership first, before anything reads — let alone adopts — a shim
        // socket, a cursor file or a run record. A second server on the same
        // instance stops here with a message naming the holder — and, because
        // the error stays typed rather than becoming a message, with the exit
        // code `up` reads as "wait for the holder" instead of "bad binary".
        let paths = Paths::for_instance(config.app.home(), &config.app.instance());
        let dirs = instance_dirs(&paths, &config.app.instance())?;
        let lock = if self.skip_lock {
            None
        } else {
            let lock = InstanceLock::acquire(&dirs).map_err(crate::supervisor::typed_instance_error)?;
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
            let _ = state.lock.set(lock.clone());
        }
        // Adoption is the first thing the *owner* does, and it happens before
        // the listener exists: `GET /api/terminals` must never answer from a
        // half-scanned instance directory.
        //
        // "Before the listener" is bounded, though, and deliberately so.
        // [`crate::terminals::Registry::readopt_all`] gives the whole instance
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
                // staging file of an upgrade the real server has in flight.
                crate::selfupdate::sweep_staging(&paths);
                // The Claude root remote-hand owns used to be shared by every
                // instance; the `default` one claims it here, before anything
                // lists a session out of either place. Renaming a directory two
                // servers could be reading is an owner's move, so it sits behind
                // the same lock every adoption below does.
                crate::claude_roots::migrate_legacy_managed_root(&paths);
                // Same shape, one directory over: the layouts and the push
                // subscriptions used to be shared by every instance and are now
                // this one's own. A copy, so the file the older build still
                // reads is left alone. Before the router exists, because the
                // first `GET /api/screens` must already see what this instance
                // inherited.
                //
                // The desktop app's instance is the exception for the layouts
                // (`instance::Heir::DesktopApp`), and an app an older build
                // already gave them to has them taken back here, untouched ones
                // only (`screens::forget_inherited`).
                let heir = if state.desktop_app {
                    crate::instance::Heir::DesktopApp
                } else {
                    crate::instance::Heir::CommandLine
                };
                crate::instance::migrate_shared_sidecars(&paths, heir);
                if state.desktop_app {
                    crate::screens::forget_inherited(&paths).await;
                }
                // After the move, so a legacy root's leftovers are swept too:
                // an adoption (a feature earlier builds had) interrupted between
                // its copy and its rename left a full transcript nothing will
                // ever list.
                crate::sessions::adoption_leftovers::sweep_partial_adoptions(&paths);
                // Who this server is, minted once and kept for ever after.
                // Under the lock like everything else here: it writes to the
                // instance's database, and a throwaway in-process server must
                // not mint an identity into the real one's file. Nothing reads
                // the uuid yet — see [`crate::identity`] for why it is written
                // before anything does.
                crate::identity::ensure(&paths);
                // The directories the user adopted for a CLI found off the
                // PATH, back on every child's PATH. Under the lock because the
                // PATH is the whole process's: a throwaway in-process server
                // must not rewrite it.
                state.setup.apply_overlay();
                // The plugins' boot step, today only the legacy task ledger's
                // import and reconciliation (`plugins::legacy_tasks`), and only
                // when the build bundles that plugin. Before the listener, so
                // the first `GET /api/settings` already reports the plugin on
                // where the ledger was taken in; behind the lock, because it
                // writes this instance's state and stores.
                let plugins = state.plugins.clone();
                if let Err(error) = tokio::task::spawn_blocking(move || plugins.boot()).await {
                    tracing::error!(%error, "the plugins' boot step panicked");
                }
                // The registry is the one piece of ownership state that cannot
                // be rebuilt from the filesystem, so the owner tidies it here
                // rather than on every list refresh.
                crate::sessions::owned_sessions::prune(&paths).await;
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
                let runs = state.drive.injections.adopt_all().await;
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
                crate::schedule::runner::spawn(&state);
                // The quota fallback probe starts CLIs as well — against the
                // usage logins every instance on this machine shares — so it
                // sits behind the same gate. It only probes a login nothing has
                // read for a while, and never one another process is probing.
                crate::quota::spawn_fallback_ticker(state.drive.quota.clone(), state.drive.accounts.clone(), state.cancel.clone());
            }
        }
        let cancel = state.cancel.clone();
        let desktop_app = state.desktop_app;
        let auth = state.drive.auth.clone();
        let host = config.server.host.clone();
        let cloud = state.cloud.control.clone();
        let link_settings = state.cloud.clone();
        let drive_for_link = state.drive.clone();
        let setup = state.setup.clone();
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
        let router = router.fallback_service(crate::apps::embed::fallback(cleanups.apps.clone(), crate::static_files::service()));
        // The authentication guard goes on **here**, after the last
        // `fallback_service`, and that ordering is the whole point.
        // `axum::Router::layer` wraps the routes and the fallback *as they stand
        // when it is called*; mounting the guard inside `crate::api::routes` —
        // where `crate::origin`'s sits — would cover the documented routes and
        // leave `/openapi.json`, `/redoc`, `/scalar`, the SPA fallback and the
        // `/embed/{id}/*` proxy outside it. Those are the first paths anyone
        // without a token would try.
        //
        // Being the outermost layer also puts it in front of the origin guard,
        // which is why it hands `OPTIONS` straight through: a browser sends no
        // credentials on a preflight, and a `401` there would break every
        // cross-origin request before the real one was made.
        let auth_for_link = auth.clone();
        let router = router.layer(crate::auth::layer(auth));

        let bind = format!("{}:{}", config.server.host, config.server.port);
        let listener = TcpListener::bind(&bind)
            .await
            .map_err(|source| GotchaError::Bind { addr: bind.clone(), source })?;
        let addr = listener.local_addr().map_err(GotchaError::Io)?;
        // The address the desktop app's window is at, `127.0.0.1:<port>`, is
        // what the first-run guide's install routes check a request against
        // (`setup::app_caller`).
        setup.set_listen_port(addr.port());
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
            let url = crate::auth::setup::local_url(&host, addr.port());
            if auth_for_link.is_off() {
                warn_auth_off(&url);
            } else {
                // Not on the desktop app's instance, which never leaves its
                // setup state (its account is app-internal): a code on file
                // there would let any process of that user make an account. Its
                // first account is made on a page the app opens as the owner
                // (`crate::auth::local_open`, "Not on the desktop app's
                // instance").
                if desktop_app {
                    crate::auth::setup::remove_code_file(&crate::auth::setup::code_file(&paths));
                } else {
                    auth_for_link.setup.arm_for(&paths, url);
                }
                auth_for_link.in_setup().await;
                // The local capability: `remote-hand-server open` asks this
                // socket — which only the OS user running this server can
                // reach — for a one-time token that opens the page signed in
                // as this device's owner (`crate::auth::local_open`).
                let page = crate::auth::local_open::page_url(addr);
                if let Err(error) = crate::auth::local_open::serve(&paths, auth_for_link.clone(), page, desktop_app, cancel.clone()) {
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
                        let mut tick = tokio::time::interval(crate::auth::SETUP_TICK);
                        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                        let mut was_setup = None;
                        loop {
                            tokio::select! {
                                () = cancel.cancelled() => break,
                                _ = tick.tick() => {
                                    let setup = auth.in_setup().await;
                                    if !setup && was_setup != Some(false) {
                                        let ended = auth.sessions.revoke_user(crate::auth::LOCAL_OWNER, None).await;
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
            cloud.attach(crate::agent::link::LinkEnv {
                addr,
                credential: auth_for_link.internal_credential().to_string(),
                drive: drive_for_link,
                cancel: cancel.clone(),
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
    pub async fn serve_until_signalled(self) -> GotchaResult<()> {
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
            // exactly as long as this server is serving.
            let _lock = _lock;
            // With the peer address, which `auth::guard` needs to accept the
            // cloud agent's bridge only from loopback.
            let served = gotcha::axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
                .with_graceful_shutdown(async move { cancel.cancelled().await })
                .await;
            // A deletion of the other devices' copies still running (the
            // aggregator turned off just before) gets a moment to finish.
            crate::aggregator::finish_deletions(std::time::Duration::from_secs(5)).await;
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

/// This server's instance directory, or a refusal naming the bad name.
fn instance_dirs(paths: &Paths, name: &str) -> GotchaResult<InstanceDirs> {
    InstanceDirs::new(&paths.data_dir(), name).map_err(GotchaError::message)
}

fn join(result: Result<std::io::Result<()>, tokio::task::JoinError>) -> GotchaResult<()> {
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(GotchaError::Io(error)),
        Err(error) => Err(GotchaError::message(error)),
    }
}
