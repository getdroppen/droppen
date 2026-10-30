//! The record of every CLI run that could outlive this server.
//!
//! A resident `claude` or a `codex exec` run is spawned with
//! `kill_on_drop(false)` and is not on the shutdown list, precisely so that a
//! restart does not throw away a turn the user asked for (see
//! `shutdown::Cleanups`). That has always been true; what was missing
//! is any way for the *next* server to know the process is there. In-memory
//! injection state died with the process, so the composer unblocked, a second
//! send spawned a second CLI against the same session, and two agents wrote to
//! one transcript.
//!
//! The two kinds of record are told apart by [`RunRecord::resident`], because
//! what the next server does with them is not the same thing: a `codex exec`
//! child is probed by pid and otherwise left alone, while a resident is
//! re-attached to through the shim socket named by [`RunRecord::shim`].
//!
//! This journal is the missing half. It is the `runs` table of the instance's
//! database — the server that holds [`droppen_instance::instance::InstanceLock`] is the
//! only one that may read or write it — and holds exactly the runs that are
//! *in flight*:
//!
//! ```text
//! <data>/instances/<name>/state.db
//! ```
//!
//! The `runs.json` this module is still named after was read once, on the first
//! boot of a build that has the table. It is no longer read: one left on disk
//! is inert.
//!
//! # Why it stays small
//!
//! A row is inserted when a CLI is spawned and deleted when it finishes, so the
//! table holds one row per live run and is empty the rest of the time.
//! Completed runs are deliberately not kept: the transcript is the record of
//! what happened, and a log of finished runs would be a second, worse one that
//! grows without bound.
//!
//! # Durability
//!
//! One transaction per change, against a WAL running at `synchronous = FULL`
//! (see [`droppen_instance::db`]): the commit does not return until the write-ahead log is
//! on the platter, so a run that was recorded stays recorded across a power
//! cut. That is the guarantee the old whole-file rewrite bought by hand —
//! encode, `fsync` the temporary, `rename`, `fsync` the *directory* so the
//! rename itself survives — except that it now costs one small append per
//! change instead of re-encoding and re-syncing every in-flight run several
//! times per user message. A torn journal is still the one thing adoption
//! cannot recover from; it is simply no longer this module's job to prevent it.

use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use droppen_sessions::Source;

/// One in-flight run, as it survives on disk: what every CLI's run says, and
/// beside it, in the same row, what the provider that started it keeps there.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord<P = Payload> {
    /// Identifies this run across a rename, so completion removes the entry it
    /// created even after the session id underneath it changed.
    pub run_id: String,
    /// Which CLI is running.
    pub source: Source,
    /// The session the run belongs to. Rewritten by
    /// the provider when its CLI announces the id it actually chose.
    pub session_id: String,
    /// Where the CLI was started, for the record; adoption does not re-derive
    /// anything from it, since the process is already running.
    pub cwd: Option<String>,
    /// The CLI's pid — the thing adoption actually probes.
    pub cli_pid: u32,
    /// When the run started, ISO-8601, so an adopted run does not claim to have
    /// begun at the moment the server restarted.
    pub started_at: String,
    /// The pid-reuse guard: see [`droppen_base::procs`]. `None` on a platform that has
    /// none, and on any record written before the guard existed.
    #[serde(default)]
    pub pid_token: Option<String>,
    /// True when `cli_pid` is a *resident* claude behind a session shim rather
    /// than a one-shot `--print` child.
    ///
    /// The two are adopted in completely different ways — a `--print` child is
    /// probed by pid and left alone, a resident is re-attached to through its
    /// shim socket — and a reader that could not tell them apart would either
    /// poll a process it can talk to or talk to one that is not there. Absent in
    /// every record written before residents existed, which deserializes to
    /// `false`: exactly right, because those records are all `--print` runs.
    #[serde(default)]
    pub resident: bool,
    /// The [`super::shim::Sidecars`] id this resident's shim was started with.
    ///
    /// Not derivable from `session_id`: a resident that is respawned (a settings
    /// change, later a crash-recovery rung) gets fresh sidecars, because the
    /// outgoing shim unlinks the socket path on its way out and would take the
    /// new one's socket with it. So the journal is the index from session to
    /// shim, and the only one.
    #[serde(default)]
    pub shim: Option<String>,
    /// What the provider that started the run keeps beside the fields above,
    /// in the order it writes them: `P` as that provider reads it, and
    /// [`Payload`] while the row is read as no provider's.
    #[serde(flatten)]
    pub payload: P,
}

