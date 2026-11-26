//! The git a task's session runs in: the worktree a new context is given
//! (`docs/a2a.md` §5), and what the `result` artifact says of the tree
//! afterwards — its branch and the files that changed (§3.3).
//!
//! Every command is `git` from the server's `PATH`, run in the entry's
//! directory, with no terminal to prompt on and a time limit: a repository on
//! a hung network mount must not hold a task's start or its end for ever.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long one git command may take.
const GIT_TIMEOUT: Duration = Duration::from_secs(30);

/// The most changed files a `result` lists: a task that rewrote a vendored
/// tree is told so by the first thousand paths, and the artifact stays small.
const MAX_CHANGED_FILES: usize = 1000;

/// The worktree a new context was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Worktree {
    /// Where its session runs: the worktree, at the place the entry's
    /// directory is in its repository.
    pub cwd: String,
    /// Its branch, `a2a/<contextId>`.
    pub branch: String,
    /// The commit the branch started from, which the changed files are
    /// counted against.
    pub base: String,
}

/// What a git command printed, or why it did not run.
async fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        // A read here must not take the index lock a session's own git is
        // about to want.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(GIT_TIMEOUT, command.output())
        .await
        .map_err(|_| format!("git {} took longer than {}s", args.first().unwrap_or(&""), GIT_TIMEOUT.as_secs()))?
        .map_err(|error| format!("cannot run git: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(stderr.trim().lines().last().unwrap_or("git failed").to_string())
    }
}

/// The first line git printed, trimmed.
fn line(out: &[u8]) -> String {
    String::from_utf8_lossy(out).lines().next().unwrap_or_default().trim().to_string()
}

/// The top of the repository `dir` is in, or `None` outside one.
pub(crate) async fn toplevel(dir: &Path) -> Option<PathBuf> {
    git(dir, &["rev-parse", "--show-toplevel"])
        .await
        .ok()
        .map(|out| PathBuf::from(line(&out)))
        .filter(|top| !top.as_os_str().is_empty())
}

/// Whether `context_id` can name a worktree and a branch as it is: the
/// context ids are this server's own UUIDs, and nothing else is let near a
/// path or a ref name.
fn plain_id(context_id: &str) -> bool {
    !context_id.is_empty() && context_id.len() <= 64 && context_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Give context `context_id` its own worktree of the repository `cwd` is in:
/// `<cwd>/.worktrees/a2a-<contextId>` on a new branch `a2a/<contextId>`, from
/// the commit `cwd` has checked out.
///
/// A worktree or a branch of that name already there is refused, never
/// reused: it is somebody's, and a session let into it would work on top of
/// whatever is in it. Nothing is cleaned up afterwards either: the worktree
/// is where the task's work is.
///
/// `Err` is the sentence the task is rejected with
/// (`droppen.isolation_failed`).
pub(crate) async fn make_worktree(cwd: &str, context_id: &str) -> Result<Worktree, String> {
    if !plain_id(context_id) {
        return Err(format!("cannot name a worktree after the context id {context_id:?}"));
    }
    let dir = Path::new(cwd);
    let Some(top) = toplevel(dir).await else {
        return Err(format!("{cwd} is not in a git repository, so it cannot be given a worktree"));
    };
    let base = git(dir, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])
        .await
        .map(|out| line(&out))
        .ok()
        .filter(|base| !base.is_empty())
        .ok_or_else(|| format!("the repository at {} has no commit to branch from", top.display()))?;
    let path = dir.join(".worktrees").join(format!("a2a-{context_id}"));
    if std::fs::symlink_metadata(&path).is_ok() {
        return Err(format!("{} already exists; a context's worktree is never reused", path.display()));
    }
    let branch = format!("a2a/{context_id}");
    if git(dir, &["show-ref", "--verify", "--quiet", &format!("refs/heads/{branch}")])
        .await
        .is_ok()
    {
        return Err(format!("the branch {branch} already exists; a context's branch is never reused"));
    }
    let target = path.to_string_lossy().into_owned();
    git(dir, &["worktree", "add", "--quiet", "-b", &branch, &target, &base])
        .await
        .map_err(|why| format!("cannot add the worktree {target}: {why}"))?;
    // The session starts where the entry's directory is inside the repository,
    // so an entry pointed at a subdirectory works in the same subdirectory.
    let inside = std::fs::canonicalize(dir)
        .ok()
        .zip(std::fs::canonicalize(&top).ok())
        .and_then(|(dir, top)| dir.strip_prefix(&top).ok().map(Path::to_path_buf))
        .filter(|inside| !inside.as_os_str().is_empty());
    let cwd = match inside.map(|inside| path.join(inside)).filter(|nested| nested.is_dir()) {
        Some(nested) => nested,
        None => path,
    };
    Ok(Worktree {
        cwd: cwd.to_string_lossy().into_owned(),
        branch,
        base,
    })
}

/// The branch `cwd` has checked out, or `None` outside a repository or on a
/// detached head.
pub(crate) async fn current_branch(cwd: &str) -> Option<String> {
    let name = line(&git(Path::new(cwd), &["rev-parse", "--abbrev-ref", "HEAD"]).await.ok()?);
    (!name.is_empty() && name != "HEAD").then_some(name)
}

/// The files that changed in the repository `cwd` is in, as paths from its
/// top: against `base` — committed or not, and the new files git does not
/// ignore — when the context has a worktree, else whatever `git status`
/// reports. Empty outside a repository.
pub(crate) async fn changed_files(cwd: &str, base: Option<&str>) -> Vec<String> {
    let Some(top) = toplevel(Path::new(cwd)).await else {
        return Vec::new();
    };
    let mut files: Vec<String> = match base {
        Some(base) => {
            let mut files = split_nul(&git(&top, &["diff", "--name-only", "-z", base, "--"]).await.unwrap_or_default());
            files.extend(split_nul(
                &git(&top, &["ls-files", "--others", "--exclude-standard", "-z"]).await.unwrap_or_default(),
            ));
            files
        }
        None => status_paths(
            &git(&top, &["status", "--porcelain=v1", "-z", "--untracked-files=all"])
                .await
                .unwrap_or_default(),
        ),
    };
    files.sort();
    files.dedup();
    files.truncate(MAX_CHANGED_FILES);
    files
}

/// NUL-separated paths.
fn split_nul(out: &[u8]) -> Vec<String> {
    out.split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect()
}

/// The paths of `git status --porcelain=v1 -z`: `XY path`, and for a rename
/// or a copy the path it came from as the next entry, which is not a change of
/// its own here.
fn status_paths(out: &[u8]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut entries = out.split(|byte| *byte == 0).filter(|entry| !entry.is_empty());
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }
        let (code, path) = entry.split_at(3);
        paths.push(String::from_utf8_lossy(path).into_owned());
        if code[0] == b'R' || code[0] == b'C' {
            entries.next();
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_lists_each_changed_path_once_and_skips_where_a_rename_came_from() {
        let out = b" M src/a.rs\0?? new.txt\0R  moved.rs\0old.rs\0A  added.rs\0";
        assert_eq!(status_paths(out), ["src/a.rs", "new.txt", "moved.rs", "added.rs"]);
    }

    #[test]
    fn only_a_plain_id_names_a_worktree() {
        assert!(plain_id("0b8f3c52-1e1d-4c56-9f0e-6f1b2b9a7c11"));
        for bad in ["", "../x", "a/b", "a b", "a\0b", &"x".repeat(65)] {
            assert!(!plain_id(bad), "{bad:?}");
        }
    }
}
