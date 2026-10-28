//! The stores that are a map, and the one-time import that fills them.
//!
//! Six of the things this migration replaces are the same store wearing
//! different names: a flat map from a string to a small value, read whole,
//! written whole, and reset to empty by a single bad byte. The preferences, the
//! titles someone typed, the archived list, the read marks, the composer's
//! per-session settings, the chat provider. None of them is ever *queried* —
//! they are fetched by key, or fetched entirely — so none of them earns a table.
//! They earn a row in this one.
//!
//! What they gain by moving is exactly the three things the module note claims,
//! and it is worth being concrete about which bug each one closes:
//!
//! * `archived.json` is read-modify-written with **no lock at all**
//!   (`crate::drive::archive`), so two archives in flight lose one. An `UPSERT`
//!   of one row cannot lose the other.
//! * `titles.json`, `prefs.json` and `composer-prefs.json` are written **in
//!   place** — `tokio::fs::write` straight over the live file — so a crash mid
//!   write truncates them, and every reader turns a truncated file into an empty
//!   one. Every custom title, silently, at once. A transaction has no such
//!   state.
//! * All six live in `~/.config/remote-hand`, which **both servers on this
//!   machine share**, guarded only by a mutex private to each process. The
//!   loser of a race loses their edit and is never told.
//!
//! Two caps also stop being necessary. `seen` keeps 500 entries and
//! `composer-prefs` keeps 300, not because anyone wanted a limit but because the
//! whole file is rewritten on every change and it had to stop growing somewhere.
//! A row does not have that problem.

use std::collections::BTreeMap;

use super::Db;

/// Where each migrated map lives inside [`super::SCHEMA`]'s `kv` table.
///
/// Named constants rather than string literals at the call sites: a typo in a
/// scope is a store that silently reads as empty, which is the exact failure
/// this migration exists to end.
pub mod scope {
    /// Instance-local `source/id` rows for pinned sidebar sessions.
    pub const SESSION_PINS: &str = "session-pins";
    /// `prefs.json` — five UI settings, one row each.
    pub const PREFS: &str = "prefs";
    /// `titles.json` — `"<source>:<id>"` to the name a person gave it.
    pub const TITLES: &str = "titles";
    /// `archived.json` — one row per hidden claude session; the value is unused.
    pub const ARCHIVED: &str = "archived";
    /// `seen.json` — `"<source>/<id>"` to an ISO-8601 read mark.
    pub const SEEN: &str = "seen";
    /// `composer-prefs.json` — the defaults, and one row per session.
    pub const COMPOSER: &str = "composer";
    /// `chat-provider.json` — base URL, key and model for the BYO-API chat.
    pub const CHAT_PROVIDER: &str = "chat-provider";
    /// The configured ACP agents, one row per agent id: its name, command
    /// line and extra environment, as JSON ([`crate::acp_agent`]).
    pub const ACP_AGENTS: &str = "acp-agents";
    /// `notify.json` — the Bark channel's settings and its HMAC secret.
    pub const NOTIFY: &str = "notify";
    /// `push.json`'s `vapidPrivate`, and nothing else from that file.
    ///
    /// The subscriptions in it have a table of their own, because there are many
    /// and they are keyed; the VAPID private scalar is one string, and a table
    /// with one row in it is a worse `kv` than `kv`. It lands here rather than
    /// beside the subscriptions for the reason [`super::Db::keep_to_ourselves`]
    /// gives: it is a private key, and this file is `0600` because of values
    /// like it.
    pub const PUSH: &str = "push";
    /// The screen list's own bookkeeping — today one row, `list-revision`,
    /// bumped by every create, delete and reorder (see [`crate::screens`]).
    pub const SCREENS: &str = "screens";
    /// Which plugins this instance turned on or off, one row per plugin id:
    /// `{"enabled", "by", "at", "reason"}` (see [`crate::plugins`]). Instance
    /// state only — never the machine-wide database two instances share.
    pub const PLUGINS: &str = "plugins";
    /// The first-run guide's own state ([`crate::setup`]): `progress` (where
    /// the guide stands), `pathOverlay` (the directories the user adopted for
    /// a CLI found off the `PATH`), `tasks` (the last install, update or
    /// sign-in per tool) and `practiceFolder` (the practice folder step 03
    /// made last). Instance state: each instance has its own guide.
    pub const SETUP: &str = "setup";
    /// Not a store: the record of which JSON files have already been drained.
    pub const IMPORTED: &str = "imported";
}

impl Db {
    /// One value, or nothing.
    pub fn kv_get(&self, scope: &str, key: &str) -> Option<String> {
        self.read(|conn| {
            conn.query_row("SELECT value FROM kv WHERE scope = ?1 AND key = ?2", (scope, key), |row| {
                row.get::<_, String>(0)
            })
        })
        .ok()
    }

