//! Restart-safe CLI drivers: Claude stream-json and Codex App Server.
//!
//! Both residents use the protocol-agnostic session shim. Legacy codex exec
//! runs are still adopted by PID until they finish, but no new exec is spawned.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;

use droppen_accounts::store::{AccountError, AccountLaunch, AccountProvider};
use droppen_instance::identity::ServerId;
use droppen_instance::instance::InstanceDirs;
use droppen_instance::paths::Paths;
use droppen_base::process_env::{redact_secrets, Env};
use droppen_base::procs;
use droppen_sessions::{Injection, InjectionRunState, SessionRef, Source};
use droppen_base::time::iso8601;
use gotcha::Schematic;

pub mod acp;
pub mod claude_code;
pub mod codex;
pub mod common;
pub mod drafts;
pub mod gate;
pub mod questions;
pub mod receipts;
mod registry;
pub mod runs;
pub mod shim;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
#[cfg(test)]
mod with_providers;

pub use common::{AnswerOutcome, Phase, QueueOutcome, Reply, ResidentState, StopTaskOutcome};
pub use gate::{SessionLease, SpawnGate, SpawnPermit};
pub use registry::{Capabilities, ContinueError, Provider, ProviderRegistry, Run, SendTo, StartAt};

/// How often an adopted run's pid is probed.
///
/// An adopted run has no `Child` handle to `wait()` on — the process that
/// forked it is gone, and the orphan was reparented to pid 1, which is what
/// reaps it. `kill(pid, 0)` turning `ESRCH` is therefore the only completion
/// signal available, and polling is the only way to read it. Two seconds is
/// well inside what a human reads as "the composer unblocked when it
/// finished", and costs one syscall per live run.
const ADOPTED_POLL: Duration = Duration::from_secs(2);
/// What an adopted run whose CLI was cut short is told.
const RESTART_NOTE: &str = "server restarted during this run";
/// Everything that can stop a run before it starts.
#[derive(Debug, Clone, thiserror::Error)]
pub enum InjectError {
    /// A plain refusal, reported to the client verbatim.
    #[error("{0}")]
    Message(String),
    /// Sessions in the user's own `~/.claude` keep using their own login, always.
    #[error("local_session_cannot_use_account")]
    LocalSessionAccount,
    /// A spawn was handed the other CLI's account.
    #[error("this account authenticates {} and cannot run a {} session", account.label(), wanted.label())]
    ProviderMismatch {
        /// The CLI the account belongs to.
        account: AccountProvider,
        /// The CLI being spawned.
        wanted: AccountProvider,
    },
    /// The account store refused.
    #[error("{0}")]
    Account(#[from] AccountError),
    /// A plain I/O failure on the way to a spawn.
    ///
    /// The TypeScript lets these escape as ordinary `Error`s, which
    /// `spawnErrorMessage` (the former TypeScript server's `app.ts`) replaces
    /// with the route's own wording rather than showing the user an `errno`.
    #[error("{0}")]
    Internal(String),
}

impl InjectError {
    /// What the client is told, masking anything the TypeScript would mask.
    pub fn client_message(&self, fallback: &str) -> String {
        match self {
            InjectError::Internal(_) | InjectError::Account(AccountError::Io(_)) => fallback.to_string(),
            other => other.to_string(),
        }
    }
}

impl InjectError {
    /// A failure to report as it is worded.
    pub fn message(text: impl Into<String>) -> Self {
        InjectError::Message(text.into())
    }
}

/// Which login a spawn should run under.
///
/// The three states are the wire's three states: the field absent, the field
/// `null`, and the field naming an account. They mean different things — `null`
/// is "the local login even though a default is configured", while absent leaves
/// the choice to the caller's fallback.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum AccountChoice {
    /// The `accountId` field was absent.
    #[default]
    Unset,
    /// `accountId: null` — the local login, explicitly.
    Local,
    /// `accountId: "<uuid>"`.
    Account(String),
    /// `accountId: "<uuid>"` with `accountFrom: "start"`: the account
    /// `GET /api/accounts/start` named for a pair no model-less start reaches
    /// (`placed: false`). The start rule's choice, not a person's — logged as
    /// that, and re-checked at the spawn ([`RunHost::rule_account`]).
    Rule(String),
}

/// Where a request's `accountId` came from, when it was not somebody's pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Schematic)]
#[serde(rename_all = "snake_case")]
pub enum AccountFrom {
    /// `GET /api/accounts/start` named it, with `placed: false`.
    Start,
}

impl AccountChoice {
    /// [`Self::from_request`], with where a named account came from: one the
    /// start rule named is the rule's choice ([`AccountChoice::Rule`]).
    pub fn from_start_request(value: Option<Option<String>>, from: Option<AccountFrom>) -> Self {
        match (Self::from_request(value), from) {
            (AccountChoice::Account(id), Some(AccountFrom::Start)) => AccountChoice::Rule(id),
            (choice, _) => choice,
        }
    }

    /// Build the choice from the double-`Option` a request body deserializes to.
    pub fn from_request(value: Option<Option<String>>) -> Self {
        match value {
            None => AccountChoice::Unset,
            Some(None) => AccountChoice::Local,
            Some(Some(id)) => AccountChoice::Account(id),
        }
    }

    /// The account named, if one was.
    pub fn named(&self) -> Option<&str> {
        match self {
            AccountChoice::Account(id) => Some(id),
            _ => None,
        }
    }
}

/// Everything the composer can ask a spawn to do differently.
#[derive(Debug, Clone, Default)]
pub struct SendOptions {
    /// `--model` / `-m`.
    pub model: Option<String>,
    /// `--effort` / `-c model_reasoning_effort=`.
    pub effort: Option<String>,
    /// `--permission-mode` / `--sandbox`.
    pub permission: Option<String>,
    /// Instant chat: replace the coding persona and disable all tools.
    pub chat: bool,
    /// Image paths from `/api/uploads`.
    pub images: Vec<String>,
    /// Non-image attachment paths from `/api/uploads`.
    pub files: Vec<String>,
    /// Which login to run under.
    pub account: AccountChoice,
    /// New claude sessions only: fall back to the configured default account.
    pub use_default_account: bool,
    /// The queue id the message is to travel under, when the route chose one.
    ///
    /// Set for a message the client named (`clientMessageId`): the route
    /// claims the name and picks the id *before* anything is enqueued, so the
    /// receipt can follow the message from the first instant a driver holds it.
    /// `None` mints one, as every send did before.
    pub message_id: Option<String>,
    /// New ACP sessions only: the id of the configured agent to start
    /// ([`crate::provider::acp::agents`]). A message to an existing session always goes to
    /// the agent that session runs on.
    pub acp_agent: Option<String>,
}

/// The suffix that tells a CLI where its attachments are.
///
/// It is also the contract the transcript parser reads back
/// (`droppen_sessions::util::parse_attachment_suffix`), so the wording is fixed.
pub fn attachment_suffix(paths: &[String]) -> String {
    let list = paths.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n");
    format!("\n\n[{} attached file(s) — view with the Read tool:]\n{list}", paths.len())
}

/// A prompt as claude reads it: the text, and where its attachments are.
///
/// Public for the one other place a user's words reach claude: an answer to a
/// question it is blocked on, which travels in the permission response rather
/// than as a message, and has to point at an attachment just the same.
pub fn claude_prompt(text: &str, images: &[String], files: &[String]) -> String {
    let attach: Vec<String> = images.iter().chain(files.iter()).cloned().collect();
    if attach.is_empty() {
        text.to_string()
    } else {
        format!("{text}{}", attachment_suffix(&attach))
    }
}

/// Refuse an account that belongs to the other CLI.
pub fn require_provider(launch: &AccountLaunch, wanted: AccountProvider) -> Result<(), InjectError> {
    if launch.provider == wanted {
        return Ok(());
    }
    Err(InjectError::ProviderMismatch {
        account: launch.provider,
        wanted,
    })
}

/// What a claude spawn's environment gains just before the CLI starts, beyond
/// its root and its account: the channel to its user, when there is one.
///
/// A port, because what goes in is the channel's business and the spawn only
/// knows when. Infallible: a session that cannot notify is still a session,
/// and one refused for want of a channel would be worse.
pub trait SpawnDecoration: Send + Sync {
    /// Add to `env` what session `id` of `source` starts with.
    fn decorate<'a>(&'a self, source: Source, id: &'a str, env: &'a mut Env) -> BoxFuture<'a, ()>;
}

