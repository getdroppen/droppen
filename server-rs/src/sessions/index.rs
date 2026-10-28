//! What this server has already worked out about somebody else's transcripts.
//!
//! # Why this exists
//!
//! [`crate::db`] is about atomicity; this file is the one that is about speed,
//! and the difference is worth keeping straight. The stores that module replaces
//! are kilobytes and answer in microseconds either way. The corpus *this* one
//! stands in front of is 194 MB of claude transcripts and a `~/.codex/sessions`
//! tree beside it, and the server reads a slice of every single file to build one
//! session list: a 128 KiB head parse plus a 64 KiB tail scan per transcript,
//! 11.2 MB of `read(2)` and `serde` before a row exists. On top of that sit 39
//! `git rev-parse` spawns to work out which repository each working directory
//! belongs to — 84 ms of process creation, four at a time.
//!
//! None of that is wasted twice, because every one of those answers is already
//! cached. The problem is *where*: in a `LazyLock<Mutex<HashMap>>`, which dies
//! with the process. Warm, `GET /api/sessions` answers in 3.8 ms; cold it takes
//! 150–300 ms, and this server goes cold often — self-upgrade, crash recovery,
//! and the user's own deploys all restart it. The caches were never wrong, they
//! were just amnesiac. This file gives them somewhere to remember.
//!
//! # What is in it, and what is emphatically not
//!
//! Metadata about files this process does not own: a derived summary, the
//! watermark that says which version of the file it was derived from, and a
//! working directory's repository key. **No transcript content.** The 194 MB of
//! JSONL belongs to claude and codex and stays where they put it, exactly as
//! [`crate::db`]'s module note says; a database that had copied it in would be
//! a second corpus to keep in step with the first, which is worse than no index
//! at all.
//!
//! # It is a cache, and it behaves like one
//!
//! Three properties, in the order they matter:
//!
//! * **A miss is never an error.** Absent, empty, corrupt, or holding a row that
//!   disagrees with the file on disk — every one of those ends the same way, by
//!   reading the transcript like it was 3.8 ms ago. Nothing here can fail a
//!   request, and nothing here can stop a boot.
//! * **Invalidation is exactly as strict as the memory it replaces.** The
//!   watermark is the file's path, its size and its mtime in milliseconds, and
//!   any disagreement at all is a miss. A durable cache that serves a stale
//!   summary is far worse than a slow one — a stale summary outlives the restart
//!   that used to cure it.
//! * **Losing it costs a re-scan, not a fact.** Which is what
//!   [`Durability::Rebuildable`] means, and why this lives in its own file:
//!   [`crate::paths::Paths::index_db`] explains the rest of that choice.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use rusqlite::{params, OptionalExtension};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::db::{at_with, Db, Durability};
use crate::paths::Paths;
use crate::sessions::Source;

