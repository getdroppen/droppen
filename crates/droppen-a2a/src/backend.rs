//! The seam between the protocol and the sessions it drives (`docs/a2a.md` §5).
//!
//! The protocol side — tokens, tasks, streams, the store — is this crate's;
//! running a session is not. [`SessionBackend`] is everything this crate asks
//! of the sessions: start one, send to one, answer its question, cancel a
//! turn, and pick a task up again after a restart. What happens to a task
//! afterwards comes back as [`BackendEvent`]s, through the [`Events`] handle
//! [`event_channel`] hands the backend.
//!
//! The real backend lives in the entry, on top of the provider registry, the
//! `Alerts` hooks and the global feed: it decides which turn a task's message
//! went into and when that turn ended (the task's own turn, §3.2), makes the
//! worktree, and builds the artifacts. [`crate::testing::FakeBackend`] stands
//! in for it in tests, and [`Unwired`] in a build that has none.
//!
//! Every call names the task it is for ([`TaskRef`]), and every event names
//! the task (or, for a session-wide one, the context) it is about. The task's
//! state is this crate's alone: the backend only says what happened, and an
//! event for a task that has already ended is dropped.

use std::collections::BTreeMap;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Which CLI an entry runs: a provider's wire name, or the TCK's stand-in.
///
/// Its own enum rather than the sessions' `Source`, so this crate names no
/// provider; the backend picks its provider by [`EntrySource::as_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, gotcha::Schematic)]
#[serde(rename_all = "lowercase")]
pub enum EntrySource {
    /// Claude Code.
    Claude,
    /// Codex.
    Codex,
    /// An ACP agent; the entry names which (`acpAgent`).
    Acp,
    /// No CLI: the A2A TCK's scenarios, played by [`crate::tck`]. Only an
    /// instance started with `DROPPEN_A2A_TCK=1` may have such an entry, and
    /// the backend never sees one.
    Tck,
}

impl EntrySource {
    /// The wire name: `claude`, `codex`, `acp` or `tck`.
    pub fn as_str(self) -> &'static str {
        match self {
            EntrySource::Claude => "claude",
            EntrySource::Codex => "codex",
            EntrySource::Acp => "acp",
            EntrySource::Tck => "tck",
        }
    }

    /// The source a wire name spells, if any.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "claude" => Some(EntrySource::Claude),
            "codex" => Some(EntrySource::Codex),
            "acp" => Some(EntrySource::Acp),
            "tck" => Some(EntrySource::Tck),
            _ => None,
        }
    }
}

/// Where a new context's session runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, gotcha::Schematic)]
#[serde(rename_all = "lowercase")]
pub enum Isolation {
    /// In the entry's directory itself.
    None,
    /// In a worktree of its own: `<cwd>/.worktrees/a2a-<contextId>`, on the
    /// branch `a2a/<contextId>`, the whole uuid; one that exists already is a
    /// rejection, and none is removed afterwards (§5).
    #[default]
    Worktree,
}

/// Which account a session runs under, as `NewSession.accountId` says it:
/// absent, `null`, or an id.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, gotcha::Schematic)]
#[serde(tag = "kind", content = "id", rename_all = "lowercase")]
pub enum RunAccount {
    /// The instance's default account, as a session started from the page
    /// gets without naming one.
    #[default]
    Default,
    /// The CLI's own login on this machine.
    Local,
    /// A stored account, by its id.
    Id(String),
}

/// How a session of an entry runs: everything of the entry the backend needs,
/// and nothing the message could have set (§0: what runs is decided here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSpec {
    /// Which CLI.
    pub source: EntrySource,
    /// The configured ACP agent, for [`EntrySource::Acp`].
    pub acp_agent: Option<String>,
    /// The entry's directory, absolute.
    pub cwd: String,
    /// The account to run under.
    pub account: RunAccount,
    /// `--model`, or the CLI's default.
    pub model: Option<String>,
    /// Reasoning effort, or the CLI's default.
    pub effort: Option<String>,
    /// Permission / sandbox mode, as `NewSession.permission` takes it.
    pub permission: Option<String>,
    /// Whether each new context gets a worktree.
    pub isolation: Isolation,
}

