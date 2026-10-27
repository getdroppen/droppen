//! `remote-hand-server up` — the dumb loop that keeps a server running.
//!
//! This is a supervisor in the smallest sense the word allows: it starts one
//! child, waits for it, starts it again, and reaps whatever else wanders in. It
//! deliberately has no configuration, no health endpoint and no opinion about
//! what the child does, because everything it might have grown into is something
//! the server itself already knows how to do — and a supervisor that can crash
//! is worse than no supervisor at all.
//!
//! It exists for two jobs.
//!
//! # Restarting into a new binary
//!
//! [`crate::selfupdate`] installs a new binary and then stops the server with a
//! zero exit. Something has to notice and start the new one; in a container
//! there is nothing else to. So a clean exit here means "start again
//! immediately" rather than "we are done" — the inversion of the usual rule, and
//! the reason this loop exists at all.
//!
//! A binary that will not stay up is the failure mode that turns a self-upgrade
//! into a dead container, so the loop climbs down a ladder rather than
//! hammering: back off from two seconds to thirty, and after three fast failures
//! in a row move the installed binary aside as `.bad` and fall back to `.prev`,
//! then to the one baked into the image. Every step down is logged loudly,
//! because "your upgrade was rolled back" is not something to discover from a
//! version string.
//!
//! # Waiting for a holder, which is not a failure
//!
//! One kind of fast exit says nothing about the binary: `serve` finding that
//! another server already holds its instance lock ([`crate::instance`]). The
//! everyday way to get there is this loop itself being killed outright — a
//! `SIGKILL`, the OOM killer, a service manager whose stop timeout ran out. Its
//! `serve` child survives by design (a process group of its own,
//! `KillMode=process`, `AbandonProcessGroup`), keeps the lock, and keeps
//! serving. The service manager then starts a fresh supervisor, whose `serve`
//! is refused at once. Counting that refusal as a failure would move a
//! perfectly good binary to `.bad` after three tries — and with no `.prev` and
//! nothing better baked in, leave the service down until somebody reinstalled
//! it.
//!
//! So `serve` exits with [`INSTANCE_HELD_EXIT`] for that refusal and nothing
//! else, and this loop treats it as "wait": it neither counts the exit as a fast
//! failure nor forgives earlier ones, never steps down over it, says whose pid
//! holds the lock, and tries again on the usual backoff (capped at thirty
//! seconds). The moment the holder exits — a self-upgrade, which exits zero and
//! expects a supervisor that is no longer there, or an operator stopping it —
//! the next attempt takes the lock and this loop is supervising again.
//!
//! It deliberately does not *adopt* the orphan — watch its pid, forward signals
//! to it, wait on it. It is not this process's child, so it cannot be
//! `waitpid`ed, only polled by pid; the pid in the lock file is advisory and a
//! dead holder's pid can be reused, so signalling it is exactly the
//! "`kill` a pid this loop did not start" that the signal rules below forbid;
//! and the holder need not be an orphan at all (a second `up` started by
//! mistake, or a person running `serve` by hand), in which case adopting it
//! would be stealing. Retrying the real `serve` keeps one source of truth — the
//! kernel drops the `flock` exactly when the holder dies, with no pid reuse to
//! get wrong — at the cost of one short-lived process per backoff and up to
//! thirty seconds between the holder leaving and this loop's server arriving.
//!
//! # Reaping orphans (this process is pid 1)
//!
//! In the container this is pid 1, which makes it the machine's reaper.
//! `crate::instance`'s module docs set this problem up: every pty shim, embedded
//! app and detached CLI is spawned to *outlive* the server, so when the server
//! exits they are reparented to pid 1 — and if pid 1 never `wait()`s, each one
//! becomes a zombie that holds a pid until the container dies. A shell script
//! with `exec` in it is not a reaper. This loop is: it waits on **every** child,
//! not just its own, and throws away the statuses of the ones it did not start.
//!
//! # Signals
//!
//! `SIGTERM` and `SIGINT` are forwarded **to the serve child only** — never to
//! its process group, and never broadcast. This is the whole point of the
//! design, and it is not a detail: the shims and apps are in process groups of
//! their own precisely so that stopping the server does not stop them, and a
//! supervisor that signalled groups would undo every part of restart-survival in
//! one line. `systemd` spells this `KillMode=process`; here it is spelled
//! `kill(child, sig)`.
//!
//! After forwarding, the loop keeps waiting and then exits with the child's
//! status instead of restarting it: a signal is somebody asking for the whole
//! thing to stop.

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use crate::instance::InstanceError;
use crate::selfupdate;
use crate::selfupdate::store::{BinStore, Fallback};
use crate::sessions::Paths;
use crate::GotchaError;

