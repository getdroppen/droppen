//! The sessions behind the A2A server (`docs/a2a.md` §3, §5): the
//! [`SessionBackend`] the protocol side drives, over this server's providers.
//!
//! The protocol side keeps the tasks: their states, the concurrency gate,
//! the prompt template, idempotency and the streams. This is the other half:
//! it starts a context's session and sends it the next task, answers a
//! task's question, cancels a turn, and says what became of each task as
//! [`BackendEvent`]s. It lives in the entry because it needs what only the
//! entry sees together: the drive's start and answer paths, the providers'
//! drivers through the `Alerts` hook, the session readers, git, and the
//! Droppen link (for the device's name and the session's my.droppen.app
//! address).
//!
//! # Which turn is a task's
//!
//! Every message a task sends travels under a queue id it claimed in the
//! receipt store ([`droppen_provider::receipts`]), under the task's
//! `clientMessageId`, and the drivers report by that id when the message is
//! taken into a turn (`sent`). The first turn that ends after that — the
//! drivers' [`droppen_provider::Alerts::turn`], called as the turn closes and
//! before the next queued message goes in — is the task's turn, provided none
//! of the task's own messages is still queued. So a task sent to a busy
//! session stays `SUBMITTED` behind the turn in front of it, and a message
//! typed on the Droppen page belongs to no task.
//!
//! "Ended" is the driver's: no message of the task queued, and no background
//! work claude is still minding with nothing queued after it — claude opens a
//! turn of its own when that work finishes, and the task ends with that one.
//!
//! The rest is read as it happens, every half second: the receipt that moves the
//! task to `WORKING`, a question it asks and one answered in Droppen, and the
//! agent's latest sentence.
//!
//! # A restart
//!
//! What a task is (its messages' queue ids, where it stood) is written down
//! (`store`), and [`SessionBackend::resume`] reads it against what the
//! receipts, the questions and the drivers the boot adopted say now.

mod git;
mod store;
#[cfg(test)]
mod tests;
mod turn;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use futures_util::future::BoxFuture;
use tokio::sync::{mpsc, watch};

use droppen_a2a::backend::{
    self as a2a, BackendError, BackendEvent, EntrySource, ErrorCode, EventInbox, Isolation, Question, QuestionItem, ResultData, Resumed, RunSpec,
    SendKind, SendToSession, SessionBackend, StartSession, Started, TaskRef, TurnOutcome, TurnResult,
};
use droppen_drive::routes::{NewSession, NewSessionResult, SendMessageResult};
use droppen_drive::{Drive, DriveState};
use droppen_http::error::ApiError;
use droppen_http::openapi_ext::Optional;
use droppen_provider::questions;
use droppen_provider::receipts::{Claim, Kind, Receipts};
use droppen_provider::{QueueOutcome, Reply, SendOptions};
use droppen_sessions::delivery::{QuestionState, ReceiptStatus};
use droppen_sessions::{DriverStates, InjectionRunState, ResidentPhase, Source};

use store::{ContextRecord, Message, TaskRecord};

/// How often the tasks this backend follows are looked at: the receipt that
/// says a task's message went into its turn, its questions, its progress.
const TICK: Duration = Duration::from_millis(500);

/// How many ticks go by between two readings of a working task's latest
/// sentence. The protocol side sends at most one status message per five
/// seconds whatever comes in; this keeps the transcript reads to one per
/// task every two seconds.
const PROGRESS_EVERY: u32 = 4;

/// How many looks in a row a task's session must be found at rest, its turn
/// ended without the hook having said so, before the task is ended anyway: a
/// turn end no driver reported (one replayed after a restart, a reply that
/// overtook its own acknowledgement) must not leave a task working for ever.
const AT_REST_LOOKS: u8 = 4;

/// How long a resume waits for the boot to have adopted the runs the
/// previous server left behind, without which a running turn reads as gone.
const ADOPTION_WAIT: Duration = Duration::from_secs(60);

/// How long a call waits for an earlier attempt under the same message id
/// to answer.
const CLAIM_WAIT: Duration = Duration::from_secs(5);

/// How long the end of a turn waits for its reply to reach the transcript.
const REPLY_WAIT: [Duration; 3] = [Duration::from_millis(200), Duration::from_millis(500), Duration::from_millis(1000)];

/// How many cancels of tasks this backend did not know are remembered, for a
/// start or a send of one still on its way in.
const CANCELLED_KEPT: usize = 256;

/// How many images one message may carry: the composer's own cap.
const MAX_IMAGES: usize = 20;
/// How large one image may be: what a CLI is handed at most
/// ([`droppen_provider::chat_attachments::MAX_IMAGE_BYTES`]).
const MAX_IMAGE_BYTES: usize = droppen_provider::chat_attachments::MAX_IMAGE_BYTES as usize;
/// How large a message's images may be together: the protocol side's cap on a
/// whole request.
const MAX_IMAGES_BYTES: usize = 10 * 1024 * 1024;

/// The sessions behind the A2A server: the real [`SessionBackend`].
///
/// Made once per server, with the state ([`A2aSessions::new`]); the drivers'
/// alerts reach it through [`A2aSessions::alerts`], and the protocol side
/// takes it and its events' inbox ([`A2aSessions::take_inbox`]) when it is
/// built.
pub struct A2aSessions {
    inner: Arc<Inner>,
    /// The receiving end of the events, until the protocol side takes it.
    inbox: Mutex<Option<EventInbox>>,
}

/// What the backend works with and what it follows.
struct Inner {
    /// The providers, the readers, the receipts and the questions.
    drive: Arc<Drive>,
    /// The same, as the drive's start path takes it.
    state: DriveState,
    /// The Droppen link: the device's name and its my.droppen.app address.
    cloud: Arc<droppen_cloud::Cloud>,
    /// Where the events go.
    report: Reporter,
    /// What survives a restart.
    store: store::Store,
    /// Every task followed now, by id.
    tasks: Mutex<HashMap<String, Task>>,
    /// The tasks cancelled before this backend followed them, newest last: a
    /// revocation cancels every unfinished task of its caller, the ones whose
    /// start is still on its way in among them.
    cancelled: Mutex<VecDeque<String>>,
    /// To the watcher: a turn the hook saw end, a question to look at.
    internal: mpsc::UnboundedSender<Internal>,
    /// Set once the boot has adopted what the previous server left behind.
    adopted: watch::Sender<bool>,
}

/// The events' way out, and in tests a copy of each for the test.
struct Reporter {
    events: a2a::Events,
    #[cfg(test)]
    tap: Mutex<Option<mpsc::UnboundedSender<BackendEvent>>>,
}

impl Reporter {
    fn send(&self, event: BackendEvent) {
        #[cfg(test)]
        if let Some(tap) = self.tap.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
            let _ = tap.send(event.clone());
        }
        self.events.send(event);
    }
}

/// A task followed now.
#[derive(Debug, Clone)]
struct Task {
    /// What is written down of it.
    record: TaskRecord,
    /// Its session's CLI.
    source: Source,
    /// Where its turn stands.
    stage: Stage,
    /// The call that made it (a start, or a new turn's send) has not
    /// answered yet: nothing is reported of it meanwhile, and its end waits.
    calling: bool,
    /// Cancelled while that call was still out: the call stops it when it
    /// answers.
    cancelled: bool,
    /// When its message was seen taken into its turn, epoch milliseconds.
    delivered_ms: Option<i64>,
    /// The last progress line reported.
    progress: Option<String>,
    /// Its session's transcript, once found.
    transcript: Option<PathBuf>,
    /// The question answered through [`SessionBackend::answer`], whose close
    /// is no news.
    answered: Option<String>,
    /// How many looks in a row found its session at rest.
    at_rest: u8,
}

