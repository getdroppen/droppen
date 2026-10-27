//! Instance ownership: the directory one server owns, and the lock that says so.
//!
//! Everything remote-hand can *adopt* after a restart — the pty shims of
//! [`crate::terminals`] today, injected runs and embedded apps in the phases
//! that follow — is a resource that survives the process that made it. Two
//! servers started on one machine would therefore both find it and both believe
//! it is theirs: they would adopt the same shim socket, race on the same cursor
//! file and kill each other's shells.
//!
//! The fix is ownership rather than coordination. Every adoptable thing lives
//! under `<data>/instances/<name>/`, and a server holds an exclusive `flock` on
//! `<data>/instances/<name>/lock` for its whole life. A second server on the
//! same instance name refuses to start and says whose pid holds it; a second
//! server that wants to coexist is given a different name (`APP_INSTANCE`) and a
//! disjoint directory, so there is nothing left to race over.
//!
//! Shared, non-adoptable state deliberately stays where it is: `accounts.json`,
//! UI preferences, session titles, the seen and archived lists, uploads and
//! `notify.json` are files two instances may legitimately both read. They are
//! keyed by globally unique session ids or hold settings the user meant
//! everywhere, so one copy is the right number. `notify.json` is the clearest
//! case of the latter: it names the phone the user wants notifications on, and
//! that is the same phone whichever instance an agent happens to be running
//! under (see [`crate::notify::store`]).
//!
//! Two shared directories go further than "may both read": they exist *to* be
//! read by the server that did not write them, and making either per-instance
//! would cut the only channel a constraint travels down.
//!
//! * `inject-locks` — one instance's claim on a running session, which is what
//!   stops another spawning a second CLI against it ([`crate::inject::lock`]).
//! * `codex-claims` — which instance made which codex thread
//!   ([`crate::sessions::codex_claims`]). Every other kind of ownership can be a
//!   directory; this one cannot, because `~/.codex` is one directory for the
//!   whole machine, so a sibling can only tell A's thread from the user's own if
//!   A says so somewhere the sibling reads.
//!
//! `session-bin` is a third instance-private directory, created on demand by
//! [`crate::notify::inject`] rather than by [`InstanceDirs::create`]: it holds
//! the `rh-notify` a spawned CLI finds on its `PATH`, and that script names
//! *this* server's binary and is reached with tokens minted for *this*
//! instance.
//!
//! Three sidecars are the exception in the other direction, for the same reason
//! the shims are: what they hold only means something to the server that wrote
//! it.
//!
//! * `screens.json` — the layouts an older build saved. Their leaves name
//!   terminal and resident session ids that are instance-private, so a shared
//!   file showed each instance the other's screens full of dead panes, and the
//!   last writer erased both. Layouts are rows in the instance database now, and
//!   the file is not read at all.
//! * `push.json` — the VAPID keypair and the browser subscriptions granted to
//!   *one origin*. Sharing it has the second instance signing with the first
//!   one's identity and delivering its notifications to the first one's PWA.
//! * `scheduled-tasks.json` — the cron tasks this server *acts on*
//!   ([`crate::schedule`]). The sharpest of the three: the other two would merely
//!   confuse, while a shared schedule is read by every server on the machine and
//!   fired by every one of them, so one nine-o'clock task becomes one session per
//!   instance.
//!
//! All three therefore live under `<data>/instances/<name>/`. The upgrade floor
//! is 0.2.125, whose first boot already copied the shared `screens.json` and
//! `push.json` into its instance, so the copy hop is gone: an install older than
//! that does not inherit the shared files.
//!
//! `users.json` — the local accounts — joined them, later and for a different
//! reason. The three above are per-instance because a shared one would be
//! *wrong*: it names resources the other server does not have. The accounts
//! were shared quite correctly right up until an instance was handed to
//! somebody else, at which point one `user add` put both servers behind one
//! person's password. That is the boundary this module exists to draw, and
//! [`crate::auth::users`] draws it: the reasoning, and why the machine-wide set
//! an older build kept is left on disk rather than moved or deleted, are all in
//! its module note.
//!
//! # Orphan reaping
//!
//! A pty shim is spawned detached, in its own process group, precisely so it
//! outlives the server. When the server exits, the shim is reparented to the
//! init process — and it is that process which must `wait()` for it once it
//! finally exits, or the machine accumulates zombies.
//!
//! * On a normal Linux or macOS desktop the reaper is pid 1 (systemd, launchd)
//!   and there is nothing to do.
//! * **In a container the entrypoint can be this server's own `up`**, which is
//!   pid 1 and waits on every child it did not start ([`crate::supervisor`]).
//!   A plain shell entrypoint is not a reaper: run `up`, or give the container an
//!   init (`docker run --init`, `tini`, `dumb-init`) or a `wait` loop. Without
//!   that, every restarted terminal leaves a zombie.
//! * On macOS the launchd analogue is `AbandonProcessGroup` in a job's plist:
//!   without it launchd kills the whole process group when the job stops, which
//!   would defeat the shim on exactly the restart it exists for. A shim spawned
//!   with `process_group(0)` leaves the job's group, which is what makes the
//!   default behaviour survivable.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

