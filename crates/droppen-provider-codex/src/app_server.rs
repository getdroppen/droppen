//! A Codex App Server per session, connected over the restart-safe session shim.
//!
//! Only explicit turn/completed notifications end a turn. Steer is acknowledged
//! by turn/steer, never implemented by killing the CLI. The journal checkpoints
//! the output cursor and outstanding request IDs together; replay can therefore
//! finish an acknowledged delivery without ever submitting it twice.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(test)]
use tokio::io::AsyncBufReadExt;
use tokio::sync::{mpsc, oneshot};

use droppen_accounts::store::AccountProvider;
use droppen_sessions::{Injection, InjectionRunState, Source};
use droppen_base::process_env::{apply_env, clean_env, clean_env_bare};
use droppen_base::procs;
use droppen_base::time::iso8601;

use droppen_provider::common::{busy_message, new_uuid, session_workdir, Phase, QueueOutcome, ResidentState, CHAT_SYSTEM_PROMPT};
use droppen_provider::gate::{ClaimError, SessionLease, SpawnGate};
use droppen_sessions::archive::ArchiveError;
use droppen_provider::{
    attachment_suffix, require_provider, runs, shim, AccountChoice, Capabilities, DriverStatesBuilder, InjectError, InjectionRegistry,
    Provider, ProviderRuns, Run, RunHost, SendOptions, SendTo, Sent, SpawnPermit, StartAt, WrittenAt,
};

mod driver;
mod protocol;
#[cfg(test)]
mod tests;

use driver::Driver;
use protocol::{
    message, reject_request, rpc, write, ApprovalPolicy, ClientInfo, InitializeParams, NoParams, Notification, Request,
    StoredParams, ThreadArchiveParams, ThreadParams, ThreadSettings, TurnInterruptParams, TurnStartOnThread,
    TurnSteerParams,
};

/// codex has no flag for non-image files, so point it at them in the prompt text.
fn with_codex_files(text: &str, options: &SendOptions) -> String {
    if options.files.is_empty() {
        text.to_string()
    } else {
        format!("{text}{}", attachment_suffix(&options.files))
    }
}

/// One codex account, resolved and ready to be spawned under.
#[derive(Debug, Clone)]
pub(crate) struct CodexAccount {
    /// Which account it is, so a live process can be compared against it.
    id: String,
    /// What it contributes to the spawn.
    launch: droppen_accounts::store::AccountLaunch,
}

