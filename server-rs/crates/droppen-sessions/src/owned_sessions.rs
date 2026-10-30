//! The sessions this instance made whose transcripts live somewhere else.
//!
//! Ownership is "this server made it", and for most Claude sessions that is a
//! path: an account-backed session lands in the Claude root this instance owns,
//! so the directory answers the question with no bookkeeping at all. Two kinds of
//! session cannot be answered that way, and both are ordinary rather than exotic:
//!
//! * **codex**, which has one `~/.codex` and no documented way to move it;
//! * **a claude session started on the local login** — `accountId: null`, or an
//!   instant chat with no default account. It is deliberately spawned with the
//!   user's own `~/.claude` (that is what "local login" means), so its transcript
//!   lands in a directory this server does not own.
//!
//! Without a record those two would be external the moment they were created:
//! the server would refuse to send the *second* message to a session it had just
//! started. So the instance's `owned_sessions` table records them, and scope
//! reads "in my root **or** in my registry".
//!
//! What the registry deliberately does *not* change is how a session spawns. A
//! registry-owned local session resumes exactly as it always did — bare
//! environment, no `CLAUDE_CONFIG_DIR`, the user's own login — because it is the
//! user's own session in every sense except who is allowed to drive it from here.
//! Ownership decides *who may resume*; the root decides *which login resumes*.
//!
//! The registry is per-instance, so a sibling server never inherits these. For a
//! claude session on the local login that is the whole answer: instance B lists
//! A's creation under `external`, which is exactly right — it is a session in
//! the user's own `~/.claude`, B did not make it, and B must not append turns to
//! it.
//!
//! For codex it is half the answer. A registry can only ever say "mine": it is
//! deliberately never read across instances, so it cannot say "someone else's",
//! and B would go on listing every thread A made as though the user had started
//! it. The other half is `droppen_provider_codex::sessions::codex_claims` — a claim in the *shared* data
//! directory, which B does read, and on which B hides the row altogether. That
//! is the same silence an account-backed claude session in A's own Claude root
//! already gets for free, arrived at the only way one `~/.codex` allows.
//!
//! Read tolerantly. A record that cannot be parsed costs the ownership of the
//! sessions it named — they read as external, which is the safe direction:
//! external is browsable and refuses to be driven, while a wrongly owned session
//! would be driven by two servers at once.
//!
//! # Where the registry actually is
//!
//! In [`droppen_instance::db`], as `owned_sessions`, and the table's shape is the point:
//! its primary key is `(source, session_id)`, so "the same id under two sources"
//! — the one thing `{"claude": [...], "codex": [...]}` could always express and
//! never mean — is not a row that can be written.
//!
//! `owned-sessions.json` was written for a while after the move: `app`
//! calls this the one piece of ownership state that cannot be rebuilt from the
//! filesystem, so a `droppen_supervisor::supervisor` demotion to a `.prev` binary that had
//! never heard of the database would have had nothing at all. No binary that
//! old is left to demote to, and the file is no longer read: one left on disk
//! is inert.
//!
//! Nor is there a write lock any more. Every change here is a single statement —
//! `INSERT OR IGNORE`, `DELETE` — so the transaction is the whole of the
//! exclusion this needs; the mutex existed to stop two requests interleaving
//! their whole-file rewrites of the JSON.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;

use super::{Source, Transcripts};
use droppen_instance::db::Db;
use droppen_instance::identity::ServerId;
use droppen_instance::paths::Paths;

/// The sources a row may name, by wire name: the CLIs whose sessions this
/// instance records. A row naming any other is dropped ([`stored`]); an ACP
/// session is owned by where its transcript is kept, not by a row here (see
/// `droppen_provider_acp::sessions`).
const RECORDED: [&str; 2] = ["claude", "codex"];

/// The ids one instance claims, per CLI, each with the server that made it.
///
/// A map rather than a set since the rows carry a `server_id`: the value is the
/// server the row was recorded on, which today is always the one reading it and
/// tomorrow need not be.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OwnedSessions {
    by_source: BTreeMap<Source, BTreeMap<String, ServerId>>,
}