/// The instance a server owns when nothing says otherwise.
pub const DEFAULT_INSTANCE: &str = "default";

/// The longest an instance name may be.
const MAX_NAME: usize = 32;

/// Why an instance could not be opened or claimed.
#[derive(Debug, thiserror::Error)]
pub enum InstanceError {
    /// The name is not `[a-z0-9-]{1,32}`.
    #[error("instance name {0:?} must be 1-32 characters of a-z, 0-9 or '-'")]
    Name(String),
    /// The directory could not be created, or the lock file could not be opened.
    #[error("instance directory {path}: {source}")]
    Io {
        /// What was being opened.
        path: PathBuf,
        /// The underlying failure.
        source: std::io::Error,
    },
    /// Another process already owns this instance.
    ///
    /// `serve` exits with [`crate::supervisor::INSTANCE_HELD_EXIT`] for this one
    /// error rather than the generic failure code, so `up` can tell "somebody
    /// else is serving this instance" from "this binary is broken": the first
    /// is waited out, the second rolls the binary back. That only works while
    /// the error reaches `main` typed, which is why `crate::app` hands it over
    /// as `GotchaError::Other` instead of flattening it into a message.
    #[error(
        "another Droppen server ({holder}) already owns instance {name:?} ({path}).\n\
         Stop it, or start this one under a different instance name: APP_INSTANCE=<name> \
         (a-z, 0-9, '-'), which gives it its own terminals, runs and apps."
    )]
    Held {
        /// `pid N`, or `pid unknown` when the lock file held nothing readable.
        holder: String,
        /// The instance name that is taken.
        name: String,
        /// The lock file the holder has.
        path: PathBuf,
    },
}

impl InstanceError {
    fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// Whether `name` is a legal instance name.
///
/// Deliberately narrow: the name becomes a path segment under the data
/// directory, so anything that could traverse (`.`, `..`, `/`) or collide
/// case-insensitively on macOS is refused rather than sanitised.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `<data>/instances/<name>` — the one place the layout is spelled.
///
/// [`crate::sessions::Paths`] needs this too (the Claude config root an instance
/// owns hangs off it) and cannot go through [`InstanceDirs`], which validates and
/// therefore returns a `Result` that a path accessor has nowhere to put. Keeping
/// the join here rather than duplicating it is what stops the two from drifting;
/// `the_paths_and_the_instance_dirs_agree_on_the_root` pins that they do not.
pub fn instance_root(data_dir: &Path, name: &str) -> PathBuf {
    data_dir.join("instances").join(name)
}

/// One instance's directory, and every path inside it later phases will want.
///
/// Constructing this validates the name; it does not touch the filesystem.
/// [`InstanceDirs::create`] is the side-effecting half, called once at boot.
#[derive(Debug, Clone)]
pub struct InstanceDirs {
    name: String,
    root: PathBuf,
}

impl InstanceDirs {
    /// The directory for `name` under a data directory.
    pub fn new(data_dir: &Path, name: &str) -> Result<Self, InstanceError> {
        if !valid_name(name) {
            return Err(InstanceError::Name(name.to_string()));
        }
        Ok(Self {
            name: name.to_string(),
            root: instance_root(data_dir, name),
        })
    }