const RPC_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a retiring app-server is given to actually exit.
///
/// Generous, because the only alternative to waiting is refusing the send: a
/// replacement started beside a process that still holds the thread's writer
/// lock is two CLIs on one transcript, billed to two different accounts.
const RETIRE_EXIT_WAIT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct State {
    cursor: u64,
    active: Option<String>,
    completed_turn: Option<String>,
    /// The message whose `turn/start` started the turn running, when this
    /// driver sent it: what a Stop that names that message stops. Kept out of
    /// the journal: a Stop dies with the process it was sent to, so a process
    /// after a restart never has one to match.
    #[serde(skip)]
    active_message: Option<String>,
    queue: Vec<Message>,
    settings: Value,
    paused: bool,
    injection: Option<Injection>,
    interrupt_requested: bool,
    // Persist before writing, so restart replay can reconcile a late archive
    // acknowledgement without resending it or starting another turn.
    archive_request: Option<String>,
    archived: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Message {
    id: String,
    text: String,
    params: Value,
    // Persisted BEFORE writing. An unanswered request is never blindly retried.
    request: Option<String>,
    /// The old process died without acknowledging this request. Only an
    /// explicit withdrawal is safe; automatically replaying may duplicate work.
    #[serde(default)]
    uncertain: bool,
}

#[derive(Clone)]
pub(crate) struct Handle {
    tx: mpsc::UnboundedSender<Cmd>,
    shared: Arc<Mutex<State>>,
    /// The account this process was started under, or `None` for the machine's
    /// own codex login.
    ///
    /// The codex answer to the claude resident's fingerprint: a live app-server
    /// authenticates as whoever it was spawned as, and there is no way to hand
    /// it another key over the socket. So the account is *part of the process*,
    /// and a message for a different one cannot be delivered into it — see
    /// [`CodexRuns::codex_deliver`].
    account: Option<String>,
    /// Set while the app server is still starting, to the message it is
    /// starting for: the session is reserved and messages queue in the
    /// channel for the driver to take, but nothing has been asked of codex
    /// yet ([`Starting`]). Cleared by the driver once it has that message in
    /// its queue, where a Stop finds it from then on.
    starting: Arc<Mutex<Option<String>>>,
}

impl Handle {
    pub(crate) fn alive(&self) -> bool {
        !self.tx.is_closed()
    }

    /// The handle of an app server about to start under `account` for
    /// `message`, with `state` on view meanwhile, and the receiver its driver
    /// takes: marked starting for the message, which is in the channel before
    /// anybody can hold the handle, so a Stop sent through it always finds the
    /// message ahead of it.
    fn starting_for(
        state: State,
        account: Option<String>,
        message: Message,
    ) -> Result<(Handle, mpsc::UnboundedReceiver<Cmd>, Delivery), InjectError> {
        let (tx, rx) = mpsc::unbounded_channel();
        let handle = Handle {
            tx,
            shared: Arc::new(Mutex::new(state)),
            account,
            starting: Arc::new(Mutex::new(Some(message.id.clone()))),
        };
        let delivery = handle.deliver(message)?;
        Ok((handle, rx, delivery))
    }

    /// Whether this session's app server is still starting.
    fn starting(&self) -> bool {
        self.starting.lock().unwrap().is_some()
    }

    /// Hand `message` to the driver: what it will answer is the [`Delivery`].
    fn deliver(&self, message: Message) -> Result<Delivery, InjectError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Deliver(message, reply))
            .map_err(|_| InjectError::message("Codex App Server disconnected"))?;
        Ok(Delivery(rx))
    }

    /// Whether this process is already running under the account a send wants.
    fn runs_as(&self, account: Option<&str>) -> bool {
        self.account.as_deref() == account
    }

    /// Whether anything is in flight that a shutdown would cut short; a start
    /// still under way is.
    fn busy(&self) -> bool {
        let state = self.shared.lock().unwrap();
        self.starting() || state.active.is_some() || !state.queue.is_empty() || state.archive_request.is_some()
    }
    pub(crate) fn visible(&self) -> bool {
        let state = self.shared.lock().unwrap();
        !state.archived && (self.alive() || !state.queue.is_empty())
    }
    pub(crate) fn injection(&self) -> Option<Injection> {
        self.shared.lock().unwrap().injection.clone()
    }
    pub(crate) fn state(&self) -> ResidentState {
        let state = self.shared.lock().unwrap();
        ResidentState {
            phase: if self.starting() {
                Phase::Starting
            } else if state.active.is_some() || state.queue.iter().any(|q| q.request.is_some()) {
                Phase::Working
            } else {
                Phase::Idle
            },
            stalled: state.paused,
            queued: state.queue.len(),
            queued_messages: state
                .queue
                .iter()
                .map(|q| droppen_sessions::QueuedMessage {
                    id: q.id.clone(),
                    text: q.text.chars().take(200).collect(),
                    steered: q.request.is_some(),
                })
                .collect(),
            background: 0,
            tasks: vec![],
            awaiting_background: false,
            // Codex's app-server driver keeps no turn clock of its own.
            turn_started_at: None,
            // Approvals are off, and every interactive request the app server
            // makes is refused: codex never stops to ask.
            question: None,
            permission: None,
            // Codex states each turn's window itself.
            model: None,
        }
    }
    pub(crate) fn info(&self) -> droppen_sessions::ResidentInfo {
        self.state().into()
    }
    /// Stop a turn by name ([`Stop`]): while the app server starts, the turn
    /// of the message it starts for; otherwise the turn running, the one a
    /// request in flight is starting, or the one the head of a queue that is
    /// not paused starts. The driver stops that turn and no other, so a Stop
    /// it reads after that turn is over stops nothing. Sent while the message is on its way, the Stop
    /// waits behind it in the channel, and the driver interrupts the turn once
    /// codex says it started (`interrupt_requested`).
    pub(crate) fn interrupt(&self) -> bool {
        let starting_for = self.starting.lock().unwrap().clone();
        let state = self.shared.lock().unwrap();
        // The start's message first: until the driver has taken it, what the
        // attach copied in (a turn the resume reports in progress, messages
        // left queued) is not what the start is for.
        let stop = if let Some(message) = starting_for {
            Stop::Message(message)
        } else if let Some(turn) = &state.active {
            Stop::Turn(turn.clone())
        } else if let Some(message) = state.queue.iter().find(|q| q.request.is_some()) {
            Stop::Message(message.id.clone())
        } else if let Some(message) = state.queue.first().filter(|_| !state.paused) {
            Stop::Message(message.id.clone())
        } else {
            return false;
        };
        self.tx.send(Cmd::Interrupt(stop)).is_ok()
    }
    pub(crate) async fn queue_action(&self, id: &str, steer: bool) -> QueueOutcome {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(Cmd::Queue { id: id.into(), steer, reply }).is_err() {
            return QueueOutcome::Unreachable;
        }
        rx.await.unwrap_or(QueueOutcome::Unreachable)
    }
}

enum Cmd {
    Deliver(Message, oneshot::Sender<Result<Sent, InjectError>>),
    /// End this process cleanly so a replacement can run under another account.
    Retire(oneshot::Sender<Result<(), InjectError>>),
    Queue {
        id: String,
        steer: bool,
        reply: oneshot::Sender<QueueOutcome>,
    },
    Interrupt(Stop),
    Archive(oneshot::Sender<Result<(), InjectError>>),
}

/// What a Stop names ([`Handle::interrupt`]): the turn it is for, so that
/// one read after that turn is over stops no other.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Stop {
    /// The codex turn by this id, running when the Stop was sent.
    Turn(String),
    /// The turn this message starts: its `turn/start` was in flight when the
    /// Stop was sent, the app server was starting for it, or it was next to
    /// start.
    Message(String),
}

/// What a Codex run keeps in its journal row, beside what every run says
/// ([`runs::RunRecord<CodexRun>`]): written after those fields, in this order.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct CodexRun {
    /// Three keys every row carries and only a Claude Code resident reads: a
    /// Codex run's are `null`, `null` and `[]`, kept as they were read so a
    /// Codex row is written back unchanged.
    #[serde(default)]
    fingerprint: Option<Value>,
    #[serde(default)]
    turn_at: Option<Value>,
    #[serde(default)]
    queued: Vec<Value>,
    /// Codex App Server delivery state and replay checkpoint. Absent for a
    /// legacy exec run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) codex: Option<State>,
    /// The stored account a codex app-server was started under.
    ///
    /// The codex half of a Claude Code run's fingerprint, and a field of its own
    /// rather than a reuse of it because the two are read by different drivers
    /// and the claude one carries six settings this has no opinion about. An id
    /// and never a secret. `None` is the machine's own `~/.codex` login, which
    /// is what every record written before accounts had a second CLI means.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) codex_account: Option<String>,
}

/// Codex, as the provider registry knows it.
pub struct Codex {
    /// Its sessions' app servers.
    runs: Arc<CodexRuns>,
    /// Its CLI's models.
    models: crate::models::CodexModels,
}