/// A context's session, as far as it is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRef {
    /// The CLI it runs on.
    pub source: EntrySource,
    /// The CLI's own session id. `None` until the backend learns it: before
    /// [`SessionBackend::start`] answers, or for Codex until `thread/start`
    /// has ([`BackendEvent::SessionIdentified`]).
    pub session_id: Option<String>,
    /// The directory the session runs in: the entry's, or its worktree.
    pub cwd: String,
    /// The worktree's branch, when there is one.
    pub branch: Option<String>,
}

/// The task a call is about, and where it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRef {
    /// The task, a uuid this crate minted.
    pub task_id: String,
    /// Its context, a uuid this crate minted: one per session.
    pub context_id: String,
    /// The entry it came in through.
    pub entry_id: String,
    /// Who sent it: `a2a:token:<token id>` (§7: `a2a:device:<id>`).
    pub caller: String,
    /// The context's session.
    pub session: SessionRef,
}

/// One image a message carried, to go to the session as an attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// `image/png` or `image/jpeg`.
    pub media_type: String,
    /// The file name the part gave, cut down to letters, digits, `.`, `-`
    /// and `_`: a label to show, never a path to write to (§5).
    pub filename: Option<String>,
    /// The bytes.
    pub bytes: Vec<u8>,
}

/// What goes into the session: the entry's template filled in (§5), and the
/// images the message carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    /// The text the session is sent.
    pub text: String,
    /// The images, which the backend stores in the upload directory and sends
    /// as the message's image attachments.
    pub images: Vec<Image>,
}

/// Start a new session for a task that came with no context.
#[derive(Debug, Clone)]
pub struct StartSession {
    /// The task; its `session.session_id` is `None`, and `session.cwd` the
    /// entry's directory.
    pub task: TaskRef,
    /// How the entry runs.
    pub run: RunSpec,
    /// The first message.
    pub prompt: Prompt,
    /// `a2a:<caller>:<messageId>`, the `clientMessageId` the start goes under,
    /// so a client's retry cannot start a second session.
    pub client_message_id: String,
}

/// What a start made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    /// The session's id, when the CLI has named it already.
    pub session_id: Option<String>,
    /// The directory it runs in: the worktree, or the entry's directory.
    pub cwd: String,
    /// The worktree's branch; `None` without one.
    pub branch: Option<String>,
}

/// What a message to a context's session is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendKind {
    /// A new task in the context: its first message, the start of its own
    /// turn. Queued behind the session's current turn while it is busy.
    NewTurn,
    /// More for a task still submitted or working, as the page's "add to the
    /// running turn" sends it. The task's own turn is still the one it waits
    /// for.
    Append,
}

/// Send a message into a context's session.
#[derive(Debug, Clone)]
pub struct SendToSession {
    /// The task, and the session it goes to.
    pub task: TaskRef,
    /// Which kind of message.
    pub kind: SendKind,
    /// The message.
    pub prompt: Prompt,
    /// `a2a:<caller>:<messageId>`.
    pub client_message_id: String,
}

/// One answer to a question: a label, or every label of a multi-select one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnswerValue {
    /// One label, or free text.
    One(String),
    /// Every label chosen.
    Many(Vec<String>),
}

/// An answer to the question a task waits on: `AnswerQuestion{answers, notes,
/// response}` (§3.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The question it answers ([`Question::request_id`]).
    pub request_id: String,
    /// The exact question text → its answer.
    pub answers: BTreeMap<String, AnswerValue>,
    /// The exact question text → a note beside the choice.
    pub notes: BTreeMap<String, String>,
    /// Free text, from the message's text parts.
    pub response: Option<String>,
}

/// One choice of a question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionOption {
    /// What the agent offers.
    pub label: String,
    /// What it means.
    #[serde(default)]
    pub description: String,
}

/// One question, as `AskUserQuestion` asks it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionItem {
    /// The question text, which an answer names it by.
    pub question: String,
    /// Its short header.
    #[serde(default)]
    pub header: String,
    /// Whether more than one option may be chosen.
    #[serde(default)]
    pub multi_select: bool,
    /// The choices.
    #[serde(default)]
    pub options: Vec<QuestionOption>,
}