    /// Create the instance root and every subdirectory that is always present.
    pub fn create(&self) -> Result<(), InstanceError> {
        for dir in [self.root.clone(), self.pty_shims(), self.session_shims(), self.session_logs()] {
            std::fs::create_dir_all(&dir).map_err(|source| InstanceError::io(&dir, source))?;
        }
        Ok(())
    }

    /// The instance name, as `GET /api/settings` reports it.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// `<data>/instances/<name>`.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The file whose `flock` is this instance's ownership.
    pub fn lock_file(&self) -> PathBuf {
        self.root.join("lock")
    }

    /// One socket, tombstone, cursor and meta file per surviving terminal.
    pub fn pty_shims(&self) -> PathBuf {
        self.root.join("pty-shims")
    }

    /// One socket, stderr tail and tombstone per resident claude.
    ///
    /// Kept apart from [`InstanceDirs::pty_shims`] rather than merged with it:
    /// both hold shim sidecars, and adoption of either is "attach to whatever is
    /// in this directory", which two kinds of shim sharing one directory would
    /// make ambiguous.
    pub fn session_shims(&self) -> PathBuf {
        self.root.join("session-shims")
    }

    /// One append-only NDJSON recording per claude session this server drives.
    ///
    /// Instance-scoped like the shims above, and for the same reason: the file
    /// is appended to by whichever server owns the session, and two servers
    /// interleaving lines into one recording would produce a file that replays
    /// as neither session. See [`crate::record`].
    pub fn session_logs(&self) -> PathBuf {
        self.root.join("session-logs")
    }

    /// The port this instance's listener bound, written after the bind so a
    /// sibling tool can find a running server without guessing.
    pub fn port_file(&self) -> PathBuf {
        self.root.join("server.port")
    }

    /// Injected runs that outlive the server (phase 2).
    pub fn runs_file(&self) -> PathBuf {
        self.root.join("runs.json")
    }

    /// Embedded apps that outlive the server (phase 3).
    pub fn apps_file(&self) -> PathBuf {
        self.root.join("apps.json")
    }

    /// Record the port the listener actually bound.
    ///
    /// Best effort on purpose: a server that cannot write this file still
    /// serves, and the file is a hint for other tools, not state anything
    /// depends on.
    pub fn write_port(&self, port: u16) {
        let path = self.port_file();
        if let Err(error) = std::fs::write(&path, format!("{port}\n")) {
            tracing::warn!(path = %path.display(), %error, "cannot record the bound port");
        }
    }
}

/// An acquired instance lock. Holding this value is what holds the lock.
///
/// `flock(2)` is released when the last descriptor for the open file
/// description is closed, so the lock lives exactly as long as this struct —
/// which the application keeps alive for the whole of `serve`. A process that
/// dies for any reason, `SIGKILL` included, releases it in the kernel, which is
/// why this is a lock file rather than a pid file: there is no stale state to
/// clean up.
///
/// What the kernel does *not* defend is the name. An `flock` belongs to the
/// open file description, so it follows the inode and not the path: delete the
/// lock file — a tidy-up script, a volume shuffle, an operator with a `rm` —
/// and this process still holds a perfectly valid lock on an inode nothing can
/// reach, while the next server creates a fresh file at the same path, locks
/// that, and starts adopting. Two owners of one instance, which is the single
/// state this whole module exists to make impossible.
///
/// [`InstanceLock::verify_still_held`] is how a caller finds out, and
/// [`InstanceLock::identity`] is what makes the answer possible: the file's
/// `(device, inode)` is recorded at acquire time, exactly as
/// `crate::inject::lock` records its own.
#[derive(Debug)]
pub struct InstanceLock {
    /// Kept open: closing it is what releases the `flock`. Also read by
    /// [`InstanceLock::verify_still_held`], to prove the descriptor holding the
    /// lock is still the file the path names.
    file: File,
    path: PathBuf,
    /// What the path resolved to when the lock was taken, or `None` if the
    /// filesystem would not say.
    identity: Option<(u64, u64)>,
}

/// Why an instance lock can no longer be trusted.
///
/// Deliberately not a `bool`: every variant is a different accident with a
/// different remedy, and the caller — a self-upgrade about to restart the
/// server, an adoption pass about to take over another server's processes — has
/// to be able to say which one happened in the message it refuses with.
#[derive(Debug, thiserror::Error)]
pub enum LockLost {
    /// The lock file has been deleted. The `flock` is still held, on an inode
    /// with no name, and stops nobody.
    #[error(
        "the instance lock file {path} has been deleted; this server still holds a lock on the file that was there, \
         but nothing now stops a second server creating that path and claiming the instance"
    )]
    Deleted {
        /// The path that no longer exists.
        path: PathBuf,
    },
    /// The path names a different file than the one that was locked — deleted
    /// and re-created, or replaced wholesale by a restore or a rename.
    #[error("the instance lock file {path} is not the file this server locked; another server may already hold the one that is there now")]
    Replaced {
        /// The path that now names something else.
        path: PathBuf,
    },
    /// Neither the path nor the descriptor could be inspected, so the question
    /// has no answer. Treated as a failure rather than a pass: a caller asks
    /// this only when it is about to do something it cannot take back.
    #[error("cannot check the instance lock file {path}: {source}")]
    Unreadable {
        /// The path that could not be inspected.
        path: PathBuf,
        /// The underlying failure.
        source: std::io::Error,
    },
}

