//! The providers, and the registry that picks a session's.
//!
//! A provider is one CLI this server drives: Claude Code, Codex, or the ACP
//! agents. Each says what it knows about the sessions it runs through [`Run`],
//! and the [`ProviderRegistry`] answers for a session by asking the provider of
//! its [`Source`]. That is what lets the routes, the feeds and the schedules ask
//! about any session without naming the CLI behind it: only the entry, which
//! registers the providers, names them.

use std::collections::BTreeMap;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use tokio::sync::watch;

use super::{drafts, AnswerOutcome, InjectionRegistry, QueueOutcome, Reply, ResidentState, StopTaskOutcome};
use crate::sessions::{DriverStates, Injection, InjectionRunState, ResidentInfo, SessionSummary, Source};
use droppen_http::error::ApiError;

/// One CLI this server drives.
pub trait Provider: Send + Sync {
    /// The sessions it runs: the key the registry files it under.
    fn source(&self) -> Source;

    /// What its sessions can do that not every CLI's can.
    fn capabilities(&self) -> Capabilities;

    /// What it says about those sessions.
    fn run(&self) -> &dyn Run;
}

/// What a provider's sessions can do that not every CLI's can.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// Its sessions stop to ask the user questions, which this server keeps
    /// and answers into the turn that asked.
    pub questions: bool,
}

/// What a provider says about the sessions it runs, each named by its id.
pub trait Run: Send + Sync {
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
}

/// Every provider, keyed by the [`Source`] it runs.
///
/// Kept in [`Source`]'s order, so a pass over every provider asks them in the
/// same order whichever was registered first.
pub struct ProviderRegistry {
    providers: BTreeMap<Source, Arc<dyn Provider>>,
    /// The runs map every provider reads its sessions' state from.
    injections: Arc<InjectionRegistry>,
}

impl ProviderRegistry {
    /// The registry over `injections`, with the providers `register` adds.
    ///
    /// `register` is the entry's `crate::app::register_providers`, the one
    /// place that names every provider; a test passes the same function.
    pub fn new(injections: Arc<InjectionRegistry>, register: fn(&mut ProviderRegistry, &Arc<InjectionRegistry>)) -> Arc<Self> {
        let mut registry = Self {
            providers: BTreeMap::new(),
            injections: injections.clone(),
        };
        register(&mut registry, &injections);
        Arc::new(registry)
    }

    /// Add `provider` under its own source, in place of any provider there.
    pub fn register(&mut self, provider: Arc<dyn Provider>) {
        self.providers.insert(provider.source(), provider);
    }