impl OwnedSessions {
    /// Whether this instance recorded creating this session.
    pub fn contains(&self, source: Source, id: &str) -> bool {
        self.set(source).contains_key(id)
    }

    /// Which server recorded creating this session, if any did.
    ///
    /// `None` is "no row", not "no server": a row whose `server_id` is `NULL`
    /// answers with the local server, for the reason [`stored`] sets out.
    pub fn server_of(&self, source: Source, id: &str) -> Option<&ServerId> {
        self.set(source).get(id)
    }

    /// Whether anything is recorded at all, so a caller can skip a pass.
    pub fn is_empty(&self) -> bool {
        self.by_source.values().all(BTreeMap::is_empty)
    }

    /// Every id recorded for one CLI, for a pass that has to visit them all.
    pub fn ids(&self, source: Source) -> impl Iterator<Item = &str> {
        self.set(source).keys().map(String::as_str)
    }

    fn set(&self, source: Source) -> &BTreeMap<String, ServerId> {
        static NONE: BTreeMap<String, ServerId> = BTreeMap::new();
        self.by_source.get(&source).unwrap_or(&NONE)
    }

    /// Record that this instance created session `id` of `reader`'s CLI.
    ///
    /// For codex this is called on the *successful* end of the handshake, once the
    /// CLI has announced the thread id it actually used; recording the provisional
    /// id would own a session that never existed. For claude the id is minted before
    /// the spawn, so it is recorded before it — a crash mid-launch leaves an entry
    /// for a session with no transcript, which nothing lists and nothing resumes.
    ///
    /// The only way this module writes a row, and the row's source is the
    /// reader's own: the reader's [`Transcripts::on_owned`] runs right after
    /// it, so no row is written without what its CLI needs beside it. For Codex
    /// that is the thread's claim in the shared `<data>/codex-claims`; without
    /// the claim the sibling instances go on listing the thread. The row's
    /// outcome is the answer: the step after it is best effort, and it runs
    /// whichever way the row went, so a second record of the same id retries it.
    ///
    /// A source the registry is not written for (`RECORDED`) gets no row, since
    /// reading would drop it; its CLI's step still runs.
    pub async fn record(paths: &Paths, reader: &dyn Transcripts, id: &str) -> std::io::Result<()> {
        let source = reader.source();
        let outcome = if RECORDED.contains(&source.as_str()) { insert(paths, source, id).await } else { Ok(()) };
        reader.on_owned(paths, id).await;
        outcome
    }
}

/// Record `id` as this instance's own under `source` without that CLI's
/// reader: for a test of one CLI's reader that needs another CLI's row beside
/// its own.
#[cfg(any(test, feature = "test-support"))]
pub async fn record_as(paths: &Paths, source: Source, id: &str) -> std::io::Result<()> {
    insert(paths, source, id).await
}

/// This instance's database.
///
/// No `stat`-keyed parse cache any more, and its absence is deliberate rather
/// than an oversight. The cache existed because this is read once per session
/// listing — every five seconds, for every attached client — and re-parsing a
/// file that often was the thing worth avoiding. Two indexed queries against a
/// table of tens of rows are cheaper than the `stat` the cache needed to decide
/// it could skip the parse, and a cache keyed on the file's mtime would now be
/// keyed on a file nothing writes.
fn store(paths: &Paths) -> Arc<Db> {
    droppen_instance::db::at(paths.instance_db())
}

/// The sessions this instance created, or an empty registry when there is none.
///
/// A row naming a CLI this build has never heard of is dropped rather than
/// fatal, which lands on the safe side of the same trade the file's tolerant
/// parse made: an unrecognised row costs ownership, and an unowned session is
/// browsable but refuses to be driven.
pub async fn read(paths: &Paths) -> OwnedSessions {
    let local = ServerId::local(paths);
    stored(&store(paths), &local)
}

