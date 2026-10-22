//! User-set session titles.
//!
//! Port of the former TypeScript server's `titles.ts`. The titles are kept
//! outside the CLIs' own session files, which are never mutated, and they are
//! the highest-precedence title for both sources — so a rename in remote-hand
//! survives a restart and works for Codex, which has no custom-title mechanism
//! of its own.
//!
//! One row per title in the machine-wide state database ([`crate::db`]) under
//! [`scope::TITLES`], keyed `"<source>:<id>"`. `titles.json` used to be written
//! *in place* — `tokio::fs::write` straight over the live file — so a crash
//! between truncate and write left a file every reader turns into an empty map:
//! every custom title on the machine, gone at once and silently. A row cannot be
//! half there.
//!
//! The TypeScript keeps a write-through in-memory cache so a rename is visible
//! to the very next refresh. Here there is no cache to go stale: every reader
//! asks the database, which is as immediate and cannot disagree with what was
//! committed.
//!
//! `titles.json` is read exactly once, by the import on the first boot after
//! the upgrade, and never written again. It was a mirror for one version, so a
//! `.prev` binary that had never heard of the database would still find the
//! names; the rollback floor has since risen to a build that reads the
//! database, and `crate::sessions::claude::custom_titles` — the last reader
//! that went to the file — now asks [`all`] instead. The file is left where it
//! is rather than deleted: it costs nothing, and it is the cheapest way to seed
//! a database that has to be rebuilt from scratch.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::db::{scope, Db};
use crate::sessions::{Paths, Source};

/// The titles store, backed by `state.db`.
///
/// No write lock: the only mutation is one upserted or deleted row, and two
/// renames cannot lose each other. The lock that used to be here existed to
/// keep two whole-file rewrites of `titles.json` from being applied in the
/// opposite order to the commits they described, and there is no longer a file
/// to rewrite.
pub struct TitlesStore {
    paths: Arc<Paths>,
}

/// The `titles` row one session's name lives in.
///
/// A colon rather than the slash every other durable key uses, because that is
/// what the rows on disk are already keyed by; see
/// [`crate::sessions::SessionRef::title_key`]. Server-free, because a name a
/// person gave a session is theirs and not one server's.
fn key_of(source: Source, id: &str) -> String {
    crate::sessions::reference::title_key(source, id)
}

/// Every title anyone has set, for a caller that holds only a path set.
///
/// Discovery builds the session list from a `&Paths` and has no store to ask,
/// which is the whole reason this exists beside [`TitlesStore::read`] rather
/// than inside it. Both go to the same rows, so a rename is on the list at the
/// next refresh without anything having to be written to disk twice.
pub fn all(paths: &Paths) -> BTreeMap<String, String> {
    let db = crate::db::at(paths.config_db());
    db.import_once(scope::TITLES, || from_file(&paths.titles_file()).into_iter().collect());
    db.kv_scope(scope::TITLES)
}

impl TitlesStore {
    /// A store rooted at the given path set.
    pub fn new(paths: Arc<Paths>) -> Self {
        Self { paths }
    }

    /// Where the file the import drains lives.
    pub fn path(&self) -> PathBuf {
        self.paths.titles_file()
    }

    /// The database, with `titles.json` drained into it if nobody has yet.
    fn db(&self) -> Arc<Db> {
        let db = crate::db::at(self.paths.config_db());
        db.import_once(scope::TITLES, || from_file(&self.path()).into_iter().collect());
        db
    }

    /// Every title anyone has set, keyed `"<source>:<id>"`.
    ///
    /// Public where it used to be private, because it is the authoritative
    /// answer and the file is not — this and [`all`] are what the session list
    /// is built from.
    pub async fn read(&self) -> BTreeMap<String, String> {
        self.db().kv_scope(scope::TITLES)
    }

    /// Set a session's display title. An empty title resets it to the derived one.
    ///
    /// Still `io::Result` because the route's error path is built on it. There
    /// is no I/O left in here that can fail — the rename is one committed row —
    /// so the result is unconditionally `Ok`.
    pub async fn set(&self, source: Source, id: &str, title: &str) -> std::io::Result<()> {
        let db = self.db();
        let key = key_of(source, id);
        let trimmed = title.trim();
        // One row either way, so renaming two sessions at once — from two tabs,
        // or from the two servers that share this directory — can no longer
        // cost one of the two names.
        if trimmed.is_empty() {
            db.kv_remove(scope::TITLES, &key);
        } else {
            db.kv_set(scope::TITLES, &key, trimmed);
        }
        Ok(())
    }
}