/// The first wait after a failure.
const MIN_BACKOFF: Duration = Duration::from_secs(2);

/// The longest wait between restarts, however badly things are going.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// How long a run has to last to count as healthy, resetting the ladder.
const HEALTHY_AFTER: Duration = Duration::from_secs(60);

/// Consecutive fast failures before the installed binary is moved aside.
const FAST_FAILURES: u32 = 3;

/// The exit code of a `serve` refused because another server already owns its
/// instance: `EX_TEMPFAIL` from `sysexits.h`, "try again later".
///
/// The contract between `serve` and this loop, and the only exit code other
/// than zero that this loop reads anything into (see the module docs). Chosen
/// to stay clear of everything else a child can end with: every other failure
/// of every subcommand exits 1, and a death by signal is reported as `128 + n`.
/// A person running `serve` by hand sees the same refusal message; only the
/// status differs.
pub const INSTANCE_HELD_EXIT: u8 = 75;

/// The shortest gap between two restarts.
///
/// A clean exit restarts immediately, which is what makes a self-upgrade feel
/// instant — but "immediately" has to have a floor, or a binary that exits zero
/// the moment it starts becomes a fork bomb with good intentions.
const MIN_RESTART_GAP: Duration = Duration::from_millis(200);

/// How often the loop looks for a child that has exited.
///
/// A poll rather than a blocking `waitpid`, on purpose. Blocking would need the
/// signal handler to interrupt it, and there is an unclosable window between
/// "check whether a signal arrived" and "enter the kernel" in which a signal is
/// recorded but the wait blocks anyway — the classic race that `sigsuspend`
/// exists to solve. A supervisor that is idle by design can afford to look ten
/// times a second instead, and then the race does not exist.
const POLL: Duration = Duration::from_millis(100);

/// The signal that arrived, or 0. Written by the handler, read by the loop.
static SIGNALLED: AtomicI32 = AtomicI32::new(0);

/// Run the supervisor loop. Returns the exit code the process should use.
///
/// `child_args` is the command line for the server, e.g. `["serve"]`.
pub fn run(child_args: Vec<String>) -> Result<i32, String> {
    install_handlers()?;

    let baked = std::env::current_exe().map_err(|error| format!("cannot find this executable: {error}"))?;
    let paths = load_paths()?;
    let store = paths.seed_binary_store(&baked);

    say(&format!(
        "supervising `{}` from {} (baked: {})",
        child_args.join(" "),
        store.dir().display(),
        baked.display()
    ));

    // Only ever read, to name whoever `serve` was refused by. `None` for a name
    // `serve` will refuse anyway — as a bad name, long before any lock.
    let lock_file = crate::instance::InstanceDirs::new(&paths.data_dir(), paths.instance())
        .ok()
        .map(|dirs| dirs.lock_file());

    let mut ladder = Ladder::new();

    loop {
        let exe = store.resolve(&baked);
        let started = Instant::now();
        let (child, outcome) = match spawn(&exe, &child_args) {
            Ok(child) => {
                say(&format!("started {} (pid {child})", exe.display()));
                let status = wait_for(child)?;
                let ran = started.elapsed();
                let code = exit_code(status);
                if stopping() {
                    say(&format!("child {child} exited ({code}) after a signal; stopping"));
                    return Ok(code);
                }
                (child, Outcome::Exited { code, ran })
            }
            Err(error) => {
                say(&format!("cannot start {}: {error}", exe.display()));
                (0, Outcome::NotStarted)
            }
        };
        // `code` is also what `up` exits with if a stop arrives during the wait
        // below: the last child's status, or 0 when there was no child.
        let (code, ran) = match outcome {
            Outcome::Exited { code, ran } => (code, ran),
            Outcome::NotStarted => (0, Duration::ZERO),
        };

        match ladder.after(outcome) {
            Next::Restart => {
                // The deliberate case: a self-upgrade, or an operator stopping
                // just the server. Nothing is wrong, so nothing is waited for.
                say(&format!("child {child} exited cleanly after {ran:?}; restarting"));
                if let Some(remaining) = MIN_RESTART_GAP.checked_sub(ran) {
                    sleep_unless_signalled(remaining);
                }
            }
            Next::AwaitHolder { wait } => {
                let holder = crate::instance::describe_holder(lock_file.as_deref().and_then(crate::instance::recorded_holder));
                let at = lock_file.as_deref().map_or_else(String::new, |path| format!(" ({})", path.display()));
                say(&format!(
                    "child {child} exited with {code}: another server ({holder}) holds instance {:?}{at}. \
                     Not counted against the binary; trying again in {wait:?}, and taking over once that one exits",
                    paths.instance(),
                ));
                sleep_unless_signalled(wait);
            }
            Next::Retry {
                wait,
                fast_failures,
                step_down,
            } => {
                if matches!(outcome, Outcome::Exited { .. }) {
                    say(&format!(
                        "child {child} exited with {code} after {ran:?} (fast failures: {fast_failures}/{FAST_FAILURES})"
                    ));
                }
                if step_down {
                    store.step_down(&baked);
                }
                say(&format!("restarting in {wait:?}"));
                sleep_unless_signalled(wait);
            }
        }
        if stopping() {
            return Ok(code);
        }
    }
}

