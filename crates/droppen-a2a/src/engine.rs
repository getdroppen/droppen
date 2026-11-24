//! The tasks: what each A2A operation does to them, and what the backend's
//! events do (`docs/a2a.md` §3).
//!
//! # One writer, one order
//!
//! Every change of a task goes through [`A2a::mutate`], under one lock: the
//! row is read, changed, written with the events the change made (one
//! transaction), and the events are handed to the task's live streams and to
//! the push seam ([`TaskEventSink`]) before the lock is let go. So every
//! stream of a task sees the same events in the same order, and a stream that
//! opens takes its snapshot and its subscription under that same lock: it
//! misses nothing after the snapshot and repeats nothing before it. A stream
//! that falls behind reads what it missed back from the event table.
//!
//! The backend is never called under the lock. A task is first written as
//! submitted, then handed to the backend off the request ([`A2a::pump`]), and
//! what the backend reports comes back through the event loop.
//!
//! # A slot per working task
//!
//! An entry runs at most `maxConcurrent` tasks at once: a task holds a slot
//! from the moment it is handed to the backend until it ends or stops to ask
//! a question. One beyond that stays submitted, not yet handed over, and the
//! oldest such goes when a slot frees. A task whose context has no session
//! yet starts it; one in a context whose session is still being started
//! waits for it, and the tasks after it go first.
//!
//! # Taking back, under the same lock
//!
//! A revocation and a tightened entry (§6) read the tasks they take back
//! under the writer's lock, and a task is made, and picked to be handed over,
//! under it too, after asking whether its caller still reaches the entry and
//! its context is still open. So a task either is refused there or is one
//! they take back: none slips between.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, Weak};
use std::time::{Duration, UNIX_EPOCH};

use serde_json::{json, Value};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use droppen_base::time::{iso8601, now_ms};

use crate::backend::{
    Answer, AnswerValue, BackendError, BackendEvent, EntrySource, ErrorCode, EventInbox, Image, Prompt, Question, Resumed, SendKind, SendToSession,
    SessionBackend, SessionRef, StartSession, TaskRef, TurnOutcome, TurnResult,
};
use crate::entry::{fence, render, A2aEntry};
use crate::error::{A2aError, ErrorKind};
use crate::store::{ContextRow, CreateError, EventRow, NewTask, PushRow, Store, TaskFilter, TaskRow, TokenRow, TASKS_KEPT_MS};
use crate::wire::{
    Artifact, AuthenticationInfo, ListPushConfigsResponse, ListTasksRequest, ListTasksResponse, Message, Part, PartContent, Role, SendMessageRequest,
    StreamResponse, Task, TaskArtifactUpdateEvent, TaskPushNotificationConfig, TaskState, TaskStatus, TaskStatusUpdateEvent,
};

/// How often a working task's status message may change.
const PROGRESS_EVERY: Duration = Duration::from_secs(5);
/// The longest status message of a working task, in characters.
const PROGRESS_CHARS: usize = 280;
/// How many messages a task's history keeps.
const HISTORY_KEPT: usize = 50;
/// The most a task's history keeps once its messages have gone to the
/// backend, in bytes of JSON: the oldest messages go first (§4).
const HISTORY_BYTES: usize = 1024 * 1024;
/// The longest request, in characters of text.
const MAX_REQUEST_CHARS: usize = 100_000;
/// How many images one message may carry.
const MAX_IMAGES: usize = 8;
/// The largest image one message may carry.
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// How many push configurations one task may have (§5).
const MAX_PUSH_CONFIGS: usize = 3;
/// How many tasks of one caller may be queued at once (§5).
const MAX_QUEUED: usize = 20;
/// How many contexts one caller may open in a day (§5).
const MAX_CONTEXTS_PER_DAY: usize = 50;
/// A day, in milliseconds.
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// The largest request body on `/a2a/*` (§5).
pub const MAX_BODY: usize = 10 * 1024 * 1024;
/// The most a task waiting for a slot holds, in bytes of JSON: the messages
/// it takes into its turn once it has one, files and all (§5).
const WAITING_BYTES: usize = MAX_BODY;
/// How many streams one caller may hold open at once (§5).
const MAX_STREAMS: usize = 16;
/// How often what finished long ago is pruned (§4).
const RETENTION_EVERY: Duration = Duration::from_secs(60 * 60);
/// The media types a part may be sent in, by content.
const IMAGE_TYPES: [&str; 2] = ["image/png", "image/jpeg"];
/// How long a token's last use is not written again.
const TOUCH_EVERY_MS: i64 = 60_000;
/// A live stream's buffer, in events; a stream further behind reads back
/// from the event table.
const STREAM_BUFFER: usize = 256;

/// What the server is configured with.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Droppen's version, the Agent Card's `version`.
    pub version: String,
    /// The base of the interfaces' URLs (`a2a.public_base`); the request's
    /// `Host`, over `http`, when not set.
    pub public_base: Option<String>,
    /// `DROPPEN_A2A_TCK=1`: a `tck` entry may be made.
    pub tck: bool,
    /// The TCK's `TCK_STREAMING_TIMEOUT`.
    pub tck_streaming_timeout: Duration,
}

impl Settings {
    /// The settings this process's environment gives, under `version`.
    pub fn from_env(version: &str, public_base: Option<String>) -> Self {
        let timeout = std::env::var("TCK_STREAMING_TIMEOUT")
            .ok()
            .and_then(|value| value.trim().parse::<f64>().ok())
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0 && *seconds < 600.0)
            .unwrap_or(2.0);
        Self {
            version: version.to_string(),
            public_base: public_base
                .map(|base| base.trim().trim_end_matches('/').to_string())
                .filter(|base| !base.is_empty()),
            tck: droppen_base::env::flag("DROPPEN_A2A_TCK"),
            tck_streaming_timeout: Duration::from_secs_f64(timeout),
        }
    }
}

/// What an event is, for the push seam: a state change, a working task's
/// status message (never pushed, §5), or an artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// The state changed.
    Status,
    /// A working task's status message.
    Progress,
    /// An artifact, or a chunk of one.
    Artifact,
}

impl EventKind {
    /// How the event table spells it.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            EventKind::Status => "status",
            EventKind::Progress => "progress",
            EventKind::Artifact => "artifact",
        }
    }
}

/// One event of a task, as it was written.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskEvent {
    /// The task.
    pub task_id: String,
    /// Its context.
    pub context_id: String,
    /// Its entry, whose `pushOrigins` a push is held to.
    pub entry_id: String,
    /// Its number within the task: 1, 2, … in the order written.
    pub seq: i64,
    /// What it is.
    pub kind: EventKind,
    /// The task's state after it.
    pub state: TaskState,
    /// The `StreamResponse`, as streams and pushes carry it.
    pub response: StreamResponse,
}

/// The push seam (M1c, [`crate::push`]): told every event of every task, in
/// order per task, once it is written. Called under the writer's lock, so it
/// must only hand the event on (to a queue of its own) and never call back
/// into [`A2a`]. [`A2a::push_configs`] and [`A2a::entry`] give it the rest;
/// the event table keeps what a retry needs.
pub trait TaskEventSink: Send + Sync + 'static {
    /// One event, written.
    fn task_event(&self, event: &TaskEvent);

    /// A push configuration of `task_id` was kept: events it has not been
    /// told of may be waiting already (one a message came with is told its
    /// task from the first event). The same rules as [`Self::task_event`].
    fn push_added(&self, _task_id: &str) {}

    /// The server took its instance over after a start ([`A2a::resume`]):
    /// what was not delivered before it stopped is due.
    fn resume(&self) {}
}

/// Who a request is: a token that proved itself, on an entry in its scope.
#[derive(Debug, Clone)]
pub struct Caller {
    /// The token's id.
    pub token_id: String,
    /// `a2a:token:<token id>`.
    pub caller: String,
    /// The entry the request is for.
    pub entry: A2aEntry,
}

/// Why a request was not let in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    /// No token, or one that proves nothing: `401`.
    Unauthenticated,
    /// A token from a peer that is not this device: `403`. A2A tokens are
    /// taken over loopback only (§2): never in plain http across a network.
    NotLoopback,
    /// No such entry, or none in the token's scope: `404`, alike.
    NotFound,
}

/// The quota error of §5.
fn over_quota(what: impl Into<String>) -> A2aError {
    A2aError::unsupported(what).with_code(ErrorCode::QuotaExceeded)
}

/// The error for a caller that no longer reaches the entry: its token was
/// revoked, or the entry switched off, while its request was on its way.
fn revoked() -> A2aError {
    A2aError::unsupported("this caller's access was revoked").with_code(ErrorCode::Revoked)
}

/// The error for a follow-up to a closed context (§6).
fn context_closed() -> A2aError {
    A2aError::unsupported("the context is closed: this agent's settings changed since it was opened; leave contextId out to start a new one")
        .with_code(ErrorCode::ContextClosed)
}

/// The error for a context that does not exist, and for one this caller may
/// not reach on this entry alike (§6).
pub(crate) fn context_not_found() -> A2aError {
    A2aError::invalid("contextId is not a context of this agent's: leave it out to start a new one").with_code(ErrorCode::ContextNotFound)
}

/// What a task cancelled for `code` says.
fn canceled_text(code: Option<ErrorCode>) -> &'static str {
    match code {
        Some(ErrorCode::Revoked) => "canceled: the caller's access was revoked",
        Some(ErrorCode::ContextClosed) => "canceled: this agent's settings changed; send it again without its contextId to start a new context",
        _ => "canceled",
    }
}

/// The metadata of a status message that carries a droppen code: the same
/// `ErrorInfo` an error would (§3.5).
fn code_metadata(code: ErrorCode) -> serde_json::Map<String, Value> {
    let error = A2aError::unsupported("").with_code(code);
    let info = error.details().pop().unwrap_or(Value::Null);
    let mut map = serde_json::Map::new();
    map.insert("errorInfo".into(), info);
    map
}

/// What a sent message came to.
pub enum Sent {
    /// A message instead of a task.
    Message(Message),
    /// The task it made or moved.
    Task {
        /// The task, followed from where it stood once the message was taken.
        follow: Box<Follow>,
        /// Whether a unary `SendMessage` waits for it to settle.
        blocking: bool,
    },
}

/// What an event change emits.
enum Emit {
    /// The status, as it stands after the change.
    Status,
    /// The status, as a working task's progress.
    Progress,
    /// A whole artifact (`append = false`, `lastChunk = true`).
    Artifact(Artifact),
    /// A chunk of an artifact.
    Chunk {
        /// The chunk.
        artifact: Artifact,
        /// Whether it adds to what was sent before.
        append: bool,
        /// Whether it is the last.
        last: bool,
    },
}