/// The index's schema, appended to under the same additive-only rule as
/// [`super::SCHEMA`] — see `migrations.rs` for why an older binary must never
/// refuse a database it does not fully understand.
///
/// A separate list because it is a separate file. It would be tempting to share
/// one ladder and let each database apply the steps it cares about, but
/// `user_version` is a single integer per file and cannot express that.
pub const INDEX_SCHEMA: &[&str] = &[
    // 1 — the summaries, and the watermarks that say whether they are still true.
    //
    // The column-versus-payload rule from `migrations.rs` applies here with
    // extra force: `SessionSummary` and codex's `FileMeta` are decided by the
    // discovery code that has always decided them, and shredding them into
    // columns would move that decision here, into a file that has no business
    // knowing what a title is. So the payload is opaque JSON, and a field is a
    // column only where this module itself has to sort or compare on it: the
    // three watermark columns, and the key.
    //
    // A rowid table rather than `WITHOUT ROWID`, unlike everything in the state
    // database: these rows carry a whole serialised summary and run to hundreds
    // of bytes, well past the fraction of a page where `WITHOUT ROWID` stops
    // paying and starts spilling the primary key's B-tree into overflow pages.
    //
    // `updated_ms` is the transcript's mtime, which is doing two jobs at once
    // and can, because for a transcript they are the same number: it is the
    // `updatedAt` the list sorts on, and it is the half of the watermark that
    // notices the file moved on. If those ever come apart, the watermark wins —
    // an index that sorts slightly oddly is a cosmetic bug, and an index that
    // misses a change is a wrong answer.
    "CREATE TABLE IF NOT EXISTS sessions (
         source     TEXT    NOT NULL,
         session_id TEXT    NOT NULL,
         file_path  TEXT    NOT NULL,
         size_bytes INTEGER NOT NULL,
         updated_ms REAL    NOT NULL,
         payload    TEXT    NOT NULL,
         PRIMARY KEY (source, session_id)
     ) STRICT;
     CREATE INDEX IF NOT EXISTS sessions_by_updated ON sessions (updated_ms DESC);

     -- Which repository each working directory belongs to. The one table here
     -- that is not about a file: its input is a directory and its cost is a
     -- process, which is why it is worth remembering across a restart at all.
     --
     -- `repo_key` is nullable and the null is an *answer* — 'this directory is
     -- not in a repository' — not an absence. A row that did not distinguish
     -- them would re-spawn `git` for every plain directory on every boot, which
     -- is most of the storm this is here to stop.
     CREATE TABLE IF NOT EXISTS repo_keys (
         cwd       TEXT    NOT NULL PRIMARY KEY,
         repo_key  TEXT,
         probed_at INTEGER NOT NULL
     ) STRICT, WITHOUT ROWID;
     CREATE INDEX IF NOT EXISTS repo_keys_by_probe ON repo_keys (probed_at)",
    // 2 — the unread ledgers (`crate::attention::unread`): the timestamps of a
    // transcript's newest assistant messages, and how far into the file they
    // were read.
    //
    // Keyed by the file rather than the session, because what it records is a
    // fact about a file. The watermark is the row's own `updatedAt` and size —
    // the pair the in-memory memo already compares, so no `stat` is needed to
    // decide a hit — and unlike `sessions` a *mismatch* is still useful here:
    // the payload's `end` says where to resume reading, so a transcript that
    // grew costs the bytes it grew by rather than a fresh scan.
    "CREATE TABLE IF NOT EXISTS unread_ledgers (
         file_path  TEXT    NOT NULL PRIMARY KEY,
         updated_at TEXT    NOT NULL,
         size_bytes INTEGER NOT NULL,
         payload    TEXT    NOT NULL
     ) STRICT;
     CREATE INDEX IF NOT EXISTS unread_ledgers_by_updated ON unread_ledgers (updated_at DESC)",
];

/// How long a repository key is believed before `git` is asked again.
///
/// The in-memory cache this backs has no invalidation at all and says so: a
/// directory's repository does not change, and the two cases where it does —
/// `git init` where sessions already exist, or a repository deleted underneath
/// one — cost a stale grouping *until the server restarts*. Making the answer
/// durable quietly deletes the second half of that sentence, and "until you
/// notice and delete index.db" is not a promise worth making for a key the
/// module doc calls display-only.
///
/// A day, then. It bounds the staleness at something a person would shrug at,
/// and it costs one 84 ms storm per day against one per restart.
const REPO_KEY_TTL_SECS: i64 = 24 * 60 * 60;

/// How many summaries and repository keys are kept.
///
/// Rows are only ever added for a transcript that was listed, so the working set
/// is the corpus — 70 transcripts and 39 directories on the machine this was
/// measured on. The caps exist because a row for a transcript that has since
/// been deleted is never visited again and would otherwise sit there for the
/// life of the machine, not because the live set is anywhere near them.
const SESSION_ROWS_MAX: i64 = 10_000;
const REPO_KEY_ROWS_MAX: i64 = 4_096;

/// The index, and the connection it speaks through.
///
/// Cloning is an `Arc` bump: [`super::at_with`] keeps one connection per file per
/// process, and this is a handle onto it.
#[derive(Debug, Clone)]
pub struct Index {
    db: Arc<Db>,
}

/// The index for this path set, opened once per process.
pub fn open(paths: &Paths) -> Index {
    Index {
        db: at_with(paths.index_db(), INDEX_SCHEMA, Durability::Rebuildable),
    }
}