// ------------------------------------------------------------------ operations

/// What a send did with the message.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sent {
    /// The message went in behind a turn that is still running, and will be
    /// delivered when that turn ends.
    ///
    /// Only a resident can say this: a message to a busy session used to be a
    /// refusal, because a second `--print` child against one transcript is two
    /// agents writing over each other. One process taking messages in order has
    /// no such problem, so the composer no longer has to be blocked by a turn.
    pub queued: bool,
}

impl RunHost {
    /// An account the start rule named for a pair no model-less start reaches
    /// (`accountFrom: "start"`), checked again now: still the login a start naming
    /// no model gets ([`droppen_accounts::rotation::StartInputs::still_names`]) —
    /// then it, logged as the rule's choice — or, if the rule has moved on since
    /// the page asked, wherever the rule places this start's model now.
    pub async fn rule_account(&self, provider: AccountProvider, named: &str, model: Option<&str>) -> Option<String> {
        let inputs = droppen_accounts::rotation::StartInputs::gather(&self.accounts, &self.quota, provider).await;
        if inputs.still_names(named) {
            tracing::info!(
                provider = provider.label(),
                to = %named,
                model = model.unwrap_or("-"),
                reason = "no login's own models land on that login: the start rule named it",
                "starting the new session on the account the start rule named"
            );
            return Some(named.to_string());
        }
        let fresh = inputs.place(model).account_id.0;
        tracing::info!(
            provider = provider.label(),
            named = %named,
            to = fresh.as_deref().unwrap_or("local login"),
            model = model.unwrap_or("-"),
            reason = "the start rule no longer names that account",
            "placing the new session where the start rule puts it now"
        );
        fresh
    }

    /// Where a start that names no account runs ([`droppen_accounts::rotation::start_for`]),
    /// and a line in the log whenever that is not simply the default or the local
    /// login: which login the start was moved off, which it went to, and why.
    pub async fn start_account(&self, provider: AccountProvider, model: Option<&str>) -> Option<String> {
        use droppen_accounts::rotation::StartReason;
        let start = droppen_accounts::rotation::start_for(&self.accounts, &self.quota, provider, model).await;
        let to = start.account_id.0;
        let provider_name = provider.label();
        let default = match start.reason {
            StartReason::Rotated => self.accounts.default_account_id(provider).await.unwrap_or_default(),
            _ => String::new(),
        };
        match start.reason {
            StartReason::Rotated => tracing::info!(
                provider = provider_name,
                from = %default,
                to = to.as_deref().unwrap_or("-"),
                model = model.unwrap_or("-"),
                reason = "the default account is resting or about to run out",
                "starting the new session on another account in the pool"
            ),
            StartReason::Fallback => tracing::info!(
                provider = provider_name,
                from = "local login",
                to = to.as_deref().unwrap_or("-"),
                model = model.unwrap_or("-"),
                reason = "no default account, and the machine's own login is signed out",
                "starting the new session on an account in the pool instead of the local login"
            ),
            StartReason::LocalSignedOut => tracing::info!(
                provider = provider_name,
                model = model.unwrap_or("-"),
                reason = "no default account, the machine's own login is signed out, and no account in the pool is free",
                "starting the new session on the signed-out local login; its first turn will say how to sign in"
            ),
            StartReason::Default | StartReason::Local | StartReason::LocalChosen => {}
        }
        to
    }

    /// A CLI's own words, fit to be stored and shown.
    ///
    /// Both layers, in the order that matters: the exact stored credentials
    /// first, because those are known rather than guessed and two of the three
    /// kinds look like nothing a pattern could find; then the `sk-…` pattern,
    /// which catches a credential this server never stored — the user's own
    /// login out of `~/.claude`, a key somebody pasted into a prompt.
    pub fn redact(&self, text: &str) -> String {
        redact_secrets(&self.accounts.redact_known_secrets(text))
    }

    /// Write a limit a driver saw into this instance's limit log.
    ///
    /// Phase 1 of account rotation, and only that: whatever the driver then
    /// decides (phase 2), nothing here retries, switches or holds. The words
    /// go through both redaction layers here, before anything is stored, and
    /// the log line says what kind of limit it was and until when, never what
    /// the CLI said.
    ///
    /// Synchronous like every state-database write in this server, and only
    /// ever reached on a path where a CLI has already failed.
    pub fn record_limit(&self, mut observation: droppen_accounts::rest::Observation) {
        use droppen_accounts::rest;
        observation.raw = self.redact(&observation.raw);
        observation.reason = if observation.reason.is_empty() {
            droppen_accounts::limits::reason_line(&observation.raw, observation.verdict.class)
        } else {
            self.redact(&observation.reason)
        };
        match self.accounts.limit_log().record(&observation, rest::now_ms()) {
            Ok(recorded) => tracing::info!(
                account = %observation.account_id,
                session = observation.session_id.as_deref().unwrap_or("-"),
                source = observation.source,
                class = observation.verdict.class.as_str(),
                scope = %observation.verdict.scope,
                until = recorded.until_ms.map(rest::iso_ms).as_deref().unwrap_or("-"),
                rested = recorded.rested,
                duplicate = recorded.duplicate,
                "recorded an account limit"
            ),
            Err(error) => tracing::warn!(
                account = %observation.account_id,
                source = observation.source,
                %error,
                "cannot record an account limit"
            ),
        }
    }

    /// A turn on this login went through, so its credential works: end a
    /// refused-credential (`auth`) rest it carries.
    ///
    /// Such a rest has no end of its own. The claude local login heals by
    /// reading (a reading newer than the refusal outweighs it,
    /// [`droppen_accounts::local_login`]); codex has no reading of its login,
    /// and a stored account's rest only ended with a new credential. Without
    /// this, one passing refusal would mark a login signed out for good — and a
    /// signed-out local login hands every start that names no account to a
    /// stand-in ([`droppen_accounts::rotation::start_choice`]).
    pub fn credential_accepted(&self, account_id: &str, session_id: &str) {
        use droppen_sessions::account::LimitClass;
        let log = self.accounts.limit_log();
        if log
            .active_observed_ms(account_id, LimitClass::Auth, droppen_accounts::rest::now_ms())
            .is_none()
        {
            return;
        }
        match log.clear_class(account_id, LimitClass::Auth) {
            Ok(_) => tracing::info!(
                account = %account_id,
                session = %session_id,
                "a turn went through on a login resting on a refused credential; the rest is over"
            ),
            Err(error) => tracing::warn!(account = %account_id, %error, "cannot end a refused-credential rest"),
        }
    }
}

// -------------------------------------------------------------------- registry

/// One run's mutable state.
pub struct RunState {
    /// Running, or how it ended.
    pub state: InjectionRunState,
    /// Why it failed, when it did.
    pub error: Option<String>,
    /// Whether it ended because it was stopped.
    pub interrupted: bool,
    started_at: String,
    /// When it ended, once it has.
    pub finished_at: Option<String>,
    /// `None` once the child has exited.
    pid: Option<u32>,
    /// What [`procs::start_token`] said about [`RunState::pid`] when the run was
    /// last known to be that process, so a signal can prove it still is.
    ///
    /// `None` on a run this server spawned, and that is not a gap: its CLI is
    /// still an unreaped child of this process, and the kernel cannot hand a pid
    /// out again before its parent has reaped it. Only an inherited pid — a
    /// number the previous server wrote in a file — needs the proof.
    pid_token: Option<String>,
    last_progress: Instant,
    idle_killed: bool,
    /// The session id the watchdog checks the transcript mtime of; a codex run
    /// re-points it once the real thread id is known.
    current_id: String,
    /// Identifies this run in `runs.json`, so completion removes its own entry.
    run_id: String,
    /// This run's cross-instance claim on the session, held for exactly as long
    /// as the CLI is running. `None` once it has finished, and on a run that
    /// never launched.
    lock: Option<gate::SessionLease>,
}

impl RunState {
    fn to_injection(&self) -> Injection {
        Injection {
            state: self.state,
            error: droppen_http::openapi_ext::Nullable(self.error.clone()),
            interrupted: self.interrupted,
            started_at: self.started_at.clone(),
            finished_at: droppen_http::openapi_ext::Nullable(self.finished_at.clone()),
        }
    }
}