    /// Every key in one scope, in key order.
    ///
    /// `BTreeMap` rather than `HashMap` so the order is the same twice running:
    /// several of these maps are serialised straight back out to a client, and a
    /// response that reshuffles itself between identical requests defeats every
    /// cache in front of it.
    pub fn kv_scope(&self, scope: &str) -> BTreeMap<String, String> {
        self.read(|conn| {
            let mut statement = conn.prepare("SELECT key, value FROM kv WHERE scope = ?1 ORDER BY key")?;
            let rows = statement.query_map((scope,), |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
            Ok(rows.filter_map(Result::ok).collect())
        })
        .unwrap_or_default()
    }

    /// Write one value, replacing whatever was there.
    pub fn kv_set(&self, scope: &str, key: &str, value: &str) {
        self.commit("a setting", |tx| {
            kv_set_in(tx, scope, key, value)?;
            Ok(())
        });
    }

    /// Write one value only if its key is not there yet, leaving whatever is
    /// there untouched.
    ///
    /// For a value two callers may both try to make first — the plugin host's
    /// capability key — where the loser must read back the winner's rather
    /// than overwrite it (`crate::plugins::Plugins::asset_key`).
    pub fn kv_set_if_absent(&self, scope: &str, key: &str, value: &str) {
        self.commit("a setting", |tx| {
            tx.execute(
                "INSERT OR IGNORE INTO kv (scope, key, value, updated_at) VALUES (?1, ?2, ?3, unixepoch())",
                (scope, key, value),
            )?;
            Ok(())
        });
    }

    /// Forget one key. Forgetting what is not there is not an error.
    pub fn kv_remove(&self, scope: &str, key: &str) {
        self.commit("a removal", |tx| kv_remove_in(tx, scope, key));
    }

    /// Replace a whole scope with `entries`, in one transaction.
    ///
    /// For the stores whose API is still "here is the new map" — the caps that
    /// trim `seen` and `composer` to their newest N, and the client that PUTs a
    /// whole preferences object. The delete and the inserts land together, so a
    /// crash cannot leave the scope half replaced, which is precisely what the
    /// truncating `tokio::fs::write` this replaces could do.
    pub fn kv_replace_scope<'a>(&self, scope: &str, entries: impl IntoIterator<Item = (&'a str, &'a str)>) {
        self.commit("a store", |tx| {
            tx.execute("DELETE FROM kv WHERE scope = ?1", (scope,))?;
            for (key, value) in entries {
                kv_set_in(tx, scope, key, value)?;
            }
            Ok(())
        });
    }

    /// Drain a JSON file into the database once, and never look at it again.
    ///
    /// The bridge every migrated store crosses on its first boot. Three
    /// properties, each load-bearing:
    ///
    /// * **Once.** The mark is written in the same transaction as the rows, so
    ///   an import that is interrupted is an import that did not happen. Without
    ///   that, a crash half way through would leave a store that is partly
    ///   migrated and partly not, and the next boot would import *over* the
    ///   user's newer edits.
    /// * **The file is left alone.** Not deleted, not rewritten. It is the
    ///   rollback bridge: [`crate::supervisor`] can demote to a binary that has
    ///   never heard of this database, and that binary finds its file exactly
    ///   where it left it. Stale by however long the new version ran, but
    ///   coherent — which is the whole difference between a bad upgrade and a
    ///   lost account list.
    /// * **A missing file still counts as imported.** Otherwise every boot of a
    ///   fresh install re-runs the import, and worse, a store the user has since
    ///   emptied would be re-filled from a file that came back.
    pub fn import_once(&self, marker: &str, load: impl FnOnce() -> Vec<(String, String)>) {
        if self.kv_get(scope::IMPORTED, marker).is_some() {
            return;
        }
        let entries = load();
        let count = entries.len();
        let outcome = self.write(|tx| {
            // Re-check inside the transaction. Two servers can boot at once, and
            // the loser must not import over rows the winner has already handed
            // back to a user who has started editing them.
            let done: Option<String> = tx
                .query_row("SELECT value FROM kv WHERE scope = ?1 AND key = ?2", (scope::IMPORTED, marker), |row| {
                    row.get(0)
                })
                .ok();
            if done.is_some() {
                return Ok(false);
            }
            for (key, value) in &entries {
                // `INSERT OR IGNORE`, not `REPLACE`: if a row is somehow already
                // there, what is in the database is newer than what is in a file
                // this build has never written to.
                tx.execute(
                    "INSERT OR IGNORE INTO kv (scope, key, value) VALUES (?1, ?2, ?3)",
                    (marker, key.as_str(), value.as_str()),
                )?;
            }
            tx.execute(
                "INSERT OR REPLACE INTO kv (scope, key, value) VALUES (?1, ?2, ?3)",
                (scope::IMPORTED, marker, count.to_string()),
            )?;
            Ok(true)
        });
        match outcome {
            Ok(true) => tracing::info!(store = marker, rows = count, "imported a JSON store into the state database"),
            Ok(false) => {}
            Err(error) => tracing::error!(store = marker, %error, "cannot import a JSON store; it stays the source of truth this boot"),
        }
    }
}