/// How one attempt to run the child ended — the only input [`Ladder`] needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The binary could not be started at all.
    NotStarted,
    /// The child ran for `ran` and exited with `code` (a signal death as
    /// `128 + n`, the way [`exit_code`] reports it).
    Exited { code: i32, ran: Duration },
}

/// What the loop does next, as [`Ladder::after`] decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    /// A clean exit: start again at once, give or take [`MIN_RESTART_GAP`].
    Restart,
    /// Another server holds the instance: wait, then try again. The binary is
    /// not at fault, so nothing is counted against it.
    AwaitHolder { wait: Duration },
    /// A failure: wait, then try again, moving the binary aside first when
    /// [`BinStore::step_down`] says this was the last fast failure allowed.
    /// `fast_failures` is the count that decided it, for the log.
    Retry { wait: Duration, fast_failures: u32, step_down: bool },
}

/// The rollback ladder's memory between runs: how long the next wait is, and
/// how many fast failures in a row the current binary has had.
///
/// Pure on purpose — no clock, no files, no processes — so every rule about
/// what counts against a binary is a function of what happened and can be
/// tested as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ladder {
    backoff: Duration,
    fast_failures: u32,
}

impl Ladder {
    fn new() -> Self {
        Self {
            backoff: MIN_BACKOFF,
            fast_failures: 0,
        }
    }

    /// Record how a run ended and say what to do about it.
    fn after(&mut self, outcome: Outcome) -> Next {
        let (code, ran) = match outcome {
            Outcome::Exited { code, ran } => (code, ran),
            // Not even started is the clearest failure of all, and fast by
            // definition.
            Outcome::NotStarted => return self.failed(true),
        };
        if ran >= HEALTHY_AFTER {
            self.backoff = MIN_BACKOFF;
            self.fast_failures = 0;
        }
        if code == 0 {
            self.fast_failures = 0;
            return Next::Restart;
        }
        if code == i32::from(INSTANCE_HELD_EXIT) {
            // Neither a failure nor a success of this binary: it got as far as
            // asking for the lock and was told to wait. The count is left
            // exactly where it was, and the wait still grows — a holder that
            // stays for an hour should cost a retry every thirty seconds, not a
            // stream of them.
            return Next::AwaitHolder { wait: self.take_backoff() };
        }
        self.failed(ran < HEALTHY_AFTER)
    }

    fn failed(&mut self, fast: bool) -> Next {
        if fast {
            self.fast_failures += 1;
        }
        let fast_failures = self.fast_failures;
        let step_down = fast_failures >= FAST_FAILURES;
        if step_down {
            // The next binary starts with a clean count; the wait is *not*
            // reset — see [`step_down`].
            self.fast_failures = 0;
        }
        Next::Retry {
            wait: self.take_backoff(),
            fast_failures,
            step_down,
        }
    }

    /// The wait to use now, growing the one after it.
    fn take_backoff(&mut self) -> Duration {
        let wait = self.backoff;
        self.backoff = next_backoff(wait);
        wait
    }
}