/// A row's provider part as it was read, before any provider reads it as its
/// own: every key the fields every run has leave over, in the row's order.
pub type Payload = serde_json::Map<String, serde_json::Value>;

impl RunRecord {
    /// This run as the provider whose `P` it is reads it.
    pub fn decode<P: DeserializeOwned>(self) -> serde_json::Result<RunRecord<P>> {
        let payload = serde_json::from_value(serde_json::Value::Object(self.payload))?;
        Ok(RunRecord {
            run_id: self.run_id,
            source: self.source,
            session_id: self.session_id,
            cwd: self.cwd,
            cli_pid: self.cli_pid,
            started_at: self.started_at,
            pid_token: self.pid_token,
            resident: self.resident,
            shim: self.shim,
            payload,
        })
    }
}

// The argv and the environment a resident was spawned with are deliberately
// **not** recorded here, and the environment is the reason: it carries the
// account's OAuth token, and this journal is an unencrypted table in the
// instance directory. The consequence is stated where it bites — a re-attached
// resident cannot be respawned until a message brings a recipe with it; see
// `claude_code::resident::Driver`.
//
// The account a managed run uses is likewise not recorded. It is already
// stored, authoritatively, as the session's binding in `accounts`, and
// adoption never rebuilds an environment — the process it adopts was spawned
// with one long ago. A second copy could only go stale.

/// The journal: the `runs` table of the instance's database.
///
/// # What moved, and what did not
///
/// The records live in the instance's `state.db`, and every read and every
/// write goes there. `runs.json` was read once ever — to fill the table on the
/// first boot after the upgrade — and is no longer read, written, renamed or
/// deleted. Leaving it where it is costs nothing: it is inert history.
///
/// The release that added the table also wrote every commit back out as
/// `runs.json`, because `droppen_supervisor::supervisor` demotes to the `.prev` binary
/// after three fast failures without asking anyone, and the `.prev` of that day
/// had never heard of the database. A rollback that found no file would have
/// adopted nothing: every resident orphaned, its queued messages lost, the
/// composer unblocked on sessions that still had a CLI attached. That floor has
/// since moved — no binary a demotion can land on predates the table — so the
/// mirror is gone.
///
/// # What the move buys
///
/// Two things. `mark_progress` — which opens a turn and takes the delivered
/// message out of the queue, and *must* do both or neither — is a transaction
/// rather than a carefully-arranged single write; the two facts no longer have
/// to share a file in order to share a fate. And a user message no longer
/// re-encodes, `fsync`s and renames the whole journal several times over: this
/// was the only store here written on anything but a user action, and it was
/// written on every step of every turn.
///
/// No lock of its own. There used to be one, held across each commit so that
/// two tasks could not interleave their rewrites of the same JSON file. Nothing
/// is left for it to protect: per-record atomicity belongs to the transaction —
/// [`RunsFile::patch`] does its read-modify-write *inside* one — and
/// cross-*process* exclusion is the instance flock's job, as it always was.
#[derive(Debug)]
pub struct RunsFile {
    db: std::sync::Arc<droppen_instance::db::Db>,
}

impl RunsFile {
    /// The journal in `db`, the instance's `state.db`. The base's own: a
    /// server's journal is its host's ([`crate::RunHost::journal`]), and no
    /// other code may open one to read or remove a run behind the drivers'
    /// backs.
    pub(crate) fn with_db(db: std::sync::Arc<droppen_instance::db::Db>) -> Self {
        Self { db }
    }

