//! Deleting archived session data, and the record a retention pass keeps.
//!
//! Archiving hides a session; the transcript stays on disk for ever. The
//! retention pass ([`crate::Source`]-agnostic, driven by the server) deletes
//! what an archived session left behind once its window has passed: the
//! transcript or thread through each provider's own purge, and the metadata
//! this server keeps about it through [`forget_settled`].
//!
//! Only local data goes. Uploads and attachments are shared between sessions
//! and are swept by their own code; a session's chat claims in the shared
//! `<data>/codex-claims` are left to the boot prune, which already drops a
//! claim whose thread is gone.

use droppen_instance::db::scope;
use droppen_instance::paths::Paths;

use crate::reference::{local_key, title_key};
use crate::Source;

/// What one purge pass deleted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Purged {
    /// Sessions whose data was deleted.
    pub sessions: usize,
    /// Bytes deleted, as the files were before they went.
    pub bytes: u64,
    /// Sessions a provider could not delete, left for the next pass.
    pub failed: usize,
    /// The ids that were deleted, so their metadata can follow.
    pub ids: Vec<String>,
}

impl Purged {
    /// Record one deleted session, its size and its id.
    pub fn add(&mut self, id: &str, bytes: u64) {
        self.sessions += 1;
        self.bytes = self.bytes.saturating_add(bytes);
        self.ids.push(id.to_string());
    }

    /// Fold another pass's outcome into this one.
    pub fn absorb(&mut self, other: Purged) {
        self.sessions += other.sessions;
        self.bytes = self.bytes.saturating_add(other.bytes);
        self.failed += other.failed;
        self.ids.extend(other.ids);
    }
}

/// Delete the metadata this server keeps about one session.
///
/// The name the user gave it, its read mark, its composer settings, its pin,
/// its chat attachment rows and its ownership entry. Each store is a row keyed
/// by the session, so this is what stops them becoming orphans once the
/// transcript is gone. Best effort: a store that will not answer is logged and
/// the rest still go.
pub async fn forget_settled(paths: &Paths, source: Source, id: &str) {
    let key = local_key(source, id);
    let config = droppen_instance::db::at(paths.config_db());
    config.kv_remove(scope::TITLES, &title_key(source, id));
    config.kv_remove(scope::SEEN, &key);
    config.kv_remove(scope::COMPOSER, &key);

    let instance = droppen_instance::db::at(paths.instance_db());
    instance.kv_remove(scope::SESSION_PINS, &key);
    if let Err(error) = instance.write(|tx| tx.execute("DELETE FROM chat_deliveries WHERE source = ?1 AND session_id = ?2", (source.as_str(), id))) {
        tracing::warn!(session = %id, %error, "cannot forget a session's chat deliveries");
    }

    // `codex-claims` is shared and lives under `<data>`, not in either
    // database; the boot prune drops a claim whose thread has gone, so it is
    // deliberately not touched here.
    if let Err(error) = crate::owned_sessions::forget(paths, source, id).await {
        tracing::warn!(session = %id, %error, "cannot forget a session's ownership entry");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_rows(paths: &Paths, source: Source, id: &str) {
        let key = local_key(source, id);
        let config = droppen_instance::db::at(paths.config_db());
        config.kv_set(scope::TITLES, &title_key(source, id), "a name");
        config.kv_set(scope::SEEN, &key, "2026-01-01T00:00:00.000Z");
        config.kv_set(scope::COMPOSER, &key, "{}");
        let instance = droppen_instance::db::at(paths.instance_db());
        instance.kv_set(scope::SESSION_PINS, &key, "1");
        instance
            .write(|tx| {
                tx.execute(
                    "INSERT INTO chat_deliveries (source, session_id, delivery_id, text_hash, at_ms, images, files)
                     VALUES (?1, ?2, 'd', 'h', 0, '[]', '[]')",
                    (source.as_str(), id),
                )
            })
            .expect("delivery");
    }

    fn has_rows(paths: &Paths, source: Source, id: &str) -> bool {
        let key = local_key(source, id);
        droppen_instance::db::at(paths.config_db())
            .kv_get(scope::TITLES, &title_key(source, id))
            .is_some()
            || droppen_instance::db::at(paths.config_db()).kv_get(scope::SEEN, &key).is_some()
            || droppen_instance::db::at(paths.config_db()).kv_get(scope::COMPOSER, &key).is_some()
            || droppen_instance::db::at(paths.instance_db()).kv_get(scope::SESSION_PINS, &key).is_some()
    }

    #[tokio::test]
    async fn forgetting_one_session_leaves_the_other_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        set_rows(&paths, Source::Claude, "gone");
        set_rows(&paths, Source::Claude, "kept");

        forget_settled(&paths, Source::Claude, "gone").await;

        assert!(
            !has_rows(&paths, Source::Claude, "gone"),
            "every metadata row of the deleted session is gone"
        );
        let deliveries = droppen_instance::db::at(paths.instance_db())
            .read(|conn| {
                conn.query_row("SELECT count(*) FROM chat_deliveries WHERE session_id = 'gone'", [], |row| {
                    row.get::<_, i64>(0)
                })
            })
            .expect("count");
        assert_eq!(deliveries, 0);
        assert!(has_rows(&paths, Source::Claude, "kept"), "the kept session's rows survive");

        assert_eq!(droppen_instance::db::at(paths.config_db()).kv_get(scope::TITLES, "claude:gone"), None);
        assert_eq!(
            droppen_instance::db::at(paths.config_db())
                .kv_get(scope::TITLES, "claude:kept")
                .as_deref(),
            Some("a name")
        );
    }

    #[test]
    fn absorb_adds_every_part() {
        let mut total = Purged::default();
        total.add("one", 10);
        total.absorb(Purged {
            sessions: 2,
            bytes: 5,
            failed: 1,
            ids: vec!["two".into(), "three".into()],
        });
        assert_eq!(
            total,
            Purged {
                sessions: 3,
                bytes: 15,
                failed: 1,
                ids: vec!["one".into(), "two".into(), "three".into()],
            }
        );
    }
}