/// Every row, as a registry.
///
/// A row naming a CLI this build has never heard of is dropped rather than
/// fatal: an unrecognised row costs ownership, and an unowned session is
/// browsable but refuses to be driven.
///
/// # Why a `NULL` server is this one
///
/// `server_id` arrived after the rows did, so every row written before it is
/// `NULL` — and `COALESCE` resolves those to the local server rather than to
/// "unknown". That is a fact rather than a guess, and the table's own shape is
/// what makes it one: `owned_sessions` lives in the **per-instance** database
/// ([`droppen_instance::paths::Paths::instance_db`]), it is never read across instances (the
/// module note above turns on that), and the only thing that has ever inserted
/// into it is this instance recording a session it had just started. A row here
/// with no server named is therefore this server's, necessarily; there has
/// never been another writer for it to have come from.
///
/// A row that *does* name a server is taken at its word, because a later slice
/// will have rows that name another one.
fn stored(db: &Db, local: &ServerId) -> OwnedSessions {
    db.read(|conn| {
        let mut statement = conn.prepare("SELECT source, session_id, COALESCE(server_id, ?1) FROM owned_sessions")?;
        let rows = statement.query_map([local.as_str()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })?;
        let mut owned = OwnedSessions::default();
        for (source, id, server) in rows.filter_map(Result::ok) {
            // A server name that no longer passes validation — an instance
            // renamed to something this build refuses — costs the row its
            // server, not its ownership: the local name is the same answer the
            // `NULL` case gives, and it is the one this server can stand behind.
            let server = ServerId::new(&server).unwrap_or_else(|| local.clone());
            if !RECORDED.contains(&source.as_str()) {
                continue;
            }
            let Ok(source) = Source::deserialize(serde::de::IntoDeserializer::<serde::de::value::Error>::into_deserializer(source.as_str())) else {
                continue;
            };
            owned.by_source.entry(source).or_default().insert(id, server);
        }
        Ok(owned)
    })
    .unwrap_or_default()
}

/// The row that says this instance created session `id` of `source`. Written
/// only by [`OwnedSessions::record`], which takes the source from the reader
/// whose step runs after it.
async fn insert(paths: &Paths, source: Source, id: &str) -> std::io::Result<()> {
    // The server is stamped here rather than at the three call sites — a new
    // session, a new codex thread, the id claude answers an adoption with —
    // because all three are this process recording its own work, and there is
    // exactly one right answer for what to write: the server these `paths` are
    // rooted at. A call site that could pass a different one would be a call
    // site that could claim another server's session.
    let server = ServerId::local(paths);
    // `INSERT OR IGNORE` is the read-modify-write this used to do by hand, minus
    // the window between the read and the write — and recording an id twice is
    // the ordinary case on a codex re-handshake, not an error.
    store(paths)
        .write(|tx| {
            tx.execute(
                "INSERT OR IGNORE INTO owned_sessions (source, session_id, server_id) VALUES (?1, ?2, ?3)",
                (source.as_str(), id, server.as_str()),
            )
        })
        .map(|_| ())
        .map_err(std::io::Error::other)
}