/// The index the summarizers reach for, or `None` before anything installed one.
///
/// A process-wide handle rather than an argument, and it is worth being honest
/// about why: [`crate::sessions::claude::summarize_file`],
/// `crate::sessions::codex::file_meta` and [`crate::sessions::repo::repo_key`]
/// take a path and nothing else, because what they read is a file on this
/// machine and not anything belonging to an instance. Threading a `Paths` into
/// all three to reach the index would change the signature of every caller of
/// each of them to carry something none of them has an opinion about.
///
/// So the entry points that *do* hold a `Paths` — the list and the single-row
/// lookups — say which instance is asking on the way past, and the summarizers
/// below them use whatever that was. The one place this is loose is a process
/// serving two instances at once, where a summary could be written into the
/// other one's index. It cannot produce a wrong answer, because the watermark is
/// a fact about a file and means the same thing in either file.
static CURRENT: RwLock<Option<(PathBuf, Index)>> = RwLock::new(None);

#[cfg(test)]
thread_local! {
    /// In tests, the index this thread installed last, which [`current`]
    /// answers with before the process-wide one. `cargo test` runs the whole
    /// crate in one process, and many tests list sessions (each list installs
    /// its own instance's index) without [`exclusive`]: one of those landing
    /// in the middle of a test that asserts on what its own index answers
    /// swapped that index out from under it. Every test has a thread of its
    /// own, and a `#[tokio::test]` runs its tasks on it.
    static INSTALLED_HERE: std::cell::RefCell<Option<Index>> = const { std::cell::RefCell::new(None) };
}

/// Point the summarizers at this instance's index.
///
/// Cheap enough to call on every list: it is a read lock and a path compare
/// unless the instance has actually changed.
pub fn install(paths: &Paths) {
    let path = paths.index_db();
    #[cfg(test)]
    INSTALLED_HERE.with(|here| *here.borrow_mut() = Some(open(paths)));
    if CURRENT
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .is_some_and(|(installed, _)| *installed == path)
    {
        return;
    }
    let index = open(paths);
    // Once per boot, which is exactly when the last boot's leftovers are
    // discoverable and nothing is waiting on a list.
    index.tidy();
    *CURRENT.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((path, index));
}

/// The installed index, if there is one.
///
/// `None` is a supported answer and not a bug: a unit test summarising one file,
/// a CLI subcommand, anything that never listed a session. It means "read the
/// transcript", which is what this whole file is an optimisation over.
pub fn current() -> Option<Index> {
    #[cfg(test)]
    if let Some(index) = INSTALLED_HERE.with(|here| here.borrow().clone()) {
        return Some(index);
    }
    CURRENT
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .map(|(_, index)| index.clone())
}

/// Which version of a file an indexed answer was derived from.
///
/// All three fields, and equality on all three, because each catches something
/// the others do not: `path` catches a row about a different file that happens
/// to share an id, `size_bytes` catches an edit that landed inside the mtime's
/// resolution, and `mtime_ms` catches everything else. Any disagreement is a
/// miss — there is no "close enough" here, because the cost of being wrong is a
/// stale title that survives the restart which used to fix it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Watermark<'a> {
    /// The transcript this answer is about.
    pub path: &'a Path,
    /// Its length when the answer was derived.
    pub size_bytes: u64,
    /// Its mtime when the answer was derived, in milliseconds since the epoch.
    pub mtime_ms: f64,
}

impl<'a> Watermark<'a> {
    /// The watermark of a file that has just been `stat`ed.
    pub fn of(path: &'a Path, metadata: &std::fs::Metadata) -> Self {
        Self {
            path,
            size_bytes: metadata.len(),
            mtime_ms: crate::time::mtime_ms(metadata),
        }
    }
}

