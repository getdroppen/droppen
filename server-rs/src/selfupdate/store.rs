//! `<data>/bin`: where the binary the supervisor actually runs lives.
//!
//! A container ships a binary baked into an image layer. That binary cannot be
//! replaced from inside the container in any way that survives `docker rm`, and
//! rewriting it in place would be replacing the file a running process is
//! executing. So the *authoritative* binary is not the baked one: it is a copy
//! under the data volume, and the baked one is only ever a seed and a last
//! resort.
//!
//! ```text
//! <data>/bin/
//!   remote-hand-server         the binary `up` runs
//!   remote-hand-server.prev    the one it replaced, kept for one step back
//!   remote-hand-server.bad     one that could not stay up, kept for a look
//!   .staging-<tag>             an upload being checked; never runnable as the
//!                              installed name until it has passed
//! ```
//!
//! Two rules hold this together:
//!
//! * **Seed once, never overwrite.** A container that boots with a newer image
//!   must not silently undo an upgrade the user performed — and, more sharply,
//!   must not undo it *without* the user asking, on a restart they thought was
//!   routine. The baked binary is copied in only when nothing is installed.
//!   Downgrading is therefore explicit: delete the file, or upload the older
//!   binary.
//! * **Nothing becomes the installed name except by `rename`.** Every write goes
//!   to a staging path in the same directory and is renamed into place, so the
//!   installed path is either the whole old binary or the whole new one and
//!   never a half-copied file with the executable bit already set.

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::sessions::Paths;

/// The installed binary's name, which is also the process name in `ps`.
const NAME: &str = "remote-hand-server";

/// Mode for anything this module puts on disk meaning to run it.
const EXECUTABLE: u32 = 0o755;

/// What the supervisor fell back to when the installed binary would not stay up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    /// The previous binary was restored over the installed one.
    Previous,
    /// There was no previous binary; the baked one is next.
    Baked,
}

/// What one step down the rollback ladder actually did.
///
/// Two facts rather than one, because they come apart: there may have been
/// nothing installed to move aside — the supervisor was already running the
/// baked binary and failing — and a log line claiming a quarantine that never
/// happened sends whoever reads it looking for a `.bad` file that is not there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Demotion {
    /// Whether an installed binary was moved to `.bad`.
    pub quarantined: bool,
    /// What the supervisor should run next.
    pub fallback: Fallback,
}

/// The `<data>/bin` directory, and every path inside it.
///
/// Constructing this touches nothing; [`BinStore::create`] is the side-effecting
/// half.
#[derive(Debug, Clone)]
pub struct BinStore {
    dir: PathBuf,
}

impl BinStore {
    /// The store under a data directory.
    pub fn new(paths: &Paths) -> Self {
        Self {
            dir: paths.data_dir().join("bin"),
        }
    }