/// What a task stopped to ask (`SessionQuestion.state = pending`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// The CLI's request id, which an answer echoes.
    pub request_id: String,
    /// The questions, as asked.
    pub questions: Vec<QuestionItem>,
}

/// The `result` artifact of a completed task (§3.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultData {
    /// `claude`, `codex` or `acp`.
    pub source: String,
    /// The session's id.
    pub session_id: String,
    /// This device's name.
    pub host: String,
    /// The directory the session ran in.
    pub cwd: String,
    /// The worktree's branch; `null` outside a git repository.
    pub branch: Option<String>,
    /// What changed in the worktree since the branch began.
    pub changed_files: Vec<String>,
    /// GitHub pull-request links the turn's text named.
    pub pr_urls: Vec<String>,
    /// The session on my.droppen.app; `null` when this device is not linked.
    pub droppen_url: Option<String>,
}

/// What a finished turn produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnResult {
    /// The turn's last assistant reply, markdown: the `summary` artifact.
    pub summary: String,
    /// The `result` artifact.
    pub result: ResultData,
}

/// How a task's turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    /// It finished, with no message queued after it and no background task
    /// waited on: `COMPLETED`, with the two artifacts.
    Completed(TurnResult),
    /// It failed, crashed, or could not start: `FAILED`, with why.
    Failed(String),
    /// A person stopped it in Droppen itself: `CANCELED`, with what to say.
    Canceled(String),
    /// The entry's settings could not run it (the directory is unusable, the
    /// worktree could not be made): `REJECTED`.
    Rejected {
        /// The droppen code to carry, if one applies.
        code: Option<ErrorCode>,
        /// Why, for a person.
        message: String,
    },
}

/// What happened to a task, or to a context's session, as the backend saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendEvent {
    /// The task's message was taken into its turn: `SUBMITTED` → `WORKING`.
    TurnStarted {
        /// The task.
        task_id: String,
    },
    /// The turn's latest assistant sentence or current tool. As often as the
    /// backend likes: the status messages are thinned to one per five seconds
    /// and cut to 280 characters here.
    Progress {
        /// The task.
        task_id: String,
        /// What it is doing.
        text: String,
    },
    /// The turn asks a question: `INPUT_REQUIRED` (§3.4).
    Question {
        /// The task.
        task_id: String,
        /// What it asks.
        question: Question,
    },
    /// The question was answered or withdrawn in Droppen itself: back to
    /// `WORKING`. Not sent for an answer that came through
    /// [`SessionBackend::answer`], which moves the task already.
    QuestionResolved {
        /// The task.
        task_id: String,
    },
    /// The task's turn ended.
    TurnEnded {
        /// The task.
        task_id: String,
        /// How.
        outcome: TurnOutcome,
    },
    /// The CLI named the context's session (Codex after `thread/start`).
    SessionIdentified {
        /// The context.
        context_id: String,
        /// The session's id.
        session_id: String,
    },
    /// The context's session is gone: every unfinished task of it `FAILED`.
    SessionGone {
        /// The context.
        context_id: String,
        /// Why, for a person.
        message: String,
    },
}

/// Where a task stood when [`SessionBackend::resume`] picked it up again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resumed {
    /// Its message is still queued: `SUBMITTED`.
    Submitted,
    /// Its turn is running: `WORKING`.
    Working,
    /// Its turn waits on a question: `INPUT_REQUIRED`.
    InputRequired(Question),
    /// Its turn ended while the server was down.
    Ended(TurnOutcome),
    /// Its session is gone: `FAILED`.
    Gone(String),
}

