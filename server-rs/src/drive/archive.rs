//! Archiving a session out of the list.
//!
//! Port of the former TypeScript server's `archive.ts`. Claude has no native
//! archive, so remote-hand keeps its own hidden list; Codex has one, and it is
//! driven through the official CLI so the Codex app stays in sync.
//!
//! The hidden list is one row per archived id in the machine-wide state database
//! ([`crate::db`]) under [`scope::ARCHIVED`]. It is the store this migration is
//! most obviously for: `archived.json` was read-modify-written with **no lock of
//! any kind** — not even the process-local mutex the other sidecars take — so
//! two archives in flight each read the same list, each appended their own id,
//! and the second write threw the first one away. The session came back. An
//! upsert of one row cannot do that, to another tab or to the other server
//! sharing this directory.
//!
//! The value in each row is unused: the *presence* of the row is the fact, the
//! way membership of a JSON array was.
//!
//! `archived.json` was a mirror for one version, so a `.prev` binary that had
//! never heard of the database would still hide the same sessions; the rollback
//! floor has since risen to a build that reads the database, and
//! `crate::sessions::claude::archived_ids` — the last reader that went to the
//! file — now comes back through [`archived_ids`] here. Nothing reads the file
//! any more.

use std::sync::Arc;
use std::time::Duration;

use crate::db::{scope, Db};
use crate::process_env::{apply_env, clean_env_bare};
use crate::sessions::Paths;

/// What goes in the value column. Nothing reads it; the row is the fact.
const PRESENT: &str = "1";

/// The `codex archive` call failed, with the CLI's own words where it had any.
#[derive(Debug, Clone, thiserror::Error)]
#[error("codex archive failed: {0}")]
pub struct ArchiveError(pub String);

const CODEX_ARCHIVE_TIMEOUT: Duration = Duration::from_secs(15);

/// The machine-wide database the hidden list lives in.
fn store(paths: &Paths) -> Arc<Db> {
    crate::db::at(paths.config_db())
}

/// Every id the user has archived out of the list.
///
/// The read half of this module, and the reason the write half stopped being a
/// read-modify-write of a whole file. Discovery calls it on every refresh and it
/// answers from the table, which is the only place an archive is recorded.
pub fn archived_ids(paths: &Paths) -> std::collections::HashSet<String> {
    store(paths).kv_scope(scope::ARCHIVED).into_keys().collect()
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
    store(paths).kv_set(scope::ARCHIVED, id, PRESENT);
    Ok(())
}

/// Archive an unloaded Codex session. Loaded sessions must go through
/// `InjectionRegistry::archive_codex`, which routes to their owning App Server.
pub async fn archive_codex_session(paths: &Arc<Paths>, id: &str) -> Result<(), ArchiveError> {
    let mut command = tokio::process::Command::new("codex");
    command.args(["archive", id]).current_dir(&paths.home).kill_on_drop(true);
    apply_env(&mut command, &clean_env_bare());

    let output: Result<std::io::Result<std::process::Output>, _> = tokio::time::timeout(CODEX_ARCHIVE_TIMEOUT, command.output()).await;
    match output {
        Ok(Ok(output)) if output.status.success() => Ok(()),
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let trimmed = stderr.trim();
            let message = if trimmed.is_empty() {
                format!("codex exited with {}", output.status)
            } else {
                let lines: Vec<&str> = trimmed.split('\n').collect();
                lines[lines.len().saturating_sub(2)..].join(" ")
            };
            Err(ArchiveError(message))
        }
        Ok(Err(error)) => Err(ArchiveError(error.to_string())),
        Err(_) => Err(ArchiveError("the CLI did not answer within 15 seconds".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(
            crate::sessions::claude::archived_ids(&paths).await.contains("two"),
            "discovery asks the same table"
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
        assert_eq!(
            crate::sessions::claude::archived_ids(&paths).await,
            ["one".to_string(), "two".to_string()].into_iter().collect()
        );
    }

    #[tokio::test]
    async fn an_archive_is_still_there_through_a_fresh_connection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        archive_claude_session(&paths, "one").await.expect("archive");

        let reopened = crate::db::Db::open(paths.config_db());
        assert!(reopened.kv_scope(scope::ARCHIVED).contains_key("one"));
    }
}
