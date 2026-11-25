//! Deleting archived session data once a retention window has passed.
//!
//! Archiving only hides a session; its transcript stays on disk. This module
//! is the clock that eventually deletes it, from all three sources: Claude
//! Code transcripts, archived Codex threads and archived ACP transcripts. It
//! also forgets the metadata this server kept about a deleted session, so no
//! titles, read marks, pins or chat rows are left pointing at nothing.
//!
//! The pass is hourly and starts only in the server that holds the instance
//! lock; a lock lost since boot stops it doing more. A session some server
//! still has work in flight — a turn, a queued message, a background task — is
//! skipped, whatever its age: the process may write to the transcript again.
//! An idle resident is not a reason to skip: archiving leaves the process
//! alive and, with no idle shutdown configured, it would otherwise keep the
//! transcript for ever. `never` (the default) makes the pass a no-op.

use std::sync::Arc;
use std::time::Duration;

use droppen_base::time::now_ms;
use droppen_http::refresh::Refresh;
use droppen_instance::instance::InstanceLock;
use droppen_instance::paths::Paths;
use droppen_provider::ProviderRegistry;
use droppen_sessions::purge::forget_settled;
use droppen_sessions::{Purged, Source};
use droppen_workbench::prefs::PrefsStore;
use tokio_util::sync::CancellationToken;

/// How long after boot the first pass runs, so a restart is not a burst of
/// deletions the moment the server comes up.
const FIRST_PASS: Duration = Duration::from_secs(60);
/// How often the window is checked after that.
const EVERY: Duration = Duration::from_secs(60 * 60);

/// Run one pass with the stored window.
///
/// Returns what was deleted and what failed; an empty result when the setting
/// is `never`. Every deleted session has its metadata forgotten, and a
/// deletion wakes the session list so the row leaves every attached client.
pub async fn purge_once(paths: &Paths, providers: &ProviderRegistry, refresh: &Refresh) -> Purged {
    let Some(window) = PrefsStore::of(paths).read().await.archived_retention.ms() else {
        return Purged::default();
    };
    let cutoff = now_ms() - window;

    let mut total = Purged::default();
    // Only work in flight keeps a transcript: an idle resident may still be
    // alive after an archive, and skipping on its mere presence would keep the
    // transcript until the server restarts. Codex and ACP archive only from
    // idle and close their driver, so their `resident` is already `None`.
    //
    // Claude's registrations are shared through `~/.claude/sessions`, so a
    // session resumed in somebody's terminal — or by a sibling instance on the
    // shared local root — is in `live` and is not deleted under the running
    // CLI. A registration exists for the whole life of a CLI, idle included,
    // and every resident this server started has one too, so a registration
    // only vetoes when this registry holds no resident for the id: an idle
    // resident of ours is exactly the case retention must still delete.
    let live = droppen_provider_claude_code::sessions::claude::live_claude_sessions(paths).await;
    // Bound once so the source names below stay out of the layer count
    // (`layers.json`'s `sourceMatches`, held by a gate test).
    let (claude_source, acp_source, codex_source) = (Source::Claude, Source::Acp, Source::Codex);
    let claude_skip = |id: &str| {
        work_in_flight(providers, claude_source, id)
            || (live.contains_key(id) && providers.resident_state(claude_source, id).is_none())
    };
    let acp_skip = |id: &str| work_in_flight(providers, acp_source, id);
    let codex_skip = |id: &str| work_in_flight(providers, codex_source, id);

    let claude = droppen_provider_claude_code::sessions::claude::archive::purge_archived(paths, cutoff, &claude_skip).await;
    let acp = droppen_provider_acp::sessions::purge_archived(paths, cutoff, &acp_skip).await;
    let codex = droppen_provider_codex::sessions::purge::purge_archived(paths, cutoff, &codex_skip).await;

    for (source, purged) in [(Source::Claude, claude), (Source::Acp, acp), (Source::Codex, codex)] {
        for id in &purged.ids {
            forget_settled(paths, source, id).await;
        }
        total.absorb(purged);
    }

    if total.sessions > 0 {
        refresh.request_refresh(None);
    }
    if total.sessions > 0 || total.failed > 0 {
        tracing::info!(
            sessions = total.sessions,
            bytes = total.bytes,
            failed = total.failed,
            "archived session retention pass"
        );
    }
    total
}