impl Codex {
    /// Codex, its runs on `registry`'s host.
    pub fn new(registry: Arc<InjectionRegistry>) -> Self {
        let models = crate::models::CodexModels::new(registry.host.paths.clone());
        Self {
            runs: CodexRuns::new(registry.host.clone()),
            models,
        }
    }

    /// Its runs, for a test that checks whose they are.
    #[cfg(test)]
    pub(crate) fn runs(&self) -> &Arc<CodexRuns> {
        &self.runs
    }
}

/// Codex's runs: the app server behind each session. Kept here rather than
/// on the host they run on, whose shared parts — the paths, the runs
/// journal, the feeds — they reach through it.
pub(crate) struct CodexRuns {
    /// What they run on.
    host: Arc<RunHost>,
    /// The app server behind each session, by session id.
    codex_servers: Mutex<HashMap<String, Handle>>,
}

impl CodexRuns {
    /// None yet, on `host`.
    pub(crate) fn new(host: Arc<RunHost>) -> Arc<Self> {
        let runs = Arc::new(Self {
            host,
            codex_servers: Mutex::new(HashMap::new()),
        });
        runs
    }
}

impl ProviderRuns for CodexRuns {
    fn source(&self) -> Source {
        Source::Codex
    }

    fn host(&self) -> &Arc<RunHost> {
        &self.host
    }

    fn add_to(&self, states: &mut DriverStatesBuilder) {
        self.codex_driver_states(states);
    }

    fn cli_name(&self) -> &'static str {
        "codex"
    }

    fn readable(&self, record: &runs::RunRecord) -> bool {
        record.clone().decode::<CodexRun>().is_ok()
    }

    /// An app server's: its queue and replay checkpoint.
    fn holds_state(&self, record: &runs::RunRecord) -> bool {
        record.clone().decode::<CodexRun>().is_ok_and(|record| record.payload.codex.is_some())
    }

    fn dead(&self, record: runs::RunRecord) {
        if let Ok(record) = record.decode::<CodexRun>() {
            self.codex_dead(record);
        }
    }

    fn attach(self: Arc<Self>, record: runs::RunRecord, _sidecars: shim::Sidecars, link: shim::Link, lease: SessionLease) {
        match record.decode::<CodexRun>() {
            Ok(record) => {
                self.codex_attached(record, link, lease, None);
            }
            // Only a record it could read is handed over (`readable`).
            Err(error) => tracing::warn!(%error, "a run record cannot be read; that run will not be adopted"),
        }
    }

    fn was_cut_short<'a>(&'a self, record: &'a runs::RunRecord) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            match crate::sessions::codex::find_codex_session(&self.host.paths, &record.session_id).await {
                None => true,
                // Codex reads `Idle` for a transcript nothing has touched
                // lately, which after a long outage is every transcript. That
                // makes the answer conservative rather than wrong: silence,
                // where claude would have said something.
                Some(session) => session.status == droppen_sessions::SessionStatus::Running,
            }
        })
    }

    fn last_write<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Option<Instant>> {
        Box::pin(async move {
            let session = crate::sessions::codex::find_codex_session(&self.host.paths, id).await?;
            // The row names the plain rollout; on disk it may be compressed.
            let (file, _) = droppen_sessions::compressed::locate(std::path::Path::new(&session.file_path))
                .await
                .ok()?;
            file.written_at().await
        })
    }
}

/// A Codex row put in a journal as Codex reads and writes it, for the
/// server's tests of each CLI's rows side by side. The row's type stays this
/// crate's own: outside it, a test hands the row over as text.
#[cfg(any(test, feature = "test-support"))]
pub trait CodexRowsExt {
    /// Add `row`, read as a Codex run. Panics if it is not one.
    fn add_codex_row(&self, row: &str);
}

#[cfg(any(test, feature = "test-support"))]
impl CodexRowsExt for runs::RunsFile {
    fn add_codex_row(&self, row: &str) {
        self.add(serde_json::from_str::<runs::RunRecord<CodexRun>>(row).expect("a Codex row"));
    }
}

impl Provider for Codex {
    fn source(&self) -> Source {
        Source::Codex
    }