/// A live or finished run.
pub struct RunEntry {
    /// Its state, which its driver or watcher writes.
    pub state: Mutex<RunState>,
    /// Flipped once the child has exited and the final state has been written.
    ///
    /// A `watch` rather than a `Notify`: `notify_waiters` stores no permit, so a
    /// waiter that arrives after the run finished — which is exactly what the
    /// codex handshake does when stdout closes first — would wait forever.
    pub finished: tokio::sync::watch::Sender<bool>,
}

impl RunEntry {
    /// A `Running` turn of session `id` in a resident process: one that
    /// outlives the turn, so the run has no process and no journal entry of
    /// its own.
    pub fn resident_turn(id: &str) -> Arc<Self> {
        let (finished, _) = tokio::sync::watch::channel(false);
        Arc::new(RunEntry {
            state: Mutex::new(RunState {
                state: InjectionRunState::Running,
                error: None,
                interrupted: false,
                started_at: iso8601(std::time::SystemTime::now()),
                finished_at: None,
                // The pid is the resident's, and the resident is not this turn's to
                // end: Stop goes through the driver's ladder, which is the reason
                // nothing here is allowed to signal it directly.
                pid: None,
                pid_token: None,
                last_progress: Instant::now(),
                idle_killed: false,
                current_id: id.to_string(),
                // The journal entry belongs to the *process*, not to the turn, so a
                // turn ending must never retire it.
                run_id: String::new(),
                lock: None,
            }),
            finished,
        })
    }

    /// This run's state, in the shape the API speaks.
    pub fn injection(&self) -> Injection {
        self.state.lock().expect("run state poisoned").to_injection()
    }
}

/// Every run remote-hand started, keyed [`SessionRef::scoped_key`].
///
/// The TypeScript keeps two module-level maps. Here they are one registry that
/// lives in the application state, so two servers in one process do not kill
/// each other's children.
///
/// What every provider's runs share: the runs map and its journal, the
/// receipts, questions and drafts, whom a change is told to, the accounts a
/// start is placed on, and the watchdog's timings. Each provider's own runs
/// reach it through themselves ([`codex::app_server::CodexRuns`],
/// [`claude_code::resident::ClaudeCodeRuns`], [`acp::AcpRuns`]).
pub struct RunHost {
    /// The stored logins, for one job only: taking the credentials this server
    /// injected back out of whatever the CLI printed. See
    /// [`RunHost::redact`].
    pub accounts: Arc<droppen_accounts::store::AccountsStore>,
    /// The quota readings, for one job only: every `rate_limit_event` a
    /// resident's claude writes is a reading of the login it runs on, and the
    /// driver files it here. The same handle [`crate::drive::Drive::quota`]
    /// serves the panel from, so a reading is on the panel the moment it lands.
    pub quota: Arc<droppen_accounts::quota::QuotaCache>,
    /// This server's name, so the keys below can say whose they are.
    ///
    /// Only ever this process' own: everything keyed by it is in memory and
    /// dies with the registry. See [`RunHost::key_of`].
    server: ServerId,
    runs: Mutex<HashMap<String, Arc<RunEntry>>>,
    /// Where a resident's shim leaves its socket, stderr tail and tombstone.
    pub shim_dir: PathBuf,
    /// How long a codex run may make no progress before it is ended.
    idle_timeout: Duration,
    /// How long each rung of a resident's interrupt ladder waits for the turn to
    /// end before the next signal.
    ///
    /// The ladder starts polite — a `control_request` that ends the turn and
    /// leaves the process alive — and escalates only because a wedged CLI cannot
    /// answer it. Five seconds is well past what a healthy claude takes to write
    /// the turn's `result`, and short enough that a Stop button still feels like
    /// one.
    pub interrupt_step: Duration,
    /// How often the watchdog checks.
    pub watchdog_interval: Duration,
    /// Where transcripts are discovered, for the idle watchdog.
    pub paths: Arc<Paths>,
    /// The record of in-flight runs, in this server's own instance directory.
    pub journal: runs::RunsFile,
    /// The session feed and the list feed, told when a driver changes something
    /// no file shows. Set once, at startup; empty in tests that build no app.
    listeners: std::sync::OnceLock<Listeners>,
    /// Where the drivers send a turn's end, a question and a rotation for the
    /// phone. Set once, at startup; empty in tests that build no app.
    pub alerts: std::sync::OnceLock<Arc<dyn Alerts>>,
    /// The idempotency claims and receipts of messages a client named, in
    /// this instance's database. The drivers report into it by queue id; see
    /// [`receipts`].
    receipts: Arc<receipts::Receipts>,
    /// The questions claude stopped to ask, in this instance's database. The
    /// drivers open and close them; the answer routes compare-and-set them.
    /// See [`questions`].
    questions: Arc<questions::Questions>,
    /// What each resident claude is thinking right now, as a running count;
    /// written by the drivers, read by the session feeds. See [`drafts`].
    pub drafts: drafts::Drafts,
    /// What the spawn gate and the session's claim were at each traced point
    /// of a spawn; see [`testing::Step`].
    #[cfg(any(test, feature = "test-support"))]
    steps: Mutex<Vec<testing::Step>>,
    /// The spawn gate of the provider registry over this host, while that
    /// registry lasts: one registry per host, as two would each run a part
    /// of one source's sessions ([`Self::hold_gate`]). The steps are read off
    /// it.
    gate: Mutex<Weak<gate::SpawnGate>>,
}

/// The host whose runs every provider shares ([`RunHost`]), what a provider
/// needs to make its own runs on it, and where the runs the previous server
/// left behind are adopted ([`Self::adopt_all`]), each by the provider of its
/// CLI.
///
/// Made by [`InjectionRegistry::new`] alone (or, for a test, by
/// `testing::injection_registry_on`), never as a struct literal: its fields
/// are read everywhere, but no other crate builds one.
#[non_exhaustive]
pub struct InjectionRegistry {
    /// What its runs share with every provider's.
    pub host: Arc<RunHost>,
    /// This server's own instance directory, where a provider keeps what is
    /// its runs' alone (Claude Code's flight recordings).
    pub instance: InstanceDirs,
    /// What a claude spawn's environment gains before it starts: on the send
    /// path, and for the process an automatic rotation moves a session to,
    /// which a driver builds by itself.
    pub decoration: Arc<dyn SpawnDecoration>,
}

/// The per-session feeds as the drivers see them: the open views of a session
/// are told to look again. The feeds sit above the drivers, so they implement
/// this and the entry hands it in ([`RunHost::attach_listeners`]).
pub trait SessionFeeds: Send + Sync {
    /// Wake the open views of the session `key` names, if it has any.
    fn poke(&self, key: &str);
}

/// The alerts a run raises for the user's phone. The alert channels sit above
/// the drivers, so the entry implements this over them and hands it in
/// ([`RunHost::attach_alerts`]).
///
/// Every method returns at once and sends in the background: a driver calls
/// them while it holds a run's lock or is about to respawn a session, and a
/// slow push service must not hold either up.
pub trait Alerts: Send + Sync {
    /// A run reached its final state. `driver` is the caller's snapshot of the
    /// drivers, taken synchronously after the finished turn was written back
    /// as idle, so the alert cannot describe the next queued turn. It answers
    /// for session `id` of `source` only: its own provider's processes and the
    /// runs map ([`RunHost::driver_states_of`]).
    fn turn(&self, paths: Arc<Paths>, source: Source, id: String, outcome: InjectionRunState, error: Option<String>, driver: droppen_sessions::DriverStates);
    /// The agent stopped to ask `question`. Called once per question, the
    /// first time it is put on record.
    fn question(&self, paths: Arc<Paths>, source: Source, id: String, question: String);
    /// The session was moved to another account, or could not be; `text` is
    /// the notice the session shows.
    fn rotation(
        &self,
        paths: Arc<Paths>,
        source: Source,
        id: String,
        outcome: droppen_sessions::account::RotationOutcome,
        trigger: droppen_sessions::account::RotationTrigger,
        text: String,
    );
}

/// How a host's watchdog and interrupt ladder are timed.
struct Timings {
    /// [`RunHost::idle_timeout`].
    idle_timeout: Duration,
    /// [`RunHost::interrupt_step`].
    interrupt_step: Duration,
    /// [`RunHost::watchdog_interval`].
    watchdog_interval: Duration,
}