/// The file, parsed. Read exactly once per install, by the import.
///
/// Anything that is not a string title is dropped rather than read as one, the
/// tolerance the file reader has always had: a hand-edited file costs its own
/// entry and not the rest of the map.
fn from_file(path: &std::path::Path) -> BTreeMap<String, String> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return BTreeMap::new();
    };
    map.into_iter()
        .filter_map(|(key, value)| Some((key, value.as_str()?.to_string())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_title_round_trips_and_an_empty_one_removes_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = TitlesStore::new(Arc::new(Paths::new(dir.path().to_path_buf())));

        store.set(Source::Claude, "abc", "  Renamed  ").await.expect("set");
        assert_eq!(
            store.read().await.get("claude:abc").map(String::as_str),
            Some("Renamed"),
            "trimmed and keyed by source"
        );

        // A second source keeps its own entry.
        store.set(Source::Codex, "abc", "Other").await.expect("set");
        assert!(store.read().await.contains_key("codex:abc"));

        store.set(Source::Claude, "abc", "   ").await.expect("reset");
        let titles = store.read().await;
        assert!(!titles.contains_key("claude:abc"), "an empty title resets to the derived one");
        assert_eq!(titles.get("codex:abc").map(String::as_str), Some("Other"));
    }

    /// The file is drained on the first look and never consulted again. If it
    /// were, a title the user has since cleared would come back on the next
    /// boot from a file that still remembered it.
    #[tokio::test]
    async fn the_file_is_imported_once_and_then_stops_being_the_truth() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        std::fs::create_dir_all(paths.config_dir()).expect("config dir");
        std::fs::write(paths.titles_file(), r#"{"claude:abc":"From the file","claude:bad":7}"#).expect("seed");

        let store = TitlesStore::new(Arc::new(paths.clone()));
        let titles = store.read().await;
        assert_eq!(titles.get("claude:abc").map(String::as_str), Some("From the file"));
        assert!(!titles.contains_key("claude:bad"), "a title that is not a string costs its own entry");

        std::fs::write(paths.titles_file(), r#"{"claude:zzz":"Edited behind its back"}"#).expect("re-seed");
        let titles = store.read().await;
        assert_eq!(titles.get("claude:abc").map(String::as_str), Some("From the file"));
        assert!(!titles.contains_key("claude:zzz"), "the import does not run a second time");
    }

    /// The file is an input, once, and never an output. A rename that wrote it
    /// back would be a rename that a hand-restored `titles.json` could not
    /// survive being imported from.
    #[tokio::test]
    async fn a_rename_never_writes_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = TitlesStore::new(Arc::new(Paths::new(dir.path().to_path_buf())));
        store.set(Source::Claude, "abc", "Renamed").await.expect("set");
        assert!(!store.path().exists(), "no file is created where there was none");
    }

    #[tokio::test]
    async fn a_pre_existing_file_is_read_and_then_left_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        std::fs::create_dir_all(paths.config_dir()).expect("config dir");
        let before = "{\n  \"claude:abc\": \"From the file\"\n}\n";
        std::fs::write(paths.titles_file(), before).expect("seed");

        let store = TitlesStore::new(Arc::new(paths.clone()));
        store.set(Source::Claude, "abc", "Renamed again").await.expect("set");
        store.set(Source::Codex, "abc", "").await.expect("reset");

        assert_eq!(store.read().await.get("claude:abc").map(String::as_str), Some("Renamed again"));
        assert_eq!(
            std::fs::read_to_string(paths.titles_file()).expect("read"),
            before,
            "byte for byte as it was found, import and renames notwithstanding"
        );
    }

    #[tokio::test]
    async fn a_title_is_still_there_through_a_fresh_connection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        let store = TitlesStore::new(Arc::new(paths.clone()));
        store.set(Source::Codex, "abc", "Renamed").await.expect("set");

        let reopened = crate::db::Db::open(paths.config_db());
        assert_eq!(
            reopened.kv_scope(scope::TITLES).get("codex:abc").map(String::as_str),
            Some("Renamed"),
            "the rename is committed, not held in a cache that dies with the connection"
        );
    }
}