    /// Approvals are off and every request the app server makes is refused:
    /// codex never stops to ask.
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            questions: false,
            permissions: false,
            chats: true,
            chat_attachments: true,
            chat_inline_attachments: false,
            schedules: true,
        }
    }

    fn run(&self) -> &dyn Run {
        self
    }

    fn runs(&self) -> Arc<dyn ProviderRuns> {
        self.runs.clone()
    }

    // A thread switches accounts in place: its login is a binding, not a root.
    fn continue_on_account<'a>(&'a self, _id: &'a str, _new_id: &'a str) -> BoxFuture<'a, Result<std::path::PathBuf, droppen_provider::ContinueError>> {
        Box::pin(std::future::ready(Err(droppen_provider::ContinueError::Unsupported)))
    }

    fn models(&self) -> &dyn droppen_provider::models::Models {
        &self.models
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl Run for Codex {
    fn send<'a>(&'a self, gate: &'a SpawnGate, to: SendTo<'a>) -> BoxFuture<'a, Result<Sent, InjectError>> {
        Box::pin(async move {
            let SendTo { id, cwd, text, options } = to;
            // Resolved before anything is built, exactly as claude's send does: an
            // account that cannot be read must not reach a spawn.
            let account = self.codex_account(Some(id), options).await?;
            self
                .runs
                .codex_deliver(gate, Some(id), cwd, text, options, account)
                .await
                .map(|(_, sent)| sent)
        })
    }

    fn start<'a>(&'a self, gate: &'a SpawnGate, at: StartAt<'a>) -> BoxFuture<'a, Result<String, InjectError>> {
        Box::pin(async move {
            let StartAt { cwd, text, options } = at;
            let account = self.codex_account(None, options).await?;
            let account_id = account.as_ref().map(|account| account.id.clone());
            let (id, _) = self.runs.codex_deliver(gate, None, Some(cwd), text, options, account).await?;
            // Codex has one config root and no way to isolate it, so ownership of a
            // thread is a record rather than a path — written the moment the CLI tells us
            // which thread it actually made. A failure here costs the session its
            // ownership (it lists as external) and nothing else, so it is logged rather
            // than turned into a refusal of a run that has already started.
            if let Err(error) = crate::sessions::codex::CodexSessions.record_owned(&self.runs.host.paths, &id).await {
                tracing::error!(session = %id, %error, "cannot record this codex session as ours; it will list as external");
            }
            // Written beside the ownership record and for the same reason: the thread id
            // does not exist until `thread/start` answers, and the next message has to
            // find the same login this one ran under.
            if let Some(account_id) = account_id {
                if let Err(error) = self.runs.host.accounts.bind_session_account(&id, Some(&account_id), AccountProvider::Codex).await {
                    tracing::error!(session = %id, %error, "cannot bind this codex session to its account; the next message will fall back to the default");
                }
            }
            Ok(id)
        })
    }

    fn injection(&self, id: &str) -> Option<Injection> {
        self.runs.codex_injection(id)
    }

    fn resident(&self, id: &str) -> Option<ResidentState> {
        self.runs.codex_handle(id).filter(|h| h.visible()).map(|h| h.state())
    }

    /// The schedule waits for the run to finish: whether it is still running
    /// is the whole question asked of codex.
    fn schedule_busy(&self, id: &str) -> Option<&'static str> {
        self.injection(id)
            .is_some_and(|i| i.state == InjectionRunState::Running)
            .then_some("the previous run has not finished")
    }

    fn drop_queued<'a>(&'a self, id: &'a str, qid: &'a str) -> BoxFuture<'a, QueueOutcome> {
        Box::pin(self.runs.codex_queue_action(id, qid, false))
    }

    fn steer_queued<'a>(&'a self, id: &'a str, qid: &'a str) -> BoxFuture<'a, QueueOutcome> {
        Box::pin(self.runs.codex_queue_action(id, qid, true))
    }

    /// The app server, when one is running; nothing, when the list still shows
    /// a handle whose process is gone, because `codex_dead` already closed that
    /// turn as failed and removed its runs entry, so nothing is left to stop;
    /// otherwise a `codex exec` run adopted from the previous server.
    fn interrupt(&self, id: &str) -> bool {
        if let Some(handle) = self.runs.codex_handle(id).filter(Handle::alive) {
            return handle.interrupt();
        }
        if self.runs.codex_handle(id).is_some_and(|handle| handle.visible()) {
            return false;
        }
        self.runs.host.interrupt_legacy(&*self.runs, id)
    }

    // Through the session's app server, or `codex archive` when it has none:
    // the archive is codex's own, so the Codex app sees it too.
    fn archive<'a>(&'a self, gate: &'a SpawnGate, id: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move { self.runs.archive_codex(gate, id).await.map_err(|error| error.to_string()) })
    }
}

impl Codex {
    /// Which account a codex spawn runs under, and what it puts into it.
    ///
    /// The same three-state resolution `claude_code::resident::ClaudeCodeRuns::resume_claude_env` does, and for the same
    /// reason: `accountId: null` is "the machine's own codex login" even when a
    /// default is configured, absent is "whatever this thread is already bound to",
    /// and a named account both wins and is written down.
    ///
    /// `None` is the answer for a machine with no codex accounts, and it is the
    /// answer that matters most: it must produce the bare spawn — no extra argv,
    /// no extra environment, codex's own `~/.codex/auth.json` and nothing else.
    async fn codex_account(&self, id: Option<&str>, options: &SendOptions) -> Result<Option<CodexAccount>, InjectError> {
        let bound = match id {
            Some(id) => self.runs.host.accounts.session_account_id(id).await,
            None => None,
        };
        let chosen = match &options.account {
            AccountChoice::Local => None,
            AccountChoice::Account(id) => Some(id.clone()),
            AccountChoice::Rule(named) => self.runs.host.rule_account(AccountProvider::Codex, named, options.model.as_deref()).await,
            AccountChoice::Unset => match bound.clone() {
                Some(bound) => Some(bound),
                // The configured default, or — with none, and `~/.codex` known to
                // be signed out — a codex account in the pool, if one is free.
                None if options.use_default_account => self.runs.host.start_account(AccountProvider::Codex, options.model.as_deref()).await,
                None => None,
            },
        };
        let Some(account_id) = chosen else { return Ok(None) };
        let launch = self.runs.host.accounts.account_env(&account_id).await?;
        require_provider(&launch, AccountProvider::Codex)?;
        // An explicit choice on a thread that already exists is also a re-binding:
        // the next message must find the account this one asked for.
        if let (Some(id), Some(named)) = (id, options.account.named()) {
            if Some(named) != bound.as_deref() {
                self.runs.host.accounts.bind_session_account(id, Some(named), AccountProvider::Codex).await?;
            }
        }
        Ok(Some(CodexAccount { id: account_id, launch }))
    }
}

impl CodexRuns {
    /// The state of a codex session's run: its app server's, whenever there is
    /// a handle (even one that has nothing to say yet), and otherwise the runs
    /// map's, where a `codex exec` run adopted from the previous server is.
    pub(crate) fn codex_injection(&self, id: &str) -> Option<Injection> {
        if let Some(handle) = self.codex_handle(id) {
            return handle.injection();
        }
        self.host.run_injection(Source::Codex, id)
    }

    /// Codex's part of the session list's snapshot ([`ProviderRuns::add_to`]): the app servers
    /// the list is to show, and the state of every app server's run.
    pub(crate) fn codex_driver_states(&self, states: &mut DriverStatesBuilder) {
        for (id, handle) in self.codex_servers.lock().unwrap().iter().filter(|(_, h)| h.visible()) {
            states.resident(self.host.key_of(Source::Codex, id), handle.info());
        }
        for (id, handle) in self.codex_servers.lock().unwrap().iter() {
            if let Some(injection) = handle.injection() {
                states.run(self.host.key_of(Source::Codex, id), injection);
            }
        }
    }