/// The throttle of one working task's status messages.
#[derive(Default)]
struct ProgressGate {
    last: Option<tokio::time::Instant>,
    pending: Option<String>,
    scheduled: bool,
}

/// What [`A2a::next_job`] hands the backend.
struct Job {
    task: TaskRef,
    run: crate::backend::RunSpec,
    prompt: Prompt,
    client_message_id: String,
    start: bool,
}

/// A task [`A2a::mutate_pick`] handed over.
struct Picked {
    /// The task, marked handed over.
    task: TaskRow,
    /// Its messages as they were taken, files and all; the row keeps
    /// placeholders for the files from now on.
    messages: Vec<Message>,
    /// Its context.
    context: ContextRow,
}

/// Where [`A2a::append`] put a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Appended {
    /// Into the task's messages: it waits for a slot.
    Queued,
    /// Nowhere: a waiting task holds no more (§5).
    Full,
    /// To the backend: the task has gone to it.
    Handed,
}

/// Which rules a new task is made under ([`A2a::create`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Making {
    /// A client's message: claimed by its `(caller, messageId)` and held to
    /// the caller's quotas (§5).
    Client,
    /// A TCK scenario's: neither, as the TCK sends one message id again and
    /// again and opens a context for nearly every test.
    Tck,
}

/// A place among the streams one caller may hold open (§5), given back
/// when it is dropped with the stream that held it.
pub struct StreamSlot {
    a2a: Arc<A2a>,
    caller: String,
}

impl Drop for StreamSlot {
    fn drop(&mut self) {
        let mut streams = locked(&self.a2a.streams);
        if let Some(held) = streams.get_mut(&self.caller) {
            *held = held.saturating_sub(1);
            if *held == 0 {
                streams.remove(&self.caller);
            }
        }
    }
}

