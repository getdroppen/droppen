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
use crate::sessions::{DriverStates, Injection, InjectionRunState, ResidentInfo, Source};

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
}
