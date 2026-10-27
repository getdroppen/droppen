//! Where the next boot finds the apps this one left running.
//!
//! ```text
//! <instance>/state.db       one `apps` row per running app  (server-owned)
//! <instance>/apps/<id>.log  that app's stdout+stderr        (app-owned)
//! ```
//!
//! The pty shims of [`crate::pty_shim`] can be found by scanning a directory,
//! because each one leaves a socket a server can knock on. An embedded app
//! leaves nothing: it is an ordinary `code-server` on an ordinary loopback port,
//! indistinguishable from anything else the machine happens to be running. So
//! the server writes down what it started, and that record — not the port, and
//! not the process table — is the only thing that says an app was ours.
//!
//! Which makes the record's integrity the whole story. A record that is a little
//! bit wrong points at a pid, and the boot that reads it may *signal* that pid.
//! Hence: replaced whole on every change, never appended to, and read back
//! defensively — a record that will not parse means "adopt nothing", never
//! "guess". The pid in it is a claim to be checked (see [`crate::procs`]), never
//! an instruction to be followed.
//!
//! # Where the records actually are
//!
//! One row each in [`crate::db`]'s `apps` table, in the instance database that
//! sits beside `apps.json`. Everything the paragraph above asks for is the
//! transaction's: replacing all the rows at once is atomic by construction, and
//! the WAL at `synchronous = FULL` buys what a hand-rolled tempfile / `rename` /
//! directory-`fsync` dance would.
//!
//! Nothing reads or writes `apps.json`; a file left on disk is inert.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::AppType;
use crate::db::Db;

/// One app that was running when this file was last written.
///
/// Deliberately not [`super::AppInfo`]: that is the wire shape, and the two
/// answer different questions. `AppInfo` describes an app to a browser; this
/// describes it to a process that has never seen it before and has to decide
/// whether it still exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppRecord {
    /// The app id, and so the `/embed/{id}/` path it is reached on.
    pub id: String,
    /// Which kind of app it is, and so which program its pid should be running.
    pub kind: AppType,
    /// The process group leader the server spawned.
    ///
    /// A *group*, because `code-server` is a node process that starts more of
    /// them; the leader is what a signal has to be aimed at to reach the lot.
    pub pid: u32,
    /// The loopback port it listens on.
    pub port: u16,
    /// The folder it was opened on, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// When it was started, ISO-8601.
    ///
    /// Restores `AppInfo::created_at`, so an adopted app does not claim to have
    /// been created at the moment the server restarted, and orders adoption so
    /// the titles come back in the order the user made them.
    pub started_at: String,
    /// The pid-reuse guard: see [`crate::procs`]. `None` on a platform that has
    /// none, and on any record written before the guard existed — which is why
    /// it defaults rather than being required.
    #[serde(default)]
    pub pid_token: Option<String>,
}

/// The instance database that sits beside `apps.json`.
///
/// Derived from the path rather than taken as a [`crate::sessions::Paths`], and
/// the reason is that the two are the same directory: the only caller hands this
/// module `<instance>/apps.json`, and `<instance>/state.db` is its sibling.
/// Threading a `Paths` down here instead would change every caller's signature
/// to say something the path it already passes has always said.
fn store(path: &Path) -> Arc<Db> {
    crate::db::at(path.with_file_name("state.db"))
}

impl AppRecord {
    /// This record as the `payload` column holds it: the JSON it has always
    /// been.
    fn payload(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "null".to_string())
    }
}

/// Every record the database holds, oldest slot first.
///
/// A row that will not parse is dropped rather than fatal: a record that is a
/// little bit wrong points at a pid this boot may signal. `ordinal` preserves
/// the order the rows were written, which [`super::Registry::readopt_all`]
/// re-sorts by `startedAt` anyway — but the column costs nothing and losing an
/// order would be a silent change.
pub fn load(path: &Path) -> Vec<AppRecord> {
    store(path)
        .read(|conn| {
            let mut statement = conn.prepare("SELECT payload FROM apps ORDER BY ordinal")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            Ok(rows
                .filter_map(Result::ok)
                .filter_map(|payload| serde_json::from_str::<AppRecord>(&payload).ok())
                .collect())
        })
        .unwrap_or_default()
}

