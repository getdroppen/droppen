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

use super::{drafts, AnswerOutcome, InjectError, InjectionRegistry, QueueOutcome, Reply, ResidentState, SendOptions, Sent, SpawnGate, StopTaskOutcome};
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

    /// The models its CLI offers, and which efforts and permission modes it
    /// takes.
    fn models(&self) -> &dyn Models;

    /// Itself, for a route of its own that needs it by its type
    /// ([`ProviderRegistry::provider`]).
    fn as_any(&self) -> &dyn std::any::Any;
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
    /// `register` is the entry's `crate::app::register_providers`, the one
    /// place that names every provider; a test passes the same function.
    pub fn new(injections: Arc<InjectionRegistry>, register: impl FnOnce(&mut ProviderRegistry, &Arc<InjectionRegistry>)) -> Arc<Self> {
        let gate = Arc::new(SpawnGate::new(injections.host.paths.data_dir().join("inject-locks")));
        #[cfg(test)]
        injections.host.watch_gate(&gate);
        let mut registry = Self {
            providers: BTreeMap::new(),
            injections: injections.clone(),
            gate,
        };
        register(&mut registry, &injections);
        Arc::new(registry)
    }

    /// Add `provider` under its own source, which its runs are listed under
    /// on the host. A host lists one provider's runs per source
    /// ([`RunHost::list`](super::RunHost::list)), so a second provider for a
    /// source is refused when its runs are made, before it gets here.
    pub fn register(&mut self, provider: Arc<dyn Provider>) {
        let source = provider.source();
        assert!(self.injections.host.runs_of(source).is_some(), "the {source} provider's runs are not listed under {source}");
        self.providers.insert(source, provider);
    }

    /// The spawn gate, for a provider's own route that starts or stops its
    /// CLI outside a [`Run`] call.
    pub(crate) fn gate(&self) -> &SpawnGate {
        &self.gate
    }

    /// Re-attach to every run the previous server left behind; see
    /// [`InjectionRegistry::adopt_all`].
    pub async fn adopt_all(&self) -> super::Adopted {
        self.injections.adopt_all(&self.gate).await
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
    /// [`super::RunHost::driver_states`].
    pub fn driver_states(&self) -> DriverStates {
        self.injections.host.driver_states()
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
        let accounts = Arc::new(droppen_accounts::store::AccountsStore::new(paths.clone(), crate::app::logins()));
        let quota = Arc::new(droppen_accounts::quota::QuotaCache::new(paths.clone(), crate::app::logins()));
        let decoration = crate::drive::NotifyDecoration::new(paths.clone(), dirs.clone());
        ProviderRegistry::new(InjectionRegistry::new(paths, &dirs, accounts, quota, decoration), register)
    }

    /// Claude Code alone.
    fn claude_only(providers: &mut ProviderRegistry, registry: &Arc<InjectionRegistry>) {
        providers.register(Arc::new(crate::provider::claude_code::resident::ClaudeCode::new(registry.clone())));
    }

    /// Codex alone.
    fn codex_only(providers: &mut ProviderRegistry, registry: &Arc<InjectionRegistry>) {
        providers.register(Arc::new(crate::provider::app_server::Codex::new(registry.clone())));
    }

    /// The ACP agents alone.
    fn acp_only(providers: &mut ProviderRegistry, registry: &Arc<InjectionRegistry>) {
        providers.register(Arc::new(crate::provider::acp::Acp::new(registry.clone())));
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

    /// Each provider runs its sessions on the runs its host lists and adopts
    /// into, not on runs of its own that nothing lists or adopts.
    #[test]
    fn each_provider_runs_on_the_runs_its_host_adopts_into() {
        let home = tempfile::tempdir().expect("tempdir");
        let providers = providers(home.path());
        let listed = |source| providers.injections.host.runs_of(source).expect("listed");
        let codex = providers.provider::<crate::provider::app_server::Codex>().expect("Codex");
        assert!(std::ptr::addr_eq(Arc::as_ptr(codex.runs()), Arc::as_ptr(&listed(Source::Codex))), "Codex's");
        let claude = providers.provider::<crate::provider::claude_code::resident::ClaudeCode>().expect("Claude Code");
        assert!(std::ptr::addr_eq(Arc::as_ptr(claude.runs()), Arc::as_ptr(&listed(Source::Claude))), "Claude Code's");
    }

    /// One provider's runs per source on a host: a second provider registry
    /// over the same injection registry is refused while the first's runs
    /// last, whichever provider it starts with, and builds once they have gone.
    #[test]
    fn a_second_live_provider_registry_on_one_host_is_refused() {
        let home = tempfile::tempdir().expect("tempdir");
        let first = providers(home.path());
        let injections = first.injections.clone();
        let register: [(Source, fn(&mut ProviderRegistry, &Arc<InjectionRegistry>)); 4] =
            [(Source::Claude, crate::app::register_providers), (Source::Claude, claude_only), (Source::Codex, codex_only), (Source::Acp, acp_only)];
        for (source, register) in register {
            let second = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ProviderRegistry::new(injections.clone(), register)));
            let refused = second.err().unwrap_or_else(|| panic!("a second {source} provider refused while the first lasts"));
            let why = refused.downcast_ref::<String>().map(String::as_str).unwrap_or_default();
            assert!(why.contains(&format!("{source} runs are listed on this host already")), "{why}");
        }
        drop(first);
        let again = ProviderRegistry::new(injections, crate::app::register_providers);
        assert!(again.provider::<crate::provider::acp::Acp>().is_some());
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

    /// An instant chat turns attachments away when its CLI's chats cannot read
    /// them (Claude Code's), and when no provider runs its source. Codex's chats
    /// read them, and ACP runs no instant chats to refuse them in.
    #[test]
    fn a_chat_refuses_the_attachments_its_cli_cannot_read() {
        let home = tempfile::tempdir().expect("tempdir");
        let providers = providers(home.path());
        assert!(providers.chat_refuses_attachments(Source::Claude));
        assert!(!providers.chat_refuses_attachments(Source::Codex));
        assert!(!providers.chat_refuses_attachments(Source::Acp));
        let other = tempfile::tempdir().expect("tempdir");
        let claude = providers_from(other.path(), claude_only);
        assert!(claude.chat_refuses_attachments(Source::Claude));
        assert!(claude.chat_refuses_attachments(Source::Codex), "no provider runs codex here");
        assert!(claude.chat_refuses_attachments(Source::Acp), "no provider runs acp here");
    }

    /// An ACP model list names its agent by an id that can be a directory name,
    /// checked before the agent is looked up, and refused in these words
    /// otherwise; the other CLIs' lists ignore the field. A source no provider
    /// runs has no list.
    #[tokio::test]
    async fn an_acp_model_list_names_its_agent_by_an_id_that_can_be_a_directory_name() {
        let home = tempfile::tempdir().expect("tempdir");
        let providers = providers(home.path());
        let query = |source, agent: Option<&str>| crate::models::ModelsQuery {
            source,
            account_id: None,
            acp_agent: agent.map(str::to_owned),
        };
        let acp = providers.models(Source::Acp).expect("the ACP provider");
        let too_long = "x".repeat(crate::sessions::acp_transcript::MAX_AGENT_ID + 1);
        for agent in [None, Some("../fake"), Some("Fake"), Some(""), Some("a/b"), Some("a\0b"), Some(too_long.as_str())] {
            assert_eq!(
                acp.list(&query(Source::Acp, agent)).await.err().as_deref(),
                Some("acpAgent names the ACP agent to list models for"),
                "{agent:?}"
            );
        }
        let unconfigured = acp.list(&query(Source::Acp, Some("other"))).await.expect("a list");
        assert!(unconfigured.models.is_empty() && unconfigured.hidden.is_none());
        assert_eq!(unconfigured.source, crate::models::ModelListSource::Fallback, "an agent that is not configured has no list");
        let codex = providers.models(Source::Codex).expect("the Codex provider");
        assert!(codex.list(&query(Source::Codex, Some("../fake"))).await.is_ok(), "only an ACP list reads acpAgent");

        let other = tempfile::tempdir().expect("tempdir");
        assert!(providers_from(other.path(), |_, _| {}).models(Source::Acp).is_none());
    }

    /// Each CLI's model ids are held to its own spelling: names for Claude Code
    /// and Codex, `provider:model` paths for an ACP agent, and nothing at all
    /// for a source no provider runs.
    #[test]
    fn each_cli_spells_its_model_ids_its_own_way() {
        use crate::models::ModelRule;
        let home = tempfile::tempdir().expect("tempdir");
        let providers = providers(home.path());
        assert_eq!(providers.model_rule(Source::Claude), ModelRule::NAMED);
        assert_eq!(providers.model_rule(Source::Codex), ModelRule::NAMED);
        let acp = providers.model_rule(Source::Acp);
        assert!(acp.admits("openrouter:vendor/name") && acp.admits(&"m".repeat(200)));
        assert!(!acp.admits(&"m".repeat(201)) && !acp.admits("--add-dir") && !acp.admits("a b"));
        assert!(!ModelRule::NAMED.admits("openrouter:vendor/name"), "a path is not a name");

        let other = tempfile::tempdir().expect("tempdir");
        let none = providers_from(other.path(), |_, _| {});
        assert_eq!(none.model_rule(Source::Claude), ModelRule::NONE);
        assert!(!ModelRule::NONE.admits("opus"), "a source no provider runs takes no model");
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

    /// Each provider puts its own sessions away: Claude Code onto this server's
    /// hidden list, and ACP refuses, in its archive's own words, a session that
    /// is not there. A source no provider runs is refused.
    #[tokio::test]
    async fn each_provider_archives_its_own_sessions() {
        let home = tempfile::tempdir().expect("tempdir");
        let providers = providers(home.path());
        let paths = Paths::new(home.path().to_path_buf());
        providers.archive(Source::Claude, "c1").await.expect("archived");
        assert!(crate::provider::claude_code::sessions::claude::archive::archived_ids(&paths).contains("c1"));
        assert_eq!(
            providers.archive(Source::Acp, "a1").await,
            Err(droppen_sessions::archive::ArchiveError("cannot archive the ACP session: it is gone".to_string()).to_string())
        );

        let other = tempfile::tempdir().expect("tempdir");
        let none = providers_from(other.path(), |_, _| {});
        for source in [Source::Claude, Source::Codex, Source::Acp] {
            assert_eq!(none.archive(source, "x").await, Err(format!("no provider runs {source} sessions")), "{source}");
        }
        assert!(crate::provider::claude_code::sessions::claude::archive::archived_ids(&Paths::new(other.path().to_path_buf())).is_empty(), "nothing was archived");
    }
}
