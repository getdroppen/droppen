//! Instance-local sidebar pins; never modify the CLI's transcript or shared titles.
use droppen_instance::db::scope;
use droppen_instance::paths::Paths;
use crate::sessions::reference::local_key;
use crate::sessions::{SessionScope, SessionSummary, Source};

/// The `kv` key a pin is a row under.
///
/// [`local_key`] and not [`crate::sessions::SessionRef::scoped_key`]: these
/// rows already live in the *instance* database, so naming the server again
/// would only be saying it twice — and would orphan every pin already set.
fn key(source: Source, id: &str) -> String {
    local_key(source, id)
}

/// One atomic row per session, so concurrent pins cannot overwrite each other.
pub fn set(paths: &Paths, source: Source, id: &str, pinned: bool) -> rusqlite::Result<()> {
    droppen_instance::db::at(paths.instance_db()).write(|tx| {
        let key = key(source, id);
        if pinned {
            tx.execute(
                "INSERT OR REPLACE INTO kv (scope, key, value) VALUES (?1, ?2, '1')",
                (scope::SESSION_PINS, key),
            )?;
        } else {
            tx.execute("DELETE FROM kv WHERE scope = ?1 AND key = ?2", (scope::SESSION_PINS, key))?;
        }
        Ok(())
    })
}

/// Overlay current instance pins, including clearing stale cached values.
pub fn apply(paths: &Paths, sessions: &mut [SessionSummary]) {
    let pins = droppen_instance::db::at(paths.instance_db()).kv_scope(scope::SESSION_PINS);
    for session in sessions {
        session.pinned = session.scope == SessionScope::Owned && pins.contains_key(&key(session.source, &session.id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_are_durable_independent_rows_scoped_by_instance_and_source() {
        let home = tempfile::tempdir().unwrap();
        let a = Paths::for_instance(home.path().to_owned(), "a");
        let b = Paths::for_instance(home.path().to_owned(), "b");
        set(&a, Source::Claude, "same", true).unwrap();
        set(&a, Source::Codex, "same", true).unwrap();
        set(&a, Source::Claude, "other", true).unwrap();
        set(&a, Source::Claude, "same", true).unwrap();
        set(&a, Source::Claude, "same", false).unwrap();
        let reopened = droppen_instance::db::Db::open(a.instance_db());
        assert_eq!(reopened.kv_scope(scope::SESSION_PINS).len(), 2);
        assert!(reopened.kv_get(scope::SESSION_PINS, "claude/same").is_none());
        assert_eq!(reopened.kv_get(scope::SESSION_PINS, "codex/same").as_deref(), Some("1"));
        assert!(droppen_instance::db::at(b.instance_db()).kv_scope(scope::SESSION_PINS).is_empty());
    }
}
