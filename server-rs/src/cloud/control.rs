//! Starting and stopping the hub link while the server runs: [`Control`] is
//! the server's handle on its link.
//!
//! - at boot it starts the link when `agent/state.json` is there (an instance
//!   that never enrolled starts nothing);
//! - [`Control::enroll`] runs the same enrollment `agent enroll` does
//!   ([`AgentFiles::begin_enrollment`]) in this process and starts the link at once,
//!   which then follows the chain until a device adds the machine;
//! - the link stops itself when this machine is removed from the account
//!   ([`crate::agent::link`]), and [`Control::status`] says so;
//! - [`Control::unenroll`] stops it and deletes everything under `agent/`.
//!
//! Nothing here ever runs on a request path that is not about the link: the
//! local API's other routes never wait for the hub.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use rh_proto::identity::{EnrollCode, MachineIdentity};

use crate::agent::enroll::BeginError;
use crate::agent::link::{Health, LinkEnv, Running};
use crate::agent::state::AgentState;
use crate::agent::status::{AgentStatus, Standing};
use crate::agent::AgentFiles;
use droppen_instance::paths::Paths;

/// Why [`Control`] could not do what it was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlError {
    /// This server cannot run a link: it does not own its instance
    /// (`export-openapi`, a test server), or `DROPPEN_AGENT_LINK=0`.
    Unavailable(String),
    /// The enrollment could not begin.
    Begin(BeginError),
    /// The hub could not be reached.
    Unreachable(String),
    /// Anything else, in words.
    Failed(String),
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlError::Unavailable(m) | ControlError::Failed(m) | ControlError::Unreachable(m) => f.write_str(m),
            ControlError::Begin(e) => write!(f, "{e}"),
        }
    }
}

impl From<crate::agent::hub::DeleteError> for ControlError {
    fn from(error: crate::agent::hub::DeleteError) -> Self {
        match error {
            crate::agent::hub::DeleteError::Failed(m) => ControlError::Failed(m),
            crate::agent::hub::DeleteError::Unreachable(m) => ControlError::Unreachable(m),
        }
    }
}

/// The server's handle on its hub link. One per server, in
/// [`crate::api::AppState`].
pub struct Control {
    /// Itself, for the link to ask for a restart once it took a renewed
    /// identity key ([`Control::restart`]).
    me: std::sync::Weak<Control>,
    paths: Paths,
    files: AgentFiles,
    /// Set once the server owns its instance and listens
    /// ([`crate::app::RemoteHandApp::prepare`]); a server without it (no
    /// lock: `export-openapi`, tests) never starts a link.
    env: OnceLock<LinkEnv>,
    running: Mutex<Option<Running>>,
    /// Enrolling, starting, stopping and unenrolling one at a time.
    gate: tokio::sync::Mutex<()>,
    /// Bumped by every enrollment, so a stale watchdog does nothing.
    generation: Mutex<u64>,
    /// The other devices' copies in memory, for the local gateway:
    /// the copies task of whichever link runs publishes into it.
    live: Arc<crate::aggregator::live::LiveCopies>,
}

