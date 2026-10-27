//! The instance's own state, in SQLite rather than in a drawer full of JSON.
//!
//! # Why this exists
//!
//! Every store this replaces was a *whole-file rewrite of a small map*: read
//! `runs.json`, change one record, encode the lot, `fsync`, rename over the top.
//! That shape is correct on its own — [`crate::inject::runs`] wrote it carefully,
//! down to syncing the directory after the rename — and it is the reason none of
//! these files has ever been found torn. What it cannot do is any of the three
//! things a database does for free:
//!
//! * **Change two things at once.** A turn opening and a message leaving the
//!   queue is one fact, and today it is one fact only because both live in the
//!   same file. The moment a change spans two files — a run record and the
//!   session's ownership, say — there is no way to write them together, and the
//!   codebase's answer has been to arrange never to need it.
//! * **Be written by two processes.** `remote-hand-server user add` and the
//!   running server both write `users.json`, and nothing serialises them: the
//!   loser's edit is silently gone. The instance flock does not help, because
//!   the CLI deliberately runs while a server holds it.
//! * **Answer a question without reading everything.** Every list endpoint today
//!   loads a whole store and filters in Rust.
//!
//! Be precise about that third one, because it is the one most easily oversold:
//! **none of the stores here is big enough for a query to be faster than the
//! `read(2)` it replaces.** The largest is `runs.json` at 7.7 KB, and the warm
//! session list already answers in under four milliseconds. What this module
//! buys is the first two — atomicity, and safety between processes. The *speed*
//! claim belongs entirely to [`crate::db::index`], which is a different file
//! about a different problem: 194 MB of somebody else's transcripts, re-read
//! from scratch after every restart because every cache in this server is a
//! `LazyLock<Mutex<HashMap>>` that dies with the process.
//!
//! # What this is not
//!
//! It is not a place to put the transcripts. Those belong to claude and codex,
//! they are hundreds of megabytes, and remote-hand only ever reads them — see
//! [`crate::db::index`] for the durable *index* over them, which is where the
//! speed actually comes from. It is also not where the session recordings go:
//! [`crate::record`] appends NDJSON, an append is already crash-safe, and a
//! 1.4 GB table of lines nothing queries would be a database used as a bucket.
//!
//! # Durability
//!
//! WAL, and `synchronous = FULL`. That pairing is deliberately *not* the usual
//! recommendation (`NORMAL`), because the thing being replaced fsynced every
//! write and dropping that quietly would be a regression nobody would notice
//! until a power cut. It is still cheaper than what it replaces: one append to
//! the WAL instead of re-encoding and re-syncing the entire store.
//!
//! # Failure
//!
//! Opening cannot fail, in the same sense that reading a corrupt `runs.json`
//! could not: a server that refuses to boot because its bookkeeping is damaged
//! is worse than one that boots having forgotten it. A database that will not
//! open is moved aside — kept, not deleted, because a corrupt file is evidence —
//! and a fresh one takes its place. If even that fails, the connection is
//! in-memory: the server runs, this boot's state is not durable, and the log
//! says so loudly.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use rusqlite::Connection;

pub mod index;
mod kv;
mod migrations;

pub(crate) use kv::{kv_remove_in, kv_set_in};
pub use kv::scope;
pub use migrations::SCHEMA;

/// One open database, and the lock that makes it usable from anywhere.
///
/// A single connection behind a mutex rather than a pool, and the reason is the
/// shape of the load rather than laziness: these tables are kilobytes, every
/// query is against a primary key or a covering index, and the server they serve
/// answers a handful of requests a second from one or two people. A pool would
/// add a dependency and a lifetime to every call site to remove contention that
/// cannot be measured. [`crate::db::index`] is the one store that will grow, and it is
/// read far more than written; if it ever wants readers of its own, WAL is
/// already on and a second connection is three lines here — not a change to any
/// caller.
#[derive(Debug)]
pub struct Db {
    path: PathBuf,
    pub(super) schema: &'static [&'static str],
    conn: Mutex<Connection>,
}