    /// A dead CLI must not take already-accepted, undelivered messages with it.
    pub(crate) fn codex_dead(&self, mut record: runs::RunRecord<CodexRun>) {
        let mut state = record.payload.codex.clone().unwrap_or_default();
        let interrupted = state.interrupt_requested || state.injection.as_ref().is_some_and(|i| i.interrupted);
        let unfinished = state.active.take().is_some() || state.queue.iter().any(|q| q.request.is_some());
        state.active_message = None;
        for message in &mut state.queue {
            if message.request.take().is_some() {
                message.uncertain = true;
            }
        }
        state.interrupt_requested = false;
        state.paused = true;
        if unfinished {
            state.injection = Some(Injection {
                state: InjectionRunState::Failed,
                error: droppen_http::openapi_ext::Nullable(Some(
                    "Codex App Server exited. Queued messages are retained; inspect the transcript before resending any unacknowledged delivery."
                        .into(),
                )),
                interrupted,
                started_at: record.started_at.clone(),
                finished_at: droppen_http::openapi_ext::Nullable(Some(iso8601(SystemTime::now()))),
            });
        }
        record.payload.codex = Some(state.clone());
        let id = record.session_id.clone();
        let account = record.payload.codex_account.clone();
        self.host.withdraw_run(Source::Codex, &id);
        if state.archived {
            self.host.journal.remove(&record.run_id);
        } else {
            self.host.journal.add(record);
        }
        let (tx, rx) = mpsc::unbounded_channel();
        drop(rx);
        self.codex_servers.lock().unwrap().insert(
            id,
            Handle {
                tx,
                shared: Arc::new(Mutex::new(state)),
                account,
                starting: Default::default(),
            },
        );
    }

    pub(crate) async fn codex_queue_action(&self, id: &str, qid: &str, steer: bool) -> QueueOutcome {
        let Some(handle) = self.codex_handle(id) else {
            return QueueOutcome::NoResident;
        };
        // Refused at once rather than waited for: its driver takes nothing off
        // the queue until the app server has answered.
        if handle.starting() {
            return QueueOutcome::Refused {
                reason: "Codex is still starting this session; try again once it has answered".into(),
            };
        }
        if handle.alive() {
            return handle.queue_action(qid, steer).await;
        }
        if steer {
            return QueueOutcome::Unreachable;
        }
        let Some(mut record) = self.app_server_record(id) else {
            return QueueOutcome::NoResident;
        };
        {
            let mut state = handle.shared.lock().unwrap();
            let Some(index) = state.queue.iter().position(|m| m.id == qid) else {
                return QueueOutcome::Unknown;
            };
            state.queue.remove(index);
            record.payload.codex = Some(state.clone());
        }
        // Persist to SQLite outside the in-memory `handle.shared` lock.
        self.host.journal.add(record);
        self.host.note_receipt(Source::Codex, id, qid, droppen_sessions::delivery::ReceiptStatus::Failed);
        QueueOutcome::Done
    }

    /// The row of session `id`'s app server, as Codex reads it.
    fn app_server_record(&self, id: &str) -> Option<runs::RunRecord<CodexRun>> {
        self.host.journal
            .read()
            .into_iter()
            .filter(|r| r.source == Source::Codex && r.session_id == id)
            .filter_map(|r| r.decode::<CodexRun>().ok())
            .find(|r| r.payload.codex.is_some())
    }

    pub(crate) fn codex_handle(&self, id: &str) -> Option<Handle> {
        self.codex_servers.lock().unwrap().get(id).cloned()
    }

