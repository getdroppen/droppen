//! Archiving a session out of the list.
//!
//! Port of the former TypeScript server's `archive.ts`. Claude has no native
//! archive, so remote-hand keeps its own hidden list; Codex has one, and it is
//! driven through the official CLI so the Codex app stays in sync.
//!
//! The hidden list is one row per archived id in the machine-wide state database
//! ([`droppen_instance::db`]) under [`scope::ARCHIVED`]. It is the store this migration is
//! most obviously for: `archived.json` was read-modify-written with **no lock of
//! any kind** — not even the process-local mutex the other sidecars take — so
//! two archives in flight each read the same list, each appended their own id,
//! and the second write threw the first one away. The session came back. An
//! upsert of one row cannot do that, to another tab or to the other server
//! sharing this directory.
//!
//! The value in each row is the instant the session was archived, in epoch
//! milliseconds. Rows written before that was recorded are the literal `"1"`;
//! the retention purge reads them as "unknown" and falls back on the
//! transcript's mtime.
//!
//! `archived.json` was a mirror for one version, so a `.prev` binary that had
//! never heard of the database would still hide the same sessions; the rollback
//! floor has since risen to a build that reads the database, and
//! `droppen_provider_claude_code::sessions::claude::archived_ids` — the last reader that went to the
//! file — now comes back through [`archived_ids`] here. Nothing reads the file
//! any more.

use std::sync::Arc;

use droppen_base::time::{mtime_ms, now_ms};
use droppen_instance::db::{scope, Db};
use droppen_instance::paths::{ClaudeRoot, Paths};
use droppen_sessions::Purged;

/// The value a row written before archive instants carried. The row's presence
/// was the fact; nothing read its value.
const PRESENT: &str = "1";

/// The machine-wide database the hidden list lives in.
fn store(paths: &Paths) -> Arc<Db> {
    droppen_instance::db::at(paths.config_db())
}

/// Every id the user has archived out of the list.
///
/// The read half of this module, and the reason the write half stopped being a
/// read-modify-write of a whole file. Discovery calls it on every refresh and it
/// answers from the table, which is the only place an archive is recorded.
pub fn archived_ids(paths: &Paths) -> std::collections::HashSet<String> {
    store(paths).kv_scope(scope::ARCHIVED).into_keys().collect()
}

/// Every archived id with the instant it was archived, if the row recorded one.
///
/// `None` is a legacy `"1"` row: the archive happened, but before this module
/// kept when. The retention purge dates those by the transcript's mtime.
pub fn archived_rows(paths: &Paths) -> Vec<(String, Option<i64>)> {
    store(paths)
        .kv_scope(scope::ARCHIVED)
        .into_iter()
        .map(|(id, value)| {
            let at = if value == PRESENT { None } else { value.parse::<i64>().ok() };
            (id, at)
        })
        .collect()
}

/// Forget one archived id. Forgetting what is not there is not an error.
pub fn forget(paths: &Paths, id: &str) {
    store(paths).kv_remove(scope::ARCHIVED, id);
}

