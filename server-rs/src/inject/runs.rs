//! The record of every CLI run that could outlive this server.
//!
//! A resident `claude` or a `codex exec` run is spawned with
//! `kill_on_drop(false)` and is not on the shutdown list, precisely so that a
//! restart does not throw away a turn the user asked for (see
//! [`crate::shutdown::Cleanups`]). That has always been true; what was missing
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
//! database — the server that holds [`crate::instance::InstanceLock`] is the
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
//! (see [`crate::db`]): the commit does not return until the write-ahead log is
//! on the platter, so a run that was recorded stays recorded across a power
//! cut. That is the guarantee the old whole-file rewrite bought by hand —
//! encode, `fsync` the temporary, `rename`, `fsync` the *directory* so the
//! rename itself survives — except that it now costs one small append per
//! change instead of re-encoding and re-syncing every in-flight run several
//! times per user message. A torn journal is still the one thing adoption
//! cannot recover from; it is simply no longer this module's job to prevent it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::sessions::Source;

/// One in-flight run, as it survives on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    /// Identifies this run across a rename, so completion removes the entry it
    /// created even after the session id underneath it changed.
    pub run_id: String,
    /// Which CLI is running.
    pub source: Source,
    /// The session the run belongs to. Rewritten by
    /// [`RunsFile::repoint`] when codex announces its real thread id.
    pub session_id: String,
    /// Where the CLI was started, for the record; adoption does not re-derive
    /// anything from it, since the process is already running.
    pub cwd: Option<String>,
    /// The CLI's pid — the thing adoption actually probes.
    pub cli_pid: u32,
    /// When the run started, ISO-8601, so an adopted run does not claim to have
    /// begun at the moment the server restarted.
    pub started_at: String,
    /// The pid-reuse guard: see [`super::proc`]. `None` on a platform that has
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
    /// What this resident's command line settled: model, effort, permission
    /// mode, account, persona, uploads.
    ///
    /// Recorded because a re-attached resident has to answer "may this message
    /// go to the process that is already running" — and the answer is a
    /// comparison against the settings the process was *started* with, which
    /// nothing else on disk remembers. Getting it wrong in either direction
    /// costs: a false mismatch throws a warm session away on the first message
    /// after every restart, and a false match sends an opus message to a
    /// haiku process.
    ///
    /// Every field is a setting the user chose, and none is a secret. `None`
    /// in a record from before this existed, which reads as "unknown" and is
    /// handled as such rather than as "no settings at all" — see
    /// [`super::resident::Fingerprint`].
    ///
    /// Written at the spawn and then *healed* rather than left alone: an
    /// unknown one is filled in by the first message an adopted resident is
    /// given, and the model is rewritten when `system:init` resolves the alias
    /// the command line asked for into the name the CLI answers under. Both are
    /// [`RunsFile::mark_fingerprint`], and both exist so that the record the
    /// next server reads is what the process is really running rather than what
    /// some older server managed to write down.
    #[serde(default)]
    pub fingerprint: Option<super::resident::Fingerprint>,
    /// Where in claude's stdout a turn was written into this resident, if one
    /// was still in flight when this was last written.
    ///
    /// The one thing a re-attached server cannot work out for itself. A turn
    /// begins when a prompt is written to claude's *stdin*, and the shim records
    /// only its stdout — so a turn that has said nothing yet, which is every
    /// turn spent thinking or inside a long tool call, is invisible in the
    /// replay. Without this a restart in that window reads the session as idle,
    /// unblocks the composer, and lets the next message be written into the
    /// middle of a turn that is very much still running.
    ///
    /// A byte offset rather than a flag, because *when* is the whole question: a
    /// turn that ended while no server was attached left its `result` in the
    /// shim's ring, and adoption has to be able to tell that `result` from the
    /// ones belonging to earlier turns. The offset is the reader's cursor at the
    /// moment the prompt went in, so everything before it is history and the
    /// first `result` after it is this turn's end. `None` means "no turn was in
    /// flight", which is also what every record written before this existed says.
    #[serde(default)]
    pub turn_at: Option<u64>,
    /// The messages accepted for this resident and not yet written to it.
    ///
    /// The other half of what used to die with a server. A send is answered
    /// `200` the moment it is queued — that is what lets a second message be
    /// composed while the first is still running — and the queue itself was in
    /// memory, so a restart between "accepted" and "written to claude's stdin"
    /// threw the message away in silence. That window is not small: a message
    /// sent behind a turn sits in it for as long as the turn takes.
    ///
    /// In order, popped as each one goes in, and empty the rest of the time.
    #[serde(default)]
    pub queued: Vec<Queued>,
    /// Codex App Server delivery state and replay checkpoint. Absent for legacy
    /// exec runs and Claude residents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<super::app_server::State>,
    /// The stored account a codex app-server was started under.
    ///
    /// The codex half of [`RunRecord::fingerprint`], and a field of its own
    /// rather than a reuse of it because the two are read by different drivers
    /// and the claude one carries six settings this has no opinion about. An id
    /// and never a secret. `None` is the machine's own `~/.codex` login, which
    /// is what every record written before accounts had a second CLI means.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_account: Option<String>,
    /// A claude resident's queue held after a limit with no account to move
    /// to, so the next server keeps holding it rather than feeding the queue
    /// to the account that refused. See [`super::resident::Hold`]. Absent on
    /// every record that is not holding anything, and on every record written
    /// before there was a hold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<super::resident::Hold>,
}

