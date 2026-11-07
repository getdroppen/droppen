//! Several worktrees of one repository read as one project.
//!
//! `git worktree add` is how a repository gets checked out four times so four
//! agents can work without treading on each other. Every one of those checkouts
//! is a different `cwd`, so a sidebar grouped by directory shows one project four
//! times with nothing saying they are the same thing. `repoKey` is the grouping
//! key that fixes it, and `recent-cwds` is the first consumer: the directory
//! picker offers one entry per repository rather than one per checkout.
//!
//! Strictly a display key. It never widens what a server lists — the fixtures
//! below are all in the root this server owns for exactly that reason, so
//! grouping is tested without scope in the way.

mod support;

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use support::Probe;

/// Run one git command, returning `None` when git is unusable here.
fn git(args: &[&str], cwd: &Path) -> Option<()> {
    std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        // A developer's own git config must not decide what this test sees:
        // a template directory, a default branch name or a hook would all
        // change the answer.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|_| ())
}

/// A repository with one linked worktree, or `None` where git cannot make one.
fn repo_with_worktree(root: &Path) -> Option<(PathBuf, PathBuf)> {
    let main = root.join("main");
    std::fs::create_dir_all(&main).expect("main dir");
    git(&["init", "-q", "-b", "main", "."], &main)?;
    std::fs::write(main.join("a.txt"), "a").expect("file");
    git(&["add", "."], &main)?;
    git(&["commit", "-qm", "first"], &main)?;
    let linked = root.join("wt");
    git(&["worktree", "add", "-q", linked.to_str().expect("utf-8"), "-b", "side"], &main)?;
    Some((main, linked))
}

/// A session this server owns, in `cwd`, started at `created`.
///
/// Dated by hand because the whole question `recent-cwds` answers is *when work
/// was started*, and letting the clock decide would make the order depend on how
/// fast the test ran.
fn seed(probe: &Probe, id: &str, cwd: &Path, created: &str) {
    let project = probe.managed_root().join("projects").join(format!("-{id}"));
    std::fs::create_dir_all(&project).expect("project dir");
    std::fs::write(
        project.join(format!("{id}.jsonl")),
        format!(
            "{}\n",
            json!({
                "type": "user",
                "cwd": cwd.to_string_lossy(),
                "timestamp": created,
                "message": {"content": [{"type": "text", "text": "hello"}]},
            })
        ),
    )
    .expect("seed transcript");
}

/// The `repoKey` of each session, keyed by id.
async fn repo_keys(probe: &Probe) -> std::collections::HashMap<String, Value> {
    probe.get_json("/api/sessions?scope=all").await["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .map(|row| (row["id"].as_str().expect("id").to_string(), row["repoKey"].clone()))
        .collect()
}

#[tokio::test]
async fn worktrees_of_one_repository_share_a_key_and_collapse_in_the_picker() {
    let probe = Probe::start().await;
    let Some((main, linked)) = repo_with_worktree(&probe.workspace()) else {
        eprintln!("skipping: no usable `git` on PATH");
        probe.stop().await;
        return;
    };
    // A directory that is in no repository at all, to prove nothing unrelated is
    // merged with anything.
    let plain = probe.home.join("plain");
    std::fs::create_dir_all(&plain).expect("plain dir");

    // The linked worktree was started in most recently; the main one is older.
    seed(&probe, "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa", &main, "2026-01-01T00:00:00.000Z");
    seed(&probe, "bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb", &linked, "2026-06-01T00:00:00.000Z");
    seed(&probe, "cccccccc-3333-4333-8333-cccccccccccc", &plain, "2026-03-01T00:00:00.000Z");

    let keys = repo_keys(&probe).await;
    let from_main = keys["aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa"].clone();
    let from_linked = keys["bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb"].clone();
    assert!(from_main.as_str().is_some(), "a session in a repository has a key: {from_main}");
    assert_eq!(from_main, from_linked, "two checkouts of one repository are one project");
    assert_eq!(
        keys["cccccccc-3333-4333-8333-cccccccccccc"],
        Value::Null,
        "a directory outside git gets no key rather than a made-up one"
    );

    // The picker offers the repository once, represented by the checkout work
    // was most recently started in — and the unrelated directory separately.
    let cwds = probe.get_json("/api/sessions/recent-cwds").await["cwds"].clone();
    assert_eq!(
        cwds,
        json!([linked.to_string_lossy(), plain.to_string_lossy()]),
        "one entry per repository, newest start first"
    );

    probe.stop().await;
}