/// Whoever follows a session from outside the driver: the per-session feed and
/// the session list.
struct Listeners {
    feeds: Arc<dyn SessionFeeds>,
    refresh: droppen_http::refresh::Refresh,
}

/// A duration from the environment, in milliseconds, or the fallback
/// ([`droppen_base::env::var`]).
pub fn env_ms(name: &str, fallback: Duration) -> Duration {
    droppen_base::env::var(name)
        .and_then(|raw| raw.parse::<f64>().ok())
        .filter(|ms| ms.is_finite() && *ms > 0.0)
        .map(Duration::from_secs_f64_millis)
        .unwrap_or(fallback)
}

trait DurationFromMillis {
    fn from_secs_f64_millis(ms: f64) -> Duration;
}
impl DurationFromMillis for Duration {
    fn from_secs_f64_millis(ms: f64) -> Duration {
        Duration::from_secs_f64(ms / 1000.0)
    }
}

impl InjectionRegistry {
    /// A registry with the timeouts the environment asks for.
    ///
    /// Runs are killed for being *stuck*, never for taking long: a healthy
    /// agentic run can legitimately work for hours, and a wall-clock cap would
    /// execute it mid-task.
    ///
    /// `instance` says where `runs.json` goes, and it is the only per-instance
    /// thing here: the session claims deliberately live one level up, in the
    /// data directory every instance shares. `decoration` is what a claude
    /// spawn's environment gains before it starts.
    pub fn new(
        paths: Arc<Paths>,
        instance: &InstanceDirs,
        accounts: Arc<droppen_accounts::store::AccountsStore>,
        quota: Arc<droppen_accounts::quota::QuotaCache>,
        decoration: Arc<dyn SpawnDecoration>,
    ) -> Arc<Self> {
        Arc::new(Self {
            host: RunHost::new(paths, instance, accounts, quota),
            instance: instance.clone(),
            decoration,
        })
    }
}

impl RunHost {
    /// The host of a registry's runs, the watchdog timed as the environment
    /// asks ([`InjectionRegistry::new`]).
    fn new(
        paths: Arc<Paths>,
        instance: &InstanceDirs,
        accounts: Arc<droppen_accounts::store::AccountsStore>,
        quota: Arc<droppen_accounts::quota::QuotaCache>,
    ) -> Arc<Self> {
        let timings = Timings {
            idle_timeout: env_ms("DROPPEN_IDLE_TIMEOUT_MS", Duration::from_secs(20 * 60)),
            interrupt_step: env_ms("DROPPEN_INTERRUPT_STEP_MS", Duration::from_secs(5)),
            watchdog_interval: env_ms("DROPPEN_WATCHDOG_INTERVAL_MS", Duration::from_secs(60)),
        };
        Self::timed(paths, instance, accounts, quota, timings)
    }

    /// The host of a registry's runs, the watchdog timed by `timings`.
    fn timed(
        paths: Arc<Paths>,
        instance: &InstanceDirs,
        accounts: Arc<droppen_accounts::store::AccountsStore>,
        quota: Arc<droppen_accounts::quota::QuotaCache>,
        timings: Timings,
    ) -> Arc<Self> {
        let receipts = Arc::new(receipts::Receipts::new(droppen_instance::db::at(paths.instance_db())));
        let questions = Arc::new(questions::Questions::new(droppen_instance::db::at(paths.instance_db())));
        Arc::new(Self {
            receipts,
            questions,
            accounts,
            quota,
            server: ServerId::local(&paths),
            runs: Mutex::new(HashMap::new()),
            shim_dir: instance.session_shims(),
            idle_timeout: timings.idle_timeout,
            interrupt_step: timings.interrupt_step,
            watchdog_interval: timings.watchdog_interval,
            journal: runs::RunsFile::new(instance.runs_file()),
            listeners: std::sync::OnceLock::new(),
            alerts: std::sync::OnceLock::new(),
            drafts: drafts::Drafts::default(),
            #[cfg(any(test, feature = "test-support"))]
            steps: Mutex::new(Vec::new()),
            gate: Mutex::new(Weak::new()),
            paths,
        })
    }

    /// Give the drivers someone to tell. Called once, when the application
    /// state is assembled; a second call is ignored.
    pub fn attach_listeners(&self, feeds: Arc<dyn SessionFeeds>, refresh: droppen_http::refresh::Refresh) {
        let _ = self.listeners.set(Listeners { feeds, refresh });
    }

    /// Give the drivers somewhere to send their alerts. Called once, when the
    /// application state is assembled and before any driver runs; a second
    /// call is ignored.
    pub fn attach_alerts(&self, alerts: Arc<dyn Alerts>) {
        let _ = self.alerts.set(alerts);
    }

    /// Tell the session's feed and the session list that it changed in a way
    /// no file shows: its account binding and its rotation record, today.
    pub fn announce(&self, source: Source, id: &str) {
        let Some(listeners) = self.listeners.get() else { return };
        listeners.feeds.poke(&self.key_of(source, id));
        listeners.refresh.request_refresh(Some((&source, id)));
    }

    /// Ask the session list, and nothing else, to look again.
    ///
    /// The other narrow half of [`RunHost::announce`], for a change
    /// the session's own view must hear about in a particular order: a resident
    /// that took a steered message off its queue on the transcript's word pokes
    /// the feed only once that feed has shown the transcript — see
    /// `claude_code::resident::Driver::on_absorb` — while the list, which shows no
    /// transcript, can be told straight away.
    pub fn refresh_list(&self) {
        let Some(listeners) = self.listeners.get() else { return };
        listeners.refresh.request_refresh(None);
    }

    /// Push one session's state to whoever is watching it, and nothing else.
    ///
    /// The narrow half of [`RunHost::announce`], for a change only
    /// the session's own view shows — a message's receipt — and that the list
    /// has no column for.
    pub fn poke_feed(&self, source: Source, id: &str) {
        let Some(listeners) = self.listeners.get() else { return };
        listeners.feeds.poke(&self.key_of(source, id));
    }

    /// The idempotency claims and receipts of messages a client named.
    pub fn receipts(&self) -> &Arc<receipts::Receipts> {
        &self.receipts
    }

    /// The questions claude stopped to ask, and what became of them.
    pub fn questions(&self) -> &Arc<questions::Questions> {
        &self.questions
    }

    /// A driver's word on what became of the message travelling as `qid`.
    ///
    /// Cheap for a message nobody named — the update finds no row — and a
    /// poke of the session's feed for one somebody did, so the receipt reaches
    /// the page without waiting for the next poll.
    pub fn note_receipt(&self, source: Source, session: &str, qid: &str, status: droppen_sessions::delivery::ReceiptStatus) {
        if self.receipts.mark(qid, status, session, receipts::now_ms()) {
            self.poke_feed(source, session);
        }
    }

    /// The key one session's run, resident or feed entry is filed under here.
    ///
    /// [`SessionRef::scoped_key`], so the server is part of it. Every map it
    /// keys is in this process' memory and dies with it — nothing durable and
    /// nothing on the wire is spelled this way, which is exactly what makes the
    /// prefix free.
    pub fn key_of(&self, source: Source, id: &str) -> String {
        SessionRef::new(self.server.clone(), source, id).scoped_key()
    }

    /// Put `run` in the runs map as session `id`'s of `source`, in place of
    /// any run there.
    pub fn publish_run(&self, source: Source, id: &str, run: Arc<RunEntry>) {
        self.runs.lock().expect("injections poisoned").insert(self.key_of(source, id), run);
    }

    /// Take session `id`'s run of `source` out of the runs map, if there is one.
    pub fn withdraw_run(&self, source: Source, id: &str) {
        self.runs.lock().expect("injections poisoned").remove(&self.key_of(source, id));
    }

    /// Session `id`'s run of `source`, if the runs map has one and it is
    /// still running: its state read under the map's lock.
    pub fn running_run(&self, source: Source, id: &str) -> Option<Arc<RunEntry>> {
        let runs = self.runs.lock().expect("injections poisoned");
        runs.get(&self.key_of(source, id))
            .filter(|run| run.state.lock().expect("run state poisoned").state == InjectionRunState::Running)
            .cloned()
    }