/// How much a database is willing to lose to a power cut.
///
/// Two answers, because there are two kinds of database here and pretending
/// otherwise would cost one of them. State is authoritative and irreplaceable;
/// the index is a cache over other people's files and can always be rebuilt by
/// reading them again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    /// `synchronous = FULL` — every commit is on the platter before it returns.
    ///
    /// The default, and chosen to match rather than quietly undercut what it
    /// replaces: the JSON stores `fsync`ed the temporary file *and* the
    /// directory on every write. `NORMAL` is the usual advice for WAL and it is
    /// good advice, but taking it here would be a durability regression nobody
    /// would notice until the day it mattered. It is still cheaper than what it
    /// replaces — one append to the WAL against re-encoding an entire store.
    Committed,
    /// `synchronous = NORMAL` — safe against this process dying, not against the
    /// machine losing power mid-commit.
    ///
    /// Right for anything derived. Losing the last few commits of the transcript
    /// index costs a re-scan of a few files, which is the thing the index exists
    /// to avoid doing and not the thing it exists to *remember*.
    Rebuildable,
}

impl Db {
    /// Open — or make — the state database at `path`.
    ///
    /// Infallible by construction; see the module note on failure.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        Self::open_with(path, SCHEMA, Durability::Committed)
    }

    /// Open a database with a schema and a durability of its own.
    pub fn open_with(path: impl Into<PathBuf>, schema: &'static [&'static str], durability: Durability) -> Self {
        let path = path.into();
        if let Some(parent) = path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                tracing::warn!(path = %parent.display(), %error, "cannot make the directory the database lives in");
            }
        }
        let conn = open_prepared(&path, durability)
            .or_else(|error| {
                tracing::error!(path = %path.display(), %error, "the state database will not open; setting it aside and starting a fresh one");
                set_aside(&path);
                open_prepared(&path, durability)
            })
            .unwrap_or_else(|error| {
                tracing::error!(
                    path = %path.display(),
                    %error,
                    "no state database can be opened here; this boot keeps its state in memory and will forget it"
                );
                Connection::open_in_memory().expect("an in-memory database needs no filesystem")
            });
        let db = Self {
            path,
            schema,
            conn: Mutex::new(conn),
        };
        db.migrate();
        db.keep_to_ourselves();
        db
    }

    /// `0600` on the database and its two sidecars.
    ///
    /// Not housekeeping: this file absorbs `accounts.json`, `users.json`,
    /// `push.json` and `notify.json`, and holds the bearer-token digests the
    /// auth sessions mint. Each of those is kept `0600` because it holds an
    /// OAuth token, an argon2 hash, a VAPID private key or a bearer digest.
    /// Merging them into one file merges their permissions too, and the merge
    /// is only safe if it takes the *strictest* of them. WAL means there are
    /// three files, and the `-wal` one holds committed rows that have not been
    /// checkpointed yet — so it needs the same treatment, not just the database
    /// it belongs to.
    fn keep_to_ourselves(&self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for suffix in ["", "-wal", "-shm"] {
                let path = PathBuf::from(format!("{}{suffix}", self.path.display()));
                if !path.exists() {
                    continue;
                }
                if let Err(error) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
                    tracing::warn!(path = %path.display(), %error, "cannot restrict the state database to this account");
                }
            }
        }
    }

    /// The file this database lives in, for the log lines that name it.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Borrow the connection for a read.
    ///
    /// Synchronous, and called from async code without a `spawn_blocking`, which
    /// is a deliberate continuation of what the files did: `RunsFile::read` was
    /// a `std::fs::read` on the runtime thread too. An indexed query against a
    /// page already in SQLite's cache is faster than the `read(2)` it replaces,
    /// so this trades no latency it was not already spending.
    pub fn read<T>(&self, work: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
        work(&self.lock())
    }

    /// Borrow the connection for a change, inside one transaction.
    ///
    /// The transaction is the point of the whole module: everything the closure
    /// does lands together or not at all, so a crash cannot leave a turn open
    /// with its message still queued behind it.
    pub fn write<T>(&self, work: impl FnOnce(&rusqlite::Transaction<'_>) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
        let mut guard = self.lock();
        let tx = guard.transaction()?;
        let out = work(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// [`Db::write`], but taking the write lock up front (`BEGIN IMMEDIATE`).
    ///
    /// For a change that reads before it writes and must not interleave with a
    /// writer in another process — a compare-and-set. A deferred transaction
    /// would read a snapshot, then fail its upgrade to a writer with
    /// `SQLITE_BUSY` the moment another connection committed in between;
    /// taking the lock first makes it wait out `busy_timeout` instead.
    pub fn write_immediate<T>(&self, work: impl FnOnce(&rusqlite::Transaction<'_>) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
        let mut guard = self.lock();
        let tx = guard.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let out = work(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// A change whose failure is not the caller's problem, logged and dropped.
    ///
    /// The overwhelmingly common shape: the file stores this replaces reported a
    /// failed write by `tracing::warn!` and carrying on, because turning a
    /// durability hiccup into a failed HTTP request helps nobody.
    pub fn commit(&self, what: &'static str, work: impl FnOnce(&rusqlite::Transaction<'_>) -> rusqlite::Result<()>) {
        if let Err(error) = self.write(work) {
            tracing::warn!(path = %self.path.display(), %error, "cannot record {what}");
        }
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the connection poisons the mutex. The connection
        // itself is still sound — SQLite rolled back whatever transaction was
        // open when the stack unwound — so refusing every later query would turn
        // one failed request into a dead server.
        self.conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Open with the pragmas every connection needs, and prove it is a database.
///
/// The `SELECT count(*) FROM sqlite_schema` is not ceremony: `Connection::open`
/// is lazy and happily hands back a handle to a file of rubbish, so without a
/// statement that touches a page, a corrupt database would be discovered by the
/// first real query rather than here, where there is still somewhere to put it.
fn open_prepared(path: &Path, durability: Durability) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    // WAL survives the connection and is stored in the file, so this is a no-op
    // on every open after the first. `query_row` rather than `execute`, because
    // setting the journal mode answers with the mode it settled on.
    let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    conn.pragma_update(
        None,
        "synchronous",
        match durability {
            Durability::Committed => "FULL",
            Durability::Rebuildable => "NORMAL",
        },
    )?;
    // The CLI subcommands open this file while the server holds it. Five seconds
    // is far longer than any transaction here can take and far shorter than a
    // person's patience with `remote-hand-server user add`.
    conn.pragma_update(None, "busy_timeout", 5_000)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get::<_, i64>(0))?;
    Ok(conn)
}

/// Move a database that will not open out of the way, keeping it.
fn set_aside(path: &Path) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default();
    let corpse = path.with_extension(format!("corrupt-{stamp}"));
    // The sidecars go too, or the fresh database inherits the old one's WAL and
    // is corrupt on arrival.
    for suffix in ["", "-wal", "-shm"] {
        let from = PathBuf::from(format!("{}{suffix}", path.display()));
        if !from.exists() {
            continue;
        }
        let to = PathBuf::from(format!("{}{suffix}", corpse.display()));
        if let Err(error) = std::fs::rename(&from, &to) {
            tracing::error!(from = %from.display(), to = %to.display(), %error, "cannot set the damaged database aside; removing it instead");
            let _ = std::fs::remove_file(&from);
        }
    }
    tracing::error!(kept_as = %corpse.display(), "the damaged database was kept for inspection");
}

/// Every open database in this process, one per file.
///
/// A registry rather than a handle threaded through [`crate::sessions::Paths`],
/// for a reason that is about correctness and not convenience: `Paths` is a
/// *value*, two of them naming one instance exist at once (`app.rs` builds one
/// to take the lock and another for the state), and a handle per value would put
/// two writers on one file inside a single process. `busy_timeout` would make
/// that work and it would still be two connections contending for no reason.
/// Keyed by path, so a test with its own home gets its own database and the CLI
/// opening the same file as the server in a *different* process is left to
/// SQLite, which is the one thing that has always been able to arbitrate it.
static OPEN: OnceLock<Mutex<HashMap<PathBuf, Arc<Db>>>> = OnceLock::new();

/// The state database at `path`, opened once per process.
pub fn at(path: impl Into<PathBuf>) -> Arc<Db> {
    at_with(path, SCHEMA, Durability::Committed)
}

/// The database at `path` under a schema of its own, opened once per process.
pub fn at_with(path: impl Into<PathBuf>, schema: &'static [&'static str], durability: Durability) -> Arc<Db> {
    let path = path.into();
    let mut open = OPEN
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open.entry(path.clone())
        .or_insert_with(|| Arc::new(Db::open_with(path, schema, durability)))
        .clone()
}

/// Forget the process's connection to the database at `path`, before the
/// file is deleted: the next [`at`] / [`at_with`] opens the file anew rather
/// than handing back a connection to the deleted one (whose rows would come
/// back, and whose writes would go to an unlinked inode). Whoever still holds
/// the old handle keeps it until they drop it — stop them first. `true` when
/// there was one.
pub fn forget(path: &Path) -> bool {
    let mut open = OPEN
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open.remove(path).is_some()
}

/// The "at most once every N" guard a store puts in front of a periodic prune.
///
/// Two stores kept the same `Mutex<i64>` plus interval check by hand; this is
/// that check, once. Each store owns one and asks [`PruneGate::due`] before its
/// delete, so a burst of writes pays for one prune per interval, not one each.
#[derive(Debug)]
pub struct PruneGate {
    every_ms: i64,
    last: Mutex<i64>,
}

impl PruneGate {
    /// A gate that opens at most once every `every_ms` milliseconds.
    pub const fn new(every_ms: i64) -> Self {
        Self {
            every_ms,
            last: Mutex::new(i64::MIN),
        }
    }

    /// Whether a prune is due now; records `now` when it says yes.
    pub fn due(&self, now: i64) -> bool {
        let mut last = self.last.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if now.saturating_sub(*last) < self.every_ms {
            return false;
        }
        *last = now;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_database_arrives_with_its_schema_and_keeps_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("state.db");
        let db = Db::open(&path);
        assert!(path.exists(), "the directory it lives in is made on the way");
        assert_eq!(db.user_version(), SCHEMA.len() as i64, "a new database is at the newest version");

        // Re-opening applies nothing and loses nothing.
        db.commit("a probe", |tx| {
            tx.execute("INSERT INTO kv (scope, key, value) VALUES ('t', 'k', 'v')", [])?;
            Ok(())
        });
        drop(db);
        let again = Db::open(&path);
        let value: String = again
            .read(|conn| conn.query_row("SELECT value FROM kv WHERE scope = 't' AND key = 'k'", [], |row| row.get(0)))
            .expect("the row survives a re-open");
        assert_eq!(value, "v");
    }

    #[test]
    fn a_change_that_fails_half_way_leaves_nothing_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open(dir.path().join("state.db"));
        let outcome = db.write(|tx| {
            tx.execute("INSERT INTO kv (scope, key, value) VALUES ('t', 'first', '1')", [])?;
            // A second row on the same primary key: the statement fails, and the
            // first insert has to go with it.
            tx.execute("INSERT INTO kv (scope, key, value) VALUES ('t', 'first', '2')", [])?;
            Ok(())
        });
        assert!(outcome.is_err(), "the conflicting write is reported");
        let rows: i64 = db
            .read(|conn| conn.query_row("SELECT count(*) FROM kv", [], |row| row.get(0)))
            .expect("count");
        assert_eq!(rows, 0, "the transaction took the first insert down with it");
    }

    #[test]
    fn a_database_of_rubbish_is_kept_and_replaced_rather_than_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.db");
        std::fs::write(&path, b"this is not a database, it is a sentence").expect("write");

        let db = Db::open(&path);
        assert_eq!(db.user_version(), SCHEMA.len() as i64, "the replacement is a real, current database");
        db.commit("a probe", |tx| {
            tx.execute("INSERT INTO kv (scope, key, value) VALUES ('t', 'k', 'v')", [])?;
            Ok(())
        });
        let rows: i64 = db
            .read(|conn| conn.query_row("SELECT count(*) FROM kv", [], |row| row.get(0)))
            .expect("count");
        assert_eq!(rows, 1, "and it is usable");

        let kept: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read_dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("corrupt-"))
            .collect();
        assert_eq!(kept.len(), 1, "the damaged file is evidence, not litter: {kept:?}");
    }

    #[test]
    fn one_path_is_one_connection_however_many_times_it_is_asked_for() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.db");
        let first = at(&path);
        let second = at(&path);
        assert!(Arc::ptr_eq(&first, &second), "two callers naming one file share one database");
        assert!(
            !Arc::ptr_eq(&first, &at(dir.path().join("other.db"))),
            "and a different file is a different one"
        );
    }
}