/// [`Db::kv_set`]'s statement, for a caller already inside a transaction.
pub(crate) fn kv_set_in(tx: &rusqlite::Transaction<'_>, scope: &str, key: &str, value: &str) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO kv (scope, key, value, updated_at) VALUES (?1, ?2, ?3, unixepoch())
         ON CONFLICT (scope, key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        (scope, key, value),
    )?;
    Ok(())
}

/// [`Db::kv_remove`]'s statement, for a caller already inside a transaction.
pub(crate) fn kv_remove_in(tx: &rusqlite::Transaction<'_>, scope: &str, key: &str) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM kv WHERE scope = ?1 AND key = ?2", (scope, key))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open(dir.path().join("state.db"));
        (dir, db)
    }

    #[test]
    fn a_value_can_be_written_read_and_forgotten() {
        let (_dir, db) = db();
        assert_eq!(db.kv_get(scope::TITLES, "claude:a"), None);
        db.kv_set(scope::TITLES, "claude:a", "the one about the parser");
        assert_eq!(db.kv_get(scope::TITLES, "claude:a").as_deref(), Some("the one about the parser"));

        // Writing again replaces rather than duplicating: the primary key is the
        // whole point of the table.
        db.kv_set(scope::TITLES, "claude:a", "renamed");
        assert_eq!(db.kv_scope(scope::TITLES).len(), 1);
        assert_eq!(db.kv_get(scope::TITLES, "claude:a").as_deref(), Some("renamed"));

        db.kv_remove(scope::TITLES, "claude:a");
        assert_eq!(db.kv_get(scope::TITLES, "claude:a"), None);
        db.kv_remove(scope::TITLES, "claude:a");
    }

    #[test]
    fn scopes_do_not_see_each_other() {
        let (_dir, db) = db();
        db.kv_set(scope::TITLES, "same-key", "a title");
        db.kv_set(scope::SEEN, "same-key", "a timestamp");
        assert_eq!(db.kv_get(scope::TITLES, "same-key").as_deref(), Some("a title"));
        assert_eq!(db.kv_get(scope::SEEN, "same-key").as_deref(), Some("a timestamp"));
        assert_eq!(db.kv_scope(scope::TITLES).len(), 1, "one store's keys are not another's");
    }

    #[test]
    fn replacing_a_scope_leaves_only_what_was_given() {
        let (_dir, db) = db();
        db.kv_set(scope::SEEN, "old", "1");
        db.kv_set(scope::SEEN, "kept", "1");
        db.kv_replace_scope(scope::SEEN, [("kept", "2"), ("new", "3")]);
        let all = db.kv_scope(scope::SEEN);
        assert_eq!(all.len(), 2);
        assert_eq!(all.get("kept").map(String::as_str), Some("2"));
        assert_eq!(all.get("new").map(String::as_str), Some("3"));
        assert!(!all.contains_key("old"), "what was not given is gone");
    }

    #[test]
    fn a_store_is_imported_exactly_once_and_never_over_newer_rows() {
        let (_dir, db) = db();
        let mut loads = 0;

        db.import_once(scope::TITLES, || {
            loads += 1;
            vec![("claude:a".to_string(), "from the file".to_string())]
        });
        assert_eq!(loads, 1);
        assert_eq!(db.kv_get(scope::TITLES, "claude:a").as_deref(), Some("from the file"));

        // The user renames it. The next boot must not put the file's value back.
        db.kv_set(scope::TITLES, "claude:a", "renamed since");
        db.import_once(scope::TITLES, || {
            loads += 1;
            vec![("claude:a".to_string(), "from the file".to_string())]
        });
        assert_eq!(loads, 1, "the file is not even read a second time");
        assert_eq!(db.kv_get(scope::TITLES, "claude:a").as_deref(), Some("renamed since"));
    }

    #[test]
    fn an_absent_file_is_imported_too_so_the_question_is_never_asked_twice() {
        let (_dir, db) = db();
        let mut loads = 0;
        for _ in 0..3 {
            db.import_once(scope::ARCHIVED, || {
                loads += 1;
                Vec::new()
            });
        }
        assert_eq!(loads, 1, "a store with nothing in it is still a store that has been migrated");
    }
}