/// A file's `(device, inode)`, which is what identifies it across a rename.
fn identity_of(meta: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

impl InstanceLock {
    /// Take exclusive ownership of `dirs`, or fail naming the holder.
    ///
    /// Must run before anything adopts: the whole point is that only the holder
    /// ever looks inside the instance directory.
    pub fn acquire(dirs: &InstanceDirs) -> Result<Self, InstanceError> {
        dirs.create()?;
        let path = dirs.lock_file();
        let mut file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|source| InstanceError::io(&path, source))?;

        // SAFETY: `file` is an open descriptor this process owns for the whole
        // call, and `LOCK_EX | LOCK_NB` takes no other arguments.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            // Anything other than "someone else has it" is a real I/O failure —
            // a read-only filesystem, a path that is not a file — and reporting
            // it as contention would send the user hunting for a process that
            // does not exist.
            if !matches!(error.raw_os_error(), Some(libc::EWOULDBLOCK) | Some(libc::EINTR)) {
                return Err(InstanceError::io(&path, error));
            }
            return Err(InstanceError::Held {
                holder: read_holder(&mut file),
                name: dirs.name.clone(),
                path,
            });
        }

        // The pid is only ever advisory — the lock is the truth — but it is what
        // turns "instance is taken" into something the user can act on.
        let _ = file.set_len(0);
        let _ = file.seek(SeekFrom::Start(0));
        let _ = writeln!(file, "{}", std::process::id());
        let _ = file.flush();

        Ok(Self {
            identity: file.metadata().as_ref().map(identity_of).ok(),
            file,
            path,
        })
    }

    /// The lock file being held, for logging.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether this lock still means what it meant when it was taken.
    ///
    /// The `flock` itself needs no checking — the kernel holds it until this
    /// process dies. What is checked is that the *path* still resolves to the
    /// very inode the descriptor holds, because that equality is the only thing
    /// making the lock visible to the next server that opens it. A path that no
    /// longer names the locked file is an instance with no gatekeeper, whatever
    /// this process's descriptor still says.
    ///
    /// Call this before anything that takes over another server's resources or
    /// cannot be undone:
    ///
    /// * an adoption pass, which re-parents another process's pty shims,
    ///   injected runs and embedded apps on the strength of owning the
    ///   instance;
    /// * `POST /api/server/self-upgrade`, which replaces the running binary and
    ///   restarts — a restart that must not race a second server that quietly
    ///   became the owner while this one was up.
    ///
    /// Both should refuse loudly rather than proceed on a lock that has stopped
    /// meaning anything. Nothing is repaired here: re-creating the file would
    /// hand the instance to whichever server won that race, and this one cannot
    /// tell whether it did.
    pub fn verify_still_held(&self) -> Result<(), LockLost> {
        let held = match self.file.metadata() {
            Ok(meta) => identity_of(&meta),
            Err(source) => {
                return Err(LockLost::Unreadable {
                    path: self.path.clone(),
                    source,
                })
            }
        };
        let named = match std::fs::metadata(&self.path) {
            Ok(meta) => identity_of(&meta),
            // `ENOENT` is the case worth its own variant: nothing is there at
            // all, so the next server will create one and lock it unopposed.
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Err(LockLost::Deleted { path: self.path.clone() }),
            Err(source) => {
                return Err(LockLost::Unreadable {
                    path: self.path.clone(),
                    source,
                })
            }
        };
        // The identity recorded at acquire time is checked too, so a descriptor
        // that somehow stopped being the file it locked cannot pass by matching
        // an equally wrong path.
        if named != held || self.identity.is_some_and(|acquired| acquired != held) {
            return Err(LockLost::Replaced { path: self.path.clone() });
        }
        Ok(())
    }
}

