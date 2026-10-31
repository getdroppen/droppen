//! The providers, and the registry that picks a session's.
//!
//! A provider is one CLI this server drives: Claude Code, Codex, or the ACP
//! agents. Each says what it knows about the sessions it runs through [`Run`],
//! and the [`ProviderRegistry`] answers for a session by asking the provider of
//! its [`Source`]. That is what lets the routes, the feeds and the schedules ask
//! about any session without naming the CLI behind it: only the entry, which
//! registers the providers, names them.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use tokio::sync::watch;

use super::{drafts, AnswerOutcome, InjectError, InjectionRegistry, ProviderRuns, QueueOutcome, Reply, ResidentState, SendOptions, Sent, SpawnGate, StopTaskOutcome};
use crate::models::Models;
use droppen_sessions::{DriverStates, Injection, InjectionRunState, ResidentInfo, SessionSummary, Source};
use droppen_http::error::ApiError;

/// One CLI this server drives.
pub trait Provider: Send + Sync {
    /// The sessions it runs: the key the registry files it under.
    fn source(&self) -> Source;

    /// What its sessions can do that not every CLI's can.
    fn capabilities(&self) -> Capabilities;

    /// What it says about those sessions.
    fn run(&self) -> &dyn Run;

    /// The runs it drives those sessions on, on its registry's host: what the
    /// registry lists and adopts into for its source.
    fn runs(&self) -> Arc<dyn ProviderRuns>;

    /// The models its CLI offers, and which efforts and permission modes it
    /// takes.
    fn models(&self) -> &dyn Models;

    /// Itself, for a route of its own that needs it by its type
    /// ([`ProviderRegistry::provider`]).
    fn as_any(&self) -> &dyn std::any::Any;

    /// Carry session `id` onto an account as the new session `new_id`: its
    /// conversation copied into this instance's own root, the original left
    /// as it was. Answers where the copy's transcript is.
    ///
    /// Only a CLI whose sessions can sit in a root this instance does not own
    /// has one to carry; every other provider refuses
    /// ([`ContinueError::Unsupported`]), its sessions switching accounts in
    /// place.
    fn continue_on_account<'a>(&'a self, id: &'a str, new_id: &'a str) -> BoxFuture<'a, Result<PathBuf, ContinueError>>;
}

/// Why a session could not be continued on an account
/// ([`Provider::continue_on_account`]).
#[derive(Debug, thiserror::Error)]
pub enum ContinueError {
    /// Its CLI's sessions switch accounts in place; there is nothing to carry.
    #[error("this CLI's sessions are not continued on an account")]
    Unsupported,
    /// No such session where a continued one comes from.
    #[error("session not found")]
    NotFound,
    /// The new id is already taken; nothing was written.
    #[error("the new session id is already in use")]
    Taken,
    /// The copy failed part-way; nothing usable was left behind.
    #[error("cannot copy the transcript: {0}")]
    Io(String),
}

/// What a provider's sessions can do that not every CLI's can.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// Its sessions stop to ask the user questions, which this server keeps
    /// and answers into the turn that asked.
    pub questions: bool,
    /// It runs instant chats: its sessions in the chats directory are listed
    /// as chats.
    pub chats: bool,
    /// Its instant chats can read what is attached to a message. Only asked of
    /// a provider with `chats`.
    pub chat_attachments: bool,
    /// Scheduled tasks can run on it.
    pub schedules: bool,
}

/// A message for one of a provider's sessions.
#[derive(Debug, Clone, Copy)]
pub struct SendTo<'a> {
    /// The session.
    pub id: &'a str,
    /// The directory the session ran in, as the session list knows it.
    pub cwd: Option<&'a str>,
    /// What the user wrote.
    pub text: &'a str,
    /// Everything else the composer asked for.
    pub options: &'a SendOptions,
}

/// The first message of a session a provider is to start.
#[derive(Debug, Clone, Copy)]
pub struct StartAt<'a> {
    /// The directory the session runs in.
    pub cwd: &'a str,
    /// What the user wrote.
    pub text: &'a str,
    /// Everything else the composer asked for.
    pub options: &'a SendOptions,
}

