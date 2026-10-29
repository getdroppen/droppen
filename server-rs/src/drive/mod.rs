//! Driving sessions: start one, send to one, stop one, rename, archive, and the
//! composer preferences that decide what a spawn is asked for.
//!
//! The routes are ports of the session-driving half of the former TypeScript
//! server's `app.ts`; the composer preferences are a port of its
//! `composer-prefs.ts`. The title and archive stores the routes write live in
//! [`crate::sessions`], which reads them to build the session list.
//!
//! [`Drive`] is the one field this subsystem adds to `AppState`. It owns the
//! account store, the setup-token captures, the injection registry, the quota
//! cache and the composer preferences, so the merge surface in `api/mod.rs`
//! stays a single line.

mod attachments;
pub mod composer_prefs;
pub(crate) mod idempotent;
pub mod routes;

use std::sync::Arc;

use crate::accounts::login::Logins;
use crate::accounts::quota::QuotaCache;
use crate::accounts::setup_token::Captures;
use crate::accounts::store::AccountsStore;
use crate::auth::Auth;
use crate::inject::{InjectionRegistry, ProviderRegistry, SpawnDecoration};
use crate::sessions::Source;
use droppen_base::process_env::Env;
use droppen_instance::instance::InstanceDirs;
use futures_util::future::BoxFuture;
use droppen_instance::paths::Paths;
use crate::sessions::{titles, TranscriptSources};

pub use routes::{describe, DriveState};

/// Everything needed to drive the local CLIs, as one piece of application state.
pub struct Drive {
    /// Where everything is discovered and written.
    pub paths: Arc<Paths>,
    /// Each CLI's transcripts, which the session routes, the feeds and the
    /// cloud streams list and read sessions through.
    pub transcripts: TranscriptSources,
    /// The stored logins, for both CLIs.
    ///
    /// Behind an `Arc` because the injection registry holds the same store: a
    /// spawn's captured output is redacted against the credentials this server
    /// injected, and the only thing that knows what those are is this.
    pub accounts: Arc<AccountsStore>,
    /// Every `claude setup-token` run this server has started.
    pub wizards: Arc<Captures>,
    /// Every run remote-hand started.
    pub injections: Arc<InjectionRegistry>,
    /// Every CLI this server drives: what the routes, the feeds and the
    /// schedules ask about a session's runs, whichever CLI it is.
    pub providers: Arc<ProviderRegistry>,
    /// Claude quota readings per login — off the sessions' own
    /// `rate_limit_event`s first, the `claude -p /usage` fallback second.
    ///
    /// Behind an `Arc` because the injection registry holds the same one: its
    /// drivers are where the session readings come from.
    pub quota: Arc<QuotaCache>,
    /// The Claude model list, asked of the CLI and cached for the process.
    pub models: Arc<crate::models::ClaudeModelCache>,
    /// Composer model/effort/permission choices, shared across devices.
    pub composer: composer_prefs::ComposerPrefsStore,
    /// User-set session titles.
    pub titles: titles::TitlesStore,
    /// Local accounts, live logins, tickets and failed-login counters.
    ///
    /// Here rather than as a field of its own on
    /// [`crate::api::AppState`] so the merge surface stays one line, and because
    /// the `/api/auth/*` routes reach it exactly the way every other subsystem
    /// reaches its own state. [`crate::app::RemoteHandApp::prepare`] takes a
    /// second handle to it for the middleware, which runs outside the router and
    /// so cannot go through `State<AppState>`.
    pub auth: Arc<Auth>,
}

impl Drive {
    /// Build the subsystem for one server instance.
    ///
    /// `instance` reaches this far in for one reason: the injection registry
    /// records its in-flight runs in the instance directory, so a restart can
    /// find them again. `auth` is built by the caller rather than here, because
    /// the middleware needs the same handle and is mounted outside the router.
    /// `providers` is the entry's `crate::app::register_providers`, which
    /// names every CLI; `logins` likewise, each a CLI's own code, which only
    /// the entry names.
    pub fn new(
        paths: Arc<Paths>,
        instance: &droppen_instance::instance::InstanceDirs,
        auth: Arc<Auth>,
        transcripts: TranscriptSources,
        providers: fn(&mut ProviderRegistry, &Arc<InjectionRegistry>),
        logins: Logins,
    ) -> Arc<Self> {
        let accounts = Arc::new(AccountsStore::new(paths.clone(), logins));
        let quota = Arc::new(QuotaCache::new(paths.clone()));
        let decoration = NotifyDecoration::new(paths.clone(), instance.clone());
        let injections = InjectionRegistry::new(paths.clone(), instance, accounts.clone(), quota.clone(), decoration);
        Arc::new(Self {
            auth,
            wizards: Captures::new(),
            providers: ProviderRegistry::new(injections.clone(), providers),
            injections,
            accounts,
            quota,
            models: Arc::new(crate::models::ClaudeModelCache::new(paths.clone())),
            composer: composer_prefs::ComposerPrefsStore::new(paths.clone()),
            titles: titles::TitlesStore::new(paths.clone()),
            paths,
            transcripts,
        })
    }
}

/// The notify channel, as a claude spawn's environment gains it: the
/// `droppen-notify` program on its `PATH`, this server's address and a token
/// for the session, when the channel is on (see [`crate::notify::inject`]).
pub(crate) struct NotifyDecoration {
    /// Where the channel's settings and secret are stored.
    paths: Arc<Paths>,
    /// This server's own instance directory: its port and the program.
    instance: InstanceDirs,
}

impl NotifyDecoration {
    /// The channel of the server at `paths` and `instance`.
    pub(crate) fn new(paths: Arc<Paths>, instance: InstanceDirs) -> Arc<Self> {
        Arc::new(Self { paths, instance })
    }
}

impl SpawnDecoration for NotifyDecoration {
    fn decorate<'a>(&'a self, source: Source, id: &'a str, env: &'a mut Env) -> BoxFuture<'a, ()> {
        Box::pin(crate::notify::inject::decorate(&self.paths, &self.instance, source, id, env))
    }
}