/// The A2A server.
pub struct A2a {
    store: Store,
    backend: Arc<dyn SessionBackend>,
    settings: Settings,
    cancel: CancellationToken,
    write: Mutex<()>,
    hubs: Mutex<HashMap<String, broadcast::Sender<Arc<TaskEvent>>>>,
    sink: RwLock<Option<Arc<dyn TaskEventSink>>>,
    progress: Mutex<HashMap<String, ProgressGate>>,
    scenarios: Mutex<HashMap<String, tokio::task::AbortHandle>>,
    touched: Mutex<HashMap<String, i64>>,
    revocations: tokio::sync::watch::Sender<u64>,
    /// The contexts whose session the backend is starting.
    starting: Mutex<std::collections::HashSet<String>>,
    /// How many streams each caller holds open.
    streams: Mutex<HashMap<String, usize>>,
    me: Weak<A2a>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// An epoch-milliseconds instant as the protocol writes one.
pub(crate) fn iso_ms(ms: i64) -> String {
    iso8601(UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64))
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// `text` cut to `chars` characters.
fn clip(text: &str, chars: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

impl TaskRow {
    /// The status as the protocol writes it.
    pub fn status(&self) -> TaskStatus {
        TaskStatus {
            state: self.state,
            message: self.status_message.clone(),
            timestamp: Some(iso_ms(self.status_ms)),
        }
    }

    /// The task as the protocol writes it: `history_length` of its history
    /// (all of it when not said; none for 0), and its artifacts when
    /// `artifacts` asks (`Some(true)`: always, an empty list included;
    /// `None`: when there are any).
    pub fn to_task(&self, history_length: Option<i32>, artifacts: Option<bool>) -> Task {
        let history = match history_length {
            Some(n) if n <= 0 => None,
            Some(n) => Some(self.history[self.history.len().saturating_sub(n as usize)..].to_vec()),
            None => Some(self.history.clone()),
        }
        .filter(|history| !history.is_empty());
        let artifacts = match artifacts {
            Some(true) => Some(self.artifacts.clone()),
            Some(false) => None,
            None => (!self.artifacts.is_empty()).then(|| self.artifacts.clone()),
        };
        Task {
            id: self.id.clone(),
            context_id: self.context_id.clone(),
            status: self.status(),
            artifacts,
            history,
            metadata: None,
        }
    }

    /// The request id of the question the task waits on, from its status
    /// message.
    fn pending_question(&self) -> Option<String> {
        if self.state != TaskState::InputRequired {
            return None;
        }
        self.status_message.as_ref()?.parts.iter().find_map(|part| match &part.content {
            PartContent::Data(Value::Object(data)) if data.get("kind").and_then(Value::as_str) == Some("question") => {
                data.get("requestId").and_then(Value::as_str).map(str::to_string)
            }
            _ => None,
        })
    }

    /// Keep a message the backend has had: its files as placeholders only,
    /// and the oldest messages dropped while the history is over
    /// [`HISTORY_KEPT`] messages or [`HISTORY_BYTES`].
    fn push_history(&mut self, message: Message) {
        self.history.push(without_files(message));
        self.trim_history();
    }

    /// Keep a message as it came, files and all: one the task takes into its
    /// turn once it has a slot.
    fn queue_history(&mut self, message: Message) {
        self.history.push(message);
    }

    /// The history has gone to the backend: from now on it keeps
    /// placeholders for its files, and no more than [`Self::push_history`]
    /// keeps.
    fn dispatched_history(&mut self) {
        let history = std::mem::take(&mut self.history);
        self.history = history.into_iter().map(without_files).collect();
        self.trim_history();
    }

    /// Drop the oldest messages while there are more than [`HISTORY_KEPT`]
    /// or they take more than [`HISTORY_BYTES`]; the newest one stays.
    fn trim_history(&mut self) {
        let sizes: Vec<usize> = self.history.iter().map(json_size).collect();
        let mut total: usize = sizes.iter().sum();
        let mut dropped = 0;
        while dropped + 1 < self.history.len() && (self.history.len() - dropped > HISTORY_KEPT || total > HISTORY_BYTES) {
            total -= sizes[dropped];
            dropped += 1;
        }
        self.history.drain(..dropped);
    }

    fn agent_message(&self, parts: Vec<Part>, metadata: Option<serde_json::Map<String, Value>>) -> Message {
        Message {
            message_id: new_id(),
            context_id: self.context_id.clone(),
            task_id: self.id.clone(),
            role: Role::Agent,
            parts,
            metadata,
            ..Message::default()
        }
    }
}

/// How many bytes `value` takes as JSON.
fn json_size(value: &impl serde::Serialize) -> usize {
    serde_json::to_vec(value).map_or(0, |bytes| bytes.len())
}

/// `message` with each file's bytes replaced by what they were: a data part
/// `{"kind": "omitted_file", "mediaType": …, "size": <bytes>}`, its file name
/// kept. The bytes went to the session; the history keeps no copy (§4).
fn without_files(mut message: Message) -> Message {
    for part in &mut message.parts {
        let PartContent::Raw(bytes) = &part.content else { continue };
        let placeholder = json!({"kind": "omitted_file", "mediaType": base_type(part), "size": bytes.len()});
        part.content = PartContent::Data(placeholder);
        part.media_type = "application/json".to_string();
    }
    message
}

/// The text a question's status message shows a client that reads text only.
fn question_text(question: &Question) -> String {
    let mut out = String::new();
    for (index, item) in question.questions.iter().enumerate() {
        if index > 0 {
            out.push_str("\n\n");
        }
        if item.header.is_empty() {
            out.push_str(&item.question);
        } else {
            out.push_str(&format!("{}: {}", item.header, item.question));
        }
        for (n, option) in item.options.iter().enumerate() {
            if option.description.is_empty() {
                out.push_str(&format!("\n  {}. {}", n + 1, option.label));
            } else {
                out.push_str(&format!("\n  {}. {} — {}", n + 1, option.label, option.description));
            }
        }
        if item.multi_select {
            out.push_str("\n  (more than one may be chosen)");
        }
    }
    out.push_str("\n\nAnswer with an option's label, or in your own words.");
    out
}

/// The two parts of an `INPUT_REQUIRED` status message (§3.4).
fn question_parts(question: &Question) -> Vec<Part> {
    vec![
        Part::text(question_text(question)).with_media_type("text/plain"),
        Part::data(json!({"kind": "question", "requestId": question.request_id, "questions": question.questions})),
    ]
}

/// The artifacts of a finished turn (§3.3).
fn turn_artifacts(result: &TurnResult) -> [Artifact; 2] {
    [
        Artifact {
            artifact_id: "summary".into(),
            name: "summary".into(),
            parts: vec![Part::text(result.summary.clone()).with_media_type("text/markdown")],
            ..Artifact::default()
        },
        Artifact {
            artifact_id: "result".into(),
            name: "result".into(),
            parts: vec![Part::data(serde_json::to_value(&result.result).unwrap_or(Value::Null))],
            ..Artifact::default()
        },
    ]
}

/// `artifact` in `artifacts`, replacing one of the same id.
fn put_artifact(artifacts: &mut Vec<Artifact>, artifact: Artifact) {
    match artifacts.iter_mut().find(|existing| existing.artifact_id == artifact.artifact_id) {
        Some(existing) => *existing = artifact,
        None => artifacts.push(artifact),
    }
}

/// The media type a part names, without its parameters.
fn base_type(part: &Part) -> String {
    part.media_type.split(';').next().unwrap_or_default().trim().to_ascii_lowercase()
}

/// Whether every part is in a media type the card lists (`defaultInputModes`).
fn check_parts(parts: &[Part]) -> Result<(), A2aError> {
    let refuse = |what: String| Err(A2aError::new(ErrorKind::ContentTypeNotSupported, what));
    let mut images = 0;
    let mut chars = 0;
    for part in parts {
        let media = base_type(part);
        match &part.content {
            PartContent::Text(text) => {
                if !(media.is_empty() || media == "text/plain") {
                    return refuse(format!("a text part in {media} is not taken: text/plain only"));
                }
                chars += text.chars().count();
            }
            PartContent::Data(data) => {
                if !(media.is_empty() || media == "application/json") {
                    return refuse(format!("a data part in {media} is not taken: application/json only"));
                }
                chars += data.to_string().chars().count();
            }
            PartContent::Raw(bytes) => {
                if !IMAGE_TYPES.contains(&media.as_str()) {
                    return refuse(format!(
                        "a file in {} is not taken: image/png and image/jpeg only",
                        if media.is_empty() { "no media type" } else { &media }
                    ));
                }
                if bytes.len() > MAX_IMAGE_BYTES {
                    return Err(A2aError::invalid(format!("an image is at most {} MiB", MAX_IMAGE_BYTES / (1024 * 1024))));
                }
                images += 1;
            }
            PartContent::Url(_) => return refuse("a file by URL is not fetched: send its bytes (raw)".into()),
        }
    }
    if images > MAX_IMAGES {
        return Err(A2aError::invalid(format!("a message carries at most {MAX_IMAGES} images")));
    }
    if chars > MAX_REQUEST_CHARS {
        return Err(A2aError::invalid(format!("a message is at most {MAX_REQUEST_CHARS} characters")));
    }
    Ok(())
}

/// The request a task's messages make: every text part in order, then each
/// JSON part as compact JSON, and the images (§5).
fn request_of(messages: &[Message]) -> (String, Vec<Image>) {
    let mut pieces = Vec::new();
    let mut images = Vec::new();
    for message in messages {
        for part in &message.parts {
            match &part.content {
                PartContent::Text(text) if !text.trim().is_empty() => pieces.push(text.clone()),
                PartContent::Data(data) => pieces.push(data.to_string()),
                PartContent::Raw(bytes) => images.push(Image {
                    media_type: base_type(part),
                    filename: label_of(&part.filename),
                    bytes: bytes.clone(),
                }),
                _ => {}
            }
        }
    }
    (pieces.join("\n\n"), images)
}

/// A part's file name as a label: its last component, cut down to letters,
/// digits, `.`, `-` and `_`, never starting with a dot. Never a path (§5).
fn label_of(filename: &str) -> Option<String> {
    let last = filename.rsplit(['/', '\\']).next().unwrap_or_default();
    let label: String = last
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .take(64)
        .collect();
    let label = label.trim_start_matches('.').to_string();
    (!label.is_empty()).then_some(label)
}

/// An answer out of a message (§3.4): `data` `{answers, notes?, requestId?}`
/// and/or text.
fn answer_of(message: &Message, open: &str) -> Result<Answer, A2aError> {
    let shape = "an answer's data part is {\"answers\": {question: label or [labels]}, \"notes\"?: {question: text}, \"requestId\"?: string}";
    let mut answer = Answer {
        request_id: open.to_string(),
        answers: BTreeMap::new(),
        notes: BTreeMap::new(),
        response: None,
    };
    let mut texts = Vec::new();
    for part in &message.parts {
        match &part.content {
            PartContent::Text(text) if !text.trim().is_empty() => texts.push(text.trim().to_string()),
            PartContent::Text(_) => {}
            PartContent::Data(Value::Object(data)) => {
                if let Some(request_id) = data.get("requestId") {
                    if request_id.as_str() != Some(open) {
                        return Err(A2aError::invalid("requestId names a question the task is not waiting on"));
                    }
                }
                if let Some(answers) = data.get("answers") {
                    let answers: BTreeMap<String, AnswerValue> = serde_json::from_value(answers.clone()).map_err(|_| A2aError::invalid(shape))?;
                    answer.answers.extend(answers);
                }
                if let Some(notes) = data.get("notes") {
                    let notes: BTreeMap<String, String> = serde_json::from_value(notes.clone()).map_err(|_| A2aError::invalid(shape))?;
                    answer.notes.extend(notes);
                }
            }
            _ => return Err(A2aError::invalid(shape)),
        }
    }
    if !texts.is_empty() {
        answer.response = Some(texts.join("\n"));
    }
    if answer.answers.is_empty() && answer.response.is_none() {
        return Err(A2aError::invalid(format!("the task waits on an answer: {shape}, or text")));
    }
    Ok(answer)
}

impl A2a {
    /// The server over `store`, driving sessions through `backend`, which
    /// reports into `inbox`. Its event loop runs until `cancel`.
    pub fn new(store: Store, backend: Arc<dyn SessionBackend>, inbox: EventInbox, settings: Settings, cancel: CancellationToken) -> Arc<Self> {
        let a2a = Arc::new_cyclic(|me| A2a {
            store,
            backend,
            settings,
            cancel: cancel.clone(),
            write: Mutex::new(()),
            hubs: Mutex::new(HashMap::new()),
            sink: RwLock::new(None),
            progress: Mutex::new(HashMap::new()),
            scenarios: Mutex::new(HashMap::new()),
            touched: Mutex::new(HashMap::new()),
            revocations: tokio::sync::watch::channel(0).0,
            starting: Mutex::new(std::collections::HashSet::new()),
            streams: Mutex::new(HashMap::new()),
            me: me.clone(),
        });
        let weak = Arc::downgrade(&a2a);
        let mut inbox = inbox.0;
        tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    () = cancel.cancelled() => return,
                    event = inbox.recv() => event,
                };
                let (Some(event), Some(a2a)) = (event, weak.upgrade()) else { return };
                a2a.on_backend_event(event).await;
            }
        });
        a2a
    }

    fn me(&self) -> Arc<A2a> {
        self.me.upgrade().expect("the server is alive while it is called")
    }

    /// The store, for the management routes and the push seam.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The settings.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The backend.
    pub fn backend(&self) -> &Arc<dyn SessionBackend> {
        &self.backend
    }

    /// Cancelled when the server stops; streams end on it.
    pub fn cancelled(&self) -> &CancellationToken {
        &self.cancel
    }

    /// Hand every event from now on to `sink` as well (the push seam).
    pub fn set_event_sink(&self, sink: Arc<dyn TaskEventSink>) {
        *self.sink.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sink);
    }

    /// One entry.
    pub fn entry(&self, id: &str) -> Option<A2aEntry> {
        self.store.entry(id)
    }

    /// A task's push configurations.
    pub fn push_configs(&self, task_id: &str) -> Vec<PushRow> {
        self.store.pushes(task_id)
    }

    /* ── who is calling ──────────────────────────────────────────────── */

    /// Who `authorization` (the header's value), sent from `peer`, speaks
    /// for on `entry_id`. Only a peer on this device's loopback is heard; an
    /// unknown peer is not one.
    pub fn authorize(&self, authorization: Option<&str>, peer: Option<std::net::IpAddr>, entry_id: &str) -> Result<Caller, Denied> {
        if !peer.is_some_and(|ip| ip.to_canonical().is_loopback()) {
            return Err(Denied::NotLoopback);
        }
        let bearer = authorization
            .and_then(|value| value.split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
            .map(|(_, token)| token.trim())
            .ok_or(Denied::Unauthenticated)?;
        let (id, secret) = crate::token::parse(bearer).ok_or(Denied::Unauthenticated)?;
        let row = self.store.token(id).ok_or(Denied::Unauthenticated)?;
        if !crate::token::proves(&row, secret) {
            return Err(Denied::Unauthenticated);
        }
        self.touch(&row);
        if !row.entries.iter().any(|entry| entry == entry_id) {
            return Err(Denied::NotFound);
        }
        let entry = self.store.entry(entry_id).ok_or(Denied::NotFound)?;
        Ok(Caller {
            token_id: row.id.clone(),
            caller: crate::token::caller_of(&row.id),
            entry,
        })
    }

    /// `who` asked again as things stand now: its token still stands and
    /// still has the entry in scope, and the entry is still there, read
    /// afresh. A request asks once its body is in, so a token revoked while
    /// the body was on its way proves nothing (§6).
    pub fn recheck(&self, who: &Caller) -> Result<Caller, Denied> {
        let token = self
            .store
            .token(&who.token_id)
            .filter(|token| token.revoked_ms.is_none())
            .ok_or(Denied::Unauthenticated)?;
        if !token.entries.contains(&who.entry.id) {
            return Err(Denied::NotFound);
        }
        let entry = self.store.entry(&who.entry.id).ok_or(Denied::NotFound)?;
        Ok(Caller {
            token_id: who.token_id.clone(),
            caller: who.caller.clone(),
            entry,
        })
    }

    /// A place for one more open stream of `who`'s, while it has fewer than
    /// [`MAX_STREAMS`] (§5); given back when the slot is dropped.
    pub fn stream_slot(&self, who: &Caller) -> Result<StreamSlot, A2aError> {
        let mut streams = locked(&self.streams);
        let held = streams.entry(who.caller.clone()).or_default();
        if *held >= MAX_STREAMS {
            return Err(over_quota(format!("a caller holds at most {MAX_STREAMS} streams open at once")));
        }
        *held += 1;
        Ok(StreamSlot {
            a2a: self.me(),
            caller: who.caller.clone(),
        })
    }

    /// Write down that a token was used, at most once a minute.
    fn touch(&self, row: &TokenRow) {
        let now = now_ms();
        {
            let mut touched = locked(&self.touched);
            let last = touched.get(&row.id).copied().or(row.last_used_ms).unwrap_or(i64::MIN);
            if now.saturating_sub(last) < TOUCH_EVERY_MS {
                return;
            }
            touched.insert(row.id.clone(), now);
        }
        self.store.touch_token(&row.id, now);
    }

    /// A task `caller` may see: of its entry, and sent by it. Another
    /// caller's task is not found, exactly as one that does not exist (§6).
    fn scoped(&self, caller: &Caller, task_id: &str) -> Result<TaskRow, A2aError> {
        self.store
            .task_for(&caller.entry.id, &caller.caller, task_id)
            .ok_or_else(A2aError::task_not_found)
    }

    /// How a caller is named to the agent (`{{caller}}`): a token's name
    /// beside its caller id, so the agent reads who it is and the id stays
    /// the one that counts.
    pub fn caller_label(&self, caller: &str) -> String {
        caller
            .strip_prefix("a2a:token:")
            .and_then(|id| self.store.token(id))
            .map_or_else(|| caller.to_string(), |token| format!("{} ({caller})", token.name))
    }

    /// Whether `who` may still reach its entry: its token stands and has the
    /// entry in scope, and the entry is there and on. Asked again before
    /// every stream event (§6).
    pub fn still_reaches(&self, who: &Caller) -> bool {
        let token = self.store.token(&who.token_id);
        let entry = self.store.entry(&who.entry.id);
        token.is_some_and(|token| token.revoked_ms.is_none() && token.entries.contains(&who.entry.id)) && entry.is_some_and(|entry| entry.enabled)
    }

    /// Whether `caller` (`a2a:token:<token id>` or `a2a:device:<id>`) may
    /// still reach `entry_id`: what a push asks before each delivery (§6).
    pub fn caller_reaches(&self, caller: &str, entry_id: &str) -> bool {
        let Some(entry) = self.store.entry(entry_id).filter(|entry| entry.enabled) else {
            return false;
        };
        if let Some(token_id) = caller.strip_prefix("a2a:token:") {
            return self
                .store
                .token(token_id)
                .is_some_and(|token| token.revoked_ms.is_none() && token.entries.contains(&entry.id));
        }
        if let Some(device) = caller.strip_prefix("a2a:device:") {
            return entry.allowed_devices.iter().any(|allowed| allowed == device);
        }
        false
    }

    /// Take away what `caller` has on `entry_id` (every entry when `None`):
    /// each unfinished task cancelled — withdrawn while queued, its turn
    /// interrupted once it runs — with `droppen.revoked` in its status, its
    /// push configurations deleted, and every open stream told to look again
    /// (§6).
    ///
    /// The revocation itself (the token's row, the entry's settings) is
    /// written before this is called. The tasks are read under the writer's
    /// lock: a task made, or picked to be handed over, before the revocation
    /// was written is among them, and one after it is refused there.
    pub async fn revoke(&self, caller: Option<&str>, entry_id: Option<&str>) {
        self.revocations.send_modify(|generation| *generation += 1);
        let tasks: Vec<TaskRow> = {
            let _writer = locked(&self.write);
            self.store
                .open_tasks()
                .into_iter()
                .filter(|task| caller.is_none_or(|caller| task.caller == caller) && entry_id.is_none_or(|entry| task.entry == entry))
                .collect()
        };
        for task in tasks {
            if let Err(error) = self.cancel(task.clone(), Some(ErrorCode::Revoked)).await {
                tracing::warn!(task = %task.id, %error, "a revoked caller's task could not be cancelled through the backend");
            }
            if let Err(error) = self.store.delete_pushes(&task.id) {
                tracing::warn!(task = %task.id, %error, "cannot delete a revoked caller's push configurations");
            }
        }
        self.revocations.send_modify(|generation| *generation += 1);
    }

    /// Keep an entry's new settings. When they changed where a session is
    /// bound to them — its source (and ACP agent), directory, account,
    /// permission mode or isolation — every context of the entry is closed
    /// with them, in the same write and under the writer's lock, so no task
    /// is made or handed over in between; their unfinished tasks are then
    /// cancelled with `droppen.context_closed` (§6).
    pub(crate) async fn replace_entry(&self, before: &A2aEntry, entry: &A2aEntry, now: i64) -> rusqlite::Result<()> {
        let closing = before.source != entry.source
            || before.acp_agent != entry.acp_agent
            || before.cwd != entry.cwd
            || before.account != entry.account
            || before.permission != entry.permission
            || before.isolation != entry.isolation;
        let tasks = {
            let _writer = locked(&self.write);
            self.store.replace_entry(entry, now, closing)?;
            if closing {
                self.store.open_tasks_of_entry(&entry.id)
            } else {
                Vec::new()
            }
        };
        for task in tasks {
            if let Err(error) = self.cancel(task.clone(), Some(ErrorCode::ContextClosed)).await {
                tracing::warn!(task = %task.id, %error, "a closed context's task could not be cancelled through the backend");
            }
        }
        Ok(())
    }

    /// Forget the tasks that ended more than 30 days before `now`, with
    /// their events, messages and push configurations, and the contexts
    /// they leave empty (§4). Under the writer's lock, so no task is made in
    /// a context as it goes. How many tasks went.
    pub fn retain(&self, now: i64) -> usize {
        let _writer = locked(&self.write);
        match self.store.prune_finished(now - TASKS_KEPT_MS) {
            Ok(pruned) => pruned,
            Err(error) => {
                tracing::warn!(%error, "cannot prune finished A2A tasks");
                0
            }
        }
    }

    /// Prune what finished long ago ([`Self::retain`]) now and every hour,
    /// until the server stops. The instance's owner calls it, once.
    pub fn spawn_retention(&self) {
        let weak = self.me.clone();
        let cancel = self.cancel.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(RETENTION_EVERY);
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    () = cancel.cancelled() => return,
                    _ = every.tick() => {}
                }
                let Some(a2a) = weak.upgrade() else { return };
                let pruned = a2a.retain(now_ms());
                if pruned > 0 {
                    tracing::info!(pruned, "pruned A2A tasks finished more than 30 days ago");
                }
            }
        });
    }

    /// The backend's view of a task.
    fn task_ref(&self, task: &TaskRow) -> Option<TaskRef> {
        let context = self.store.context(&task.context_id)?;
        Some(TaskRef {
            task_id: task.id.clone(),
            context_id: task.context_id.clone(),
            entry_id: task.entry.clone(),
            caller: task.caller.clone(),
            session: context.session,
        })
    }

    /* ── writing ─────────────────────────────────────────────────────── */

    /// Change a task under the writer's lock, and write and send what the
    /// change emits. `None` when there is no such task.
    fn mutate<R>(&self, task_id: &str, change: impl FnOnce(&mut TaskRow) -> (Vec<Emit>, R)) -> Option<(TaskRow, R)> {
        let writer = locked(&self.write);
        let row = self.store.task(task_id)?;
        self.write_change(&writer, row, change)
    }

    /// [`Self::mutate`] on `row`, read under the writer's lock the caller
    /// holds (`_writer`).
    fn write_change<R>(
        &self,
        _writer: &MutexGuard<'_, ()>,
        mut row: TaskRow,
        change: impl FnOnce(&mut TaskRow) -> (Vec<Emit>, R),
    ) -> Option<(TaskRow, R)> {
        let before = row.clone();
        let (emits, out) = change(&mut row);
        if emits.is_empty() && row == before {
            return Some((row, out));
        }
        let now = now_ms().max(row.updated_ms);
        row.updated_ms = now;
        let mut events = Vec::with_capacity(emits.len());
        for emit in emits {
            row.seq += 1;
            let (kind, response) = match emit {
                Emit::Status | Emit::Progress => {
                    row.status_ms = now;
                    let kind = if matches!(emit, Emit::Status) {
                        EventKind::Status
                    } else {
                        EventKind::Progress
                    };
                    (
                        kind,
                        StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
                            task_id: row.id.clone(),
                            context_id: row.context_id.clone(),
                            status: row.status(),
                            metadata: None,
                        }),
                    )
                }
                Emit::Artifact(artifact) => (EventKind::Artifact, self.artifact_update(&row, artifact, false, true)),
                Emit::Chunk { artifact, append, last } => (EventKind::Artifact, self.artifact_update(&row, artifact, append, last)),
            };
            events.push(TaskEvent {
                task_id: row.id.clone(),
                context_id: row.context_id.clone(),
                entry_id: row.entry.clone(),
                seq: row.seq,
                kind,
                state: row.state,
                response,
            });
        }
        let rows: Vec<EventRow> = events
            .iter()
            .map(|event| EventRow {
                seq: event.seq,
                kind: event.kind.as_str().to_string(),
                payload: serde_json::to_string(&event.response).unwrap_or_default(),
            })
            .collect();
        if let Err(error) = self.store.save(&row, &rows, row.history != before.history) {
            tracing::warn!(task = %row.id, %error, "cannot write an A2A task's change");
            return None;
        }
        let sink = self.sink.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let mut hubs = locked(&self.hubs);
        for event in events {
            let event = Arc::new(event);
            if let Some(hub) = hubs.get(&row.id) {
                let _ = hub.send(event.clone());
            }
            if let Some(sink) = &sink {
                sink.task_event(&event);
            }
        }
        if row.state.is_terminal() {
            // The last event is out; the streams drain it and end.
            hubs.remove(&row.id);
            locked(&self.progress).remove(&row.id);
            locked(&self.scenarios).remove(&row.id);
        }
        Some((row, out))
    }

    fn artifact_update(&self, row: &TaskRow, artifact: Artifact, append: bool, last_chunk: bool) -> StreamResponse {
        StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
            task_id: row.id.clone(),
            context_id: row.context_id.clone(),
            artifact,
            append,
            last_chunk,
            metadata: None,
        })
    }

    /// Follow a task from now, for `who`: its snapshot, and every event
    /// after it while `who` may still reach the entry.
    pub fn follow(&self, who: Option<&Caller>, task_id: &str, history_length: Option<i32>) -> Option<Follow> {
        let revocations = self.revocations.subscribe();
        let _writer = locked(&self.write);
        let snapshot = self.store.task(task_id)?;
        let rx = (!snapshot.state.is_terminal()).then(|| {
            locked(&self.hubs)
                .entry(task_id.to_string())
                .or_insert_with(|| broadcast::channel(STREAM_BUFFER).0)
                .subscribe()
        });
        Some(Follow {
            a2a: self.me(),
            who: who.cloned(),
            revocations,
            last_seq: snapshot.seq,
            // A stream ends when the task *reaches* a terminal or interrupted
            // state: one opened on a task already waiting on an answer waits
            // with it, for what comes after the answer.
            done: snapshot.state.is_terminal(),
            snapshot,
            rx,
            backlog: std::collections::VecDeque::new(),
            history_length,
        })
    }

    /// End a task in `state`, with `text` as its status message and `code`
    /// in its metadata, unless it has ended already. Frees its slot.
    fn end(&self, task_id: &str, state: TaskState, text: &str, code: Option<ErrorCode>) {
        let ended = {
            let writer = locked(&self.write);
            self.end_in(&writer, task_id, state, text, code)
        };
        if let Some(entry) = ended {
            self.pump(&entry);
        }
    }

    /// [`Self::end`] under the writer's lock the caller holds, and without
    /// handing out the slot it frees: the task's entry, when it ended it.
    fn end_in(&self, writer: &MutexGuard<'_, ()>, task_id: &str, state: TaskState, text: &str, code: Option<ErrorCode>) -> Option<String> {
        let row = self.store.task(task_id)?;
        let (_, entry) = self.write_change(writer, row, |task| {
            if task.state.is_terminal() {
                return (Vec::new(), None);
            }
            task.state = state;
            task.status_message = (!text.is_empty()).then(|| task.agent_message(vec![Part::text(text)], code.map(code_metadata)));
            (vec![Emit::Status], Some(task.entry.clone()))
        })?;
        entry
    }

    /* ── the backend's reports ───────────────────────────────────────── */

    async fn on_backend_event(&self, event: BackendEvent) {
        match event {
            BackendEvent::TurnStarted { task_id } => {
                self.mutate(&task_id, |task| {
                    if task.state != TaskState::Submitted {
                        return (Vec::new(), ());
                    }
                    task.state = TaskState::Working;
                    task.status_message = None;
                    (vec![Emit::Status], ())
                });
            }
            BackendEvent::Progress { task_id, text } => self.progress(&task_id, &text),
            BackendEvent::Question { task_id, question } => self.ask(&task_id, &question),
            BackendEvent::QuestionResolved { task_id } => {
                self.mutate(&task_id, |task| {
                    if task.state != TaskState::InputRequired {
                        return (Vec::new(), ());
                    }
                    task.state = TaskState::Working;
                    task.status_message = None;
                    (vec![Emit::Status], ())
                });
            }
            BackendEvent::TurnEnded { task_id, outcome } => self.turn_ended(&task_id, outcome),
            BackendEvent::SessionIdentified { context_id, session_id } => {
                self.store.set_session(&context_id, Some(&session_id), None, None);
                // The context's later tasks waited for its session.
                if let Some(context) = self.store.context(&context_id) {
                    self.pump(&context.entry);
                }
            }
            BackendEvent::SessionGone { context_id, message } => {
                for task in self.store.open_tasks_of_context(&context_id) {
                    self.end(&task.id, TaskState::Failed, &message, None);
                }
            }
        }
    }

    fn ask(&self, task_id: &str, question: &Question) {
        let asked = self.mutate(task_id, |task| {
            if task.state.is_terminal() {
                return (Vec::new(), None);
            }
            task.state = TaskState::InputRequired;
            task.status_message = Some(task.agent_message(question_parts(question), None));
            (vec![Emit::Status], Some(task.entry.clone()))
        });
        if let Some((_, Some(entry))) = asked {
            self.pump(&entry);
        }
    }

    fn turn_ended(&self, task_id: &str, outcome: TurnOutcome) {
        match outcome {
            TurnOutcome::Completed(result) => {
                let done = self.mutate(task_id, |task| {
                    if task.state.is_terminal() {
                        return (Vec::new(), None);
                    }
                    let [summary, result_artifact] = turn_artifacts(&result);
                    put_artifact(&mut task.artifacts, summary.clone());
                    put_artifact(&mut task.artifacts, result_artifact.clone());
                    task.state = TaskState::Completed;
                    let said = clip(&result.summary, PROGRESS_CHARS);
                    task.status_message = (!said.is_empty()).then(|| task.agent_message(vec![Part::text(said)], None));
                    (
                        vec![Emit::Artifact(summary), Emit::Artifact(result_artifact), Emit::Status],
                        Some(task.entry.clone()),
                    )
                });
                if let Some((_, Some(entry))) = done {
                    self.pump(&entry);
                }
            }
            TurnOutcome::Failed(message) => self.end(task_id, TaskState::Failed, &message, None),
            TurnOutcome::Canceled(message) => self.end(task_id, TaskState::Canceled, &message, None),
            TurnOutcome::Rejected { code, message } => self.end(task_id, TaskState::Rejected, &message, code),
        }
    }

    /// A working task's progress, thinned to one status message per
    /// [`PROGRESS_EVERY`]: the latest one waits for the window to close.
    fn progress(&self, task_id: &str, text: &str) {
        let text = clip(text, PROGRESS_CHARS);
        if text.is_empty() {
            return;
        }
        let now = tokio::time::Instant::now();
        let flush_at = {
            let mut gates = locked(&self.progress);
            let gate = gates.entry(task_id.to_string()).or_default();
            match gate.last {
                Some(last) if now < last + PROGRESS_EVERY => {
                    gate.pending = Some(text);
                    if gate.scheduled {
                        return;
                    }
                    gate.scheduled = true;
                    Err(last + PROGRESS_EVERY)
                }
                _ => {
                    gate.last = Some(now);
                    Ok(text)
                }
            }
        };
        match flush_at {
            Ok(text) => self.say_progress(task_id, text),
            Err(at) => {
                let me = self.me();
                let task_id = task_id.to_string();
                tokio::spawn(async move {
                    tokio::time::sleep_until(at).await;
                    let pending = {
                        let mut gates = locked(&me.progress);
                        let Some(gate) = gates.get_mut(&task_id) else { return };
                        gate.scheduled = false;
                        gate.last = Some(tokio::time::Instant::now());
                        gate.pending.take()
                    };
                    if let Some(text) = pending {
                        me.say_progress(&task_id, text);
                    }
                });
            }
        }
    }

    fn say_progress(&self, task_id: &str, text: String) {
        self.mutate(task_id, |task| {
            if task.state != TaskState::Working {
                return (Vec::new(), ());
            }
            task.status_message = Some(task.agent_message(vec![Part::text(text)], None));
            (vec![Emit::Progress], ())
        });
    }

    /* ── handing tasks to the backend ────────────────────────────────── */

    /// Hand the entry's waiting tasks to the backend while it has slots.
    pub(crate) fn pump(&self, entry_id: &str) {
        let me = self.me();
        let entry_id = entry_id.to_string();
        tokio::spawn(async move {
            while let Some(job) = me.next_job(&entry_id) {
                let me = me.clone();
                tokio::spawn(async move { me.dispatch(job).await });
            }
        });
    }

    /// The next task of an entry to hand over, marked handed over; `None`
    /// when there is none or no slot. A switched-off entry's waiting tasks
    /// are rejected instead.
    fn next_job(&self, entry_id: &str) -> Option<Job> {
        let entry = self.store.entry(entry_id)?;
        if entry.source == EntrySource::Tck {
            return None;
        }
        if !entry.enabled {
            while let Some(task) = self.store.next_pending(entry_id) {
                self.end(&task.id, TaskState::Rejected, "the agent is switched off", Some(ErrorCode::EntryDisabled));
            }
            return None;
        }
        let Picked { task, messages, context } = self.mutate_pick(&entry)?;
        let (request, images) = request_of(&messages);
        Some(Job {
            start: context.session.session_id.is_none(),
            task: TaskRef {
                task_id: task.id.clone(),
                context_id: task.context_id.clone(),
                entry_id: task.entry.clone(),
                caller: task.caller.clone(),
                session: context.session,
            },
            run: entry.run_spec(),
            prompt: Prompt {
                text: render(entry.template(), &self.caller_label(&task.caller), &request, &fence()),
                images,
            },
            client_message_id: task.client_message_id.clone(),
        })
    }

    /// Under the writer's lock: the oldest waiting task of `entry` that may
    /// go, if a slot is free, marked handed over, with its messages as they
    /// stood. On the way, a task whose caller no longer reaches the entry is
    /// cancelled with `droppen.revoked`, and one whose context was closed
    /// with `droppen.context_closed` (§6). A task whose context's session is
    /// being started, or runs its first turn with no id known yet, waits for
    /// it; the tasks of other contexts go before it.
    fn mutate_pick(&self, entry: &A2aEntry) -> Option<Picked> {
        let writer = locked(&self.write);
        if self.store.running(&entry.id) >= entry.max_concurrent.max(1) as usize {
            return None;
        }
        for (task_id, context_id, caller) in self.store.pending(&entry.id) {
            let context = self.store.context(&context_id);
            let refused = if !self.caller_reaches(&caller, &entry.id) {
                Some((TaskState::Canceled, canceled_text(Some(ErrorCode::Revoked)), Some(ErrorCode::Revoked)))
            } else {
                match &context {
                    None => Some((TaskState::Failed, "the task's context is gone", None)),
                    Some(context) if context.closed_ms.is_some() => Some((
                        TaskState::Canceled,
                        canceled_text(Some(ErrorCode::ContextClosed)),
                        Some(ErrorCode::ContextClosed),
                    )),
                    Some(_) => None,
                }
            };
            if let Some((state, text, code)) = refused {
                self.end_in(&writer, &task_id, state, text, code);
                continue;
            }
            let Some(context) = context else { continue };
            if context.session.session_id.is_none() && (locked(&self.starting).contains(&context_id) || self.store.context_running(&context_id)) {
                continue;
            }
            let Some(task) = self.store.task(&task_id) else { continue };
            let messages = task.history.clone();
            let (task, ()) = self.write_change(&writer, task, |row| {
                row.dispatched = true;
                row.dispatched_history();
                (Vec::new(), ())
            })?;
            if context.session.session_id.is_none() {
                locked(&self.starting).insert(context_id);
            }
            return Some(Picked { task, messages, context });
        }
        None
    }

    async fn dispatch(&self, job: Job) {
        let task_id = job.task.task_id.clone();
        let context_id = job.task.context_id.clone();
        let entry_id = job.task.entry_id.clone();
        let caller = job.task.caller.clone();
        let start = job.start;
        let outcome = if start {
            let started = self
                .backend
                .start(StartSession {
                    task: job.task,
                    run: job.run,
                    prompt: job.prompt,
                    client_message_id: job.client_message_id,
                })
                .await;
            let started = started.map(|started| {
                self.store.set_session(
                    &context_id,
                    started.session_id.as_deref(),
                    Some(&started.cwd),
                    Some(started.branch.as_deref()),
                );
            });
            locked(&self.starting).remove(&context_id);
            started
        } else {
            self.backend
                .send(SendToSession {
                    task: job.task,
                    kind: SendKind::NewTurn,
                    prompt: job.prompt,
                    client_message_id: job.client_message_id,
                })
                .await
        };
        match outcome {
            Ok(()) => {
                // A revocation or a tightened entry that came while the call
                // was out may have found a task the backend had not taken
                // yet: the turn just begun is stopped here.
                let code = if !self.caller_reaches(&caller, &entry_id) {
                    Some(ErrorCode::Revoked)
                } else if self.store.context(&context_id).is_some_and(|context| context.closed_ms.is_some()) {
                    Some(ErrorCode::ContextClosed)
                } else {
                    None
                };
                if let Some(code) = code {
                    if let Some(task_ref) = self.store.task(&task_id).and_then(|task| self.task_ref(&task)) {
                        if let Err(error) = self.backend.cancel(&task_ref).await {
                            tracing::warn!(task = %task_id, %error, "a task taken back while it was handed over could not be stopped");
                        }
                    }
                    self.end(&task_id, TaskState::Canceled, canceled_text(Some(code)), Some(code));
                }
                if start {
                    // The context's later tasks waited for its session.
                    self.pump(&entry_id);
                }
            }
            Err(BackendError::Rejected { code, message }) => self.end(&task_id, TaskState::Rejected, &message, code),
            Err(BackendError::Invalid(message)) => self.end(&task_id, TaskState::Rejected, &message, None),
            Err(other) => self.end(&task_id, TaskState::Failed, &other.to_string(), None),
        }
    }

    /// After a restart: every unfinished task handed to the backend is picked
    /// up again where its session stands, and the waiting ones go out as
    /// slots allow. A TCK task, which nothing keeps across a restart, fails.
    pub async fn resume(&self) {
        let sink = self.sink.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        if let Some(sink) = sink {
            sink.resume();
        }
        let mut entries = std::collections::BTreeSet::new();
        for task in self.store.open_tasks() {
            entries.insert(task.entry.clone());
            let entry = self.store.entry(&task.entry);
            if entry.as_ref().is_none_or(|entry| entry.source == EntrySource::Tck) {
                self.end(&task.id, TaskState::Failed, "the server restarted while the task ran", None);
                continue;
            }
            if !task.dispatched {
                continue;
            }
            let Some(task_ref) = self.task_ref(&task) else {
                self.end(&task.id, TaskState::Failed, "the task's context is gone", None);
                continue;
            };
            match self.backend.resume(&task_ref).await {
                Ok(Resumed::Submitted) => {}
                Ok(Resumed::Working) => {
                    self.mutate(&task.id, |task| {
                        if task.state == TaskState::Submitted || task.state == TaskState::InputRequired {
                            task.state = TaskState::Working;
                            task.status_message = None;
                            return (vec![Emit::Status], ());
                        }
                        (Vec::new(), ())
                    });
                }
                Ok(Resumed::InputRequired(question)) => {
                    if task.pending_question().as_deref() != Some(question.request_id.as_str()) {
                        self.ask(&task.id, &question);
                    }
                }
                Ok(Resumed::Ended(outcome)) => self.turn_ended(&task.id, outcome),
                Ok(Resumed::Gone(message)) => self.end(&task.id, TaskState::Failed, &message, None),
                Err(error) => self.end(&task.id, TaskState::Failed, &error.to_string(), None),
            }
        }
        for entry in entries {
            self.pump(&entry);
        }
    }

    /* ── the operations ──────────────────────────────────────────────── */

    /// `SendMessage` (`streaming = false`) and `SendStreamingMessage`.
    pub async fn send(&self, who: &Caller, request: SendMessageRequest, streaming: bool) -> Result<Sent, A2aError> {
        let mut message = request.message.ok_or_else(|| A2aError::invalid("message is required"))?;
        if message.message_id.trim().is_empty() {
            return Err(A2aError::invalid("message.messageId is required"));
        }
        if message.message_id.chars().count() > 256 {
            return Err(A2aError::invalid("message.messageId is at most 256 characters"));
        }
        if message.role == Role::Agent {
            return Err(A2aError::invalid("message.role is ROLE_USER"));
        }
        message.role = Role::User;
        if message.parts.is_empty() {
            return Err(A2aError::invalid("message.parts needs at least one part"));
        }
        check_parts(&message.parts)?;
        let config = request.configuration.unwrap_or_default();
        if config.history_length.is_some_and(|n| n < 0) {
            return Err(A2aError::invalid("configuration.historyLength is not negative"));
        }
        let blocking = !streaming && !config.return_immediately;
        let history_length = config.history_length;
        if let Some(push) = &config.task_push_notification_config {
            self.check_push(&who.entry, push)?;
        }
        if !who.entry.enabled {
            return Err(A2aError::unsupported("this agent is switched off").with_code(ErrorCode::EntryDisabled));
        }
        if who.entry.source == EntrySource::Tck {
            return crate::tck::send(&self.me(), who, message, config, blocking).await;
        }

        if let Some(task_id) = self.store.message_task(&who.caller, &message.message_id) {
            return self.again(who, &task_id, history_length, blocking);
        }
        if !message.task_id.is_empty() {
            let task = self.scoped(who, &message.task_id)?;
            if !message.context_id.is_empty() && message.context_id != task.context_id {
                return Err(A2aError::invalid("contextId is not the task's context"));
            }
            message.context_id = task.context_id.clone();
            if !task.state.is_terminal() && self.store.context(&task.context_id).is_some_and(|context| context.closed_ms.is_some()) {
                return Err(context_closed());
            }
            return match task.state {
                state if state.is_terminal() => Err(A2aError::unsupported(
                    "the task is finished: send a new message with its contextId and no taskId to continue the session",
                )),
                TaskState::InputRequired => self.answer(who, task, message, history_length, blocking).await,
                TaskState::Submitted | TaskState::Working => self.append(who, task, message, history_length, blocking).await,
                _ => Err(A2aError::unsupported("the task takes no message in its state")),
            };
        }
        let context = if message.context_id.is_empty() {
            None
        } else {
            let context = self
                .store
                .context_for(&who.entry.id, &who.caller, &message.context_id)
                .ok_or_else(context_not_found)?;
            if context.closed_ms.is_some() {
                return Err(context_closed());
            }
            Some(context)
        };
        let created = self.create(who, message, context, Making::Client, config.task_push_notification_config)?;
        let follow = match created {
            Created::New(follow) => follow,
            Created::Duplicate(task_id) => return self.again(who, &task_id, history_length, blocking),
        };
        self.pump(&who.entry.id);
        Ok(Sent::Task {
            follow: follow.history(history_length),
            blocking,
        })
    }

    /// A message already taken: the task it went to, from where it stands.
    fn again(&self, who: &Caller, task_id: &str, history_length: Option<i32>, blocking: bool) -> Result<Sent, A2aError> {
        self.scoped(who, task_id)?;
        let follow = self.follow(Some(who), task_id, history_length).ok_or_else(A2aError::task_not_found)?;
        Ok(Sent::Task {
            follow: Box::new(follow),
            blocking,
        })
    }

    /// Make a task for `message`: in `context`, or in a new one, with `push`
    /// if the message came with a push configuration (checked already).
    /// Followed from its first snapshot, before anything can move it.
    ///
    /// Under the writer's lock, the caller is asked again whether it still
    /// reaches the entry, the context whether it is still open, and the
    /// caller's quotas (§5) are counted: a revocation, a closing and another
    /// request of the same caller each come before or after, never between.
    pub(crate) fn create(
        &self,
        who: &Caller,
        mut message: Message,
        context: Option<ContextRow>,
        making: Making,
        push: Option<TaskPushNotificationConfig>,
    ) -> Result<Created, A2aError> {
        let now = now_ms();
        let task_id = new_id();
        let existing = context.as_ref().map(|context| context.context_id.clone());
        let (context_id, new_context) = match context {
            Some(context) => (context.context_id, None),
            None => {
                let context_id = new_id();
                let context = ContextRow {
                    context_id: context_id.clone(),
                    entry: who.entry.id.clone(),
                    caller: who.caller.clone(),
                    session: SessionRef {
                        source: who.entry.source,
                        session_id: None,
                        cwd: who.entry.cwd.clone(),
                        branch: None,
                    },
                    created_ms: now,
                    closed_ms: None,
                };
                (context_id, Some(context))
            }
        };
        message.context_id = context_id.clone();
        message.task_id = task_id.clone();
        let client_message_id = format!("a2a:{}:{}", who.caller, message.message_id);
        let message_key = (making == Making::Client).then(|| (who.caller.clone(), message.message_id.clone()));
        let mut task = TaskRow {
            id: task_id.clone(),
            context_id,
            entry: who.entry.id.clone(),
            caller: who.caller.clone(),
            state: TaskState::Submitted,
            status_message: None,
            status_ms: now,
            artifacts: Vec::new(),
            history: Vec::new(),
            client_message_id,
            dispatched: false,
            seq: 1,
            created_ms: now,
            updated_ms: now,
        };
        task.queue_history(message);
        let first = StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
            task_id: task.id.clone(),
            context_id: task.context_id.clone(),
            status: task.status(),
            metadata: None,
        });
        let new = NewTask {
            event: EventRow {
                seq: 1,
                kind: EventKind::Status.as_str().into(),
                payload: serde_json::to_string(&first).unwrap_or_default(),
            },
            push: push.map(|push| push_row(&task_id, push, now)),
            task,
            context: new_context,
            message_key,
        };
        {
            let _writer = locked(&self.write);
            if !self.still_reaches(who) {
                return Err(revoked());
            }
            if let Some(context_id) = &existing {
                match self.store.context(context_id) {
                    None => return Err(context_not_found()),
                    Some(context) if context.closed_ms.is_some() => return Err(context_closed()),
                    Some(_) => {}
                }
            }
            if let Some((caller, message_id)) = &new.message_key {
                if let Some(holder) = self.store.message_task(caller, message_id) {
                    return Ok(Created::Duplicate(holder));
                }
                if self.store.queued(caller) >= MAX_QUEUED {
                    return Err(over_quota(format!("at most {MAX_QUEUED} of a caller's tasks may be queued")));
                }
                if new.context.is_some() && self.store.contexts_since(caller, now - DAY_MS) >= MAX_CONTEXTS_PER_DAY {
                    return Err(over_quota(format!("a caller opens at most {MAX_CONTEXTS_PER_DAY} contexts a day")));
                }
            }
            match self.store.create(&new) {
                Ok(()) => {}
                Err(CreateError::Duplicate(holder)) => return Ok(Created::Duplicate(holder)),
                Err(CreateError::Db(error)) => return Err(A2aError::internal(format!("cannot keep the task: {error}"))),
            }
            let sink = self.sink.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
            if let Some(sink) = sink {
                sink.task_event(&TaskEvent {
                    task_id: new.task.id.clone(),
                    context_id: new.task.context_id.clone(),
                    entry_id: new.task.entry.clone(),
                    seq: 1,
                    kind: EventKind::Status,
                    state: TaskState::Submitted,
                    response: first,
                });
                if new.push.is_some() {
                    sink.push_added(&task_id);
                }
            }
        }
        let follow = self
            .follow(Some(who), &task_id, None)
            .ok_or_else(|| A2aError::internal("the task just made is gone"))?;
        Ok(Created::New(Box::new(follow)))
    }

    /// An answer to the question `task` waits on (§3.4).
    async fn answer(&self, who: &Caller, task: TaskRow, message: Message, history_length: Option<i32>, blocking: bool) -> Result<Sent, A2aError> {
        if !who.entry.allow_remote_answers {
            return Err(A2aError::unsupported("this agent takes no answers over A2A: answer in Droppen").with_code(ErrorCode::RemoteAnswersDisabled));
        }
        let open = task
            .pending_question()
            .ok_or_else(|| A2aError::unsupported("the task is not waiting on a question"))?;
        let answer = answer_of(&message, &open)?;
        let task_ref = self.task_ref(&task).ok_or_else(|| A2aError::internal("the task's context is gone"))?;
        if let Some(holder) = self.claim(who, &message, &task.id)? {
            return self.again(who, &holder, history_length, blocking);
        }
        // Asked again right before the backend is: a revocation while the
        // request was on its way takes it back (§6).
        if !self.still_reaches(who) {
            self.store.release_message(&who.caller, &message.message_id);
            return Err(revoked());
        }
        match self.backend.answer(&task_ref, answer).await {
            Ok(()) => {
                self.mutate(&task.id, |row| {
                    let mut message = message;
                    message.task_id = row.id.clone();
                    row.push_history(message);
                    if row.state == TaskState::InputRequired && row.pending_question().as_deref() == Some(open.as_str()) {
                        row.state = TaskState::Working;
                        row.status_message = None;
                        return (vec![Emit::Status], ());
                    }
                    (Vec::new(), ())
                });
            }
            Err(BackendError::Gone(reason)) => self.end(&task.id, TaskState::Failed, &reason, None),
            Err(BackendError::QuestionClosed) => {
                self.store.release_message(&who.caller, &message.message_id);
                return Err(A2aError::unsupported("the question was already answered or withdrawn"));
            }
            Err(BackendError::Invalid(reason)) => {
                self.store.release_message(&who.caller, &message.message_id);
                return Err(A2aError::invalid(reason));
            }
            Err(other) => {
                self.store.release_message(&who.caller, &message.message_id);
                return Err(A2aError::internal(other.to_string()));
            }
        }
        self.again(who, &task.id, history_length, blocking)
    }

    /// More for a task still submitted or working: into its first message
    /// while it waits for a slot, to the session once it has gone.
    async fn append(&self, who: &Caller, task: TaskRow, message: Message, history_length: Option<i32>, blocking: bool) -> Result<Sent, A2aError> {
        if let Some(holder) = self.claim(who, &message, &task.id)? {
            return self.again(who, &holder, history_length, blocking);
        }
        let mut message = message;
        message.task_id = task.id.clone();
        let size = json_size(&message);
        let appended = self
            .mutate(&task.id, |row| {
                if row.dispatched || row.state.is_terminal() {
                    return (Vec::new(), Appended::Handed);
                }
                if row.history.len() >= HISTORY_KEPT || json_size(&row.history) + size > WAITING_BYTES {
                    return (Vec::new(), Appended::Full);
                }
                row.queue_history(message.clone());
                (Vec::new(), Appended::Queued)
            })
            .map_or(Appended::Handed, |(_, appended)| appended);
        if appended == Appended::Full {
            self.store.release_message(&who.caller, &message.message_id);
            return Err(over_quota(format!(
                "a task waiting for a slot holds at most {HISTORY_KEPT} messages and {} MiB: send more once it runs",
                WAITING_BYTES / (1024 * 1024)
            )));
        }
        if appended == Appended::Handed {
            let task = self.store.task(&task.id).ok_or_else(A2aError::task_not_found)?;
            if task.state.is_terminal() {
                self.store.release_message(&who.caller, &message.message_id);
                return Err(A2aError::unsupported("the task finished meanwhile"));
            }
            // Asked again right before the backend is (§6).
            if !self.still_reaches(who) {
                self.store.release_message(&who.caller, &message.message_id);
                return Err(revoked());
            }
            let task_ref = self.task_ref(&task).ok_or_else(|| A2aError::internal("the task's context is gone"))?;
            let (request, images) = request_of(std::slice::from_ref(&message));
            let sent = self
                .backend
                .send(SendToSession {
                    task: task_ref,
                    kind: SendKind::Append,
                    prompt: Prompt {
                        text: render(who.entry.template(), &self.caller_label(&who.caller), &request, &fence()),
                        images,
                    },
                    client_message_id: format!("a2a:{}:{}", who.caller, message.message_id),
                })
                .await;
            match sent {
                Ok(()) => {
                    self.mutate(&task.id, |row| {
                        row.push_history(message);
                        (Vec::new(), ())
                    });
                }
                Err(BackendError::Gone(reason)) => self.end(&task.id, TaskState::Failed, &reason, None),
                Err(BackendError::Invalid(reason)) => {
                    self.store.release_message(&who.caller, &message.message_id);
                    return Err(A2aError::invalid(reason));
                }
                Err(other) => {
                    self.store.release_message(&who.caller, &message.message_id);
                    return Err(A2aError::internal(other.to_string()));
                }
            }
        }
        self.again(who, &task.id, history_length, blocking)
    }

    /// Claim `(caller, messageId)` for `task_id`; the task that holds it
    /// already, if one does.
    fn claim(&self, who: &Caller, message: &Message, task_id: &str) -> Result<Option<String>, A2aError> {
        self.store
            .claim_message(&who.caller, &message.message_id, task_id, now_ms())
            .map_err(|error| A2aError::internal(format!("cannot keep the message: {error}")))
    }

    /// `GetTask`.
    pub fn get_task(&self, who: &Caller, task_id: &str, history_length: Option<i32>) -> Result<Task, A2aError> {
        if history_length.is_some_and(|n| n < 0) {
            return Err(A2aError::invalid("historyLength is not negative"));
        }
        Ok(self.scoped(who, task_id)?.to_task(history_length, None))
    }

    /// `ListTasks`: the caller's tasks of this entry, the most recently
    /// changed first.
    pub fn list_tasks(&self, who: &Caller, request: &ListTasksRequest) -> Result<ListTasksResponse, A2aError> {
        let page_size = match request.page_size {
            None => 50,
            Some(n) if (1..=100).contains(&n) => n,
            Some(_) => return Err(A2aError::invalid("pageSize is 1 to 100")),
        };
        if request.history_length.is_some_and(|n| n < 0) {
            return Err(A2aError::invalid("historyLength is not negative"));
        }
        let after = if request.page_token.is_empty() {
            None
        } else {
            Some(decode_page_token(&request.page_token).ok_or_else(|| A2aError::invalid("pageToken is not one this server gave"))?)
        };
        let status_after_ms = match &request.status_timestamp_after {
            None => None,
            Some(stamp) if stamp.is_empty() => None,
            Some(stamp) => Some(
                droppen_base::time::parse_iso(stamp)
                    .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
                    .map(|since| since.as_millis() as i64)
                    .ok_or_else(|| A2aError::invalid("statusTimestampAfter is an ISO 8601 instant"))?,
            ),
        };
        let filter = TaskFilter {
            entry: Some(who.entry.id.clone()),
            caller: Some(who.caller.clone()),
            context_id: (!request.context_id.is_empty()).then(|| request.context_id.clone()),
            state: request.status.filter(|state| *state != TaskState::Unspecified),
            status_after_ms,
        };
        let (mut rows, total) = self.store.list(&filter, after, page_size as usize + 1);
        let next_page_token = if rows.len() > page_size as usize {
            rows.truncate(page_size as usize);
            rows.last().map(|last| encode_page_token(last.status_ms, &last.id)).unwrap_or_default()
        } else {
            String::new()
        };
        let include = request.include_artifacts.unwrap_or(false);
        // A list carries no history unless it is asked for: a task's history
        // is up to a megabyte (v1.0 §3.2.4 leaves the default to the server).
        let history_length = request.history_length.or(Some(0));
        Ok(ListTasksResponse {
            tasks: rows.iter().map(|row| row.to_task(history_length, Some(include))).collect(),
            next_page_token,
            page_size,
            total_size: i32::try_from(total).unwrap_or(i32::MAX),
        })
    }

    /// `CancelTask` (§3.1): withdrawn while it waits, its turn interrupted
    /// once it runs; the session stays.
    pub async fn cancel_task(&self, who: &Caller, task_id: &str) -> Result<Task, A2aError> {
        let task = self.scoped(who, task_id)?;
        if task.state.is_terminal() {
            return Err(A2aError::new(ErrorKind::TaskNotCancelable, "the task is finished already"));
        }
        self.cancel(task, None).await?;
        Ok(self.scoped(who, task_id)?.to_task(None, None))
    }

    /// Cancel `task`, whoever asks; `code` says why in its status message
    /// (`droppen.revoked` for a revocation, `droppen.context_closed` for a
    /// closed context).
    pub(crate) async fn cancel(&self, task: TaskRow, code: Option<ErrorCode>) -> Result<(), A2aError> {
        let scenario = locked(&self.scenarios).remove(&task.id);
        if let Some(handle) = scenario {
            handle.abort();
        }
        let text = canceled_text(code);
        let canceled = |row: &mut TaskRow| {
            row.state = TaskState::Canceled;
            row.status_message = Some(row.agent_message(vec![Part::text(text)], code.map(code_metadata)));
            (vec![Emit::Status], Some(row.entry.clone()))
        };
        let waiting = self.mutate(&task.id, |row| {
            if row.state.is_terminal() {
                return (Vec::new(), (true, None));
            }
            if !row.dispatched || self.store.entry(&row.entry).is_some_and(|entry| entry.source == EntrySource::Tck) {
                let (emits, entry) = canceled(row);
                return (emits, (true, entry));
            }
            (Vec::new(), (false, None))
        });
        let Some((_, (done, entry))) = waiting else {
            return Err(A2aError::task_not_found());
        };
        if let Some(entry) = entry {
            self.pump(&entry);
        }
        if done {
            return Ok(());
        }
        let task_ref = self.task_ref(&task).ok_or_else(|| A2aError::internal("the task's context is gone"))?;
        match self.backend.cancel(&task_ref).await {
            Ok(()) | Err(BackendError::Gone(_)) => {
                let ended = self.mutate(&task.id, |row| {
                    if row.state.is_terminal() {
                        return (Vec::new(), None);
                    }
                    canceled(row)
                });
                if let Some((_, Some(entry))) = ended {
                    self.pump(&entry);
                }
                Ok(())
            }
            Err(other) => Err(A2aError::internal(other.to_string())),
        }
    }

    /// `SubscribeToTask`.
    pub fn subscribe(&self, who: &Caller, task_id: &str) -> Result<Follow, A2aError> {
        let task = self.scoped(who, task_id)?;
        if task.state.is_terminal() {
            return Err(A2aError::unsupported("the task is finished: GetTask reads it"));
        }
        self.follow(Some(who), task_id, None).ok_or_else(A2aError::task_not_found)
    }

    /* ── push configurations ─────────────────────────────────────────── */

    fn check_push(&self, entry: &A2aEntry, push: &TaskPushNotificationConfig) -> Result<(), A2aError> {
        if push.url.is_empty() {
            return Err(A2aError::invalid("a push configuration needs a url"));
        }
        // The rule the sender holds every attempt to (§6), so a configuration
        // made here is one it delivers to while the entry stays as it is.
        if let Err(refusal) = crate::push::guard::push_target(&push.url, &entry.push_origins, entry.source, self.settings.tck) {
            // The whole request is refused, with a code a client can tell
            // apart: it may send it again without the push configuration.
            return Err(
                A2aError::invalid(format!("a push url is https, at an origin this agent's pushOrigins lists: {refusal}"))
                    .with_code(ErrorCode::PushOriginNotAllowed),
            );
        }
        if push.id.chars().count() > 128 || push.id.chars().any(char::is_control) {
            return Err(A2aError::invalid("a push configuration's id is at most 128 characters"));
        }
        Ok(())
    }

    /// Keep a push configuration of `task_id`: told the task from its first
    /// event when it came with the message that made the task (`from_start`),
    /// else from the next event on.
    fn put_push(&self, task_id: &str, push: TaskPushNotificationConfig, from_start: bool) -> Result<TaskPushNotificationConfig, A2aError> {
        let row = push_row(task_id, push, now_ms());
        let existing = self.store.pushes(task_id);
        if existing.len() >= MAX_PUSH_CONFIGS && !existing.iter().any(|kept| kept.id == row.id) {
            return Err(over_quota(format!("a task has at most {MAX_PUSH_CONFIGS} push configurations")));
        }
        self.store
            .put_push(&row, from_start)
            .map_err(|error| A2aError::internal(format!("cannot keep the push configuration: {error}")))?;
        let sink = self.sink.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        if let Some(sink) = sink {
            sink.push_added(task_id);
        }
        Ok(push_config(&row))
    }

    /// `CreateTaskPushNotificationConfig`.
    pub fn create_push(&self, who: &Caller, task_id: &str, push: TaskPushNotificationConfig) -> Result<TaskPushNotificationConfig, A2aError> {
        self.scoped(who, task_id)?;
        self.check_push(&who.entry, &push)?;
        self.put_push(task_id, push, false)
    }

    /// `GetTaskPushNotificationConfig`.
    pub fn get_push(&self, who: &Caller, task_id: &str, id: &str) -> Result<TaskPushNotificationConfig, A2aError> {
        self.scoped(who, task_id)?;
        self.store
            .push(task_id, id)
            .map(|row| push_config(&row))
            .ok_or_else(A2aError::task_not_found)
    }

    /// `ListTaskPushNotificationConfigs`.
    pub fn list_push(&self, who: &Caller, task_id: &str) -> Result<ListPushConfigsResponse, A2aError> {
        self.scoped(who, task_id)?;
        Ok(ListPushConfigsResponse {
            configs: self.store.pushes(task_id).iter().map(push_config).collect(),
            next_page_token: String::new(),
        })
    }

    /// `DeleteTaskPushNotificationConfig`: a configuration already gone is no
    /// error.
    pub fn delete_push(&self, who: &Caller, task_id: &str, id: &str) -> Result<(), A2aError> {
        self.scoped(who, task_id)?;
        self.store
            .delete_push(task_id, id)
            .map_err(|error| A2aError::internal(format!("cannot remove the push configuration: {error}")))
    }

    /* ── what the TCK's scenarios do ─────────────────────────────────── */

    /// Set a task's state and status text (`None`: no message).
    pub(crate) fn set_state(&self, task_id: &str, state: TaskState, text: Option<&str>) {
        let changed = self.mutate(task_id, |task| {
            if task.state.is_terminal() {
                return (Vec::new(), None);
            }
            task.state = state;
            task.dispatched = true;
            task.status_message = text.map(|text| task.agent_message(vec![Part::text(text)], None));
            (vec![Emit::Status], Some(task.entry.clone()))
        });
        if let Some((_, Some(entry))) = changed {
            if state.is_settled() {
                self.pump(&entry);
            }
        }
    }

    /// Add an artifact, or a chunk of one.
    pub(crate) fn add_artifact(&self, task_id: &str, artifact: Artifact, append: bool, last: bool) {
        self.mutate(task_id, |task| {
            if task.state.is_terminal() {
                return (Vec::new(), ());
            }
            match task.artifacts.iter_mut().find(|existing| existing.artifact_id == artifact.artifact_id) {
                Some(existing) if append => existing.parts.extend(artifact.parts.clone()),
                _ => put_artifact(&mut task.artifacts, artifact.clone()),
            }
            (vec![Emit::Chunk { artifact, append, last }], ())
        });
    }

    /// A TCK scenario of `task_id` runs as `handle`.
    pub(crate) fn scenarios_insert(&self, task_id: &str, handle: tokio::task::AbortHandle) {
        if !handle.is_finished() && self.store.task(task_id).is_some_and(|task| !task.state.is_terminal()) {
            locked(&self.scenarios).insert(task_id.to_string(), handle);
        }
    }

    /// A TCK scenario of `task_id` has played out.
    pub(crate) fn scenarios_done(&self, task_id: &str) {
        locked(&self.scenarios).remove(task_id);
    }

    /// Keep a push configuration of a task as if its first message came with
    /// it, unchecked: what a test plants.
    #[cfg(test)]
    pub(crate) fn add_push(&self, task_id: &str, push: TaskPushNotificationConfig) -> Result<TaskPushNotificationConfig, A2aError> {
        self.put_push(task_id, push, true)
    }

    /// Record a message a TCK task was sent.
    pub(crate) fn note_message(&self, task_id: &str, message: Message) {
        self.mutate(task_id, |task| {
            let mut message = message;
            message.task_id = task.id.clone();
            message.context_id = task.context_id.clone();
            task.push_history(message);
            (Vec::new(), ())
        });
    }

    /// A task `caller` may see, for a scenario.
    pub(crate) fn scoped_task(&self, who: &Caller, id: &str) -> Result<TaskRow, A2aError> {
        self.scoped(who, id)
    }
}