/// One message waiting its turn, as it survives on disk.
///
/// The prompt is here in plain text, which is a line worth saying out loud why
/// it is crossed: it is one step away from being written into the session
/// transcript — a plain file in the same data directory, holding every prompt
/// the session has ever had — so this reveals nothing the transcript does not,
/// and it holds it only until claude has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queued {
    /// What the queue routes name this message by, kept across a restart so a
    /// client holding an id from before it still holds a usable one.
    ///
    /// Empty in every record written before the queue was addressable, which
    /// [`super::resident`] answers by minting a fresh id on the way back in:
    /// the message survives, only its name changes.
    #[serde(default)]
    pub id: String,
    /// The prompt as claude will see it, attachment suffix included.
    pub text: String,
    /// The process this message needs, so a re-attached server can tell whether
    /// the resident it inherited may answer it at all.
    ///
    /// The environment is not here and cannot be — it carries the account's
    /// token — so a message that does *not* fit the running process cannot be
    /// re-delivered by a server that did not start that process. Recording what
    /// it wanted is what lets adoption notice, and say so, instead of sending an
    /// opus message to a haiku process.
    #[serde(default)]
    pub fingerprint: super::resident::Fingerprint,
    /// Whether this message had already been written to the running process'
    /// stdin, ahead of its turn, when this was recorded.
    ///
    /// The difference matters to exactly one reader: the server that re-attaches
    /// to the process this record names. That process still holds the line in
    /// its stdin buffer and will run it, so re-delivering it would run it twice
    /// — see [`super::resident::inherited_queue`]. A record read back against
    /// any *other* process (there is no such path today) would have to treat it
    /// as undelivered.
    #[serde(default)]
    pub steered: bool,
    /// How many times an automatic account rotation has already moved the
    /// message chain this one belongs to: `0` for anything a person sent, and
    /// the count for the continuation a rotation queued in its place. Kept so
    /// the cap in [`crate::accounts::rotation::ROTATION_CAP`] survives a
    /// restart; absent, and zero, on every record written before there was one.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation_attempt: u8,
    /// For a rotation's continuation, what it needs to say which background
    /// tasks its respawn cut short — kept so a continuation parked behind
    /// background work survives a restart whole. Absent on everything else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<super::resident::Continuation>,
}

/// Whether a counter is at its default, so a record that never rotated is
/// written exactly as it always was.
fn is_zero(value: &u8) -> bool {
    *value == 0
}