    /// A loaded thread's writer owns its archive operation too. A separate
    /// `codex archive` cannot acquire that writer lock, even between turns.
    pub(crate) async fn archive_codex(self: &Arc<Self>, gate: &SpawnGate, id: &str) -> Result<(), droppen_sessions::archive::ArchiveError> {
        // Match delivery's gate: no new resident or accepted message can slip
        // between selecting the archive path and committing its result.
        let mut permit = gate.permit().await;
        if let Some(handle) = self.codex_handle(id) {
            // Refused at once rather than waited for: the start goes on outside
            // the gate, and waiting for it here would hold the gate as long.
            if handle.starting() {
                return Err(ArchiveError("Codex is still starting this session; archive it once it has answered".into()));
            }
            {
                let state = handle.shared.lock().unwrap();
                if state.archived {
                    return Ok(());
                }
                if !state.queue.is_empty() {
                    return Err(ArchiveError("Codex has queued messages; send or remove them before archiving".into()));
                }
            }
            if handle.alive() {
                let (reply, rx) = oneshot::channel();
                handle
                    .tx
                    .send(Cmd::Archive(reply))
                    .map_err(|_| ArchiveError("Codex App Server disconnected".into()))?;
                // The live driver owns the session now; release the global
                // SpawnGate before waiting up to RPC_TIMEOUT on the reply.
                drop(permit);
                return rx
                    .await
                    .map_err(|_| ArchiveError("Codex App Server disconnected before confirming archive".into()))?
                    .map_err(|error| ArchiveError(error.to_string()));
            }
        }
        // Includes legacy or unreachable residents and other server instances.
        permit
            .claim(Source::Codex, id)
            .map_err(|error| ArchiveError(claim_refusal(error).to_string()))?;
        // Reserve the session slot while the gate is still held so concurrent
        // `codex_deliver` calls see `archive_request` immediately rather than
        // starting a new handle after we release the global gate.
        let (tx, rx) = mpsc::unbounded_channel();
        drop(rx);
        let placeholder = Handle {
            tx,
            shared: Arc::new(Mutex::new(State {
                archive_request: Some("offline-archive".into()),
                ..Default::default()
            })),
            account: None,
            starting: Default::default(),
        };
        let previous = self.codex_servers.lock().unwrap().insert(id.into(), placeholder.clone());
        struct ArchiveGuard<'a> {
            runs: &'a CodexRuns,
            id: String,
            previous: Option<Handle>,
            committed: bool,
        }
        impl Drop for ArchiveGuard<'_> {
            fn drop(&mut self) {
                if !self.committed {
                    let mut servers = self.runs.codex_servers.lock().unwrap();
                    match self.previous.take() {
                        Some(prev) => {
                            servers.insert(self.id.clone(), prev);
                        }
                        None => {
                            servers.remove(&self.id);
                        }
                    }
                }
            }
        }
        let mut guard = ArchiveGuard {
            runs: self,
            id: id.into(),
            previous,
            committed: false,
        };
        // Release the global SpawnGate while keeping the per-session claim for
        // the `codex archive` subprocess.
        permit.ungate();
        #[cfg(test)]
        self.host.step("codex archive", Source::Codex, Some(id));
        archive_codex_session(&permit, &self.host.paths, id).await?;
        {
            let mut state = placeholder.shared.lock().unwrap();
            state.archive_request = None;
            state.archived = true;
        }
        guard.committed = true;
        for record in self
            .host.journal
            .read()
            .into_iter()
            .filter(|r| r.source == Source::Codex && r.session_id == id)
        {
            self.host.journal.remove(&record.run_id);
        }
        Ok(())
    }

    pub(crate) async fn codex_deliver(
        self: &Arc<Self>,
        gate: &SpawnGate,
        id: Option<&str>,
        cwd: Option<&str>,
        text: &str,
        options: &SendOptions,
        account: Option<CodexAccount>,
    ) -> Result<(String, Sent), InjectError> {
        let message = message(text, options)?;
        // A new thread: nobody can name it before its app server does, so no
        // other start can be for it, and the gate it takes its permit from is
        // given up at once rather than held while it starts. Its id is claimed
        // once codex has picked it.
        let Some(id) = id else {
            let mut permit = gate.permit().await;
            permit.ungate();
            let (id, handle) = self.codex_spawn(&mut permit, None, cwd, options, account, None).await?;
            let sent = handle.deliver(message)?.taken().await?;
            return Ok((id, sent));
        };
        // The gate is held to decide where the message goes and to reserve the
        // session for it, and not while an app server starts, an old one shuts
        // down or the driver takes the message: those take minutes at worst,
        // and every other send would wait them out.
        let mut permit = gate.permit().await;
        if let Some(handle) = self.codex_handle(id) {
            let state = handle.shared.lock().unwrap();
            if state.archived || state.archive_request.is_some() {
                return Err(InjectError::message(
                    "Codex session is archived or awaiting archive confirmation; refresh the session list",
                ));
            }
        }
        let wanted = account.as_ref().map(|account| account.id.as_str());
        let existing = self.codex_handle(id).filter(Handle::alive);
        // Into the session's own app server, running or still starting: its
        // channel keeps the order, so the gate goes before the wait.
        if let Some(handle) = existing.as_ref().filter(|handle| handle.runs_as(wanted)) {
            let delivery = handle.deliver(message)?;
            permit.ungate();
            return Ok((id.to_owned(), delivery.taken().await?));
        }
        // A live app-server holds the credential it was started with, so a
        // message for another account cannot go into it. From idle the process
        // is retired and resumed under the new one; mid-turn it is the same
        // refusal a second message would get, because killing a turn the user is
        // watching is not what changing an account means.
        if existing.as_ref().is_some_and(Handle::busy) {
            return Err(InjectError::message("a message is already running for this session"));
        }
        // Also excludes a legacy codex exec inherited during this upgrade.
        if self.codex_injection(id).is_some_and(|injection| injection.state == InjectionRunState::Running) {
            return Err(InjectError::message("a message is already running for this session"));
        }
        // Reserved, with the message on its way: a send for this session from
        // here on queues behind it instead of starting a second app server.
        let (starting, delivery) = self.codex_starting(id, wanted.map(str::to_owned), message)?;
        permit.ungate();
        if let Some(existing) = existing {
            retire(&existing).await?;
        }
        self.codex_spawn(&mut permit, Some(id), cwd, options, account, Some(starting)).await?;
        let sent = delivery.taken().await?;
        Ok((id.to_owned(), sent))
    }

    /// Reserve session `id` for an app server about to start under `account`
    /// for `message`: its handle in the map, marked starting for that message,
    /// in place of whatever was there (what a dead process left stays on view
    /// meanwhile), with the message on its way ([`Handle::starting_for`]).
    /// The session's feed and the list are told at once, not left to their
    /// next poll. Under the spawn gate.
    fn codex_starting(self: &Arc<Self>, id: &str, account: Option<String>, message: Message) -> Result<(Starting, Delivery), InjectError> {
        let mut servers = self.codex_servers.lock().unwrap();
        let state = servers.get(id).map(|before| before.shared.lock().unwrap().clone()).unwrap_or_default();
        let (handle, rx, delivery) = Handle::starting_for(state, account, message)?;
        let before = servers.insert(id.to_owned(), handle.clone());
        drop(servers);
        self.host.announce(Source::Codex, id);
        #[cfg(test)]
        self.host.step("codex reserved", Source::Codex, Some(id));
        let starting = Starting {
            runs: self.clone(),
            id: id.to_owned(),
            handle,
            rx: Some(rx),
            before,
        };
        Ok((starting, delivery))
    }

    async fn codex_spawn(
        self: &Arc<Self>,
        permit: &mut SpawnPermit<'_>,
        id: Option<&str>,
        cwd: Option<&str>,
        options: &SendOptions,
        account: Option<CodexAccount>,
        starting: Option<Starting>,
    ) -> Result<(String, Handle), InjectError> {
        if let Some(id) = id {
            permit.claim(Source::Codex, id).map_err(claim_refusal)?;
        }
        let sidecars = shim::Sidecars::new(self.host.shim_dir.clone(), new_uuid());
        let cwd = session_workdir(&self.host.paths, cwd);
        // The thread request names the directory as a JSON string, which a
        // path that is not UTF-8 cannot be written as: refuse it before codex
        // is started.
        let Some(thread_cwd) = cwd.to_str() else {
            return Err(InjectError::message("the session's working directory is not valid UTF-8"));
        };
        // With no account, argv and environment are byte for byte what they
        // have always been: `codex app-server`, the process environment, and
        // whatever `~/.codex/auth.json` holds. An account adds a model
        // provider that reads its key from a variable of ours, because
        // `app-server` reads no key variable of codex's own — see
        // [`crate::login::CodexBaseUrl::provider_args`].
        let launch = account.as_ref().map(|account| &account.launch);
        let mut args: Vec<String> = vec!["app-server".into()];
        args.extend(launch.into_iter().flat_map(|launch| launch.codex_args.iter().cloned()));
        let env = match launch {
            Some(launch) => clean_env(launch.env.iter().cloned()),
            None => clean_env_bare(),
        };
        #[cfg(test)]
        droppen_provider::testing::shims_run_as(tests::SHIMS);
        let mut link = shim::spawn_via_shim(shim::Spawn {
            permit,
            sidecars: &sidecars,
            program: "codex",
            args: &args,
            workdir: Some(&cwd),
            env: &env,
            buffer_bytes: None,
        })
        .await
        .map_err(|e| InjectError::message(e.to_string()))?;
        #[cfg(test)]
        self.host.step("codex started", Source::Codex, id);
        let setup = async {
            let client_info = ClientInfo {
                name: "remote_hand",
                title: "remote-hand",
                version: droppen_http::PACKAGE_VERSION,
            };
            rpc(self, &mut link, "initialize", InitializeParams { client_info }).await?;
            write(
                &mut link,
                &Notification {
                    method: "initialized",
                    params: NoParams {},
                },
            )
            .await?;
            let params = ThreadParams {
                cwd: thread_cwd,
                approval_policy: ApprovalPolicy::Never,
                thread_id: id,
                // Belt and braces with `-c model_provider=…` on the command line:
                // the flag is what a `codex exec` probe proves works, and naming it
                // on the thread as well costs nothing if the flag already took.
                model_provider: launch
                    .is_some_and(|launch| !launch.codex_args.is_empty())
                    .then_some(crate::login::CODEX_PROVIDER_ID),
                base_instructions: options.chat.then_some(CHAT_SYSTEM_PROMPT),
            };
            let result = rpc(self, &mut link, if id.is_some() { "thread/resume" } else { "thread/start" }, params).await?;
            let resolved = result["thread"]["id"]
                .as_str()
                .ok_or_else(|| InjectError::message("Codex returned no thread id"))?
                .to_owned();
            if id.is_some_and(|id| id != resolved) {
                return Err(InjectError::message("Codex resumed a different thread"));
            }
            if id.is_none() {
                permit.claim(Source::Codex, &resolved).map_err(claim_refusal)?;
            }
            #[cfg(test)]
            self.host.step("codex thread", Source::Codex, Some(&resolved));
            Ok::<_, InjectError>((resolved, result))
        }
        .await;
        let (id, result) = match setup {
            Ok(setup) => setup,
            Err(error) => {
                // The session goes before the stdin, as it did when the claim
                // was the setup's own.
                permit.unclaim();
                #[cfg(test)]
                self.host.step("codex setup failed", Source::Codex, id);
                let _ = link.close_stdin().await;
                return Err(error);
            }
        };
        let old = self.app_server_record(&id);
        let mut state = old.as_ref().and_then(|r| r.payload.codex.clone()).unwrap_or_default();
        for message in &mut state.queue {
            if message.request.take().is_some() {
                message.uncertain = true;
            }
        }
        state.cursor = link.at();
        state.active = result["thread"]["turns"]
            .as_array()
            .and_then(|turns| turns.iter().rev().find(|t| t["status"] == "inProgress"))
            .and_then(|t| t["id"].as_str())
            .map(str::to_owned);
        state.settings = serde_json::to_value(ThreadSettings {
            model: &result["model"],
            effort: &result["reasoningEffort"],
            sandbox_policy: &result["sandbox"],
        })
        .expect("thread settings encode");
        state.paused = state.queue.iter().any(|m| m.uncertain);
        state.interrupt_requested = false;
        let record = runs::RunRecord {
            run_id: new_uuid(),
            source: Source::Codex,
            session_id: id.clone(),
            cwd: Some(cwd.to_string_lossy().into()),
            cli_pid: link.claude_pid(),
            started_at: iso8601(SystemTime::now()),
            pid_token: procs::start_token(link.claude_pid()),
            // Older binaries must adopt this as an opaque PID, never attach a
            // Claude stream-json driver to the Codex JSON-RPC socket.
            resident: false,
            shim: Some(sidecars.id().into()),
            payload: CodexRun {
                codex: Some(state),
                codex_account: account.as_ref().map(|account| account.id.clone()),
                ..CodexRun::default()
            },
        };
        self.host.journal.add(record.clone());
        if let Some(old) = old {
            self.host.journal.remove(&old.run_id);
        }
        let handle = self.codex_attached(record, link, permit.lease(), starting);
        Ok((id, handle))
    }

    /// Drive `record`'s app server from here: on the channel `starting`
    /// reserved, with what was sent meanwhile waiting in it, or on a new one.
    fn codex_attached(
        self: &Arc<Self>,
        record: runs::RunRecord<CodexRun>,
        link: shim::Link,
        lease: SessionLease,
        starting: Option<Starting>,
    ) -> Handle {
        // A finished legacy exec entry must not overwrite this driver's newer
        // finish time in the list snapshot after the session migrates.
        self.host.withdraw_run(Source::Codex, &record.session_id);
        let state = record.payload.codex.clone().unwrap_or_default();
        let (handle, rx) = match starting {
            Some(mut starting) => {
                let rx = starting.rx.take().expect("a reserved session is attached once");
                *starting.handle.shared.lock().unwrap() = state.clone();
                (starting.handle.clone(), rx)
            }
            None => {
                let (tx, rx) = mpsc::unbounded_channel();
                let handle = Handle {
                    tx,
                    shared: Arc::new(Mutex::new(state.clone())),
                    account: record.payload.codex_account.clone(),
                    starting: Default::default(),
                };
                (handle, rx)
            }
        };
        self.codex_servers.lock().unwrap().insert(record.session_id.clone(), handle.clone());
        #[cfg(test)]
        self.host.step("codex mapped", Source::Codex, Some(&record.session_id));
        let driver = Driver {
            runs: self.clone(),
            record,
            link,
            lease,
            state,
            shared: handle.shared.clone(),
            rx,
            replies: HashMap::new(),
            pending_since: Instant::now(),
            interrupt: None,
            stop_next: None,
            starting: handle.starting.clone(),
            archive_reply: None,
            archive_since: Instant::now(),
            retiring: false,
            retire_wait: RETIRE_EXIT_WAIT,
        };
        tokio::spawn(driver.run());
        handle
    }
}

