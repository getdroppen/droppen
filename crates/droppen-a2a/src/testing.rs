//! A [`SessionBackend`] of the tests' own: it runs nothing, writes down every
//! call, answers each the way a test set it up to, and reports whatever the
//! test says happened.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;

use crate::backend::{
    Answer, BackendError, BackendEvent, Events, Question, ResultData, Resumed, RunSpec, SendToSession, SessionBackend, StartSession, Started,
    TaskRef, TurnOutcome, TurnResult,
};

/// One call the backend was asked to make.
#[derive(Debug, Clone)]
pub enum Call {
    /// [`SessionBackend::check_entry`].
    CheckEntry(RunSpec),
    /// [`SessionBackend::start`].
    Start(StartSession),
    /// [`SessionBackend::send`].
    Send(SendToSession),
    /// [`SessionBackend::answer`].
    Answer(TaskRef, Answer),
    /// [`SessionBackend::cancel`].
    Cancel(TaskRef),
    /// [`SessionBackend::resume`].
    Resume(TaskRef),
}

impl Call {
    /// The task the call is about, for every call but an entry check.
    pub fn task_id(&self) -> Option<&str> {
        match self {
            Call::CheckEntry(_) => None,
            Call::Start(start) => Some(&start.task.task_id),
            Call::Send(send) => Some(&send.task.task_id),
            Call::Answer(task, _) | Call::Cancel(task) | Call::Resume(task) => Some(&task.task_id),
        }
    }
}

#[derive(Default)]
struct Script {
    calls: Vec<Call>,
    starts: VecDeque<Result<Started, BackendError>>,
    sends: VecDeque<Result<(), BackendError>>,
    answers: VecDeque<Result<(), BackendError>>,
    cancels: VecDeque<Result<(), BackendError>>,
    resumes: HashMap<String, Result<Resumed, BackendError>>,
    entry_check: Option<String>,
    sessions: usize,
}

/// The fake backend. Every call succeeds unless a test queued another
/// answer for it; a start names its session `session-<n>`.
pub struct FakeBackend {
    events: Events,
    script: Mutex<Script>,
    called: tokio::sync::Notify,
}

impl FakeBackend {
    /// A fake that reports through `events`.
    pub fn new(events: Events) -> Arc<Self> {
        Arc::new(Self {
            events,
            script: Mutex::new(Script::default()),
            called: tokio::sync::Notify::new(),
        })
    }

    fn script(&self) -> std::sync::MutexGuard<'_, Script> {
        self.script.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn record(&self, call: Call) {
        self.script().calls.push(call);
        self.called.notify_waiters();
    }

    /// Every call so far, oldest first.
    pub fn calls(&self) -> Vec<Call> {
        self.script().calls.clone()
    }

    /// Wait until a call `matches` has been made, and hand it back. Fails the
    /// test after five seconds.
    pub async fn wait_for(&self, matches: impl Fn(&Call) -> bool) -> Call {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let notified = self.called.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(found) = self.calls().into_iter().find(|call| matches(call)) {
                return found;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                panic!("the backend was never asked for that call; it was asked for {:?}", self.calls());
            }
        }
    }

    /// The next start answers `result`.
    pub fn next_start(&self, result: Result<Started, BackendError>) {
        self.script().starts.push_back(result);
    }

    /// The next send answers `result`.
    pub fn next_send(&self, result: Result<(), BackendError>) {
        self.script().sends.push_back(result);
    }

    /// The next answer answers `result`.
    pub fn next_answer(&self, result: Result<(), BackendError>) {
        self.script().answers.push_back(result);
    }

    /// The next cancel answers `result`.
    pub fn next_cancel(&self, result: Result<(), BackendError>) {
        self.script().cancels.push_back(result);
    }

    /// Resuming `task_id` answers `result`; a task with none set is
    /// [`Resumed::Working`].
    pub fn on_resume(&self, task_id: &str, result: Result<Resumed, BackendError>) {
        self.script().resumes.insert(task_id.to_string(), result);
    }

    /// Every entry check refuses with `message` (`None`: accepts again).
    pub fn refuse_entries(&self, message: Option<&str>) {
        self.script().entry_check = message.map(str::to_string);
    }

    /// Report `event`.
    pub fn report(&self, event: BackendEvent) {
        self.events.send(event);
    }

    /// The task's message was taken into its turn.
    pub fn turn_started(&self, task_id: &str) {
        self.report(BackendEvent::TurnStarted { task_id: task_id.into() });
    }

    /// The task's turn says what it is doing.
    pub fn progress(&self, task_id: &str, text: &str) {
        self.report(BackendEvent::Progress {
            task_id: task_id.into(),
            text: text.into(),
        });
    }

    /// The task's turn asks `question`.
    pub fn ask(&self, task_id: &str, question: Question) {
        self.report(BackendEvent::Question {
            task_id: task_id.into(),
            question,
        });
    }

    /// The task's turn ended well, its last reply `summary`.
    pub fn complete(&self, task_id: &str, summary: &str) {
        self.report(BackendEvent::TurnEnded {
            task_id: task_id.into(),
            outcome: TurnOutcome::Completed(Self::result(summary)),
        });
    }

    /// The task's turn failed with `message`.
    pub fn fail(&self, task_id: &str, message: &str) {
        self.report(BackendEvent::TurnEnded {
            task_id: task_id.into(),
            outcome: TurnOutcome::Failed(message.into()),
        });
    }

    /// A person stopped the task's turn in Droppen.
    pub fn stopped_in_droppen(&self, task_id: &str) {
        self.report(BackendEvent::TurnEnded {
            task_id: task_id.into(),
            outcome: TurnOutcome::Canceled("stopped in Droppen".into()),
        });
    }

    /// The finished turn a test's [`FakeBackend::complete`] reports.
    pub fn result(summary: &str) -> TurnResult {
        TurnResult {
            summary: summary.into(),
            result: ResultData {
                source: "claude".into(),
                session_id: "session-1".into(),
                host: "test-host".into(),
                cwd: "/work".into(),
                branch: None,
                changed_files: Vec::new(),
                pr_urls: Vec::new(),
                droppen_url: None,
            },
        }
    }
}

