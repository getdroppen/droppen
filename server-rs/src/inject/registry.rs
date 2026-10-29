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

use tokio::sync::watch;

use super::{drafts, InjectionRegistry, ResidentState};
use crate::sessions::{DriverStates, Injection, InjectionRunState, ResidentInfo, SessionSummary, Source};
use droppen_http::error::ApiError;

/// One CLI this server drives.
pub trait Provider: Send + Sync {
    /// The sessions it runs: the key the registry files it under.
    fn source(&self) -> Source;

    /// What it says about those sessions.
    fn run(&self) -> &dyn Run;
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
        no_agent(acp_agent)
    }

    /// Why no message can go to this session whatever is running for it, or
    /// `None` when nothing in the provider stops one.
    fn send_block(&self, _summary: &SessionSummary) -> Option<String> {
        None
    }
}

/// [`Run::check_start`] of a provider that runs no ACP agent.
fn no_agent(acp_agent: Option<&str>) -> Result<(), ApiError> {
    match acp_agent {
        None => Ok(()),
        Some(_) => Err(ApiError::bad_request("acpAgent is only for ACP sessions")),
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

    /// See [`Run::check_start`]; a source with no provider takes no agent
    /// either.
    pub fn check_start(&self, source: Source, acp_agent: Option<&str>) -> Result<(), ApiError> {
        match self.run(source) {
            Some(run) => run.check_start(acp_agent),
            None => no_agent(acp_agent),
        }
    }

    /// See [`Run::send_block`], asked of the provider of the session's source;
    /// `None` for a source with no provider.
    pub fn send_block(&self, summary: &SessionSummary) -> Option<String> {
        self.run(summary.source)?.send_block(summary)
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
        ProviderRegistry::new(InjectionRegistry::new(paths, &dirs, accounts, quota), crate::app::register_providers)
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
        let row = |source: &str, agent: Option<&str>| -> SessionSummary {
            serde_json::from_value(serde_json::json!({
                "pinned": false, "id": "s", "source": source, "scope": "owned", "server": "s", "title": "t",
                "cwd": null, "repoKey": null, "gitBranch": null, "updatedAt": "", "createdAt": "",
                "sizeBytes": 1, "filePath": "/x", "status": "idle", "activity": "idle", "resident": null,
                "claudeRoot": null, "accountId": null, "attentionAt": null, "unread": false, "unreadCount": 0,
                "acpAgent": agent,
            }))
            .unwrap_or_else(|error| panic!("a session row: {error}"))
        };
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