    /// Move session `from`'s run of `source` to session `to`, which it watches
    /// from now on, under one hold of the map's lock; nothing when `from` has
    /// no run.
    pub fn rename_run(&self, source: Source, from: &str, to: &str) {
        let mut runs = self.runs.lock().expect("injections poisoned");
        if let Some(run) = runs.remove(&self.key_of(source, from)) {
            run.state.lock().expect("run state poisoned").current_id = to.to_string();
            runs.insert(self.key_of(source, to), run);
        }
    }

    /// The state of this session's run in the runs map: a turn a resident
    /// published there, or a run adopted from the previous server.
    pub fn run_injection(&self, source: Source, id: &str) -> Option<Injection> {
        self.runs
            .lock()
            .expect("injections poisoned")
            .get(&self.key_of(source, id))
            .map(|run| run.injection())
    }

    /// What `parts`' processes and the runs map say of their sessions, in one
    /// pass: the session list's snapshot when `parts` are every provider's
    /// runs ([`ProviderRegistry::driver_states`]).
    ///
    /// Taken once per listing rather than asked per row: a list of two hundred
    /// sessions would otherwise take the residents lock and the runs lock four
    /// hundred times, against a registry every send and every driver contends
    /// on. Both registries are bounded by sessions this server process has
    /// driven, rather than by transcript or list rows, so the snapshot is cheap
    /// to make and cheap to carry.
    fn snapshot<'a>(&self, parts: impl IntoIterator<Item = &'a dyn ProviderRuns>) -> droppen_sessions::DriverStates {
        let mut states = DriverStatesBuilder::default();
        for runs in parts {
            runs.add_to(&mut states);
        }
        // Last, so that where a run here and a CLI's own handle share a key,
        // the run's finishing time is the one the list shows.
        for (key, run) in self.runs.lock().expect("injections poisoned").iter() {
            states.run(key.clone(), run.injection());
        }
        droppen_sessions::DriverStates::new(self.server.clone(), states.residents, states.running, states.finished)
    }

    /// The snapshot a driver of `runs` hands the alert for one of their
    /// sessions' turns ([`Alerts::turn`]): `runs`' own processes and the runs
    /// map, which is all the alert reads of that session. Taken synchronously
    /// by the driver, as the alert asks.
    pub fn driver_states_of(&self, runs: &dyn ProviderRuns) -> droppen_sessions::DriverStates {
        self.snapshot([runs])
    }

    /// Lend the spawn gate of the provider registry over this host: one at a
    /// time, as two registries would each run a part of one source's
    /// sessions. A second, while the first's gate lasts, is refused.
    ///
    /// The refusal follows the registry's lifetime, which is its gate's, not
    /// its runs': a registry dropped while a driver still holds its runs lets
    /// a new one in. Nothing in production drops one: the entry builds one and
    /// keeps it for the life of the server.
    fn hold_gate(&self, gate: &Arc<gate::SpawnGate>) {
        let mut held = self.gate.lock().expect("held gate");
        if held.strong_count() > 0 {
            drop(held);
            panic!("a provider registry is over this host already: one per host");
        }
        *held = Arc::downgrade(gate);
    }

    /// Stop a run in the runs map, which no resident drives: a one-shot run
    /// adopted from the previous server. `false` when nothing is running.
    ///
    /// For such a run there is no turn to end short of the process, and the
    /// old rule applies: a pid that no longer names the run it was recorded for
    /// is *not* signalled, and the run is closed out as finished instead — see
    /// [`vanished`] for why that is the only safe direction.
    pub fn interrupt_legacy(&self, runs: &dyn ProviderRuns, id: &str) -> bool {
        let source = runs.source();
        // The handle is cloned out so the map lock is released first: the
        // fail-closed path below rewrites `runs.json` and unlinks a claim, and
        // neither belongs under a lock every route contends on.
        let run = self.runs.lock().expect("injections poisoned").get(&self.key_of(source, id)).cloned();
        let Some(run) = run else { return false };
        let mut state = run.state.lock().expect("run state poisoned");
        if state.state != InjectionRunState::Running {
            return false;
        }
        let Some(pid) = state.pid else { return false };
        if !procs::is_same_process(pid, state.pid_token.as_deref()) {
            tracing::info!(pid, session = %id, source = %source, "the run's pid is no longer its CLI; ending it instead of signalling");
            vanished(&mut state, runs.cli_name(), self.idle_timeout);
            let claim = release(self, &mut state);
            drop(state);
            drop(claim);
            let _ = run.finished.send(true);
            return false;
        }
        state.interrupted = true;
        drop(state);
        signal(pid, libc::SIGTERM);
        true
    }
}

/// A provider's runs, kept by the provider rather than here: what the session
/// list shows of them ([`ProviderRegistry::driver_states`]), and what becomes
/// of a run of its CLI that the previous server left behind
/// ([`InjectionRegistry::adopt_all`]).
///
/// The record each method is handed is one of this provider's own CLI: adoption
/// hands each run to the provider of the source it names.
pub trait ProviderRuns: Send + Sync {
    /// The source whose sessions they run.
    fn source(&self) -> Source;

    /// The host they run on.
    fn host(&self) -> &Arc<RunHost>;

    /// Add its sessions' processes and the state of their runs to `states`.
    fn add_to(&self, states: &mut DriverStatesBuilder);

    /// Its CLI, as an ended run's error names it.
    fn cli_name(&self) -> &'static str;

    /// Whether the provider can read its part of `record`. A run it cannot is
    /// one this version cannot read, and is not adopted.
    fn readable(&self, record: &runs::RunRecord) -> bool;

    /// Whether `record` holds state of the provider's own that outlives its
    /// process: such a run is re-attached to through its shim while it lives,
    /// and handed back ([`ProviderRuns::dead`]) rather than buried once it has
    /// gone.
    fn holds_state(&self, _record: &runs::RunRecord) -> bool {
        false
    }

    /// Take back a run whose process is gone and that holds state of the
    /// provider's own ([`ProviderRuns::holds_state`]).
    fn dead(&self, _record: runs::RunRecord) {}

    /// Drive again the process `record` names, whose shim answered: `link` is
    /// its stdio, and `lease` its session's claim.
    fn attach(self: Arc<Self>, record: runs::RunRecord, sidecars: shim::Sidecars, link: shim::Link, lease: gate::SessionLease);

    /// Whether the session's transcript ends mid-turn, for a run whose process
    /// went with the last server.
    ///
    /// Each CLI already has a "is this session in the middle of something"
    /// reading, derived from the tail of its transcript, and it is exactly the
    /// question here: a turn that ended with the assistant's answer is a turn
    /// that completed, whereas one ending on the user's message or a tool call
    /// never got its reply. Reusing it keeps one definition of mid-turn in the
    /// codebase rather than two that can drift.
    fn was_cut_short<'a>(&'a self, record: &'a runs::RunRecord) -> BoxFuture<'a, bool>;

    /// When session `id`'s transcript was last written, on the clock an
    /// adopted run's watchdog counts progress by.
    fn last_write<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Option<Instant>>;

    /// The sessions whose questions can still be answered in the turn that
    /// asked them: those whose process came back with them.
    fn question_holders(&self) -> Vec<String> {
        Vec::new()
    }
}

/// When a file was last written, on the clock `Instant` keeps.
pub trait WrittenAt {
    /// When it was last written, or `None` when that cannot be read.
    fn written_at(&self) -> impl std::future::Future<Output = Option<Instant>> + Send;
}

impl WrittenAt for Path {
    fn written_at(&self) -> impl std::future::Future<Output = Option<Instant>> + Send {
        async move {
            let modified = tokio::fs::metadata(self).await.ok()?.modified().ok()?;
            let age = std::time::SystemTime::now().duration_since(modified).ok()?;
            Instant::now().checked_sub(age)
        }
    }
}

/// The session list's view of the runs, gathered a CLI at a time; see
/// [`ProviderRuns::add_to`].
#[derive(Default)]
pub struct DriverStatesBuilder {
    residents: HashMap<String, droppen_sessions::ResidentInfo>,
    running: std::collections::HashSet<String>,
    finished: HashMap<String, String>,
}

impl DriverStatesBuilder {
    /// A resident the list is to show, under its [`RunHost::key_of`]
    /// key: every CLI files its sessions under that one key, so
    /// [`droppen_sessions::DriverStates::resident`] finds any of them with it.
    pub fn resident(&mut self, key: String, info: droppen_sessions::ResidentInfo) {
        self.residents.insert(key, info);
    }

