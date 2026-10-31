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
    /// Set while the app server is still starting: the session is reserved
    /// and messages queue in the channel for the driver to take, but nothing
    /// has been asked of codex yet ([`Starting`]). Cleared when the driver
    /// takes over.
    starting: Arc<std::sync::atomic::AtomicBool>,
}

impl Handle {
    pub(crate) fn alive(&self) -> bool {
        !self.tx.is_closed()
    }

    /// Whether this session's app server is still starting.
    fn starting(&self) -> bool {
        self.starting.load(std::sync::atomic::Ordering::SeqCst)
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
        }
    }
    pub(crate) fn info(&self) -> droppen_sessions::ResidentInfo {
        self.state().into()
    }
    /// Stop the turn running or being started, or the one a starting app
    /// server is about to run: sent while the message is on its way, the
    /// interrupt waits behind it in the channel, and the driver interrupts
    /// the turn once codex says it started (`interrupt_requested`).
    pub(crate) fn interrupt(&self) -> bool {
        let state = self.shared.lock().unwrap();
        (self.starting() || state.active.is_some() || state.queue.iter().any(|q| q.request.is_some())) && self.tx.send(Cmd::Interrupt).is_ok()
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
    Interrupt,
    Archive(oneshot::Sender<Result<(), InjectError>>),
}

/// A request to codex. Its app server speaks JSON-RPC 2.0 without the
/// `jsonrpc` member on the wire.
#[derive(Serialize)]
struct Request<'a, P> {
    id: &'a str,
    method: &'a str,
    params: P,
}

/// A notification to codex: a request without an id, which nothing answers.
#[derive(Serialize)]
struct Notification<'a, P> {
    method: &'a str,
    params: P,
}

/// The params of a call that takes none, written `{}`.
#[derive(Serialize)]
struct NoParams {}

/// The answer to a request codex makes of us, with the request's id echoed in
/// the type it came in.
#[derive(Serialize)]
struct ErrorResponse<'a> {
    id: &'a Value,
    error: RpcError,
}

#[derive(Serialize)]
struct RpcError {
    code: i64,
    message: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InitializeParams {
    client_info: ClientInfo,
}

#[derive(Serialize)]
struct ClientInfo {
    name: &'static str,
    title: &'static str,
    version: &'static str,
}

/// Codex never stops a headless turn to ask; see [`reject_request`].
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
enum ApprovalPolicy {
    Never,
}

/// The params of `thread/start`, and of `thread/resume` when they name the
/// thread.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadParams<'a> {
    cwd: &'a str,
    approval_policy: ApprovalPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_provider: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_instructions: Option<&'static str>,
}

/// A thread answer's settings as [`State::settings`] keeps them: what codex
/// said, and `null` for any it left out.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadSettings<'a> {
    model: &'a Value,
    effort: &'a Value,
    sandbox_policy: &'a Value,
}

/// A message's `turn/start` params as [`Message::params`] keeps them. The
/// thread and the message's id are added when it is sent, by
/// [`TurnStartOnThread`].
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TurnStartParams<'a> {
    input: Vec<UserInput<'a>>,
    approval_policy: ApprovalPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sandbox_policy: Option<SandboxPolicy>,
}

/// One item of a turn's input.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum UserInput<'a> {
    Text { text: String },
    LocalImage { path: &'a str },
}

/// The sandbox a turn runs in. A turn that names none runs in the thread's.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum SandboxPolicy {
    WorkspaceWrite,
    ReadOnly,
    DangerFullAccess,
}

/// `turn/start`'s params as sent: a queued message's params exactly as the
/// journal holds them, whichever build wrote them, then the thread and the
/// message they start.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TurnStartOnThread<'a> {
    #[serde(flatten)]
    params: StoredParams<'a>,
    thread_id: &'a str,
    client_user_message_id: &'a str,
}

/// A queued message's stored params, less the `threadId` and
/// `clientUserMessageId` [`TurnStartOnThread`] writes after them: a row that
/// holds its own must not name either twice, and the ones written now win.
struct StoredParams<'a>(&'a Value);

impl Serialize for StoredParams<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Object(params) => serializer.collect_map(
                params
                    .iter()
                    .filter(|(key, _)| !matches!(key.as_str(), "threadId" | "clientUserMessageId")),
            ),
            other => other.serialize(serializer),
        }
    }
}

/// `turn/steer`'s params: a queued message's input, into the turn it expects
/// to be running.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TurnSteerParams<'a> {
    thread_id: &'a str,
    expected_turn_id: Option<&'a str>,
    input: &'a Value,
    client_user_message_id: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TurnInterruptParams<'a> {
    thread_id: &'a str,
    turn_id: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadArchiveParams<'a> {
    thread_id: &'a str,
}

fn message(text: &str, options: &SendOptions) -> Result<Message, InjectError> {
    let mut input = vec![UserInput::Text {
        text: with_codex_files(text, options),
    }];
    input.extend(options.images.iter().map(|path| UserInput::LocalImage { path }));
    let permission = if options.chat {
        Some("read-only")
    } else {
        options.permission.as_deref()
    };
    let sandbox_policy = match permission {
        Some(permission) => {
            codex_permission_flags(permission)?;
            Some(match permission {
                "auto" | "workspace-write" => SandboxPolicy::WorkspaceWrite,
                "read-only" => SandboxPolicy::ReadOnly,
                _ => SandboxPolicy::DangerFullAccess,
            })
        }
        None => None,
    };
    let params = TurnStartParams {
        input,
        approval_policy: ApprovalPolicy::Never,
        model: options.model.as_deref(),
        effort: options.effort.as_deref(),
        sandbox_policy,
    };
    Ok(Message {
        // The send path's choice when a client named the message, so its
        // receipt can follow it; see [`droppen_provider::receipts`].
        id: options.message_id.clone().unwrap_or_else(new_uuid),
        text: text.into(),
        // A stored value, because the journal reads back rows other builds
        // wrote.
        params: serde_json::to_value(params).expect("turn/start params encode"),
        request: None,
        uncertain: false,
    })
}

async fn write(link: &mut shim::Link, message: &impl Serialize) -> Result<(), InjectError> {
    let line = serde_json::to_string(message).expect("Codex App Server message encode");
    tokio::time::timeout(RPC_TIMEOUT, link.send_line(&line))
        .await
        .map_err(|_| InjectError::message("Codex App Server write timed out"))?
        .map_err(|e| InjectError::Internal(e.to_string()))
}

// Headless permissions remain `never`. Unexpected interactive requests must get
// an explicit error, not hang the CLI or silently approve a tool invocation.
async fn reject_request(link: &mut shim::Link, value: &Value) -> Result<(), InjectError> {
    let answer = ErrorResponse {
        id: &value["id"],
        error: RpcError {
            code: -32601,
            message: "interactive requests are not supported by Droppen",
        },
    };
    write(link, &answer).await
}