/// Delete archived transcripts whose archive instant is at or before
/// `cutoff_ms`, and forget their rows.
///
/// A session's data is not only its transcript: the `subagents/` directory
/// beside it, its `tasks/` sidecars, and its recording under the instance's
/// `session-logs/` all go with it.
///
/// The row is machine-wide and the roots of a sibling instance are not walkable
/// from here, so a row with no transcript *here* is only forgotten when this
/// instance owns the session: another instance may be holding it. A removal
/// that fails for any reason other than "already gone" keeps the row for the
/// next pass, so a transcript is never resurrected unarchived.
///
/// `skip` is asked for each id that would otherwise go, so a session a CLI is
/// still driving keeps its transcript.
pub async fn purge_archived(paths: &Paths, cutoff_ms: i64, skip: &(dyn Fn(&str) -> bool + Sync)) -> Purged {
    let owned = droppen_sessions::owned_sessions::read(paths).await;
    let mut purged = Purged::default();
    for (id, recorded) in archived_rows(paths) {
        let mut found = Vec::new();
        for root in ClaudeRoot::ALL {
            if let Some(file) = super::find_in_root(paths, root, &id).await {
                found.push((root, file));
            }
        }
        if found.is_empty() {
            // Another instance's managed root is not walkable from here, so
            // only the owner may tidy a row with nothing here. Its leftovers —
            // a sidecar directory, a checklist, a recording — still are here,
            // and a removal that failed after the transcript went is retired
            // by this pass rather than forgotten. A CLI still driving it keeps
            // everything, even if its transcript is momentarily absent.
            if skip(&id) || !owned.contains(droppen_sessions::Source::Claude, &id) {
                continue;
            }
            let mut bytes = 0;
            let mut gone = true;
            for root in ClaudeRoot::ALL {
                if let Some(tasks) = crate::sessions::checklist::tasks_dir(paths, root, &id) {
                    gone &= remove_dir(&tasks, &id, &mut bytes);
                }
            }
            for sidecars in sidecar_dirs(paths, &id).await {
                gone &= remove_dir(&sidecars, &id, &mut bytes);
            }
            let logs = paths.instance_dir().join("session-logs");
            gone &= remove_file(&logs.join(format!("{id}.ndjson")), &id, &mut bytes);
            gone &= remove_file(&logs.join(format!("{id}.offsets.json")), &id, &mut bytes);
            if gone {
                forget(paths, &id);
                purged.add(&id, bytes);
            }
            continue;
        }
        // A legacy row has no instant; the transcript's own last write is the
        // best substitute there is.
        let archived_at = recorded.unwrap_or_else(|| {
            found
                .iter()
                .filter_map(|(_, file)| std::fs::metadata(file).ok())
                .map(|metadata| mtime_ms(&metadata) as i64)
                .max()
                .unwrap_or(0)
        });
        if archived_at > cutoff_ms || skip(&id) {
            continue;
        }
        let mut bytes = 0;
        let mut gone = true;
        for (root, file) in &found {
            gone &= remove_file(file, &id, &mut bytes);
            // `<id>/` holds the subagents and the tool results the CLI kept
            // for this session; it is this transcript's alone.
            if let Some(sidecars) = crate::sessions::transcript::subagent_dir(file).parent() {
                gone &= remove_dir(sidecars, &id, &mut bytes);
            }
            if let Some(tasks) = crate::sessions::checklist::tasks_dir(paths, *root, &id) {
                gone &= remove_dir(&tasks, &id, &mut bytes);
            }
        }
        // The recording is this instance's, and only an owning instance has
        // one; an absent directory is the ordinary case.
        let logs = paths.instance_dir().join("session-logs");
        gone &= remove_file(&logs.join(format!("{id}.ndjson")), &id, &mut bytes);
        gone &= remove_file(&logs.join(format!("{id}.offsets.json")), &id, &mut bytes);
        if !gone {
            continue;
        }
        forget(paths, &id);
        purged.add(&id, bytes);
    }
    purged
}

/// Every `<project>/<id>/` directory a transcript's sidecars could have lived
/// in, for a transcript that is already gone. There is normally one; the same
/// id copied into both roots leaves one under each.
async fn sidecar_dirs(paths: &Paths, id: &str) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    if !super::is_session_id(id) {
        return found;
    }
    let roots = crate::claude_roots::ClaudeRoots::of(paths);
    for root in ClaudeRoot::ALL {
        let Ok(mut projects) = tokio::fs::read_dir(roots.projects_dir(root)).await else { continue };
        while let Ok(Some(project)) = projects.next_entry().await {
            let candidate = project.path().join(id);
            if tokio::fs::metadata(&candidate).await.is_ok_and(|metadata| metadata.is_dir()) {
                found.push(candidate);
            }
        }
    }
    found
}

/// Delete one file, adding its size to `bytes`. `true` when nothing is left
/// behind.
fn remove_file(path: &std::path::Path, id: &str, bytes: &mut u64) -> bool {
    *bytes += std::fs::metadata(path).map(|metadata| metadata.len()).unwrap_or(0);
    match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => {
            tracing::warn!(session = %id, %error, "cannot delete an archived transcript");
            false
        }
    }
}

/// Delete one directory tree, adding its contents to `bytes`. `true` when
/// nothing is left behind.
fn remove_dir(path: &std::path::Path, id: &str, bytes: &mut u64) -> bool {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return true,
        Err(error) => {
            tracing::warn!(session = %id, path = %path.display(), %error, "cannot look at an archived session's sidecar directory");
            return false;
        }
    };
    if !metadata.is_dir() {
        return remove_file(path, id, bytes);
    }
    *bytes += tree_bytes(path);
    match std::fs::remove_dir_all(path) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => {
            tracing::warn!(session = %id, %error, "cannot delete an archived session's sidecar directory");
            false
        }
    }
}

/// The bytes in a directory tree, skipping what cannot be read.
fn tree_bytes(path: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else { return 0 };
    entries
        .flatten()
        .map(|entry| match entry.metadata() {
            Ok(metadata) if metadata.is_dir() => tree_bytes(&entry.path()),
            Ok(metadata) => metadata.len(),
            Err(_) => 0,
        })
        .sum()
}