    /// A journal of a test's own, in the `state.db` beside `path`, for the
    /// tests of how each CLI's rows are written and read.
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::with_db(droppen_instance::db::at(path.into().with_file_name("state.db")))
    }

    /// Every recorded run, or nothing at all.
    ///
    /// An empty table is the ordinary case — no run has ever been started, or
    /// none was in flight when the last server stopped. A journal that will not
    /// open is not, and is deliberately treated the same way rather than
    /// refused: adoption is an optimisation, and a server that will not boot
    /// because its bookkeeping is corrupt is worse than one that boots having
    /// forgotten a run.
    pub fn read(&self) -> Vec<RunRecord> {
        self.db
            .read(|conn| {
                let mut statement = conn.prepare("SELECT payload FROM runs ORDER BY ordinal")?;
                let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
                Ok(rows
                    .filter_map(Result::ok)
                    .filter_map(|json| match serde_json::from_str::<RunRecord>(&json) {
                        Ok(record) => Some(record),
                        Err(error) => {
                            // One unreadable row costs one run, not the whole
                            // adoption. The same trade the file made, at a
                            // finer grain: a record this build cannot parse is
                            // one process left un-adopted rather than every
                            // process left un-adopted.
                            tracing::warn!(%error, "a run record cannot be read; that run will not be adopted");
                            None
                        }
                    })
                    .collect())
            })
            .unwrap_or_else(|error| {
                tracing::warn!(path = %self.db.path().display(), %error, "the run journal is unreadable; no run will be adopted");
                Vec::new()
            })
    }

    /// Record a run that has just been spawned.
    pub fn add<P: Serialize>(&self, record: RunRecord<P>) {
        self.db.commit("a started run", |tx| {
            let Ok(payload) = serde_json::to_string(&record) else {
                return Ok(());
            };
            // Delete then insert at the end rather than upsert in place: a
            // re-added run is the newest thing that happened and the file put
            // it last, which `read` still reports and adoption still relies on.
            tx.execute("DELETE FROM runs WHERE run_id = ?1", (&record.run_id,))?;
            let next: i64 = tx.query_row("SELECT COALESCE(MAX(ordinal), 0) + 1 FROM runs", [], |row| row.get(0))?;
            tx.execute(
                "INSERT INTO runs (run_id, session_id, ordinal, payload) VALUES (?1, ?2, ?3, ?4)",
                (&record.run_id, &record.session_id, next, payload.as_str()),
            )?;
            Ok(())
        });
    }

    /// Forget a run that has finished, by whatever route.
    pub fn remove(&self, run_id: &str) {
        self.db.commit("a finished run", |tx| {
            tx.execute("DELETE FROM runs WHERE run_id = ?1", (run_id,))?;
            Ok(())
        });
    }

    /// Change one record, in one transaction, and only the record named.
    ///
    /// The read-modify-write happens *inside* the transaction, which is the
    /// difference that matters: nothing can land between the `SELECT` and the
    /// `UPDATE`, so two callers marking two different things about the same run
    /// can no longer have one of them silently win. Naming a run that is not
    /// there is not an error — a turn can end after the process that was running
    /// it has already been retired.
    pub fn patch<P: Serialize + DeserializeOwned>(&self, what: &'static str, run_id: &str, change: impl FnOnce(&mut RunRecord<P>)) {
        self.db.commit(what, |tx| {
            let found: Option<String> = tx
                .query_row("SELECT payload FROM runs WHERE run_id = ?1", (run_id,), |row| row.get(0))
                .ok();
            let Some(json) = found else {
                return Ok(());
            };
            let Ok(mut record) = serde_json::from_str::<RunRecord<P>>(&json) else {
                return Ok(());
            };
            change(&mut record);
            let Ok(payload) = serde_json::to_string(&record) else {
                return Ok(());
            };
            tx.execute(
                "UPDATE runs SET session_id = ?2, payload = ?3 WHERE run_id = ?1",
                (run_id, &record.session_id, payload.as_str()),
            )?;
            Ok(())
        });
    }
}

#[cfg(any(test, feature = "test-support"))]
impl RunsFile {
    /// Put a row in as it is, `payload` and all: one another version wrote.
    pub fn insert_raw(&self, run_id: &str, session_id: &str, payload: &str) {
        self.db.commit("a row as written", |tx| {
            let next: i64 = tx.query_row("SELECT COALESCE(MAX(ordinal), 0) + 1 FROM runs", [], |row| row.get(0))?;
            tx.execute(
                "INSERT INTO runs (run_id, session_id, ordinal, payload) VALUES (?1, ?2, ?3, ?4)",
                (run_id, session_id, next, payload),
            )?;
            Ok(())
        });
    }

    /// Every row's payload as it is on disk, oldest first.
    pub fn raw(&self) -> Vec<String> {
        self.db
            .read(|conn| {
                let mut statement = conn.prepare("SELECT payload FROM runs ORDER BY ordinal")?;
                let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
                Ok(rows.filter_map(Result::ok).collect())
            })
            .expect("the journal")
    }
}