    /// A run's state under its key: running, or finished at a time. A later
    /// finish under the same key replaces an earlier one's time.
    pub fn run(&mut self, key: String, injection: Injection) {
        if injection.state == InjectionRunState::Running {
            self.running.insert(key);
        } else if let Some(at) = injection.finished_at.0 {
            self.finished.insert(key, at);
        }
    }
}

// ---------------------------------------------------------------- adoption

/// What one boot's adoption pass found.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Adopted {
    /// Runs whose CLI is still working, now tracked again.
    pub recovered: usize,
    /// Records whose CLI is gone, cleaned up.
    pub finished: usize,
    /// Records left for a later boot, because another instance holds the
    /// session right now. Pathological, and logged when non-zero.
    pub deferred: usize,
}

impl Adopted {
    /// Whether the pass found nothing at all, which is the ordinary case: a
    /// server that stopped with no run in flight leaves nothing to adopt.
    pub fn is_empty(&self) -> bool {
        self.recovered == 0 && self.finished == 0 && self.deferred == 0
    }
}

/// What became of one resident record on the way through adoption.
enum Reattached {
    /// Its stdio is back and a driver has it.
    Driving,
    /// The process is alive but unreachable; the claim comes back out to be
    /// held by a pid watcher instead.
    Watching(gate::SessionLease),
    /// There was nothing left to attach to.
    Buried,
}

impl InjectionRegistry {
    /// Re-attach to every run the previous server left behind.
    ///
    /// Must run **after** the instance flock and **before** the listener: a
    /// `GET /api/sessions/...` answered from a half-scanned `runs.json` would
    /// report a live run as absent and unblock the composer, which is the exact
    /// double-spawn this whole mechanism exists to prevent. It is gated on
    /// ownership for the same reason terminal adoption is — `export-openapi`
    /// runs a throwaway server in-process, and a throwaway server must not
    /// claim a real one's runs.
    ///
    /// `by_source` is each source's provider's runs, as the provider registry
    /// holds them ([`ProviderRegistry::adopt_all`]): each run goes to the
    /// provider of the source it names.
    pub(crate) async fn adopt_all(self: &Arc<Self>, gate: &gate::SpawnGate, by_source: &BTreeMap<Source, Arc<dyn ProviderRuns>>) -> Adopted {
        let mut report = Adopted::default();
        // Its claims need no gate because it runs once, before the server
        // listens: no send can race it.
        let Some(boot) = gate.boot() else {
            tracing::warn!("adoption runs once per server; not adopting again");
            return report;
        };
        for record in self.host.journal.read() {
            // Each run is its own CLI's provider's to take back. One whose part
            // that provider cannot read is one this version cannot read, and is
            // left where it is. One of a source no provider is registered for
            // has no one to take it back at all: this server was put together
            // without it.
            let Some(runs) = by_source.get(&record.source).cloned() else {
                tracing::error!(session = %record.session_id, source = %record.source, "no provider is registered for this run's source; that run will not be adopted");
                continue;
            };
            if !runs.readable(&record) {
                tracing::warn!(session = %record.session_id, source = %record.source, "a run record cannot be read; that run will not be adopted");
                continue;
            }
            if !procs::is_same_process(record.cli_pid, record.pid_token.as_deref()) {
                if runs.holds_state(&record) {
                    runs.dead(record);
                } else {
                    self.bury(&*runs, &record, &boot).await;
                }
                report.finished += 1;
                continue;
            }
            // The dead server's claim died with its descriptor, so this
            // normally succeeds. It failing means some *other* live server is
            // running against the same session — which can only happen if this
            // instance died and another took the session over while it was
            // down. Adopting on top of that would be a second tracker for one
            // session, so the record is left for a later boot instead: the pid
            // check above is what stops it lingering for ever.
            let claim = match boot.claim(record.source, &record.session_id) {
                Ok(claim) => claim,
                Err(gate::ClaimError::Busy { holder }) => {
                    tracing::warn!(
                        session = %record.session_id,
                        source = %record.source,
                        pid = record.cli_pid,
                        ?holder,
                        "another server is already running this session; not adopting"
                    );
                    report.deferred += 1;
                    continue;
                }
                Err(gate::ClaimError::Io(error)) => {
                    tracing::warn!(%error, session = %record.session_id, "cannot claim a session to adopt it");
                    report.deferred += 1;
                    continue;
                }
            };
            #[cfg(any(test, feature = "test-support"))]
            self.host.step("adoption claimed", record.source, Some(&record.session_id));
            // A resident is *re-attached to*, not watched: the shim held the
            // process' stdio open across the restart, so this server gets the
            // conversation back rather than just the knowledge that a pid
            // exists. A `codex exec` child has no such thing and is watched.
            if record.resident || runs.holds_state(&record) {
                match self.clone().reattach(&runs, &record, claim, &boot).await {
                    Reattached::Driving => report.recovered += 1,
                    Reattached::Watching(claim) => {
                        self.clone().resume(runs, record, claim);
                        report.recovered += 1;
                    }
                    Reattached::Buried => report.finished += 1,
                }
                continue;
            }
            self.clone().resume(runs, record, claim);
            report.recovered += 1;
        }
        // A question can only be answered into the turn that asked it, and that
        // turn is a process: one that did not come back with its session can
        // never take the answer. Those are expired here, once every survivor is
        // in the map, and stay answerable as an ordinary message. A survivor's
        // own questions are its driver's to settle when its replay ends.
        let held: std::collections::HashSet<String> = by_source.values().flat_map(|runs| runs.question_holders()).collect();
        let expired = self.host.questions.expire_unheld(&held, questions::now_ms());
        if expired > 0 {
            tracing::info!(
                expired,
                "questions whose process did not survive the restart can no longer be answered in their turn"
            );
        }
        report
    }

    /// Get a resident's stdio back, or work out why that is not possible.
    ///
    /// The three answers are the three states a shim can be in when its server
    /// comes back, and each is a different amount of bad news:
    ///
    /// * the socket answers and claude is alive → the session is *driveable*
    ///   again, exactly as if this server had started it;
    /// * the socket is gone or will not talk, but the pid is still there → the
    ///   process is running and unreachable. The claim is kept and the pid is
    ///   watched, because the one thing that must not happen is a second claude
    ///   against a transcript the first is still writing;
    /// * the shim is minding a corpse, or the pid went while we were asking →
    ///   there is nothing to attach to, and the record is buried.
    async fn reattach(self: Arc<Self>, runs: &Arc<dyn ProviderRuns>, record: &runs::RunRecord, claim: gate::SessionLease, boot: &gate::Boot<'_>) -> Reattached {
        let Some(shim_id) = record.shim.as_deref() else {
            // A resident record with no shim named cannot be dialled at all.
            // Only reachable from a record written by a server old enough to
            // predate the field, which is to say never in practice.
            tracing::warn!(session = %record.session_id, "a resident record names no shim; watching its pid instead");
            return Reattached::Watching(claim);
        };
        let sidecars = shim::Sidecars::new(self.host.shim_dir.clone(), shim_id);
        match shim::attach(&sidecars).await {
            Ok(link) => {
                runs.clone().attach(record.clone(), sidecars, link, claim);
                Reattached::Driving
            }
            // A tombstone, or a pid that went during the handshake. The process
            // is over; what is left is to say how it ended, which the shim wrote
            // down before it closed the stream.
            Err(shim::LinkError::Corpse(why)) => {
                tracing::info!(session = %record.session_id, %why, "the resident named by this record is already dead");
                drop(claim);
                self.bury_resident(&**runs, record, &sidecars, boot).await;
                Reattached::Buried
            }
            Err(error) => {
                if !procs::is_same_process(record.cli_pid, record.pid_token.as_deref()) {
                    tracing::info!(session = %record.session_id, %error, "the resident named by this record has gone");
                    drop(claim);
                    self.bury_resident(&**runs, record, &sidecars, boot).await;
                    return Reattached::Buried;
                }
                // The socket is unreachable and the process is not. Everything
                // the shim was for is lost — the turn's end, its output, the
                // ability to send anything at all — but the session stays
                // claimed, which is the half that must not wait: an untracked
                // resident reads as a session with nothing in flight, so the
                // composer opens and the next message starts a second claude
                // against the transcript the first one is still writing.
                tracing::warn!(
                    session = %record.session_id,
                    pid = record.cli_pid,
                    shim = %shim_id,
                    %error,
                    "cannot re-attach to a resident that is still running; holding its session and watching the pid"
                );
                Reattached::Watching(claim)
            }
        }
    }

