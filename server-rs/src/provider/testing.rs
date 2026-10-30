//! What a test that drives a provider reads and builds that production code
//! never does: the steps of a spawn, the alerts a turn raised, a host timed
//! as the test needs, an injection registry on stand-in logins, a spawn
//! decoration that adds nothing, and the session shim run as a test binary.
//! Only built for tests and with the `test-support` feature.
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

use std::ffi::OsString;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use droppen_base::process_env::Env;
use tokio::process::Command;

use droppen_instance::instance::InstanceDirs;
use droppen_instance::paths::Paths;
use droppen_sessions::{InjectionRunState, Source};
use futures_util::future::BoxFuture;

use super::{gate, runs, Alerts, InjectionRegistry, ProviderRuns, RunHost, SpawnDecoration, Timings};

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

/// A spawn decoration that adds nothing: a test's spawns, with no channel to
/// a user.
pub fn no_decoration() -> Arc<dyn SpawnDecoration> {
    Arc::new(NoDecoration)
}

/// What [`no_decoration`] hands out.
struct NoDecoration;

impl SpawnDecoration for NoDecoration {
    fn decorate<'a>(&'a self, _source: Source, _id: &'a str, _env: &'a mut Env) -> BoxFuture<'a, ()> {
        Box::pin(std::future::ready(()))
    }
}

/// An injection registry on `home`, the base's alone: a fresh instance
/// there, the accounts on stand-in logins
/// ([`droppen_accounts::login::Logins::stand_ins`]) and spawns decorated with
/// nothing ([`no_decoration`]). No provider is made on it.
pub fn injection_registry(home: &std::path::Path) -> Arc<InjectionRegistry> {
    let paths = Arc::new(Paths::new(home.to_path_buf()));
    let dirs = InstanceDirs::new(&paths.data_dir(), droppen_instance::instance::DEFAULT_INSTANCE).expect("valid name");
    dirs.create().expect("instance dirs");
    let logins = droppen_accounts::login::Logins::stand_ins;
    InjectionRegistry::new(
        paths.clone(),
        &dirs,
        Arc::new(droppen_accounts::store::AccountsStore::new(paths.clone(), logins())),
        Arc::new(droppen_accounts::quota::QuotaCache::new(paths, logins())),
        no_decoration(),
    )
}

/// An injection registry over `host`, which the test made with the timings
/// it waits out ([`host`]) rather than the environment's.
pub fn injection_registry_on(host: Arc<RunHost>, instance: InstanceDirs, decoration: Arc<dyn SpawnDecoration>) -> Arc<InjectionRegistry> {
    Arc::new(InjectionRegistry { host, instance, decoration })
}

/// Watch `record`'s run as the boot pass does once it has claimed it with
/// `claim`: back in the runs map, its pid watched until it ends.
pub fn watch_adopted(registry: &Arc<InjectionRegistry>, runs: Arc<dyn ProviderRuns>, record: runs::RunRecord, claim: gate::SessionLease) {
    registry.clone().resume(runs, record, claim);
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

/// Where a test binary run as the session shim finds the shim's own
/// arguments, as a JSON array: a libtest command line has no room for them.
const SHIM_ARGV: &str = "DROPPEN_UNIT_TEST_SHIM_ARGV";

/// The pid of the test process that started the shim, which the shim
/// [`session_shim`] ends with.
const SHIM_TEST: &str = "DROPPEN_UNIT_TEST_SHIM_TEST";

/// The module whose `session_shim` test this process's shims run as, once a
/// test has said so ([`shims_run_as`]).
static SHIM_MODULE: OnceLock<&'static str> = OnceLock::new();

/// Run every session shim this process starts as this test binary itself,
/// running the test `session_shim` of `module` (its `module_path!()`), whose
/// body is [`session_shim`].
///
/// [`droppen_pty::shim_program`] cannot serve a test. From a test binary it
/// falls back to the server binary beside it in `target/debug`, which
/// `cargo test --lib` never builds and which any other run may have left there
/// from older source. Absent, the test binary itself was run as
/// `session-shim --socket …`, libtest refused the option and exited 101, and
/// every respawn timed out on a socket nobody would bind. Present, the shim
/// under test was whatever that binary happened to be. The test binary is
/// always there, and always this source. This crate's own tests run theirs
/// without asking.
pub fn shims_run_as(module: &'static str) {
    let _ = SHIM_MODULE.set(module);
}

/// The test binary as the session shim told `argv`, in exactly `env`, once a
/// test has said which test is the shim, and always in this crate's own tests:
/// libtest asked for that test and nothing else, the shim's arguments in
/// [`SHIM_ARGV`] and this process's pid in [`SHIM_TEST`].
pub(crate) fn shim_command(argv: &[OsString], env: &Env) -> Option<Command> {
    #[cfg(test)]
    let module = Some(SHIM_MODULE.get().copied().unwrap_or(concat!(module_path!(), "::tests")));
    #[cfg(not(test))]
    let module = SHIM_MODULE.get().copied();
    let module = module?;
    let argv: Vec<String> = argv
        .iter()
        .map(|arg| arg.to_str().expect("a test's shim arguments are UTF-8").to_owned())
        .collect();
    // libtest names a test by its path inside the crate.
    let (_crate, module) = module.split_once("::").expect("a module inside a crate");
    let mut command = Command::new(std::env::current_exe().expect("this test binary"));
    command
        .args(["--ignored", "--exact", "--quiet", &format!("{module}::session_shim")])
        .env_clear()
        .envs(env)
        .env(SHIM_ARGV, serde_json::to_string(&argv).expect("json"))
        .env(SHIM_TEST, std::process::id().to_string());
    Some(command)
}

/// The session shim, as the body of a test binary's `session_shim` test when
/// [`shims_run_as`] runs the binary as one. Run any other way it has no
/// arguments, and returns.
///
/// It exits rather than returns once the shim is done, with the code the
/// server's `session-shim` would: the process is the shim, and libtest's
/// summary after it would be a second ending.
///
/// Unlike the server's, it also ends with the test process that started it.
/// A server's shim outlives the server, which is what it is for; a test's has
/// nothing to outlive it for, and a test can end while its driver is still
/// respawning, before the new run is in the journal its harness ends
/// processes from. The shim would then hold its claude's stdin open for ever.
/// Once it exits, its claude reads end of file and goes too.
pub fn session_shim() {
    let Ok(argv) = std::env::var(SHIM_ARGV) else {
        return;
    };
    let test: u32 = std::env::var(SHIM_TEST).ok().and_then(|pid| pid.parse().ok()).expect("the test's pid");
    std::thread::spawn(move || {
        // Once the test process is gone, this one is someone else's child.
        while std::os::unix::process::parent_id() == test {
            std::thread::sleep(Duration::from_millis(100));
        }
        std::process::exit(1);
    });
    let argv: Vec<String> = serde_json::from_str(&argv).expect("the shim's arguments");
    let outcome = super::shim::parse_command_line(&argv).map_err(|error| error.to_string()).and_then(|args| {
        let runtime = tokio::runtime::Runtime::new().map_err(|error| format!("cannot start a runtime: {error}"))?;
        runtime.block_on(args.run())
    });
    std::process::exit(i32::from(outcome.is_err()));
}

#[cfg(test)]
mod tests {
    /// Not a test: this crate's own session shim ([`super::session_shim`]).
    #[test]
    #[ignore = "the session shim a unit test's respawn runs, not a test"]
    fn session_shim() {
        super::session_shim();
    }
}
