//! What a pid means once the process that spawned it is gone.
//!
//! Everything remote-hand adopts after a restart — an injected CLI run, an
//! embedded app, a pty shim — is remembered as a number in a file. A number in a
//! file is not a process. Between the write and the next boot the machine may
//! have rebooted, the child may have crashed, and — the case this module exists
//! for — the kernel may have handed that number to something else entirely. Pids
//! wrap: on a busy macOS box the counter is 15 bits by default, so "the pid we
//! wrote yesterday" and "somebody's `make`" are the same integer far more often
//! than the arithmetic suggests.
//!
//! `kill(pid, 0)` answers "some process has this pid", which is not the question
//! adoption asks. Adopting a stranger would mean reporting somebody else's build
//! as a live agent run, blocking the composer on it, and eventually `SIGTERM`ing
//! it when the idle watchdog gave up. So there are two stronger guards here, and
//! which one a caller wants depends on what its record remembers.
//!
//! # [`is_same_process`] — identity, when a token was recorded
//!
//! The exact guard. A record stores *two* things: the pid, and a token derived
//! from the process's start time — the one property a recycled pid cannot share,
//! because two processes with the same pid necessarily started at different
//! moments. Both must match.
//!
//! The token is deliberately opaque and platform-specific:
//!
//! * **macOS** — `proc_pidinfo(PROC_PIDTBSDINFO)`, whose `pbi_start_tvsec` /
//!   `pbi_start_tvusec` are the process's start time. A documented `libproc`
//!   call, not a guess at `kinfo_proc`'s layout.
//! * **Linux** — field 22 of `/proc/<pid>/stat`, the start time in clock ticks
//!   since boot.
//! * **anything else** — `None`, and the guard degrades to bare liveness. A
//!   token that could not be read is never treated as a mismatch, in either
//!   direction: refusing to adopt because a platform has no clock to check would
//!   throw away a live run on every restart, which is worse than the narrow
//!   reuse window it protects against.
//!
//! The tokens of two different boots are not comparable, and nothing tries to: a
//! token is only ever compared against one taken for the same pid.
//!
//! # [`is_running`] — recognition, when only a program name was recorded
//!
//! The guard in front of every *signal* aimed at a process this server did not
//! spawn. Adoption can afford to be wrong (it fails to adopt, and an app is left
//! running); killing cannot. So this one fails closed in both directions: a dead
//! pid is not running anything, and a live pid `ps` will not describe is assumed
//! to belong to somebody else. It loses an app and never a stranger.
//!
//! Neither guard is needed for a process the *running* server started: that one
//! is still its child, watched by a supervisor that stores the moment it exits,
//! and a pid cannot be recycled while its parent has yet to reap it.

use std::time::Duration;

/// How long a process group is given to stop before it is made to.
const KILL_GRACE: Duration = Duration::from_secs(3);

/// How long `ps` is given to answer before its answer is given up on.
const PS_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether a process is still there.
///
/// `kill(pid, 0)` performs the permission and existence checks and delivers
/// nothing, which is exactly the question a corpse guard asks. `EPERM` counts as
/// alive: the process exists, it simply is not ours — which is the case for a
/// shim adopted across a user switch, and refusing to attach then would be a
/// false negative.
///
/// Pids 0 and 1 are refused: `kill(0, …)` addresses this process's own group
/// and `kill(1, …)` init, and neither is ever a child this server is tracking.
pub fn alive(pid: u32) -> bool {
    if pid <= 1 {
        return false;
    }
    // SAFETY: signal 0 is the existence probe; it delivers nothing and touches
    // no memory.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Whether `pid` is alive *and* is still the process `token` was taken from.
///
/// `None` for `token` means the record predates the guard or the platform has
/// none; liveness alone then decides.
pub fn is_same_process(pid: u32, token: Option<&str>) -> bool {
    if !alive(pid) {
        return false;
    }
    match (token, start_token(pid)) {
        (Some(recorded), Some(current)) => recorded == current,
        // Either side missing is "cannot tell", which reads as the same process:
        // see the module docs for why that direction is the safe one.
        _ => true,
    }
}

/// An opaque token for when `pid` started, or `None` if it cannot be read.
#[cfg(target_os = "macos")]
pub fn start_token(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `info` is a live, correctly sized `proc_bsdinfo` this call fills
    // in; `proc_pidinfo` writes at most `size` bytes into it and touches
    // nothing else.
    let read = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut libc::proc_bsdinfo as *mut libc::c_void,
            size,
        )
    };
    // A short read means the kernel did not fill the struct — a process that
    // exited between the liveness probe and here, or one this uid may not
    // inspect. Either way there is no start time to compare.
    if read != size {
        return None;
    }
    Some(format!("{}.{:06}", info.pbi_start_tvsec, info.pbi_start_tvusec))
}

/// An opaque token for when `pid` started, or `None` if it cannot be read.
#[cfg(target_os = "linux")]
pub fn start_token(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Field 2 is the executable name in parentheses and may itself contain
    // spaces and parentheses, so the split has to start after the *last* `)`
    // rather than at the first space.
    let after_comm = raw.rsplit_once(')')?.1;
    // What follows `)` begins at field 3, so field 22 is the 20th token.
    let starttime = after_comm.split_whitespace().nth(19)?;
    Some(starttime.to_string())
}

/// An opaque token for when `pid` started, or `None` if it cannot be read.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn start_token(_pid: u32) -> Option<String> {
    None
}