impl Index {
    /// The stored summary for a session, if it is still about this exact file.
    ///
    /// Generic over the payload so the index never learns what a summary is:
    /// claude stores a `SessionSummary` and codex a `FileMeta`, and both remain
    /// the business of the module that builds them.
    pub fn summary<T: DeserializeOwned>(&self, source: Source, id: &str, mark: Watermark<'_>) -> Option<T> {
        let payload = self
            .db
            .read(|conn| {
                // Cached, because a cold list runs this once per transcript and
                // re-compiling the same SQL 70 times is a cost this file exists
                // to avoid rather than to introduce.
                let mut stmt = conn.prepare_cached(
                    "SELECT payload FROM sessions
                      WHERE source = ?1 AND session_id = ?2
                        AND file_path = ?3 AND size_bytes = ?4 AND updated_ms = ?5",
                )?;
                stmt.query_row(
                    params![source.as_str(), id, mark.path.to_string_lossy(), mark.size_bytes as i64, mark.mtime_ms],
                    |row| row.get::<_, String>(0),
                )
                .optional()
            })
            .unwrap_or_else(|error| {
                // A damaged or half-migrated index. Nothing to do about it here:
                // the caller reads the transcript, which is the answer to every
                // question this file cannot answer.
                tracing::debug!(%error, "the transcript index cannot be read; falling back to the file");
                None
            })?;

        match serde_json::from_str(&payload) {
            Ok(value) => Some(value),
            // A row written by a build whose summary had a different shape. The
            // additive-only rule keeps *columns* readable across versions; it
            // says nothing about a payload, so this is the payload's version of
            // the same tolerance.
            Err(error) => {
                tracing::debug!(%source, %id, %error, "an indexed summary no longer parses; re-reading the transcript");
                None
            }
        }
    }

    /// Record a summary against the version of the file it was derived from.
    pub fn remember_summary<T: Serialize>(&self, source: Source, id: &str, mark: Watermark<'_>, value: &T) {
        let Ok(payload) = serde_json::to_string(value) else {
            tracing::debug!(%source, %id, "a summary will not serialise; leaving it out of the index");
            return;
        };
        self.db.commit("a session summary", |tx| {
            tx.execute(
                "INSERT OR REPLACE INTO sessions (source, session_id, file_path, size_bytes, updated_ms, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    source.as_str(),
                    id,
                    mark.path.to_string_lossy(),
                    mark.size_bytes as i64,
                    mark.mtime_ms,
                    payload
                ],
            )?;
            Ok(())
        });
    }

    /// The last unread ledger derived for a transcript, and the row watermark
    /// (`updatedAt`, size) it was derived against.
    ///
    /// Returned whatever the watermark says: the caller decides between "still
    /// current" and "resume from where it stopped". A payload this build cannot
    /// read is no ledger at all, and costs a scan.
    pub fn unread_ledger<T: DeserializeOwned>(&self, path: &str) -> Option<(String, u64, T)> {
        let (updated_at, size_bytes, payload) = self
            .db
            .read(|conn| {
                let mut stmt = conn.prepare_cached("SELECT updated_at, size_bytes, payload FROM unread_ledgers WHERE file_path = ?1")?;
                stmt.query_row(params![path], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?))
                })
                .optional()
            })
            .unwrap_or_else(|error| {
                tracing::debug!(%error, "the unread ledger index cannot be read; scanning the transcript");
                None
            })?;
        let ledger = serde_json::from_str(&payload).ok()?;
        Some((updated_at, u64::try_from(size_bytes).unwrap_or_default(), ledger))
    }

    /// Record a transcript's unread ledger against the row watermark it was
    /// derived for.
    pub fn remember_unread_ledger<T: Serialize>(&self, path: &str, updated_at: &str, size_bytes: u64, ledger: &T) {
        let Ok(payload) = serde_json::to_string(ledger) else { return };
        self.db.commit("an unread ledger", |tx| {
            tx.execute(
                "INSERT OR REPLACE INTO unread_ledgers (file_path, updated_at, size_bytes, payload) VALUES (?1, ?2, ?3, ?4)",
                params![path, updated_at, size_bytes as i64, payload],
            )?;
            Ok(())
        });
    }

    /// What `git` last said about this directory, if it was recently enough.
    ///
    /// Two layers of `Option` and they mean different things: the outer one is
    /// whether the index knows, the inner one is the answer — and `Some(None)`,
    /// "known to be in no repository", is the common case worth remembering.
    pub fn repo_key(&self, cwd: &Path) -> Option<Option<String>> {
        self.db
            .read(|conn| {
                let mut stmt = conn.prepare_cached("SELECT repo_key FROM repo_keys WHERE cwd = ?1 AND probed_at > unixepoch() - ?2")?;
                stmt.query_row(params![cwd.to_string_lossy(), REPO_KEY_TTL_SECS], |row| row.get::<_, Option<String>>(0))
                    .optional()
            })
            .unwrap_or_else(|error| {
                tracing::debug!(%error, "the repository key index cannot be read; asking git instead");
                None
            })
    }

    /// Record what `git` said about a directory, including that it said nothing.
    pub fn remember_repo_key(&self, cwd: &Path, key: Option<&str>) {
        self.db.commit("a repository key", |tx| {
            tx.execute(
                "INSERT OR REPLACE INTO repo_keys (cwd, repo_key, probed_at) VALUES (?1, ?2, unixepoch())",
                params![cwd.to_string_lossy(), key],
            )?;
            Ok(())
        });
    }

    /// Drop what has aged out and what is over the cap.
    ///
    /// Run once when the index is installed rather than on every write: a prune
    /// on the write path would put a scan in front of each of the 70 summaries a
    /// cold list produces, to bound a table whose live set is those same 70 rows.
    /// Nothing depends on this having run — an oversized index is a large cache,
    /// not a wrong one.
    pub fn tidy(&self) {
        self.db.commit("the index's housekeeping", |tx| {
            // `LIMIT -1 OFFSET n` is SQLite for "everything after the first n":
            // keep the newest rows, delete whatever is behind them.
            tx.execute(
                "DELETE FROM sessions WHERE rowid IN
                   (SELECT rowid FROM sessions ORDER BY updated_ms DESC LIMIT -1 OFFSET ?1)",
                params![SESSION_ROWS_MAX],
            )?;
            // The same cap and the same reasoning: one row per transcript ever
            // listed, and a deleted transcript's row is never visited again.
            tx.execute(
                "DELETE FROM unread_ledgers WHERE rowid IN
                   (SELECT rowid FROM unread_ledgers ORDER BY updated_at DESC LIMIT -1 OFFSET ?1)",
                params![SESSION_ROWS_MAX],
            )?;
            tx.execute("DELETE FROM repo_keys WHERE probed_at <= unixepoch() - ?1", params![REPO_KEY_TTL_SECS])?;
            tx.execute(
                "DELETE FROM repo_keys WHERE cwd IN
                   (SELECT cwd FROM repo_keys ORDER BY probed_at DESC LIMIT -1 OFFSET ?1)",
                params![REPO_KEY_ROWS_MAX],
            )?;
            Ok(())
        });
    }
}