    /// Clean up after a resident that did not survive, tombstone and all.
    ///
    /// The generic [`InjectionRegistry::bury`] asks the transcript whether the
    /// turn was cut short, because for a `--print` child that is the only
    /// evidence there is. A resident leaves better: the shim's job on the way
    /// out is to write down how claude ended, and that note is worth more than
    /// an inference — so it is used where it exists and the sidecars are swept
    /// up behind it.
    async fn bury_resident(&self, runs: &dyn ProviderRuns, record: &runs::RunRecord, sidecars: &shim::Sidecars, boot: &gate::Boot<'_>) {
        if runs.holds_state(record) {
            runs.dead(record.clone());
            return;
        }
        if let Some(exit) = sidecars.read_tombstone() {
            tracing::info!(session = %record.session_id, ?exit, "the resident's shim recorded how it ended");
        }
        self.bury(runs, record, boot).await;
        let _ = std::fs::remove_file(sidecars.exit());
        let _ = std::fs::remove_file(sidecars.stderr());
    }

    /// Put an inherited run back in the map and start watching its pid.
    fn resume(self: Arc<Self>, runs: Arc<dyn ProviderRuns>, record: runs::RunRecord, claim: gate::SessionLease) {
        let (finished_tx, _) = tokio::sync::watch::channel(false);
        let run = Arc::new(RunEntry {
            state: Mutex::new(RunState {
                state: InjectionRunState::Running,
                error: None,
                interrupted: false,
                // The original timestamp, so an adopted run does not claim to
                // have started when the server restarted.
                started_at: record.started_at.clone(),
                finished_at: None,
                pid: Some(record.cli_pid),
                // Carried out of the record so that every later signal can make
                // the same check adoption just made, rather than trusting a
                // number whose meaning expires the moment the CLI exits.
                pid_token: record.pid_token.clone(),
                // The pipes died with the process that opened them. There is no
                // way to recover what was already written, and no way to read
                // what comes next: the CLI's stdout now goes nowhere.
                // The old clock is unrecoverable — `Instant` has no meaning
                // across processes — so the run is given a full idle grace
                // period from now. Erring towards patience: killing a healthy
                // run is worse than noticing a stuck one late.
                last_progress: Instant::now(),
                idle_killed: false,
                current_id: record.session_id.clone(),
                run_id: record.run_id.clone(),
                lock: Some(claim),
            }),
            finished: finished_tx,
        });
        self.host.runs
            .lock()
            .expect("injections poisoned")
            .insert(self.host.key_of(record.source, &record.session_id), run.clone());
        tracing::info!(
            session = %record.session_id,
            source = %record.source,
            pid = record.cli_pid,
            "adopted a run that outlived the last server"
        );
        tokio::spawn(watch_adopted(run, record, self, runs));
    }

    /// Deal with a record whose CLI is gone.
    ///
    /// The record's mere presence says the run had not finished when the last
    /// server wrote the file, but not how it ended: the CLI may have run to
    /// completion after the server died, or been killed along with it. Only the
    /// transcript can tell the two apart, so that is what is asked — and only
    /// when it says the turn was cut short does anything surface, because a
    /// spurious failure on every restart would be worse than silence.
    async fn bury(&self, runs: &dyn ProviderRuns, record: &runs::RunRecord, boot: &gate::Boot<'_>) {
        self.host.journal.remove(&record.run_id);
        // Taken and immediately dropped: a lock file whose run is over is
        // debris, and leaving it would keep the session claimed by nobody.
        boot.sweep(record.source, &record.session_id);
        #[cfg(any(test, feature = "test-support"))]
        self.host.step("buried", record.source, Some(&record.session_id));
        if !runs.was_cut_short(record).await {
            tracing::info!(session = %record.session_id, "a run finished while the server was down");
            return;
        }

        let key = self.host.key_of(record.source, &record.session_id);
        {
            let runs = self.host.runs.lock().expect("injections poisoned");
            // A run started since boot outranks a note about a dead one.
            if runs.contains_key(&key) {
                return;
            }
        }
        let (finished_tx, _) = tokio::sync::watch::channel(true);
        let run = Arc::new(RunEntry {
            state: Mutex::new(RunState {
                // The existing failure path, with a softer reason than usual —
                // rather than a new state the web app would have to learn.
                state: InjectionRunState::Failed,
                error: Some(RESTART_NOTE.to_string()),
                interrupted: false,
                started_at: record.started_at.clone(),
                finished_at: Some(iso8601(std::time::SystemTime::now())),
                pid: None,
                pid_token: None,
                last_progress: Instant::now(),
                idle_killed: false,
                current_id: record.session_id.clone(),
                run_id: record.run_id.clone(),
                lock: None,
            }),
            finished: finished_tx,
        });
        self.host.runs.lock().expect("injections poisoned").insert(key, run);
        tracing::info!(session = %record.session_id, source = %record.source, "a run was cut short by a restart");
    }
}

/// Watch an adopted run's pid until it exits, re-arming the idle watchdog.
///
/// Unlike a newly spawned child, an adopted process has no `Child` handle:
/// completion is a poll rather than a `wait`, and the idle watchdog has only
/// the transcript's mtime to go on — the "the CLI printed something" half of
/// progress is gone with the pipes.
///
/// A resident claude reaches here too, until it can be re-attached through its
/// shim, and the watchdog is skipped for it. Not as a caution: a resident is the
/// *session*, so "quiet for twenty minutes" is what a session between messages
/// looks like, and signalling one would end every conversation on the machine
/// that nobody happened to be talking to. See [`claude_code::resident::Driver::watchdog`] for
/// why a live resident is only ever marked, never reaped.
async fn watch_adopted(run: Arc<RunEntry>, record: runs::RunRecord, registry: Arc<InjectionRegistry>, runs: Arc<dyn ProviderRuns>) {
    let mut since_watchdog = Instant::now();
    loop {
        tokio::time::sleep(ADOPTED_POLL).await;

        if !procs::is_same_process(record.cli_pid, record.pid_token.as_deref()) {
            break;
        }
        if !(record.resident || runs.holds_state(&record)) && since_watchdog.elapsed() >= registry.host.watchdog_interval {
            since_watchdog = Instant::now();
            if let Some(pid) = check_idle(&run, &*runs, registry.host.idle_timeout).await {
                // Checked again rather than relying on the check at the top of
                // the loop: `check_idle` reads the transcript's mtime, and a pid
                // can stop being this run across that await.
                if !procs::is_same_process(pid, record.pid_token.as_deref()) {
                    tracing::info!(pid, session = %record.session_id, "the idle watchdog's pid is no longer its CLI; not signalling");
                    // Nothing was ended by remote-hand, so the run must not go
                    // on to say it was. The loop's own exit condition — the pid
                    // is gone — is what has actually happened.
                    run.state.lock().expect("run state poisoned").idle_killed = false;
                    break;
                }
                signal(pid, libc::SIGTERM);
            }
        }
    }

    let mut state = run.state.lock().expect("run state poisoned");
    vanished(&mut state, runs.cli_name(), registry.host.idle_timeout);
    let holds_state = runs.holds_state(&record);
    let claim = if holds_state {
        state.lock.take()
    } else {
        release(&registry.host, &mut state)
    };
    drop(state);
    drop(claim);
    if holds_state {
        runs.dead(record);
    }
    let _ = run.finished.send(true);
}

/// Signal one process, having already established that it is the right one.
///
/// Every caller owes that proof: [`procs::is_same_process`] must agree that the pid is still
///   the process its record named — the check
///   [`InjectionRegistry::interrupt`] and [`watch_adopted`] make immediately
///   before calling this.
///
/// Reaching here without one of those is how a Stop button ends a stranger's
/// build, so the check belongs at the call site where the provenance is known,
/// not behind a flag passed down to here.
fn signal(pid: u32, sig: libc::c_int) {
    // SAFETY: a signal to a single pid whose identity the caller has just
    // established; the worst outcome is an `ESRCH`, which is ignored.
    unsafe { libc::kill(pid as libc::pid_t, sig) };
}

