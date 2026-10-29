//! Restart-safe CLI drivers: Claude stream-json and Codex App Server.
//!
//! Both residents use the protocol-agnostic session shim. Legacy codex exec
//! runs are still adopted by PID until they finish, but no new exec is spawned.

use std::collections::HashMap;
use std::path::PathBuf;
#[cfg(test)]
use std::process::Stdio;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;

use crate::accounts::store::{AccountError, AccountLaunch, AccountProvider};
use droppen_instance::identity::ServerId;
use droppen_instance::instance::InstanceDirs;
use droppen_instance::paths::Paths;
use droppen_base::process_env::{redact_secrets, Env};
use droppen_base::procs;
use crate::record;
use crate::sessions::{claude, codex};
use droppen_sessions::{Injection, InjectionRunState, SessionRef, SessionStatus, Source};
use droppen_base::time::iso8601;
use gotcha::Schematic;

pub mod acp;
mod app_server;
mod common;
pub mod drafts;
mod gate;
pub mod questions;
pub mod receipts;
mod registry;
mod resident;
mod runs;
pub mod shim;

pub(crate) use app_server::Codex;
pub use common::{AnswerOutcome, Phase, QueueOutcome, Reply, ResidentState, StopTaskOutcome};
pub use gate::SpawnPermit;
pub use registry::{Capabilities, Provider, ProviderRegistry, Run, SendTo, StartAt};
pub(crate) use resident::ClaudeCode;

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
    fn message(text: impl Into<String>) -> Self {
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
    /// ([`crate::acp_agent`]). A message to an existing session always goes to
    /// the agent that session runs on.
    pub acp_agent: Option<String>,
}

/// Mode names aligned with current Codex terminology, mapped onto what
/// `codex exec` actually supports (approvals are always "never" headless).
/// Raw sandbox values are kept as aliases for older stored preferences.
pub(crate) fn codex_permission_flags(permission: &str) -> Result<Vec<&'static str>, InjectError> {
    Ok(match permission {
        "auto" | "workspace-write" => vec!["--sandbox", "workspace-write"],
        "read-only" => vec!["--sandbox", "read-only"],
        "full-access" | "danger-full-access" => vec!["--sandbox", "danger-full-access"],
        "bypass-all" => vec!["--dangerously-bypass-approvals-and-sandbox"],
        other => return Err(InjectError::message(format!("unknown codex permission mode: {other}"))),
    })
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

/// codex has no flag for non-image files, so point it at them in the prompt text.
fn with_codex_files(text: &str, options: &SendOptions) -> String {
    if options.files.is_empty() {
        text.to_string()
    } else {
        format!("{text}{}", attachment_suffix(&options.files))
    }
}

/// Refuse an account that belongs to the other CLI.
fn require_provider(launch: &AccountLaunch, wanted: AccountProvider) -> Result<(), InjectError> {
    if launch.provider == wanted {
        return Ok(());
    }
    Err(InjectError::ProviderMismatch {
        account: launch.provider,
        wanted,
    })
}

