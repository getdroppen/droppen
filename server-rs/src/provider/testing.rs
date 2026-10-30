//! What a test that drives a provider reads and builds that production code
//! never does: the steps of a spawn, the alerts a turn raised, a host timed
//! as the test needs. Only built for tests and with the `test-support`
//! feature.
//!
//! The probes that read a type's own private state sit beside that type,
//! under the same condition: [`SpawnGate::waiting`](super::SpawnGate::waiting),
//! [`SpawnGate::held`](super::SpawnGate::held),
//! [`SpawnGate::claim_held`](super::SpawnGate::claim_held),
//! [`SpawnGate::empty_lease`](super::SpawnGate::empty_lease),
//! [`SpawnGate::lease`](super::SpawnGate::lease),
//! [`SessionLease::holds`](super::SessionLease::holds),
//! [`ProviderRegistry::gate`](super::ProviderRegistry::gate),
//! [`RunsFile::insert_raw`](super::runs::RunsFile::insert_raw),
//! [`RunsFile::raw`](super::runs::RunsFile::raw) and
//! [`shim::connect_in_process`](super::shim::connect_in_process).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use droppen_instance::instance::InstanceDirs;
use droppen_instance::paths::Paths;
use droppen_sessions::{InjectionRunState, Source};

use super::{gate, Alerts, RunHost, Timings};

/// One point of a spawn, as the tests see it: whether the spawn gate and the
/// session's claim were held there.
///
/// The gate and the claim are the two locks that keep a second CLI off a
/// session, and which of them is held where is the contract the pins in the
/// tests check; a refactor of either keeps every sequence they record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// The point, by name.
    pub what: &'static str,
    /// Whether the spawn gate was held, by anyone.
    pub gate: bool,
    /// Whether the session's claim was held, by anyone; `None` where the
    /// session has no id yet.
    pub claim: Option<bool>,
}

impl RunHost {
    /// Note a [`Step`] of session `id`'s spawn.
    pub fn step(&self, what: &'static str, source: Source, id: Option<&str>) {
        let step = Step {
            what,
            gate: self.gate_held(),
            claim: id.map(|id| self.claim_held(source, id)),
        };
        self.steps.lock().expect("steps poisoned").push(step);
    }

    /// Every step noted so far, oldest first.
    pub fn steps(&self) -> Vec<Step> {
        self.steps.lock().expect("steps poisoned").clone()
    }

    /// Whether anyone holds the spawn gate.
    pub fn gate_held(&self) -> bool {
        self.watched_gate().is_some_and(|gate| gate.held())
    }

    /// Whether anyone holds session `id`'s claim: a claim of our own would be
    /// refused. One that is granted is let go at once.
    pub fn claim_held(&self, source: Source, id: &str) -> bool {
        self.watched_gate().is_some_and(|gate| gate.claim_held(source, id))
    }

    /// The gate the steps are read off, while its registry lasts.
    fn watched_gate(&self) -> Option<Arc<gate::SpawnGate>> {
        self.gate.lock().expect("held gate").upgrade()
    }
}

/// A host whose watchdog and interrupt ladder are timed as the test asks
/// rather than as the environment does, which is process-global and shared
/// with every other test in the binary.
pub fn host(
    paths: Arc<Paths>,
    instance: &InstanceDirs,
    accounts: Arc<droppen_accounts::store::AccountsStore>,
    quota: Arc<droppen_accounts::quota::QuotaCache>,
    idle_timeout: Duration,
    interrupt_step: Duration,
    watchdog_interval: Duration,
) -> Arc<RunHost> {
    let timings = Timings {
        idle_timeout,
        interrupt_step,
        watchdog_interval,
    };
    RunHost::timed(paths, instance, accounts, quota, timings)
}

/// Send `sig` to `pid`, as the host does to a run it has proved is its own:
/// for a test that kills the stand-in it started.
pub fn signal(pid: u32, sig: libc::c_int) {
    super::signal(pid, sig);
}

/// The turn alerts a test's drivers raised, each with what its snapshot said
/// of the alert's own session: its resident, if the snapshot had one.
#[derive(Default)]
pub struct TurnAlerts(Mutex<Vec<(Source, String, Option<droppen_sessions::ResidentInfo>)>>);

impl TurnAlerts {
    /// Every turn alert so far, oldest first.
    pub fn seen(&self) -> Vec<(Source, String, Option<droppen_sessions::ResidentInfo>)> {
        self.0.lock().expect("turn alerts").clone()
    }
}

impl Alerts for TurnAlerts {
    fn turn(&self, _paths: Arc<Paths>, source: Source, id: String, _outcome: InjectionRunState, _error: Option<String>, driver: droppen_sessions::DriverStates) {
        let resident = driver.resident(source, &id);
        self.0.lock().expect("turn alerts").push((source, id, resident));
    }

    fn question(&self, _paths: Arc<Paths>, _source: Source, _id: String, _question: String) {}

    fn rotation(
        &self,
        _paths: Arc<Paths>,
        _source: Source,
        _id: String,
        _outcome: droppen_sessions::account::RotationOutcome,
        _trigger: droppen_sessions::account::RotationTrigger,
        _text: String,
    ) {
    }
}