// The argv and the environment a resident was spawned with are deliberately
// **not** recorded here, and the environment is the reason: it carries the
// account's OAuth token, and this journal is an unencrypted table in the
// instance directory. The consequence is stated where it bites — a re-attached
// resident cannot be respawned until a message brings a recipe with it; see
// [`super::resident::Driver`].
//
// The account a managed run uses is likewise not recorded. It is already
// stored, authoritatively, as the session's binding in `crate::accounts`, and
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
/// `runs.json`, because [`crate::supervisor`] demotes to the `.prev` binary
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
    db: std::sync::Arc<crate::db::Db>,
}

impl RunsFile {
    /// The journal beside `path`, in the same directory's `state.db`.
    ///
    /// Derived from the old file's path rather than passed in, so that every
    /// caller and every test that already knew where `runs.json` goes needs to
    /// learn nothing new.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        Self {
            db: crate::db::at(path.with_file_name("state.db")),
        }
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
    pub fn add(&self, record: RunRecord) {
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

    /// Say what a resident is in the middle of: the turn in flight, and what is
    /// still waiting behind it.
    ///
    /// Both in one transaction, and that is the point rather than an
    /// optimisation. Handing a message to claude moves it out of the queue *and*
    /// opens a turn, and a crash between two separate commits would leave the
    /// record claiming either that a delivered message is still waiting — so the
    /// next boot sends it twice — or that neither is happening, which unblocks
    /// the composer on a live turn. One transaction has neither state.
    pub fn mark_progress(&self, run_id: &str, turn_at: Option<u64>, queued: &[Queued]) {
        let queued = queued.to_vec();
        self.patch("progress on a run", run_id, move |run| {
            run.turn_at = turn_at;
            run.queued = queued;
        });
    }

    /// Say whether this resident's queue is held after a limit, and on what.
    pub fn mark_hold(&self, run_id: &str, hold: Option<super::resident::Hold>) {
        self.patch("a held queue", run_id, move |run| {
            run.hold = hold;
        });
    }

    /// Say what the process this record names is really running.
    ///
    /// A write of its own rather than another argument to
    /// [`RunsFile::mark_progress`], because it happens on a different rhythm:
    /// the settings are fixed for a process' whole life and are recorded at the
    /// spawn, so this is only ever a *correction* — the record that predates the
    /// field and whose driver has since been told what it is holding, and the
    /// model alias `system:init` resolves. Rare, and each one saves the next
    /// server from having to learn the same thing again.
    ///
    /// The account is part of what is written, and is the reason this is not
    /// simply left to the next spawn: a record that says nothing is one the next
    /// boot cannot compare a message against, and comparing against the default
    /// instead is what threw queued messages away after an upgrade.
    pub fn mark_fingerprint(&self, run_id: &str, fingerprint: &super::resident::Fingerprint) {
        let fingerprint = fingerprint.clone();
        self.patch("what a run is holding", run_id, move |run| {
            run.fingerprint = Some(fingerprint);
        });
    }

    /// Point a run at the session id the CLI actually chose.
    ///
    /// Claude's `--fork-session` adopt path needs this: the CLI may keep the
    /// session under an id of its own choosing rather than the one the fork
    /// asked for, and announces it on `system:init`, so a run is recorded under
    /// a provisional id first. Without the rewrite, adoption would re-arm the
    /// idle watchdog on a transcript that does not exist.
    pub fn repoint(&self, run_id: &str, session_id: &str) {
        let session_id = session_id.to_string();
        self.patch("the session a run belongs to", run_id, move |run| {
            run.session_id = session_id;
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
    fn patch(&self, what: &'static str, run_id: &str, change: impl FnOnce(&mut RunRecord)) {
        self.db.commit(what, |tx| {
            let found: Option<String> = tx
                .query_row("SELECT payload FROM runs WHERE run_id = ?1", (run_id,), |row| row.get(0))
                .ok();
            let Some(json) = found else {
                return Ok(());
            };
            let Ok(mut record) = serde_json::from_str::<RunRecord>(&json) else {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn record(run_id: &str, session_id: &str, pid: u32) -> RunRecord {
        RunRecord {
            codex: None,
            codex_account: None,
            hold: None,
            run_id: run_id.to_string(),
            source: Source::Claude,
            session_id: session_id.to_string(),
            cwd: Some("/workspace".to_string()),
            cli_pid: pid,
            started_at: "2026-01-01T00:00:00.000Z".to_string(),
            pid_token: Some("123.000000".to_string()),
            resident: false,
            shim: None,
            fingerprint: None,
            turn_at: None,
            queued: Vec::new(),
        }
    }

    #[test]
    fn a_run_is_recorded_until_it_finishes_and_no_longer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = RunsFile::new(dir.path().join("runs.json"));

        assert!(file.read().is_empty(), "an empty database is an empty journal, not an error");

        file.add(record("run-1", "session-a", 4242));
        file.add(record("run-2", "session-b", 4243));
        let runs = file.read();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].session_id, "session-a");
        assert_eq!(runs[0].cli_pid, 4242);
        assert_eq!(runs[0].source, Source::Claude);
        assert_eq!(runs[0].pid_token.as_deref(), Some("123.000000"));

        // Re-adding one run replaces it rather than duplicating it, so a retry
        // cannot leave two records pointing at one process.
        file.add(record("run-1", "session-a", 5555));
        assert_eq!(file.read().len(), 2);
        assert_eq!(file.read()[1].cli_pid, 5555, "the newest wins and moves to the end");

        file.repoint("run-2", "0199c1a1-2b3c-4d5e-8f60-112233445566");
        assert_eq!(file.read()[0].session_id, "0199c1a1-2b3c-4d5e-8f60-112233445566");

        // The turn mark and the queue go on and come off the record they name,
        // and only it: a resident answering one session must not mark another's
        // turn open or inherit its backlog.
        assert!(file.read().iter().all(|run| run.turn_at.is_none()), "no turn is in flight to begin with");
        let waiting = vec![Queued {
            id: "q-1".to_string(),
            text: "next please".to_string(),
            fingerprint: Default::default(),
            steered: false,
            rotation_attempt: 0,
            continuation: None,
        }];
        file.mark_progress("run-1", Some(4096), &waiting);
        let runs = file.read();
        let marked = runs.iter().find(|run| run.run_id == "run-1").expect("run-1");
        assert_eq!(marked.turn_at, Some(4096));
        assert_eq!(marked.queued, waiting);
        let untouched = runs.iter().find(|run| run.run_id == "run-2").expect("run-2");
        assert_eq!(untouched.turn_at, None);
        assert!(untouched.queued.is_empty());
        file.mark_progress("run-1", None, &[]);
        let runs = file.read();
        assert!(runs.iter().all(|run| run.turn_at.is_none()), "and the turn ending takes it off again");
        assert!(runs.iter().all(|run| run.queued.is_empty()), "and the delivered message with it");
        // Marking a run that is not there is not an error: a turn can end after
        // the process that was running it has already been retired.
        file.mark_progress("run-gone", Some(1), &waiting);

        file.remove("run-2");
        assert_eq!(file.read().len(), 1);
        file.remove("run-1");
        assert!(file.read().is_empty(), "a finished run leaves nothing behind");
        // Removing what is not there is not an error: completion paths overlap.
        file.remove("run-1");
    }

    /// A record can be less than the truth, and saying so is the point of being
    /// able to rewrite it.
    ///
    /// Two ways it happens, both to a *running* process: the record predates the
    /// field, so it says nothing at all; or the CLI resolved the model alias the
    /// command line asked for into the name it answers under. Either way the
    /// next server reads this journal and nothing else, so what it says has to
    /// become what is true — a record left saying "unknown" is one every later
    /// boot has to guess at again.
    #[test]
    fn a_fingerprint_learned_after_the_spawn_replaces_what_the_record_said() {
        use super::super::resident::Fingerprint;

        let dir = tempfile::tempdir().expect("tempdir");
        let file = RunsFile::new(dir.path().join("runs.json"));
        file.add(record("run-1", "session-a", 4242));
        file.add(record("run-2", "session-b", 4243));
        assert!(file.read()[0].fingerprint.is_none(), "a record from before the field says nothing");

        let learned = Fingerprint {
            model: Some("claude-opus-4-5".to_string()),
            account: Some("work".to_string()),
            ..Default::default()
        };
        file.mark_fingerprint("run-1", &learned);
        let runs = file.read();
        let healed = runs.iter().find(|run| run.run_id == "run-1").expect("run-1");
        assert_eq!(healed.fingerprint.as_ref(), Some(&learned));
        let untouched = runs.iter().find(|run| run.run_id == "run-2").expect("run-2");
        assert!(untouched.fingerprint.is_none(), "and only the run it names");

        // A correction of a correction is the model alias case, and it wins:
        // the newest thing the driver was told is the one the process answers to.
        let resolved = Fingerprint {
            model: Some("claude-opus-4-5-20260101".to_string()),
            ..learned.clone()
        };
        file.mark_fingerprint("run-1", &resolved);
        assert_eq!(file.read()[0].fingerprint.as_ref(), Some(&resolved));
        // Naming a run that is not there is not an error, for the same reason
        // marking progress on one is not: a driver can outlive its record.
        file.mark_fingerprint("run-gone", &resolved);
    }

    /// The table is the journal, and `runs.json` is neither written nor read.
    ///
    /// This is the whole of what the move bought, stated as a test. Before, this
    /// file *was* the journal, and a truncated one meant every in-flight run was
    /// orphaned — the composer unblocked on sessions that still had a CLI
    /// attached. Then it was a mirror, rewritten whole on every step of every
    /// turn for a rolled-back binary's sake. Now nothing touches it: a turn costs
    /// no whole-file encode, no `fsync` and no rename, and whatever the file
    /// happens to contain neither helps nor hurts.
    #[test]
    fn the_file_the_table_replaced_is_neither_written_nor_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("runs.json");
        let file = RunsFile::new(&path);
        file.add(record("run-1", "session-a", 4242));
        file.add(record("run-2", "session-b", 4243));
        assert!(!path.exists(), "recording a run writes the table and nothing else");

        // A file that appears afterwards — a rollback's leavings, a restore done
        // by hand — is left exactly as found. Nothing reads it, so it is not
        // adopted, and there is no writer left to overwrite it.
        let planted = b"{\"runs\": [{\"run_id\"".to_vec();
        std::fs::write(&path, &planted).expect("write");
        file.remove("run-2");
        file.mark_progress("run-1", Some(4096), &[]);
        assert_eq!(file.read().len(), 1, "the journal is the table, not the file beside it");
        assert_eq!(std::fs::read(&path).expect("read"), planted, "and the file is byte for byte what it was");
        assert!(
            !dir.path().join("runs.json.tmp").exists(),
            "no temporary either: there is nothing left that writes through one"
        );
    }

    /// What a restarting server sees is what the last one committed.
    #[test]
    fn a_run_survives_the_server_that_recorded_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("runs.json");
        {
            let file = RunsFile::new(&path);
            file.add(record("run-1", "session-a", 4242));
            file.mark_progress("run-1", Some(4096), &[]);
        }
        // A second `RunsFile` over the same directory is the next boot.
        let next = RunsFile::new(&path);
        let runs = next.read();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].cli_pid, 4242);
        assert_eq!(runs[0].turn_at, Some(4096), "including what it was in the middle of");
    }

    /// A row that will not parse costs that run, not the whole adoption.
    #[test]
    fn a_corrupt_journal_forgets_runs_instead_of_refusing_to_boot() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("runs.json");
        let db = crate::db::at(path.with_file_name("state.db"));
        db.write(|tx| {
            tx.execute(
                "INSERT INTO runs (run_id, session_id, ordinal, payload) VALUES ('half-written', 'session-a', 0, ?1)",
                (r#"{"run_id":"half-writ"#,),
            )
        })
        .expect("a half-written row");

        let file = RunsFile::new(&path);
        assert!(file.read().is_empty());

        // And it costs only the run in the rubbish: the table came up otherwise
        // whole, so the next run is recorded normally.
        file.add(record("run-1", "session-a", 7));
        assert_eq!(file.read().len(), 1);
        assert!(!path.exists(), "and nothing here creates a file");
    }
}