/// A session reserved under the gate while its app server starts outside it
/// ([`CodexRuns::codex_deliver`]): its handle in the map, marked starting, with
/// what is sent meanwhile queued in its channel for the driver to take.
/// Dropped before it is attached (the start failed), it puts back what the map
/// held for the session before, and the messages queued on it are answered by
/// their channel closing.
struct Starting {
    runs: Arc<CodexRuns>,
    id: String,
    handle: Handle,
    /// The driver's end of the channel, until it is attached.
    rx: Option<mpsc::UnboundedReceiver<Cmd>>,
    before: Option<Handle>,
}

impl Drop for Starting {
    fn drop(&mut self) {
        if self.rx.is_none() {
            return;
        }
        let mut servers = self.runs.codex_servers.lock().unwrap();
        // Only its own entry: an old process reported dead meanwhile is what
        // the session is now.
        if servers.get(&self.id).is_some_and(|handle| Arc::ptr_eq(&handle.shared, &self.handle.shared)) {
            match self.before.take() {
                Some(before) => servers.insert(self.id.clone(), before),
                None => servers.remove(&self.id),
            };
            // Let go before the announcement: readers of it take this lock.
            drop(servers);
            self.runs.host.announce(Source::Codex, &self.id);
        }
    }
}

/// A message handed to a driver ([`Handle::deliver`]), and the answer it will
/// give: awaited after the gate has gone, as the driver can be slow to give it.
#[must_use]
struct Delivery(oneshot::Receiver<Result<Sent, InjectError>>);