/// `serve`'s instance-lock failure, kept typed so [`failure_exit_code`] can
/// still recognise it.
///
/// Flattening it with `GotchaError::message`, as every other boot failure is,
/// would print the same text and lose the one fact the exit code is for. The
/// `Other` variant is transparent, so the message a person sees is unchanged.
pub fn typed_instance_error(error: InstanceError) -> GotchaError {
    GotchaError::Other(Box::new(error))
}

/// The exit status a failed command should end with: [`INSTANCE_HELD_EXIT`]
/// when it was refused an instance another server holds, 1 for anything else.
pub fn failure_exit_code(error: &GotchaError) -> u8 {
    let held = match error {
        GotchaError::Other(inner) => matches!(inner.downcast_ref::<InstanceError>(), Some(InstanceError::Held { .. })),
        _ => false,
    };
    if held {
        INSTANCE_HELD_EXIT
    } else {
        1
    }
}

impl BinStore {
    /// Move the failing binary aside and say what will be tried instead.
    ///
    /// Note what this does *not* do: reset the backoff. A reset would make a
    /// machine where everything is broken cycle 2s, 4s, 2s, 4s forever — three
    /// failures, a step down, a reset, three more failures — hammering a binary
    /// that cannot run at all. Stepping down is not evidence that things are
    /// getting better, so the wait keeps growing.
    fn step_down(&self, baked: &Path) {
        let step = match self.demote() {
            Ok(step) => step,
            Err(error) => {
                say(&format!("cannot move the failing binary aside: {error}"));
                return;
            }
        };
        let moved = if step.quarantined {
            format!("moved the installed binary to {}", self.quarantine().display())
        } else {
            // Reached when the supervisor was already running the baked binary,
            // because nothing was ever installed. Saying "quarantined" here
            // would be a lie about a file that does not exist.
            "found nothing installed to move aside".to_string()
        };
        match step.fallback {
            Fallback::Previous => say(&format!(
                "{FAST_FAILURES} fast failures: {moved} and restored {}",
                self.installed().display()
            )),
            Fallback::Baked => say(&format!(
                "{FAST_FAILURES} fast failures: {moved}; there is no previous binary, so falling back to {}",
                baked.display()
            )),
        }
    }
}

/// Start the server, in a process group of its own.
///
/// The group matters for what it *prevents*: a `Ctrl-C` in a terminal goes to
/// the foreground process group, and without this the child would receive it
/// directly as well as through this loop's forwarding, turning one interrupt
/// into two.
fn spawn(exe: &Path, args: &[String]) -> io::Result<i32> {
    use std::os::unix::process::CommandExt;

    let child = std::process::Command::new(exe)
        .args(args)
        // How the server knows it may restart itself; see
        // [`crate::selfupdate::SUPERVISED_ENV`]. Under both names: after a
        // rollback the child may be a pre-rename server, which
        // reads only `RH_SUPERVISED`.
        .envs(crate::env::with_old_name(selfupdate::SUPERVISED_ENV).map(|name| (name, "1")))
        // And every setting this `up` was given under a new name, under the
        // old one too: such a child reads only `REMOTE_HAND_DATA_DIR`, and
        // without it would run on the default install's files, not this one's.
        .envs(crate::env::settings_under_old_names())
        .process_group(0)
        .spawn()?;
    let pid = child.id() as i32;
    // Deliberately not `wait`ed on through this handle: the loop reaps with
    // `waitpid(-1)`, which would race with `Child::wait` over the same status.
    // Dropping a `Child` does not reap in Rust, so this simply forgets a pid the
    // loop is already tracking.
    drop(child);
    Ok(pid)
}

/// Wait for `child`, reaping every other exited process while doing so.
fn wait_for(child: i32) -> Result<i32, String> {
    loop {
        if let Some(signal) = taken_signal() {
            say(&format!("forwarding signal {signal} to the server ({child}) and not to its group"));
            // SAFETY: a signal to one pid this loop started. Never `-child`:
            // that would reach the shims and apps the whole design keeps alive.
            unsafe {
                libc::kill(child, signal);
            }
        }

        loop {
            let mut status: libc::c_int = 0;
            // SAFETY: `status` is a live `c_int` for the whole call.
            let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
            if pid == child {
                return Ok(status);
            }
            if pid > 0 {
                // An orphan: a pty shim, an embedded app or a detached CLI whose
                // server has exited. Reaped and forgotten — this loop never
                // started it and has nothing to say about it.
                continue;
            }
            if pid == 0 {
                break;
            }
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EINTR) => continue,
                // No children at all, which after a successful spawn means the
                // status was collected somewhere this loop cannot see.
                Some(libc::ECHILD) => return Err(format!("lost track of the server process ({child})")),
                _ => return Err(format!("waitpid: {error}")),
            }
        }

        std::thread::sleep(POLL);
    }
}