/// Drop `id` from the registry, because the session no longer needs to be in it.
///
/// The caller is Claude Code's prune at boot
/// (`provider::claude_code::sessions::claude::ClaudeCodeSessions::prune_owned`): an entry for a
/// transcript that is in neither root stands for a session nothing lists or
/// resumes.
pub async fn forget(paths: &Paths, source: Source, id: &str) -> std::io::Result<()> {
    store(paths)
        .write(|tx| tx.execute("DELETE FROM owned_sessions WHERE source = ?1 AND session_id = ?2", (source.as_str(), id)))
        .map(|_| ())
        .map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ContextUsage;
    use crate::sources::test_reader::TestReader;
    use crate::util::TranscriptItem;
    use crate::{CliChoice, SessionStatus, SessionSummary};
    use futures_util::future::BoxFuture;
    use serde_json::{Map, Value};
    use std::path::{Path, PathBuf};

    fn paths(dir: &tempfile::TempDir) -> Paths {
        Paths::new(dir.path().to_path_buf())
    }

    #[tokio::test]
    async fn an_absent_registry_owns_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(read(&paths(&dir)).await.is_empty());
    }

    #[tokio::test]
    async fn each_cli_is_recorded_separately_and_recording_twice_is_a_no_op() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = paths(&dir);
        insert(&paths, Source::Codex, "thread-a").await.expect("record");
        insert(&paths, Source::Claude, "session-a").await.expect("record");
        insert(&paths, Source::Codex, "thread-a").await.expect("recording twice is fine");

        let owned = read(&paths).await;
        assert!(owned.contains(Source::Codex, "thread-a"));
        assert!(owned.contains(Source::Claude, "session-a"));
        assert!(!owned.contains(Source::Claude, "thread-a"), "one CLI's id is not the other's");
    }

    /// A change writes the table and nothing else: `owned-sessions.json` is no
    /// longer read, so a registry left beside the database is inert.
    #[tokio::test]
    async fn no_change_writes_the_file_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = paths(&dir);
        insert(&paths, Source::Codex, "thread-a").await.expect("record");
        assert!(!paths.instance_dir().join("owned-sessions.json").exists(), "recording writes the table and nothing else");
        assert!(!paths.instance_dir().join("owned-sessions.json").with_extension("json.tmp").exists());

        // A registry that appears afterwards is left exactly as found: nothing
        // reads it, and there is no writer left to overwrite it.
        let file = paths.instance_dir().join("owned-sessions.json");
        std::fs::create_dir_all(file.parent().expect("parent")).expect("instance dir");
        let planted = r#"{"claude":["planted"],"codex":[]}"#;
        std::fs::write(&file, planted).expect("plant");
        insert(&paths, Source::Claude, "session-b").await.expect("record");
        forget(&paths, Source::Codex, "thread-a").await.expect("forget");
        assert!(!read(&paths).await.contains(Source::Claude, "planted"), "and it is not adopted");
        assert_eq!(std::fs::read_to_string(&file).expect("read"), planted);
    }

    /// Only the sources the registry is written for are read back: a row naming
    /// `acp`, whose sessions are owned by where their transcript is kept, or a
    /// CLI this build has never heard of, owns nothing.
    #[tokio::test]
    async fn a_row_for_a_source_the_registry_is_not_written_for_owns_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = paths(&dir);
        store(&paths)
            .write(|tx| {
                tx.execute("INSERT INTO owned_sessions (source, session_id) VALUES ('acp', 'x')", [])?;
                tx.execute("INSERT INTO owned_sessions (source, session_id) VALUES ('gemini', 'y')", [])
            })
            .expect("rows written by hand");

        let owned = read(&paths).await;
        assert!(owned.is_empty(), "{owned:?}");
        assert!(!owned.contains(Source::Acp, "x"));
    }

    /// The registry is the one piece of ownership state that cannot be rebuilt
    /// from the filesystem, so surviving a restart is the whole of its job.
    #[tokio::test]
    async fn an_entry_survives_the_database_being_reopened() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = paths(&dir);
        insert(&paths, Source::Codex, "thread-a").await.expect("record");

        let reopened = droppen_instance::db::Db::open(paths.instance_db());
        let source: String = reopened
            .read(|conn| conn.query_row("SELECT source FROM owned_sessions WHERE session_id = 'thread-a'", [], |row| row.get(0)))
            .expect("the row is there for anyone who opens the file");
        assert_eq!(source, "codex");
    }

    /// The composite key is what makes "the same id under two sources" a row
    /// that cannot exist, rather than two lists that happen not to overlap.
    #[tokio::test]
    async fn one_id_under_two_sources_is_two_rows_and_not_a_collision() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = paths(&dir);
        insert(&paths, Source::Claude, "shared-id").await.expect("record");
        insert(&paths, Source::Codex, "shared-id").await.expect("record");

        let owned = read(&paths).await;
        assert!(owned.contains(Source::Claude, "shared-id"));
        assert!(owned.contains(Source::Codex, "shared-id"));

        forget(&paths, Source::Claude, "shared-id").await.expect("forget");
        let owned = read(&paths).await;
        assert!(!owned.contains(Source::Claude, "shared-id"));
        assert!(owned.contains(Source::Codex, "shared-id"), "one CLI's row is not the other's");
    }

    /// The rows the previous release wrote have no server in them, and reading
    /// them as this one's is a fact rather than a guess — see [`stored`].
    ///
    /// The database is built at step 2 by hand, which is what that release left
    /// on disk: `SCHEMA` is append-only, so its first two entries are its ladder.
    #[tokio::test]
    async fn a_row_with_no_server_is_this_one_and_a_new_row_carries_its_own() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_instance(dir.path().to_path_buf(), "alice");
        let file = paths.instance_db();
        std::fs::create_dir_all(file.parent().expect("parent")).expect("instance dir");
        {
            let conn = rusqlite::Connection::open(&file).expect("open");
            for step in &droppen_instance::db::SCHEMA[..2] {
                conn.execute_batch(step).expect("the previous release's ladder");
            }
            conn.pragma_update(None, "user_version", 2).expect("version");
            conn.execute("INSERT INTO owned_sessions (source, session_id) VALUES ('claude', 'made-before')", [])
                .expect("a session that release started");
            conn.execute("INSERT INTO owned_sessions (source, session_id) VALUES ('codex', 'thread-before')", [])
                .expect("and a thread");
        }

        // Opening migrates; recording is what this release does differently.
        insert(&paths, Source::Claude, "made-after").await.expect("record");

        let owned = read(&paths).await;
        assert_eq!(
            owned.server_of(Source::Claude, "made-before").map(ServerId::as_str),
            Some("alice"),
            "a row with no server is this server's"
        );
        assert_eq!(owned.server_of(Source::Codex, "thread-before").map(ServerId::as_str), Some("alice"));
        assert_eq!(
            owned.server_of(Source::Claude, "made-after").map(ServerId::as_str),
            Some("alice"),
            "and so is one that says so"
        );
        assert!(owned.contains(Source::Claude, "made-before"), "ownership is unchanged by any of this");

        let db = droppen_instance::db::at(paths.instance_db());
        assert_eq!(db.user_version(), droppen_instance::db::SCHEMA.len() as i64, "the ladder ran");
        let stamped: Option<String> = db
            .read(|conn| {
                conn.query_row("SELECT server_id FROM owned_sessions WHERE session_id = 'made-after'", [], |row| {
                    row.get(0)
                })
            })
            .expect("read");
        assert_eq!(stamped.as_deref(), Some("alice"), "the new row carries its server in the table");
        let untouched: Option<String> = db
            .read(|conn| {
                conn.query_row("SELECT server_id FROM owned_sessions WHERE session_id = 'made-before'", [], |row| {
                    row.get(0)
                })
            })
            .expect("read");
        assert_eq!(untouched, None, "and the old one is still exactly as it was written");
    }

    /// Two instances are two servers, and each stamps its own name.
    #[tokio::test]
    async fn each_instance_stamps_itself() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = Paths::for_instance(dir.path().to_path_buf(), "alice");
        let b = Paths::for_instance(dir.path().to_path_buf(), "bob");
        insert(&a, Source::Codex, "thread-a").await.expect("record");
        insert(&b, Source::Codex, "thread-b").await.expect("record");

        assert_eq!(read(&a).await.server_of(Source::Codex, "thread-a").map(ServerId::as_str), Some("alice"));
        assert_eq!(
            read(&b).await.server_of(Source::Codex, "thread-b").map(ServerId::as_str),
            Some("bob")
        );
        assert_eq!(
            read(&b).await.server_of(Source::Codex, "thread-a"),
            None,
            "and neither is in the other's registry"
        );
    }

    /// Two instances keep two registries, which is what stops one from claiming
    /// the other's local-login sessions in the `~/.claude` they share.
    #[tokio::test]
    async fn two_instances_record_separately() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = Paths::for_instance(dir.path().to_path_buf(), "a");
        let b = Paths::for_instance(dir.path().to_path_buf(), "b");
        insert(&a, Source::Claude, "session-a").await.expect("record");
        assert!(read(&a).await.contains(Source::Claude, "session-a"));
        assert!(read(&b).await.is_empty(), "a sibling's registry is not this one's");
    }

    /// The core's stand-in reader, whose step after a row notes whether the
    /// row was already there.
    struct Witness {
        reader: TestReader,
        saw_row: std::sync::Mutex<Vec<bool>>,
    }

    impl Witness {
        fn of(source: Source) -> Self {
            Self {
                reader: TestReader(source),
                saw_row: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn saw_row(&self) -> Vec<bool> {
            self.saw_row.lock().expect("witness").clone()
        }
    }

    impl Transcripts for Witness {
        fn source(&self) -> Source {
            self.reader.source()
        }

        fn list<'a>(&'a self, paths: &'a Paths, bindings: &'a BTreeMap<String, String>) -> BoxFuture<'a, Vec<SessionSummary>> {
            self.reader.list(paths, bindings)
        }

        fn find<'a>(&'a self, paths: &'a Paths, id: &'a str) -> BoxFuture<'a, Option<SessionSummary>> {
            self.reader.find(paths, id)
        }

        fn find_root<'a>(&'a self, paths: &'a Paths, id: &'a str) -> BoxFuture<'a, Option<droppen_instance::paths::ClaudeRoot>> {
            self.reader.find_root(paths, id)
        }

        fn has_own_sessions<'a>(&'a self, paths: &'a Paths) -> BoxFuture<'a, bool> {
            self.reader.has_own_sessions(paths)
        }

        fn parse<'a>(&'a self, paths: &'a Paths, file_path: &'a Path) -> BoxFuture<'a, std::io::Result<Vec<TranscriptItem>>> {
            self.reader.parse(paths, file_path)
        }

        fn cli_choice<'a>(&'a self, file_path: &'a Path) -> BoxFuture<'a, Option<CliChoice>> {
            self.reader.cli_choice(file_path)
        }

        fn context<'a>(&'a self, file_path: &'a Path) -> BoxFuture<'a, Option<ContextUsage>> {
            self.reader.context(file_path)
        }

        fn spoken_texts(&self, entry: &Map<String, Value>) -> Vec<String> {
            self.reader.spoken_texts(entry)
        }

        fn external_reason(&self) -> String {
            self.reader.external_reason()
        }

        fn live_block(&self, status: SessionStatus) -> Option<String> {
            self.reader.live_block(status)
        }

        fn foreign_sessions(&self) -> bool {
            self.reader.foreign_sessions()
        }

        fn on_owned<'a>(&'a self, paths: &'a Paths, id: &'a str) -> BoxFuture<'a, ()> {
            Box::pin(async move {
                let row = read(paths).await.contains(self.source(), id);
                self.saw_row.lock().expect("witness").push(row);
            })
        }

        fn watch_dirs<'a>(&'a self, paths: &'a Paths) -> BoxFuture<'a, Vec<PathBuf>> {
            self.reader.watch_dirs(paths)
        }
    }

    /// The row first, then its CLI's step, every time. Claim first, and a row
    /// that then fails leaves a claim with no row: a thread every sibling hides
    /// and this instance does not own, which nothing heals. A row whose claim
    /// failed is healed at boot (`droppen_provider_codex::sessions::codex_claims::migrate`).
    #[tokio::test]
    async fn a_row_is_written_before_its_cli_s_step_runs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = paths(&dir);
        let codex = Witness::of(Source::Codex);
        OwnedSessions::record(&paths, &codex, "thread-a").await.expect("record");
        OwnedSessions::record(&paths, &codex, "thread-a").await.expect("recording twice is fine");
        assert_eq!(codex.saw_row(), [true, true], "the step ran once per record, each time after the row");
        assert!(read(&paths).await.contains(Source::Codex, "thread-a"));
    }

    /// No row for a source the registry is not written for: reading would drop
    /// it, so it would sit in the table owning nothing. The CLI's step still runs.
    #[tokio::test]
    async fn a_source_the_registry_is_not_written_for_gets_no_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = paths(&dir);
        let acp = Witness::of(Source::Acp);
        OwnedSessions::record(&paths, &acp, "acp-a").await.expect("nothing to fail");
        assert_eq!(acp.saw_row(), [false], "its step ran, with no row before it");
        let rows: i64 = store(&paths)
            .read(|conn| conn.query_row("SELECT COUNT(*) FROM owned_sessions", [], |row| row.get(0)))
            .expect("count");
        assert_eq!(rows, 0, "and the table holds nothing");
    }
}