    /// `<data>/bin`.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Create the directory, if it is not there yet.
    pub fn create(&self) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)
    }

    /// `<data>/bin/remote-hand-server` — the one the supervisor runs.
    pub fn installed(&self) -> PathBuf {
        self.dir.join(NAME)
    }

    /// The binary the installed one replaced.
    pub fn previous(&self) -> PathBuf {
        self.dir.join(format!("{NAME}.prev"))
    }

    /// A binary that could not stay up, moved aside rather than deleted.
    ///
    /// Kept because "the upgrade broke it" is a thing somebody will want to look
    /// at, and because deleting the evidence of a crash loop is how a crash loop
    /// becomes unexplainable.
    pub fn quarantine(&self) -> PathBuf {
        self.dir.join(format!("{NAME}.bad"))
    }

    /// Where an upload is written while it is still being checked.
    ///
    /// In the same directory on purpose: the promotion is a `rename`, and
    /// `rename` is only atomic within one filesystem.
    pub fn staging(&self, tag: &str) -> PathBuf {
        // The tag is a caller-chosen label, never a path: anything that is not
        // plainly safe becomes an underscore rather than a directory traversal.
        let tag: String = tag
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+' {
                    c
                } else {
                    '_'
                }
            })
            .take(64)
            .collect();
        self.dir.join(format!(".staging-{tag}"))
    }

    /// Whether a binary has been installed into the store.
    pub fn is_installed(&self) -> bool {
        self.installed().is_file()
    }

    /// The binary to run: the installed one if there is one, else `fallback`.
    ///
    /// `fallback` is the caller's `current_exe` — the baked binary in a
    /// container, or the developer's `target/debug` build.
    pub fn resolve(&self, fallback: &Path) -> PathBuf {
        if self.is_installed() {
            self.installed()
        } else {
            fallback.to_path_buf()
        }
    }

    /// Copy `from` in as the installed binary, but only if nothing is installed.
    ///
    /// Returns whether it seeded. See the module docs for why this never
    /// overwrites.
    pub fn seed(&self, from: &Path) -> io::Result<bool> {
        self.create()?;
        if self.is_installed() {
            return Ok(false);
        }
        let staged = self.staging("seed");
        install_copy(from, &staged)?;
        std::fs::rename(&staged, self.installed())?;
        Ok(true)
    }

    /// Make `staged` the installed binary, keeping the incumbent as `.prev`.
    ///
    /// The order matters and is the whole reason this is one function: the
    /// incumbent is copied aside *before* the rename, so a crash between the two
    /// steps leaves a working binary installed and merely a stale `.prev`.
    pub fn promote(&self, staged: &Path) -> io::Result<()> {
        let installed = self.installed();
        if installed.is_file() {
            // A copy rather than a rename: renaming would leave nothing at the
            // installed path for the moment between the two calls, and a
            // supervisor restarting in that window would find no binary at all.
            //
            // The name is unique per call: two upgrades in flight at the same
            // time would otherwise write to one path and rename it twice, and
            // the loser's half-copied file is what would become `.prev` — the
            // binary the rollback ladder reaches for when things are already
            // going badly.
            let keeping = self.staging(&format!("prev-{}", uuid::Uuid::new_v4()));
            install_copy(&installed, &keeping)?;
            std::fs::rename(&keeping, self.previous())?;
        }
        std::fs::rename(staged, &installed)
    }

    /// Move the installed binary out of the way and put the previous one back.
    ///
    /// The supervisor's last resort after a binary has proved it cannot stay up.
    /// Returns what the caller should run next.
    pub fn demote(&self) -> io::Result<Demotion> {
        let installed = self.installed();
        let quarantined = installed.is_file();
        if quarantined {
            // Replace rather than accumulate: one `.bad` is a clue, a directory
            // full of them is a mess nobody reads.
            let _ = std::fs::remove_file(self.quarantine());
            std::fs::rename(&installed, self.quarantine())?;
        }
        let previous = self.previous();
        if previous.is_file() {
            std::fs::rename(&previous, &installed)?;
            return Ok(Demotion {
                quarantined,
                fallback: Fallback::Previous,
            });
        }
        Ok(Demotion {
            quarantined,
            fallback: Fallback::Baked,
        })
    }

    /// Delete every `.staging-*` left over from an attempt that did not finish.
    ///
    /// Uploads are guarded and clean up after themselves, but a process that is
    /// killed between `create` and the guard's `Drop` — an OOM kill, a
    /// `SIGKILL`, a power cut — has no chance to run anything. Each of those
    /// leaves a file the size of a server binary, so the sweep exists to make
    /// that cost one file rather than one per crash forever.
    ///
    /// Best effort and quiet about individual failures: a file that cannot be
    /// removed is not a reason to refuse to boot or to refuse an upgrade.
    pub fn sweep_staging(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.starts_with(".staging-") {
                continue;
            }
            let path = entry.path();
            match std::fs::remove_file(&path) {
                Ok(()) => tracing::info!(path = %path.display(), "removed a staged binary left by an unfinished upgrade"),
                Err(error) => tracing::warn!(path = %path.display(), %error, "cannot remove a stale staged binary"),
            }
        }
    }
}

/// Copy a file and make it executable, leaving nothing behind on failure.
fn install_copy(from: &Path, to: &Path) -> io::Result<()> {
    // A stale staging file from an interrupted attempt would otherwise be
    // appended to nothing and silently kept at its old length.
    let _ = std::fs::remove_file(to);
    std::fs::copy(from, to)?;
    std::fs::set_permissions(to, std::fs::Permissions::from_mode(EXECUTABLE))?;
    Ok(())
}