/// Give up everything a finished run was holding: its record, and its claim.
///
/// The order matters in one direction — the record goes before the claim,
/// because a claim released first would let another instance start a run this
/// server is still advertising as in flight.
///
/// The claim is handed back rather than dropped here so the caller can release
/// the state lock before its unlink runs.
#[must_use = "the claim is released when this is dropped, which should happen outside the state lock"]
fn release(registry: &RunHost, state: &mut RunState) -> Option<gate::SessionLease> {
    registry.journal.remove(&state.run_id);
    state.lock.take()
}

/// Write the ending of a run whose pid stopped being its CLI.
///
/// Two places reach this, and they must not word the same fact differently or
/// the state would flicker between them: [`watch_adopted`]'s poll, which is how
/// an inherited run normally ends, and [`InjectionRegistry::interrupt`] when the
/// pid fails the identity guard.
///
/// Failing closed there is the asymmetry the whole pid-reuse guard rests on. A
/// signal cannot be taken back: `SIGTERM` to a recycled pid ends a stranger's
/// process, and no later check undoes it. Declining to signal a run that has in
/// fact already ended costs nothing at all — the run is over either way, and
/// saying so here is the truth rather than a guess.
fn vanished(state: &mut RunState, cli: &str, idle_timeout: Duration) {
    state.pid = None;
    state.finished_at = Some(iso8601(std::time::SystemTime::now()));
    // There is no exit status to read: the process that could have reaped this
    // child is gone, and pid 1 got the code. So the run is reported as having
    // ended, which is the one thing actually known, and the transcript says
    // what it achieved. Calling it a failure would paint every successful
    // adopted run red.
    if state.interrupted {
        state.state = InjectionRunState::Failed;
        state.error = Some("interrupted from Droppen".to_string());
    } else if state.idle_killed {
        state.state = InjectionRunState::Failed;
        state.error = Some(format!(
            "{} stopped making progress for {} minutes, so Droppen ended it",
            cli,
            (idle_timeout.as_secs_f64() / 60.0).round() as u64
        ));
    } else {
        state.state = InjectionRunState::Ok;
    }
}

/// Node reports signals by name (`SIGTERM`), and the message is user-facing.
pub fn signal_name(signal: i32) -> String {
    match signal {
        libc::SIGTERM => "SIGTERM".to_string(),
        libc::SIGKILL => "SIGKILL".to_string(),
        libc::SIGINT => "SIGINT".to_string(),
        libc::SIGHUP => "SIGHUP".to_string(),
        libc::SIGSEGV => "SIGSEGV".to_string(),
        other => format!("signal {other}"),
    }
}

/// Whether the run has gone quiet for too long; returns the pid to end.
///
/// Progress is either CLI output (codex exec --json streams events) or growth of
/// the session transcript, which both CLIs write continuously — that is what
/// the live feed renders.
async fn check_idle(run: &RunEntry, runs: &dyn ProviderRuns, idle_timeout: Duration) -> Option<u32> {
    let current_id = run.state.lock().expect("run state poisoned").current_id.clone();
    let transcript_progress = runs.last_write(&current_id).await;

    let mut state = run.state.lock().expect("run state poisoned");
    if let Some(progress) = transcript_progress {
        if progress > state.last_progress {
            state.last_progress = progress;
        }
    }
    if state.last_progress.elapsed() < idle_timeout {
        return None;
    }
    state.idle_killed = true;
    state.pid
}

/// How long one push stands for every other move ahead between the same two
/// accounts.
const AHEAD_PUSH_WINDOW: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Whether this move ahead from `from` to `to` is the first one in the current
/// [`AHEAD_PUSH_WINDOW`], and so the one that gets a push.
pub fn first_ahead_push(from: &str, to: &str, now: std::time::Instant) -> bool {
    static LAST: std::sync::OnceLock<std::sync::Mutex<HashMap<(String, String), std::time::Instant>>> = std::sync::OnceLock::new();
    let mut last = LAST.get_or_init(Default::default).lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    last.retain(|_, at| now.saturating_duration_since(*at) < AHEAD_PUSH_WINDOW);
    let key = (from.to_string(), to.to_string());
    if last.contains_key(&key) {
        return false;
    }
    last.insert(key, now);
    true
}

#[cfg(test)]
mod tests {
    /// A burst of moves ahead between the same two accounts pushes once; a
    /// different pair, or the same pair after the window, pushes again.
    #[test]
    fn a_burst_of_moves_ahead_between_two_accounts_pushes_once() {
        let t0 = std::time::Instant::now();
        assert!(super::first_ahead_push("burst-a", "burst-b", t0));
        assert!(!super::first_ahead_push("burst-a", "burst-b", t0 + std::time::Duration::from_secs(60)));
        assert!(super::first_ahead_push("burst-a", "burst-c", t0 + std::time::Duration::from_secs(61)));
        assert!(super::first_ahead_push(
            "burst-a",
            "burst-b",
            t0 + super::AHEAD_PUSH_WINDOW + std::time::Duration::from_secs(1)
        ));
    }

    use super::*;

    #[test]
    fn the_attachment_suffix_is_what_the_transcript_parser_reads_back() {
        let suffix = attachment_suffix(&["/tmp/a.png".to_string(), "/tmp/b.txt".to_string()]);
        let split = droppen_sessions::util::parse_attachment_suffix(&format!("hi{suffix}"));
        assert_eq!(split.text, "hi");
        assert_eq!(split.images, ["/tmp/a.png"]);
        assert_eq!(split.files[0].path, "/tmp/b.txt");
    }

    #[test]
    fn the_three_account_states_are_distinguishable() {
        assert_eq!(AccountChoice::from_request(None), AccountChoice::Unset);
        assert_eq!(AccountChoice::from_request(Some(None)), AccountChoice::Local);
        assert_eq!(AccountChoice::from_request(Some(Some("x".into()))), AccountChoice::Account("x".into()));
        assert_eq!(AccountChoice::Local.named(), None, "null is not an account");
        assert_eq!(
            AccountChoice::from_start_request(Some(Some("x".into())), Some(AccountFrom::Start)),
            AccountChoice::Rule("x".into()),
            "an account the start rule named is the rule's, not a pick"
        );
        assert_eq!(AccountChoice::Rule("x".into()).named(), None, "and nothing binds it as somebody's choice");
        assert_eq!(AccountChoice::from_start_request(Some(Some("x".into())), None), AccountChoice::Account("x".into()));
        assert_eq!(AccountChoice::from_start_request(Some(None), Some(AccountFrom::Start)), AccountChoice::Local);
    }

    #[test]
    fn an_environment_override_only_wins_when_it_is_a_positive_number() {
        assert_eq!(env_ms("DROPPEN_DEFINITELY_UNSET_TIMEOUT", Duration::from_secs(7)), Duration::from_secs(7));
    }

    /// The runs map as a driver keeps its sessions' turns in it: a turn put
    /// in is found by its session and source while it runs and not once it
    /// has ended, follows its session to a new id along with the id the
    /// watchdog reads, and goes when it is taken out.
    #[test]
    fn a_driver_s_turn_is_found_renamed_and_withdrawn_by_its_session() {
        let home = tempfile::tempdir().expect("tempdir");
        let host = testing::injection_registry(home.path()).host.clone();

        host.publish_run(Source::Claude, "first", RunEntry::resident_turn("first"));
        assert!(host.running_run(Source::Claude, "first").is_some());
        assert!(host.running_run(Source::Codex, "first").is_none(), "another source's session of that id has none");

        host.rename_run(Source::Claude, "first", "second");
        assert!(host.run_injection(Source::Claude, "first").is_none(), "gone from the old id");
        let run = host.running_run(Source::Claude, "second").expect("under the new id");
        assert_eq!(run.state.lock().expect("run state").current_id, "second", "the id the watchdog reads");

        run.state.lock().expect("run state").state = InjectionRunState::Ok;
        assert!(host.running_run(Source::Claude, "second").is_none(), "an ended turn is not running");
        assert!(host.run_injection(Source::Claude, "second").is_some(), "though it is still in the map");
        host.withdraw_run(Source::Claude, "second");
        assert!(host.run_injection(Source::Claude, "second").is_none());
    }
}
