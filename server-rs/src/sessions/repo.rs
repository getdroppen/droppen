//! Which git repository a session's working directory belongs to.
//!
//! A single project routinely occupies several directories at once: `git
//! worktree add` is how one repository is checked out four times so four agents
//! can work without treading on each other, and each of those checkouts is a
//! distinct `cwd`. A sidebar that groups by directory shows that project four
//! times, with no hint that the four are the same thing.
//!
//! `repoKey` is the grouping key that fixes it: the canonical path of `git
//! rev-parse --git-common-dir`, which every worktree of one repository agrees on
//! (a linked worktree's own `.git` is a file pointing at the main one, and
//! `--git-common-dir` is exactly "where the shared object store lives"). A
//! directory that is not in a repository has none, and gets `null` rather than a
//! made-up key that would merge unrelated directories.
//!
//! Two properties are load-bearing and easy to lose:
//!
//! * **it is display-only.** Scope decides what a server lists; this decides how
//!   the client may stack the rows it got. Grouping must never pull in a session
//!   the scope filter excluded.
//! * **it costs a process.** So it is cached per directory, bounded, and given a
//!   short deadline: a `git` that hangs on a network filesystem must cost one
//!   `null`, not the session list.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use futures_util::StreamExt;

use super::util::BoundedMap;
use super::SessionSummary;

/// How long `git` gets before the answer is "no repository".
///
/// Generous for a local `rev-parse` (which is a `stat` walk and no more) and
/// short enough that an unreachable network mount cannot hold a list refresh.
const GIT_TIMEOUT: Duration = Duration::from_secs(2);

/// How many directories are remembered.
///
/// A session list is tens of directories; the cap exists so a long-lived server
/// that has seen thousands cannot grow without bound, not to make the common
/// case a miss.
const CACHE_MAX: usize = 512;

/// `cwd` -> its repository key, `None` when it is not in one.
///
/// Answers are cached without invalidation, which is the right trade for what
/// this is: a directory's repository does not change, and the two cases where it
/// does — `git init` in a directory that already had sessions, or a repository
/// deleted underneath one — cost a stale grouping, not a wrong session list.
///
/// The near tier of two. Behind it is [`crate::db::index`], where the same
/// answers survive a restart, which is what makes this the cache with the
/// largest single payoff in the whole listing: 39 distinct directories at 8.6 ms
/// of process spawn each, four at a time, is 84 ms that a cold list used to pay
/// every time and now pays once a day. The durable tier is the one place a
/// bounded expiry had to be introduced, precisely because "until the server
/// restarts" stops being an answer once the answer outlives the server.
static CACHE: LazyLock<BoundedMap<PathBuf, Option<String>>> = LazyLock::new(|| BoundedMap::new(CACHE_MAX));

/// The repository `cwd` belongs to, or `None`.
pub async fn repo_key(cwd: &Path) -> Option<String> {
    if let Some(hit) = CACHE.get(cwd) {
        return hit;
    }
    let index = crate::db::index::current();
    if let Some(answer) = index.as_ref().and_then(|index| index.repo_key(cwd)) {
        CACHE.insert(cwd.to_path_buf(), answer.clone());
        return answer;
    }
    // `tokio::fs`, not `Path::is_dir`: this runs inside the list refresh, and a
    // blocking `stat` on a stalled network mount would hold a runtime worker —
    // and with it every other request that worker was going to poll.
    //
    // Hoisted out of [`probe`] because it now has a second reader: a directory
    // that is not there is not a fact about a repository, and writing it down
    // durably would leave a directory created a minute later ungrouped for as
    // long as the row lived. `git init` in a directory that *does* exist is
    // covered by the durable tier's expiry; this case is covered by nothing, so
    // it is simply never written.
    let exists = tokio::fs::metadata(cwd).await.is_ok_and(|meta| meta.is_dir());
    let answer = if exists { probe(cwd).await } else { None };
    CACHE.insert(cwd.to_path_buf(), answer.clone());
    if exists {
        if let Some(index) = index {
            index.remember_repo_key(cwd, answer.as_deref());
        }
    }
    answer
}