/// Sleep, but wake early if a signal arrives, so a `docker stop` is not held up
/// by a thirty-second backoff.
fn sleep_unless_signalled(total: Duration) {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline {
        if SIGNALLED.load(Ordering::SeqCst) != 0 {
            return;
        }
        // Orphans keep arriving while the loop is between children, and pid 1
        // has to keep taking them.
        reap_orphans();
        std::thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

/// Collect any exited process, ignoring who it was.
fn reap_orphans() {
    loop {
        let mut status: libc::c_int = 0;
        // SAFETY: `status` is a live `c_int` for the whole call.
        let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
        if pid <= 0 {
            return;
        }
    }
}

/// Whether a stop was asked for.
fn stopping() -> bool {
    SIGNALLED.load(Ordering::SeqCst) != 0
}

/// Take the signal that arrived, leaving the stop flag set.
///
/// The number is consumed so it is forwarded once, but the flag itself stays —
/// "somebody asked us to stop" is not something that stops being true.
fn taken_signal() -> Option<libc::c_int> {
    let signal = SIGNALLED.load(Ordering::SeqCst);
    if signal <= 0 {
        return None;
    }
    // Consumed by replacing the number with a marker rather than with zero: the
    // signal is forwarded exactly once, and `stopping` keeps saying yes.
    SIGNALLED.store(-1, Ordering::SeqCst);
    Some(signal)
}

/// Record the signal for the loop to act on.
///
/// Async-signal-safe: one relaxed store into a static, and nothing else.
extern "C" fn on_signal(signal: libc::c_int) {
    SIGNALLED.store(signal, Ordering::SeqCst);
}

/// Catch the signals a stop arrives as.
fn install_handlers() -> Result<(), String> {
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGQUIT, libc::SIGHUP] {
        // SAFETY: a zeroed `sigaction` with a handler that only touches an
        // atomic, installed once before any child exists.
        let ok = unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as *const () as usize;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signal, &action, std::ptr::null_mut()) == 0
        };
        if !ok {
            return Err(format!("cannot handle signal {signal}: {}", io::Error::last_os_error()));
        }
    }
    Ok(())
}

/// The exit code of a `waitpid` status, with a signal death mapped the way a
/// shell maps it.
fn exit_code(status: libc::c_int) -> i32 {
    if libc::WIFEXITED(status) {
        return libc::WEXITSTATUS(status);
    }
    if libc::WIFSIGNALED(status) {
        return 128 + libc::WTERMSIG(status);
    }
    // Neither exited nor signalled is not a thing `waitpid` returns for a
    // reaped child, but a supervisor should not panic over it.
    1
}

/// Double the wait, up to the ceiling.
fn next_backoff(current: Duration) -> Duration {
    (current * 2).min(MAX_BACKOFF)
}

/// The data directory the store lives under, and the instance the server will
/// claim, from the same configuration the server itself will load.
fn load_paths() -> Result<Paths, String> {
    let config = crate::config::load(std::env::var("GOTCHA_ACTIVE_PROFILE").ok()).map_err(|error| format!("cannot load configuration: {error}"))?;
    Ok(Paths::for_instance(config.app.home(), &config.app.instance()))
}

/// Say something on stderr, prefixed so it is findable in `docker logs`.
///
/// Not `tracing`: this loop runs before — and outside — the server's logging
/// setup, and a supervisor whose output depends on a subscriber being installed
/// is a supervisor that goes quiet in exactly the situation it is for.
fn say(message: &str) {
    eprintln!("[rh-up] {message}");
}

/// The default command when `up` is given none.
pub fn default_child_args() -> Vec<String> {
    vec!["serve".to_string()]
}