impl SessionBackend for FakeBackend {
    fn check_entry<'a>(&'a self, run: &'a RunSpec) -> BoxFuture<'a, Result<(), String>> {
        self.record(Call::CheckEntry(run.clone()));
        let refused = self.script().entry_check.clone();
        Box::pin(async move { refused.map_or(Ok(()), Err) })
    }

    fn start(&self, start: StartSession) -> BoxFuture<'_, Result<Started, BackendError>> {
        let answer = {
            let mut script = self.script();
            script.sessions += 1;
            let n = script.sessions;
            script.starts.pop_front().unwrap_or_else(|| {
                Ok(Started {
                    session_id: Some(format!("session-{n}")),
                    cwd: start.task.session.cwd.clone(),
                    branch: None,
                })
            })
        };
        self.record(Call::Start(start));
        Box::pin(async move { answer })
    }

    fn send(&self, send: SendToSession) -> BoxFuture<'_, Result<(), BackendError>> {
        let answer = self.script().sends.pop_front().unwrap_or(Ok(()));
        self.record(Call::Send(send));
        Box::pin(async move { answer })
    }

    fn answer<'a>(&'a self, task: &'a TaskRef, answer: Answer) -> BoxFuture<'a, Result<(), BackendError>> {
        let result = self.script().answers.pop_front().unwrap_or(Ok(()));
        self.record(Call::Answer(task.clone(), answer));
        Box::pin(async move { result })
    }

    fn cancel<'a>(&'a self, task: &'a TaskRef) -> BoxFuture<'a, Result<(), BackendError>> {
        let result = self.script().cancels.pop_front().unwrap_or(Ok(()));
        self.record(Call::Cancel(task.clone()));
        Box::pin(async move { result })
    }

    fn resume<'a>(&'a self, task: &'a TaskRef) -> BoxFuture<'a, Result<Resumed, BackendError>> {
        let result = self.script().resumes.remove(&task.task_id).unwrap_or(Ok(Resumed::Working));
        self.record(Call::Resume(task.clone()));
        Box::pin(async move { result })
    }
}

/// A [`crate::DeviceGate`] of the tests' own: it admits the A2A proxies a
/// test lets in, and says so each time the test changes that, as the device
/// chain moving would.
pub struct FakeDevices {
    admitted: Mutex<std::collections::BTreeSet<String>>,
    changes: tokio::sync::watch::Sender<u64>,
}

impl FakeDevices {
    /// A gate admitting `ids`.
    pub fn admitting(ids: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            admitted: Mutex::new(ids.iter().map(|id| id.to_string()).collect()),
            changes: tokio::sync::watch::channel(0).0,
        })
    }

    fn admitted_ids(&self) -> std::sync::MutexGuard<'_, std::collections::BTreeSet<String>> {
        self.admitted.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Admit `id` from now, and say the chain moved.
    pub fn admit(&self, id: &str) {
        self.admitted_ids().insert(id.to_string());
        self.changes.send_modify(|n| *n += 1);
    }

    /// Admit `id` no longer, and say the chain moved.
    pub fn forget(&self, id: &str) {
        self.admitted_ids().remove(id);
        self.changes.send_modify(|n| *n += 1);
    }

    /// Admit `id` no longer, and say nothing: a chain change the A2A server
    /// has not heard of yet, which every check it makes must see all the
    /// same.
    pub fn forget_quietly(&self, id: &str) {
        self.admitted_ids().remove(id);
    }
}

impl crate::DeviceGate for FakeDevices {
    fn admits(&self, device: &str) -> bool {
        self.admitted_ids().contains(device)
    }

    fn admitted(&self) -> Vec<crate::devices::A2aDevice> {
        self.admitted_ids()
            .iter()
            .map(|id| crate::devices::A2aDevice {
                id: id.clone(),
                fingerprint: format!("fp-{id}"),
                name: droppen_http::openapi_ext::Nullable(None),
            })
            .collect()
    }

    fn changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changes.subscribe()
    }
}
