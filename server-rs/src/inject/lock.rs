//! One CLI per session, across every server on the machine.
//!
//! The in-process map in [`super::InjectionRegistry`] has always refused a
//! second send while one is running. It can only speak for its own process,
//! and there are two ordinary ways to have more than one:
//!
//! * a restart — the old server's map died with it, and the new one would
//!   happily spawn a second CLI against a session the first one's CLI is still
//!   writing to;
//! * two instances — `APP_INSTANCE=b` exists precisely so a second server can
//!   coexist, and [`crate::instance`] gives it a disjoint directory. Disjoint
//!   directories are the right answer for shims and sockets, and the wrong one
//!   here: the sessions themselves are *shared*, because they live in the
//!   user's `~/.claude` and `~/.codex`, and two agents appending to one
//!   transcript corrupt each other's turn.
//!
//! So the guarantee is moved out of memory and onto the filesystem, in the
//! **shared** data directory rather than any instance's:
//!
//! ```text
//! <data>/inject-locks/<source>--<session>.lock
//! ```
//!
//! `flock(2)` is what makes this survive the failure it exists for. The lock is
//! a property of the open file description, so it is released by the kernel
//! when the holder's descriptor closes — including when the holder is
//! `SIGKILL`ed, or its machine loses power. There is no stale lock to detect
//! and no owner-liveness check to get wrong: a dead server's claim is simply
//! gone, which is exactly what lets the next boot re-take it during adoption.
//!
//! The holder writes its pid into the file so a refusal can name it. That is
//! advisory decoration only — the lock is the truth, and the pid may be absent
//! if the reader arrives between the `flock` and the write.
//!
//! # How long a claim is held
//!
//! For codex, one `codex exec` run. For claude, the whole life of the resident
//! process — which is the same rule, not a looser one: the thing being excluded
//! is a second CLI writing the transcript, and a resident is writing it from the
//! moment it starts until it exits, whether or not a turn is in flight. A claim
//! released between turns would let a second instance start its own resident
//! against the same session, which is exactly the interleaving this prevents.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use crate::sessions::reference::lock_name;
use crate::sessions::Source;

/// The result of trying to claim a session.
#[derive(Debug)]
pub enum Outcome {
    /// The claim succeeded; hold the guard for as long as the run lasts.
    Acquired(InjectLock),
    /// Somebody else is running against this session.
    Busy {
        /// The pid the holder recorded, when one could be read.
        holder: Option<u32>,
    },
}

/// A held claim on one session. Dropping it releases the claim.
#[derive(Debug)]
pub struct InjectLock {
    /// Kept open; closing it is what releases the `flock`. Never read: it is
    /// held for its side effect, exactly as `crate::instance::InstanceLock`
    /// holds its own.
    _file: File,
    path: PathBuf,
    /// What the path resolved to when the lock was taken, so cleanup can prove
    /// it is removing its own file and not a successor's.
    identity: Option<(u64, u64, i64, i64)>,
}

impl Drop for InjectLock {
    /// Release the claim, and take the file with it if it is still ours.
    ///
    /// Unlinking keeps `inject-locks/` from accumulating one file per session
    /// ever driven. The guard is the same one
    /// `crate::pty_shim::child::unlink_if_ours` applies to sockets: compare the
    /// path's identity — device, inode and the ctime that pins it to one
    /// creation — against what was locked, and leave anything else alone.
    /// Without it, a slow drop could delete the file a *successor* has already
    /// created and locked, and the session would lose its exclusion silently.
    ///
    /// The unlink deliberately happens before the descriptor closes. A process
    /// that opens the path in the window between the two creates a fresh file
    /// and claims it — which is correct, because this run is over.
    fn drop(&mut self) {
        let Some(identity) = self.identity else { return };
        if crate::pty_shim::child::path_identity(&self.path) == Some(identity) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Claim `session_id` for the duration of a run, or report who has it.
///
/// The error case is a real I/O failure — an unwritable data directory —
/// rather than contention, which is [`Outcome::Busy`].
pub fn acquire(dir: &Path, source: Source, session_id: &str) -> std::io::Result<Outcome> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(lock_name(source.as_str(), session_id));
    let mut file = File::options().read(true).write(true).create(true).truncate(false).open(&path)?;

    // SAFETY: `file` is an open descriptor this process owns for the whole
    // call, and `LOCK_EX | LOCK_NB` takes no further arguments.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        // Anything but contention is a genuine failure, and reporting it as
        // "a message is already running" would send the user hunting for a run
        // that does not exist.
        if !matches!(error.raw_os_error(), Some(libc::EWOULDBLOCK) | Some(libc::EINTR)) {
            return Err(error);
        }
        return Ok(Outcome::Busy {
            holder: read_holder(&mut file),
        });
    }