/// Give a file the executable bit, for a path written rather than copied.
pub fn make_executable(path: &Path) -> io::Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(EXECUTABLE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(root: &Path) -> BinStore {
        BinStore { dir: root.join("bin") }
    }

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, body).expect("write");
    }

    #[test]
    fn seeding_happens_once_and_never_undoes_an_upgrade() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = store(root.path());
        let baked = root.path().join("baked");
        write(&baked, "v1");

        assert!(store.seed(&baked).expect("seed"), "an empty store takes the baked binary");
        assert_eq!(std::fs::read_to_string(store.installed()).expect("read"), "v1");
        assert_eq!(
            std::fs::metadata(store.installed()).expect("stat").permissions().mode() & 0o777,
            EXECUTABLE
        );

        // The user upgrades, then the container restarts with the old image.
        write(&store.installed(), "v2");
        assert!(!store.seed(&baked).expect("seed"), "an installed binary is never overwritten");
        assert_eq!(std::fs::read_to_string(store.installed()).expect("read"), "v2");
    }

    #[test]
    fn the_binary_to_run_is_the_installed_one_when_there_is_one() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = store(root.path());
        let baked = root.path().join("baked");
        write(&baked, "v1");

        assert_eq!(store.resolve(&baked), baked, "with nothing installed, the baked binary runs");
        store.seed(&baked).expect("seed");
        assert_eq!(store.resolve(&baked), store.installed());
    }

    #[test]
    fn promoting_keeps_exactly_one_step_back() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = store(root.path());
        store.create().expect("create");
        write(&store.installed(), "v1");

        let staged = store.staging("2.0.0");
        write(&staged, "v2");
        store.promote(&staged).expect("promote");
        assert_eq!(std::fs::read_to_string(store.installed()).expect("read"), "v2");
        assert_eq!(std::fs::read_to_string(store.previous()).expect("read"), "v1");
        assert!(!staged.exists(), "the staging file is consumed by the rename");

        let staged = store.staging("3.0.0");
        write(&staged, "v3");
        store.promote(&staged).expect("promote");
        assert_eq!(std::fs::read_to_string(store.installed()).expect("read"), "v3");
        assert_eq!(std::fs::read_to_string(store.previous()).expect("read"), "v2", "only one step is kept");
    }

    #[test]
    fn a_binary_that_cannot_stay_up_is_quarantined_and_the_previous_one_restored() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = store(root.path());
        store.create().expect("create");
        write(&store.installed(), "broken");
        write(&store.previous(), "working");

        let step = store.demote().expect("demote");
        assert_eq!(step.fallback, Fallback::Previous);
        assert!(step.quarantined, "there was an installed binary to move aside");
        assert_eq!(std::fs::read_to_string(store.installed()).expect("read"), "working");
        assert_eq!(std::fs::read_to_string(store.quarantine()).expect("read"), "broken");
        assert!(!store.previous().exists(), "the previous binary was moved, not copied");

        // Now the restored one fails too, and there is nothing left to fall back
        // to but the baked binary.
        let step = store.demote().expect("demote");
        assert_eq!(step.fallback, Fallback::Baked);
        assert!(step.quarantined, "the restored binary was itself moved aside");
        assert!(!store.is_installed(), "nothing installed means `resolve` picks the baked binary");
        assert_eq!(
            std::fs::read_to_string(store.quarantine()).expect("read"),
            "working",
            "the newest failure is the one kept"
        );
    }

    #[test]
    fn stepping_down_with_nothing_installed_quarantines_nothing_and_says_so() {
        // The supervisor is already running the baked binary and it is the one
        // failing. There is nothing to move aside, and a log line claiming a
        // `.bad` file would send a reader looking for one that is not there.
        let root = tempfile::tempdir().expect("tempdir");
        let store = store(root.path());
        store.create().expect("create");

        let step = store.demote().expect("demote");
        assert_eq!(step.fallback, Fallback::Baked);
        assert!(!step.quarantined);
        assert!(!store.quarantine().exists());
    }

    #[test]
    fn an_upgrade_that_never_finished_leaves_nothing_behind_after_a_sweep() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = store(root.path());
        store.create().expect("create");
        write(&store.installed(), "live");
        write(&store.previous(), "older");
        write(&store.staging("7-abc"), "half an upload");
        write(&store.staging("9-def"), "half another");

        store.sweep_staging();

        assert!(!store.staging("7-abc").exists());
        assert!(!store.staging("9-def").exists());
        // Everything that is not a staging file is somebody's binary.
        assert_eq!(std::fs::read_to_string(store.installed()).expect("read"), "live");
        assert_eq!(std::fs::read_to_string(store.previous()).expect("read"), "older");
    }

    #[test]
    fn two_promotions_never_share_a_staging_name() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = store(root.path());
        assert_ne!(
            store.staging(&format!("prev-{}", uuid::Uuid::new_v4())),
            store.staging(&format!("prev-{}", uuid::Uuid::new_v4())),
            "a fixed name would let one promotion overwrite another's `.prev`"
        );
    }

    #[test]
    fn a_staging_tag_is_a_filename_and_never_a_path() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = store(root.path());
        let escape = store.staging("../../etc/cron.d/x");
        assert_eq!(escape.parent(), Some(store.dir()), "{}", escape.display());
        assert_eq!(store.staging("0.1.0+g26305ca").file_name().expect("name"), ".staging-0.1.0+g26305ca");
    }
}