/// Read the pid the holder wrote, for the refusal message.
fn read_holder(file: &mut File) -> String {
    let mut text = String::new();
    if file.seek(SeekFrom::Start(0)).is_err() || file.read_to_string(&mut text).is_err() {
        return describe_holder(None);
    }
    describe_holder(parse_holder(&text))
}

/// The pid last written into the lock file at `path`, read without locking it.
///
/// For a process that wants to *name* the holder rather than become it — the
/// supervisor, logging who it is waiting for. Advisory twice over: the pid is
/// only ever a hint (the `flock` is the truth), and once the holder exits the
/// file still names it until the next owner overwrites it. Callers should ask
/// only right after something has just been refused the lock.
pub fn recorded_holder(path: &Path) -> Option<u32> {
    parse_holder(&std::fs::read_to_string(path).ok()?)
}

/// `pid N`, or `pid unknown` — the form [`InstanceError::Held`] names a holder in.
pub fn describe_holder(pid: Option<u32>) -> String {
    match pid {
        Some(pid) => format!("pid {pid}"),
        None => "pid unknown".to_string(),
    }
}

fn parse_holder(text: &str) -> Option<u32> {
    // `None` when the holder has not written its pid yet: it takes the lock
    // first and writes second, and a reader can land in between.
    text.trim().parse::<u32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_lowercase_alphanumerics_and_dashes_name_an_instance() {
        for good in ["default", "a", "ci-2", "0", &"x".repeat(MAX_NAME)] {
            assert!(valid_name(good), "{good:?} should be a legal instance name");
        }
        for bad in [
            "",
            "Default",
            "with space",
            "with/slash",
            "..",
            "with_underscore",
            &"x".repeat(MAX_NAME + 1),
        ] {
            assert!(!valid_name(bad), "{bad:?} must not be a legal instance name");
        }
    }

    #[test]
    fn a_bad_name_never_becomes_a_path() {
        let data = tempfile::tempdir().expect("tempdir");
        assert!(matches!(InstanceDirs::new(data.path(), "../escape"), Err(InstanceError::Name(_))));
        let dirs = InstanceDirs::new(data.path(), "ci-1").expect("valid name");
        assert_eq!(dirs.root(), data.path().join("instances").join("ci-1"));
        assert_eq!(dirs.pty_shims(), dirs.root().join("pty-shims"));
        assert_eq!(dirs.session_logs(), dirs.root().join("session-logs"));
        assert_eq!(dirs.lock_file(), dirs.root().join("lock"));
        assert_eq!(dirs.port_file(), dirs.root().join("server.port"));
        assert_eq!(dirs.runs_file(), dirs.root().join("runs.json"));
        assert_eq!(dirs.apps_file(), dirs.root().join("apps.json"));
    }

    #[test]
    fn the_second_holder_of_one_instance_is_refused_and_told_the_pid() {
        let data = tempfile::tempdir().expect("tempdir");
        let dirs = InstanceDirs::new(data.path(), "default").expect("valid name");

        let held = InstanceLock::acquire(&dirs).expect("first acquire");
        assert!(dirs.pty_shims().is_dir(), "the instance tree is created with the lock");
        assert!(dirs.session_logs().is_dir(), "recordings included");

        // `flock` is per open file description, so a second `open` in this very
        // process contends exactly as another process would.
        let error = InstanceLock::acquire(&dirs).expect_err("a second acquire must be refused");
        let message = error.to_string();
        assert!(message.contains(&format!("pid {}", std::process::id())), "{message}");
        assert!(
            message.contains("APP_INSTANCE"),
            "the refusal must say how to run a second server: {message}"
        );

        // A different name is a different lock, which is the escape hatch the
        // message points at.
        let other = InstanceDirs::new(data.path(), "second").expect("valid name");
        let _other = InstanceLock::acquire(&other).expect("a different instance is free");

        drop(held);
        // A child another test forks meanwhile holds a copy of every open
        // descriptor until it execs (close-on-exec only acts at the exec), and
        // with it this `flock`: the lock is free once that copy is gone.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match InstanceLock::acquire(&dirs) {
                Ok(_) => break,
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(error) => panic!("the lock is released with the guard: {error:?}"),
            }
        }
    }

    #[test]
    fn the_holder_can_be_named_without_taking_the_lock() {
        let data = tempfile::tempdir().expect("tempdir");
        let dirs = InstanceDirs::new(data.path(), "default").expect("valid name");
        assert_eq!(recorded_holder(&dirs.lock_file()), None, "no file, no holder");

        let _held = InstanceLock::acquire(&dirs).expect("first acquire");
        assert_eq!(recorded_holder(&dirs.lock_file()), Some(std::process::id()));
        // Reading did not take the lock: the refusal still names the same pid.
        let refused = InstanceLock::acquire(&dirs).expect_err("still held");
        assert!(refused.to_string().contains(&describe_holder(Some(std::process::id()))), "{refused}");

        // Mid-write, or written by something that is not a server: unknown, not a guess.
        std::fs::write(dirs.lock_file(), "").expect("truncate");
        assert_eq!(recorded_holder(&dirs.lock_file()), None);
        assert_eq!(describe_holder(None), "pid unknown");
    }

    #[test]
    fn a_lock_whose_file_was_deleted_underneath_it_stops_claiming_to_own_anything() {
        let data = tempfile::tempdir().expect("tempdir");
        let dirs = InstanceDirs::new(data.path(), "default").expect("valid name");
        let held = InstanceLock::acquire(&dirs).expect("first acquire");

        // The happy path: an untouched lock verifies, and keeps verifying.
        held.verify_still_held().expect("an untouched lock is still held");
        held.verify_still_held().expect("and asking twice does not change that");

        std::fs::remove_file(dirs.lock_file()).expect("delete the lock file");
        let error = held.verify_still_held().expect_err("a deleted lock file must not verify");
        assert!(matches!(error, LockLost::Deleted { .. }), "{error:?}");
        assert!(
            error.to_string().contains(&dirs.lock_file().display().to_string()),
            "the failure has to name the file that went missing: {error}"
        );

        // And the danger is real rather than theoretical: with the name gone,
        // a second server takes the instance while the first still holds a
        // lock on the inode that used to be there.
        let usurper = InstanceLock::acquire(&dirs).expect("a fresh file locks unopposed");
        usurper.verify_still_held().expect("the new owner's own lock is sound");

        // The original now sees a stranger at its path, which is a different
        // accident from finding nothing there.
        let error = held.verify_still_held().expect_err("a replaced lock file must not verify either");
        assert!(matches!(error, LockLost::Replaced { .. }), "{error:?}");
    }

    /// The sidecars that moved must move for *every* instance: a path that
    /// still went through the config directory would be the shared file again.
    #[test]
    fn the_moved_sidecars_are_disjoint_between_instances() {
        let a = crate::sessions::Paths::for_instance(PathBuf::from("/home/me"), "a");
        let b = crate::sessions::Paths::for_instance(PathBuf::from("/home/me"), "b");

        assert_ne!(a.scheduled_tasks_file(), b.scheduled_tasks_file());

        // And the sidecars that are shared by design did not move with them.
        assert_eq!(a.prefs_file(), b.prefs_file());
        assert_eq!(a.titles_file(), b.titles_file());
        assert_eq!(a.seen_file(), b.seen_file());
        assert_eq!(a.archived_file(), b.archived_file());
    }

    #[test]
    fn the_bound_port_is_recorded() {
        let data = tempfile::tempdir().expect("tempdir");
        let dirs = InstanceDirs::new(data.path(), "ports").expect("valid name");
        dirs.create().expect("create");
        dirs.write_port(8700);
        assert_eq!(std::fs::read_to_string(dirs.port_file()).expect("port file").trim(), "8700");
    }
}