/// What a provider says about the sessions it runs, each named by its id.
pub trait Run: Send + Sync {
    /// Hand a message to the CLI that owns an existing session, starting one
    /// under `gate` if it has to.
    fn send<'a>(&'a self, gate: &'a SpawnGate, to: SendTo<'a>) -> BoxFuture<'a, Result<Sent, InjectError>>;

    /// Start a brand-new session under `gate` and resolve once its canonical
    /// id is known.
    fn start<'a>(&'a self, gate: &'a SpawnGate, at: StartAt<'a>) -> BoxFuture<'a, Result<String, InjectError>>;

    /// The state of the run for this session, if this server ever started one.
    fn injection(&self, id: &str) -> Option<Injection>;

    /// The process this server holds for the session, when the session list is
    /// to show it.
    fn resident(&self, id: &str) -> Option<ResidentState>;

    /// Why a scheduled task's previous session is not ready for its next run,
    /// or `None` when it is.
    ///
    /// The schedule asks before it starts a session, so that a task firing
    /// every minute does not pile run upon run behind one that is still going.
    fn schedule_busy(&self, id: &str) -> Option<&'static str>;

    /// Follow what the session's CLI has said and its transcript does not show
    /// yet, for a CLI that writes its transcript only once a message is done.
    fn draft_watch(&self, _id: &str) -> Option<watch::Receiver<drafts::Draft>> {
        None
    }

    /// The session's transcript has grown, and its feed has just sent it.
    fn transcript_grew(&self, _id: &str) {}

    /// Something that could free an account has happened: every session
    /// holding its queue after a limit looks again.
    fn recheck_holds(&self) {}

    /// Refuse a new session's `acpAgent` before anything is started, when
    /// this provider's sessions cannot run on that agent: an unknown agent is
    /// a bad request rather than a session that cannot start. A provider that
    /// runs no ACP agent takes a session that names none.
    fn check_start(&self, acp_agent: Option<&str>) -> Result<(), ApiError> {
        match acp_agent {
            None => Ok(()),
            Some(_) => Err(ApiError::bad_request("acpAgent is only for ACP sessions")),
        }
    }

    /// Why no message can go to this session whatever is running for it, or
    /// `None` when nothing in the provider stops one.
    fn send_block(&self, _summary: &SessionSummary) -> Option<String> {
        None
    }

    /// Take message `qid` out of the session's queue before it is written to
    /// the CLI.
    fn drop_queued<'a>(&'a self, id: &'a str, qid: &'a str) -> BoxFuture<'a, QueueOutcome>;

    /// Hand queued message `qid` to the CLI now, ahead of the turn in flight.
    fn steer_queued<'a>(&'a self, id: &'a str, qid: &'a str) -> BoxFuture<'a, QueueOutcome>;

    /// Stop the turn this server started. `false` when nothing is running.
    fn interrupt(&self, id: &str) -> bool;

    /// Ask the CLI to stop one background task, leaving the turn alone.
    fn stop_background_task(&self, _id: &str, _task_id: &str) -> StopTaskOutcome {
        StopTaskOutcome::NoResident
    }

    /// The question the session's CLI is blocked on, by request id.
    fn open_question(&self, _id: &str) -> Option<String> {
        None
    }

    /// Answer a question the session's CLI is blocked on, or dismiss it.
    fn answer_question<'a>(&'a self, _id: &'a str, _request_id: &'a str, _reply: Reply, _by: Option<String>) -> BoxFuture<'a, AnswerOutcome> {
        Box::pin(std::future::ready(AnswerOutcome::NoResident))
    }

    /// Put the session away, so it leaves the list; or why not, in the words
    /// the archive route refuses with. Each CLI keeps its archive its own way,
    /// so every provider answers. Under `gate` where the CLI must not start
    /// meanwhile.
    fn archive<'a>(&'a self, gate: &'a SpawnGate, id: &'a str) -> BoxFuture<'a, Result<(), String>>;
}

/// Every provider, keyed by the [`Source`] it runs.
///
/// Kept in [`Source`]'s order, so a pass over every provider asks them in the
/// same order whichever was registered first.
pub struct ProviderRegistry {
    providers: BTreeMap<Source, Arc<dyn Provider>>,
    /// The runs map every provider reads its sessions' state from.
    injections: Arc<InjectionRegistry>,
    /// The one spawn gate, lent to a provider for a call that may start its
    /// CLI and kept by none: no driver can take it.
    gate: Arc<SpawnGate>,
}

