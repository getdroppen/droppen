//! The `PATH` the Droppen server runs with.
//!
//! One rule, shared by the desktop app (`desktop/core/src/env_path.rs`), the
//! server's setup guide (`server-rs/src/setup/locate.rs`) and — pinned by
//! test against this crate — the command-line installer
//! (`scripts/install.sh` `service_path`). Every copy used to carry its own
//! implementation; this crate is the only one now.
//!
//! An app opened from the Finder, the Dock or a login item is started by
//! launchd with `/usr/bin:/bin:/usr/sbin:/sbin` and nothing else. The server
//! needs more: every session it starts runs `claude` or `codex`, and those —
//! and the `node` and `git` they call — live in `~/.local/bin`, Homebrew, a
//! Node version manager or wherever their installer put them.
//!
//! So the PATH is built the way the launchd agent of a command-line install
//! builds it: whatever the user's login shell says first (read by the caller,
//! which knows the shell), then a fixed list of the usual places, as a
//! fallback for what the shell did not say (or when it would not answer). The
//! shell first, so the server runs the `node` and `claude` the user's terminal
//! runs: nvm's, say, rather than Homebrew's. The server appends the folders
//! adopted in Settings after all of it (`with_overlay` in `setup::locate`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The directories every `PATH` has, after the login shell's, in order: the
/// user's own tool directories, then Homebrew (Apple silicon, Intel), then the
/// system.
///
/// Included whether or not they exist yet, which is the whole trick: the
/// official installers put their CLI in `~/.local/bin` and Homebrew in
/// `/opt/homebrew/bin`, so a tool installed while the server runs is found on
/// the next spawn with no re-read at all.
///
/// Absolute only: with no home (or a relative one, `HOME` unset) the user's own
/// directories are left out rather than put on the `PATH` relative to wherever a
/// child happens to run.
pub fn base_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = [".local/bin", ".npm-global/bin", ".cargo/bin", ".bun/bin", ".volta/bin"]
        .iter()
        .map(|dir| home.join(dir))
        .collect();
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"].map(PathBuf::from));
    dirs.retain(|dir| dir.is_absolute());
    dirs
}

/// The `PATH`: each directory of `shell` (the login shell's PATH) that is
/// absolute and exists, in the shell's order, then each of [`base_dirs`];
/// every directory once, where it first appears.
///
/// The shell first, so a session runs the `node` and `claude` the user's
/// terminal runs — nvm's, say, rather than Homebrew's; the base is the
/// fallback for what the shell did not say.
///
/// A session's own bin directory (`…/remote-hand/instances/<name>/session-bin`)
/// is dropped wherever it came from: it belongs to one server and names that
/// server's binary, and a `PATH` copied from a shell running inside a session
/// would otherwise hand it to this one.
pub fn build(home: &Path, shell: Option<&str>) -> String {
    let mut seen = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for dir in shell.unwrap_or_default().split(':') {
        if !dir.starts_with('/') || dir.contains("/remote-hand/instances/") || seen.contains(dir) {
            continue;
        }
        if Path::new(dir).is_dir() {
            seen.insert(dir.to_string());
            out.push(dir.to_string());
        }
    }
    for dir in base_dirs(home) {
        let dir = dir.to_string_lossy().into_owned();
        if seen.insert(dir.clone()) {
            out.push(dir);
        }
    }
    out.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule's output for `home`, as text, for the assertions below.
    fn dirs(home: &Path) -> Vec<String> {
        base_dirs(home).iter().map(|dir| dir.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn the_base_is_the_documented_directories_in_order() {
        assert_eq!(
            dirs(Path::new("/Users/ada")),
            [
                "/Users/ada/.local/bin",
                "/Users/ada/.npm-global/bin",
                "/Users/ada/.cargo/bin",
                "/Users/ada/.bun/bin",
                "/Users/ada/.volta/bin",
                "/opt/homebrew/bin",
                "/usr/local/bin",
                "/usr/bin",
                "/bin",
                "/usr/sbin",
                "/sbin",
            ]
        );
    }

    #[test]
    fn without_the_login_shell_the_base_is_the_path() {
        let path = build(Path::new("/Users/ada"), None);
        assert_eq!(path.split(':').collect::<Vec<_>>(), dirs(Path::new("/Users/ada")));
    }

    #[test]
    fn a_home_that_is_not_absolute_contributes_no_directory() {
        for home in ["", ".", "relative/home"] {
            assert!(base_dirs(Path::new(home)).iter().all(|dir| dir.is_absolute()), "{home:?}");
        }
    }
}