/// Where a followed task's turn stands.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Stage {
    /// Its message is on its way to its turn, or in it.
    Running,
    /// Its turn waits on the question with this request id.
    Asking(String),
    /// Its turn ended; the end is being reported.
    Ending,
}

/// How the hook found a task's turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ending {
    /// Well: `COMPLETED`, with the artifacts.
    Completed,
    /// Badly: `FAILED`, with why.
    Failed(String),
}

/// What the hooks hand the watcher.
enum Internal {
    /// The task's turn ended.
    Ended { task_id: String, ending: Ending },
    /// A session asked a question: look now rather than at the next tick.
    Look,
}

/// What a claim on a message id found.
enum Claimed {
    /// This call owns it: the message goes in under this queue id.
    Owned(String),
    /// An earlier attempt got it accepted already.
    Earlier {
        /// The queue id it went in under, when known.
        qid: Option<String>,
        /// Its session, when known.
        session: Option<String>,
    },
}

fn rejected(code: Option<ErrorCode>, message: impl Into<String>) -> BackendError {
    BackendError::Rejected {
        code,
        message: message.into(),
    }
}

/// A route's refusal as one line: its `error`, then its `hint` if it has one.
fn message_of(error: &ApiError) -> String {
    let body = error.body();
    let said = body["error"].as_str().unwrap_or("refused");
    match body["hint"].as_str() {
        Some(hint) => format!("{said}: {hint}"),
        None => said.to_string(),
    }
}

impl A2aSessions {
    /// The backend over this server's drive, its session-list feed and its
    /// Droppen link, and the task that follows what it was given.
    ///
    /// Called from the state's assembly, inside the runtime. Its
    /// [`Self::alerts`] has to be the alerts the drivers are handed
    /// ([`droppen_provider::RunHost::attach_alerts`]), and [`Self::adopted`]
    /// called once the boot has adopted the runs it found.
    pub fn new(
        drive: &Arc<Drive>,
        global: &Arc<droppen_drive::feed::global::GlobalFeed>,
        refresh: &droppen_http::refresh::Refresh,
        cloud: &Arc<droppen_cloud::Cloud>,
    ) -> Arc<Self> {
        let (events, inbox) = a2a::event_channel();
        let (internal, rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            state: DriveState {
                drive: drive.clone(),
                paths: drive.paths.clone(),
                global: global.clone(),
                refresh: refresh.clone(),
            },
            drive: drive.clone(),
            cloud: cloud.clone(),
            report: Reporter {
                events,
                #[cfg(test)]
                tap: Mutex::new(None),
            },
            store: store::Store::new(droppen_instance::db::at(drive.paths.instance_db())),
            tasks: Mutex::new(HashMap::new()),
            cancelled: Mutex::new(VecDeque::new()),
            internal,
            adopted: watch::channel(false).0,
        });
        tokio::spawn(watch_tasks(Arc::downgrade(&inner), rx));
        Arc::new(Self {
            inner,
            inbox: Mutex::new(Some(inbox)),
        })
    }

    /// The events' inbox, for the protocol side to read: once.
    pub fn take_inbox(&self) -> Option<EventInbox> {
        self.inbox.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    /// `alerts`, with this backend told of every turn's end and every
    /// question first: what the drivers are to be handed instead of `alerts`
    /// alone. `alerts` is called exactly as before.
    pub fn alerts<A: droppen_provider::Alerts>(&self, alerts: A) -> Hooked<A> {
        Hooked {
            alerts,
            sessions: Arc::downgrade(&self.inner),
        }
    }

    /// The boot has adopted what the previous server left behind (or owns
    /// nothing to adopt): a resume may read the drivers now.
    pub fn adopted(&self) {
        self.inner.adopted.send_replace(true);
    }

    /// A copy of every event from now on, for a test.
    #[cfg(test)]
    fn tap(&self) -> mpsc::UnboundedReceiver<BackendEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        *self.inner.report.tap.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx);
        rx
    }
}

/// A driver's alerts with the A2A backend told first ([`A2aSessions::alerts`]).
pub struct Hooked<A> {
    alerts: A,
    sessions: Weak<Inner>,
}

impl<A: droppen_provider::Alerts> droppen_provider::Alerts for Hooked<A> {
    fn turn(
        &self,
        paths: Arc<droppen_instance::paths::Paths>,
        source: Source,
        id: String,
        outcome: InjectionRunState,
        error: Option<String>,
        driver: DriverStates,
    ) {
        if let Some(sessions) = self.sessions.upgrade() {
            sessions.on_turn(source, &id, outcome, error.as_deref(), &driver);
        }
        self.alerts.turn(paths, source, id, outcome, error, driver);
    }

    fn question(&self, paths: Arc<droppen_instance::paths::Paths>, source: Source, id: String, question: String) {
        if let Some(sessions) = self.sessions.upgrade() {
            let _ = sessions.internal.send(Internal::Look);
        }
        self.alerts.question(paths, source, id, question);
    }

    fn rotation(
        &self,
        paths: Arc<droppen_instance::paths::Paths>,
        source: Source,
        id: String,
        outcome: droppen_sessions::account::RotationOutcome,
        trigger: droppen_sessions::account::RotationTrigger,
        text: String,
    ) {
        self.alerts.rotation(paths, source, id, outcome, trigger, text);
    }
}

/// Look at every task each tick, and report the ends the hook found, until
/// the backend is gone.
async fn watch_tasks(inner: Weak<Inner>, mut internal: mpsc::UnboundedReceiver<Internal>) {
    let mut ticks = tokio::time::interval(TICK);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut count: u32 = 0;
    // Ends of tasks whose call had not answered yet when their turn ended.
    let mut deferred: Vec<(String, Ending)> = Vec::new();
    loop {
        tokio::select! {
            message = internal.recv() => {
                let Some(message) = message else { break };
                let Some(inner) = inner.upgrade() else { break };
                match message {
                    Internal::Ended { task_id, ending } => {
                        if !inner.finish(&task_id, &ending).await {
                            deferred.push((task_id, ending));
                        }
                    }
                    Internal::Look => inner.look(false).await,
                }
            }
            _ = ticks.tick() => {
                let Some(inner) = inner.upgrade() else { break };
                count = count.wrapping_add(1);
                let mut waiting = Vec::new();
                for (task_id, ending) in std::mem::take(&mut deferred) {
                    if !inner.finish(&task_id, &ending).await {
                        waiting.push((task_id, ending));
                    }
                }
                deferred = waiting;
                inner.look(count.is_multiple_of(PROGRESS_EVERY)).await;
            }
        }
    }
}

impl Inner {
    fn receipts(&self) -> &Arc<Receipts> {
        self.drive.injections.host.receipts()
    }

    /// The source an entry runs, when a provider here runs it.
    fn source(&self, entry: EntrySource) -> Option<Source> {
        self.source_named(entry.as_str())
    }

    /// The source a wire name spells, when a provider here runs it.
    fn source_named(&self, name: &str) -> Option<Source> {
        self.drive.providers.sources().find(|source| source.as_str() == name)
    }

    /// Run `change` on task `id` if it is followed, under the lock, and write
    /// it down after.
    fn with_task<T>(&self, id: &str, change: impl FnOnce(&mut Task) -> T) -> Option<T> {
        let mut tasks = self.tasks.lock().unwrap_or_else(PoisonError::into_inner);
        let task = tasks.get_mut(id)?;
        let before = task.record.clone();
        let answer = change(task);
        if task.record != before {
            self.store.put_task(id, &task.record);
        }
        Some(answer)
    }