impl Control {
    /// The handle for one server.
    pub fn new(paths: Paths) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            me: me.clone(),
            files: AgentFiles::of(&paths),
            paths,
            env: OnceLock::new(),
            running: Mutex::new(None),
            gate: tokio::sync::Mutex::new(()),
            generation: Mutex::new(0),
            live: crate::aggregator::live::LiveCopies::new(),
        })
    }

    /// What a link calls once it renewed its identity key: this control
    /// restarts it ([`Control::restart`]) on a task of its own — the link
    /// that asks is the one stopped.
    fn restart_hook(&self) -> crate::agent::rotation::Restart {
        let me = self.me.clone();
        Arc::new(move || {
            let Some(control) = me.upgrade() else { return };
            tokio::spawn(async move {
                if let Err(error) = control.restart().await {
                    tracing::error!(%error, "cannot restart the hub link with the renewed identity key");
                }
            });
        })
    }

    /// Stop the link and start it again from what is on disk — after an
    /// identity renewal, with the new key: its hub connection, relays,
    /// channels, this computer's device role and the streams' writer. The
    /// server, its AI sessions and terminals go on untouched (they never ran
    /// under the link).
    pub async fn restart(&self) -> Result<(), ControlError> {
        let _gate = self.gate.lock().await;
        self.stop_locked().await;
        tracing::info!("restarting the hub link");
        self.start_locked().await
    }

    /// The other devices' copies as the gateway serves them.
    pub fn live(&self) -> &Arc<crate::aggregator::live::LiveCopies> {
        &self.live
    }

    /// Give the handle what a link needs (the owner, once listening). First,
    /// whatever deletion of the other devices' copies an earlier run left
    /// unfinished is finished — whether the aggregator is on or not
    /// ([`crate::aggregator::sweep_deleted`]).
    pub fn attach(&self, env: LinkEnv) {
        crate::aggregator::sweep_deleted(&self.paths);
        // Adding this device again under a new identity, cut short by a
        // crash: finished before anything starts.
        if let Err(error) = crate::agent::rotation::finish_re_enroll(&self.paths) {
            tracing::error!(%error, "cannot finish adding this device again under a new identity");
        }
        // A dispute statement the hub has not taken yet: delivered again
        // (it outlives the enrollment it disputes).
        self.files.deliver_disputes();
        let _ = self.env.set(env);
    }

    /// Whether this server can run a link at all.
    pub fn available(&self) -> Result<&LinkEnv, ControlError> {
        if !crate::agent::link::enabled() {
            return Err(ControlError::Unavailable("the hub link is turned off here (DROPPEN_AGENT_LINK=0)".into()));
        }
        self.env
            .get()
            .ok_or_else(|| ControlError::Unavailable("this server does not own its instance, so it runs no hub link".into()))
    }

    /// At boot: start the link if this instance is enrolled (or enrolling)
    /// and not removed. `false`, and nothing started, otherwise — the server
    /// is then exactly the single-machine server.
    pub async fn start_if_enrolled(&self) -> bool {
        if self.available().is_err() || !self.files.has_state() {
            return false;
        }
        let _gate = self.gate.lock().await;
        match self.start_locked().await {
            Ok(()) => true,
            Err(error) => {
                tracing::error!(%error, "not connecting to the hub");
                false
            }
        }
    }

    async fn start_locked(&self) -> Result<(), ControlError> {
        let env = self.available()?;
        let old = {
            let mut running = self.running.lock().expect("running");
            if running.as_ref().is_some_and(|r| !r.finished()) {
                return Ok(());
            }
            running.take()
        };
        // A link that ended on its own (a removal, other keys) is stopped
        // before another starts: its device role, or its streams' writer, may
        // still be going.
        if let Some(old) = old {
            old.stop().await;
        }
        // An install still enrolled at the legacy apex hub moves to the
        // canonical `api.` origin once, if the hub takes its sign-in
        // ([`crate::agent::migrate_hub`]).
        crate::agent::migrate_hub(&self.paths).await.map_err(ControlError::Failed)?;
        let started = crate::agent::link::start(&self.paths, env, self.live.clone(), self.restart_hook()).map_err(ControlError::Failed)?;
        *self.running.lock().expect("running") = Some(started);
        Ok(())
    }

    /// Stop the link, if it runs, and wait for it.
    pub async fn stop(&self) {
        let _gate = self.gate.lock().await;
        self.stop_locked().await;
    }

    async fn stop_locked(&self) {
        let running = self.running.lock().expect("running").take();
        if let Some(running) = running {
            running.stop().await;
        }
        // Whatever the copies task left in memory goes with it (it clears
        // it itself when it ends, unless it had to be cut short).
        self.live.clear();
    }

    /// Where the link stands, without waiting for anything but the local
    /// disk: the running link's own view, or the saved state.
    pub fn status(&self) -> AgentStatus {
        let running = self.running.lock().expect("running");
        if let Some(run) = running.as_ref() {
            let mut status = run.with_state(|st| AgentStatus::of_instance(&self.files, st));
            let finished = run.finished();
            let Health {
                connected,
                since_ms,
                error,
                refused,
            } = run.health();
            status.running = !finished;
            status.connected = connected && !finished;
            status.since_ms = (since_ms > 0).then_some(since_ms);
            status.error = error;
            status.refused = refused;
            return status;
        }
        drop(running);
        match AgentState::load(&self.files) {
            Ok(Some(st)) => AgentStatus::of_instance(&self.files, &st),
            Ok(None) => AgentStatus::none(),
            Err(error) => AgentStatus {
                state: Standing::Broken,
                error: Some(error),
                ..AgentStatus::none()
            },
        }
    }

    /// What this computer's device role reports: `None` when the
    /// link does not run here.
    pub fn control_status(&self) -> Option<crate::aggregator::controller::Status> {
        let running = self.running.lock().expect("running");
        running.as_ref().filter(|r| !r.finished()).map(Running::control_status)
    }

    /// This instance's machine identity (made on first use, kept in
    /// `agent/machine-key.json`): what a device checks an enrollment against.
    pub fn identity(&self) -> Result<MachineIdentity, String> {
        crate::agent::load_or_create(&self.paths).map(|(identity, _)| identity)
    }

    /// Enroll in this process: present `code` at `hub_url`, prove it, save the
    /// enrolling state and start the link, which completes the enrollment as
    /// soon as a device adds this machine — no restart. The account and the
    /// machine id the hub assigned. If no device adds it within the code's
    /// lifetime, the link stops and the enrolling state is forgotten (the
    /// machine key stays, for the next try).
    pub async fn enroll(self: &Arc<Self>, code: &EnrollCode, hub_url: &str) -> Result<(String, String), ControlError> {
        self.available()?;
        let _gate = self.gate.lock().await;
        // Whatever ran (an enrollment that went nowhere) makes way for this
        // one — and comes back if this one cannot begin (the hub unreachable,
        // the code refused): a failed sign-in never leaves the link stopped.
        let was_running = self.running.lock().expect("running").as_ref().is_some_and(|r| !r.finished());
        self.stop_locked().await;
        // A seed a renewal retired (or one archived) never signs up again:
        // a fresh one then (`docs/host-identity-rotation.md` §3.8).
        let begun = match crate::agent::rotation::identity_for_enrolling(&self.paths) {
            Ok(identity) => self.files.begin_enrollment(&identity, code, hub_url)
                .await
                .map_err(ControlError::Begin),
            Err(error) => Err(ControlError::Failed(error)),
        };
        let begun = match begun {
            Ok(begun) => begun,
            Err(error) => {
                if was_running {
                    if let Err(restart) = self.start_locked().await {
                        tracing::error!(%restart, "cannot restart the hub link after a failed sign-in");
                    }
                }
                return Err(error);
            }
        };
        let (account, machine) = (begun.account.clone(), begun.machine.clone());
        tracing::info!(hub = %begun.origin, %account, %machine, "enrolled in process; starting the hub link to finish");
        // The link signs in again on its own socket and follows the chain.
        drop(begun);
        self.start_locked().await?;
        let generation = {
            let mut g = self.generation.lock().expect("generation");
            *g += 1;
            *g
        };
        let this = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(crate::agent::enroll::CONFIRM_TIMEOUT + Duration::from_secs(5)).await;
            this.expire_enrollment(generation).await;
        });
        Ok((account, machine))
    }

    /// The code's lifetime is over: an enrollment still waiting for a device
    /// is forgotten (unless another one began since).
    async fn expire_enrollment(&self, generation: u64) {
        let _gate = self.gate.lock().await;
        if *self.generation.lock().expect("generation") != generation {
            return;
        }
        let waiting = match AgentState::load(&self.files) {
            Ok(Some(st)) => waiting_for_a_device(&st),
            _ => false,
        };
        if !waiting {
            return;
        }
        tracing::warn!("no device added this machine within the enrollment code's lifetime; forgetting the enrollment");
        self.stop_locked().await;
        if let Err(error) = self.files.forget_enrollment() {
            tracing::error!(%error, "cannot forget the expired enrollment");
        }
    }

    /// Forget an enrollment no device has completed (cancelled from the
    /// local settings): stop the link and delete the enrolling state; the
    /// machine key stays. Does nothing to an enrolled (running AI or not) or
    /// removed state.
    pub async fn cancel_enrollment(&self) -> Result<bool, ControlError> {
        let _gate = self.gate.lock().await;
        let waiting = match AgentState::load(&self.files) {
            Ok(Some(st)) => waiting_for_a_device(&st),
            Ok(None) => return Ok(false),
            Err(error) => return Err(ControlError::Failed(error)),
        };
        if !waiting {
            return Ok(false);
        }
        self.stop_locked().await;
        self.files.forget_enrollment().map_err(ControlError::Failed)?;
        Ok(true)
    }

    /// Delete this machine's encrypted copy on the hub (every stream it
    /// holds for the machine), on a connection of its own: the link is
    /// stopped first, so the hub has one connection for the machine. The
    /// number of streams deleted — none for a machine that does not run AI
    /// in the account now (the hub signs in hosts only; an admin device
    /// deletes such a copy).
    pub async fn delete_copies(&self) -> Result<usize, ControlError> {
        let _gate = self.gate.lock().await;
        self.stop_locked().await;
        let st = match AgentState::load(&self.files) {
            Ok(Some(st)) if st.hosting() => st,
            Ok(_) => return Ok(0),
            Err(error) => return Err(ControlError::Failed(error)),
        };
        let identity = self.identity().map_err(ControlError::Failed)?;
        tokio::time::timeout(Duration::from_secs(30), crate::agent::hub::delete_streams(&st, &identity))
            .await
            .map_err(|_| ControlError::Unreachable("the hub did not answer in time".into()))?
            .map_err(ControlError::from)
    }

    /// Before a sign-in shows this computer's keys: when the seed in use is
    /// one it must never use again — archived, or replaced elsewhere on the
    /// chain it knows ([`AgentFiles::seed_retired`]: never its own
    /// renewal to a key it staged) — it never signs up again:
    /// the link stops, the seed is archived, the old enrollment forgotten and
    /// a fresh seed made ([`Control::re_enroll_fresh`]). `true` when that happened.
    pub async fn renew_retired_seed(&self) -> Result<bool, ControlError> {
        let _gate = self.gate.lock().await;
        if !self.files.seed_in_use_retired().map_err(ControlError::Failed)? {
            return Ok(false);
        }
        self.stop_locked().await;
        crate::agent::rotation::re_enroll_fresh(&self.paths, droppen_base::time::now_ms_u64()).map_err(ControlError::Failed)?;
        Ok(true)
    }

    /// "Add this device again with a new identity": for a device left holding
    /// only a retired identity key (`link.rotation.state` `foreign`), stop
    /// the link, archive the seed, forget the enrollment and make a fresh
    /// seed ([`crate::agent::rotation::re_enroll_fresh`]). The new identity; the
    /// instance is then not enrolled, waiting for a registration code.
    pub async fn re_enroll_fresh(&self) -> Result<MachineIdentity, ControlError> {
        let _gate = self.gate.lock().await;
        self.stop_locked().await;
        crate::agent::rotation::re_enroll_fresh(&self.paths, droppen_base::time::now_ms_u64()).map_err(ControlError::Failed)
    }

    /// Stop the link and delete everything under `agent/`: the state, the
    /// machine keys, the streams' store and the machine key itself. The
    /// instance is the single-machine server again.
    pub async fn unenroll(&self) -> Result<(), ControlError> {
        let _gate = self.gate.lock().await;
        self.stop_locked().await;
        self.files.unenroll().map_err(ControlError::Failed)
    }
}

/// Whether `st` is an enrollment still waiting for a device to add this
/// machine: the chain does not name it (yet), and has not removed it. Never
/// a member that merely stopped running AI: counting one as waiting would let the
/// code's expiry, or cancel, delete its state.
fn waiting_for_a_device(st: &AgentState) -> bool {
    !st.enrolled() && !st.removed()
}

/// What the agent's tests of a running link reach (`agent::control_tests`):
/// the link this control runs, the restart hook it hands the link, and the
/// expiry of an enrollment's code.
#[cfg(test)]
impl Control {
    pub(crate) fn running_link(&self) -> std::sync::MutexGuard<'_, Option<Running>> {
        self.running.lock().unwrap()
    }

    pub(crate) fn link_restart_hook(&self) -> crate::agent::rotation::Restart {
        self.restart_hook()
    }

    pub(crate) async fn expire_enrollment_now(&self, generation: u64) {
        self.expire_enrollment(generation).await
    }
}