/// The full command line of `pid`, as `ps(1)` reports it, or `None`.
///
/// `-o command=` rather than `-o comm=` deliberately. `code-server` is a node
/// script: the *executable* is `node`, and on Linux `comm` is additionally
/// truncated to 15 characters. The only place the name that identifies the
/// process survives is the argument vector.
pub async fn command_line(pid: u32) -> Option<String> {
    let output = tokio::process::Command::new("/bin/ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    // A `ps` that never returns must not hold a boot up; adoption runs before
    // the listener exists.
    let output = tokio::time::timeout(PS_TIMEOUT, output).await.ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Whether `pid` is still running something whose command line names `program`.
///
/// Fails closed in both directions — see the module docs.
pub async fn is_running(pid: u32, program: &str) -> bool {
    if !alive(pid) {
        return false;
    }
    match command_line(pid).await {
        Some(line) => line.contains(program),
        None => {
            tracing::warn!(pid, "cannot read the command line of an adopted pid; treating it as not ours");
            false
        }
    }
}

/// Signal a whole process group.
///
/// An app is spawned into a group of its own (`process_group(0)`) precisely so
/// this reaches every node process `code-server` started, and not one of ours.
///
/// The `<= 1` guard is the lesson the pty shim's own group signal carries, in
/// numeric form: a pid that was never learned is `0`, and `kill(-0, …)` means
/// "my own process group" — the server's — while `kill(-1, …)` means "every
/// process I may signal". Both are the accident, not the intent. It is a
/// `libc::kill` rather than a `kill(1)` for the same reason: a computed pid put
/// into a string that another program re-parses is how a missing `--` becomes an
/// outage.
pub fn signal_group(pid: u32, signal: libc::c_int) {
    let group = pid as libc::pid_t;
    if group <= 1 {
        tracing::warn!(pid, "refusing to signal an implausible process group");
        return;
    }
    // SAFETY: a signal to a process group id derived from a pid this server
    // spawned, guarded above against the two wildcard values.
    unsafe {
        libc::kill(-group, signal);
    }
}

/// Ask a process group to stop, and insist if it does not.
///
/// `code-server` handles `SIGTERM` and takes a moment over it — it has a
/// workspace state database to close — so the polite signal is the one that
/// matters and the escalation is the backstop. Requires a runtime: the
/// escalation is a timer, not a sleep on the caller's thread.
pub fn terminate_group(pid: u32) {
    signal_group(pid, libc::SIGTERM);
    tokio::spawn(async move {
        tokio::time::sleep(KILL_GRACE).await;
        if alive(pid) {
            tracing::info!(pid, "an app ignored SIGTERM; killing its process group");
            signal_group(pid, libc::SIGKILL);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_is_identified_by_its_pid_and_when_it_started() {
        let me = std::process::id();
        let token = start_token(me);
        assert!(token.is_some(), "this platform is macOS or Linux, so a start time must be readable");
        assert_eq!(token, start_token(me), "the token of one process never changes");
        assert!(is_same_process(me, token.as_deref()));

        // The guard's whole point: same pid, a token from some other moment.
        assert!(!is_same_process(me, Some("0.000000")), "a recycled pid is not the same process");

        // A pid that cannot exist is not alive, whatever token is offered.
        assert!(!is_same_process(u32::MAX - 1, None));
        assert!(!is_same_process(u32::MAX - 1, token.as_deref()));

        // A record written before the guard existed still adopts on liveness.
        assert!(is_same_process(me, None));
    }

    #[test]
    fn a_dead_child_is_neither_alive_nor_identifiable() {
        let child = std::process::Command::new("/bin/sh").args(["-c", "exit 0"]).spawn().expect("spawn");
        let pid = child.id();
        let mut child = child;
        child.wait().expect("reap");
        assert!(!alive(pid), "a reaped child is gone");
        assert!(!is_same_process(pid, None));
    }

    #[tokio::test]
    async fn a_process_is_recognised_by_its_arguments_and_not_its_executable() {
        let mine = std::process::id();
        assert!(alive(mine), "this process is alive");

        let line = command_line(mine).await.expect("ps describes this process");
        assert!(!line.is_empty());
        // The test binary's own command line is the one thing that is certainly
        // in it, and it is an *argument*, not the interpreter's name.
        let exe = std::env::current_exe().expect("current exe");
        let stem = exe.file_name().expect("file name").to_string_lossy().into_owned();
        assert!(line.contains(&stem), "{line} should name {stem}");

        assert!(is_running(mine, &stem).await, "the guard lets through what it recognises");
        assert!(!is_running(mine, "definitely-not-this-program").await, "and refuses what it does not");
    }

    #[tokio::test]
    async fn a_pid_that_is_not_there_is_never_running_anything() {
        // Above every platform's `pid_max`, so no test can be unlucky enough to
        // have been handed it.
        let absent = u32::MAX - 1;
        assert!(!alive(absent));
        assert!(command_line(absent).await.is_none(), "ps has nothing to say about it");
        assert!(!is_running(absent, "code-server").await);
    }

    #[test]
    fn liveness_is_a_signal_probe_and_never_a_wildcard() {
        assert!(alive(std::process::id()), "this process is alive");
        assert!(!alive(0), "pid 0 is `my own process group`, not a process");
        assert!(!alive(1), "pid 1 is never a shell of ours");
        // A pid that cannot exist: the reserved maximum on every platform this
        // runs on is well below it.
        assert!(!alive(u32::MAX - 1));
    }

    #[test]
    fn the_wildcard_process_groups_are_never_signalled() {
        // `kill(-0, SIGKILL)` would take down this test binary, and `kill(-1, …)`
        // every process the user owns. Reaching either is the bug; that this
        // test returns at all is the assertion.
        for wildcard in [0, 1] {
            signal_group(wildcard, libc::SIGKILL);
        }
    }
}