impl ProviderRegistry {
    /// The registry over `injections`, with the providers `register` adds.
    ///
    /// `register` is the entry's `app::register_providers`, the one
    /// place that names every provider; a test passes the same function.
    pub fn new(injections: Arc<InjectionRegistry>, register: impl FnOnce(&mut ProviderRegistry, &Arc<InjectionRegistry>)) -> Arc<Self> {
        let gate = Arc::new(SpawnGate::new(injections.host.paths.data_dir().join("inject-locks")));
        injections.host.hold_gate(&gate);
        let mut registry = Self {
            providers: BTreeMap::new(),
            injections: injections.clone(),
            gate,
        };
        register(&mut registry, &injections);
        Arc::new(registry)
    }

    /// Add `provider` under its own source: one per source, its runs on this
    /// registry's host.
    pub fn register(&mut self, provider: Arc<dyn Provider>) {
        let source = provider.source();
        let runs = provider.runs();
        assert_eq!(runs.source(), source, "the {source} provider's runs run another source's sessions");
        assert!(Arc::ptr_eq(runs.host(), &self.injections.host), "the {source} provider's runs are on another host than this registry's");
        assert!(!self.providers.contains_key(&source), "a {source} provider is registered already: one per source");
        self.providers.insert(source, provider);
    }

