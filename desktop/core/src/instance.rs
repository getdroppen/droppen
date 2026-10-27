//! The server instance the app runs, and how the app makes sure it is its own.
//!
//! The app's server is an ordinary instance — `droppen-app`, in the binary's
//! normal data directory, `~/.local/share/remote-hand/instances/droppen-app` —
//! so the device's other Droppen installs share its machine-wide files exactly
//! as they share each other's. Three rules keep the app from touching what is
//! not its own:
//!
//! * **Ownership.** The app creates the directory itself, with a marker file
//!   ([`MARKER`]) in it, before anything else writes there. An existing
//!   directory without the marker belongs to someone else (a hand-made
//!   instance of the same name) and the app refuses to run any `user` command
//!   against it ([`ownership`]).
//! * **The lock.** A running server holds an exclusive `flock` on the
//!   instance's `lock` file and writes its pid into it. Before the app touches
//!   the instance it checks nobody holds it ([`lock_state`]).
//! * **The listener.** Before the app sends its password to the port, it checks
//!   the server answering there is its own child: the instance's lock names the
//!   child's pid, and the instance's `server.port` — which only the lock's
//!   holder writes, right after binding — names the port ([`serves_on`]).

use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The marker the app leaves in the instance directory it created.
pub const MARKER: &str = "droppen-app.json";

/// `<home>/.local/share/remote-hand/instances/<instance>`: the directory the
/// server resolves for this instance when nothing overrides its data directory
/// (the app removes every override from the server's environment).
pub fn dir_under(home: &Path, instance: &str) -> PathBuf {
    home.join(".local").join("share").join("remote-hand").join("instances").join(instance)
}

/// What the marker records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Marker {
    /// Always [`crate::BUNDLE_ID`].
    pub created_by: String,
    /// When, ISO-8601.
    pub created_at: String,
    /// Whether the accounts the new instance inherited from the device have
    /// been removed. Until it says so, every start tries again: a first start
    /// cut short between creating the folder and pruning must not leave
    /// someone else's login on the app's server for good.
    #[serde(default)]
    pub pruned: bool,
}

/// Whose the instance directory is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ownership {
    /// There is no such directory yet.
    New,
    /// The app made it.
    Ours,
    /// Something else made it; the app keeps its hands off.
    NotOurs,
}

/// Whose `dir` is.
pub fn ownership(dir: &Path) -> Ownership {
    if !dir.exists() {
        return Ownership::New;
    }
    match marker(dir) {
        Some(marker) if marker.created_by == crate::BUNDLE_ID => Ownership::Ours,
        _ => Ownership::NotOurs,
    }
}

/// The marker in `dir`, if there is a readable one.
pub fn marker(dir: &Path) -> Option<Marker> {
    std::fs::read(dir.join(MARKER)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn write_marker(dir: &Path, marker: &Marker) -> io::Result<()> {
    let body = serde_json::to_vec_pretty(marker).map_err(io::Error::other)?;
    crate::state::write_atomically(&dir.join(MARKER), &body, 0o644)
}

/// Whether the inherited accounts still have to be removed.
pub fn needs_pruning(dir: &Path) -> bool {
    !marker(dir).is_some_and(|marker| marker.pruned)
}

/// Record in the marker that the inherited accounts are gone.
pub fn mark_pruned(dir: &Path) -> io::Result<()> {
    let mut marker = marker(dir).ok_or_else(|| io::Error::other("the instance has no marker"))?;
    marker.pruned = true;
    write_marker(dir, &marker)
}

/// Create `dir` (which must not exist yet) with the app's marker in it.
pub fn claim_new(dir: &Path) -> io::Result<()> {
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir(dir)?;
    let marker = Marker {
        created_by: crate::BUNDLE_ID.to_string(),
        created_at: crate::logging::timestamp(std::time::SystemTime::now()),
        pruned: false,
    };
    write_marker(dir, &marker)
}

/// Whether a server holds the instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockState {
    /// Nobody does (or there is no lock file yet).
    Free,
    /// Somebody does; the pid it wrote, if it wrote one.
    Held(Option<u32>),
}

/// Ask the kernel whether the instance's lock is held, without keeping it: an
/// exclusive non-blocking `flock`, released at once. Never creates the file.
pub fn lock_state(dir: &Path) -> io::Result<LockState> {
    let path = dir.join("lock");
    let file = match std::fs::OpenOptions::new().read(true).write(true).open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(LockState::Free),
        Err(error) => return Err(error),
    };
    // SAFETY: flock on a descriptor this function owns for the whole call.
    let taken = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    if taken {
        // SAFETY: as above; closing the file would release it too.
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        return Ok(LockState::Free);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(LockState::Held(recorded_holder(dir)))
    } else {
        Err(error)
    }
}

/// The pid the lock's holder wrote into it — advisory, like the server's own
/// reading of it: the `flock` is the truth, the pid a hint.
pub fn recorded_holder(dir: &Path) -> Option<u32> {
    std::fs::read_to_string(dir.join("lock")).ok()?.trim().parse().ok()
}