async fn rpc(runs: &CodexRuns, link: &mut shim::Link, method: &str, params: impl Serialize) -> Result<Value, InjectError> {
    let id = new_uuid();
    write(link, &Request { id: &id, method, params }).await?;
    tokio::time::timeout(RPC_TIMEOUT, async {
        while let Some(output) = link.recv().await {
            let shim::Output::Line(line) = output else {
                return Err(InjectError::message("Codex handshake output was lost"));
            };
            let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
            if value.get("method").is_some() && value.get("id").is_some() {
                reject_request(link, &value).await?;
            }
            if value["id"] == id {
                if let Some(error) = value.get("error") {
                    return Err(InjectError::message(runs.host.redact(&error.to_string())));
                }
                return Ok(value["result"].clone());
            }
        }
        Err(InjectError::message("Codex App Server disconnected during handshake"))
    })
    .await
    .map_err(|_| InjectError::message("Codex App Server handshake timed out"))?
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
            std::path::PathBuf::from(session.file_path).written_at().await
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
            chats: true,
            chat_attachments: true,
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
            self.host.journal.add(record);
        }
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
        #[cfg(test)]
        self.host.step("codex archive", Source::Codex, Some(id));
        archive_codex_session(&permit, &self.host.paths, id).await?;
        let (tx, rx) = mpsc::unbounded_channel();
        drop(rx);
        self.codex_servers.lock().unwrap().insert(
            id.into(),
            Handle {
                tx,
                shared: Arc::new(Mutex::new(State {
                    archived: true,
                    ..Default::default()
                })),
                account: None,
                starting: Default::default(),
            },
        );
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
        let starting = self.codex_starting(id, wanted.map(str::to_owned));
        let delivery = starting.handle.deliver(message)?;
        permit.ungate();
        if let Some(existing) = existing {
            retire(&existing).await?;
        }
        self.codex_spawn(&mut permit, Some(id), cwd, options, account, Some(starting)).await?;
        let sent = delivery.taken().await?;
        Ok((id.to_owned(), sent))
    }

    /// Reserve session `id` for an app server about to start under `account`:
    /// its handle in the map, marked starting, in place of whatever was there
    /// (what a dead process left stays on view meanwhile). Under the spawn gate.
    fn codex_starting(self: &Arc<Self>, id: &str, account: Option<String>) -> Starting {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut servers = self.codex_servers.lock().unwrap();
        let state = servers.get(id).map(|before| before.shared.lock().unwrap().clone()).unwrap_or_default();
        let handle = Handle {
            tx,
            shared: Arc::new(Mutex::new(state)),
            account,
            starting: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        };
        let before = servers.insert(id.to_owned(), handle.clone());
        drop(servers);
        #[cfg(test)]
        self.host.step("codex reserved", Source::Codex, Some(id));
        Starting {
            runs: self.clone(),
            id: id.to_owned(),
            handle,
            rx: Some(rx),
            before,
        }
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
        handle.starting.store(false, std::sync::atomic::Ordering::SeqCst);
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

struct Driver {
    runs: Arc<CodexRuns>,
    record: runs::RunRecord<CodexRun>,
    link: shim::Link,
    /// The session's claim. Dropped with the driver, except on
    /// [`Cmd::Retire`] — which hands the session straight to a replacement and
    /// so has to let go of the claim before it answers.
    lease: SessionLease,
    state: State,
    shared: Arc<Mutex<State>>,
    rx: mpsc::UnboundedReceiver<Cmd>,
    replies: HashMap<String, oneshot::Sender<QueueOutcome>>,
    pending_since: Instant,
    // Target turn (unknown until turn/started), time sent, escalation rung.
    interrupt: Option<(Option<String>, Instant, u8)>,
    archive_reply: Option<oneshot::Sender<Result<(), InjectError>>>,
    archive_since: Instant,
    /// Set by [`Cmd::Retire`]: leave the loop without reporting a death.
    retiring: bool,
    /// How long [`Cmd::Retire`] waits for the CLI to exit; a field rather than
    /// the constant so a test can drive the timeout in milliseconds.
    retire_wait: Duration,
}

impl Driver {
    fn checkpoint(&mut self) {
        // The cursor only advances with the state it describes, in one DB row.
        self.state.cursor = self.link.at().max(self.state.cursor);
        self.record.payload.codex = Some(self.state.clone());
        self.runs.host.journal.add(self.record.clone());
        *self.shared.lock().unwrap() = self.state.clone();
    }

    fn running(&mut self) {
        self.state.injection = Some(Injection {
            state: InjectionRunState::Running,
            error: droppen_http::openapi_ext::Nullable(None),
            interrupted: false,
            started_at: iso8601(SystemTime::now()),
            finished_at: droppen_http::openapi_ext::Nullable(None),
        });
    }

    fn finish(&mut self, error: Option<String>, interrupted: bool) {
        let started_at = self
            .state
            .injection
            .as_ref()
            .map(|i| i.started_at.clone())
            .unwrap_or_else(|| self.record.started_at.clone());
        self.state.injection = Some(Injection {
            state: if error.is_some() || interrupted {
                InjectionRunState::Failed
            } else {
                InjectionRunState::Ok
            },
            error: droppen_http::openapi_ext::Nullable(error),
            interrupted,
            started_at,
            finished_at: droppen_http::openapi_ext::Nullable(Some(iso8601(SystemTime::now()))),
        });
    }

    async fn submit(&mut self, index: usize, steer: bool) -> Result<(), InjectError> {
        let id = format!("{}:{}", if steer { "steer" } else { "start" }, self.state.queue[index].id);
        if !steer {
            self.running();
        }
        self.state.queue[index].request = Some(id.clone());
        self.pending_since = Instant::now();
        self.checkpoint();
        let message = &self.state.queue[index];
        let thread_id = self.record.session_id.as_str();
        if steer {
            let request = Request {
                id: &id,
                method: "turn/steer",
                params: TurnSteerParams {
                    thread_id,
                    expected_turn_id: self.state.active.as_deref(),
                    input: &message.params["input"],
                    client_user_message_id: &message.id,
                },
            };
            write(&mut self.link, &request).await
        } else {
            let request = Request {
                id: &id,
                method: "turn/start",
                params: TurnStartOnThread {
                    params: StoredParams(&message.params),
                    thread_id,
                    client_user_message_id: &message.id,
                },
            };
            write(&mut self.link, &request).await
        }
    }

    async fn run(mut self) {
        if self.state.archived {
            self.close_archived().await;
            return;
        }
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            let outcome = tokio::select! {
                output = self.link.recv() => match output {
                    Some(output) => self.output(output).await,
                    None => Err(InjectError::message("Codex App Server disconnected; undelivered messages remain in the session journal")),
                },
                cmd = self.rx.recv() => match cmd { Some(cmd) => self.command(cmd).await, None => break },
                _ = tick.tick() => self.tick().await,
            };
            if let Err(error) = outcome {
                if let Some(reply) = self.archive_reply.take() {
                    let _ = reply.send(Err(InjectError::message(self.runs.host.redact(&error.to_string()))));
                }
                self.state.paused = true;
                let reported = self.runs.host.redact(&error.to_string());
                self.finish(Some(reported), self.state.interrupt_requested);
                self.checkpoint();
                // Keep the claim while the process is alive, even if the socket
                // has failed: a replacement must never run beside an orphan.
                while procs::is_same_process(self.record.cli_pid, self.record.pid_token.as_deref()) {
                    match tokio::time::timeout(Duration::from_secs(2), self.rx.recv()).await {
                        Ok(Some(Cmd::Interrupt)) => self.link.kill_claude(libc::SIGTERM),
                        Ok(Some(Cmd::Deliver(_, reply))) => {
                            let _ = reply.send(Err(InjectError::message("Codex connection lost; restart the server to reattach")));
                        }
                        Ok(Some(Cmd::Archive(reply))) => {
                            let _ = reply.send(Err(InjectError::message(
                                "Codex connection lost; restart the server to reattach before archiving",
                            )));
                        }
                        Ok(Some(Cmd::Queue { reply, .. })) => {
                            let _ = reply.send(QueueOutcome::Unreachable);
                        }
                        Ok(Some(Cmd::Retire(reply))) => {
                            // The process is going anyway; the caller only needs
                            // to know it may not respawn yet.
                            let _ = reply.send(Err(InjectError::message("Codex connection lost; restart the server to reattach")));
                        }
                        Ok(None) => break,
                        Err(_) => {}
                    }
                }
                self.runs.codex_dead(self.record.clone());
                break;
            }
            if self.state.archived || self.retiring {
                break;
            }
            if self.link.at() >= self.link.total()
                && self.state.archive_request.is_none()
                && self.state.active.is_none()
                && !self.state.paused
                && !self.state.queue.is_empty()
                && self.state.queue.iter().all(|q| q.request.is_none())
            {
                if let Err(error) = self.submit(0, false).await {
                    self.state.paused = true;
                    self.finish(Some(error.to_string()), false);
                    self.checkpoint();
                }
            }
        }
    }

    async fn command(&mut self, cmd: Cmd) -> Result<(), InjectError> {
        match cmd {
            Cmd::Archive(reply) => {
                if self.state.active.is_some() || !self.state.queue.is_empty() {
                    let _ = reply.send(Err(InjectError::message(
                        "Codex is running or has queued messages; stop it and clear the queue before archiving",
                    )));
                    return Ok(());
                }
                if self.state.archive_request.is_some() || self.link.at() < self.link.total() {
                    let _ = reply.send(Err(InjectError::message(
                        "Codex is awaiting archive confirmation or replaying session state; retry after it settles",
                    )));
                    return Ok(());
                }
                let id = format!("archive:{}", new_uuid());
                self.state.archive_request = Some(id.clone());
                self.archive_reply = Some(reply);
                self.archive_since = Instant::now();
                self.checkpoint();
                let request = Request {
                    id: &id,
                    method: "thread/archive",
                    params: ThreadArchiveParams {
                        thread_id: &self.record.session_id,
                    },
                };
                write(&mut self.link, &request).await?;
            }
            Cmd::Deliver(message, reply) => {
                if self.state.archived || self.state.archive_request.is_some() {
                    let _ = reply.send(Err(InjectError::message("Codex session is archived or awaiting archive confirmation")));
                    return Ok(());
                }
                let queued = self.state.active.is_some() || !self.state.queue.is_empty();
                self.state.queue.push(message);
                if self.state.queue.iter().all(|q| q.request.is_none() && !q.uncertain) {
                    self.state.paused = false;
                }
                self.checkpoint();
                let _ = reply.send(Ok(Sent { queued }));
            }
            Cmd::Retire(reply) => {
                if self.state.active.is_some() || !self.state.queue.is_empty() || self.state.archive_request.is_some() {
                    let _ = reply.send(Err(InjectError::message("a message is already running for this session")));
                    return Ok(());
                }
                // EOF is the polite end: codex finishes, writes nothing further
                // and exits. A link that will not take it is a process nobody
                // can replace safely, so the caller is told rather than raced.
                if tokio::time::timeout(RPC_TIMEOUT, self.link.close_stdin()).await.is_err() {
                    let _ = reply.send(Err(InjectError::message(
                        "the running Codex process did not accept the shutdown; try again",
                    )));
                    return Ok(());
                }
                // Waited out rather than assumed. A process that has not gone
                // when the budget runs out has *not* retired, and saying it has
                // is the worst answer available: the caller would immediately
                // start a second app-server on the same thread, under the new
                // key, beside one still holding the old key and the writer
                // lock. So nothing is torn down and nothing is handed over —
                // the run, the claim and the handle all stay exactly where they
                // are, and the send is refused. The EOF already sent still
                // stands, so the ordinary disconnect path cleans up when the
                // process does finally go.
                let deadline = Instant::now() + self.retire_wait;
                while procs::is_same_process(self.record.cli_pid, self.record.pid_token.as_deref()) && Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                if procs::is_same_process(self.record.cli_pid, self.record.pid_token.as_deref()) {
                    tracing::warn!(
                        session = %self.record.session_id,
                        pid = self.record.cli_pid,
                        "the Codex process did not exit after its stdin was closed; keeping the session on the account it is already running under"
                    );
                    let _ = reply.send(Err(InjectError::message(
                        "the running Codex process did not accept the shutdown; try again",
                    )));
                    return Ok(());
                }
                self.retiring = true;
                self.runs.host.journal.remove(&self.record.run_id);
                self.runs.host.withdraw_run(Source::Codex, &self.record.session_id);
                // Only its own entry: the send that retires it has put the
                // session's next app server, still starting, in its place.
                {
                    let mut servers = self.runs.codex_servers.lock().unwrap();
                    if servers.get(&self.record.session_id).is_some_and(|handle| Arc::ptr_eq(&handle.shared, &self.shared)) {
                        servers.remove(&self.record.session_id);
                    }
                }
                // Released before the reply, because the caller's very next move
                // is to claim this session for the process it is about to start.
                self.lease.release();
                #[cfg(test)]
                self.runs.host.step("codex retired", Source::Codex, Some(&self.record.session_id));
                let _ = reply.send(Ok(()));
            }
            Cmd::Interrupt => {
                self.state.interrupt_requested = true;
                self.checkpoint();
                self.send_interrupt().await?;
            }
            Cmd::Queue { id, steer, reply } => {
                let Some(index) = self.state.queue.iter().position(|q| q.id == id) else {
                    let _ = reply.send(QueueOutcome::Unknown);
                    return Ok(());
                };
                if self.state.queue[index].request.is_some() {
                    let _ = reply.send(QueueOutcome::AlreadySteered);
                    return Ok(());
                }
                if !steer {
                    let dropped = self.state.queue.remove(index);
                    self.checkpoint();
                    self.runs.host.note_receipt(
                        Source::Codex,
                        &self.record.session_id,
                        &dropped.id,
                        droppen_sessions::delivery::ReceiptStatus::Failed,
                    );
                    let _ = reply.send(QueueOutcome::Done);
                } else {
                    if self.state.queue[index].uncertain {
                        let _ = reply.send(QueueOutcome::Refused { reason: "delivery was not acknowledged before Codex exited; inspect the transcript, then remove this queued entry before resending".into() });
                        return Ok(());
                    }
                    let busy = self.state.active.is_some();
                    if busy
                        && ["model", "effort", "sandboxPolicy"].iter().any(|key| {
                            self.state.queue[index].params.get(*key).is_some_and(|wanted| {
                                if *key == "sandboxPolicy" {
                                    self.state.settings[*key]["type"] != wanted["type"]
                                } else {
                                    self.state.settings.get(*key) != Some(wanted)
                                }
                            })
                        })
                    {
                        let _ = reply.send(QueueOutcome::Refused {
                            reason: "Codex steer keeps the active turn's model, effort and permissions; wait for the next turn to change them".into(),
                        });
                        return Ok(());
                    }
                    if !busy && self.state.queue.iter().any(|q| q.request.is_some()) {
                        let _ = reply.send(QueueOutcome::Unreachable);
                        return Ok(());
                    }
                    self.state.paused = false;
                    self.replies.insert(id, reply);
                    self.submit(index, busy).await?;
                }
            }
        }
        Ok(())
    }

    async fn output(&mut self, output: shim::Output) -> Result<(), InjectError> {
        if self.link.at() <= self.state.cursor {
            return Ok(());
        }
        let shim::Output::Line(line) = output else {
            return Err(InjectError::message("Codex replay buffer overflowed; refusing to guess delivery state"));
        };
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            return Ok(());
        };
        if value.get("method").is_some() && value.get("id").is_some() {
            return reject_request(&mut self.link, &value).await;
        }
        if self.state.archive_request.is_some() && self.state.archive_request.as_deref() == value["id"].as_str() {
            self.state.archive_request = None;
            let result = if let Some(error) = value.get("error") {
                self.checkpoint();
                Err(InjectError::message(self.runs.host.redact(&error.to_string())))
            } else {
                self.state.archived = true;
                self.state.injection = None;
                self.checkpoint();
                self.close_archived().await;
                Ok(())
            };
            if let Some(reply) = self.archive_reply.take() {
                let _ = reply.send(result);
            }
            return Ok(());
        }
        if let Some(index) = self
            .state
            .queue
            .iter()
            .position(|q| q.request.as_deref() == value["id"].as_str() && q.request.is_some())
        {
            let request = self.state.queue[index].request.take().unwrap();
            let qid = self.state.queue[index].id.clone();
            let outcome = if let Some(error) = value.get("error") {
                self.state.paused = true;
                if request.starts_with("start:") {
                    self.state.interrupt_requested = false;
                    self.interrupt = None;
                    let reported = self.runs.host.redact(&error.to_string());
                    self.finish(Some(reported), false);
                }
                QueueOutcome::Refused {
                    reason: self.runs.host.redact(&error.to_string()),
                }
            } else {
                let sent = self.state.queue.remove(index);
                // Acknowledged: a turn started with it, or it was steered into
                // the one running. Either way codex has it.
                self.runs
                    .host.note_receipt(Source::Codex, &self.record.session_id, &sent.id, droppen_sessions::delivery::ReceiptStatus::Sent);
                if request.starts_with("start:") {
                    if let Some(turn) = value["result"]["turn"]["id"].as_str() {
                        // A response may follow turn/completed; do not resurrect it.
                        if self.state.completed_turn.as_deref() != Some(turn) {
                            self.state.active = Some(turn.into());
                            if !self.state.injection.as_ref().is_some_and(|i| i.state == InjectionRunState::Running) {
                                self.running();
                            }
                        }
                    }
                    for key in ["model", "effort", "sandboxPolicy"] {
                        if let Some(v) = sent.params.get(key) {
                            self.state.settings[key] = v.clone();
                        }
                    }
                }
                QueueOutcome::Done
            };
            self.checkpoint();
            if let Some(reply) = self.replies.remove(&qid) {
                let _ = reply.send(outcome);
            }
        }
        // Ahead of the thread filter, which it would not pass: the update is
        // about the *account*, carries no `threadId`, and this process is that
        // account's for its whole life. Recorded, and otherwise ignored exactly
        // as it always was.
        if value["method"] == "account/rateLimits/updated" {
            self.note_rate_limits(&value["params"]);
            return Ok(());
        }
        if value["params"]["threadId"] != self.record.session_id {
            return Ok(());
        }
        match value["method"].as_str() {
            Some("turn/started") => {
                self.state.active = value["params"]["turn"]["id"].as_str().map(str::to_owned);
                if !self.state.injection.as_ref().is_some_and(|i| i.state == InjectionRunState::Running) {
                    self.running();
                }
                self.checkpoint();
                self.send_interrupt().await?;
            }
            Some("turn/completed") => {
                let turn = &value["params"]["turn"];
                if self.state.active.as_deref().is_none_or(|active| Some(active) == turn["id"].as_str()) {
                    self.state.active = None;
                    self.state.completed_turn = turn["id"].as_str().map(str::to_owned);
                    self.state.interrupt_requested = false;
                    self.interrupt = None;
                    let interrupted = turn["status"] == "interrupted";
                    let failed = turn["status"] == "failed";
                    self.state.paused |= failed;
                    self.finish(
                        if failed {
                            Some(self.runs.host.redact(&turn["error"].to_string()))
                        } else {
                            None
                        },
                        interrupted,
                    );
                    self.checkpoint();
                    if failed {
                        self.note_turn_error(&turn["error"]);
                    } else if turn["status"] == "completed" {
                        self.note_turn_completed();
                    }
                    if self.link.at() > self.link.total() {
                        if let (Some(injection), Some(alerts)) = (&self.state.injection, self.runs.host.alerts.get()) {
                            alerts.turn(
                                self.runs.host.paths.clone(),
                                Source::Codex,
                                self.record.session_id.clone(),
                                injection.state,
                                injection.error.0.clone(),
                                self.runs.host.driver_states_of(&*self.runs),
                            );
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Whether the line just read is new, rather than history an attach is
    /// replaying — the same boundary the push notification keeps, and for the
    /// same reason: a limit hit before this server existed is not news.
    fn live(&self) -> bool {
        self.link.at() > self.link.total()
    }

    /// The account and model this process runs, as the limit log names them.
    fn limit_subject(&self) -> (String, Option<String>) {
        (
            droppen_accounts::rest::account_key(self.record.payload.codex_account.as_deref(), AccountProvider::Codex),
            self.state.settings.get("model").and_then(Value::as_str).map(str::to_owned),
        )
    }

    /// A turn that completed: the login it ran on took its credential, so a
    /// refused-credential rest on it is over. Codex has no reading to heal its
    /// local login the way claude's heals ([`droppen_accounts::local_login`]);
    /// this is how a `signed_out` codex login comes back.
    fn note_turn_completed(&self) {
        if !self.live() {
            return;
        }
        let (account_id, _) = self.limit_subject();
        self.runs.host.credential_accepted(&account_id, &self.record.session_id);
    }

    /// Record a failed turn whose error is an account limit. Recording only:
    /// the queue pauses exactly as it did before.
    fn note_turn_error(&self, error: &Value) {
        if !self.live() {
            return;
        }
        let (account_id, model) = self.limit_subject();
        let Some(verdict) = droppen_accounts::limits::read_codex_turn_error(error) else {
            return;
        };
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| error.as_str())
            .unwrap_or_default()
            .to_string();
        self.runs.host.record_limit(droppen_accounts::rest::Observation {
            account_id,
            session_id: Some(self.record.session_id.clone()),
            source: "codex:turn",
            model,
            verdict,
            raw: error.to_string(),
            reason: message,
            may_rest: true,
        });
    }

    /// Record an `account/rateLimits/updated` that says a limit was reached.
    ///
    /// Most of these are readings — codex sends one after every response — and
    /// are not events. One with a full window, or a limit or spend control
    /// reached, is; it rests the account only when it says until when.
    fn note_rate_limits(&self, params: &Value) {
        if !self.live() {
            return;
        }
        let Some(verdict) = droppen_accounts::limits::read_codex_rate_limits(params) else {
            return;
        };
        let (account_id, model) = self.limit_subject();
        let may_rest = verdict.until_ms.is_some();
        let reason = params
            .pointer("/rateLimits/rateLimitReachedType")
            .and_then(Value::as_str)
            .map(|kind| format!("codex rate limits: {kind}"))
            .unwrap_or_else(|| "codex rate limits: a window is at 100%".to_string());
        self.runs.host.record_limit(droppen_accounts::rest::Observation {
            account_id,
            session_id: Some(self.record.session_id.clone()),
            source: "codex:rate_limits",
            model,
            verdict,
            raw: params.to_string(),
            reason,
            may_rest,
        });
    }

    async fn tick(&mut self) -> Result<(), InjectError> {
        if self.archive_reply.is_some() && self.archive_since.elapsed() > RPC_TIMEOUT {
            let _ = self.archive_reply.take().unwrap().send(Err(InjectError::message(
                "Codex archive acknowledgement timed out; waiting for confirmation without retrying or accepting new messages",
            )));
            // Keep the persisted ID for a late reply or restart replay.
        }
        if self.link.at() >= self.link.total() {
            self.send_interrupt().await?;
        }
        if !self.state.paused && self.state.queue.iter().any(|q| q.request.is_some()) && self.pending_since.elapsed() > RPC_TIMEOUT {
            self.state.paused = true;
            for (_, reply) in self.replies.drain() {
                let _ = reply.send(QueueOutcome::Unreachable);
            }
            // Keep the request IDs for late acknowledgements/replay. Never resend.
            if self.state.active.is_none() {
                self.finish(
                    Some("Codex delivery acknowledgement timed out; message retained without automatic retry".into()),
                    false,
                );
            }
            self.checkpoint();
        }
        if let Some((turn, at, rung)) = &self.interrupt {
            let patience = if *rung == 0 { RPC_TIMEOUT } else { self.runs.host.interrupt_step };
            let still_target = self.state.active == *turn && (turn.is_some() || self.state.queue.iter().any(|q| q.request.is_some()));
            if at.elapsed() > patience && still_target {
                // A CLI wedged before announcing a turn is cancellable too.
                self.link.kill_claude(if *rung == 0 { libc::SIGTERM } else { libc::SIGKILL });
                self.interrupt = Some((turn.clone(), Instant::now(), 1));
            }
        }
        Ok(())
    }

    async fn close_archived(&mut self) {
        // Closing this per-session server is safe only after archive succeeds.
        // Keep its archived checkpoint if EOF cannot be sent, for next boot.
        match tokio::time::timeout(RPC_TIMEOUT, self.link.close_stdin()).await {
            Ok(Ok(())) => self.runs.host.journal.remove(&self.record.run_id),
            _ => tracing::warn!(session = %self.record.session_id, "archived Codex server could not be closed; keeping its cleanup record"),
        }
    }

    async fn send_interrupt(&mut self) -> Result<(), InjectError> {
        let waiting_for_id = self.interrupt.as_ref().is_some_and(|(turn, _, rung)| turn.is_none() && *rung == 0);
        if self.state.interrupt_requested && (self.interrupt.is_none() || waiting_for_id) && self.link.at() >= self.link.total() {
            if let Some(turn) = self.state.active.clone() {
                let id = new_uuid();
                self.interrupt = Some((Some(turn.clone()), Instant::now(), 0));
                let request = Request {
                    id: &id,
                    method: "turn/interrupt",
                    params: TurnInterruptParams {
                        thread_id: &self.record.session_id,
                        turn_id: &turn,
                    },
                };
                write(&mut self.link, &request).await?;
            } else if self.interrupt.is_none() && self.state.queue.iter().any(|q| q.request.is_some()) {
                self.interrupt = Some((None, Instant::now(), 0));
            }
        }
        Ok(())
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

#[cfg(test)]
mod tests {
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
    use droppen_instance::instance::InstanceDirs;
    use droppen_instance::paths::Paths;
    use serde_json::json;
    use tokio::io::{AsyncWriteExt, BufReader};
    use tokio::net::{
        unix::{OwnedReadHalf, OwnedWriteHalf},
        UnixListener,
    };

    /// Where this binary's session shim is: Codex's spawns under test run it
    /// as this module's `session_shim`
    /// ([`droppen_provider::testing::shims_run_as`]).
    pub(super) const SHIMS: &str = module_path!();

    /// Not a test: the session shim Codex's spawns under test run
    /// ([`droppen_provider::testing::session_shim`]).
    #[test]
    #[ignore = "the session shim a Codex test's spawn runs, not a test"]
    fn session_shim() {
        droppen_provider::testing::session_shim();
    }

    /// The logins the accounts are handed here: Codex's own, and a stand-in
    /// for Claude Code's, which nothing here starts.
    fn logins() -> droppen_accounts::login::Logins {
        droppen_accounts::login::Logins {
            codex: Arc::new(crate::login::CodexLogin),
            ..droppen_accounts::login::Logins::stand_ins()
        }
    }

    /// The providers over `registry`: Codex alone, the one these tests drive.
    fn codex_only(registry: &Arc<InjectionRegistry>) -> Arc<droppen_provider::ProviderRegistry> {
        droppen_provider::ProviderRegistry::new(registry.clone(), |providers, registry| {
            providers.register(Arc::new(Codex::new(registry.clone())))
        })
    }

    struct Harness {
        driver: Driver,
        input: BufReader<OwnedReadHalf>,
        output: OwnedWriteHalf,
        /// The providers over the driver's registry, for their spawn gate.
        providers: Arc<droppen_provider::ProviderRegistry>,
        _home: tempfile::TempDir,
    }

    impl Harness {
        async fn new() -> Self {
            let home = tempfile::tempdir().unwrap();
            let paths = Arc::new(Paths::new(home.path().into()));
            let dirs = InstanceDirs::new(&paths.data_dir(), "codex-test").unwrap();
            dirs.create().unwrap();
            let accounts = Arc::new(droppen_accounts::store::AccountsStore::new(paths.clone(), logins()));
            let quota = Arc::new(droppen_accounts::quota::QuotaCache::new(paths.clone(), logins()));
            let decoration = droppen_provider::testing::no_decoration();
            let registry = InjectionRegistry::new(paths, &dirs, accounts, quota, decoration);
            let providers = codex_only(&registry);
            let socket = home.path().join("mock.sock");
            let listener = UnixListener::bind(&socket).unwrap();
            let connector = tokio::spawn(async move { shim::connect_in_process(&socket).await.unwrap() });
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut output) = stream.into_split();
            let mut input = BufReader::new(read);
            let mut hello = String::new();
            input.read_line(&mut hello).await.unwrap();
            output
                .write_all(format!("{{\"shim_hello\":{{\"claude_pid\":{},\"total\":0}}}}\n", std::process::id()).as_bytes())
                .await
                .unwrap();
            let link = connector.await.unwrap();
            let record = runs::RunRecord {
                run_id: new_uuid(),
                source: Source::Codex,
                session_id: "test-thread".into(),
                cwd: None,
                cli_pid: std::process::id(),
                started_at: iso8601(SystemTime::now()),
                pid_token: procs::start_token(std::process::id()),
                resident: true,
                shim: None,
                payload: CodexRun {
                    codex: Some(State::default()),
                    ..CodexRun::default()
                },
            };
            let (_tx, rx) = mpsc::unbounded_channel();
            let driver = Driver {
                lease: providers.gate().lease(Source::Codex, "test-thread").unwrap(),
                runs: codex(&providers).clone(),
                record,
                link,
                state: State::default(),
                shared: Arc::new(Mutex::new(State::default())),
                rx,
                replies: HashMap::new(),
                pending_since: Instant::now(),
                interrupt: None,
                archive_reply: None,
                archive_since: Instant::now(),
                retiring: false,
                // Milliseconds, not the half-minute a real shutdown is given:
                // every test here drives a mock socket whose "CLI" is this very
                // process, so the live branch would otherwise wait out the lot.
                retire_wait: Duration::from_millis(100),
            };
            Self {
                driver,
                input,
                output,
                providers,
                _home: home,
            }
        }
        /// A handle wired to this driver's shared state, as the registry holds
        /// one. The receiver comes back with it so the caller can keep the
        /// handle *alive* — a dropped one reads as a driver that has stopped.
        fn handle(&self) -> (Handle, mpsc::UnboundedReceiver<Cmd>) {
            let (tx, rx) = mpsc::unbounded_channel();
            (
                Handle {
                    tx,
                    shared: self.driver.shared.clone(),
                    account: None,
                    starting: Default::default(),
                },
                rx,
            )
        }
        async fn event(&mut self, value: Value) {
            self.output.write_all(format!("{value}\n").as_bytes()).await.unwrap();
            let output = self.driver.link.recv().await.unwrap();
            self.driver.output(output).await.unwrap();
        }
        /// The next line the driver wrote, byte for byte, newline included.
        async fn raw(&mut self) -> String {
            let mut line = String::new();
            tokio::time::timeout(Duration::from_secs(2), self.input.read_line(&mut line))
                .await
                .unwrap()
                .unwrap();
            line
        }
        async fn request(&mut self) -> Value {
            serde_json::from_str(&self.raw().await).unwrap()
        }
        async fn enqueue(&mut self, text: &str) -> String {
            let message = message(text, &SendOptions::default()).unwrap();
            let id = message.id.clone();
            let (tx, rx) = oneshot::channel();
            self.driver.command(Cmd::Deliver(message, tx)).await.unwrap();
            rx.await.unwrap().unwrap();
            id
        }
        async fn start(&mut self) -> Value {
            self.enqueue("first").await;
            self.driver.submit(0, false).await.unwrap();
            let request = self.request().await;
            self.event(json!({"method":"turn/started","params":{"threadId":"test-thread","turn":{"id":"turn-1","status":"inProgress"}}}))
                .await;
            self.event(json!({"id":request["id"],"result":{"turn":{"id":"turn-1","status":"inProgress"}}}))
                .await;
            request
        }
        async fn complete(&mut self, status: &str) {
            self.event(json!({"method":"turn/completed","params":{"threadId":"test-thread","turn":{"id":"turn-1","status":status,"error":{"message":"test failure"}}}})).await;
        }
    }

    /// One written line: `template` with each placeholder replaced by the
    /// value that differs from run to run, and the newline that ends it.
    fn wire(template: &str, values: &[(&str, &str)]) -> String {
        let mut line = values.iter().fold(template.to_owned(), |line, (from, to)| line.replace(from, to));
        line.push('\n');
        line
    }

    /// The `id` member of a written line, for the requests that mint one.
    fn id_of(line: &str) -> String {
        serde_json::from_str::<Value>(line).unwrap()["id"].as_str().unwrap().to_owned()
    }

    /// The codex local login is `signed_out` on a refused credential, a rest
    /// with no end of its own. A turn that completes on it proves the
    /// credential works again and ends that rest; a failed one does not, and a
    /// quota rest is left alone.
    #[tokio::test]
    async fn a_completed_turn_ends_a_refused_credential_rest_on_its_login() {
        use droppen_accounts::limits::Verdict;
        use droppen_sessions::account::LimitClass;
        use droppen_accounts::local_login::LocalLoginState;
        use droppen_accounts::rest::{now_ms, Observation, LOCAL_CODEX};
        let mut h = Harness::new().await;
        h.start().await;
        let accounts = h.driver.runs.host.accounts.clone();
        let rest = |class: LimitClass, until_ms: Option<i64>| Observation {
            account_id: LOCAL_CODEX.to_string(),
            session_id: Some("test-thread".into()),
            source: "codex:turn",
            model: None,
            verdict: Verdict {
                class,
                scope: "all".into(),
                until_ms,
            },
            raw: "401 Unauthorized".into(),
            reason: String::new(),
            may_rest: true,
        };
        accounts.limit_log().record(&rest(LimitClass::Auth, None), now_ms()).expect("refused");
        assert_eq!(accounts.local_logins().codex.state, LocalLoginState::SignedOut);

        h.complete("failed").await;
        assert_eq!(accounts.local_logins().codex.state, LocalLoginState::SignedOut, "a failed turn proves nothing");

        accounts
            .limit_log()
            .record(&rest(LimitClass::Session, Some(now_ms() + 3_600_000)), now_ms())
            .expect("spent");
        h.complete("completed").await;
        let local = accounts.local_logins().codex;
        assert_eq!(local.state, LocalLoginState::Unknown, "signed in again, as far as anything says");
        assert_eq!(
            local.rest.iter().map(|rest| rest.class).collect::<Vec<_>>(),
            vec![LimitClass::Session],
            "only the refused credential ends"
        );
    }

    /// A failed turn that is an account limit, and a rate-limit update with a
    /// full window, land in the limit log — and nothing else about the driver
    /// changes: the failed turn pauses the queue exactly as any failed turn
    /// always has.
    #[tokio::test]
    async fn a_usage_limit_is_recorded_and_changes_nothing_else() {
        use droppen_sessions::account::LimitClass;
        let mut h = Harness::new().await;
        h.start().await;
        let log = h.driver.runs.host.accounts.limit_log();

        h.event(json!({"method":"account/rateLimits/updated","params":{"rateLimits":{"primary":{"usedPercent":40,"resetsAt":1790000000}}}}))
            .await;
        assert!(log.events(None, 10).is_empty(), "a reading is not an event");

        // A full window resetting in 2100, then the turn failing on it.
        h.event(json!({"method":"account/rateLimits/updated","params":{"rateLimits":{"primary":{"usedPercent":100,"windowDurationMins":300,"resetsAt":4_102_444_800i64}}}}))
            .await;
        h.event(json!({"method":"turn/completed","params":{"threadId":"test-thread","turn":{"id":"turn-1","status":"failed","error":{"message":"You've hit your usage limit. Try again later.","codexErrorInfo":"usageLimitExceeded"}}}}))
            .await;
        assert!(h.driver.state.paused, "the queue pauses on a failed turn, as it always did");
        assert!(h.driver.state.active.is_none());

        let events = log.events(None, 10);
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!((events[0].source.as_str(), events[0].class), ("codex:turn", LimitClass::Session));
        assert_eq!(events[0].account_id.0, None, "the machine's own codex login goes out as null");
        assert_eq!(events[0].session_id.0.as_deref(), Some("test-thread"));
        assert_eq!((events[1].source.as_str(), events[1].class), ("codex:rate_limits", LimitClass::Rate));
        assert!(events[1].until_at.0.as_deref().is_some_and(|at| at.starts_with("2100-01-01")));

        // One rest for the whole account: the known reset, which the usage
        // limit's thirty-minute default did not cut short.
        let rests = log.active_rests(droppen_accounts::rest::now_ms());
        let local = &rests[droppen_accounts::rest::LOCAL_CODEX];
        assert_eq!(local.len(), 1);
        assert_eq!(local[0].class, LimitClass::Rate);
        assert!(local[0].until_at.0.as_deref().is_some_and(|at| at.starts_with("2100-01-01")));

        // A failure that is not a limit is not an event.
        h.complete("failed").await;
        assert_eq!(log.events(None, 10).len(), 2);
    }

    #[test]
    fn turn_params_preserve_settings_and_attachments() {
        let m = message(
            "hello",
            &SendOptions {
                model: Some("gpt-5".into()),
                effort: Some("high".into()),
                permission: Some("auto".into()),
                images: vec!["/uploads/a.png".into()],
                files: vec!["/uploads/a.txt".into()],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(m.params["sandboxPolicy"]["type"], "workspaceWrite");
        assert_eq!(m.params["approvalPolicy"], "never");
        assert_eq!(m.params["model"], "gpt-5");
        assert_eq!(m.params["effort"], "high");
        assert_eq!(m.params["input"][1], json!({"type":"localImage","path":"/uploads/a.png"}));
        assert!(m.params["input"][0]["text"].as_str().unwrap().contains("/uploads/a.txt"));
        // The bytes the journal keeps and every later turn/start is cloned from.
        assert_eq!(
            serde_json::to_string(&m.params).unwrap(),
            r#"{"input":[{"type":"text","text":"hello\n\n[1 attached file(s) — view with the Read tool:]\n- /uploads/a.txt"},{"type":"localImage","path":"/uploads/a.png"}],"approvalPolicy":"never","model":"gpt-5","effort":"high","sandboxPolicy":{"type":"workspaceWrite"}}"#
        );
        assert!(message(
            "x",
            &SendOptions {
                permission: Some("invalid".into()),
                ..Default::default()
            }
        )
        .is_err());
    }

    /// The sandbox is the one security boundary a headless turn has, so the
    /// turn params of every permission mode, of none and of one codex does not
    /// know are pinned byte for byte, outside a chat and in one. A chat is
    /// read-only whatever mode it names, an unknown one included; outside a
    /// chat an unknown mode is refused, and no mode at all names no sandbox and
    /// leaves the thread's own in force.
    #[test]
    fn turn_params_name_the_sandbox_each_permission_mode_maps_to() {
        const UNNAMED: &str = r#"{"input":[{"type":"text","text":"hi"}],"approvalPolicy":"never"}"#;
        const WRITE: &str = r#"{"input":[{"type":"text","text":"hi"}],"approvalPolicy":"never","sandboxPolicy":{"type":"workspaceWrite"}}"#;
        const READ: &str = r#"{"input":[{"type":"text","text":"hi"}],"approvalPolicy":"never","sandboxPolicy":{"type":"readOnly"}}"#;
        const FULL: &str = r#"{"input":[{"type":"text","text":"hi"}],"approvalPolicy":"never","sandboxPolicy":{"type":"dangerFullAccess"}}"#;
        const REFUSED: &str = "unknown codex permission mode: invalid";
        let params = |permission: Option<&str>, chat: bool| {
            let options = SendOptions {
                permission: permission.map(str::to_owned),
                chat,
                ..Default::default()
            };
            message("hi", &options)
                .map(|message| serde_json::to_string(&message.params).unwrap())
                .map_err(|error| error.to_string())
        };
        let cases = [
            (None, false, Ok(UNNAMED)),
            (Some("auto"), false, Ok(WRITE)),
            (Some("workspace-write"), false, Ok(WRITE)),
            (Some("read-only"), false, Ok(READ)),
            (Some("full-access"), false, Ok(FULL)),
            (Some("danger-full-access"), false, Ok(FULL)),
            (Some("bypass-all"), false, Ok(FULL)),
            (Some("invalid"), false, Err(REFUSED)),
            (None, true, Ok(READ)),
            (Some("auto"), true, Ok(READ)),
            (Some("workspace-write"), true, Ok(READ)),
            (Some("read-only"), true, Ok(READ)),
            (Some("full-access"), true, Ok(READ)),
            (Some("danger-full-access"), true, Ok(READ)),
            (Some("bypass-all"), true, Ok(READ)),
            (Some("invalid"), true, Ok(READ)),
        ];
        for (permission, chat, expected) in cases {
            let expected = expected.map(str::to_owned).map_err(str::to_owned);
            assert_eq!(params(permission, chat), expected, "permission {permission:?}, chat {chat}");
        }
    }

    /// What the driver writes for a turn, a steer into it, the interrupt that
    /// ends it and an archive, byte for byte, as codex reads them. Only the
    /// ids the driver mints are filled in. None of them carries a `jsonrpc`
    /// member, unlike the quota probe's requests to the same protocol.
    #[tokio::test]
    async fn the_driver_writes_turn_steer_interrupt_and_archive_requests_byte_for_byte() {
        let mut h = Harness::new().await;
        let first = h.enqueue("first").await;
        h.driver.submit(0, false).await.unwrap();
        assert_eq!(
            h.raw().await,
            wire(
                r#"{"id":"start:QID","method":"turn/start","params":{"input":[{"type":"text","text":"first"}],"approvalPolicy":"never","threadId":"test-thread","clientUserMessageId":"QID"}}"#,
                &[("QID", &first)]
            )
        );
        h.event(json!({"method":"turn/started","params":{"threadId":"test-thread","turn":{"id":"turn-1","status":"inProgress"}}}))
            .await;
        h.event(json!({"id":format!("start:{first}"),"result":{"turn":{"id":"turn-1","status":"inProgress"}}}))
            .await;

        let correction = h.enqueue("correction").await;
        let (reply, rx) = oneshot::channel();
        h.driver
            .command(Cmd::Queue {
                id: correction.clone(),
                steer: true,
                reply,
            })
            .await
            .unwrap();
        assert_eq!(
            h.raw().await,
            wire(
                r#"{"id":"steer:QID","method":"turn/steer","params":{"threadId":"test-thread","expectedTurnId":"turn-1","input":[{"type":"text","text":"correction"}],"clientUserMessageId":"QID"}}"#,
                &[("QID", &correction)]
            )
        );
        h.event(json!({"id":format!("steer:{correction}"),"result":{"turnId":"turn-1"}})).await;
        assert_eq!(rx.await.unwrap(), QueueOutcome::Done);

        h.driver.command(Cmd::Interrupt).await.unwrap();
        let interrupt = h.raw().await;
        let id = id_of(&interrupt);
        assert_eq!(
            interrupt,
            wire(
                r#"{"id":"UUID","method":"turn/interrupt","params":{"threadId":"test-thread","turnId":"turn-1"}}"#,
                &[("UUID", &id)]
            )
        );
        h.event(json!({"id":id,"result":{}})).await;
        h.complete("interrupted").await;

        let (reply, rx) = oneshot::channel();
        h.driver.command(Cmd::Archive(reply)).await.unwrap();
        let archive = h.raw().await;
        let id = id_of(&archive);
        assert_eq!(
            archive,
            wire(
                r#"{"id":"archive:UUID","method":"thread/archive","params":{"threadId":"test-thread"}}"#,
                &[("UUID", id.strip_prefix("archive:").unwrap())]
            )
        );
        h.event(json!({"id":id,"result":{}})).await;
        rx.await.unwrap().unwrap();
    }

    /// A stored row that already names a thread and a message, where the
    /// turn's request puts them, is sent naming each once: the thread it runs
    /// on and the message it is.
    #[tokio::test]
    async fn a_stored_rows_own_thread_and_message_give_way_to_the_ones_sent() {
        let mut h = Harness::new().await;
        let stored: Message = serde_json::from_value(json!({
            "id": "queued-1",
            "text": "first",
            "params": {
                "input": [{"type": "text", "text": "first"}],
                "approvalPolicy": "never",
                "threadId": "stale-thread",
                "clientUserMessageId": "stale-message",
            },
            "request": null,
        }))
        .unwrap();
        let (reply, rx) = oneshot::channel();
        h.driver.command(Cmd::Deliver(stored, reply)).await.unwrap();
        rx.await.unwrap().unwrap();
        h.driver.submit(0, false).await.unwrap();
        assert_eq!(
            h.raw().await,
            wire(
                r#"{"id":"start:queued-1","method":"turn/start","params":{"input":[{"type":"text","text":"first"}],"approvalPolicy":"never","threadId":"test-thread","clientUserMessageId":"queued-1"}}"#,
                &[]
            )
        );
    }

    /// Switching account retires the running process — but only when it has
    /// really gone. A CLI that ignores the EOF must leave everything exactly
    /// where it was, because the caller's next move is to start a second
    /// app-server on the same thread under a different key.
    #[tokio::test]
    async fn a_process_that_will_not_exit_keeps_its_claim_instead_of_being_replaced() {
        let mut h = Harness::new().await;
        let run_id = h.driver.record.run_id.clone();
        h.driver.runs.host.journal.add(h.driver.record.clone());
        let (handle, _rx) = h.handle();
        h.driver.runs.codex_servers.lock().unwrap().insert("test-thread".into(), handle);

        // `cli_pid` is this test process, which is not going anywhere.
        let (reply, rx) = oneshot::channel();
        h.driver.command(Cmd::Retire(reply)).await.unwrap();
        let error = rx.await.unwrap().expect_err("a process that is still there has not retired");
        assert!(error.to_string().contains("did not accept the shutdown"), "{error}");

        assert!(!h.driver.retiring, "the driver stays in its loop");
        assert!(h.driver.lease.holds(), "and keeps the whole-machine claim");
        assert!(
            h.driver.runs.host.journal.read().iter().any(|record| record.run_id == run_id),
            "the run is still the one on disk"
        );
        assert!(
            h.driver.runs.codex_handle("test-thread").is_some(),
            "so a send is refused rather than double-spawned"
        );
    }

    /// The other half: once the process is gone, the claim is released *before*
    /// the reply, so the caller can spawn the replacement immediately.
    #[tokio::test]
    async fn a_retired_process_hands_over_its_claim_and_its_journal_entry() {
        let mut h = Harness::new().await;
        let run_id = h.driver.record.run_id.clone();
        h.driver.runs.host.journal.add(h.driver.record.clone());
        let (handle, _rx) = h.handle();
        h.driver.runs.codex_servers.lock().unwrap().insert("test-thread".into(), handle);
        // A pid that has really been and gone, reaped before it is named here:
        // a made-up number risks `kill(-1, 0)` and other pids that are not pids.
        let mut gone = std::process::Command::new("true").spawn().expect("spawn");
        let gone_pid = gone.id();
        gone.wait().expect("reap");
        h.driver.record.cli_pid = gone_pid;
        h.driver.record.pid_token = None;
        assert!(!procs::is_same_process(gone_pid, None), "the fixture must really be dead");

        let (reply, rx) = oneshot::channel();
        h.driver.command(Cmd::Retire(reply)).await.unwrap();
        rx.await.unwrap().expect("the process is gone, so the session is free");

        assert!(h.driver.retiring, "the loop is asked to end without reporting a death");
        assert!(!h.driver.lease.holds(), "the claim is released before the caller is told");
        assert!(!h.driver.runs.host.journal.read().iter().any(|record| record.run_id == run_id));
        assert!(h.driver.runs.codex_handle("test-thread").is_none());
    }

    #[tokio::test]
    async fn archive_uses_the_resident_and_removes_its_journal_only_after_ack() {
        let mut h = Harness::new().await;
        let (reply, rx) = oneshot::channel();
        h.driver.command(Cmd::Archive(reply)).await.unwrap();
        let request = h.request().await;
        assert_eq!(request["method"], "thread/archive");
        assert_eq!(request["params"]["threadId"], "test-thread");
        let saved = h.driver.runs.host.journal.read().pop().unwrap().decode::<CodexRun>().unwrap().payload.codex.unwrap();
        assert_eq!(saved.archive_request.as_deref(), request["id"].as_str());
        assert!(!saved.archived);
        h.event(json!({"id":request["id"],"result":{}})).await;
        rx.await.unwrap().unwrap();
        assert!(h.driver.shared.lock().unwrap().archived);
        assert!(h.driver.runs.host.journal.read().is_empty());
        assert!(h.driver.state.injection.is_none());
    }

    #[tokio::test]
    async fn archive_rejection_preserves_the_resident_and_allows_delivery() {
        let mut h = Harness::new().await;
        let (reply, rx) = oneshot::channel();
        h.driver.command(Cmd::Archive(reply)).await.unwrap();
        let request = h.request().await;
        h.event(json!({"id":request["id"],"error":{"code":-32600,"message":"archive refused"}}))
            .await;
        assert!(rx.await.unwrap().unwrap_err().to_string().contains("archive refused"));
        assert!(!h.driver.state.archived);
        assert!(h.driver.state.archive_request.is_none());
        assert_eq!(h.driver.runs.host.journal.read().len(), 1);
        h.enqueue("still usable").await;
        assert_eq!(h.driver.state.queue[0].text, "still usable");
    }

    #[tokio::test]
    async fn archive_refuses_running_and_queued_work_without_dropping_it() {
        for running in [false, true] {
            let mut h = Harness::new().await;
            if running {
                h.start().await;
            } else {
                h.enqueue("keep me").await;
            }
            let before = serde_json::to_value(&h.driver.state).unwrap();
            let (reply, rx) = oneshot::channel();
            h.driver.command(Cmd::Archive(reply)).await.unwrap();
            assert!(rx.await.unwrap().is_err());
            assert_eq!(serde_json::to_value(&h.driver.state).unwrap(), before);
        }
    }

    #[tokio::test]
    async fn archive_timeout_blocks_delivery_and_reconciles_late_ack_after_replay() {
        let mut h = Harness::new().await;
        let (reply, rx) = oneshot::channel();
        h.driver.command(Cmd::Archive(reply)).await.unwrap();
        let request = h.request().await;
        h.driver.archive_since = Instant::now() - RPC_TIMEOUT - Duration::from_secs(1);
        h.driver.tick().await.unwrap();
        assert!(rx.await.unwrap().unwrap_err().to_string().contains("timed out"));
        let (reply, rx) = oneshot::channel();
        h.driver
            .command(Cmd::Deliver(message("too late", &SendOptions::default()).unwrap(), reply))
            .await
            .unwrap();
        assert!(rx.await.unwrap().is_err());
        assert!(h.driver.state.queue.is_empty());
        h.driver.state = h.driver.runs.host.journal.read().pop().unwrap().decode::<CodexRun>().unwrap().payload.codex.unwrap();
        assert_eq!(h.driver.state.archive_request.as_deref(), request["id"].as_str());
        h.event(json!({"id":request["id"],"result":{}})).await;
        assert!(h.driver.state.archived);
        assert!(h.driver.runs.host.journal.read().is_empty());
    }

    #[tokio::test]
    async fn archive_gate_prevents_a_concurrent_send_from_resurrecting_the_session() {
        let mut h = Harness::new().await;
        let registry = h.driver.runs.clone();
        let (tx, rx) = mpsc::unbounded_channel();
        h.driver.rx = rx;
        let handle = Handle {
            tx,
            shared: h.driver.shared.clone(),
            account: None,
            starting: Default::default(),
        };
        registry.codex_servers.lock().unwrap().insert("test-thread".into(), handle.clone());
        let driver = tokio::spawn(h.driver.run());
        let (archiver, providers) = (registry.clone(), h.providers.clone());
        let archive = tokio::spawn(async move { archiver.archive_codex(providers.gate(), "test-thread").await });
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(2), h.input.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["method"], "thread/archive");
        let (sender, providers) = (registry.clone(), h.providers.clone());
        let send = tokio::spawn(async move {
            sender
                .codex_deliver(providers.gate(), Some("test-thread"), None, "stale send", &SendOptions::default(), None)
                .await
        });
        h.output
            .write_all(format!("{}\n", json!({"id":request["id"],"result":{}})).as_bytes())
            .await
            .unwrap();
        archive.await.unwrap().unwrap();
        assert!(send.await.unwrap().unwrap_err().to_string().contains("archived"));
        driver.await.unwrap();
        assert!(!handle.visible());
        assert!(registry.host.journal.read().is_empty());
        assert!(!registry.host.claim_held(Source::Codex, "test-thread"), "the driver released its claim");
    }

    #[tokio::test]
    async fn native_steer_is_acknowledged_without_ending_the_turn() {
        let mut h = Harness::new().await;
        h.start().await;
        let id = h.enqueue("correction").await;
        let (reply, rx) = oneshot::channel();
        h.driver
            .command(Cmd::Queue {
                id: id.clone(),
                steer: true,
                reply,
            })
            .await
            .unwrap();
        let request = h.request().await;
        assert_eq!(request["method"], "turn/steer");
        assert_eq!(request["params"]["expectedTurnId"], "turn-1");
        assert_eq!(request["params"]["input"][0]["text"], "correction");
        assert!(h.driver.state.queue[0].request.is_some());
        h.event(json!({"id":request["id"],"result":{"turnId":"turn-1"}})).await;
        assert_eq!(rx.await.unwrap(), QueueOutcome::Done);
        assert!(h.driver.state.queue.is_empty());
        assert_eq!(h.driver.state.active.as_deref(), Some("turn-1"));
        assert_eq!(h.driver.state.injection.as_ref().unwrap().state, InjectionRunState::Running);
    }

    #[tokio::test]
    async fn rejected_steer_preserves_the_message_and_current_turn() {
        let mut h = Harness::new().await;
        h.start().await;
        let id = h.enqueue("correction").await;
        let (reply, rx) = oneshot::channel();
        h.driver
            .command(Cmd::Queue {
                id: id.clone(),
                steer: true,
                reply,
            })
            .await
            .unwrap();
        let request = h.request().await;
        h.event(json!({"id":request["id"],"error":{"code":-32600,"message":"turn changed"}}))
            .await;
        assert!(matches!(rx.await.unwrap(), QueueOutcome::Refused { reason } if reason.contains("turn changed")));
        assert_eq!(h.driver.state.queue[0].id, id);
        assert!(h.driver.state.queue[0].request.is_none());
        assert_eq!(h.driver.state.active.as_deref(), Some("turn-1"));
    }

    #[tokio::test]
    async fn interrupt_waits_for_completion_and_keeps_the_queue() {
        let mut h = Harness::new().await;
        h.start().await;
        h.enqueue("next").await;
        h.driver.command(Cmd::Interrupt).await.unwrap();
        let request = h.request().await;
        assert_eq!(request["method"], "turn/interrupt");
        assert_eq!(request["params"]["turnId"], "turn-1");
        h.event(json!({"id":request["id"],"result":{}})).await;
        assert!(h.driver.state.active.is_some());
        h.complete("interrupted").await;
        assert!(h.driver.state.active.is_none());
        assert!(h.driver.state.injection.as_ref().unwrap().interrupted);
        assert_eq!(h.driver.state.queue.len(), 1);
        assert!(!h.driver.state.paused);
    }

    #[tokio::test]
    async fn stop_during_start_handshake_is_not_lost() {
        let mut h = Harness::new().await;
        h.enqueue("first").await;
        h.driver.submit(0, false).await.unwrap();
        h.request().await;
        h.driver.command(Cmd::Interrupt).await.unwrap();
        h.event(json!({"method":"turn/started","params":{"threadId":"test-thread","turn":{"id":"turn-1"}}}))
            .await;
        assert_eq!(h.request().await["method"], "turn/interrupt");
    }

    #[tokio::test]
    async fn late_start_response_does_not_resurrect_a_completed_turn() {
        let mut h = Harness::new().await;
        h.enqueue("first").await;
        h.driver.submit(0, false).await.unwrap();
        let request = h.request().await;
        h.complete("completed").await;
        h.event(json!({"id":request["id"],"result":{"turn":{"id":"turn-1","status":"inProgress"}}}))
            .await;
        assert!(h.driver.state.active.is_none());
        assert!(h.driver.state.queue.is_empty());
        assert_eq!(h.driver.state.injection.as_ref().unwrap().state, InjectionRunState::Ok);
    }

    #[tokio::test]
    async fn queue_and_pending_rpc_are_durable_before_acknowledgement() {
        let mut h = Harness::new().await;
        h.start().await;
        h.enqueue("next").await;
        h.driver.submit(0, true).await.unwrap();
        let request = h.request().await;
        let saved = h.driver.runs.host.journal.read().pop().unwrap().decode::<CodexRun>().unwrap().payload.codex.unwrap();
        assert_eq!(saved.queue[0].request.as_deref(), request["id"].as_str());
        // Reconstruct exactly the state adoption starts with, then consume the
        // response produced while remote-hand was disconnected.
        h.driver.state = serde_json::from_value(serde_json::to_value(saved).unwrap()).unwrap();
        h.event(json!({"id":request["id"],"result":{"turnId":"turn-1"}})).await;
        assert!(h.driver.runs.host.journal.read().remove(0).decode::<CodexRun>().unwrap().payload.codex.as_ref().unwrap().queue.is_empty());
        assert_eq!(h.driver.state.active.as_deref(), Some("turn-1"));
    }

    #[tokio::test]
    async fn failed_turn_pauses_queue_and_other_thread_events_are_ignored() {
        let mut h = Harness::new().await;
        h.start().await;
        h.enqueue("next").await;
        h.event(json!({"method":"turn/completed","params":{"threadId":"other-thread","turn":{"id":"turn-1","status":"completed"}}}))
            .await;
        assert!(h.driver.state.active.is_some());
        h.complete("failed").await;
        assert!(h.driver.state.paused);
        assert_eq!(h.driver.state.queue[0].text, "next");
    }

    #[tokio::test]
    async fn unexpected_approval_requests_are_rejected_not_approved() {
        let mut h = Harness::new().await;
        h.event(json!({"id":42,"method":"item/commandExecution/requestApproval","params":{}}))
            .await;
        let response = h.request().await;
        assert_eq!(response["id"], 42);
        assert_eq!(response["error"]["code"], -32601);
        assert!(response.get("result").is_none());
        // Byte for byte, with the request's id echoed in the type it came in.
        h.event(json!({"id":"req-7","method":"item/fileChange/requestApproval","params":{}}))
            .await;
        assert_eq!(
            h.raw().await,
            wire(r#"{"id":"req-7","error":{"code":-32601,"message":"interactive requests are not supported by Droppen"}}"#, &[])
        );
        h.event(json!({"id":43,"method":"item/commandExecution/requestApproval","params":{}}))
            .await;
        assert_eq!(
            h.raw().await,
            wire(r#"{"id":43,"error":{"code":-32601,"message":"interactive requests are not supported by Droppen"}}"#, &[])
        );
    }

    #[tokio::test]
    async fn dead_process_retains_queue_and_allows_explicit_withdrawal() {
        let mut h = Harness::new().await;
        let id = h.enqueue("possibly delivered").await;
        h.driver.submit(0, false).await.unwrap();
        h.request().await;
        let record = h.driver.runs.host.journal.read().pop().unwrap().decode().unwrap();
        h.driver.runs.codex_dead(record);
        let handle = h.driver.runs.codex_handle("test-thread").unwrap();
        assert!(!handle.alive());
        assert!(handle.visible());
        assert_eq!(handle.info().queued, 1);
        assert!(!handle.info().queued_messages[0].steered, "withdrawal is safe once the process is dead");
        assert!(h.driver.runs.host.journal.read().remove(0).decode::<CodexRun>().unwrap().payload.codex.as_ref().unwrap().queue[0].uncertain);
        assert_eq!(h.driver.runs.codex_queue_action("test-thread", &id, false).await, QueueOutcome::Done);
        assert!(h.driver.runs.host.journal.read().remove(0).decode::<CodexRun>().unwrap().payload.codex.as_ref().unwrap().queue.is_empty());
    }

    #[tokio::test]
    async fn acknowledgement_timeout_never_resubmits_and_late_ack_still_reconciles() {
        let mut h = Harness::new().await;
        h.enqueue("first").await;
        h.driver.submit(0, false).await.unwrap();
        let request = h.request().await;
        h.driver.pending_since = Instant::now() - RPC_TIMEOUT - Duration::from_secs(1);
        h.driver.tick().await.unwrap();
        assert!(h.driver.state.paused);
        assert!(h.driver.state.queue[0].request.is_some());
        h.event(json!({"id":request["id"],"result":{"turn":{"id":"turn-1","status":"inProgress"}}}))
            .await;
        assert!(h.driver.state.queue.is_empty());
        assert_eq!(h.driver.state.active.as_deref(), Some("turn-1"));
        assert_eq!(h.driver.state.injection.as_ref().unwrap().state, InjectionRunState::Running);
    }

    #[tokio::test]
    async fn steering_cannot_change_model_or_drop_a_written_message() {
        let mut h = Harness::new().await;
        h.start().await;
        let id = h.enqueue("use different model").await;
        h.driver.state.queue[0].params["model"] = json!("other-model");
        let (reply, rx) = oneshot::channel();
        h.driver
            .command(Cmd::Queue {
                id: id.clone(),
                steer: true,
                reply,
            })
            .await
            .unwrap();
        assert!(matches!(rx.await.unwrap(), QueueOutcome::Refused { .. }));
        h.driver.state.queue[0].params.as_object_mut().unwrap().remove("model");
        h.driver.submit(0, true).await.unwrap();
        h.request().await;
        let (reply, rx) = oneshot::channel();
        h.driver.command(Cmd::Queue { id, steer: false, reply }).await.unwrap();
        assert_eq!(rx.await.unwrap(), QueueOutcome::AlreadySteered);
    }

    /// Codex gets the same receipts: a named message is queued until codex
    /// acknowledges the `turn/start` that carries it, sent once it has, and
    /// failed when it is withdrawn instead.
    #[tokio::test]
    async fn a_named_messages_receipt_follows_it_into_codex_and_out_of_the_queue() {
        use droppen_provider::receipts::{Claim, Kind};
        use droppen_sessions::delivery::ReceiptStatus;
        let mut h = Harness::new().await;
        let receipts = h.driver.runs.host.receipts().clone();
        let named = |text: &str| {
            let client = new_uuid();
            let now = droppen_provider::receipts::now_ms();
            let Claim::Owned { qid } = receipts.claim(&client, Kind::Message, Source::Codex, Some("test-thread"), now).unwrap() else {
                panic!("a fresh id is owned");
            };
            receipts.complete(&client, "{}", None, now);
            let options = SendOptions {
                message_id: Some(qid.clone()),
                ..SendOptions::default()
            };
            let message = message(text, &options).unwrap();
            assert_eq!(message.id, qid, "the message travels under the queue id the claim chose");
            message
        };
        let status = |receipts: &droppen_provider::receipts::Receipts, qid: &str| {
            receipts
                .for_session(Source::Codex, "test-thread", droppen_provider::receipts::now_ms())
                .into_iter()
                .find(|receipt| receipt.qid.0.as_deref() == Some(qid))
                .map(|receipt| receipt.status)
        };

        let first = named("first");
        let first_id = first.id.clone();
        let (tx, rx) = oneshot::channel();
        h.driver.command(Cmd::Deliver(first, tx)).await.unwrap();
        rx.await.unwrap().unwrap();
        h.driver.submit(0, false).await.unwrap();
        let request = h.request().await;
        assert_eq!(request["params"]["clientUserMessageId"], first_id.as_str());
        assert_eq!(status(&receipts, &first_id), Some(ReceiptStatus::Queued), "written, not yet acknowledged");
        h.event(json!({"id":request["id"],"result":{"turn":{"id":"turn-1","status":"inProgress"}}}))
            .await;
        assert_eq!(status(&receipts, &first_id), Some(ReceiptStatus::Sent));

        let second = named("withdrawn");
        let second_id = second.id.clone();
        let (tx, rx) = oneshot::channel();
        h.driver.command(Cmd::Deliver(second, tx)).await.unwrap();
        rx.await.unwrap().unwrap();
        let (reply, rx) = oneshot::channel();
        h.driver
            .command(Cmd::Queue {
                id: second_id.clone(),
                steer: false,
                reply,
            })
            .await
            .unwrap();
        assert_eq!(rx.await.unwrap(), QueueOutcome::Done);
        assert_eq!(status(&receipts, &second_id), Some(ReceiptStatus::Failed));
    }
    /// A registry whose `codex` is `tests/support/fake_codex.py`, which logs
    /// every line it reads to `codex-stdin.jsonl` in its working directory.
    /// `home` becomes the test root the fake insists on; the guard puts the
    /// child `PATH` back when it goes.
    fn fake_codex_registry(home: &std::path::Path, instance: &str) -> (droppen_base::process_env::ChildPathGuard, Arc<InjectionRegistry>) {
        let guard = droppen_base::process_env::ChildPathGuard::take();
        // The stand-in app server runs only under a test root.
        std::fs::write(home.join(".rh-test-root"), b"").expect("test root");
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).expect("bin");
        let codex = bin.join("codex");
        let fake = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/fake_codex.py");
        std::fs::write(&codex, format!("#!/bin/sh\nexec python3 {fake} \"$@\"\n")).expect("codex");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let path = format!("{}:{}", bin.display(), droppen_base::process_env::child_path());
        droppen_base::process_env::set_child_path(Some(path));
        let paths = Arc::new(Paths::new(home.into()));
        let dirs = InstanceDirs::new(&paths.data_dir(), instance).unwrap();
        dirs.create().unwrap();
        let accounts = Arc::new(droppen_accounts::store::AccountsStore::new(paths.clone(), logins()));
        let quota = Arc::new(droppen_accounts::quota::QuotaCache::new(paths.clone(), logins()));
        let decoration = droppen_provider::testing::no_decoration();
        (guard, InjectionRegistry::new(paths, &dirs, accounts, quota, decoration))
    }

    /// The Codex runs `providers` hold.
    fn codex(providers: &droppen_provider::ProviderRegistry) -> &Arc<CodexRuns> {
        providers.provider::<Codex>().expect("Codex").runs()
    }

    /// A step of a spawn, as [`droppen_provider::testing::Step`] records it.
    fn step(what: &'static str, gate: bool, claim: Option<bool>) -> droppen_provider::testing::Step {
        droppen_provider::testing::Step { what, gate, claim }
    }

    /// The spawn gate only reserves a Codex session; its app server starts
    /// outside it. A new thread takes the gate only to take its permit, and is
    /// claimed once the app server has named it. A thread this server has no process for is
    /// reserved under the gate, then claimed and started outside it. A process
    /// retired for another account is retired outside the gate too, and lets
    /// its claim go before it answers. An unloaded session is archived with
    /// `codex archive` under the gate and its claim, and the claim goes first.
    /// A resume the app server answers with another thread lets its claim go
    /// before the app server's stdin closes.
    #[tokio::test]
    async fn codex_reserves_under_the_gate_and_starts_and_retires_outside_it() {
        const RESUMED: &str = "0199c1a1-0000-4000-8000-000000000001";
        const UNLOADED: &str = "0199c1a1-0000-4000-8000-000000000002";
        const MISMATCHED: &str = "0199c1a1-0000-4000-8000-000000000003";
        const OTHER: &str = "0199c1a1-0000-4000-8000-000000000004";
        let home = tempfile::tempdir().expect("tempdir");
        let (_path, registry) = fake_codex_registry(home.path(), "codex-steps");
        let providers = codex_only(&registry);
        let gate = providers.gate();
        let work = home.path().join("work");
        std::fs::create_dir_all(&work).expect("work");
        let work = work.to_string_lossy().into_owned();
        let plain = SendOptions::default();

        let (id, _) = codex(&providers).codex_deliver(gate, None, Some(&work), "hello", &plain, None).await.expect("a new thread");
        assert_eq!(
            registry.host.steps(),
            [
                step("codex started", false, None),
                step("codex thread", false, Some(true)),
                step("codex mapped", false, Some(true)),
            ]
        );
        assert!(!registry.host.gate_held());
        assert!(registry.host.claim_held(Source::Codex, &id), "the driver holds the claim");

        for _ in 0..500 {
            if codex(&providers).codex_handle(&id).is_some_and(|handle| !handle.busy()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let other = CodexAccount {
            id: "another".to_string(),
            launch: droppen_accounts::store::AccountLaunch {
                provider: AccountProvider::Codex,
                env: vec![],
                codex_args: vec![],
            },
        };
        codex(&providers).codex_deliver(gate, Some(&id), Some(&work), "again", &plain, Some(other))
            .await
            .expect("resumed under the other account");
        assert_eq!(
            registry.host.steps()[3..],
            [
                // The old process still holds the session until it retires.
                step("codex reserved", true, Some(true)),
                step("codex retired", false, Some(false)),
                step("codex started", false, Some(true)),
                step("codex thread", false, Some(true)),
                step("codex mapped", false, Some(true)),
            ]
        );

        codex(&providers).codex_deliver(gate, Some(RESUMED), Some(&work), "resume", &plain, None).await.expect("resumed");
        assert_eq!(
            registry.host.steps()[8..],
            [
                step("codex reserved", true, Some(false)),
                step("codex started", false, Some(true)),
                step("codex thread", false, Some(true)),
                step("codex mapped", false, Some(true)),
            ]
        );

        codex(&providers).archive_codex(gate, UNLOADED).await.expect("archived");
        assert_eq!(registry.host.steps()[12..], [step("codex archive", true, Some(true))]);
        assert!(!registry.host.gate_held());
        assert!(!registry.host.claim_held(Source::Codex, UNLOADED), "the claim went with the archive");

        let elsewhere = home.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).expect("elsewhere");
        std::fs::write(elsewhere.join("resume-thread.txt"), OTHER).expect("resume answer");
        let elsewhere = elsewhere.to_string_lossy().into_owned();
        let refused = codex(&providers).codex_deliver(gate, Some(MISMATCHED), Some(&elsewhere), "resume", &plain, None)
            .await
            .expect_err("another thread is refused");
        assert_eq!(refused.to_string(), "Codex resumed a different thread");
        assert_eq!(
            registry.host.steps()[13..],
            [
                step("codex reserved", true, Some(false)),
                step("codex started", false, Some(true)),
                step("codex setup failed", false, Some(false)),
            ]
        );
        assert!(!registry.host.gate_held());
        assert!(!registry.host.claim_held(Source::Codex, MISMATCHED));

        for thread in [id.as_str(), RESUMED] {
            if let Some(handle) = codex(&providers).codex_handle(thread) {
                for _ in 0..500 {
                    if !handle.busy() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                retire(&handle).await.expect("retired");
            }
        }
    }

    /// A scratch home for the tests of a start outside the gate: the fake
    /// codex's registry and providers, and its working directory.
    fn unheld(name: &str) -> (tempfile::TempDir, droppen_base::process_env::ChildPathGuard, Arc<InjectionRegistry>, Arc<droppen_provider::ProviderRegistry>, std::path::PathBuf) {
        let home = tempfile::tempdir().expect("tempdir");
        let (path, registry) = fake_codex_registry(home.path(), name);
        let providers = codex_only(&registry);
        let work = home.path().join("work");
        std::fs::create_dir_all(&work).expect("work");
        (home, path, registry, providers, work)
    }

    /// Wait until the steps taken so far include `what` `times` times.
    async fn wait_for_step(registry: &InjectionRegistry, what: &str, times: usize) {
        for _ in 0..1000 {
            if registry.host.steps().iter().filter(|step| step.what == what).count() >= times {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no {what:?} ({times}) in {:?}", registry.host.steps());
    }

    /// Every app server's `turn/start` and `turn/steer` texts, as codex read them, in order.
    fn sent_texts(work: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(work.join("codex-stdin.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("json"))
            .filter(|request| matches!(request["method"].as_str(), Some("turn/start" | "turn/steer")))
            .filter_map(|request| request["params"]["input"][0]["text"].as_str().map(str::to_owned))
            .collect()
    }

    /// Retire every app server the test left running.
    async fn retire_all(runs: &CodexRuns, ids: &[&str]) {
        for id in ids {
            if let Some(handle) = runs.codex_handle(id).filter(Handle::alive) {
                for _ in 0..500 {
                    if !handle.busy() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                retire(&handle).await.expect("retired");
            }
        }
    }

    /// An app server resuming a thread starts outside the spawn gate: another
    /// send takes the gate while it is held at its first answer. The session is
    /// reserved meanwhile, as starting, and a second message for it waits for
    /// that app server rather than starting another: one start, both messages
    /// delivered, in the order they were sent.
    #[tokio::test]
    async fn a_codex_start_holds_no_gate_and_takes_what_is_sent_to_it_meanwhile() {
        const RESUMED: &str = "0199c1a1-0000-4000-8000-000000000005";
        let (_home, _path, registry, providers, work) = unheld("codex-unheld");
        std::fs::write(work.join("init-hold"), "").expect("hold");
        let runs = codex(&providers).clone();
        let send = |text: &'static str| {
            let (runs, providers, work) = (runs.clone(), providers.clone(), work.to_string_lossy().into_owned());
            tokio::spawn(async move { runs.codex_deliver(providers.gate(), Some(RESUMED), Some(&work), text, &SendOptions::default(), None).await })
        };
        let first = send("first");
        wait_for_step(&registry, "codex started", 1).await;
        assert!(!registry.host.gate_held(), "the start holds no gate");
        drop(tokio::time::timeout(Duration::from_secs(1), providers.gate().permit()).await.expect("another send takes the gate meanwhile"));
        assert_eq!(runs.codex_handle(RESUMED).expect("reserved").state().phase, Phase::Starting);

        let second = send("second");
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        std::fs::remove_file(work.join("init-hold")).expect("release");
        first.await.expect("joined").expect("the first is delivered");
        second.await.expect("joined").expect("the second is delivered");
        assert_eq!(registry.host.steps().iter().filter(|step| step.what == "codex started").count(), 1, "one app server");
        for _ in 0..500 {
            if sent_texts(&work).len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(sent_texts(&work), ["first", "second"]);
        retire_all(&runs, &[RESUMED]).await;
    }

    /// A new thread holds the gate only to take its permit: while its app
    /// server is held at its first answer, another send takes the gate.
    #[tokio::test]
    async fn a_new_codex_thread_starts_outside_the_gate() {
        let (_home, _path, registry, providers, work) = unheld("codex-new-unheld");
        std::fs::write(work.join("init-hold"), "").expect("hold");
        let start = {
            let (runs, providers, work) = (codex(&providers).clone(), providers.clone(), work.to_string_lossy().into_owned());
            tokio::spawn(async move { runs.codex_deliver(providers.gate(), None, Some(&work), "hello", &SendOptions::default(), None).await })
        };
        wait_for_step(&registry, "codex started", 1).await;
        assert!(!registry.host.gate_held(), "the start holds no gate");
        drop(tokio::time::timeout(Duration::from_secs(1), providers.gate().permit()).await.expect("another send takes the gate meanwhile"));
        std::fs::remove_file(work.join("init-hold")).expect("release");
        let (id, _) = start.await.expect("joined").expect("started");
        assert!(registry.host.claim_held(Source::Codex, &id), "claimed once codex named it");
        retire_all(codex(&providers), &[&id]).await;
    }

    /// A start that fails gives the session back as it found it: no handle,
    /// no claim, and a message sent to it meanwhile is answered, not kept.
    #[tokio::test]
    async fn a_codex_start_that_fails_gives_the_session_back() {
        const MISMATCHED: &str = "0199c1a1-0000-4000-8000-000000000006";
        let (_home, _path, registry, providers, work) = unheld("codex-unheld-fails");
        std::fs::write(work.join("resume-thread.txt"), "0199c1a1-0000-4000-8000-000000000007").expect("resume answer");
        std::fs::write(work.join("init-hold"), "").expect("hold");
        let runs = codex(&providers).clone();
        let send = |text: &'static str| {
            let (runs, providers, work) = (runs.clone(), providers.clone(), work.to_string_lossy().into_owned());
            tokio::spawn(async move { runs.codex_deliver(providers.gate(), Some(MISMATCHED), Some(&work), text, &SendOptions::default(), None).await })
        };
        let first = send("first");
        wait_for_step(&registry, "codex started", 1).await;
        let second = send("second");
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        std::fs::remove_file(work.join("init-hold")).expect("release");
        assert_eq!(first.await.expect("joined").expect_err("refused").to_string(), "Codex resumed a different thread");
        second.await.expect("joined").expect_err("the message sent meanwhile is answered");
        assert!(runs.codex_handle(MISMATCHED).is_none(), "no handle is left for the session");
        assert!(!registry.host.claim_held(Source::Codex, MISMATCHED), "nor its claim");
    }

    /// An archive of a session whose app server is still starting is refused
    /// at once: waiting for the start would hold the gate as long.
    #[tokio::test]
    async fn archive_refuses_a_codex_session_still_starting() {
        const RESUMED: &str = "0199c1a1-0000-4000-8000-000000000008";
        let (_home, _path, registry, providers, work) = unheld("codex-unheld-archive");
        std::fs::write(work.join("init-hold"), "").expect("hold");
        let runs = codex(&providers).clone();
        let start = {
            let (runs, providers, work) = (runs.clone(), providers.clone(), work.to_string_lossy().into_owned());
            tokio::spawn(async move { runs.codex_deliver(providers.gate(), Some(RESUMED), Some(&work), "hello", &SendOptions::default(), None).await })
        };
        wait_for_step(&registry, "codex started", 1).await;
        let refused = tokio::time::timeout(Duration::from_secs(1), runs.archive_codex(providers.gate(), RESUMED))
            .await
            .expect("refused at once")
            .expect_err("refused");
        assert!(refused.to_string().contains("still starting"), "{refused}");
        std::fs::remove_file(work.join("init-hold")).expect("release");
        start.await.expect("joined").expect("delivered");
        retire_all(&runs, &[RESUMED]).await;
    }

    /// A message into a running app server is handed over under the gate and
    /// waited for outside it: a driver whose loop is stuck (in a write to its
    /// app server, say) answers late, and nothing else waits for that.
    #[tokio::test]
    async fn a_send_into_a_running_app_server_waits_for_its_answer_outside_the_gate() {
        let (_home, _path, _registry, providers, _work) = unheld("codex-unheld-running");
        let runs = codex(&providers).clone();
        // A driver that takes nothing off its channel.
        let (tx, stuck) = mpsc::unbounded_channel();
        let handle = Handle {
            tx,
            shared: Arc::new(Mutex::new(State::default())),
            account: None,
            starting: Default::default(),
        };
        runs.codex_servers.lock().unwrap().insert("stuck-thread".into(), handle);
        let send = {
            let (runs, providers) = (runs.clone(), providers.clone());
            tokio::spawn(async move { runs.codex_deliver(providers.gate(), Some("stuck-thread"), None, "again", &SendOptions::default(), None).await })
        };
        for _ in 0..500 {
            if !stuck.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!stuck.is_empty(), "the message was handed to the driver");
        drop(tokio::time::timeout(Duration::from_secs(1), providers.gate().permit()).await.expect("the gate is free while the driver answers"));
        assert!(!send.is_finished(), "the send is still waiting for its answer");
        drop(stuck);
        send.await.expect("joined").expect_err("a driver that went away answers nothing");
    }

    /// A message sent while a session moves to another account waits for the
    /// app server it moves to: the old one, retiring, leaves the new one's
    /// reservation where it is.
    #[tokio::test]
    async fn a_send_during_an_account_switch_waits_for_the_new_app_server() {
        let (_home, _path, registry, providers, work) = unheld("codex-unheld-switch");
        let work_ = work.to_string_lossy().into_owned();
        let runs = codex(&providers).clone();
        let (id, _) = runs.codex_deliver(providers.gate(), None, Some(&work_), "hello", &SendOptions::default(), None).await.expect("a new thread");
        for _ in 0..500 {
            if runs.codex_handle(&id).is_some_and(|handle| !handle.busy()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let other = || CodexAccount {
            id: "another".to_string(),
            launch: droppen_accounts::store::AccountLaunch {
                provider: AccountProvider::Codex,
                env: vec![],
                codex_args: vec![],
            },
        };
        std::fs::write(work.join("init-hold"), "").expect("hold");
        let send = |text: &'static str| {
            let (runs, providers, work, id, account) = (runs.clone(), providers.clone(), work_.clone(), id.clone(), other());
            tokio::spawn(async move { runs.codex_deliver(providers.gate(), Some(&id), Some(&work), text, &SendOptions::default(), Some(account)).await })
        };
        let switched = send("switched");
        wait_for_step(&registry, "codex started", 2).await;
        let after = send("after");
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        std::fs::remove_file(work.join("init-hold")).expect("release");
        switched.await.expect("joined").expect("delivered under the other account");
        after.await.expect("joined").expect("delivered into the same app server");
        assert_eq!(registry.host.steps().iter().filter(|step| step.what == "codex started").count(), 2, "the first and the switched one");
        retire_all(&runs, &[&id]).await;
    }

    /// A session whose app server restarts for a new message, here under
    /// another account, reads as working while the new one starts, and still
    /// says how its last turn ended: the reserved handle takes that from the
    /// one it replaces, until the new app server runs the next turn.
    #[tokio::test]
    async fn the_last_turn_stays_on_view_while_a_codex_app_server_restarts_for_a_message() {
        let (_home, _path, registry, providers, work) = unheld("codex-unheld-last-turn");
        let work_ = work.to_string_lossy().into_owned();
        let runs = codex(&providers).clone();
        let (id, _) = runs
            .codex_deliver(providers.gate(), None, Some(&work_), "hello", &SendOptions::default(), None)
            .await
            .expect("a new thread");
        for _ in 0..500 {
            if runs.codex_handle(&id).is_some_and(|handle| !handle.busy()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let shown = |injection: Option<Injection>| injection.map(|injection| (injection.state, injection.started_at, injection.finished_at.0));
        let last = shown(providers.injection_for(Source::Codex, &id));
        assert_eq!(last.as_ref().map(|last| last.0), Some(InjectionRunState::Ok), "the first turn ended");

        std::fs::write(work.join("init-hold"), "").expect("hold");
        let other = CodexAccount {
            id: "another".to_string(),
            launch: droppen_accounts::store::AccountLaunch {
                provider: AccountProvider::Codex,
                env: vec![],
                codex_args: vec![],
            },
        };
        let switched = {
            let (runs, providers, work, id) = (runs.clone(), providers.clone(), work_.clone(), id.clone());
            tokio::spawn(async move {
                runs.codex_deliver(providers.gate(), Some(&id), Some(&work), "again", &SendOptions::default(), Some(other))
                    .await
            })
        };
        wait_for_step(&registry, "codex started", 2).await;

        let resident = providers.resident_info(Source::Codex, &id).expect("the starting app server is on view");
        assert_eq!(resident.phase, droppen_sessions::ResidentPhase::Starting);
        assert_eq!(
            resident.activity(),
            droppen_sessions::SessionActivity::Working,
            "at work on the message, not idle"
        );
        assert_eq!(shown(providers.injection_for(Source::Codex, &id)), last, "the last turn stays on view");

        std::fs::remove_file(work.join("init-hold")).expect("release");
        switched.await.expect("joined").expect("delivered under the other account");
        retire_all(&runs, &[&id]).await;
    }

    /// Dropping or steering a queued message of a session whose app server is
    /// still starting is refused at once: its driver takes nothing off the
    /// queue until codex has answered, and waiting for it would leave the
    /// request hanging for the whole start.
    #[tokio::test]
    async fn queue_actions_on_a_starting_codex_session_are_refused_at_once() {
        const RESUMED: &str = "0199c1a1-0000-4000-8000-000000000009";
        let (_home, _path, registry, providers, work) = unheld("codex-unheld-queue");
        std::fs::write(work.join("init-hold"), "").expect("hold");
        let runs = codex(&providers).clone();
        let start = {
            let (runs, providers, work) = (runs.clone(), providers.clone(), work.to_string_lossy().into_owned());
            tokio::spawn(async move { runs.codex_deliver(providers.gate(), Some(RESUMED), Some(&work), "hello", &SendOptions::default(), None).await })
        };
        wait_for_step(&registry, "codex started", 1).await;
        for steer in [false, true] {
            let outcome = tokio::time::timeout(Duration::from_secs(1), runs.codex_queue_action(RESUMED, "q-1", steer))
                .await
                .expect("refused at once");
            assert!(
                matches!(&outcome, QueueOutcome::Refused { reason } if reason.contains("still starting")),
                "steer {steer}: {outcome:?}"
            );
        }
        std::fs::remove_file(work.join("init-hold")).expect("release");
        start.await.expect("joined").expect("delivered");
        retire_all(&runs, &[RESUMED]).await;
    }

    /// A message for another account while a session's app server starts is
    /// refused at once, as one would be while it runs a turn: the start is the
    /// account the session is moving to.
    #[tokio::test]
    async fn a_send_for_another_account_during_a_start_is_refused() {
        const RESUMED: &str = "0199c1a1-0000-4000-8000-00000000000a";
        let (_home, _path, registry, providers, work) = unheld("codex-unheld-other");
        std::fs::write(work.join("init-hold"), "").expect("hold");
        let runs = codex(&providers).clone();
        let work_ = work.to_string_lossy().into_owned();
        let start = {
            let (runs, providers, work) = (runs.clone(), providers.clone(), work_.clone());
            tokio::spawn(async move { runs.codex_deliver(providers.gate(), Some(RESUMED), Some(&work), "hello", &SendOptions::default(), None).await })
        };
        wait_for_step(&registry, "codex started", 1).await;
        let other = CodexAccount {
            id: "another".to_string(),
            launch: droppen_accounts::store::AccountLaunch {
                provider: AccountProvider::Codex,
                env: vec![],
                codex_args: vec![],
            },
        };
        let refused = tokio::time::timeout(
            Duration::from_secs(1),
            runs.codex_deliver(providers.gate(), Some(RESUMED), Some(&work_), "elsewhere", &SendOptions::default(), Some(other)),
        )
        .await
        .expect("refused at once")
        .expect_err("refused");
        assert_eq!(refused.to_string(), "a message is already running for this session");
        std::fs::remove_file(work.join("init-hold")).expect("release");
        start.await.expect("joined").expect("delivered");
        assert_eq!(registry.host.steps().iter().filter(|step| step.what == "codex started").count(), 1, "nothing else was started");
        retire_all(&runs, &[RESUMED]).await;
    }

    /// A Stop while a session's app server starts is taken, not refused: it
    /// waits behind the message, and the turn the message starts is
    /// interrupted as soon as codex says it started. The turn is one the
    /// stand-in runs until it is interrupted.
    #[tokio::test]
    async fn a_stop_while_a_codex_app_server_starts_ends_the_turn_it_starts() {
        const RESUMED: &str = "0199c1a1-0000-4000-8000-00000000000b";
        let (_home, _path, registry, providers, work) = unheld("codex-unheld-stop");
        std::fs::write(work.join("init-hold"), "").expect("hold");
        std::fs::write(work.join("sleep.txt"), "600").expect("a turn that runs until it is stopped");
        let runs = codex(&providers).clone();
        let start = {
            let (runs, providers, work) = (runs.clone(), providers.clone(), work.to_string_lossy().into_owned());
            tokio::spawn(async move {
                runs.codex_deliver(providers.gate(), Some(RESUMED), Some(&work), "hello", &SendOptions::default(), None)
                    .await
            })
        };
        wait_for_step(&registry, "codex started", 1).await;
        assert_eq!(runs.codex_handle(RESUMED).expect("reserved").state().phase, Phase::Starting);
        assert!(
            providers.interrupt(Source::Codex, RESUMED),
            "a Stop during the start is taken, not refused"
        );

        std::fs::remove_file(work.join("init-hold")).expect("release");
        start.await.expect("joined").expect("delivered");
        let ended = || {
            providers
                .injection_for(Source::Codex, RESUMED)
                .filter(|injection| injection.state != InjectionRunState::Running)
        };
        for _ in 0..500 {
            if ended().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let ended = ended().expect("the turn ended");
        assert!(ended.interrupted, "the turn ends as stopped: {ended:?}");
        let asked = std::fs::read_to_string(work.join("codex-stdin.jsonl")).unwrap_or_default();
        assert!(asked.contains(r#""method":"turn/interrupt""#), "codex was asked to interrupt it");
        retire_all(&runs, &[RESUMED]).await;
    }

    /// A session's app server is in the snapshot its driver hands a turn's
    /// alert ([`RunHost::driver_states_of`]), as well as in the one the session
    /// list asks the providers for.
    #[tokio::test]
    async fn a_codex_session_is_in_its_turn_alert_s_snapshot() {
        let home = tempfile::tempdir().expect("tempdir");
        let (_path, registry) = fake_codex_registry(home.path(), "codex-snapshot");
        let alerts = Arc::new(droppen_provider::testing::TurnAlerts::default());
        registry.host.attach_alerts(alerts.clone());
        let providers = codex_only(&registry);
        let work = home.path().join("work");
        std::fs::create_dir_all(&work).expect("work");
        let work = work.to_string_lossy().into_owned();
        let (id, _) = codex(&providers)
            .codex_deliver(providers.gate(), None, Some(&work), "hello", &SendOptions::default(), None)
            .await
            .expect("a new thread");
        assert!(providers.driver_states().resident(Source::Codex, &id).is_some(), "in the providers' snapshot");
        let handle = codex(&providers).codex_handle(&id).expect("its app server");
        for _ in 0..500 {
            if !handle.busy() && !alerts.seen().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let seen = alerts.seen();
        assert!(
            seen.iter().any(|(source, alerted, resident)| *source == Source::Codex && *alerted == id && resident.is_some()),
            "the turn's alert saw its app server: {seen:?}"
        );
        retire(&handle).await.expect("retired");
    }

    /// A thread request's optional members follow the two every thread gets,
    /// in this order: the thread being resumed, the provider an account with a
    /// key of its own injects, and the chat persona; a chat's turn is
    /// read-only. Byte for byte, because the order is on the wire too.
    #[tokio::test]
    async fn a_resumed_chat_under_an_account_names_its_thread_provider_and_persona_in_order() {
        const THREAD: &str = "0199c1a1-0000-4000-8000-000000000005";
        let home = tempfile::tempdir().expect("tempdir");
        let (_path, registry) = fake_codex_registry(home.path(), "codex-wire");
        let providers = codex_only(&registry);
        let work = home.path().join("work");
        std::fs::create_dir_all(&work).expect("work");
        let log = work.join("codex-stdin.jsonl");
        let work = work.to_string_lossy().into_owned();
        let account = CodexAccount {
            id: "own-key".to_string(),
            launch: droppen_accounts::store::AccountLaunch {
                provider: AccountProvider::Codex,
                env: vec![],
                codex_args: crate::login::CodexBaseUrl::parse("https://api.example.test/v1").expect("a base URL").provider_args(),
            },
        };
        let chat = SendOptions {
            chat: true,
            ..SendOptions::default()
        };
        codex(&providers).codex_deliver(providers.gate(), Some(THREAD), Some(&work), "hi", &chat, Some(account))
            .await
            .expect("resumed");

        let mut lines = Vec::new();
        for _ in 0..500 {
            lines = std::fs::read_to_string(&log)
                .unwrap_or_default()
                .lines()
                .map(|line| format!("{line}\n"))
                .collect();
            if lines.len() >= 4 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert_eq!(
            lines[0],
            wire(
                r#"{"id":"UUID","method":"initialize","params":{"clientInfo":{"name":"remote_hand","title":"remote-hand","version":"VERSION"}}}"#,
                &[("UUID", &id_of(&lines[0])), ("VERSION", droppen_http::PACKAGE_VERSION)]
            )
        );
        assert_eq!(lines[1], wire(r#"{"method":"initialized","params":{}}"#, &[]));
        assert_eq!(
            lines[2],
            wire(
                r#"{"id":"UUID","method":"thread/resume","params":{"cwd":"WORK","approvalPolicy":"never","threadId":"THREAD","modelProvider":"remote_hand","baseInstructions":"PERSONA"}}"#,
                &[("UUID", &id_of(&lines[2])), ("WORK", &work), ("THREAD", THREAD), ("PERSONA", CHAT_SYSTEM_PROMPT)]
            )
        );
        let qid = id_of(&lines[3]).strip_prefix("start:").expect("a turn's id").to_owned();
        assert_eq!(
            lines[3],
            wire(
                r#"{"id":"start:QID","method":"turn/start","params":{"input":[{"type":"text","text":"hi"}],"approvalPolicy":"never","sandboxPolicy":{"type":"readOnly"},"threadId":"THREAD","clientUserMessageId":"QID"}}"#,
                &[("QID", &qid), ("THREAD", THREAD)]
            )
        );

        let handle = codex(&providers).codex_handle(THREAD).expect("the resident");
        for _ in 0..500 {
            if !handle.busy() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        retire(&handle).await.expect("retired");
    }

    /// A thread request names its directory as a JSON string, which a path
    /// that is not UTF-8 cannot be written as: such a session is refused
    /// before codex is started, and its claim goes with the refusal. Linux
    /// only, because macOS will not create a directory under such a name.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_working_directory_that_is_not_utf8_is_refused_before_codex_starts() {
        use std::os::unix::ffi::OsStrExt;
        const THREAD: &str = "0199c1a1-0000-4000-8000-000000000006";
        let root = tempfile::tempdir().expect("tempdir");
        // The send names no directory, so the session runs in the home.
        let home = root.path().join(std::ffi::OsStr::from_bytes(b"\xff"));
        std::fs::create_dir_all(&home).expect("home");
        let (_path, registry) = fake_codex_registry(&home, "codex-utf8");
        let providers = codex_only(&registry);
        let refused = codex(&providers).codex_deliver(providers.gate(), Some(THREAD), None, "hi", &SendOptions::default(), None)
            .await
            .expect_err("refused");
        assert_eq!(refused.to_string(), "the session's working directory is not valid UTF-8");
        assert_eq!(registry.host.steps(), [step("codex reserved", true, Some(false))], "reserved, and codex was not started");
        assert!(codex(&providers).codex_handle(THREAD).is_none(), "the reservation was given back");
        assert!(!registry.host.gate_held());
        assert!(!registry.host.claim_held(Source::Codex, THREAD));
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
        let decoration = droppen_provider::testing::no_decoration();
        let registry = InjectionRegistry::new(
            paths.clone(),
            &dirs,
            Arc::new(droppen_accounts::store::AccountsStore::new(paths.clone(), logins())),
            Arc::new(droppen_accounts::quota::QuotaCache::new(paths, logins())),
            decoration,
        );
        let providers = codex_only(&registry);

        let mut live = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 30"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn a stand-in");
        let adopted = runs::RunRecord {
            run_id: "run-adopted".to_string(),
            source: Source::Codex,
            session_id: SESSION.to_string(),
            cwd: None,
            cli_pid: live.id(),
            started_at: "2026-01-01T00:00:00.000Z".to_string(),
            pid_token: procs::start_token(live.id()),
            resident: false,
            shim: None,
            payload: runs::Payload::new(),
        };

        let codex = providers.provider::<Codex>().expect("Codex").runs().clone();
        let app_server = runs::RunRecord {
            run_id: "run-app-server".to_string(),
            resident: true,
            payload: serde_json::from_value(serde_json::json!({"codex": {"queue": [{"id": "q1", "text": "next", "params": {}, "request": null}]}}))
                .expect("a codex state"),
            ..adopted.clone()
        };
        codex.codex_dead(app_server.decode().expect("a Codex record"));
        let handle = codex.codex_handle(SESSION).expect("the handle stays");
        assert!(handle.visible() && !handle.alive(), "listed, with no process behind it");

        let claim = providers.gate().lease(Source::Codex, SESSION).expect("a free session must be claimable");
        registry.watch_adopted(providers.runs()[&Source::Codex].clone(), adopted, claim);

        assert!(!providers.interrupt(Source::Codex, SESSION), "nothing is left to stop");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(live.try_wait().expect("try_wait").is_none(), "Stop got past the handle and signalled the adopted run");
        let seen = registry.host.run_injection(Source::Codex, SESSION).expect("the adopted run");
        assert_eq!(seen.state, InjectionRunState::Running);
        assert!(!seen.interrupted);

        let _ = live.kill();
        let _ = live.wait();
    }

    /// A Codex row of the run journal, `run_id` and `payload` aside: session
    /// `id`, on this process, nothing else known.
    fn gone_run(run_id: &str, id: &str, payload: runs::Payload) -> runs::RunRecord {
        runs::RunRecord {
            run_id: run_id.to_string(),
            source: Source::Codex,
            session_id: id.to_string(),
            cwd: None,
            cli_pid: std::process::id(),
            started_at: "2026-01-01T00:00:00.000Z".to_string(),
            pid_token: None,
            resident: false,
            shim: None,
            payload,
        }
    }

    /// What the base asks Codex's runs of a row: whether they can read it,
    /// whether it holds state a dead run must keep (an app server's queue and
    /// checkpoint; a `codex exec` run's row holds none), and which CLI to name
    /// when it reports on one.
    #[tokio::test]
    async fn the_runs_say_which_rows_hold_state_and_name_their_cli() {
        let home = tempfile::tempdir().expect("tempdir");
        let registry = droppen_provider::testing::injection_registry(home.path());
        let codex_runs = CodexRuns::new(registry.host.clone());
        let exec = gone_run("run-exec", "0199c1a1-9999-7999-8999-dddddddddddd", runs::Payload::new());
        let app_server = gone_run(
            "run-app-server",
            "0199c1a1-9999-7999-8999-dddddddddddd",
            serde_json::from_value(json!({"codex": {"queue": []}})).expect("a codex state"),
        );
        assert!(codex_runs.readable(&exec) && codex_runs.readable(&app_server), "both are Codex's rows");
        assert!(!codex_runs.holds_state(&exec), "a `codex exec` run keeps nothing");
        assert!(codex_runs.holds_state(&app_server), "an app server keeps its queue and checkpoint");
        assert_eq!(codex_runs.cli_name(), "codex");
    }

    /// What the boot pass asks Codex's runs of a run whose CLI is gone: whether
    /// its thread was cut short mid-turn, and when the thread was last
    /// written. A thread nothing can find counts as cut short, and so does one
    /// whose rollout ends with the user's message; one that ends with Codex's
    /// answer does not.
    #[tokio::test]
    async fn a_gone_runs_thread_says_whether_it_was_cut_short_and_when_it_was_written() {
        const ASKED: &str = "aaaaaaaa-2222-4222-8222-aaaaaaaaaaaa";
        const ANSWERED: &str = "aaaaaaaa-3333-4333-8333-aaaaaaaaaaaa";
        const MISSING: &str = "aaaaaaaa-4444-4444-8444-aaaaaaaaaaaa";
        let _guard = droppen_sessions::index::exclusive().await;
        let home = tempfile::tempdir().expect("tempdir");
        let registry = droppen_provider::testing::injection_registry(home.path());
        let codex_runs = CodexRuns::new(registry.host.clone());
        let day = registry.host.paths.codex_sessions().join("2026").join("09").join("21");
        std::fs::create_dir_all(&day).expect("rollout dir");
        for (id, last) in [(ASKED, "user_message"), (ANSWERED, "task_complete")] {
            let meta = json!({"type": "session_meta", "payload": {"cwd": "/w", "id": id}});
            let event = json!({"type": "event_msg", "payload": {"type": last}});
            std::fs::write(day.join(format!("rollout-2026-09-21T10-00-00-{id}.jsonl")), format!("{meta}\n{event}\n")).expect("rollout");
        }
        let gone = |id: &str| gone_run("run-gone", id, runs::Payload::new());
        assert!(codex_runs.was_cut_short(&gone(ASKED)).await, "the user's message was the last thing written");
        assert!(!codex_runs.was_cut_short(&gone(ANSWERED)).await, "Codex had answered");
        assert!(codex_runs.was_cut_short(&gone(MISSING)).await, "a thread that cannot be found is not known to be done");
        assert!(codex_runs.last_write(ASKED).await.is_some(), "its rollout was written");
        assert!(codex_runs.last_write(MISSING).await.is_none());
    }

    /// An app server that is gone is the session's resident only while it
    /// holds messages: with its queue empty there is nothing to show or to
    /// decide.
    #[tokio::test]
    async fn a_gone_app_server_is_a_resident_only_while_it_holds_messages() {
        const KEPT: &str = "0199c1a1-7777-7777-8777-dddddddddddd";
        const EMPTY: &str = "0199c1a1-8888-7888-8888-dddddddddddd";
        let home = tempfile::tempdir().expect("tempdir");
        let registry = droppen_provider::testing::injection_registry(home.path());
        let providers = codex_only(&registry);
        let codex = providers.provider::<Codex>().expect("Codex");
        let dead = |run_id: &str, id: &str, queue: Value| {
            let payload = serde_json::from_value(json!({"codex": {"queue": queue}})).expect("a codex state");
            gone_run(run_id, id, payload).decode().expect("a Codex record")
        };
        codex.runs().codex_dead(dead("run-kept", KEPT, json!([{"id": "q1", "text": "next", "params": {}, "request": null}])));
        codex.runs().codex_dead(dead("run-empty", EMPTY, json!([])));
        assert!(codex.run().resident(KEPT).is_some_and(|state| state.queued == 1), "its message is still there to decide on");
        assert!(codex.run().resident(EMPTY).is_none(), "nothing is left to show");
    }

    /// A schedule waits for a Codex session's turn: while one runs, the
    /// session is busy for it, and once it is over it is not.
    #[tokio::test]
    async fn a_schedule_waits_while_a_codex_turn_runs() {
        let mut h = Harness::new().await;
        let (handle, _rx) = h.handle();
        h.driver.runs.codex_servers.lock().unwrap().insert("test-thread".into(), handle);
        let providers = h.providers.clone();
        let busy = || providers.provider::<Codex>().expect("Codex").run().schedule_busy("test-thread");
        assert_eq!(busy(), None, "nothing has run yet");
        h.start().await;
        assert_eq!(busy(), Some("the previous run has not finished"));
        h.complete("completed").await;
        assert_eq!(busy(), None, "the turn is over");
    }

    /// A thread started under a stored account is bound to it as a Codex
    /// account, so the next message finds the login this one ran under.
    #[tokio::test]
    async fn a_thread_started_under_an_account_is_bound_to_it() {
        let home = tempfile::tempdir().expect("tempdir");
        let (_path, registry) = fake_codex_registry(home.path(), "codex-bound");
        let providers = codex_only(&registry);
        let account = registry
            .host
            .accounts
            .create(
                "Relay",
                AccountProvider::Codex,
                droppen_accounts::store::Credential::Env(vec![droppen_base::env_block::EnvVar {
                    key: "OPENAI_API_KEY".into(),
                    value: "not-a-real-key".into(),
                }]),
            )
            .await
            .expect("a Codex account");
        let work = home.path().join("work");
        std::fs::create_dir_all(&work).expect("work");
        let options = SendOptions {
            account: AccountChoice::Account(account.id.clone()),
            ..SendOptions::default()
        };
        let codex = providers.provider::<Codex>().expect("Codex");
        let at = StartAt {
            cwd: &work.to_string_lossy(),
            text: "hello",
            options: &options,
        };
        let id = codex.run().start(providers.gate(), at).await.expect("a new thread");
        assert_eq!(registry.host.accounts.session_account_id(&id).await, Some(account.id));
    }
}