/// What [`A2a::create`] made.
pub(crate) enum Created {
    /// A new task, followed.
    New(Box<Follow>),
    /// Nothing: the message was taken already, by this task.
    Duplicate(String),
}

/// A push configuration of `task_id` as it is kept; its id made up when it
/// has none.
fn push_row(task_id: &str, push: TaskPushNotificationConfig, now: i64) -> PushRow {
    PushRow {
        task_id: task_id.to_string(),
        id: if push.id.is_empty() { new_id() } else { push.id },
        url: push.url,
        token: (!push.token.is_empty()).then_some(push.token),
        auth_scheme: push
            .authentication
            .as_ref()
            .map(|auth| auth.scheme.clone())
            .filter(|scheme| !scheme.is_empty()),
        auth_credentials: push
            .authentication
            .as_ref()
            .map(|auth| auth.credentials.clone())
            .filter(|credentials| !credentials.is_empty()),
        created_ms: now,
    }
}

fn push_config(row: &PushRow) -> TaskPushNotificationConfig {
    TaskPushNotificationConfig {
        tenant: String::new(),
        id: row.id.clone(),
        task_id: row.task_id.clone(),
        url: row.url.clone(),
        token: row.token.clone().unwrap_or_default(),
        authentication: row.auth_scheme.as_ref().map(|scheme| AuthenticationInfo {
            scheme: scheme.clone(),
            credentials: row.auth_credentials.clone().unwrap_or_default(),
        }),
    }
}