/// Serialises the tests that install a process-wide index.
///
/// [`install`] is global on purpose — see [`current`] — and `cargo test` runs
/// this module's tests and the summarizers' in one process, so without this one
/// test's index gets swapped out from under another's assertion. Nothing the
/// server does takes this lock; it exists because the thing under test is a
/// global and a global cannot be tested twice at once.
/// `tokio`'s mutex rather than `std`'s because every caller is an async test
/// that holds this across the `await` on a summary — which is the one thing a
/// `std` guard must never do.
#[cfg(test)]
pub async fn exclusive() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    LOCK.lock().await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same index, through a connection that has never seen the first one.
    ///
    /// `at_with` hands back one connection per file per process, which is right
    /// for the server and useless for proving durability — the whole question is
    /// whether the *file* carries the answer. This is the closest a unit test
    /// gets to a restart.
    fn reopened(path: &Path) -> Index {
        Index {
            db: Arc::new(Db::open_with(path, INDEX_SCHEMA, Durability::Rebuildable)),
        }
    }

    /// A stand-in for whatever the discovery code hands the index.
    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Row {
        title: String,
        cwd: Option<String>,
    }

    fn transcript(dir: &Path, name: &str, body: &str) -> (PathBuf, std::fs::Metadata) {
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write");
        let metadata = std::fs::metadata(&path).expect("stat");
        (path, metadata)
    }

    #[test]
    fn a_summary_outlives_the_process_that_wrote_it() {
        let home = tempfile::tempdir().expect("tempdir");
        let db_path = home.path().join("index.db");
        let (path, metadata) = transcript(home.path(), "a.jsonl", "{}\n");
        let row = Row {
            title: "from the index".into(),
            cwd: Some("/w".into()),
        };

        reopened(&db_path).remember_summary(Source::Claude, "abc", Watermark::of(&path, &metadata), &row);

        // The restart: a different connection, the same file, no memory of the
        // 128 KiB the summary came from.
        let after = reopened(&db_path);
        let found: Option<Row> = after.summary(Source::Claude, "abc", Watermark::of(&path, &metadata));
        assert_eq!(found.as_ref(), Some(&row), "the summary is read back rather than re-derived");

        // The key is composite, so the same id under the other CLI is a miss.
        let other: Option<Row> = after.summary(Source::Codex, "abc", Watermark::of(&path, &metadata));
        assert_eq!(other, None, "one id under two sources is two sessions");
    }

    #[test]
    fn a_file_that_moved_at_all_is_a_miss() {
        let home = tempfile::tempdir().expect("tempdir");
        let index = open(&Paths::new(home.path().to_path_buf()));
        let (path, metadata) = transcript(home.path(), "b.jsonl", "{}\n");
        let row = Row {
            title: "stale".into(),
            cwd: None,
        };
        index.remember_summary(Source::Claude, "b", Watermark::of(&path, &metadata), &row);

        let mark = Watermark::of(&path, &metadata);
        assert!(
            index.summary::<Row>(Source::Claude, "b", mark).is_some(),
            "the unchanged file still matches"
        );

        // Each of the three watermark fields, on its own, is enough to disagree.
        let grown = Watermark {
            size_bytes: mark.size_bytes + 1,
            ..mark
        };
        assert!(index.summary::<Row>(Source::Claude, "b", grown).is_none(), "a byte was appended");
        let touched = Watermark {
            mtime_ms: mark.mtime_ms + 1.0,
            ..mark
        };
        assert!(index.summary::<Row>(Source::Claude, "b", touched).is_none(), "the file was written");
        let elsewhere = home.path().join("copy.jsonl");
        let moved = Watermark { path: &elsewhere, ..mark };
        assert!(
            index.summary::<Row>(Source::Claude, "b", moved).is_none(),
            "a different file with the same id"
        );

        // And a real change replaces the row rather than leaving both.
        std::fs::write(&path, "{}\n{}\n").expect("append");
        let after = std::fs::metadata(&path).expect("stat");
        let fresh = Row {
            title: "current".into(),
            cwd: None,
        };
        index.remember_summary(Source::Claude, "b", Watermark::of(&path, &after), &fresh);
        assert_eq!(
            index.summary::<Row>(Source::Claude, "b", Watermark::of(&path, &after)),
            Some(fresh),
            "the new answer is the only one"
        );
        assert!(
            index.summary::<Row>(Source::Claude, "b", mark).is_none(),
            "and the version it replaced cannot be served again"
        );
    }

    #[test]
    fn an_index_that_is_missing_or_wrecked_is_a_miss_and_not_a_failure() {
        let home = tempfile::tempdir().expect("tempdir");
        let (path, metadata) = transcript(home.path(), "c.jsonl", "{}\n");
        let mark = Watermark::of(&path, &metadata);

        // Never written to, in a directory that does not exist yet.
        let fresh = reopened(&home.path().join("nested").join("index.db"));
        assert!(fresh.summary::<Row>(Source::Claude, "c", mark).is_none());
        assert!(fresh.repo_key(&path).is_none());

        // A file of rubbish where the index should be. `Db` sets it aside and
        // starts a new one, so this degrades to the empty case above rather than
        // to a panic or a refusal to boot.
        let wrecked = home.path().join("wrecked.db");
        std::fs::write(&wrecked, b"not a database, just a sentence").expect("write");
        let replaced = reopened(&wrecked);
        assert!(replaced.summary::<Row>(Source::Claude, "c", mark).is_none());
        replaced.remember_summary(
            Source::Claude,
            "c",
            mark,
            &Row {
                title: "t".into(),
                cwd: None,
            },
        );
        assert!(replaced.summary::<Row>(Source::Claude, "c", mark).is_some(), "and the replacement works");

        // A row whose payload is no longer anything this build can read.
        replaced.db.commit("a wrecked payload", |tx| {
            tx.execute("UPDATE sessions SET payload = 'half a {' WHERE session_id = 'c'", [])?;
            Ok(())
        });
        assert!(
            replaced.summary::<Row>(Source::Claude, "c", mark).is_none(),
            "an unreadable payload sends the caller back to the transcript"
        );
    }

    #[test]
    fn a_repository_key_is_remembered_including_the_absence_of_one() {
        let home = tempfile::tempdir().expect("tempdir");
        let db_path = home.path().join("index.db");
        let inside = home.path().join("project");
        let outside = home.path().join("plain");

        let before = reopened(&db_path);
        before.remember_repo_key(&inside, Some("/repos/one/.git"));
        before.remember_repo_key(&outside, None);

        let after = reopened(&db_path);
        assert_eq!(after.repo_key(&inside), Some(Some("/repos/one/.git".to_string())));
        assert_eq!(
            after.repo_key(&outside),
            Some(None),
            "'not in a repository' is an answer, and remembering it is most of the saving"
        );
        assert_eq!(
            after.repo_key(&home.path().join("never-asked")),
            None,
            "and an unknown directory is unknown"
        );
    }

    #[test]
    fn a_repository_key_stops_being_believed_eventually() {
        let home = tempfile::tempdir().expect("tempdir");
        let index = reopened(&home.path().join("index.db"));
        let cwd = home.path().join("project");
        index.remember_repo_key(&cwd, Some("/repos/one/.git"));

        // Age the row past the ttl: `git init` in a directory that already had
        // sessions used to be cured by a restart, and something has to cure it
        // now that a restart does not.
        index.db.commit("an aged probe", |tx| {
            tx.execute("UPDATE repo_keys SET probed_at = unixepoch() - ?1", params![REPO_KEY_TTL_SECS + 60])?;
            Ok(())
        });
        assert_eq!(index.repo_key(&cwd), None, "an answer this old is asked again");

        index.tidy();
        let left: i64 = index
            .db
            .read(|conn| conn.query_row("SELECT count(*) FROM repo_keys", [], |row| row.get(0)))
            .expect("count");
        assert_eq!(left, 0, "and housekeeping removes it rather than leaving it to be skipped for ever");
    }

    #[test]
    fn housekeeping_keeps_the_newest_and_drops_the_rest() {
        let home = tempfile::tempdir().expect("tempdir");
        let index = reopened(&home.path().join("index.db"));
        index.db.commit("a pile of rows", |tx| {
            for n in 0..(SESSION_ROWS_MAX + 50) {
                tx.execute(
                    "INSERT INTO sessions (source, session_id, file_path, size_bytes, updated_ms, payload)
                     VALUES ('claude', ?1, ?2, 1, ?3, '{}')",
                    params![n.to_string(), format!("/t/{n}.jsonl"), n as f64],
                )?;
            }
            Ok(())
        });

        index.tidy();
        let (rows, oldest): (i64, f64) = index
            .db
            .read(|conn| conn.query_row("SELECT count(*), min(updated_ms) FROM sessions", [], |row| Ok((row.get(0)?, row.get(1)?))))
            .expect("count");
        assert_eq!(rows, SESSION_ROWS_MAX, "the cap holds");
        assert_eq!(oldest, 50.0, "and it is the least recently touched transcripts that go");
    }

    /// Another test listing sessions installs its own index, from its own
    /// thread, at any moment: this thread still reads the one it installed
    /// (`a_summary_outlives_the_process_and_a_moved_file_does_not` used to
    /// fail when that happened between its install and its summary).
    #[tokio::test]
    async fn a_test_reads_the_index_it_installed_whatever_another_installs() {
        let _guard = exclusive().await;
        let (mine, theirs) = (tempfile::tempdir().expect("tempdir"), tempfile::tempdir().expect("tempdir"));
        let (mine, theirs) = (Paths::new(mine.path().to_path_buf()), Paths::new(theirs.path().to_path_buf()));
        install(&mine);
        let other = theirs.clone();
        std::thread::spawn(move || install(&other)).join().expect("the other test");
        let current = current().expect("an index");
        assert!(Arc::ptr_eq(&current.db, &open(&mine).db), "still mine");
        assert!(!Arc::ptr_eq(&current.db, &open(&theirs).db));
    }
}