/// Add a claude session to remote-hand's hidden list.
///
/// Still `io::Result` because the route is built on it. Nothing in here does
/// I/O that can fail — the archive is one committed row — so the result is
/// unconditionally `Ok`.
///
/// No lock, either. The one this module briefly had was never for the archive;
/// it kept two whole-file rewrites of `archived.json` from being applied in the
/// opposite order to the commits they described, which would have left the file
/// holding the older snapshot and a hidden session back on screen. There is no
/// file to rewrite, so there is nothing left to order.
pub async fn archive_claude_session(paths: &Paths, id: &str) -> std::io::Result<()> {
    // The whole fix, in one statement: an upsert of this id alone, which cannot
    // take another archive down with it and does not need to have read the list
    // first. Archiving something twice is the same row twice, so the
    // idempotence the `[...new Set(...)]` gave is now the primary key's.
    store(paths).kv_set(scope::ARCHIVED, id, &now_ms().to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    use crate::claude_roots::ClaudeRoots;

    /// A transcript in the managed root, with content to count.
    fn transcript(paths: &Paths, id: &str) -> std::path::PathBuf {
        let dir = ClaudeRoots::of(paths).projects_dir(ClaudeRoot::Managed).join("project");
        std::fs::create_dir_all(&dir).expect("project dir");
        let file = dir.join(format!("{id}.jsonl"));
        std::fs::write(&file, b"transcript body").expect("transcript");
        file
    }

    fn set_mtime(file: &Path, millis: u64) {
        let time = SystemTime::UNIX_EPOCH + Duration::from_millis(millis);
        std::fs::File::options()
            .write(true)
            .open(file)
            .expect("open")
            .set_modified(time)
            .expect("mtime");
    }

    fn archived_row(paths: &Paths, id: &str, value: String) {
        store(paths).kv_set(scope::ARCHIVED, id, &value);
    }

    #[tokio::test]
    async fn archiving_records_the_instant() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        let before = now_ms();
        archive_claude_session(&paths, "one").await.expect("archive");
        let rows = archived_rows(&paths);
        assert_eq!(rows.len(), 1);
        let at = rows[0].1.expect("an instant, not a legacy row");
        assert!(at >= before && at <= now_ms());
    }

    #[tokio::test]
    async fn purging_deletes_only_old_archived_data() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        let old = transcript(&paths, "old");
        let recent = transcript(&paths, "recent");
        // The data a session leaves beside its transcript: its `<id>/`
        // directory (subagents, tool results), its checklist and its recording.
        let sidecars = crate::sessions::transcript::subagent_dir(&old).parent().expect("sidecar dir").to_path_buf();
        std::fs::create_dir_all(sidecars.join("subagents")).expect("subagents");
        std::fs::write(sidecars.join("subagents/agent-a1.jsonl"), b"agent").expect("subagent");
        std::fs::create_dir_all(sidecars.join("tool-results")).expect("tool results");
        std::fs::write(sidecars.join("tool-results/t.txt"), b"tool").expect("tool result");
        let tasks = crate::sessions::checklist::tasks_dir(&paths, ClaudeRoot::Managed, "old").expect("tasks dir");
        std::fs::create_dir_all(&tasks).expect("tasks");
        std::fs::write(tasks.join("1.json"), b"task").expect("task");
        let logs = paths.instance_dir().join("session-logs");
        std::fs::create_dir_all(&logs).expect("logs");
        std::fs::write(logs.join("old.ndjson"), b"log").expect("recording");
        std::fs::write(logs.join("old.offsets.json"), b"of").expect("offsets");
        archived_row(&paths, "old", "1000000".to_string());
        archived_row(&paths, "recent", (now_ms() - 60_000).to_string());

        let cutoff = now_ms() - 24 * 60 * 60 * 1000;
        let purged = purge_archived(&paths, cutoff, &|_| false).await;

        assert_eq!(purged.sessions, 1);
        assert_eq!(
            purged.bytes,
            ("transcript body".len() + "agent".len() + "tool".len() + "task".len() + "log".len() + "of".len()) as u64,
            "the sidecars are what a session with subagents or a checklist costs"
        );
        assert!(!old.exists(), "the old transcript is gone");
        assert!(!sidecars.exists(), "and its subagents and tool results");
        assert!(!tasks.exists(), "and its checklist");
        assert!(!logs.join("old.ndjson").exists(), "and its recording");
        assert!(recent.exists(), "the recent one stays");
        assert_eq!(
            archived_ids(&paths),
            ["recent".to_string()].into_iter().collect::<std::collections::HashSet<_>>()
        );
    }

    #[tokio::test]
    async fn a_legacy_row_is_dated_by_the_transcript() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        let old = transcript(&paths, "legacy");
        set_mtime(&old, 1_000_000);
        archived_row(&paths, "legacy", PRESENT.to_string());

        purge_archived(&paths, now_ms() - 24 * 60 * 60 * 1000, &|_| false).await;

        assert!(!old.exists());
        assert!(archived_ids(&paths).is_empty());
    }

    #[tokio::test]
    async fn a_row_with_no_transcript_is_dropped_when_this_instance_owns_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        archived_row(&paths, "gone", "1000000".to_string());
        droppen_sessions::owned_sessions::record_as(&paths, droppen_sessions::Source::Claude, "gone")
            .await
            .expect("own");

        let purged = purge_archived(&paths, now_ms(), &|_| false).await;

        assert_eq!(purged.sessions, 1, "the row tidied, and its metadata follows");
        assert_eq!(purged.ids, ["gone"]);
        assert!(archived_ids(&paths).is_empty());
    }

    /// The transcript can be gone while its sidecars are not — a sidecar
    /// removal failed in an earlier pass, or the transcript was deleted by
    /// hand. The owner's pass still retires them.
    #[tokio::test]
    async fn leftovers_of_a_gone_transcript_are_retired() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        archived_row(&paths, "gone", "1000000".to_string());
        droppen_sessions::owned_sessions::record_as(&paths, droppen_sessions::Source::Claude, "gone")
            .await
            .expect("own");
        let sidecars = ClaudeRoots::of(&paths).projects_dir(ClaudeRoot::Managed).join("project").join("gone");
        std::fs::create_dir_all(sidecars.join("tool-results")).expect("sidecars");
        std::fs::write(sidecars.join("tool-results/t.txt"), b"tool").expect("tool result");
        let tasks = crate::sessions::checklist::tasks_dir(&paths, ClaudeRoot::Managed, "gone").expect("tasks dir");
        std::fs::create_dir_all(&tasks).expect("tasks");
        std::fs::write(tasks.join("1.json"), b"task").expect("task");
        let logs = paths.instance_dir().join("session-logs");
        std::fs::create_dir_all(&logs).expect("logs");
        std::fs::write(logs.join("gone.ndjson"), b"log").expect("recording");

        let purged = purge_archived(&paths, now_ms(), &|_| false).await;

        assert_eq!(purged.sessions, 1);
        assert_eq!(purged.bytes, ("tool".len() + "task".len() + "log".len()) as u64);
        assert!(!sidecars.exists());
        assert!(!tasks.exists());
        assert!(!logs.join("gone.ndjson").exists());
        assert!(archived_ids(&paths).is_empty());
    }

    /// The row list is shared by every instance, but another instance's managed
    /// root is not walkable from here: its session must not be un-archived.
    #[tokio::test]
    async fn a_row_with_no_transcript_here_is_left_for_a_sibling() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        archived_row(&paths, "sibling", "1000000".to_string());

        purge_archived(&paths, now_ms(), &|_| false).await;

        assert!(archived_ids(&paths).contains("sibling"));
    }

    #[tokio::test]
    async fn a_skipped_session_keeps_its_transcript() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        let file = transcript(&paths, "live");
        archived_row(&paths, "live", "1000000".to_string());

        purge_archived(&paths, now_ms(), &|id| id == "live").await;

        assert!(file.exists());
        assert!(archived_ids(&paths).contains("live"));
    }

    #[tokio::test]
    async fn archiving_appends_to_the_hidden_list_without_losing_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());

        archive_claude_session(&paths, "one").await.expect("archive");
        archive_claude_session(&paths, "two").await.expect("archive");
        archive_claude_session(&paths, "one").await.expect("idempotent");

        assert_eq!(
            archived_ids(&paths),
            ["one".to_string(), "two".to_string()].into_iter().collect(),
            "a repeat archive is a no-op, and earlier ids survive"
        );
    }

    /// The bug this store moved for. Two archives used to read the same list,
    /// each append their own id and each write the whole thing back, so the
    /// loser's session reappeared. Neither call reads the list any more.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_archives_at_once_do_not_lose_each_other() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        // Open the database before the race, so the two tasks contend over the
        // archive rather than over which of them creates the file.
        let _ = store(&paths);

        let (first, second) = tokio::join!(archive_claude_session(&paths, "one"), archive_claude_session(&paths, "two"));
        first.expect("archive");
        second.expect("archive");

        let hidden = store(&paths).kv_scope(scope::ARCHIVED);
        assert!(hidden.contains_key("one"), "the first archive survived the second");
        assert!(hidden.contains_key("two"), "and the second survived the first");

        // …and the session list agrees, because it reads the same rows.
        assert_eq!(archived_ids(&paths), ["one".to_string(), "two".to_string()].into_iter().collect());
    }

    #[tokio::test]
    async fn an_archive_is_still_there_through_a_fresh_connection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        archive_claude_session(&paths, "one").await.expect("archive");

        let reopened = droppen_instance::db::Db::open(paths.config_db());
        assert!(reopened.kv_scope(scope::ARCHIVED).contains_key("one"));
    }
}