/// A droppen error code (§3.5), carried beside the A2A error it refines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// `droppen.entry_disabled`: the entry is switched off.
    EntryDisabled,
    /// `droppen.remote_answers_disabled`: the entry takes no answers over A2A.
    RemoteAnswersDisabled,
    /// `droppen.isolation_failed`: the worktree could not be made.
    IsolationFailed,
    /// `droppen.device_offline` (§7).
    DeviceOffline,
    /// `droppen.attestation_stale` (§7).
    AttestationStale,
    /// `droppen.not_accepted` (§7).
    NotAccepted,
    /// `droppen.device_removed` (§7).
    DeviceRemoved,
    /// `droppen.quota_exceeded`: one of the per-caller caps of §5.
    QuotaExceeded,
    /// `droppen.revoked`: the caller's token was revoked, its device taken
    /// out of `allowedDevices`, or the entry switched off.
    Revoked,
    /// `droppen.context_closed`: the entry's source, directory, account,
    /// permission mode or isolation changed since the context was opened
    /// (§6): a follow-up opens a new context.
    ContextClosed,
    /// `droppen.context_not_found`: no such context, or not this caller's on
    /// this entry; the two are not told apart (§6).
    ContextNotFound,
    /// `droppen.push_origin_not_allowed`: a push configuration's URL is not
    /// at an origin the entry's `pushOrigins` lists (§6).
    PushOriginNotAllowed,
}

impl ErrorCode {
    /// The code as clients read it: `droppen.<snake_case>`.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::EntryDisabled => "droppen.entry_disabled",
            ErrorCode::RemoteAnswersDisabled => "droppen.remote_answers_disabled",
            ErrorCode::IsolationFailed => "droppen.isolation_failed",
            ErrorCode::DeviceOffline => "droppen.device_offline",
            ErrorCode::AttestationStale => "droppen.attestation_stale",
            ErrorCode::NotAccepted => "droppen.not_accepted",
            ErrorCode::DeviceRemoved => "droppen.device_removed",
            ErrorCode::QuotaExceeded => "droppen.quota_exceeded",
            ErrorCode::Revoked => "droppen.revoked",
            ErrorCode::ContextClosed => "droppen.context_closed",
            ErrorCode::ContextNotFound => "droppen.context_not_found",
            ErrorCode::PushOriginNotAllowed => "droppen.push_origin_not_allowed",
        }
    }

    /// The `reason` of the `google.rpc.ErrorInfo` that carries it (§3.5):
    /// the code without `droppen.`, in upper snake case.
    pub fn reason(self) -> String {
        self.as_str().trim_start_matches("droppen.").to_ascii_uppercase()
    }
}

/// Why a backend call did not do what it was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// The entry's settings cannot run it (the directory is unusable, the
    /// worktree cannot be made). A start or a new turn is `REJECTED`.
    Rejected {
        /// The droppen code to carry, if one applies.
        code: Option<ErrorCode>,
        /// Why, for a person.
        message: String,
    },
    /// The question is no longer open: it was answered or withdrawn in
    /// Droppen meanwhile.
    QuestionClosed,
    /// The context's session is gone. The task is `FAILED`.
    Gone(String),
    /// What was asked is not one the backend takes — an answer that does not
    /// fit the question's options, say. The caller gets an invalid-params
    /// error and the task stays as it is (an answer's task still waits); a
    /// start or a new turn so refused is `REJECTED`.
    Invalid(String),
    /// Anything else. A start or a new turn is `FAILED`; the caller of an
    /// answer, an append or a cancel gets an internal error.
    Failed(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::Rejected { message, .. } | BackendError::Gone(message) | BackendError::Invalid(message) | BackendError::Failed(message) => {
                f.write_str(message)
            }
            BackendError::QuestionClosed => f.write_str("the question was already answered or withdrawn"),
        }
    }
}