/// A page token: where a page ended, as `(time, id)`.
pub(crate) fn encode_page_token(status_ms: i64, id: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("{status_ms}:{id}"))
}

/// The `(time, id)` a page token says a page ended at.
pub(crate) fn decode_page_token(token: &str) -> Option<(i64, String)> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(token).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    let (ms, id) = text.split_once(':')?;
    Some((ms.parse().ok()?, id.to_string()))
}

/// A task followed from a snapshot: the snapshot, then every event after it,
/// until it settles.
pub struct Follow {
    a2a: Arc<A2a>,
    who: Option<Caller>,
    revocations: tokio::sync::watch::Receiver<u64>,
    snapshot: TaskRow,
    rx: Option<broadcast::Receiver<Arc<TaskEvent>>>,
    backlog: std::collections::VecDeque<StreamResponse>,
    last_seq: i64,
    done: bool,
    history_length: Option<i32>,
}

impl Follow {
    /// This follow, its snapshots cut to `history_length`.
    pub(crate) fn history(mut self: Box<Self>, history_length: Option<i32>) -> Box<Self> {
        self.history_length = history_length;
        self
    }

    /// The task as it stood when it was followed.
    pub fn snapshot(&self) -> Task {
        self.snapshot.to_task(self.history_length, None)
    }