    /// The provider of type `P` and the spawn gate, for a route of that
    /// provider's own that starts or stops its CLI outside a [`Run`] call;
    /// `None` when none is registered.
    ///
    /// The type does not hold the lend: any code that can name `P` can call
    /// this. A source scan does (`only_the_providers_reach_the_spawn_gate`):
    /// outside this file, only the ACP agent routes' `AcpAgentState::gate`
    /// names it.
    pub fn provider_with_gate<P: Provider + 'static>(&self) -> Option<(&P, &SpawnGate)> {
        Some((self.provider::<P>()?, &self.gate))
    }

    /// The spawn gate, for a test that drives a provider by hand.
    #[cfg(any(test, feature = "test-support"))]
    pub fn gate(&self) -> &SpawnGate {
        &self.gate
    }

    /// The injection registry it is over, for a test.
    #[cfg(any(test, feature = "test-support"))]
    pub fn injections(&self) -> &Arc<InjectionRegistry> {
        &self.injections
    }

    /// Re-attach to every run the previous server left behind; see
    /// [`InjectionRegistry::adopt_all`].
    pub async fn adopt_all(&self) -> super::Adopted {
        self.injections.adopt_all(&self.gate, &self.all_runs()).await
    }

    /// Each source's provider's runs, for a test that adopts a run by hand
    /// ([`InjectionRegistry::watch_adopted`]).
    #[cfg(any(test, feature = "test-support"))]
    pub fn runs(&self) -> BTreeMap<Source, Arc<dyn ProviderRuns>> {
        self.all_runs()
    }

    /// Each source's provider's runs.
    fn all_runs(&self) -> BTreeMap<Source, Arc<dyn ProviderRuns>> {
        self.providers.iter().map(|(source, provider)| (*source, provider.runs())).collect()
    }

    /// Every source a provider is registered for, in [`Source`]'s order.
    pub fn sources(&self) -> impl Iterator<Item = Source> + '_ {
        self.providers.keys().copied()
    }

    /// The provider of type `P`, for a route of its own; `None` when none is
    /// registered.
    pub fn provider<P: Provider + 'static>(&self) -> Option<&P> {
        self.providers.values().find_map(|provider| provider.as_any().downcast_ref())
    }

    /// The runs of `source`'s provider, when one is registered.
    fn run(&self, source: Source) -> Option<&dyn Run> {
        self.providers.get(&source).map(|provider| provider.run())
    }

    /// Hand a message to the CLI that owns an existing session.
    pub async fn send_to_session(&self, source: Source, id: &str, cwd: Option<&str>, text: &str, options: &SendOptions) -> Result<Sent, InjectError> {
        match self.run(source) {
            Some(run) => run.send(&self.gate, SendTo { id, cwd, text, options }).await,
            None => Err(InjectError::Internal(format!("no provider runs {source} sessions"))),
        }
    }

    /// Start a brand-new session in `cwd` and resolve once its canonical id is known.
    pub async fn start_new_session(&self, source: Source, cwd: &str, text: &str, options: &SendOptions) -> Result<String, InjectError> {
        match self.run(source) {
            Some(run) => run.start(&self.gate, StartAt { cwd, text, options }).await,
            None => Err(InjectError::Internal(format!("no provider runs {source} sessions"))),
        }
    }

    /// Continue session `id` on an account as the new session `new_id`, by
    /// its own provider ([`Provider::continue_on_account`]).
    pub async fn continue_on_account(&self, source: Source, id: &str, new_id: &str) -> Result<PathBuf, ContinueError> {
        match self.providers.get(&source) {
            Some(provider) => provider.continue_on_account(id, new_id).await,
            None => Err(ContinueError::Unsupported),
        }
    }

    /// The state of the run for this session, if this server ever started one.
    pub fn injection_for(&self, source: Source, id: &str) -> Option<Injection> {
        self.run(source)?.injection(id)
    }

    /// Whether a run is in flight for this session.
    pub fn is_running(&self, source: Source, id: &str) -> bool {
        self.injection_for(source, id)
            .map(|i| i.state == InjectionRunState::Running)
            .unwrap_or(false)
    }

    /// Whether this server holds a process for the session that the session
    /// list shows.
    ///
    /// The composer gate asks this. A session whose CLI is registered as live
    /// is normally refused, because writing into a conversation somebody's own
    /// terminal has open interleaves two agents in one transcript — and a
    /// resident registers itself exactly the same way, because it *is* a real
    /// CLI. Without this the first turn would be the last one this server
    /// could send.
    pub fn has_resident(&self, source: Source, id: &str) -> bool {
        self.run(source).and_then(|run| run.resident(id)).is_some()
    }

    /// This server's resident for one session, as the API reports it.
    pub fn resident_info(&self, source: Source, id: &str) -> Option<ResidentInfo> {
        self.run(source)?.resident(id).map(Into::into)
    }

    /// The model this server's resident for one session runs, as its CLI
    /// named it ([`ResidentState::model`]).
    pub fn running_model(&self, source: Source, id: &str) -> Option<String> {
        self.run(source)?.resident(id)?.model
    }

    /// Whether this server's driver for a session still holds `qid` in its
    /// queue: the evidence a claim stranded by a crash was enqueued after all.
    pub fn holds_queued(&self, source: Source, id: &str, qid: &str) -> bool {
        self.resident_info(source, id)
            .is_some_and(|info| info.queued_messages.iter().any(|message| message.id == qid))
    }

    /// Everything the session list needs from every provider's runs, in
    /// [`Source`]'s order, and from the runs map, in one pass.
    pub fn driver_states(&self) -> DriverStates {
        let runs = self.all_runs();
        self.injections.host.snapshot(runs.values().map(|runs| &**runs))
    }

    /// See [`Run::draft_watch`]; `None` for a source with no provider.
    pub fn draft_watch(&self, source: Source, id: &str) -> Option<watch::Receiver<drafts::Draft>> {
        self.run(source)?.draft_watch(id)
    }

    /// See [`Run::transcript_grew`].
    pub fn transcript_grew(&self, source: Source, id: &str) {
        if let Some(run) = self.run(source) {
            run.transcript_grew(id);
        }
    }

    /// See [`Run::recheck_holds`]; every provider is asked.
    pub fn recheck_holds(&self) {
        for provider in self.providers.values() {
            provider.run().recheck_holds();
        }
    }

    /// See [`Run::schedule_busy`]; `None` for a source with no provider.
    pub fn schedule_busy(&self, source: Source, id: &str) -> Option<&'static str> {
        self.run(source)?.schedule_busy(id)
    }

    /// See [`Run::check_start`]. A source no provider runs is refused: nothing
    /// could start its session.
    pub fn check_start(&self, source: Source, acp_agent: Option<&str>) -> Result<(), ApiError> {
        match self.run(source) {
            Some(run) => run.check_start(acp_agent),
            None => Err(ApiError::bad_request(format!("no provider runs {source} sessions"))),
        }
    }

    /// See [`Run::send_block`], asked of the provider of the session's source.
    /// A session no provider runs is blocked: nothing could take the message.
    pub fn send_block(&self, summary: &SessionSummary) -> Option<String> {
        match self.run(summary.source) {
            Some(run) => run.send_block(summary),
            None => Some(format!("no provider runs {} sessions", summary.source)),
        }
    }

    /// See [`Provider::capabilities`]; none for a source with no provider.
    pub fn capabilities(&self, source: Source) -> Capabilities {
        self.providers.get(&source).map(|provider| provider.capabilities()).unwrap_or_default()
    }

    /// See [`Provider::models`]; `None` for a source with no provider.
    pub fn models(&self, source: Source) -> Option<&dyn Models> {
        self.providers.get(&source).map(|provider| provider.models())
    }

    /// See [`Models::model_rule`]. A source no provider runs admits no model
    /// id ([`crate::models::ModelRule::NONE`]).
    pub fn model_rule(&self, source: Source) -> crate::models::ModelRule {
        self.models(source).map_or(crate::models::ModelRule::NONE, |models| models.model_rule())
    }

    /// Whether an instant chat of `source` turns away a message with
    /// attachments: one whose provider runs chats that cannot read them, and
    /// one no provider runs, since nothing can say it reads them.
    pub fn chat_refuses_attachments(&self, source: Source) -> bool {
        match self.providers.get(&source) {
            Some(provider) => {
                let capabilities = provider.capabilities();
                capabilities.chats && !capabilities.chat_attachments
            }
            None => true,
        }
    }

    /// Take a message out of a session's queue before it is written to the CLI.
    ///
    /// Both resident protocols hold durable queues. Legacy exec runs do not.
    pub async fn drop_queued_message(&self, source: Source, id: &str, qid: &str) -> QueueOutcome {
        match self.run(source) {
            Some(run) => run.drop_queued(id, qid).await,
            None => QueueOutcome::NoResident,
        }
    }

    /// Hand a queued message to the CLI now, ahead of the turn in flight.
    ///
    /// See `claude_code::resident::Driver::steer` for what that means to claude and why it
    /// is a delivery rather than an interruption.
    pub async fn steer_queued_message(&self, source: Source, id: &str, qid: &str) -> QueueOutcome {
        match self.run(source) {
            Some(run) => run.steer_queued(id, qid).await,
            None => QueueOutcome::NoResident,
        }
    }

    /// Stop the turn remote-hand started. `false` when nothing is running.
    ///
    /// A resident is asked rather than killed: the first rung of its ladder is
    /// a `control_request` that ends the *turn* and leaves the process — and
    /// anything it started in the background — alive. Only a CLI that will not
    /// answer gets signalled, and only by the driver that can tell whether the
    /// turn ended in between. Claude and Codex fall back to
    /// [`super::RunHost::interrupt_legacy`] when they hold no process for the
    /// session; ACP never runs outside its agent.
    pub fn interrupt(&self, source: Source, id: &str) -> bool {
        self.run(source).is_some_and(|run| run.interrupt(id))
    }

    /// Ask claude to stop one background task, leaving the turn alone.
    ///
    /// The narrow counterpart to [`ProviderRegistry::interrupt`], and
    /// deliberately not reachable the same way: Stop ends the *turn*, which is a
    /// thing this server started and can therefore take back, while a
    /// background task is a child claude is minding on its own clock. The CLI's
    /// `stop_task` control request is the only thing that can end one of those
    /// without ending the process, so a session with no resident — every codex
    /// run, and a claude session this server is not driving — has no way to do
    /// it at all rather than a worse way.
    pub fn stop_background_task(&self, source: Source, id: &str, task_id: &str) -> StopTaskOutcome {
        self.run(source).map_or(StopTaskOutcome::NoResident, |run| run.stop_background_task(id, task_id))
    }

    /// See [`Run::open_question`]; `None` for a source with no provider.
    pub fn open_question(&self, source: Source, id: &str) -> Option<String> {
        self.run(source)?.open_question(id)
    }

    /// See [`Run::answer_question`].
    pub async fn answer_question(&self, source: Source, id: &str, request_id: &str, reply: Reply, by: Option<String>) -> AnswerOutcome {
        match self.run(source) {
            Some(run) => run.answer_question(id, request_id, reply, by).await,
            None => AnswerOutcome::NoResident,
        }
    }

    /// See [`Run::archive`]. A source no provider runs is refused: nothing
    /// could put its session away.
    pub async fn archive(&self, source: Source, id: &str) -> Result<(), String> {
        match self.run(source) {
            Some(run) => run.archive(&self.gate, id).await,
            None => Err(format!("no provider runs {source} sessions")),
        }
    }
}

impl droppen_accounts::ModelIds for ProviderRegistry {
    // A source no provider runs offers no model of its own.
    fn model_ids<'a>(&'a self, source: Source, account_id: Option<&'a str>) -> BoxFuture<'a, Vec<String>> {
        match self.models(source) {
            Some(models) => models.ids(account_id),
            None => Box::pin(std::future::ready(Vec::new())),
        }
    }
}