/// Replace the stored records with `records`.
///
/// A failure is logged rather than returned, because there is nothing a caller
/// could do with it: the app itself is unaffected, and refusing to start an
/// editor because its bookkeeping would not write is the worse trade. What it
/// costs is one generation of apps left running with nobody to adopt them.
///
/// The transaction is the whole of the save; nothing re-encodes and renames
/// `apps.json` after it, because nothing reads it.
pub fn save(path: &Path, records: &[AppRecord]) {
    let db = store(path);
    db.commit("the running apps", |tx| {
        // Replace rather than merge: a rewrite is the current truth, and a
        // record left behind is a pid a later boot would consider signalling.
        tx.execute("DELETE FROM apps", [])?;
        for (ordinal, record) in records.iter().enumerate() {
            tx.execute(
                "INSERT OR REPLACE INTO apps (id, ordinal, payload) VALUES (?1, ?2, ?3)",
                (&record.id, ordinal as i64, record.payload()),
            )?;
        }
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, pid: u32) -> AppRecord {
        AppRecord {
            id: id.to_string(),
            kind: AppType::Vscode,
            pid,
            port: 8710,
            cwd: Some("/home/user".to_string()),
            started_at: "2026-01-01T00:00:00.000Z".to_string(),
            pid_token: Some("123.000000".to_string()),
        }
    }

    #[test]
    fn a_record_survives_a_round_trip_and_the_records_are_replaced_whole() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("apps.json");

        assert!(load(&path).is_empty(), "no database at all is no apps, not a failure");

        save(&path, &[record("one", 101), record("two", 102)]);
        assert_eq!(load(&path).len(), 2);

        // A rewrite replaces, never appends: what is stored is the current
        // truth, and a record left behind is a pid a later boot would consider
        // signalling.
        save(&path, &[record("two", 102)]);
        let loaded = load(&path);
        assert_eq!(loaded, vec![record("two", 102)]);

        save(&path, &[]);
        assert!(load(&path).is_empty());
    }

    #[test]
    fn the_optional_folder_is_absent_rather_than_null() {
        let mut without = record("bare", 7);
        without.cwd = None;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("apps.json");
        save(&path, &[without.clone()]);

        // The stored JSON, which is the `payload` column now rather than the
        // file: an absent folder writes no key at all, so a build that later
        // reads it back cannot mistake a `null` for a path.
        let stored: String = store(&path)
            .read(|conn| conn.query_row("SELECT payload FROM apps", [], |row| row.get(0)))
            .expect("the row is there");
        assert!(!stored.contains("cwd"), "an absent folder writes no key: {stored}");
        assert_eq!(load(&path), vec![without]);
    }

    /// What a restart is, from the store's point of view.
    #[test]
    fn a_record_survives_the_database_being_reopened() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("apps.json");
        save(&path, &[record("one", 101)]);

        let reopened = Db::open(path.with_file_name("state.db"));
        let stored: String = reopened
            .read(|conn| conn.query_row("SELECT id FROM apps", [], |row| row.get(0)))
            .expect("the row is there for anyone who opens the file");
        assert_eq!(stored, "one");
    }

    /// A save writes the table and nothing else: `apps.json` is neither read
    /// nor written, so a file left beside the database is inert and a save must
    /// not recreate one.
    #[test]
    fn no_save_writes_the_file_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("apps.json");

        save(&path, &[record("one", 101)]);
        assert!(!path.exists(), "a save writes the table and creates no file");
        assert!(!path.with_extension("json.tmp").exists());

        // A file that appears afterwards is left exactly as found: nothing reads
        // it, and there is no writer left to overwrite it.
        let planted = b"[{\"id\":\"planted\"".to_vec();
        std::fs::write(&path, &planted).expect("plant");
        save(&path, &[record("two", 102)]);
        assert_eq!(load(&path), vec![record("two", 102)]);
        assert_eq!(std::fs::read(&path).expect("read"), planted);
    }

    /// A path that cannot be written costs nothing: the record is in the
    /// database, and nothing tries to reach `apps.json` at all.
    #[test]
    fn an_unwritable_file_location_no_longer_costs_the_record() {
        let dir = tempfile::tempdir().expect("tempdir");
        let blocked = dir.path().join("blocked").join("apps.json");
        std::fs::create_dir_all(&blocked).expect("a directory in the way");

        save(&blocked, &[record("two", 102)]);
        assert_eq!(load(&blocked), vec![record("two", 102)], "the database is what the next boot reads");
        assert!(
            std::fs::metadata(&blocked).expect("stat").is_dir(),
            "and the path it once wrote is exactly as unwritable as it was"
        );
    }

    #[test]
    fn the_record_is_written_even_when_the_instance_directory_is_not_there_yet() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("made").join("up").join("apps.json");
        save(&path, &[record("deep", 3)]);
        assert_eq!(load(&path), vec![record("deep", 3)]);
    }
}