/// Whether this server has work in flight for the session, which the CLI could
/// still write to.
fn work_in_flight(providers: &ProviderRegistry, source: Source, id: &str) -> bool {
    providers.resident_state(source, id).is_some_and(|state| {
        state.phase != droppen_provider::Phase::Idle || state.queued > 0 || state.background > 0 || state.awaiting_background
    })
}

/// Start the hourly pass. Returns immediately; the task stops on `cancel`.
///
/// `held` is this server's instance lock, checked before each pass so a server
/// whose claim was taken over stops deleting under the new owner.
pub fn spawn(
    held: Arc<InstanceLock>,
    paths: Arc<Paths>,
    providers: Arc<ProviderRegistry>,
    refresh: Refresh,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + FIRST_PASS, EVERY);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = ticks.tick() => {
                    if let Err(lost) = held.verify_still_held() {
                        tracing::warn!(%lost, "instance lock lost; skipping the archived retention pass");
                        continue;
                    }
                    purge_once(&paths, &providers, &refresh).await;
                }
            }
        }
        tracing::debug!("archived retention stopped");
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use droppen_provider::testing::injection_registry;
    use droppen_sessions::reference::title_key;
    use droppen_workbench::prefs::{ArchivedRetention, Prefs};

    const CLAUDE_OLD: &str = "claude-old";
    const CLAUDE_NEW: &str = "claude-new";
    const CLAUDE_LIVE: &str = "claude-live";
    const ACP_OLD: &str = "acp-old";
    const ACP_NEW: &str = "acp-new";
    const CODEX_OLD: &str = "0199c1a1-2b3c-4d5e-8f60-000000000001";
    const CODEX_NEW: &str = "0199c1a1-2b3c-4d5e-8f60-000000000002";

    fn home() -> (tempfile::TempDir, droppen_base::process_env::ChildPathGuard, Paths) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(".rh-test-root"), b"").expect("test root");
        let guard = droppen_base::process_env::ChildPathGuard::take();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).expect("bin");
        let codex = bin.join("codex");
        let fake = concat!(env!("CARGO_MANIFEST_DIR"), "/../droppen-provider-codex/tests/support/fake_codex.py");
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

    fn write(path: &Path, body: &[u8]) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, body).expect("file");
    }

    fn backdate(path: &Path) {
        let epoch = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        std::fs::File::options()
            .write(true)
            .open(path)
            .expect("open")
            .set_modified(epoch)
            .expect("mtime");
    }

    fn claude(paths: &Paths, id: &str, value: &str) -> PathBuf {
        let dir = droppen_provider_claude_code::claude_roots::ClaudeRoots::of(paths)
            .projects_dir(droppen_instance::paths::ClaudeRoot::Managed)
            .join("p");
        let file = dir.join(format!("{id}.jsonl"));
        write(&file, b"claude body");
        droppen_instance::db::at(paths.config_db()).kv_set(droppen_instance::db::scope::ARCHIVED, id, value);
        file
    }

    fn acp(paths: &Paths, id: &str) -> PathBuf {
        let file = paths
            .instance_dir()
            .join("acp-sessions")
            .join("agent")
            .join("archived")
            .join(format!("{id}.jsonl"));
        write(&file, b"acp body");
        file
    }

    fn codex(paths: &Paths) -> (PathBuf, PathBuf) {
        let archived = paths.codex_dir().join("archived_sessions");
        let old = archived.join(format!("rollout-{CODEX_OLD}.jsonl"));
        let new = archived.join(format!("rollout-{CODEX_NEW}.jsonl"));
        write(&old, b"codex old");
        write(&new, b"codex new");
        let db = paths.codex_dir().join("state_5.sqlite");
        let connection = rusqlite::Connection::open(db).expect("state db");
        connection
            .execute_batch(
                "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT, archived INTEGER, updated_at INTEGER, archived_at INTEGER);",
            )
            .expect("schema");
        connection
            .execute(
                "INSERT INTO threads VALUES (?1, ?2, 1, 0, 1000)",
                rusqlite::params![CODEX_OLD, old.to_string_lossy()],
            )
            .expect("old row");
        connection
            .execute(
                "INSERT INTO threads VALUES (?1, ?2, 1, 0, ?3)",
                rusqlite::params![CODEX_NEW, new.to_string_lossy(), now_ms() / 1000],
            )
            .expect("new row");
        (old, new)
    }

    /// Seven old-or-new files: the old three must go, the new three and the
    /// live one stay.
    fn fixture(paths: &Paths) -> [PathBuf; 7] {
        let claude_old = claude(paths, CLAUDE_OLD, "1000");
        let claude_new = claude(paths, CLAUDE_NEW, &now_ms().to_string());
        let acp_old = acp(paths, ACP_OLD);
        let acp_new = acp(paths, ACP_NEW);
        backdate(&acp_old);
        let (codex_old, codex_new) = codex(paths);
        // A third old claude session a CLI is still running: its registration
        // in the shared `sessions` directory keeps it this pass.
        let live = claude(paths, CLAUDE_LIVE, "1000");
        let registrations = paths.claude_root(droppen_instance::paths::ClaudeRoot::Local).join("sessions");
        std::fs::create_dir_all(&registrations).expect("registrations");
        std::fs::write(
            registrations.join("live.json"),
            serde_json::json!({"pid": std::process::id(), "sessionId": CLAUDE_LIVE, "startedAt": 0}).to_string(),
        )
        .expect("registration");
        // Titles on the old sessions, which the pass must forget.
        for (source, id) in [(Source::Claude, CLAUDE_OLD), (Source::Acp, ACP_OLD), (Source::Codex, CODEX_OLD)] {
            droppen_instance::db::at(paths.config_db()).kv_set(droppen_instance::db::scope::TITLES, &title_key(source, id), "name");
        }
        [claude_old, claude_new, acp_old, acp_new, codex_old, codex_new, live]
    }

    fn providers(home: &Path) -> Arc<ProviderRegistry> {
        ProviderRegistry::new(injection_registry(home), |_, _| {})
    }

    async fn with_pref(paths: &Paths, retention: ArchivedRetention) {
        PrefsStore::of(paths)
            .write(&Prefs {
                archived_retention: retention,
                ..Prefs::default()
            })
            .await
            .expect("prefs");
    }

    fn titles(paths: &Paths) -> Vec<String> {
        droppen_instance::db::at(paths.config_db())
            .kv_scope(droppen_instance::db::scope::TITLES)
            .into_keys()
            .collect()
    }

    #[tokio::test]
    async fn a_day_window_deletes_the_old_data_from_every_source() {
        let (dir, _guard, paths) = home();
        let [claude_old, claude_new, acp_old, acp_new, codex_old, codex_new, live] = fixture(&paths);
        with_pref(&paths, ArchivedRetention::Day1).await;
        let providers = providers(dir.path());
        let refresh = Refresh::new(droppen_drive::feed::global::GlobalFeed::new());

        let purged = purge_once(&paths, &providers, &refresh).await;

        assert_eq!(purged.sessions, 3, "one per source, all old");
        assert_eq!(purged.failed, 0);
        for gone in [&claude_old, &acp_old, &codex_old] {
            assert!(!gone.exists(), "{} is gone", gone.display());
        }
        for kept in [&claude_new, &acp_new, &codex_new, &live] {
            assert!(kept.exists(), "{} stays", kept.display());
        }
        assert!(titles(&paths).is_empty(), "the deleted sessions' metadata went with them");
        assert!(droppen_provider_claude_code::sessions::claude::archive::archived_ids(&paths).contains(CLAUDE_NEW));
    }

    #[tokio::test]
    async fn never_deletes_nothing() {
        let (dir, _guard, paths) = home();
        let files = fixture(&paths);
        with_pref(&paths, ArchivedRetention::Never).await;
        let providers = providers(dir.path());
        let refresh = Refresh::new(droppen_drive::feed::global::GlobalFeed::new());

        let purged = purge_once(&paths, &providers, &refresh).await;

        assert_eq!(purged, Purged::default());
        assert!(files.iter().all(|file| file.exists()));
    }
}
