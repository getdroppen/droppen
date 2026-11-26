//! Deleting archived Codex threads once their retention window has passed.
//!
//! Codex owns the archive: the `threads` row says `archived = 1` and the
//! rollout lives under `archived_sessions`. Deleting the file alone would
//! leave the row and the title behind, so deletion goes through
//! `codex delete --force`, the same official CLI the archive itself uses. The
//! `--force` is required: without a TTY, plain `delete` asks for confirmation
//! and waits for an answer no one gives.
//!
//! A failed delete leaves the row in place and counts as `failed`, so the next
//! hourly pass retries it. Nothing here touches a thread a server still has
//! resident: the caller's `skip` decides that.

use std::path::Path;
use std::time::Duration;

use droppen_base::process_env::{apply_env, clean_env_bare};
use droppen_instance::paths::Paths;
use droppen_sessions::Purged;

use super::codex::{open_state_db, CodexHome};

/// How long `codex delete` may take before the pass gives up on it.
const DELETE_TIMEOUT: Duration = Duration::from_secs(15);

/// One archived thread, with the instant to date it by: its archive time, or
/// its last update for a row Codex archived before it recorded one.
struct ArchivedThread {
    id: String,
    rollout_path: String,
    archived_at_ms: i64,
}

/// The archived threads, read off the runtime's worker threads because SQLite
/// is a blocking API.
fn archived_threads(db: &Path) -> Vec<ArchivedThread> {
    let Some(connection) = open_state_db(db) else { return Vec::new() };
    // An older Codex has no `archived_at`; its last update is the best date
    // there is.
    let mut statement = match connection
        .prepare("SELECT id, rollout_path, COALESCE(archived_at, updated_at) * 1000 FROM threads WHERE archived = 1")
        .or_else(|_| connection.prepare("SELECT id, rollout_path, updated_at * 1000 FROM threads WHERE archived = 1"))
    {
        Ok(statement) => statement,
        Err(_) => return Vec::new(),
    };
    let rows = statement.query_map([], |row| {
        Ok(ArchivedThread {
            id: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
            rollout_path: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            archived_at_ms: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
        })
    });
    match rows {
        Ok(rows) => rows.filter_map(Result::ok).filter(|thread| !thread.id.is_empty()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Delete archived threads whose archive instant is at or before `cutoff_ms`.
///
/// `skip` is asked for each id that would otherwise go, so a thread some
/// server is still driving keeps its rollout.
pub async fn purge_archived(paths: &Paths, cutoff_ms: i64, skip: &(dyn Fn(&str) -> bool + Sync)) -> Purged {
    let db = CodexHome::of(paths).state_db();
    let threads = tokio::task::spawn_blocking(move || archived_threads(&db)).await.unwrap_or_default();

    let mut purged = Purged::default();
    for thread in threads {
        if thread.archived_at_ms > cutoff_ms || skip(&thread.id) {
            continue;
        }
        // `codex delete --force` only takes a UUID. A thread named otherwise
        // cannot be deleted through it; leaving the row means a warning each
        // pass rather than a silent no-op every time.
        if !is_uuid(&thread.id) {
            tracing::warn!(session = %thread.id, "an archived codex thread has no UUID; leaving it for a retry");
            purged.failed += 1;
            continue;
        }
        // A compressed rollout names the plain path in its row; count whichever
        // form is on disk.
        let plain = Path::new(&thread.rollout_path);
        let bytes = [Some(plain.to_path_buf()), Some(droppen_sessions::compressed::compressed_path(plain))]
            .into_iter()
            .flatten()
            .filter_map(|path| std::fs::metadata(path).ok())
            .map(|metadata| metadata.len())
            .sum();
        match delete(paths, &thread.id).await {
            Ok(()) => purged.add(&thread.id, bytes),
            Err(error) => {
                // Another server sharing `~/.codex` may have deleted the row
                // between our read and our call. A row that is no longer there
                // is a success, not a failure to retry every hour.
                let db = CodexHome::of(paths).state_db();
                let id = thread.id.clone();
                let gone = tokio::task::spawn_blocking(move || still_archived(&db, &id))
                    .await
                    .ok()
                    .flatten()
                    == Some(false);
                if gone {
                    purged.add(&thread.id, bytes);
                } else {
                    tracing::warn!(session = %thread.id, %error, "cannot delete an archived codex thread");
                    purged.failed += 1;
                }
            }
        }
    }
    purged
}

/// Whether `id` is a UUID: the only form `codex delete --force` accepts.
fn is_uuid(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

/// Whether `id` is still an archived thread; `None` when the database cannot be
/// read, in which case a failed delete is left to the next pass.
fn still_archived(db: &Path, id: &str) -> Option<bool> {
    let connection = open_state_db(db)?;
    match connection.query_row("SELECT 1 FROM threads WHERE id = ?1 AND archived = 1", [id], |_| Ok(())) {
        Ok(()) => Some(true),
        Err(rusqlite::Error::QueryReturnedNoRows) => Some(false),
        // Busy or unreadable: do not call a live row gone.
        Err(_) => None,
    }
}

/// Run `codex delete --force <id>` in the user's home with the clean child
/// environment the other CLI calls use.
async fn delete(paths: &Paths, id: &str) -> Result<(), String> {
    let mut command = tokio::process::Command::new("codex");
    command.args(["delete", "--force", id]).current_dir(&paths.home).kill_on_drop(true);
    apply_env(&mut command, &clean_env_bare());

    let output = tokio::time::timeout(DELETE_TIMEOUT, command.output()).await;
    match output {
        Ok(Ok(output)) if output.status.success() => Ok(()),
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let trimmed = stderr.trim();
            Err(if trimmed.is_empty() {
                format!("codex exited with {}", output.status)
            } else {
                trimmed.lines().last().unwrap_or(trimmed).to_string()
            })
        }
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("the CLI did not answer within 15 seconds".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use droppen_base::time::now_ms;

    const OLD: &str = "0199c1a1-2b3c-4d5e-8f60-000000000001";
    const NEW: &str = "0199c1a1-2b3c-4d5e-8f60-000000000002";

    #[test]
    fn only_the_uuid_form_is_accepted() {
        assert!(is_uuid("0199c1a1-2b3c-4d5e-8f60-000000000001"));
        assert!(!is_uuid("thread-one"));
        assert!(!is_uuid("0199c1a1-2b3c-4d5e-8f60-00000000001"));
        assert!(!is_uuid("z199c1a1-2b3c-4d5e-8f60-000000000001"));
    }

    /// A home with the test-root marker, a fake `codex` on the child `PATH`,
    /// and its archive directory.
    fn home() -> (tempfile::TempDir, droppen_base::process_env::ChildPathGuard, Paths) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(".rh-test-root"), b"").expect("test root");
        let guard = droppen_base::process_env::ChildPathGuard::take();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).expect("bin");
        let codex = bin.join("codex");
        let fake = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/fake_codex.py");
        std::fs::write(&codex, format!("#!/bin/sh\nexec python3 {fake} \"$@\"\n")).expect("codex");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let path = format!("{}:{}", bin.display(), droppen_base::process_env::child_path());
        droppen_base::process_env::set_child_path(Some(path));
        let paths = Paths::new(dir.path().to_path_buf());
        (dir, guard, paths)
    }

    /// A state database with one old and one recent archived thread, and their
    /// rollouts where the fake deletes from.
    fn archive(paths: &Paths) -> (std::path::PathBuf, std::path::PathBuf) {
        let archived = paths.codex_dir().join("archived_sessions");
        std::fs::create_dir_all(&archived).expect("archive dir");
        let old = archived.join(format!("rollout-2026-01-01-{OLD}.jsonl"));
        let new = archived.join(format!("rollout-2026-01-02-{NEW}.jsonl"));
        std::fs::write(&old, b"old body").expect("old");
        std::fs::write(&new, b"new body").expect("new");

        let connection = rusqlite::Connection::open(CodexHome::of(paths).state_db()).expect("state db");
        connection
            .execute_batch(
                "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT, archived INTEGER, updated_at INTEGER, archived_at INTEGER);",
            )
            .expect("schema");
        connection
            .execute(
                "INSERT INTO threads VALUES (?1, ?2, 1, 0, 1000)",
                rusqlite::params![OLD, old.to_string_lossy()],
            )
            .expect("old row");
        connection
            .execute(
                "INSERT INTO threads VALUES (?1, ?2, 1, 0, ?3)",
                rusqlite::params![NEW, new.to_string_lossy(), now_ms() / 1000],
            )
            .expect("new row");
        (old, new)
    }

    #[tokio::test]
    async fn only_archived_threads_past_the_cutoff_are_deleted() {
        let (dir, _guard, paths) = home();
        let (old, new) = archive(&paths);

        let purged = purge_archived(&paths, now_ms() - 86_400_000, &|_| false).await;

        assert_eq!(purged.sessions, 1);
        assert_eq!(purged.ids, [OLD]);
        assert_eq!(purged.failed, 0);
        assert!(!old.exists(), "the old rollout is gone");
        assert!(new.exists(), "the recent one stays");
        let deleted = std::fs::read_to_string(dir.path().join("codex-deleted.txt")).expect("deletion log");
        assert_eq!(deleted.trim(), OLD);
    }

    #[tokio::test]
    async fn a_skipped_thread_survives() {
        let (_dir, _guard, paths) = home();
        let (old, _new) = archive(&paths);

        purge_archived(&paths, now_ms(), &|id| id == OLD).await;

        assert!(old.exists());
    }

    #[tokio::test]
    async fn a_failing_cli_counts_as_failed_and_keeps_the_row() {
        let (dir, _guard, paths) = home();
        let (old, _new) = archive(&paths);
        // The fake exits non-zero where this file is.
        std::fs::write(dir.path().join("codex-delete-exit.txt"), b"1").expect("failure flag");

        let purged = purge_archived(&paths, now_ms(), &|_| false).await;

        assert_eq!((purged.sessions, purged.failed), (0, 2), "both archived threads are left for a retry");
        assert!(old.exists());
    }

    /// Two servers share `~/.codex`: the other instance deleted the row just
    /// before our `codex delete` ran, so its failure is not ours to retry.
    #[tokio::test]
    async fn a_row_another_server_deleted_is_not_counted_as_failed() {
        let (dir, _guard, paths) = home();
        let (old, _new) = archive(&paths);
        // The fake fails both calls, and only for the first deletes its row.
        std::fs::write(dir.path().join("codex-delete-exit.txt"), b"after").expect("failure flag");

        let purged = purge_archived(&paths, now_ms(), &|_| false).await;

        assert_eq!(purged.sessions, 1, "the one whose row went is a success");
        assert_eq!(purged.failed, 1, "the other is still there to retry");
        assert_eq!(purged.ids, [OLD]);
        // The fake did not get to remove the file; the row is what decides.
        assert!(old.exists());
    }
}