    let _ = file.set_len(0);
    let _ = file.seek(SeekFrom::Start(0));
    let _ = writeln!(file, "{}", std::process::id());
    let _ = file.flush();

    Ok(Outcome::Acquired(InjectLock {
        identity: crate::pty_shim::child::path_identity(&path),
        _file: file,
        path,
    }))
}

// The lock filename lives in `crate::sessions::reference::lock_name`, with
// the rest of the session-reference spellings. It is the one of the four that
// must *not* carry the server: this directory is shared by every instance on
// the machine so that a second one is refused a session the first is driving,
// and a server-scoped name would hand each of them their own file.

/// The pid the holder wrote, if it wrote one yet.
fn read_holder(file: &mut File) -> Option<u32> {
    let mut text = String::new();
    file.seek(SeekFrom::Start(0)).ok()?;
    file.read_to_string(&mut text).ok()?;
    text.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::reference::{stem, MAX_STEM};

    #[test]
    fn one_session_admits_one_holder_and_names_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let id = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa";

        let Outcome::Acquired(held) = acquire(dir.path(), Source::Claude, id).expect("acquire") else {
            panic!("a free session must be claimable");
        };
        assert!(held.path.exists());

        // `flock` is per open file description, so a second `open` in this very
        // process contends exactly as another process would — which is what
        // makes the cross-instance guarantee testable without a second binary.
        let second = acquire(dir.path(), Source::Claude, id).expect("acquire");
        match second {
            Outcome::Busy { holder } => assert_eq!(holder, Some(std::process::id()), "the refusal names the holder"),
            Outcome::Acquired(_) => panic!("a claimed session must be refused"),
        }

        // A different session, and the same id under the other CLI, are
        // different locks: the key is the pair, not the id.
        let _other_session = acquire(dir.path(), Source::Claude, "bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb").expect("acquire");
        let _other_source = acquire(dir.path(), Source::Codex, id).expect("acquire");

        let path = held.path.clone();
        drop(held);
        assert!(!path.exists(), "releasing takes the lock file with it");
        let Outcome::Acquired(_again) = acquire(dir.path(), Source::Claude, id).expect("acquire") else {
            panic!("a released session is claimable again");
        };
    }

    #[test]
    fn a_replaced_lock_file_is_left_for_its_own_holder_to_remove() {
        let dir = tempfile::tempdir().expect("tempdir");
        let id = "cccccccc-3333-4333-8333-cccccccccccc";
        let Outcome::Acquired(held) = acquire(dir.path(), Source::Claude, id).expect("acquire") else {
            panic!("claimable");
        };
        let path = held.path.clone();

        // Stand in for the successor: the path now names a different inode.
        std::fs::remove_file(&path).expect("unlink");
        std::fs::write(&path, b"9999\n").expect("successor");

        drop(held);
        assert!(path.exists(), "a lock file that is no longer ours must survive our release");
    }

    #[test]
    fn an_id_that_is_not_a_filename_becomes_a_digest_that_still_separates_sessions() {
        assert_eq!(stem("aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa"), "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa");
        assert_eq!(stem("new-1234"), "new-1234");

        for hostile in ["../../etc/passwd", "with/slash", "with space", "", &"x".repeat(MAX_STEM + 1)] {
            let stem = stem(hostile);
            assert_eq!(stem.len(), 32, "{hostile:?} must become a digest");
            assert!(stem.bytes().all(|b| b.is_ascii_hexdigit()), "{hostile:?} -> {stem}");
        }
        assert_ne!(stem("../a"), stem("../b"), "two ids must never share one lock");
    }

    #[test]
    fn the_separator_and_not_the_spelling_of_the_sources_keeps_two_pairs_apart() {
        // What a single dash did: it is part of a session id's own alphabet, so
        // the name cannot say which side of it the source ended on.
        assert_eq!(
            format!("claude-code-{}.lock", stem("x")),
            format!("claude-{}.lock", stem("code-x")),
            "one dash makes these two pairs the same file"
        );
        assert_ne!(
            lock_name("claude-code", "x"),
            lock_name("claude", "code-x"),
            "two sessions must never share one lock, whatever the sources are called"
        );

        // The pairs that exist today, which is what the acquire path composes.
        assert_eq!(lock_name(Source::Claude.as_str(), "new-1234"), "claude--new-1234.lock");
        assert_ne!(lock_name(Source::Claude.as_str(), "a"), lock_name(Source::Codex.as_str(), "a"));
    }

    #[test]
    fn a_hostile_id_cannot_reach_outside_the_lock_directory() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("inject-locks");
        let Outcome::Acquired(held) = acquire(&dir, Source::Claude, "../../escape").expect("acquire") else {
            panic!("claimable");
        };
        assert_eq!(held.path.parent(), Some(dir.as_path()), "the lock stays in its directory");
    }
}