/// The port the instance's owner last wrote into `server.port`.
pub fn recorded_port(dir: &Path) -> Option<u16> {
    std::fs::read_to_string(dir.join("server.port")).ok()?.trim().parse().ok()
}

/// Forget the recorded port before a start, so a value left by an earlier
/// server cannot vouch for whatever answers next.
pub fn forget_port(dir: &Path) {
    let _ = std::fs::remove_file(dir.join("server.port"));
}

/// Whether the server answering on 127.0.0.1:`port` is `pid`: `pid` holds the
/// instance's lock (and wrote it there) and wrote `port` as the port it bound.
///
/// Sound because only one socket can be bound to 127.0.0.1:`port`, the server
/// writes `server.port` only after its own bind succeeded and only while it
/// holds the lock, and [`forget_port`] runs before every start. The lock is
/// checked with the kernel, not just read: a stale pid in the file is not
/// enough.
pub fn serves_on(dir: &Path, pid: u32, port: u16) -> bool {
    matches!(lock_state(dir), Ok(LockState::Held(Some(holder))) if holder == pid) && recorded_port(dir) == Some(port)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hold `dir/lock` the way a server does: flock it and write a pid.
    fn hold(dir: &Path, pid: u32) -> std::fs::File {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(dir.join("lock"))
            .expect("lock file");
        // SAFETY: flock on a descriptor the test owns.
        assert_eq!(unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0);
        std::fs::write(dir.join("lock"), format!("{pid}\n")).expect("pid");
        file
    }

    #[test]
    fn the_instance_lives_where_the_server_puts_it() {
        assert_eq!(
            dir_under(Path::new("/Users/someone"), "droppen-app"),
            PathBuf::from("/Users/someone/.local/share/remote-hand/instances/droppen-app")
        );
    }

    #[test]
    fn a_new_directory_is_claimed_and_then_ours() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = dir_under(root.path(), "droppen-app");
        assert_eq!(ownership(&dir), Ownership::New);
        claim_new(&dir).expect("claim");
        assert_eq!(ownership(&dir), Ownership::Ours);
        assert!(claim_new(&dir).is_err(), "never claims a directory that exists");
    }

    #[test]
    fn pruning_is_owed_until_it_is_recorded() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = dir_under(root.path(), "droppen-app");
        claim_new(&dir).expect("claim");
        assert!(needs_pruning(&dir), "a new instance still has its inherited accounts");
        // A start cut short here leaves the debt in place for the next one.
        assert!(needs_pruning(&dir));
        mark_pruned(&dir).expect("mark");
        assert!(!needs_pruning(&dir));
        assert_eq!(ownership(&dir), Ownership::Ours, "still the app's");
        // A marker written before this field existed reads as not yet pruned.
        std::fs::write(dir.join(MARKER), format!(r#"{{"createdBy":"{}","createdAt":"x"}}"#, crate::BUNDLE_ID)).expect("old marker");
        assert!(needs_pruning(&dir));
    }

    #[test]
    fn a_directory_without_the_marker_is_not_ours() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("instances/droppen-app");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("users.json"), "{}").expect("someone else's file");
        assert_eq!(ownership(&dir), Ownership::NotOurs);
        std::fs::write(dir.join(MARKER), r#"{"createdBy":"com.example.other","createdAt":"x"}"#).expect("marker");
        assert_eq!(ownership(&dir), Ownership::NotOurs, "a marker by someone else is not ours either");
    }

    #[test]
    fn the_lock_is_asked_of_the_kernel_not_read_off_the_file() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path();
        assert_eq!(lock_state(dir).expect("no file"), LockState::Free);
        assert!(!dir.join("lock").exists(), "asking never creates the lock file");
        // A pid left in the file by a server that has gone is not a holder.
        std::fs::write(dir.join("lock"), "4242\n").expect("stale");
        assert_eq!(lock_state(dir).expect("stale"), LockState::Free);
        let held = hold(dir, 4242);
        assert_eq!(lock_state(dir).expect("held"), LockState::Held(Some(4242)));
        drop(held);
        assert_eq!(lock_state(dir).expect("released"), LockState::Free);
    }

    #[test]
    fn the_listener_is_vouched_for_only_by_the_lock_holder_and_its_port() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path();
        let _held = hold(dir, 4242);
        std::fs::write(dir.join("server.port"), "51234\n").expect("port");
        assert!(serves_on(dir, 4242, 51_234));
        assert!(!serves_on(dir, 4243, 51_234), "another pid");
        assert!(!serves_on(dir, 4242, 51_235), "another port");
        forget_port(dir);
        assert!(!serves_on(dir, 4242, 51_234), "no port written since the start");
    }

    #[test]
    fn a_released_lock_vouches_for_nobody() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path();
        drop(hold(dir, 4242));
        std::fs::write(dir.join("server.port"), "51234\n").expect("port");
        assert!(!serves_on(dir, 4242, 51_234));
    }
}