    /// Whether task `id` was cancelled before it was followed.
    fn cancelled_early(&self, id: &str) -> bool {
        self.cancelled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|known| known == id)
    }

    /// Follow task `id` from now on, and write it down. A task cancelled
    /// before this is followed as cancelled, for its call to stop.
    fn follow(&self, id: &str, source: Source, record: TaskRecord, stage: Stage, calling: bool) {
        self.store.put_task(id, &record);
        let task = Task {
            record,
            source,
            stage,
            calling,
            cancelled: self.cancelled_early(id),
            delivered_ms: None,
            progress: None,
            transcript: None,
            answered: None,
            at_rest: 0,
        };
        self.tasks.lock().unwrap_or_else(PoisonError::into_inner).insert(id.to_string(), task);
    }

    /// Stop following task `id`, and forget it.
    fn unfollow(&self, id: &str) -> Option<Task> {
        let task = self.tasks.lock().unwrap_or_else(PoisonError::into_inner).remove(id);
        self.store.forget_task(id);
        task
    }

    // ------------------------------------------------------------ the hook

    /// A turn of session `id` ended: whichever task it was the turn of ends
    /// with it.
    ///
    /// Called by the driver as it closes the turn, after the turn is written
    /// back as idle and before the next queued message goes in, so the
    /// receipts and `driver` say exactly what the turn had: a task whose
    /// message is `sent` was in it (or in one before it, still unended), and
    /// one whose message is queued was not. Synchronous and short: a lock, and
    /// a receipt read per message of the session's tasks.
    fn on_turn(&self, source: Source, id: &str, outcome: InjectionRunState, error: Option<&str>, driver: &DriverStates) {
        if outcome == InjectionRunState::Running {
            return;
        }
        let mut tasks = self.tasks.lock().unwrap_or_else(PoisonError::into_inner);
        if tasks.is_empty() {
            return;
        }
        let resident = driver.resident(source, id);
        let queued: Vec<&str> = resident
            .iter()
            .flat_map(|resident| resident.queued_messages.iter())
            .map(|message| message.id.as_str())
            .collect();
        let background = resident.as_ref().is_some_and(|resident| resident.background > 0);
        let receipts = self.receipts();
        let mut ended = Vec::new();
        for (task_id, task) in tasks.iter_mut() {
            if task.source != source || task.stage == Stage::Ending {
                continue;
            }
            let main = receipts.whereabouts(&task.record.main.qid);
            match task.record.session_id.as_deref() {
                Some(session) if session == id => {}
                // A new session's turn can end before its start has answered:
                // the receipt knows the session already.
                None if main.as_ref().and_then(|main| main.session_id.as_deref()) == Some(id) => {
                    task.record.session_id = Some(id.to_string());
                    self.store.put_task(task_id, &task.record);
                }
                _ => continue,
            }
            let ending = match main.map(|main| main.status) {
                Some(ReceiptStatus::Failed) => Some(Ending::Failed(error.unwrap_or("the task's message was not delivered").to_string())),
                Some(ReceiptStatus::Sent) if !queued.contains(&task.record.main.qid.as_str()) => {
                    let more = task.record.appends.iter().any(|message| {
                        queued.contains(&message.qid.as_str())
                            || receipts.whereabouts(&message.qid).is_some_and(|at| at.status == ReceiptStatus::Queued)
                    });
                    match outcome {
                        _ if more => None,
                        InjectionRunState::Failed => Some(Ending::Failed(error.unwrap_or("the turn failed").to_string())),
                        // claude opens a turn of its own once the work it is
                        // minding finishes, and that turn is still this task's
                        // — unless a message is queued, which is the next one's.
                        _ if background && queued.is_empty() => None,
                        _ => Some(Ending::Completed),
                    }
                }
                _ => None,
            };
            if let Some(ending) = ending {
                task.stage = Stage::Ending;
                ended.push((task_id.clone(), ending));
            }
        }
        drop(tasks);
        for (task_id, ending) in ended {
            let _ = self.internal.send(Internal::Ended { task_id, ending });
        }
    }

    // ------------------------------------------------------------ the watcher

    /// Look at every followed task once.
    async fn look(&self, progress: bool) {
        let ids: Vec<String> = self.tasks.lock().unwrap_or_else(PoisonError::into_inner).keys().cloned().collect();
        for id in ids {
            self.look_at(&id, progress).await;
        }
    }

    /// Whether task `id`'s message went into its turn, whether it asks a
    /// question or had one answered in Droppen, and what it is doing.
    async fn look_at(&self, id: &str, progress: bool) {
        let Some(task) = self.with_task(id, |task| task.clone()) else { return };
        if task.calling || task.stage == Stage::Ending {
            return;
        }
        let Some(session) = task.record.session_id.clone() else { return };
        if !task.record.turn_started {
            match self.receipts().whereabouts(&task.record.main.qid).map(|main| main.status) {
                Some(ReceiptStatus::Sent) => {
                    let started = self.with_task(id, |task| {
                        let fresh = task.stage != Stage::Ending && !task.record.turn_started;
                        if fresh {
                            task.record.turn_started = true;
                            task.delivered_ms = Some(droppen_base::time::now_ms());
                        }
                        fresh
                    });
                    if started == Some(true) {
                        self.report.send(BackendEvent::TurnStarted { task_id: id.to_string() });
                    }
                }
                Some(ReceiptStatus::Failed) => self.end(id, Ending::Failed("the task's message was dropped before it reached the session".into())),
                _ => {}
            }
            return;
        }
        match &task.stage {
            Stage::Running => {
                if let Some(question) = self.open_question(task.source, &session, task.answered.as_deref()) {
                    let request_id = question.request_id.clone();
                    let asking = self.with_task(id, |task| {
                        let fresh = task.stage == Stage::Running;
                        if fresh {
                            task.stage = Stage::Asking(request_id.clone());
                            task.record.asked = Some(request_id.clone());
                        }
                        fresh
                    });
                    if asking == Some(true) {
                        self.report.send(BackendEvent::Question {
                            task_id: id.to_string(),
                            question,
                        });
                    }
                    return;
                }
                if self.at_rest(task.source, &session, &task.record) {
                    let looks = self.with_task(id, |task| {
                        task.at_rest = task.at_rest.saturating_add(1);
                        task.at_rest
                    });
                    if looks.is_some_and(|looks| looks >= AT_REST_LOOKS) {
                        self.end(id, self.ended_while_unwatched(task.source, &session));
                    }
                    return;
                }
                self.with_task(id, |task| task.at_rest = 0);
                if progress {
                    self.progress(id, &task, &session).await;
                }
            }
            Stage::Asking(request_id) => {
                let state = self.drive.injections.host.questions().get(request_id).map(|question| question.state);
                // Expired is still answerable — the answer goes to the session
                // as a message — so the task still waits on it.
                if matches!(state, Some(QuestionState::Pending | QuestionState::Expired)) {
                    return;
                }
                let resolved = self.with_task(id, |task| {
                    let here = task.stage == Stage::Asking(request_id.clone());
                    if here {
                        task.stage = Stage::Running;
                        task.record.asked = None;
                    }
                    here && task.answered.as_deref() != Some(request_id.as_str())
                });
                if resolved == Some(true) {
                    self.report.send(BackendEvent::QuestionResolved { task_id: id.to_string() });
                }
            }
            Stage::Ending => {}
        }
    }

    /// Whether nothing more will happen in session `id` for the task `record`
    /// is: no turn in flight, none of its messages waiting, and no background
    /// work claude would open a turn of its own for.
    fn at_rest(&self, source: Source, id: &str, record: &TaskRecord) -> bool {
        let providers = &self.drive.providers;
        let resident = providers.resident_info(source, id);
        let busy = providers.is_running(source, id) || resident.as_ref().is_some_and(|resident| resident.phase != ResidentPhase::Idle);
        let receipts = self.receipts();
        let more = std::iter::once(&record.main).chain(&record.appends).any(|message| {
            providers.holds_queued(source, id, &message.qid)
                || receipts.whereabouts(&message.qid).is_some_and(|at| at.status == ReceiptStatus::Queued)
        });
        let minding = resident.as_ref().is_some_and(|resident| resident.background > 0 && resident.queued == 0);
        !(busy || more || minding)
    }

    /// How a task's turn ended that ended unwatched: as the session's last
    /// run did.
    fn ended_while_unwatched(&self, source: Source, id: &str) -> Ending {
        self.drive
            .providers
            .injection_for(source, id)
            .filter(|injection| injection.state == InjectionRunState::Failed)
            .map_or(Ending::Completed, |injection| {
                Ending::Failed(injection.error.0.unwrap_or_else(|| "the turn failed".to_string()))
            })
    }

    /// The question session `id` is blocked on, unless it is the one this
    /// backend just answered.
    fn open_question(&self, source: Source, id: &str, answered: Option<&str>) -> Option<Question> {
        if !self.drive.providers.capabilities(source).questions {
            return None;
        }
        let stored = self
            .drive
            .injections
            .host
            .questions()
            .pending_for(id)
            .into_iter()
            .find(|question| Some(question.request_id.as_str()) != answered)?;
        Some(question_of(&stored))
    }

    /// Report the task's latest sentence, if it said a new one in its turn.
    async fn progress(&self, id: &str, task: &Task, session: &str) {
        let transcripts = &self.drive.transcripts;
        let path = match &task.transcript {
            Some(path) => path.clone(),
            None => {
                let Some(path) = transcripts.find_file(&self.drive.paths, task.source, session).await else {
                    return;
                };
                self.with_task(id, |task| task.transcript = Some(path.clone()));
                path
            }
        };
        let Some(echo) = transcripts.read_echo(task.source, &path).await else {
            return;
        };
        // What the turn before said is not this turn's progress.
        let said_at = echo.at.as_deref().and_then(droppen_base::time::parse_iso_ms);
        if let (Some(said_at), Some(delivered)) = (said_at, task.delivered_ms) {
            if said_at + 1000 < delivered {
                return;
            }
        }
        let line = turn::progress_line(&echo.text);
        if line.is_empty() || task.progress.as_deref() == Some(line.as_str()) {
            return;
        }
        let fresh = self.with_task(id, |task| {
            let fresh = task.stage == Stage::Running;
            if fresh {
                task.progress = Some(line.clone());
            }
            fresh
        });
        if fresh == Some(true) {
            self.report.send(BackendEvent::Progress {
                task_id: id.to_string(),
                text: line,
            });
        }
    }

    /// Hand task `id`'s end to the watcher.
    fn end(&self, id: &str, ending: Ending) {
        let fresh = self.with_task(id, |task| {
            let fresh = task.stage != Stage::Ending;
            task.stage = Stage::Ending;
            fresh
        });
        if fresh == Some(true) {
            let _ = self.internal.send(Internal::Ended {
                task_id: id.to_string(),
                ending,
            });
        }
    }

    /// Report task `id`'s end, with the artifacts of a completed one, and stop
    /// following it. `false` while the call that made the task has not
    /// answered: its end is reported once it has.
    async fn finish(&self, id: &str, ending: &Ending) -> bool {
        let Some(task) = self.with_task(id, |task| task.clone()) else {
            return true;
        };
        if task.calling {
            return false;
        }
        if !task.record.turn_started {
            self.report.send(BackendEvent::TurnStarted { task_id: id.to_string() });
        }
        // A question answered or withdrawn in Droppen whose turn ended before
        // the watcher looked again: it is still news, and comes first.
        if task.record.asked.is_some() && task.record.asked != task.answered {
            self.report.send(BackendEvent::QuestionResolved { task_id: id.to_string() });
        }
        let outcome = match ending {
            Ending::Failed(why) => TurnOutcome::Failed(why.clone()),
            Ending::Completed => TurnOutcome::Completed(self.artifacts(&task).await),
        };
        // Cancelled while the artifacts were being made: nothing more of it.
        if self.unfollow(id).is_none() {
            return true;
        }
        self.report.send(BackendEvent::TurnEnded {
            task_id: id.to_string(),
            outcome,
        });
        true
    }

    // ------------------------------------------------------------ artifacts

    /// What a completed task's turns produced (§3.3).
    async fn artifacts(&self, task: &Task) -> TurnResult {
        let session = task.record.session_id.clone().unwrap_or_default();
        let probes: Vec<String> = std::iter::once(&task.record.main)
            .chain(&task.record.appends)
            .map(|message| message.probe.clone())
            .collect();
        let mut said = self.said(task.source, &session, &probes).await;
        for wait in REPLY_WAIT {
            if !said.summary.is_empty() {
                break;
            }
            tokio::time::sleep(wait).await;
            said = self.said(task.source, &session, &probes).await;
        }
        let context = self.store.context(&task.record.context_id).unwrap_or_default();
        let branch = match &task.record.branch {
            Some(branch) => Some(branch.clone()),
            None => git::current_branch(&task.record.cwd).await,
        };
        TurnResult {
            result: ResultData {
                source: task.source.as_str().to_string(),
                session_id: session.clone(),
                host: self.host_name().await,
                cwd: task.record.cwd.clone(),
                branch,
                changed_files: git::changed_files(&task.record.cwd, context.base.as_deref()).await,
                pr_urls: turn::pr_urls(said.texts.iter().map(String::as_str)),
                droppen_url: self.droppen_url(task.source, &session).await,
            },
            summary: said.summary,
        }
    }

    /// What the task's turns said in session `id`, read off its transcript.
    async fn said(&self, source: Source, id: &str, probes: &[String]) -> turn::Said {
        let transcripts = &self.drive.transcripts;
        let Some(path) = transcripts.find_file(&self.drive.paths, source, id).await else {
            return turn::Said::default();
        };
        match transcripts.parse(&self.drive.paths, source, &path).await {
            Ok(items) => turn::said(&items, probes),
            Err(error) => {
                tracing::warn!(session = id, %error, "cannot read an A2A task's transcript");
                turn::Said::default()
            }
        }
    }

    /// This device's name: the one its devices call it by, else its host
    /// name.
    async fn host_name(&self) -> String {
        let paths = self.drive.paths.clone();
        let named = tokio::task::spawn_blocking(move || {
            let files = droppen_agent::AgentFiles::of(&paths);
            droppen_agent::state::AgentState::load(&files)
                .ok()
                .flatten()
                .and_then(|state| state.display_name)
        })
        .await
        .ok()
        .flatten()
        .filter(|name| !name.trim().is_empty());
        named.unwrap_or_else(droppen_http::hostname)
    }

    /// The session's page on the Droppen web app, while this device is
    /// linked and runs AI in its account: `<app>/m/<device>/sessions/<source>/<id>`
    /// (`web/src/lib/machines/routes.ts`); `None` otherwise.
    async fn droppen_url(&self, source: Source, id: &str) -> Option<String> {
        let status = self.cloud.control.status();
        if status.state != droppen_agent::status::Standing::Linked {
            return None;
        }
        let machine = status.machine?;
        let hub = status.hub?;
        let app = self.cloud.hub_webs.for_status(&hub).await;
        Some(format!(
            "{}/m/{}/sessions/{}/{}",
            app.trim_end_matches('/'),
            encode_segment(&machine),
            source.as_str(),
            encode_segment(id)
        ))
    }

    // ------------------------------------------------------------ claims

    /// Claim message id `id` for this call, or find what an earlier attempt
    /// made of it.
    ///
    /// The receipt store's own claim, as the drive's routes take it
    /// (`droppen_drive::idempotent`): one attempt owns an id, a retry of an
    /// answered one finds its answer, and one stranded by a crash is taken over
    /// unless a driver shows it got as far as a queue.
    async fn claim(&self, id: &str, kind: Kind, source: Source, session: Option<&str>) -> Result<Claimed, BackendError> {
        let receipts = self.receipts();
        let deadline = tokio::time::Instant::now() + CLAIM_WAIT;
        loop {
            let now = droppen_base::time::now_ms();
            let claim = receipts
                .claim(id, kind, source, session, now)
                .map_err(|error| BackendError::Failed(format!("cannot record the message: {error}")))?;
            match claim {
                Claim::Owned { qid } => return Ok(Claimed::Owned(qid)),
                Claim::Replay(answer) => {
                    let answer: serde_json::Value = serde_json::from_str(&answer).unwrap_or_default();
                    let session = answer["id"].as_str().map(str::to_owned).or_else(|| session.map(str::to_owned));
                    let qid = answer["qid"].as_str().map(str::to_owned).or_else(|| {
                        let session = session.as_deref()?;
                        let lookup = receipts.lookup(source, session, &[id.to_string()], now).ok()?;
                        lookup.receipts.into_iter().find_map(|receipt| receipt.qid.0)
                    });
                    return Ok(Claimed::Earlier { qid, session });
                }
                Claim::Mismatch => return Err(BackendError::Failed("this message's id was already used for another session".into())),
                Claim::InFlight(flight) if flight.stale => {
                    if flight.status != "pending" {
                        return Ok(Claimed::Earlier {
                            qid: Some(flight.qid),
                            session: flight.session_id.or_else(|| session.map(str::to_owned)),
                        });
                    }
                    let took = receipts
                        .take_over(id, flight.created_ms, now)
                        .map_err(|error| BackendError::Failed(format!("cannot record the message: {error}")))?;
                    if took {
                        return Ok(Claimed::Owned(flight.qid));
                    }
                }
                Claim::InFlight(_) => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(BackendError::Failed(
                            "an earlier attempt with this message is still being accepted".into(),
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }

    /// Store a message's images in the upload directory, as `/api/uploads`
    /// stores an attachment: each in a directory of its own, under a name
    /// made here (never the part's own `filename`).
    async fn store_images(&self, images: &[a2a::Image]) -> Result<Vec<String>, String> {
        let mut stored = Vec::with_capacity(images.len());
        for (n, image) in images.iter().enumerate() {
            let extension = if image.media_type == "image/png" { "png" } else { "jpg" };
            let dir = self.drive.paths.uploads_dir().join(droppen_provider::common::new_uuid());
            tokio::fs::create_dir_all(&dir)
                .await
                .map_err(|error| format!("cannot store an image: {error}"))?;
            let path = dir.join(format!("a2a-image-{}.{extension}", n + 1));
            tokio::fs::write(&path, &image.bytes)
                .await
                .map_err(|error| format!("cannot store an image: {error}"))?;
            stored.push(path.to_string_lossy().into_owned());
        }
        Ok(stored)
    }

    // ------------------------------------------------------------ start

    async fn start(&self, start: StartSession) -> Result<Started, BackendError> {
        let StartSession {
            task,
            run,
            prompt,
            client_message_id,
        } = start;
        if self.cancelled_early(&task.task_id) {
            return Err(BackendError::Failed("the task was cancelled before its session started".into()));
        }
        let source = self
            .source(run.source)
            .ok_or_else(|| rejected(None, format!("this Droppen runs no {} sessions", run.source.as_str())))?;
        droppen_drive::routes::validate_text(&prompt.text).map_err(|why| rejected(None, format!("the task's message: {why}")))?;
        check_images(&prompt.images).map_err(|why| rejected(None, why))?;
        if !tokio::fs::metadata(&run.cwd).await.is_ok_and(|meta| meta.is_dir()) {
            return Err(rejected(None, format!("the entry's directory is not there: {}", run.cwd)));
        }
        let qid = match self.claim(&client_message_id, Kind::NewSession, source, None).await? {
            Claimed::Owned(qid) => qid,
            Claimed::Earlier { qid, session } => return self.started_before(&task, source, qid, session, &client_message_id, &prompt.text),
        };
        match self.start_claimed(&task, &run, source, &prompt, &client_message_id, &qid).await {
            Ok(started) => Ok(started),
            Err(error) => {
                self.receipts().abandon(&client_message_id);
                self.unfollow(&task.task_id);
                Err(error)
            }
        }
    }

    /// A start whose message id an earlier attempt got accepted: follow what
    /// it started.
    fn started_before(
        &self,
        task: &TaskRef,
        source: Source,
        qid: Option<String>,
        session: Option<String>,
        client_message_id: &str,
        text: &str,
    ) -> Result<Started, BackendError> {
        let (Some(qid), Some(session)) = (qid, session) else {
            return Err(BackendError::Failed("an earlier start of this task never got as far as a session".into()));
        };
        let context = self.store.context(&task.context_id).unwrap_or_else(|| ContextRecord {
            cwd: task.session.cwd.clone(),
            ..ContextRecord::default()
        });
        let followed = self.tasks.lock().unwrap_or_else(PoisonError::into_inner).contains_key(&task.task_id);
        if !followed {
            let record = self.store.task(&task.task_id).unwrap_or_else(|| TaskRecord {
                context_id: task.context_id.clone(),
                source: source.as_str().to_string(),
                session_id: Some(session.clone()),
                cwd: context.cwd.clone(),
                branch: context.branch.clone(),
                caller: task.caller.clone(),
                main: Message {
                    qid,
                    client_message_id: client_message_id.to_string(),
                    probe: turn::probe(text),
                },
                appends: Vec::new(),
                turn_started: false,
                asked: None,
            });
            self.follow(
                &task.task_id,
                source,
                TaskRecord {
                    session_id: Some(session.clone()),
                    ..record
                },
                Stage::Running,
                false,
            );
        }
        Ok(Started {
            session_id: Some(session),
            cwd: context.cwd,
            branch: context.branch,
        })
    }

    /// Everything a start does once its message id is its own: the worktree,
    /// the images, the session.
    async fn start_claimed(
        &self,
        task: &TaskRef,
        run: &RunSpec,
        source: Source,
        prompt: &a2a::Prompt,
        client_message_id: &str,
        qid: &str,
    ) -> Result<Started, BackendError> {
        let images = self.store_images(&prompt.images).await.map_err(BackendError::Failed)?;
        let (cwd, branch, base) = match run.isolation {
            Isolation::Worktree => {
                let worktree = git::make_worktree(&run.cwd, &task.context_id)
                    .await
                    .map_err(|why| rejected(Some(ErrorCode::IsolationFailed), why))?;
                (worktree.cwd, Some(worktree.branch), Some(worktree.base))
            }
            Isolation::None => (run.cwd.clone(), None, None),
        };
        self.store.put_context(
            &task.context_id,
            &ContextRecord {
                cwd: cwd.clone(),
                branch: branch.clone(),
                base,
                model: run.model.clone(),
                effort: run.effort.clone(),
                permission: run.permission.clone(),
            },
        );
        let record = TaskRecord {
            context_id: task.context_id.clone(),
            source: source.as_str().to_string(),
            session_id: None,
            cwd: cwd.clone(),
            branch: branch.clone(),
            caller: task.caller.clone(),
            main: Message {
                qid: qid.to_string(),
                client_message_id: client_message_id.to_string(),
                probe: turn::probe(&prompt.text),
            },
            appends: Vec::new(),
            turn_started: false,
            asked: None,
        };
        self.follow(&task.task_id, source, record, Stage::Running, true);
        if self.with_task(&task.task_id, |task| task.cancelled) == Some(true) {
            return Err(BackendError::Failed("the task was cancelled before its session started".into()));
        }
        let body = NewSession {
            source,
            cwd: cwd.clone(),
            text: prompt.text.clone(),
            images: Optional((!images.is_empty()).then_some(images)),
            files: Optional(None),
            model: Optional(run.model.clone()),
            effort: Optional(run.effort.clone()),
            permission: Optional(run.permission.clone()),
            // The entry's account, or the CLI's own login: never the default a
            // start from the page would follow, which the entry did not choose.
            account_id: Some(run.account_id.clone()),
            account_from: Optional(None),
            client_message_id: Optional(None),
            acp_agent: Optional(run.acp_agent.clone()),
        };
        let started = droppen_drive::routes::start_session(self.state.clone(), body, Some(qid.to_string()))
            .await
            .map_err(|error| {
                let message = message_of(&error);
                if error.status() == gotcha::axum::http::StatusCode::BAD_REQUEST {
                    rejected(None, message)
                } else {
                    BackendError::Failed(message)
                }
            })?;
        let session = started.id.clone();
        let answer = serde_json::to_string(&NewSessionResult {
            id: session.clone(),
            duplicate: Optional(None),
        })
        .unwrap_or_default();
        self.receipts()
            .complete(client_message_id, &answer, Some(&session), droppen_base::time::now_ms());
        let cancelled = self.with_task(&task.task_id, |task| {
            task.record.session_id = Some(session.clone());
            task.calling = false;
            task.cancelled
        });
        if cancelled == Some(true) {
            if let Some(cancelled) = self.unfollow(&task.task_id) {
                self.stop(&cancelled).await;
            }
        }
        Ok(Started {
            session_id: Some(session),
            cwd,
            branch,
        })
    }

    // ------------------------------------------------------------ send

    async fn send(&self, send: SendToSession) -> Result<(), BackendError> {
        let SendToSession {
            task,
            kind,
            prompt,
            client_message_id,
        } = send;
        if self.cancelled_early(&task.task_id) {
            return Err(BackendError::Failed("the task was cancelled before its message was sent".into()));
        }
        let source = self
            .source(task.session.source)
            .ok_or_else(|| BackendError::Gone(format!("this Droppen runs no {} sessions", task.session.source.as_str())))?;
        let session = task
            .session
            .session_id
            .clone()
            .ok_or_else(|| BackendError::Failed("the context's session is not known yet".into()))?;
        droppen_drive::routes::validate_text(&prompt.text).map_err(|why| rejected(None, format!("the task's message: {why}")))?;
        check_images(&prompt.images).map_err(|why| rejected(None, why))?;
        if kind == SendKind::Append && !self.tasks.lock().unwrap_or_else(PoisonError::into_inner).contains_key(&task.task_id) {
            return Err(BackendError::Failed("this task is not followed here any more".into()));
        }
        // A session that can no longer be sent to whatever runs (an ACP
        // session whose agent was removed) is gone for the task.
        if let Some(summary) = self.drive.transcripts.find_summary(&self.drive.paths, source, &session).await {
            if let Some(reason) = self.drive.providers.send_block(&summary) {
                return Err(BackendError::Gone(reason));
            }
        }
        let message = |qid: String| Message {
            qid,
            client_message_id: client_message_id.clone(),
            probe: turn::probe(&prompt.text),
        };
        let qid = match self.claim(&client_message_id, Kind::Message, source, Some(&session)).await? {
            Claimed::Owned(qid) => qid,
            Claimed::Earlier { qid: Some(qid), .. } => {
                self.follow_sent(&task, source, &session, kind, message(qid));
                return Ok(());
            }
            Claimed::Earlier { qid: None, .. } => return Ok(()),
        };
        let context = self.store.context(&task.context_id).unwrap_or_default();
        let outcome = self
            .send_claimed(&task, source, &session, kind, &prompt, message(qid.clone()), &context)
            .await;
        match outcome {
            Ok(queued) => {
                let answer = serde_json::to_string(&SendMessageResult {
                    accepted: true,
                    queued,
                    qid: Optional(Some(qid)),
                    duplicate: Optional(None),
                    answered: Optional(None),
                })
                .unwrap_or_default();
                self.receipts().complete(&client_message_id, &answer, None, droppen_base::time::now_ms());
                Ok(())
            }
            Err(error) => {
                self.receipts().abandon(&client_message_id);
                Err(error)
            }
        }
    }

    /// Follow a message an earlier attempt got accepted, as the call would
    /// have.
    fn follow_sent(&self, task: &TaskRef, source: Source, session: &str, kind: SendKind, message: Message) {
        match kind {
            SendKind::NewTurn => {
                if self.with_task(&task.task_id, |_| ()).is_none() {
                    self.follow(
                        &task.task_id,
                        source,
                        self.record_for(task, source, session, message),
                        Stage::Running,
                        false,
                    );
                }
            }
            SendKind::Append => {
                self.with_task(&task.task_id, |task| {
                    if !task.record.appends.iter().any(|known| known.qid == message.qid) {
                        task.record.appends.push(message);
                    }
                });
            }
        }
    }

    /// What is written down of a new task in session `session`.
    fn record_for(&self, task: &TaskRef, source: Source, session: &str, main: Message) -> TaskRecord {
        TaskRecord {
            context_id: task.context_id.clone(),
            source: source.as_str().to_string(),
            session_id: Some(session.to_string()),
            cwd: task.session.cwd.clone(),
            branch: task.session.branch.clone(),
            caller: task.caller.clone(),
            main,
            appends: Vec::new(),
            turn_started: false,
            asked: None,
        }
    }

    /// Everything a send does once its message id is its own. `Ok` says
    /// whether the message waits in the session's queue.
    #[allow(clippy::too_many_arguments)]
    async fn send_claimed(
        &self,
        task: &TaskRef,
        source: Source,
        session: &str,
        kind: SendKind,
        prompt: &a2a::Prompt,
        message: Message,
        context: &ContextRecord,
    ) -> Result<bool, BackendError> {
        let qid = message.qid.clone();
        let images = self.store_images(&prompt.images).await.map_err(BackendError::Failed)?;
        // Followed before it is sent, so a turn that ends the moment it goes
        // in is already known to be the task's.
        match kind {
            SendKind::NewTurn => {
                self.follow(
                    &task.task_id,
                    source,
                    self.record_for(task, source, session, message),
                    Stage::Running,
                    true,
                );
                if self.with_task(&task.task_id, |task| task.cancelled) == Some(true) {
                    self.unfollow(&task.task_id);
                    return Err(BackendError::Failed("the task was cancelled before its message was sent".into()));
                }
            }
            SendKind::Append => {
                if self.with_task(&task.task_id, |task| task.record.appends.push(message)).is_none() {
                    return Err(BackendError::Failed("this task is not followed here any more".into()));
                }
            }
        }
        let options = SendOptions {
            // What the context's session was started with, so a process
            // started for this message runs as the first one did.
            model: context.model.clone(),
            effort: context.effort.clone(),
            permission: context.permission.clone(),
            images,
            message_id: Some(qid.clone()),
            ..SendOptions::default()
        };
        let cwd = (!task.session.cwd.is_empty()).then_some(task.session.cwd.as_str());
        let sent = self.drive.providers.send_to_session(source, session, cwd, &prompt.text, &options).await;
        let sent = match sent {
            Ok(sent) => sent,
            Err(error) => {
                match kind {
                    SendKind::NewTurn => {
                        self.unfollow(&task.task_id);
                    }
                    SendKind::Append => {
                        self.with_task(&task.task_id, |task| task.record.appends.retain(|known| known.qid != qid));
                    }
                }
                return Err(BackendError::Failed(error.client_message("could not send the message")));
            }
        };
        match kind {
            SendKind::NewTurn => {
                let cancelled = self.with_task(&task.task_id, |task| {
                    task.calling = false;
                    task.cancelled
                });
                if cancelled == Some(true) {
                    if let Some(cancelled) = self.unfollow(&task.task_id) {
                        self.stop(&cancelled).await;
                    }
                }
            }
            // More for a turn already running goes into it, as the page's
            // steer puts a queued message in; for a task still waiting its
            // turn it waits behind the task's own message.
            SendKind::Append => {
                let running = self.with_task(&task.task_id, |task| task.record.turn_started).unwrap_or(false);
                if sent.queued && running {
                    let steered = self.drive.providers.steer_queued_message(source, session, &qid).await;
                    if steered != QueueOutcome::Done {
                        tracing::info!(session, ?steered, "an A2A task's added message waits for the next turn instead");
                    }
                }
            }
        }
        Ok(sent.queued)
    }

    // ------------------------------------------------------------ answer

    async fn answer(&self, task: &TaskRef, answer: a2a::Answer) -> Result<(), BackendError> {
        let source = self
            .source(task.session.source)
            .ok_or_else(|| BackendError::Gone("the session's CLI is not here".into()))?;
        let session = task
            .session
            .session_id
            .clone()
            .ok_or_else(|| BackendError::Gone("the context has no session".into()))?;
        let reply = Reply::Answer(questions::Answer {
            answers: answer
                .answers
                .iter()
                .map(|(question, value)| {
                    let joined = match value {
                        a2a::AnswerValue::One(text) => text.trim().to_string(),
                        a2a::AnswerValue::Many(choices) => questions::join_choices(choices),
                    };
                    (question.clone(), joined)
                })
                .filter(|(_, value)| !value.is_empty())
                .collect(),
            notes: answer
                .notes
                .iter()
                .map(|(question, note)| (question.clone(), note.trim().to_string()))
                .filter(|(_, note)| !note.is_empty())
                .collect::<BTreeMap<_, _>>(),
            response: answer
                .response
                .as_deref()
                .map(str::trim)
                .filter(|response| !response.is_empty())
                .map(str::to_owned),
        });
        let settled =
            droppen_drive::routes::settle_question(&self.drive, source, &session, &answer.request_id, reply, Some(task.caller.clone())).await;
        match settled {
            Ok(_) => {
                let request_id = answer.request_id.clone();
                self.with_task(&task.task_id, |task| {
                    task.answered = Some(request_id.clone());
                    if task.stage == Stage::Asking(request_id.clone()) {
                        task.stage = Stage::Running;
                    }
                    if task.record.asked.as_deref() == Some(request_id.as_str()) {
                        task.record.asked = None;
                    }
                });
                Ok(())
            }
            Err(error) => {
                let status = error.status();
                if status == gotcha::axum::http::StatusCode::NOT_FOUND || error.body()["error"] == "question_settled" {
                    Err(BackendError::QuestionClosed)
                } else {
                    Err(BackendError::Failed(message_of(&error)))
                }
            }
        }
    }

    // ------------------------------------------------------------ cancel

    async fn cancel(&self, task: &TaskRef) -> Result<(), BackendError> {
        let taken = {
            let mut tasks = self.tasks.lock().unwrap_or_else(PoisonError::into_inner);
            match tasks.get_mut(&task.task_id) {
                // The start or the send that made it stops it once it answers.
                Some(followed) if followed.calling => {
                    followed.cancelled = true;
                    return Ok(());
                }
                Some(_) => tasks.remove(&task.task_id),
                None => None,
            }
        };
        self.store.forget_task(&task.task_id);
        if taken.is_none() {
            // Not followed: ended already, or its start is still on its way in
            // and has not got as far as following it.
            let mut cancelled = self.cancelled.lock().unwrap_or_else(PoisonError::into_inner);
            if !cancelled.iter().any(|known| *known == task.task_id) {
                cancelled.push_back(task.task_id.clone());
                if cancelled.len() > CANCELLED_KEPT {
                    cancelled.pop_front();
                }
            }
        }
        if let Some(taken) = taken {
            self.stop(&taken).await;
        }
        Ok(())
    }

    /// Take a cancelled task's messages back out of its session's queue, or
    /// stop its turn once one of them is in it. A turn that already ended is
    /// left alone: whatever runs now is not the task's.
    async fn stop(&self, task: &Task) {
        let Some(session) = task.record.session_id.as_deref() else { return };
        if task.stage == Stage::Ending {
            return;
        }
        let providers = &self.drive.providers;
        let receipts = self.receipts();
        let sent = |qid: &str| receipts.whereabouts(qid).is_some_and(|at| at.status == ReceiptStatus::Sent);
        for message in &task.record.appends {
            if !sent(&message.qid) {
                providers.drop_queued_message(task.source, session, &message.qid).await;
            }
        }
        let main = &task.record.main.qid;
        let status = receipts.whereabouts(main).map(|at| at.status);
        let running = match status {
            Some(ReceiptStatus::Sent) => true,
            Some(ReceiptStatus::Failed) => false,
            _ => providers.drop_queued_message(task.source, session, main).await != QueueOutcome::Done && sent(main),
        };
        if running && !providers.interrupt(task.source, session) {
            tracing::debug!(session, "a cancelled A2A task's turn had already stopped");
        }
    }

    // ------------------------------------------------------------ resume

    async fn resume(&self, task: &TaskRef) -> Result<Resumed, BackendError> {
        let mut adopted = self.adopted.subscribe();
        let _ = tokio::time::timeout(ADOPTION_WAIT, adopted.wait_for(|done| *done)).await;
        // Followed already: a resume of a task this server never lost.
        if let Some(stage) = self.with_task(&task.task_id, |task| (task.stage.clone(), task.record.turn_started)) {
            return Ok(match stage {
                (Stage::Asking(request_id), _) => match self.drive.injections.host.questions().get(&request_id) {
                    Some(stored) => Resumed::InputRequired(question_of(&stored)),
                    None => Resumed::Working,
                },
                (_, true) => Resumed::Working,
                (_, false) => Resumed::Submitted,
            });
        }
        let Some(record) = self.store.task(&task.task_id) else {
            return Ok(Resumed::Gone("this device has no record of the task's message any more".into()));
        };
        let Some(source) = self.source_named(&record.source) else {
            self.store.forget_task(&task.task_id);
            return Ok(Resumed::Gone(format!("this Droppen runs no {} sessions", record.source)));
        };
        let receipts = self.receipts();
        let main = receipts.whereabouts(&record.main.qid);
        let session = record
            .session_id
            .clone()
            .or_else(|| task.session.session_id.clone())
            .or_else(|| main.as_ref().and_then(|main| main.session_id.clone()));
        let Some(session) = session else {
            self.store.forget_task(&task.task_id);
            return Ok(Resumed::Gone("the task's session never started".into()));
        };
        if task.session.session_id.is_none() {
            self.report.send(BackendEvent::SessionIdentified {
                context_id: task.context_id.clone(),
                session_id: session.clone(),
            });
        }
        let record = TaskRecord {
            session_id: Some(session.clone()),
            ..record
        };
        let providers = &self.drive.providers;
        let queued = |qid: &str| providers.holds_queued(source, &session, qid);
        let ended = |outcome: TurnOutcome| {
            self.store.forget_task(&task.task_id);
            Ok(Resumed::Ended(outcome))
        };
        match main.map(|main| main.status) {
            Some(ReceiptStatus::Sent) => {}
            Some(ReceiptStatus::Failed) => return ended(TurnOutcome::Failed("the task's message was not delivered".into())),
            // Still in the queue the boot took back: it waits its turn.
            Some(ReceiptStatus::Queued) if queued(&record.main.qid) => {
                self.follow(&task.task_id, source, waiting(record), Stage::Running, false);
                return Ok(Resumed::Submitted);
            }
            _ => return ended(TurnOutcome::Failed("the task's message was lost in a restart of Droppen".into())),
        }
        // Its turn waits on a question, or on one that outlived the process
        // that asked it, which is still answered as a message.
        if let Some(request_id) = record.asked.clone() {
            if let Some(stored) = self.drive.injections.host.questions().get(&request_id) {
                if matches!(stored.state, QuestionState::Pending | QuestionState::Expired) {
                    let question = question_of(&stored);
                    self.follow(
                        &task.task_id,
                        source,
                        TaskRecord {
                            turn_started: true,
                            ..record
                        },
                        Stage::Asking(request_id),
                        false,
                    );
                    return Ok(Resumed::InputRequired(question));
                }
            }
        }
        if let Some(question) = self.open_question(source, &session, None) {
            let request_id = question.request_id.clone();
            self.follow(
                &task.task_id,
                source,
                TaskRecord {
                    turn_started: true,
                    asked: Some(request_id.clone()),
                    ..record
                },
                Stage::Asking(request_id),
                false,
            );
            return Ok(Resumed::InputRequired(question));
        }
        if !self.at_rest(source, &session, &record) {
            self.follow(
                &task.task_id,
                source,
                TaskRecord {
                    turn_started: true,
                    ..record
                },
                Stage::Running,
                false,
            );
            return Ok(Resumed::Working);
        }
        // The turn ended while the server was down.
        if let Ending::Failed(why) = self.ended_while_unwatched(source, &session) {
            return ended(TurnOutcome::Failed(why));
        }
        let finished = Task {
            record: record.clone(),
            source,
            stage: Stage::Ending,
            calling: false,
            cancelled: false,
            delivered_ms: None,
            progress: None,
            transcript: None,
            answered: None,
            at_rest: 0,
        };
        ended(TurnOutcome::Completed(self.artifacts(&finished).await))
    }

    // ------------------------------------------------------------ entries

    async fn check_entry(&self, run: &RunSpec) -> Result<(), String> {
        if run.source == EntrySource::Tck {
            return Err("a TCK entry runs no session".into());
        }
        let Some(source) = self.source(run.source) else {
            return Err(format!("this Droppen runs no {} sessions", run.source.as_str()));
        };
        let providers = &self.drive.providers;
        providers
            .check_start(source, run.acp_agent.as_deref())
            .map_err(|error| message_of(&error))?;
        droppen_drive::routes::validate_flags(
            providers.model_rule(source),
            run.model.as_deref(),
            run.effort.as_deref(),
            run.permission.as_deref(),
        )?;
        if !std::path::Path::new(&run.cwd).is_absolute() {
            return Err(format!("the directory must be an absolute path: {}", run.cwd));
        }
        let resolved =
            droppen_files::resolve_safe(&self.drive.paths, Some(&run.cwd)).map_err(|_| format!("the directory cannot be used: {}", run.cwd))?;
        if !tokio::fs::metadata(&resolved).await.is_ok_and(|meta| meta.is_dir()) {
            return Err(format!("not a directory: {}", run.cwd));
        }
        if let Some(account_id) = &run.account_id {
            let account = self
                .drive
                .accounts
                .get(account_id)
                .await
                .ok_or_else(|| format!("no stored account {account_id}"))?;
            if droppen_accounts::store::AccountProvider::of(source) != Some(account.provider) {
                return Err(format!("the account {} is not a {} login", account.label, source.as_str()));
            }
        }
        if run.isolation == Isolation::Worktree && git::toplevel(&resolved).await.is_none() {
            return Err(format!(
                "{} is not in a git repository, so its tasks cannot each get a worktree; choose no isolation for it",
                run.cwd
            ));
        }
        Ok(())
    }
}

/// A resumed task's record as it waits in its session's queue again: not in
/// its turn, and asking nothing.
fn waiting(record: TaskRecord) -> TaskRecord {
    TaskRecord {
        turn_started: false,
        asked: None,
        ..record
    }
}

/// A question as the protocol reports it, from the stored one.
fn question_of(stored: &questions::Stored) -> Question {
    Question {
        request_id: stored.request_id.clone(),
        questions: serde_json::from_value::<Vec<QuestionItem>>(stored.questions.clone()).unwrap_or_default(),
    }
}

/// Refuse what a message's images cannot be: more of them than a message
/// carries, another kind than PNG or JPEG, or too large.
fn check_images(images: &[a2a::Image]) -> Result<(), String> {
    if images.len() > MAX_IMAGES {
        return Err(format!("a message carries at most {MAX_IMAGES} images"));
    }
    let mut total = 0usize;
    for image in images {
        if !matches!(image.media_type.as_str(), "image/png" | "image/jpeg") {
            return Err(format!("an image must be image/png or image/jpeg, not {}", image.media_type));
        }
        if image.bytes.len() > MAX_IMAGE_BYTES {
            return Err(format!("an image is larger than {} MiB", MAX_IMAGE_BYTES >> 20));
        }
        total += image.bytes.len();
    }
    if total > MAX_IMAGES_BYTES {
        return Err(format!("a message's images are larger than {} MiB together", MAX_IMAGES_BYTES >> 20));
    }
    Ok(())
}

/// One segment of a URL path: everything but the unreserved characters
/// percent-encoded.
fn encode_segment(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

impl SessionBackend for A2aSessions {
    fn check_entry<'a>(&'a self, run: &'a RunSpec) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(self.inner.check_entry(run))
    }

    fn start(&self, start: StartSession) -> BoxFuture<'_, Result<Started, BackendError>> {
        Box::pin(self.inner.start(start))
    }

    fn send(&self, send: SendToSession) -> BoxFuture<'_, Result<(), BackendError>> {
        Box::pin(self.inner.send(send))
    }

    fn answer<'a>(&'a self, task: &'a TaskRef, answer: a2a::Answer) -> BoxFuture<'a, Result<(), BackendError>> {
        Box::pin(self.inner.answer(task, answer))
    }

    fn cancel<'a>(&'a self, task: &'a TaskRef) -> BoxFuture<'a, Result<(), BackendError>> {
        Box::pin(self.inner.cancel(task))
    }

    fn resume<'a>(&'a self, task: &'a TaskRef) -> BoxFuture<'a, Result<Resumed, BackendError>> {
        Box::pin(self.inner.resume(task))
    }
}
