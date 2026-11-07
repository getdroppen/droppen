//! What each CLI that runs under stored accounts brings to them.
//!
//! The store keeps the credentials and the rotation chooses between them in the
//! same way for every CLI. How a credential reaches a child differs: claude
//! reads it out of its environment as it stands, while codex has to be handed a
//! model provider of the server's own on its command line, and each is proved
//! to work by a run of its own CLI. Each CLI says how through its [`Login`],
//! and the entry hands the store one per [`AccountProvider`] ([`Logins`]), so
//! the accounts name no CLI's own code.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use droppen_base::process_env::Env;
use futures_util::future::BoxFuture;
use serde_json::Value;

use super::local_login::LocalLoginState;
use super::quota::readings::SharedReading;
use super::quota::{CodexQuota, ProbeRoots, UsageProbe};
use super::store::{AccountError, AccountKind, AccountLaunch, AccountProvider};
use droppen_base::env_block::EnvVar;
use super::verify::Verdict;
use droppen_instance::paths::Paths;

/// One CLI's part of the accounts: how a credential starts it, and how one
/// is proved to work.
pub trait Login: Send + Sync {
    /// What one credential puts into a spawn of this CLI, without it having to
    /// be stored first.
    ///
    /// The same answer serves the spawn and the check before a credential is
    /// kept ([`super::verify`]): a check that ran with a different environment
    /// from the spawn would be a check of something else. An `Err` is a
    /// credential this CLI could never be started under, which the store
    /// refuses to keep.
    fn spawn_env(&self, kind: AccountKind, secret: &str, env: &[EnvVar]) -> Result<AccountLaunch, AccountError>;

    /// Spend one real request proving the credential `launch` was built from
    /// works: one run of this CLI with it, in a scratch home of its own, so the
    /// only login in reach is the one being checked.
    ///
    /// `secrets` are the credential's values: whatever the CLI says back is
    /// redacted of them before it reaches the verdict. Never fails: anything
    /// that is not a clean run or a clean refusal is [`Verdict::Unverifiable`].
    fn verify<'a>(&'a self, paths: &'a Paths, launch: &'a AccountLaunch, secrets: &'a [&'a str]) -> BoxFuture<'a, Verdict>;

    /// Whether the machine's own login of this CLI can start a session, from
    /// the evidence this server has of it.
    ///
    /// `refused_at` is when an active `auth` rest on that login was seen.
    /// `local_reading` is Claude Code's reading of its local login
    /// (`<data>/local-usage.json`), whichever CLI is asked: no other CLI keeps
    /// one, so every other login ignores it and weighs the refusal alone.
    fn local_state(&self, refused_at: Option<i64>, local_reading: &SharedReading) -> LocalLoginState;

    /// Run this CLI's usage probe against one of its logins: `config_dir` is an
    /// account's usage login, `None` the user's own. `None` back is a CLI with
    /// no such probe: only claude's logins keep readings for it to refresh.
    fn probe<'a>(&'a self, roots: &'a ProbeRoots, config_dir: Option<&'a Path>) -> Option<BoxFuture<'a, UsageProbe>>;

    /// The plan the user's own login of this CLI is on. `None` back is a CLI
    /// whose plan arrives with its quota reading instead.
    fn local_plan<'a>(&'a self, paths: &'a Paths) -> Option<BoxFuture<'a, LocalPlan>>;

    /// A login's plan as the quota panel and the first-run guide name it
    /// (`Max 20x`), from the subscription and the rate-limit tier this CLI
    /// reports for it. `None` back is a CLI that reports no such pair.
    fn plan_label(&self, subscription: Option<&str>, tier: Option<&str>) -> Option<String>;

    /// The quota of the user's own login of this CLI, read whole: its plan
    /// and windows, live or out of the CLI's own logs. `None` back is a CLI
    /// whose quota is read per login instead ([`Login::probe`]).
    fn local_quota<'a>(&'a self, roots: &'a ProbeRoots) -> Option<BoxFuture<'a, Option<CodexQuota>>>;

    /// Ask this CLI's app server one JSON-RPC question about its login:
    /// `program` is the binary, run in `cwd` with `env`, and the answer is the
    /// `result`, or `None` for any failure. `None` back is a CLI with no app
    /// server to ask.
    fn app_server_call<'a>(
        &'a self,
        program: &'a Path,
        cwd: &'a Path,
        env: &'a Env,
        method: &'a str,
        timeout: Duration,
    ) -> Option<BoxFuture<'a, Option<Value>>>;

    /// Whether this CLI's sessions move between accounts: off the account a
    /// limit ended a turn on, and off a configured default that is resting or
    /// about to run out ([`super::rotation`]). A CLI whose sessions stay on
    /// the account they started on answers `false`.
    fn rotates(&self) -> bool;

    /// The family `model` belongs to, as the limits scope rests to families
    /// ([`super::rotation::FAMILIES`]), or `None` for a model that names none:
    /// a family's rest stands in the way of that family's sessions only.
    fn model_family(&self, model: Option<&str>) -> Option<&'static str>;

    /// Whether each of this CLI's accounts keeps a usage reading of its own
    /// ([`super::quota::QuotaCache::cached`]), which rotation ranks the pool by.
    fn reads_per_login(&self) -> bool;
}

