//! Driving sessions: start one, send to one, stop one, rename, archive, and the
//! composer preferences that decide what a spawn is asked for.
//!
//! The routes are ports of the session-driving half of the former TypeScript
//! server's `app.ts`; the stores they sit on are ports of its `titles.ts`,
//! `archive.ts` and `composer-prefs.ts`.
//!
//! [`Drive`] is the one field this subsystem adds to `AppState`. It owns the
//! account store, the setup-token captures, the injection registry, the quota
//! cache and the composer preferences, so the merge surface in `api/mod.rs`
//! stays a single line.

pub mod archive;
pub mod composer_prefs;
pub(crate) mod idempotent;
pub mod routes;
pub mod titles;

use std::sync::Arc;

use crate::accounts::setup_token::Captures;
use crate::accounts::store::AccountsStore;
use crate::auth::Auth;
use crate::inject::InjectionRegistry;
use crate::quota::QuotaCache;
use crate::sessions::Paths;

pub use routes::{describe, routes};

/// Everything needed to drive the local CLIs, as one piece of application state.
pub struct Drive {
    /// Where everything is discovered and written.
    pub paths: Arc<Paths>,
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
    /// This server's own instance directory.
    ///
    /// Held so the send path can reach [`crate::notify::inject`], which puts the
    /// `rh-notify` program and this instance's port and tokens into a spawn.
    /// Cloned rather than shared behind an `Arc` because it is three paths and a
    /// name, and cloning it once per server costs less than the indirection.
    pub instance: crate::instance::InstanceDirs,
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
    pub fn new(paths: Arc<Paths>, instance: &crate::instance::InstanceDirs, auth: Arc<Auth>) -> Arc<Self> {
        let accounts = Arc::new(AccountsStore::new(paths.clone()));
        let quota = Arc::new(QuotaCache::new(paths.clone()));
        Arc::new(Self {
            auth,
            wizards: Captures::new(),
            injections: InjectionRegistry::new(paths.clone(), instance, accounts.clone(), quota.clone()),
            accounts,
            quota,
            models: Arc::new(crate::models::ClaudeModelCache::new(paths.clone())),
            composer: composer_prefs::ComposerPrefsStore::new(paths.clone()),
            titles: titles::TitlesStore::new(paths.clone()),
            instance: instance.clone(),
            paths,
        })
    }
}