/// Variables that would make `git` answer about a repository other than `cwd`'s.
///
/// A server started from inside a git hook — a `git commit` hook that restarts
/// remote-hand, an editor launched by `git rebase --exec` — inherits these
/// pointing at *that* repository, and then every `rev-parse` in every session's
/// directory answers with it. Every unrelated project on the machine would group
/// under one heading, and which heading would depend on how the server happened
/// to be launched.
const GIT_LOCATION_OVERRIDES: [&str; 3] = ["GIT_DIR", "GIT_COMMON_DIR", "GIT_WORK_TREE"];

/// How many `git` probes may be in flight at once.
///
/// The first list after a boot has every distinct directory to resolve, and
/// running them one after another turns a cold sidebar into N process spawns in
/// series. Small rather than unbounded: these are `stat` walks, and a wide fan
/// out on a spinning disk or a network mount would cost more than the queue.
const PROBE_CONCURRENCY: usize = 4;

/// The environment a `git` probe runs with.
///
/// Split out so the strip is testable without spawning anything.
fn git_env() -> crate::process_env::Env {
    // The harness's own variables would reach a hook `git` runs; the same
    // scrubbing every other spawn in the crate gets, plus the three that would
    // change the answer itself.
    let mut env = crate::process_env::clean_env_bare();
    for key in GIT_LOCATION_OVERRIDES {
        env.remove(key);
    }
    env
}

/// Ask `git` where this directory's shared repository lives.
///
/// The caller has already established that `cwd` is a directory.
async fn probe(cwd: &Path) -> Option<String> {
    let mut command = tokio::process::Command::new("git");
    command
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    crate::process_env::apply_env(&mut command, &git_env());

    let output = tokio::time::timeout(GIT_TIMEOUT, command.output()).await.ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    // At the top of a main worktree `git` answers the relative `.git`; inside a
    // linked one it answers the main repository's absolute path. Resolving both
    // against `cwd` and then canonicalising is what makes the two forms the same
    // string — which is the entire contract of this key.
    let resolved = if Path::new(&raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        cwd.join(raw)
    };
    // `canonicalize` resolves every symlink in the path, so it is a blocking
    // syscall per component and belongs off the runtime for the same reason the
    // `stat` above does.
    let canonical = tokio::task::spawn_blocking(move || std::fs::canonicalize(&resolved).unwrap_or(resolved))
        .await
        .ok()?;
    Some(canonical.to_string_lossy().into_owned())
}

/// Fill in `repoKey` on a freshly built list.
///
/// One probe per *distinct* directory, so a project with a dozen sessions in it
/// costs one `git` on the first pass and nothing afterwards — and the distinct
/// directories of that first pass are resolved [`PROBE_CONCURRENCY`] at a time
/// rather than one after another. A directory already in the cache resolves with
/// no process at all, so a warm list never reaches the fan-out; the storm this
/// bounds is the cold one, right after a boot, when every directory is a miss.
pub async fn apply(sessions: &mut [SessionSummary]) {
    let mut wanted: Vec<String> = Vec::new();
    for session in sessions.iter() {
        let Some(cwd) = session.cwd.as_ref().filter(|cwd| !cwd.is_empty()) else {
            continue;
        };
        if !wanted.iter().any(|seen| seen == cwd) {
            wanted.push(cwd.clone());
        }
    }
    let keys: HashMap<String, Option<String>> = futures_util::stream::iter(wanted)
        .map(|cwd| async move {
            let key = repo_key(Path::new(&cwd)).await;
            (cwd, key)
        })
        .buffer_unordered(PROBE_CONCURRENCY)
        .collect()
        .await;

    for session in sessions.iter_mut() {
        let key = session.cwd.as_ref().and_then(|cwd| keys.get(cwd.as_str())).cloned().unwrap_or_default();
        session.repo_key = crate::openapi_ext::Nullable(key);
    }
}