impl Delivery {
    /// What the driver answers: the message taken, or why not.
    async fn taken(self) -> Result<Sent, InjectError> {
        self.0.await.map_err(|_| InjectError::message("Codex App Server disconnected"))?
    }
}

/// End a live app-server so the session can be resumed under another account.
///
/// The reply comes back only once the old process is gone and its claim is
/// released, so the caller can spawn immediately; a driver that has already
/// lost its socket answers with a refusal rather than a promise.
async fn retire(handle: &Handle) -> Result<(), InjectError> {
    let (reply, rx) = oneshot::channel();
    handle
        .tx
        .send(Cmd::Retire(reply))
        .map_err(|_| InjectError::message("Codex App Server disconnected"))?;
    rx.await
        .map_err(|_| InjectError::message("Codex App Server disconnected before shutting down"))?
}

const CODEX_ARCHIVE_TIMEOUT: Duration = Duration::from_secs(15);

/// Archive an unloaded Codex session. Loaded sessions must go through
/// `CodexRuns::archive_codex`, which routes to their owning App Server.
///
/// `codex archive` writes the session, so it is started only under a permit
/// that holds the gate and the session's claim.
async fn archive_codex_session(_permit: &SpawnPermit<'_>, paths: &Arc<droppen_instance::paths::Paths>, id: &str) -> Result<(), ArchiveError> {
    let mut command = tokio::process::Command::new("codex");
    command.args(["archive", id]).current_dir(&paths.home).kill_on_drop(true);
    apply_env(&mut command, &clean_env_bare());

    let output: Result<std::io::Result<std::process::Output>, _> = tokio::time::timeout(CODEX_ARCHIVE_TIMEOUT, command.output()).await;
    match output {
        Ok(Ok(output)) if output.status.success() => Ok(()),
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let trimmed = stderr.trim();
            let message = if trimmed.is_empty() {
                format!("codex exited with {}", output.status)
            } else {
                let lines: Vec<&str> = trimmed.split('\n').collect();
                lines[lines.len().saturating_sub(2)..].join(" ")
            };
            Err(ArchiveError(message))
        }
        Ok(Err(error)) => Err(ArchiveError(error.to_string())),
        Err(_) => Err(ArchiveError("the CLI did not answer within 15 seconds".to_string())),
    }
}

/// What a send or an archive is told when its codex session cannot be claimed.
fn claim_refusal(error: ClaimError) -> InjectError {
    match error {
        ClaimError::Busy { holder } => InjectError::message(busy_message(holder)),
        ClaimError::Io(e) => InjectError::Internal(e.to_string()),
    }
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