    /// Every source a provider is registered for, in [`Source`]'s order.
    pub fn sources(&self) -> impl Iterator<Item = Source> + '_ {
        self.providers.keys().copied()
    }

    /// The runs of `source`'s provider, when one is registered.
    fn run(&self, source: Source) -> Option<&dyn Run> {
        self.providers.get(&source).map(|provider| provider.run())
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

    /// Whether this server's driver for a session still holds `qid` in its
    /// queue: the evidence a claim stranded by a crash was enqueued after all.
    pub fn holds_queued(&self, source: Source, id: &str, qid: &str) -> bool {
        self.resident_info(source, id)
            .is_some_and(|info| info.queued_messages.iter().any(|message| message.id == qid))
    }

    /// Everything the session list needs from the runs, in one pass; see
    /// [`InjectionRegistry::driver_states`].
    pub fn driver_states(&self) -> DriverStates {
        self.injections.driver_states()
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
    /// See `resident::Driver::steer` for what that means to claude and why it
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
    /// [`InjectionRegistry::interrupt_legacy`] when they hold no process for the
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp_agent::{AcpAgentRegistry, AcpAgentUpdate};
    use droppen_instance::instance::InstanceDirs;
    use droppen_instance::paths::Paths;

    /// Every provider over a fresh instance in `home`, as the entry registers
    /// them, with one ACP agent configured: `fake`.
    fn providers(home: &std::path::Path) -> Arc<ProviderRegistry> {
        providers_from(home, crate::app::register_providers)
    }

    /// The providers `register` adds, over a fresh instance in `home` with one
    /// ACP agent configured: `fake`.
    fn providers_from(home: &std::path::Path, register: fn(&mut ProviderRegistry, &Arc<InjectionRegistry>)) -> Arc<ProviderRegistry> {
        let paths = Arc::new(Paths::new(home.to_path_buf()));
        let dirs = InstanceDirs::new(&paths.data_dir(), "registry-test").expect("valid name");
        dirs.create().expect("instance dirs");
        let agent = AcpAgentUpdate {
            name: None.into(),
            command: Some("/x/agent".to_string()).into(),
            args: None.into(),
            env: None.into(),
        };
        AcpAgentRegistry::of(&paths).update("fake", &agent).expect("saved");
        let accounts = Arc::new(crate::accounts::store::AccountsStore::new(paths.clone(), crate::app::logins()));
        let quota = Arc::new(crate::accounts::quota::QuotaCache::new(paths.clone()));
        ProviderRegistry::new(InjectionRegistry::new(paths, &dirs, accounts, quota), register)
    }

    /// Claude Code alone.
    fn claude_only(providers: &mut ProviderRegistry, registry: &Arc<InjectionRegistry>) {
        providers.register(Arc::new(crate::inject::resident::ClaudeCode::new(registry.clone())));
    }

    /// A session row of `source` that ran on the ACP agent `agent`, if any.
    fn row(source: &str, agent: Option<&str>) -> SessionSummary {
        serde_json::from_value(serde_json::json!({
            "pinned": false, "id": "s", "source": source, "scope": "owned", "server": "s", "title": "t",
            "cwd": null, "repoKey": null, "gitBranch": null, "updatedAt": "", "createdAt": "",
            "sizeBytes": 1, "filePath": "/x", "status": "idle", "activity": "idle", "resident": null,
            "claudeRoot": null, "accountId": null, "attentionAt": null, "unread": false, "unreadCount": 0,
            "acpAgent": agent,
        }))
        .unwrap_or_else(|error| panic!("a session row: {error}"))
    }

    /// A source no provider runs is refused at the start and blocked at a send,
    /// whatever the request names: nothing could start or drive its session.
    #[test]
    fn a_source_no_provider_runs_is_refused() {
        let home = tempfile::tempdir().expect("tempdir");
        let none = providers_from(home.path(), |_, _| {});
        let other = tempfile::tempdir().expect("tempdir");
        let claude = providers_from(other.path(), claude_only);
        let refusal = |providers: &ProviderRegistry, source, agent| {
            providers.check_start(source, agent).err().map(|error| {
                assert_eq!(error.status(), gotcha::axum::http::StatusCode::BAD_REQUEST);
                error.body()["error"].as_str().unwrap_or_default().to_string()
            })
        };
        for source in [Source::Claude, Source::Codex, Source::Acp] {
            for agent in [None, Some("fake")] {
                assert_eq!(refusal(&none, source, agent), Some(format!("no provider runs {source} sessions")), "{source} {agent:?}");
            }
        }
        assert_eq!(refusal(&claude, Source::Claude, None), None);
        assert_eq!(refusal(&claude, Source::Acp, Some("fake")).as_deref(), Some("no provider runs acp sessions"));
        assert_eq!(refusal(&claude, Source::Codex, None).as_deref(), Some("no provider runs codex sessions"));

        assert_eq!(none.send_block(&row("claude", None)).as_deref(), Some("no provider runs claude sessions"));
        assert_eq!(none.send_block(&row("acp", Some("fake"))).as_deref(), Some("no provider runs acp sessions"));
        assert_eq!(claude.send_block(&row("claude", None)), None);
        assert_eq!(claude.send_block(&row("codex", None)).as_deref(), Some("no provider runs codex sessions"));
        assert_eq!(claude.send_block(&row("acp", Some("fake"))).as_deref(), Some("no provider runs acp sessions"));
    }

    /// A new session names an ACP agent exactly when it is an ACP session, and
    /// then a configured one, by an id that can be a directory name: the id is
    /// checked before it is looked up, and every refusal is a `400` in these
    /// words.
    #[test]
    fn a_new_session_names_an_acp_agent_exactly_when_it_is_one() {
        let home = tempfile::tempdir().expect("tempdir");
        let providers = providers(home.path());
        let refusal = |source, agent| {
            providers.check_start(source, agent).err().map(|error| {
                assert_eq!(error.status(), gotcha::axum::http::StatusCode::BAD_REQUEST);
                error.body()["error"].as_str().unwrap_or_default().to_string()
            })
        };
        assert_eq!(refusal(Source::Acp, None).as_deref(), Some("an ACP session needs acpAgent: the agent it runs on"));
        assert_eq!(refusal(Source::Acp, Some("../fake")).as_deref(), Some("invalid ACP agent id: ../fake"));
        assert_eq!(refusal(Source::Acp, Some("Fake")).as_deref(), Some("invalid ACP agent id: Fake"));
        assert_eq!(refusal(Source::Acp, Some("other")).as_deref(), Some("no ACP agent \"other\" is configured"));
        assert_eq!(refusal(Source::Acp, Some("fake")), None);
        for source in [Source::Claude, Source::Codex] {
            assert_eq!(refusal(source, None), None, "{source}");
            for agent in ["fake", "../fake"] {
                assert_eq!(refusal(source, Some(agent)).as_deref(), Some("acpAgent is only for ACP sessions"), "{source} {agent}");
            }
        }
    }

    /// Only an ACP session is ever blocked by its agent: one whose agent is
    /// gone, or that names none.
    #[test]
    fn a_session_is_blocked_only_when_its_acp_agent_is_gone() {
        let home = tempfile::tempdir().expect("tempdir");
        let providers = providers(home.path());
        assert_eq!(providers.send_block(&row("acp", Some("fake"))), None);
        assert_eq!(providers.send_block(&row("acp", Some("gone"))), Some(crate::acp_agent::removed("gone")));
        assert_eq!(
            providers.send_block(&row("acp", None)).as_deref(),
            Some("this ACP session does not say which agent it ran on")
        );
        for source in ["claude", "codex"] {
            assert_eq!(providers.send_block(&row(source, Some("gone"))), None, "{source}");
            assert_eq!(providers.send_block(&row(source, None)), None, "{source}");
        }
    }
}
