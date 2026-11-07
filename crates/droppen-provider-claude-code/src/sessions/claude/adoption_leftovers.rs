//! What the removed "adopt a session" feature can have left on disk.
//!
//! Earlier builds could take over a Claude session this instance did not start
//! by copying its transcript into the root this instance owns. That feature is
//! gone; what it copied is not. An adopted session is an ordinary transcript in
//! the managed root, listed, opened, resumed and archived like any other, so
//! nothing here has to know about it.
//!
//! The one thing an adoption could leave behind that nothing else cleans up is
//! a half-copied transcript: the copy was written as `<id>.jsonl.adopting` and
//! renamed into place last, so an adoption interrupted in between left a full
//! copy that no listing will ever pick up. The sweep below still removes those
//! at boot, so a server upgraded straight out of an interrupted adoption does
//! not keep the file for ever. It only ever deletes files with that suffix.

use crate::claude_roots;
use droppen_instance::paths::{ClaudeRoot, Paths};

/// Suffix a half-written adopted transcript wore until it was complete.
const PARTIAL_SUFFIX: &str = "jsonl.adopting";

/// Delete the half-copied transcripts of adoptions that never finished.
///
/// **Must only be called while holding the instance lock**, like every other
/// boot sweep: it deletes files inside the instance directory.
///
/// Best effort throughout: a directory that cannot be read is one that had
/// nothing to sweep as far as this is concerned, and a server that cannot tidy
/// up still serves.
pub fn sweep_partial_adoptions(paths: &Paths) {
    let projects = claude_roots::ClaudeRoots::of(paths).projects_dir(ClaudeRoot::Managed);
    let Ok(entries) = std::fs::read_dir(&projects) else {
        return;
    };
    let mut swept = 0usize;
    for project in entries.flatten() {
        let Ok(files) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for file in files.flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            // A continuation onto an account (see [`super::continuation`]) is
            // written the same way, under its own suffix, and can be cut off
            // the same way.
            if !name.ends_with(PARTIAL_SUFFIX) && !name.ends_with(super::continuation::PARTIAL_SUFFIX) {
                continue;
            }
            match std::fs::remove_file(file.path()) {
                Ok(()) => swept += 1,
                Err(error) => tracing::warn!(path = %file.path().display(), %error, "cannot remove a half-copied adoption"),
            }
        }
    }
    if swept > 0 {
        tracing::info!(swept, "removed half-copied adoptions from an earlier run");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An adoption interrupted between its copy and its rename left a full
    /// transcript on disk that nothing will ever list.
    #[test]
    fn a_half_copied_adoption_is_swept_at_boot() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        let project = claude_roots::ClaudeRoots::of(&paths).projects_dir(ClaudeRoot::Managed).join("-w");
        std::fs::create_dir_all(&project).expect("project dir");
        let partial = project.join(format!("s1.{PARTIAL_SUFFIX}"));
        std::fs::write(&partial, "{}\n").expect("half-copied transcript");
        let real = project.join("s2.jsonl");
        std::fs::write(&real, "{}\n").expect("a real transcript");
        let continuing = project.join(format!("s3.{}", super::super::continuation::PARTIAL_SUFFIX));
        std::fs::write(&continuing, "{}\n").expect("half-copied continuation");

        sweep_partial_adoptions(&paths);
        assert!(!partial.exists(), "the leftover is gone");
        assert!(!continuing.exists(), "and so is a continuation cut off mid-copy");
        assert!(real.is_file(), "and nothing else is");

        // Nothing to sweep is not an error, on a tree that does not exist either.
        sweep_partial_adoptions(&paths);
        sweep_partial_adoptions(&Paths::new(home.path().join("elsewhere")));
    }
}