/// One codex account, resolved and ready to be spawned under.
#[derive(Debug, Clone)]
pub(crate) struct CodexAccount {
    /// Which account it is, so a live process can be compared against it.
    pub(crate) id: String,
    /// What it contributes to the spawn.
    pub(crate) launch: AccountLaunch,
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
    /// no model gets ([`crate::accounts::rotation::StartInputs::still_names`]) —
    /// then it, logged as the rule's choice — or, if the rule has moved on since
    /// the page asked, wherever the rule places this start's model now.
    async fn rule_account(&self, provider: AccountProvider, named: &str, model: Option<&str>) -> Option<String> {
        let inputs = crate::accounts::rotation::StartInputs::gather(&self.accounts, &self.quota, provider).await;
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

    /// Where a start that names no account runs ([`crate::accounts::rotation::start_for`]),
    /// and a line in the log whenever that is not simply the default or the local
    /// login: which login the start was moved off, which it went to, and why.
    async fn start_account(&self, provider: AccountProvider, model: Option<&str>) -> Option<String> {
        use crate::accounts::rotation::StartReason;
        let start = crate::accounts::rotation::start_for(&self.accounts, &self.quota, provider, model).await;
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
    pub(crate) fn redact(&self, text: &str) -> String {
        redact_secrets(&self.accounts.redact_known_secrets(text))
    }
}

impl InjectionRegistry {
    /// Write a rotation decision down and tell everyone who should hear of it:
    /// the log line, the phone, the session's feed and the list.
    ///
    /// The one `info` line per decision is here, so every path that decides —
    /// a switch, nowhere to go, the cap — is logged the same way. Returns the
    /// record's id, for a switch that learns later how much it interrupted.
    fn record_rotation(&self, decision: &crate::accounts::rotation::Decision, notice: &str) -> Option<i64> {
        let id = match self.accounts.rotation_log().record(decision) {
            Ok(id) => Some(id),
            Err(error) => {
                tracing::warn!(session = %decision.session_id, %error, "cannot record an account rotation");
                None
            }
        };
        tracing::info!(
            session = %decision.session_id,
            from = %decision.from.id,
            to = %decision.to.id,
            class = decision.class.as_str(),
            scope = %decision.scope,
            until = decision.until_ms.map(crate::accounts::rest::iso_ms).as_deref().unwrap_or("-"),
            interrupted = decision.interrupted_tasks,
            outcome = decision.outcome.as_str(),
            trigger = decision.trigger.as_str(),
            used = ?decision.used_percent,
            "decided an account rotation"
        );
        // Known limitation: the failed turn behind this decision has
        // already sent its own "Run failed" notification a moment earlier, so
        // a limit reaches the phone twice — the failure, then what was done
        // about it. The second is the one that bypasses the throttle. A move
        // ahead of a limit has no failure before it, and reaches it once.
        // Every session on one account crosses the threshold together, so a
        // move ahead tends to come in a burst: N sessions from A to B within a
        // few minutes. One push per account pair per window says it; the rest
        // would only bury it. A limit-driven rotation always pushes.
        let ahead_burst = decision.trigger == droppen_sessions::account::RotationTrigger::Ahead
            && !first_ahead_push(&decision.from.id, &decision.to.id, std::time::Instant::now());
        if !ahead_burst {
            if let Some(alerts) = self.alerts.get() {
                alerts.rotation(
                    self.paths.clone(),
                    Source::Claude,
                    decision.session_id.clone(),
                    decision.outcome,
                    decision.trigger,
                    notice.to_string(),
                );
            }
        }
        self.announce(Source::Claude, &decision.session_id);
        id
    }
}

impl RunHost {
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
    pub(crate) fn record_limit(&self, mut observation: crate::accounts::rest::Observation) {
        use crate::accounts::rest;
        observation.raw = self.redact(&observation.raw);
        observation.reason = if observation.reason.is_empty() {
            crate::accounts::limits::reason_line(&observation.raw, observation.verdict.class)
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
    /// [`crate::accounts::local_login`]); codex has no reading of its login,
    /// and a stored account's rest only ended with a new credential. Without
    /// this, one passing refusal would mark a login signed out for good — and a
    /// signed-out local login hands every start that names no account to a
    /// stand-in ([`crate::accounts::rotation::start_choice`]).
    pub(crate) fn credential_accepted(&self, account_id: &str, session_id: &str) {
        use droppen_sessions::account::LimitClass;
        let log = self.accounts.limit_log();
        if log
            .active_observed_ms(account_id, LimitClass::Auth, crate::accounts::rest::now_ms())
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

fn cli_binary(source: Source) -> &'static str {
    match source {
        Source::Claude => "claude",
        Source::Codex => "codex",
        Source::Acp => "the ACP agent",
    }
}

// -------------------------------------------------------------------- registry

/// One run's mutable state.
struct RunState {
    state: InjectionRunState,
    error: Option<String>,
    interrupted: bool,
    started_at: String,
    finished_at: Option<String>,
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
    state: Mutex<RunState>,
    /// Flipped once the child has exited and the final state has been written.
    ///
    /// A `watch` rather than a `Notify`: `notify_waiters` stores no permit, so a
    /// waiter that arrives after the run finished — which is exactly what the
    /// codex handshake does when stdout closes first — would wait forever.
    finished: tokio::sync::watch::Sender<bool>,
}

impl RunEntry {
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
/// reach it through themselves ([`InjectionRegistry`], [`acp::AcpRuns`]).
pub struct RunHost {
    /// The stored logins, for one job only: taking the credentials this server
    /// injected back out of whatever the CLI printed. See
    /// [`RunHost::redact`].
    accounts: Arc<crate::accounts::store::AccountsStore>,
    /// The quota readings, for one job only: every `rate_limit_event` a
    /// resident's claude writes is a reading of the login it runs on, and the
    /// driver files it here. The same handle [`crate::drive::Drive::quota`]
    /// serves the panel from, so a reading is on the panel the moment it lands.
    quota: Arc<crate::accounts::quota::QuotaCache>,
    /// This server's name, so the keys below can say whose they are.
    ///
    /// Only ever this process' own: everything keyed by it is in memory and
    /// dies with the registry. See [`RunHost::key_of`].
    server: ServerId,
    /// Every provider's runs, which [`Self::driver_states`] gathers: each
    /// listed itself as it was made ([`ListsRuns`]).
    listed: Mutex<Vec<Weak<dyn ListsRuns>>>,
    runs: Mutex<HashMap<String, Arc<RunEntry>>>,
    /// Where a resident's shim leaves its socket, stderr tail and tombstone.
    shim_dir: PathBuf,
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
    interrupt_step: Duration,
    /// How often the watchdog checks.
    watchdog_interval: Duration,
    /// Where transcripts are discovered, for the idle watchdog.
    paths: Arc<Paths>,
    /// The record of in-flight runs, in this server's own instance directory.
    journal: runs::RunsFile,
    /// The session feed and the list feed, told when a driver changes something
    /// no file shows. Set once, at startup; empty in tests that build no app.
    listeners: std::sync::OnceLock<Listeners>,
    /// Where the drivers send a turn's end, a question and a rotation for the
    /// phone. Set once, at startup; empty in tests that build no app.
    alerts: std::sync::OnceLock<Arc<dyn Alerts>>,
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
    drafts: drafts::Drafts,
}

/// Claude Code's residents and Codex's app servers, and the spawn gate, on
/// the host whose runs every provider shares ([`RunHost`]).
pub struct InjectionRegistry {
    /// What its runs share with every provider's.
    host: Arc<RunHost>,
    codex_servers: Mutex<HashMap<String, app_server::Handle>>,
    /// The resident claude behind each session, keyed by session id alone —
    /// there is no codex resident, so the source is not part of the key.
    residents: Mutex<HashMap<String, resident::Handle>>,
    /// Serializes the gap between "no resident for this session" and "one in the
    /// map", and holds the per-session claims; see [`InjectionRegistry::deliver`].
    spawn_gate: gate::SpawnGate,
    /// How long a claude turn may make no progress before it is *marked*
    /// stalled. Nothing is ever killed for it; see [`resident`].
    stall_after: Duration,
    /// How long a settings change may be parked behind claude's background work
    /// before the process is restarted anyway; `None` to wait for ever.
    ///
    /// A ceiling rather than a policy: parking is right, and the tasks a
    /// resident exists to protect can legitimately run for an hour. What this
    /// stops is the *unbounded* case — a background set that is wrong about
    /// work which is not there, holding a message nobody can stop or take back.
    /// See [`resident::Driver::waited_long_enough`].
    await_background: Option<Duration>,
    /// `DROPPEN_ROTATE_AHEAD_PERCENT`: the used percent of a window at which a
    /// session is moved off its account at the next quiet point, before the
    /// limit refuses a turn; `None` when that is switched off. See
    /// [`resident::Driver::rotate_ahead`].
    rotate_ahead: Option<f64>,
    /// Where claude drops a background task's output file.
    ///
    /// The CLI writes one `<task-id>.output` per background task under
    /// `<root>/<cwd-slug>/<session-id>/tasks/`, and streams into it while the
    /// task runs — so for a `local_bash` task, which has no subagent transcript
    /// anywhere, this is the *only* place on disk that says it is still alive.
    /// [`resident::Driver::subagents_recently_active`] reads it for exactly that
    /// reason.
    ///
    /// A root rather than the directory itself because the slug in the middle is
    /// the working directory with its separators and its non-ASCII mangled, and
    /// reproducing that encoding is a guess; looking for the session id one level
    /// down is not. Overridable so a test can point it at a directory it owns.
    task_root: PathBuf,
    /// How long a resident may sit idle before it is retired, or `None` — the
    /// default — for never.
    ///
    /// Off by default because the resident model's whole promise is that the
    /// next message goes into a process that is already there and that nothing
    /// running in the background is ever cut off, and both are weakened by
    /// ending idle processes. It exists because a resident costs three or four
    /// hundred megabytes, and a machine with enough sessions open needs a way to
    /// say so. See [`resident::Driver::idle_shutdown`].
    resident_idle: Option<Duration>,
    /// How long the recovery ladder waits before each restart of a resident
    /// that died unexpectedly: one entry per death inside the breaker's window,
    /// the last reused past the end.
    ///
    /// A field rather than the constant it defaults to so a test can walk the
    /// whole ladder — resume, fork, breaker — without sitting out real
    /// backoffs; production always runs [`resident::RECOVERY_BACKOFF`].
    recovery_backoff: [Duration; 3],
    /// How long a turn opened for a *steered* message is given to prove it
    /// exists before the driver concludes claude answered the message inside
    /// the turn it was handed to.
    ///
    /// The whole of the merged/deferred ambiguity is timed by this, so it is a
    /// field for the same reason [`Self::recovery_backoff`] is: a test has to
    /// be able to watch both outcomes without sitting out the real one.
    /// Production runs [`resident::STEER_SETTLE`], or `DROPPEN_STEER_SETTLE_MS`.
    steer_settle: Duration,
    /// How often a resident holding steered messages looks in the transcript
    /// for claude having taken them into the turn in flight.
    ///
    /// The backstop for a session nobody is watching: a watched one is told the
    /// moment its transcript grows — see [`InjectionRegistry::transcript_grew`].
    /// Production runs [`resident::ABSORB_POLL`], or `DROPPEN_ABSORB_POLL_MS`.
    absorb_poll: Duration,
    /// What the spawn gate and the session's claim were at each traced point
    /// of a spawn; see [`Step`].
    #[cfg(test)]
    steps: Mutex<Vec<Step>>,
    /// Where a session's flight recording goes, if it is being made at all.
    ///
    /// A factory rather than a recorder: one is opened per *resident*, lives on
    /// its driver, and is what carries the session's life into
    /// [`crate::record`]. See [`resident::Driver::recorder`].
    recorders: record::Recorders,
    /// What a claude spawn's environment gains before it starts: on the send
    /// path, and for the process an automatic rotation moves a session to,
    /// which a driver builds by itself.
    decoration: Arc<dyn SpawnDecoration>,
    /// When a quota reading last woke the residents holding a queue; see
    /// [`InjectionRegistry::recheck_holds`].
    reading_recheck: Mutex<Option<Instant>>,
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
    /// as idle, so the alert cannot describe the next queued turn.
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

/// Whoever follows a session from outside the driver: the per-session feed and
/// the session list.
struct Listeners {
    feeds: Arc<dyn SessionFeeds>,
    refresh: droppen_http::refresh::Refresh,
}

/// A duration from the environment, in milliseconds, or the fallback
/// ([`droppen_base::env::var`]).
fn env_ms(name: &str, fallback: Duration) -> Duration {
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
        accounts: Arc<crate::accounts::store::AccountsStore>,
        quota: Arc<crate::accounts::quota::QuotaCache>,
        decoration: Arc<dyn SpawnDecoration>,
    ) -> Arc<Self> {
        Arc::new(Self {
            host: RunHost::new(paths.clone(), instance, accounts, quota),
            codex_servers: Mutex::new(HashMap::new()),
            residents: Mutex::new(HashMap::new()),
            spawn_gate: gate::SpawnGate::new(paths.data_dir().join("inject-locks")),
            // Ten minutes of silence is a *report*, not a sentence: the only
            // thing that happens at this threshold is that the turn is marked.
            stall_after: env_ms("DROPPEN_STALL_MS", Duration::from_secs(10 * 60)),
            // Half an hour: past every background job short enough that waiting
            // is the obvious answer, and well short of a session that is stuck.
            // Zero means "wait for ever", which is why this does not go through
            // `env_ms` either.
            await_background: match droppen_base::env::var("DROPPEN_AWAIT_BACKGROUND_MS")
                .and_then(|raw| raw.trim().parse::<u64>().ok())
            {
                Some(0) => None,
                Some(ms) => Some(Duration::from_millis(ms)),
                None => Some(Duration::from_secs(30 * 60)),
            },
            rotate_ahead: crate::accounts::rotation::ahead_threshold_from_env(),
            // `/tmp`, not `TMPDIR`: claude writes here, and on macOS `TMPDIR` is
            // a per-process directory under `/var/folders` that it never uses.
            task_root: droppen_base::env::var("DROPPEN_TASK_ROOT").map(PathBuf::from).unwrap_or_else(|| {
                // SAFETY: `getuid` reads the calling process' own uid and cannot fail.
                PathBuf::from(format!("/tmp/claude-{}", unsafe { libc::getuid() }))
            }),
            // Zero, absent and unparseable all mean "never", which is why this
            // is the one timing here that does not go through `env_ms`: its
            // fallback is not a duration.
            resident_idle: droppen_base::env::var("DROPPEN_RESIDENT_IDLE_MS")
                .and_then(|raw| raw.trim().parse::<u64>().ok())
                .filter(|ms| *ms > 0)
                .map(Duration::from_millis),
            recovery_backoff: resident::RECOVERY_BACKOFF,
            steer_settle: env_ms("DROPPEN_STEER_SETTLE_MS", resident::STEER_SETTLE),
            absorb_poll: env_ms("DROPPEN_ABSORB_POLL_MS", resident::ABSORB_POLL),
            #[cfg(test)]
            steps: Mutex::new(Vec::new()),
            recorders: record::Recorders::new(paths, instance),
            decoration,
            reading_recheck: Mutex::new(None),
        })
        .listed()
    }

    /// Itself, its Claude Code and Codex runs listed with its host, ahead of
    /// any other provider's ([`RunHost::driver_states`]).
    fn listed(self: Arc<Self>) -> Arc<Self> {
        let runs: Arc<dyn ListsRuns> = self.clone();
        self.host.list(Arc::downgrade(&runs));
        self
    }
}

impl std::ops::Deref for InjectionRegistry {
    type Target = RunHost;

    fn deref(&self) -> &RunHost {
        &self.host
    }
}

impl ListsRuns for InjectionRegistry {
    fn add_to(&self, states: &mut DriverStatesBuilder) {
        self.claude_driver_states(states);
        self.codex_driver_states(states);
    }
}

impl RunHost {
    /// The host of a registry's runs, the watchdog timed as the environment
    /// asks ([`InjectionRegistry::new`]).
    fn new(
        paths: Arc<Paths>,
        instance: &InstanceDirs,
        accounts: Arc<crate::accounts::store::AccountsStore>,
        quota: Arc<crate::accounts::quota::QuotaCache>,
    ) -> Arc<Self> {
        let receipts = Arc::new(receipts::Receipts::new(droppen_instance::db::at(paths.instance_db())));
        let questions = Arc::new(questions::Questions::new(droppen_instance::db::at(paths.instance_db())));
        Arc::new(Self {
            receipts,
            questions,
            accounts,
            quota,
            server: ServerId::local(&paths),
            listed: Mutex::new(Vec::new()),
            runs: Mutex::new(HashMap::new()),
            shim_dir: instance.session_shims(),
            idle_timeout: env_ms("DROPPEN_IDLE_TIMEOUT_MS", Duration::from_secs(20 * 60)),
            interrupt_step: env_ms("DROPPEN_INTERRUPT_STEP_MS", Duration::from_secs(5)),
            watchdog_interval: env_ms("DROPPEN_WATCHDOG_INTERVAL_MS", Duration::from_secs(60)),
            journal: runs::RunsFile::new(instance.runs_file()),
            listeners: std::sync::OnceLock::new(),
            alerts: std::sync::OnceLock::new(),
            drafts: drafts::Drafts::default(),
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
    pub(crate) fn announce(&self, source: Source, id: &str) {
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
    /// `resident::Driver::on_absorb` — while the list, which shows no
    /// transcript, can be told straight away.
    pub(crate) fn refresh_list(&self) {
        let Some(listeners) = self.listeners.get() else { return };
        listeners.refresh.request_refresh(None);
    }

    /// Push one session's state to whoever is watching it, and nothing else.
    ///
    /// The narrow half of [`RunHost::announce`], for a change only
    /// the session's own view shows — a message's receipt — and that the list
    /// has no column for.
    pub(crate) fn poke_feed(&self, source: Source, id: &str) {
        let Some(listeners) = self.listeners.get() else { return };
        listeners.feeds.poke(&self.key_of(source, id));
    }

    /// The channel claude session `id`'s draft is published on.
    ///
    /// Its driver's [`drafts::DraftWriter`] is the only thing that writes to it
    /// in a running server; a test of the feed stands in for that driver here.
    pub(crate) fn draft_channel(&self, id: &str) -> Arc<tokio::sync::watch::Sender<drafts::Draft>> {
        self.drafts.channel(&self.key_of(Source::Claude, id))
    }

    /// The writer a driver for session `id` publishes its draft through.
    fn draft_writer(&self, id: &str) -> drafts::DraftWriter {
        drafts::DraftWriter::new(self.draft_channel(id))
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
    pub(crate) fn note_receipt(&self, source: Source, session: &str, qid: &str, status: droppen_sessions::delivery::ReceiptStatus) {
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
    pub(super) fn key_of(&self, source: Source, id: &str) -> String {
        SessionRef::new(self.server.clone(), source, id).scoped_key()
    }

    /// The state of this session's run in the runs map: a turn a resident
    /// published there, or a run adopted from the previous server.
    pub(crate) fn run_injection(&self, source: Source, id: &str) -> Option<Injection> {
        self.runs
            .lock()
            .expect("injections poisoned")
            .get(&self.key_of(source, id))
            .map(|run| run.injection())
    }

    /// Everything the session list needs from this subsystem, in one pass.
    ///
    /// Taken once per listing rather than asked per row: a list of two hundred
    /// sessions would otherwise take the residents lock and the runs lock four
    /// hundred times, against a registry every send and every driver contends
    /// on. Both registries are bounded by sessions this server process has
    /// driven, rather than by transcript or list rows, so the snapshot is cheap
    /// to make and cheap to carry.
    pub fn driver_states(&self) -> droppen_sessions::DriverStates {
        let mut states = DriverStatesBuilder::default();
        // In the order they were listed: Claude Code's, then Codex's, then
        // ACP's. Taken out of the list first, so that no provider's lock is
        // taken under it.
        let listed: Vec<Arc<dyn ListsRuns>> = self.listed.lock().expect("listed runs").iter().filter_map(Weak::upgrade).collect();
        for runs in listed {
            runs.add_to(&mut states);
        }
        // Last, so that where a run here and a CLI's own handle share a key,
        // the run's finishing time is the one the list shows.
        for (key, run) in self.runs.lock().expect("injections poisoned").iter() {
            states.run(key.clone(), run.injection());
        }
        droppen_sessions::DriverStates::new(self.server.clone(), states.residents, states.running, states.finished)
    }

    /// Show `runs` in every snapshot [`Self::driver_states`] takes from now
    /// on, for as long as they last.
    pub(crate) fn list(&self, runs: Weak<dyn ListsRuns>) {
        let mut listed = self.listed.lock().expect("listed runs");
        listed.retain(|runs| runs.strong_count() > 0);
        listed.push(runs);
    }

    /// Stop a run in the runs map, which no resident drives: a one-shot run
    /// adopted from the previous server. `false` when nothing is running.
    ///
    /// For such a run there is no turn to end short of the process, and the
    /// old rule applies: a pid that no longer names the run it was recorded for
    /// is *not* signalled, and the run is closed out as finished instead — see
    /// [`vanished`] for why that is the only safe direction.
    pub(crate) fn interrupt_legacy(&self, source: Source, id: &str) -> bool {
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
            vanished(&mut state, source, self.idle_timeout);
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

/// A provider's runs, kept by the provider rather than here, that the session
/// list shows ([`RunHost::driver_states`]).
pub(crate) trait ListsRuns: Send + Sync {
    /// Add its sessions' processes and the state of their runs to `states`.
    fn add_to(&self, states: &mut DriverStatesBuilder);
}

/// The session list's view of the runs, gathered a CLI at a time; see
/// [`RunHost::driver_states`].
#[derive(Default)]
pub(crate) struct DriverStatesBuilder {
    residents: HashMap<String, droppen_sessions::ResidentInfo>,
    running: std::collections::HashSet<String>,
    finished: HashMap<String, String>,
}

impl DriverStatesBuilder {
    /// A resident the list is to show, under its [`RunHost::key_of`]
    /// key: every CLI files its sessions under that one key, so
    /// [`droppen_sessions::DriverStates::resident`] finds any of them with it.
    pub(crate) fn resident(&mut self, key: String, info: droppen_sessions::ResidentInfo) {
        self.residents.insert(key, info);
    }

    /// A run's state under its key: running, or finished at a time. A later
    /// finish under the same key replaces an earlier one's time.
    pub(crate) fn run(&mut self, key: String, injection: Injection) {
        if injection.state == InjectionRunState::Running {
            self.running.insert(key);
        } else if let Some(at) = injection.finished_at.0 {
            self.finished.insert(key, at);
        }
    }
}

/// One point of a spawn, as the tests see it: whether the spawn gate and the
/// session's claim were held there.
///
/// The gate and the claim are the two locks that keep a second CLI off a
/// session, and which of them is held where is the contract the pins in the
/// tests check; a refactor of either keeps every sequence they record.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Step {
    /// The point, by name.
    pub(crate) what: &'static str,
    /// Whether the spawn gate was held, by anyone.
    pub(crate) gate: bool,
    /// Whether the session's claim was held, by anyone; `None` where the
    /// session has no id yet.
    pub(crate) claim: Option<bool>,
}

#[cfg(test)]
impl InjectionRegistry {
    /// Note a [`Step`] of session `id`'s spawn.
    pub(crate) fn step(&self, what: &'static str, source: Source, id: Option<&str>) {
        let step = Step {
            what,
            gate: self.gate_held(),
            claim: id.map(|id| self.claim_held(source, id)),
        };
        self.steps.lock().expect("steps poisoned").push(step);
    }

    /// Every step noted so far, oldest first.
    pub(crate) fn steps(&self) -> Vec<Step> {
        self.steps.lock().expect("steps poisoned").clone()
    }

    /// Whether anyone holds the spawn gate.
    pub(crate) fn gate_held(&self) -> bool {
        self.spawn_gate.held()
    }

    /// Whether anyone holds session `id`'s claim: a claim of our own would be
    /// refused. One that is granted is let go at once.
    pub(crate) fn claim_held(&self, source: Source, id: &str) -> bool {
        self.spawn_gate.claim_held(source, id)
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
    pub async fn adopt_all(self: &Arc<Self>) -> Adopted {
        let mut report = Adopted::default();
        // Its claims need no gate because it runs once, before the server
        // listens: no send can race it.
        let Some(boot) = self.spawn_gate.boot() else {
            tracing::warn!("adoption runs once per server; not adopting again");
            return report;
        };
        for record in self.journal.read() {
            if !procs::is_same_process(record.cli_pid, record.pid_token.as_deref()) {
                if record.codex.is_some() {
                    self.codex_dead(record);
                } else {
                    self.bury(&record, &boot).await;
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
            #[cfg(test)]
            self.step("adoption claimed", record.source, Some(&record.session_id));
            // A resident is *re-attached to*, not watched: the shim held the
            // process' stdio open across the restart, so this server gets the
            // conversation back rather than just the knowledge that a pid
            // exists. A `codex exec` child has no such thing and is watched.
            if record.resident || record.codex.is_some() {
                match self.clone().reattach(&record, claim, &boot).await {
                    Reattached::Driving => report.recovered += 1,
                    Reattached::Watching(claim) => {
                        self.clone().resume(record, claim);
                        report.recovered += 1;
                    }
                    Reattached::Buried => report.finished += 1,
                }
                continue;
            }
            self.clone().resume(record, claim);
            report.recovered += 1;
        }
        // A question can only be answered into the turn that asked it, and that
        // turn is a process: one that did not come back with its session can
        // never take the answer. Those are expired here, once every survivor is
        // in the map, and stay answerable as an ordinary message. A survivor's
        // own questions are its driver's to settle when its replay ends.
        let held: std::collections::HashSet<String> = self.residents.lock().expect("residents poisoned").keys().cloned().collect();
        let expired = self.questions.expire_unheld(&held, questions::now_ms());
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
    async fn reattach(self: Arc<Self>, record: &runs::RunRecord, claim: gate::SessionLease, boot: &gate::Boot<'_>) -> Reattached {
        let Some(shim_id) = record.shim.as_deref() else {
            // A resident record with no shim named cannot be dialled at all.
            // Only reachable from a record written by a server old enough to
            // predate the field, which is to say never in practice.
            tracing::warn!(session = %record.session_id, "a resident record names no shim; watching its pid instead");
            return Reattached::Watching(claim);
        };
        let sidecars = shim::Sidecars::new(self.shim_dir.clone(), shim_id);
        match shim::attach(&sidecars).await {
            Ok(link) => {
                if record.source == Source::Codex {
                    self.codex_attached(record.clone(), link, claim);
                } else {
                    self.attached(record, sidecars, link, claim);
                }
                Reattached::Driving
            }
            // A tombstone, or a pid that went during the handshake. The process
            // is over; what is left is to say how it ended, which the shim wrote
            // down before it closed the stream.
            Err(shim::LinkError::Corpse(why)) => {
                tracing::info!(session = %record.session_id, %why, "the resident named by this record is already dead");
                drop(claim);
                self.bury_resident(record, &sidecars, boot).await;
                Reattached::Buried
            }
            Err(error) => {
                if !procs::is_same_process(record.cli_pid, record.pid_token.as_deref()) {
                    tracing::info!(session = %record.session_id, %error, "the resident named by this record has gone");
                    drop(claim);
                    self.bury_resident(record, &sidecars, boot).await;
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
    async fn bury_resident(&self, record: &runs::RunRecord, sidecars: &shim::Sidecars, boot: &gate::Boot<'_>) {
        if record.codex.is_some() {
            self.codex_dead(record.clone());
            return;
        }
        if let Some(exit) = sidecars.read_tombstone() {
            tracing::info!(session = %record.session_id, ?exit, "the resident's shim recorded how it ended");
        }
        self.bury(record, boot).await;
        let _ = std::fs::remove_file(sidecars.exit());
        let _ = std::fs::remove_file(sidecars.stderr());
    }

    /// Put an inherited run back in the map and start watching its pid.
    fn resume(self: Arc<Self>, record: runs::RunRecord, claim: gate::SessionLease) {
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
        self.runs
            .lock()
            .expect("injections poisoned")
            .insert(self.key_of(record.source, &record.session_id), run.clone());
        tracing::info!(
            session = %record.session_id,
            source = %record.source,
            pid = record.cli_pid,
            "adopted a run that outlived the last server"
        );
        tokio::spawn(watch_adopted(run, record, self));
    }

    /// Deal with a record whose CLI is gone.
    ///
    /// The record's mere presence says the run had not finished when the last
    /// server wrote the file, but not how it ended: the CLI may have run to
    /// completion after the server died, or been killed along with it. Only the
    /// transcript can tell the two apart, so that is what is asked — and only
    /// when it says the turn was cut short does anything surface, because a
    /// spurious failure on every restart would be worse than silence.
    async fn bury(&self, record: &runs::RunRecord, boot: &gate::Boot<'_>) {
        self.journal.remove(&record.run_id);
        // Taken and immediately dropped: a lock file whose run is over is
        // debris, and leaving it would keep the session claimed by nobody.
        boot.sweep(record.source, &record.session_id);
        #[cfg(test)]
        self.step("buried", record.source, Some(&record.session_id));
        if !self.was_cut_short(record).await {
            tracing::info!(session = %record.session_id, "a run finished while the server was down");
            return;
        }

        let key = self.key_of(record.source, &record.session_id);
        {
            let runs = self.runs.lock().expect("injections poisoned");
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
        self.runs.lock().expect("injections poisoned").insert(key, run);
        tracing::info!(session = %record.session_id, source = %record.source, "a run was cut short by a restart");
    }

    /// Whether the session's transcript ends mid-turn.
    ///
    /// Both CLIs already have a "is this session in the middle of something"
    /// reading, derived from the tail of the transcript, and it is exactly the
    /// question here: a turn that ended with the assistant's answer is a turn
    /// that completed, whereas one ending on the user's message or a tool call
    /// never got its reply. Reusing them keeps one definition of mid-turn in
    /// the codebase rather than two that can drift.
    async fn was_cut_short(&self, record: &runs::RunRecord) -> bool {
        match record.source {
            Source::Claude => match claude::find_claude_session_file(&self.paths, &record.session_id).await {
                // No transcript at all: the CLI died before writing a line of
                // the turn it was given.
                None => true,
                Some(path) => claude::claude_activity_status(&path).await == SessionStatus::Running,
            },
            Source::Codex => match codex::find_codex_session(&self.paths, &record.session_id).await {
                None => true,
                // Codex reads `Idle` for a transcript nothing has touched
                // lately, which after a long outage is every transcript. That
                // makes the answer conservative rather than wrong: silence,
                // where claude would have said something.
                Some(session) => session.status == SessionStatus::Running,
            },
            // An ACP agent process is never journalled, so it is never adopted: it
            // dies with the server that started it (see `acp`).
            Source::Acp => false,
        }
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
/// that nobody happened to be talking to. See [`resident::Driver::watchdog`] for
/// why a live resident is only ever marked, never reaped.
async fn watch_adopted(run: Arc<RunEntry>, record: runs::RunRecord, registry: Arc<InjectionRegistry>) {
    let mut since_watchdog = Instant::now();
    loop {
        tokio::time::sleep(ADOPTED_POLL).await;

        if !procs::is_same_process(record.cli_pid, record.pid_token.as_deref()) {
            break;
        }
        if !record.resident && record.codex.is_none() && since_watchdog.elapsed() >= registry.watchdog_interval {
            since_watchdog = Instant::now();
            if let Some(pid) = check_idle(&run, record.source, &registry.paths, registry.idle_timeout).await {
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
    vanished(&mut state, record.source, registry.idle_timeout);
    let claim = if record.codex.is_some() {
        state.lock.take()
    } else {
        release(&registry, &mut state)
    };
    drop(state);
    drop(claim);
    if record.codex.is_some() {
        registry.codex_dead(record);
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
fn vanished(state: &mut RunState, source: Source, idle_timeout: Duration) {
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
            cli_binary(source),
            (idle_timeout.as_secs_f64() / 60.0).round() as u64
        ));
    } else {
        state.state = InjectionRunState::Ok;
    }
}

/// Node reports signals by name (`SIGTERM`), and the message is user-facing.
fn signal_name(signal: i32) -> String {
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
async fn check_idle(run: &RunEntry, source: Source, paths: &Paths, idle_timeout: Duration) -> Option<u32> {
    let current_id = run.state.lock().expect("run state poisoned").current_id.clone();
    let transcript_progress = transcript_progress(source, paths, &current_id).await;

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

/// The transcript's last write, expressed on the same clock as `last_progress`.
async fn transcript_progress(source: Source, paths: &Paths, id: &str) -> Option<Instant> {
    let path = match source {
        Source::Claude => claude::find_claude_session_file(paths, id).await,
        // Codex discovery is the sibling agent's; against the current stub this
        // is always `None`, which falls back to output activity exactly as the
        // TypeScript does for a session with no file yet.
        Source::Codex => codex::find_codex_session(paths, id).await.map(|s| std::path::PathBuf::from(s.file_path)),
        Source::Acp => crate::sessions::acp_transcript::AcpTranscripts::of(paths).locate(id).await.map(|(_, file)| file),
    }?;
    let modified = tokio::fs::metadata(&path).await.ok()?.modified().ok()?;
    let age = std::time::SystemTime::now().duration_since(modified).ok()?;
    Instant::now().checked_sub(age)
}

/// How long one push stands for every other move ahead between the same two
/// accounts.
const AHEAD_PUSH_WINDOW: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Whether this move ahead from `from` to `to` is the first one in the current
/// [`AHEAD_PUSH_WINDOW`], and so the one that gets a push.
fn first_ahead_push(from: &str, to: &str, now: std::time::Instant) -> bool {
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
    fn codex_permission_aliases_map_onto_exec_flags() {
        assert_eq!(codex_permission_flags("auto").expect("auto"), ["--sandbox", "workspace-write"]);
        assert_eq!(
            codex_permission_flags("workspace-write").expect("alias"),
            ["--sandbox", "workspace-write"]
        );
        assert_eq!(
            codex_permission_flags("bypass-all").expect("bypass"),
            ["--dangerously-bypass-approvals-and-sandbox"]
        );
        assert!(codex_permission_flags("nonsense").is_err());
    }

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

    /// Stop must never signal a pid that has stopped being the run it named.
    ///
    /// The window this closes is real and routine rather than exotic: an
    /// adopted run has no `Child` to be woken by, so [`watch_adopted`] only
    /// learns its CLI has exited on a two-second poll, and the kernel is free
    /// to hand that pid to anything at all in between. Pressing Stop inside
    /// those two seconds used to `SIGTERM` whatever now holds the number.
    ///
    /// The run is put in place through [`InjectionRegistry::resume`] rather
    /// than through a boot, because [`InjectionRegistry::adopt_all`] makes the
    /// same identity check and would never have adopted this record: the state
    /// under test is one that was adopted while its CLI was alive and lost it
    /// afterwards, which is the only way a live entry can point at a stranger.
    ///
    /// Stop is pressed through the providers, as the route presses it: claude
    /// and codex each fall back to the adopted run when they hold no process
    /// for the session.
    #[tokio::test]
    async fn stop_never_signals_a_pid_that_is_no_longer_the_run_it_named() {
        const CLAUDE: &str = "eeeeeeee-5555-4555-8555-eeeeeeeeeeee";
        const CODEX: &str = "0199c1a1-5555-7555-8555-eeeeeeeeeeee";

        let home = tempfile::tempdir().expect("tempdir");
        let paths = Arc::new(Paths::new(home.path().to_path_buf()));
        let dirs = InstanceDirs::new(&paths.data_dir(), droppen_instance::instance::DEFAULT_INSTANCE).expect("valid name");
        let decoration = crate::drive::NotifyDecoration::new(paths.clone(), dirs.clone());
        let registry = InjectionRegistry::new(
            paths.clone(),
            &dirs,
            Arc::new(crate::accounts::store::AccountsStore::new(paths.clone(), crate::app::logins())),
            Arc::new(crate::accounts::quota::QuotaCache::new(paths, crate::app::logins())),
            decoration,
        );

        // Alive, and emphatically not an agent run: the developer's `make`,
        // standing in for whatever the recycled pid actually belongs to.
        let mut stranger = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 30"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn a stand-in");

        let providers = ProviderRegistry::new(registry.clone(), crate::app::register_providers);
        for (source, session) in [(Source::Claude, CLAUDE), (Source::Codex, CODEX)] {
            let claim = registry.spawn_gate.lease(source, session).expect("a free session must be claimable");
            registry.clone().resume(
                runs::RunRecord {
                    codex: None,
                    codex_account: None,
                    hold: None,
                    run_id: format!("run-{session}"),
                    source,
                    session_id: session.to_string(),
                    cwd: None,
                    cli_pid: stranger.id(),
                    // A start time no process alive today can have. This is exactly
                    // what the record of a run whose pid has been recycled looks
                    // like, and the only part of it that can tell.
                    started_at: "2026-01-01T00:00:00.000Z".to_string(),
                    pid_token: Some("1.000000".to_string()),
                    resident: false,
                    shim: None,
                    fingerprint: None,
                    turn_at: None,
                    queued: Vec::new(),
                },
                claim,
            );
            assert!(
                registry.run_injection(source, session).is_some_and(|run| run.state == InjectionRunState::Running),
                "{source}: the run is in flight before Stop"
            );

            assert!(
                !providers.interrupt(source, session),
                "{source}: a pid that fails the identity guard has nothing to interrupt"
            );
        }

        // The assertion the whole fix exists for.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            stranger.try_wait().expect("try_wait").is_none(),
            "a SIGTERM reached a process this server never spawned"
        );

        // And the user is not left with a run that says it is still going and a
        // Stop button that will never end it.
        for (source, session) in [(Source::Claude, CLAUDE), (Source::Codex, CODEX)] {
            let seen = registry.run_injection(source, session).expect("the run is still reported");
            assert_eq!(seen.state, InjectionRunState::Ok, "{source}: a run whose pid is gone has ended, not failed");
            assert!(!seen.interrupted, "{source}: nothing was interrupted, so nothing may claim to have been");
            assert!(seen.finished_at.0.is_some(), "{source}: and it has an end time");
        }

        let _ = stranger.kill();
        let _ = stranger.wait();
    }

    /// A codex session whose app server is gone with messages still queued
    /// keeps its handle in the list, and Stop on it stops nothing: the turn was
    /// closed when the process went. A live adopted run is put under the same
    /// session, the one the fallback would signal, so the test fails if Stop
    /// ever gets past the handle.
    #[tokio::test]
    async fn stop_on_a_codex_session_whose_app_server_is_gone_reaches_no_run() {
        const SESSION: &str = "0199c1a1-6666-7666-8666-dddddddddddd";

        let home = tempfile::tempdir().expect("tempdir");
        let paths = Arc::new(Paths::new(home.path().to_path_buf()));
        let dirs = InstanceDirs::new(&paths.data_dir(), droppen_instance::instance::DEFAULT_INSTANCE).expect("valid name");
        dirs.create().expect("instance dirs");
        let decoration = crate::drive::NotifyDecoration::new(paths.clone(), dirs.clone());
        let registry = InjectionRegistry::new(
            paths.clone(),
            &dirs,
            Arc::new(crate::accounts::store::AccountsStore::new(paths.clone(), crate::app::logins())),
            Arc::new(crate::accounts::quota::QuotaCache::new(paths, crate::app::logins())),
            decoration,
        );
        let providers = ProviderRegistry::new(registry.clone(), crate::app::register_providers);

        let mut live = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 30"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn a stand-in");
        let adopted = runs::RunRecord {
            codex: None,
            codex_account: None,
            hold: None,
            run_id: "run-adopted".to_string(),
            source: Source::Codex,
            session_id: SESSION.to_string(),
            cwd: None,
            cli_pid: live.id(),
            started_at: "2026-01-01T00:00:00.000Z".to_string(),
            pid_token: procs::start_token(live.id()),
            resident: false,
            shim: None,
            fingerprint: None,
            turn_at: None,
            queued: Vec::new(),
        };

        let queued: app_server::State =
            serde_json::from_value(serde_json::json!({"queue": [{"id": "q1", "text": "next", "params": {}, "request": null}]}))
                .expect("a codex state");
        registry.codex_dead(runs::RunRecord {
            codex: Some(queued),
            run_id: "run-app-server".to_string(),
            resident: true,
            ..adopted.clone()
        });
        let handle = registry.codex_handle(SESSION).expect("the handle stays");
        assert!(handle.visible() && !handle.alive(), "listed, with no process behind it");

        let claim = registry.spawn_gate.lease(Source::Codex, SESSION).expect("a free session must be claimable");
        registry.clone().resume(adopted, claim);

        assert!(!providers.interrupt(Source::Codex, SESSION), "nothing is left to stop");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(live.try_wait().expect("try_wait").is_none(), "Stop got past the handle and signalled the adopted run");
        let seen = registry.run_injection(Source::Codex, SESSION).expect("the adopted run");
        assert_eq!(seen.state, InjectionRunState::Running);
        assert!(!seen.interrupted);

        let _ = live.kill();
        let _ = live.wait();
    }

    /// Adoption claims each session it takes back without the spawn gate: it
    /// runs before the server listens, so no send can race it. A run whose CLI
    /// is gone is swept the same way, its claim taken and let go at once, and
    /// its lock file goes with it.
    #[tokio::test]
    async fn adoption_claims_a_live_run_and_sweeps_a_dead_one_without_the_gate() {
        const LIVE: &str = "ffffffff-6666-4666-8666-ffffffffffff";
        const DEAD: &str = "dddddddd-7777-4777-8777-dddddddddddd";

        let home = tempfile::tempdir().expect("tempdir");
        let paths = Arc::new(Paths::new(home.path().to_path_buf()));
        let locks = paths.data_dir().join("inject-locks");
        let debris = locks.join(droppen_sessions::reference::lock_name(Source::Claude.as_str(), DEAD));
        std::fs::create_dir_all(&locks).expect("inject-locks");
        std::fs::write(&debris, b"1\n").expect("the dead run's lock file");
        let dirs = InstanceDirs::new(&paths.data_dir(), droppen_instance::instance::DEFAULT_INSTANCE).expect("valid name");
        dirs.create().expect("instance dirs");
        let decoration = crate::drive::NotifyDecoration::new(paths.clone(), dirs.clone());
        let registry = InjectionRegistry::new(
            paths.clone(),
            &dirs,
            Arc::new(crate::accounts::store::AccountsStore::new(paths.clone(), crate::app::logins())),
            Arc::new(crate::accounts::quota::QuotaCache::new(paths, crate::app::logins())),
            decoration,
        );
        let spawn = |script: &str| {
            std::process::Command::new("/bin/sh")
                .args(["-c", script])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn a stand-in")
        };
        let mut live = spawn("sleep 30");
        let mut dead = spawn("exit 0");
        dead.wait().expect("it exits");
        let record = |session: &str, pid: u32| runs::RunRecord {
            codex: None,
            codex_account: None,
            hold: None,
            run_id: format!("run-{session}"),
            source: Source::Claude,
            session_id: session.to_string(),
            cwd: None,
            cli_pid: pid,
            started_at: "2026-01-01T00:00:00.000Z".to_string(),
            pid_token: procs::start_token(pid),
            resident: false,
            shim: None,
            fingerprint: None,
            turn_at: None,
            queued: Vec::new(),
        };
        registry.journal.add(record(LIVE, live.id()));
        registry.journal.add(record(DEAD, dead.id()));

        let adopted = registry.adopt_all().await;
        assert_eq!((adopted.recovered, adopted.finished), (1, 1), "{adopted:?}");
        let step = |what, claim| Step { what, gate: false, claim: Some(claim) };
        assert_eq!(registry.steps(), [step("adoption claimed", true), step("buried", false)]);
        assert!(!debris.exists(), "the sweep took the dead run's lock file");
        assert!(registry.claim_held(Source::Claude, LIVE), "the watcher holds the claim");

        let _ = live.kill();
        let _ = live.wait();
    }
}