    /// The task's id.
    pub fn task_id(&self) -> &str {
        &self.snapshot.id
    }

    /// The next event after the snapshot; `None` once the task has settled,
    /// or the server stops.
    pub async fn next(&mut self) -> Option<StreamResponse> {
        let response = self.next_unchecked().await?;
        // The caller is asked again before every event goes out (§6).
        if !self.reaches() {
            self.stop();
            return None;
        }
        Some(response)
    }

    /// End the stream: a revoked caller gets nothing more, not even what was
    /// read back.
    fn stop(&mut self) {
        self.done = true;
        self.backlog.clear();
    }

    async fn next_unchecked(&mut self) -> Option<StreamResponse> {
        enum Woke {
            Stopped,
            Revoked,
            Event(Result<Arc<TaskEvent>, broadcast::error::RecvError>),
        }
        loop {
            if let Some(response) = self.backlog.pop_front() {
                return Some(response);
            }
            if self.done {
                return None;
            }
            let woke = {
                let rx = self.rx.as_mut()?;
                tokio::select! {
                    () = self.a2a.cancel.cancelled() => Woke::Stopped,
                    changed = self.revocations.changed() => if changed.is_ok() { Woke::Revoked } else { Woke::Stopped },
                    received = rx.recv() => Woke::Event(received),
                }
            };
            match woke {
                Woke::Stopped => {
                    self.stop();
                    return None;
                }
                Woke::Revoked => {
                    if !self.reaches() {
                        self.stop();
                        return None;
                    }
                }
                Woke::Event(Ok(event)) => {
                    if event.seq <= self.last_seq {
                        continue;
                    }
                    self.last_seq = event.seq;
                    if event.kind == EventKind::Status && event.state.is_settled() {
                        self.done = true;
                    }
                    return Some(event.response.clone());
                }
                Woke::Event(Err(broadcast::error::RecvError::Lagged(_))) => self.read_back(),
                Woke::Event(Err(broadcast::error::RecvError::Closed)) => {
                    self.read_back();
                    self.done = true;
                }
            }
        }
    }

    /// Whether the caller this follows for may still reach the task's entry.
    fn reaches(&self) -> bool {
        self.who.as_ref().is_none_or(|who| self.a2a.still_reaches(who))
    }

    /// What the stream missed, from the event table.
    fn read_back(&mut self) {
        for event in self.a2a.store.events_after(&self.snapshot.id, self.last_seq) {
            let Ok(response) = serde_json::from_str::<StreamResponse>(&event.payload) else {
                continue;
            };
            self.last_seq = event.seq;
            let settled = match &response {
                StreamResponse::StatusUpdate(update) => event.kind == EventKind::Status.as_str() && update.status.state.is_settled(),
                _ => false,
            };
            self.backlog.push_back(response);
            if settled {
                self.done = true;
                return;
            }
        }
    }

    /// Wait for the task to settle — terminal, or waiting on an answer — and
    /// answer it as it then stands, its artifacts included.
    pub async fn settle(mut self) -> Task {
        while self.next().await.is_some() {}
        let history_length = self.history_length;
        self.a2a
            .store
            .task(&self.snapshot.id)
            .map(|row| row.to_task(history_length, None))
            .unwrap_or_else(|| self.snapshot.to_task(history_length, None))
    }
}