/// The sessions behind the A2A server.
///
/// Every method returns once the backend has *accepted* the request, not once
/// the turn is done: the turn's course comes back as [`BackendEvent`]s. A
/// start may take as long as spawning the CLI does; it runs off the request
/// that asked for it, with the task `SUBMITTED` meanwhile.
pub trait SessionBackend: Send + Sync + 'static {
    /// Whether an entry's settings would run, checked when the entry is saved:
    /// its CLI runs here and takes these flags, its directory is one, its ACP
    /// agent and account exist. `Err` is the sentence a person reads (a `400`).
    fn check_entry<'a>(&'a self, run: &'a RunSpec) -> BoxFuture<'a, Result<(), String>>;

    /// Start a new session for a task that came with no context, through the
    /// same `start_new_session` a `POST /api/sessions/new` takes (§3.1).
    fn start(&self, start: StartSession) -> BoxFuture<'_, Result<Started, BackendError>>;

    /// Send a message into a context's session: a new task's first message,
    /// or more for a running one.
    fn send(&self, send: SendToSession) -> BoxFuture<'_, Result<(), BackendError>>;

    /// Answer the question `task` waits on. On `Ok` the task is `WORKING`
    /// again; no [`BackendEvent::QuestionResolved`] follows.
    fn answer<'a>(&'a self, task: &'a TaskRef, answer: Answer) -> BoxFuture<'a, Result<(), BackendError>>;

    /// Cancel `task`'s turn, keeping the session: withdraw its message while
    /// it is still queued, interrupt the turn once it runs. On `Ok` the task is
    /// `CANCELED`, and what the backend reports of it afterwards is dropped.
    fn cancel<'a>(&'a self, task: &'a TaskRef) -> BoxFuture<'a, Result<(), BackendError>>;

    /// After a restart, follow an unfinished task again and say where it
    /// stands now: from the session's own state, the turn that ended while
    /// the server was down included.
    fn resume<'a>(&'a self, task: &'a TaskRef) -> BoxFuture<'a, Result<Resumed, BackendError>>;
}

/// The handle a backend reports [`BackendEvent`]s through. Cheap to clone;
/// sending never blocks, so it may be called from a synchronous hook.
#[derive(Debug, Clone)]
pub struct Events(mpsc::UnboundedSender<BackendEvent>);

impl Events {
    /// Report what happened. Dropped quietly once the server has stopped
    /// listening.
    pub fn send(&self, event: BackendEvent) {
        let _ = self.0.send(event);
    }
}

/// The receiving end of [`Events`], which [`crate::A2a::new`] takes.
#[derive(Debug)]
pub struct EventInbox(pub(crate) mpsc::UnboundedReceiver<BackendEvent>);

/// A new pair: the [`Events`] handle for the backend, and the inbox for the
/// server. Made before either, because each needs its half to be built.
pub fn event_channel() -> (Events, EventInbox) {
    let (tx, rx) = mpsc::unbounded_channel();
    (Events(tx), EventInbox(rx))
}

/// The backend of a build that has none yet: an entry can be configured, and
/// every task it is handed is rejected.
#[derive(Debug, Default, Clone, Copy)]
pub struct Unwired;

impl Unwired {
    const NOT_WIRED: &'static str = "this build of Droppen runs no sessions for A2A yet";

    /// The backend, shared.
    pub fn shared() -> Arc<dyn SessionBackend> {
        Arc::new(Unwired)
    }
}

impl SessionBackend for Unwired {
    fn check_entry<'a>(&'a self, _run: &'a RunSpec) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn start(&self, _start: StartSession) -> BoxFuture<'_, Result<Started, BackendError>> {
        Box::pin(async {
            Err(BackendError::Rejected {
                code: None,
                message: Self::NOT_WIRED.to_string(),
            })
        })
    }

    fn send(&self, _send: SendToSession) -> BoxFuture<'_, Result<(), BackendError>> {
        Box::pin(async { Err(BackendError::Gone(Self::NOT_WIRED.to_string())) })
    }

    fn answer<'a>(&'a self, _task: &'a TaskRef, _answer: Answer) -> BoxFuture<'a, Result<(), BackendError>> {
        Box::pin(async { Err(BackendError::Gone(Self::NOT_WIRED.to_string())) })
    }

    fn cancel<'a>(&'a self, _task: &'a TaskRef) -> BoxFuture<'a, Result<(), BackendError>> {
        Box::pin(async { Ok(()) })
    }

    fn resume<'a>(&'a self, _task: &'a TaskRef) -> BoxFuture<'a, Result<Resumed, BackendError>> {
        Box::pin(async { Ok(Resumed::Gone(Self::NOT_WIRED.to_string())) })
    }
}