/// How many directories the cache is holding. Test/observability only.
pub fn cache_len() -> usize {
    CACHE.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository with a linked worktree, or `None` where `git` is unavailable.
    fn repo_with_worktree(root: &Path) -> Option<(PathBuf, PathBuf)> {
        let main = root.join("main");
        std::fs::create_dir_all(&main).expect("main dir");
        let git = |args: &[&str], cwd: &Path| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(cwd)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .output()
                .ok()
                .filter(|out| out.status.success())
        };
        git(&["init", "-q", "-b", "main", "."], &main)?;
        std::fs::write(main.join("a.txt"), "a").expect("file");
        git(&["add", "."], &main)?;
        git(&["commit", "-qm", "first"], &main)?;
        let linked = root.join("wt");
        git(&["worktree", "add", "-q", linked.to_str().expect("utf-8"), "-b", "side"], &main)?;
        Some((main, linked))
    }

    #[tokio::test]
    async fn every_worktree_of_one_repository_shares_one_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let Some((main, linked)) = repo_with_worktree(dir.path()) else {
            eprintln!("skipping: no usable `git` on PATH");
            return;
        };

        let from_main = repo_key(&main).await.expect("the main worktree is in a repository");
        let from_linked = repo_key(&linked).await.expect("so is the linked one");
        assert_eq!(from_main, from_linked, "grouping by this key puts both checkouts under one heading");
        assert!(from_main.ends_with(".git"), "the key is the shared repository directory: {from_main}");
    }

    #[tokio::test]
    async fn a_directory_outside_a_repository_has_no_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let plain = dir.path().join("plain");
        std::fs::create_dir_all(&plain).expect("plain dir");
        // A tempdir can sit inside a repository on some machines; only assert the
        // negative when `git` agrees this one does not.
        if std::process::Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(&plain)
            .output()
            .is_ok_and(|out| out.status.success())
        {
            eprintln!("skipping: the temp directory is itself inside a repository");
            return;
        }
        assert_eq!(repo_key(&plain).await, None);
        assert_eq!(
            repo_key(&dir.path().join("does-not-exist")).await,
            None,
            "and a missing cwd is not a probe"
        );
    }

    /// A server launched from inside a git hook inherits `GIT_DIR` and friends
    /// pointing at *that* repository. Left in place, every `rev-parse` would
    /// answer with it and every project on the machine would group as one.
    #[test]
    fn the_variables_that_would_relocate_git_are_stripped() {
        // Set on this process, exactly as a hook would have set them.
        for key in GIT_LOCATION_OVERRIDES {
            std::env::set_var(key, "/some/other/repo/.git");
        }
        // A control: the strip must be surgical, not a scrub of everything.
        std::env::set_var("GIT_PAGER", "cat");

        let env = git_env();
        for key in GIT_LOCATION_OVERRIDES {
            assert!(!env.contains_key(key), "{key} would move the answer to another repository");
            std::env::remove_var(key);
        }
        assert_eq!(env.get("GIT_PAGER").map(String::as_str), Some("cat"), "nothing else is touched");
        std::env::remove_var("GIT_PAGER");
    }

    /// 84 ms of process spawns per cold list is the largest single cost the
    /// durable index removes, so this asserts the one thing that matters: that
    /// an answer already in the index is served without asking `git` at all.
    ///
    /// The fake key is the proof. No `rev-parse` anywhere could return it, so a
    /// probe would have overwritten it with `None`.
    #[tokio::test]
    async fn a_remembered_repository_key_costs_no_process() {
        let _guard = crate::db::index::exclusive().await;
        let home = tempfile::tempdir().expect("tempdir");
        let paths = crate::sessions::Paths::new(home.path().to_path_buf());
        crate::db::index::install(&paths);

        let cwd = home.path().join("some-checkout");
        std::fs::create_dir_all(&cwd).expect("cwd");
        crate::db::index::open(&paths).remember_repo_key(&cwd, Some("/repos/remembered/.git"));

        assert_eq!(
            repo_key(&cwd).await.as_deref(),
            Some("/repos/remembered/.git"),
            "a boot that has already resolved this directory does not resolve it again"
        );
        assert_eq!(
            CACHE.get(&cwd),
            Some(Some("/repos/remembered/.git".to_string())),
            "and the near tier is filled on the way through, so the next call is a map lookup"
        );
    }

    /// The cache exists to make re-listing free, not to grow forever.
    #[tokio::test]
    async fn the_cache_never_exceeds_its_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Missing directories never spawn `git`, so this is a cheap way to prove
        // the bound over more entries than the cap.
        for n in 0..(CACHE_MAX + 32) {
            let _ = repo_key(&dir.path().join(format!("missing-{n}"))).await;
        }
        assert!(cache_len() <= CACHE_MAX, "the cache grew to {}", cache_len());
    }
}