/// The plan the user's own login of a CLI is on ([`Login::local_plan`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalPlan {
    /// The plan as the quota panel names it, e.g. `Max 20x`.
    pub plan: Option<String>,
    /// The raw tier the plan was read from, e.g. `default_max_20x`.
    pub tier: Option<String>,
}

/// Every CLI's [`Login`], one per [`AccountProvider`].
///
/// A field per provider rather than a map: the providers are a closed set, so
/// every account's CLI has a login and finding it cannot fail. The fields are
/// named where the entry fills them in, so which login serves which CLI is
/// written down there rather than implied by an argument's position; the store
/// still refuses a launch that claims another CLI than its account's
/// ([`super::store::AccountsStore::account_env`]).
#[derive(Clone)]
pub struct Logins {
    /// Claude Code's login.
    pub claude: Arc<dyn Login>,
    /// Codex's login.
    pub codex: Arc<dyn Login>,
}

impl Logins {
    /// The login of the CLI `provider` names.
    pub fn of(&self, provider: AccountProvider) -> &Arc<dyn Login> {
        match provider {
            AccountProvider::Claude => &self.claude,
            AccountProvider::Codex => &self.codex,
        }
    }

    /// Logins for the tests that ask nothing of a CLI: each launches whatever
    /// it is handed, under its own CLI's name, checks, reads and names nothing,
    /// and its probe runs and finds no window. A test that asserts what a CLI's own
    /// login answers uses the real ones the entry builds instead.
    #[cfg(any(test, feature = "test-support"))]
    pub fn stand_ins() -> Self {
        Self {
            claude: Arc::new(StandIn(AccountProvider::Claude)),
            codex: Arc::new(StandIn(AccountProvider::Codex)),
        }
    }
}

/// A login that names its CLI and nothing else ([`Logins::stand_ins`]).
#[cfg(any(test, feature = "test-support"))]
struct StandIn(AccountProvider);

#[cfg(any(test, feature = "test-support"))]
impl Login for StandIn {
    fn spawn_env(&self, kind: AccountKind, secret: &str, env: &[EnvVar]) -> Result<AccountLaunch, AccountError> {
        Ok(AccountLaunch {
            provider: self.0,
            env: kind.vars(secret, env),
            codex_args: Vec::new(),
        })
    }

    fn verify<'a>(&'a self, _paths: &'a Paths, _launch: &'a AccountLaunch, _secrets: &'a [&'a str]) -> BoxFuture<'a, Verdict> {
        Box::pin(async { Verdict::Unverifiable("a stand-in checks nothing".to_string()) })
    }

    fn local_state(&self, _refused_at: Option<i64>, _local_reading: &SharedReading) -> LocalLoginState {
        LocalLoginState::Unknown
    }

    /// A probe that runs, so a test sees whether the cache let it: one that
    /// ran is recorded in the login's reading.
    fn probe<'a>(&'a self, _roots: &'a ProbeRoots, _config_dir: Option<&'a Path>) -> Option<BoxFuture<'a, UsageProbe>> {
        Some(Box::pin(async { UsageProbe::default() }))
    }

    fn local_plan<'a>(&'a self, _paths: &'a Paths) -> Option<BoxFuture<'a, LocalPlan>> {
        None
    }

    fn plan_label(&self, _subscription: Option<&str>, _tier: Option<&str>) -> Option<String> {
        None
    }

    fn local_quota<'a>(&'a self, _roots: &'a ProbeRoots) -> Option<BoxFuture<'a, Option<CodexQuota>>> {
        None
    }

    fn app_server_call<'a>(
        &'a self,
        _program: &'a Path,
        _cwd: &'a Path,
        _env: &'a Env,
        _method: &'a str,
        _timeout: Duration,
    ) -> Option<BoxFuture<'a, Option<Value>>> {
        None
    }

    fn rotates(&self) -> bool {
        false
    }

    fn model_family(&self, _model: Option<&str>) -> Option<&'static str> {
        None
    }

    fn reads_per_login(&self) -> bool {
        false
    }
}