/// The child command line from `up`'s own arguments.
///
/// `up -- serve --flag` and `up serve --flag` both work; the `--` is the
/// conventional way to say "everything after this belongs to the child" and is
/// accepted rather than required.
pub fn child_args(args: &[String]) -> Vec<String> {
    let rest = match args.first().map(String::as_str) {
        Some("--") => &args[1..],
        _ => args,
    };
    if rest.is_empty() {
        default_child_args()
    } else {
        rest.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_child_command_line_survives_an_optional_double_dash() {
        assert_eq!(child_args(&[]), vec!["serve".to_string()]);
        assert_eq!(child_args(&["--".to_string()]), vec!["serve".to_string()]);
        assert_eq!(
            child_args(&["--".to_string(), "serve".to_string(), "--x".to_string()]),
            vec!["serve".to_string(), "--x".to_string()]
        );
        assert_eq!(child_args(&["serve".to_string()]), vec!["serve".to_string()]);
    }

    #[test]
    fn the_backoff_doubles_to_a_ceiling_and_stops() {
        let mut backoff = MIN_BACKOFF;
        for _ in 0..10 {
            backoff = next_backoff(backoff);
        }
        assert_eq!(backoff, MAX_BACKOFF);
        assert_eq!(next_backoff(MIN_BACKOFF), MIN_BACKOFF * 2);
    }

    #[test]
    fn asking_whether_a_signal_arrived_is_not_the_same_as_one_arriving() {
        // The loop asks on every poll, and the answer has to be "no" until one
        // really does. Consuming the flag by swapping the marker in
        // unconditionally would make the very first poll look like a stop
        // request — the supervisor would exit after its first child instead of
        // restarting it, and self-upgrade would silently become a shutdown.
        assert_eq!(SIGNALLED.load(Ordering::SeqCst), 0, "no test before this one may have signalled");
        assert_eq!(taken_signal(), None);
        assert!(!stopping(), "asking must not be answering");

        SIGNALLED.store(libc::SIGTERM, Ordering::SeqCst);
        assert_eq!(taken_signal(), Some(libc::SIGTERM));
        assert!(stopping(), "the request outlives the number");
        assert_eq!(taken_signal(), None, "and is forwarded exactly once");

        SIGNALLED.store(0, Ordering::SeqCst);
    }

    const QUICK: Duration = Duration::from_millis(300);

    fn exited(code: i32, ran: Duration) -> Outcome {
        Outcome::Exited { code, ran }
    }

    #[test]
    fn a_held_instance_never_counts_against_the_binary() {
        // The supervisor was SIGKILLed, its `serve` lives on holding the lock,
        // and the service manager has started a fresh `up`. However long the
        // orphan stays, every refusal is a wait — never a fast failure, never a
        // step down — and the waits climb to the usual ceiling and stay there.
        let mut ladder = Ladder::new();
        let mut waits = Vec::new();
        for attempt in 0..20 {
            match ladder.after(exited(i32::from(INSTANCE_HELD_EXIT), QUICK)) {
                Next::AwaitHolder { wait } => waits.push(wait),
                other => panic!("attempt {attempt}: a held lock must be waited out, got {other:?}"),
            }
            assert_eq!(ladder.fast_failures, 0, "attempt {attempt}: nothing counted against the binary");
        }
        assert_eq!(&waits[..5], &[2, 4, 8, 16, 30].map(Duration::from_secs));
        assert!(waits.iter().all(|wait| *wait <= MAX_BACKOFF), "{waits:?}");

        // And when the holder finally exits, the next run takes over and is
        // judged on its own merits like any other.
        assert_eq!(ladder.after(exited(0, HEALTHY_AFTER * 10)), Next::Restart);
        assert_eq!(ladder, Ladder::new(), "a healthy run resets the whole ladder");
    }

    #[test]
    fn three_fast_failures_still_step_down() {
        let mut ladder = Ladder::new();
        let fail = |ladder: &mut Ladder| ladder.after(exited(1, QUICK));
        assert_eq!(
            fail(&mut ladder),
            Next::Retry {
                wait: Duration::from_secs(2),
                fast_failures: 1,
                step_down: false
            }
        );
        assert_eq!(
            fail(&mut ladder),
            Next::Retry {
                wait: Duration::from_secs(4),
                fast_failures: 2,
                step_down: false
            }
        );
        assert_eq!(
            fail(&mut ladder),
            Next::Retry {
                wait: Duration::from_secs(8),
                fast_failures: 3,
                step_down: true
            }
        );
        // The fallback binary starts with a clean count, but not a short wait:
        // stepping down is no evidence that anything got better.
        assert_eq!(
            fail(&mut ladder),
            Next::Retry {
                wait: Duration::from_secs(16),
                fast_failures: 1,
                step_down: false
            }
        );
    }

    #[test]
    fn a_held_lock_between_failures_neither_adds_to_them_nor_forgives_them() {
        let mut ladder = Ladder::new();
        ladder.after(exited(1, QUICK));
        ladder.after(exited(1, QUICK));
        for _ in 0..5 {
            assert!(matches!(
                ladder.after(exited(i32::from(INSTANCE_HELD_EXIT), QUICK)),
                Next::AwaitHolder { .. }
            ));
        }
        assert_eq!(ladder.fast_failures, 2);
        assert!(matches!(
            ladder.after(exited(1, QUICK)),
            Next::Retry {
                step_down: true,
                fast_failures: 3,
                ..
            }
        ));
    }

    #[test]
    fn a_binary_that_cannot_even_start_is_a_fast_failure() {
        let mut ladder = Ladder::new();
        assert!(matches!(ladder.after(Outcome::NotStarted), Next::Retry { step_down: false, .. }));
        assert!(matches!(ladder.after(Outcome::NotStarted), Next::Retry { step_down: false, .. }));
        assert!(matches!(ladder.after(Outcome::NotStarted), Next::Retry { step_down: true, .. }));
    }

    #[test]
    fn a_clean_exit_restarts_and_clears_the_count_and_a_long_run_clears_the_wait() {
        let mut ladder = Ladder::new();
        ladder.after(exited(1, QUICK));
        ladder.after(exited(1, QUICK));
        assert_eq!(ladder.after(exited(0, QUICK)), Next::Restart, "a self-upgrade restarts at once");
        assert_eq!(ladder.fast_failures, 0);
        assert_eq!(ladder.backoff, Duration::from_secs(8), "but a quick clean exit does not shorten the wait");

        // A crash after a healthy run is not a fast one, and starts the wait over.
        assert_eq!(
            ladder.after(exited(101, HEALTHY_AFTER)),
            Next::Retry {
                wait: MIN_BACKOFF,
                fast_failures: 0,
                step_down: false
            }
        );
        // A signal death counts like any other failure (128 + n is not 75).
        assert!(matches!(
            ladder.after(exited(128 + libc::SIGKILL, QUICK)),
            Next::Retry { fast_failures: 1, .. }
        ));
    }

    #[test]
    fn the_held_exit_code_collides_with_nothing_else_a_child_can_report() {
        let held = i32::from(INSTANCE_HELD_EXIT);
        assert_eq!(held, 75, "EX_TEMPFAIL, and part of a contract other builds rely on");
        assert_ne!(held, 0, "a clean exit means restart");
        assert_ne!(held, 1, "every other failure exits 1");
        assert!(held < 128, "a signal death is reported as 128 + n");
    }

    #[test]
    fn only_a_held_instance_exits_with_the_held_code() {
        let data = tempfile::tempdir().expect("tempdir");
        let dirs = crate::instance::InstanceDirs::new(data.path(), "default").expect("valid name");
        let _held = crate::instance::InstanceLock::acquire(&dirs).expect("first acquire");
        // `flock` is per open file description, so a second acquire in this
        // process is refused exactly as a second server would be.
        let refused = crate::instance::InstanceLock::acquire(&dirs).expect_err("a second acquire must be refused");
        let text = refused.to_string();

        let error = typed_instance_error(refused);
        assert_eq!(failure_exit_code(&error), INSTANCE_HELD_EXIT);
        assert_eq!(error.to_string(), text, "the message a person reads is unchanged");

        // Flattened into a message, the fact is gone — which is why `serve`
        // must keep it typed.
        assert_eq!(failure_exit_code(&GotchaError::message(&text)), 1);
        // Any other instance failure, typed or not, is an ordinary failure.
        let bad_name = crate::instance::InstanceDirs::new(data.path(), "../x").expect_err("bad name");
        assert_eq!(failure_exit_code(&typed_instance_error(bad_name)), 1);
        assert_eq!(failure_exit_code(&GotchaError::Io(io::Error::other("boom"))), 1);
    }

    #[test]
    fn a_status_becomes_the_code_a_shell_would_report() {
        // Built the way `waitpid` reports them, so the accessors are exercised
        // rather than reimplemented.
        let exited = 3 << 8;
        assert_eq!(exit_code(exited), 3);
        assert_eq!(exit_code(0), 0);
        let killed = libc::SIGKILL;
        assert_eq!(exit_code(killed), 128 + libc::SIGKILL);
    }
}
