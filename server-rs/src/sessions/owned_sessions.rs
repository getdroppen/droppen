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
//! it. The other half is [`super::codex_claims`] — a claim in the *shared* data
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
//! In [`crate::db`], as `owned_sessions`, and the table's shape is the point:
//! its primary key is `(source, session_id)`, so "the same id under two sources"
//! — the one thing `{"claude": [...], "codex": [...]}` could always express and
//! never mean — is not a row that can be written.
//!
//! `owned-sessions.json` was written for a while after the move: [`crate::app`]
//! calls this the one piece of ownership state that cannot be rebuilt from the
//! filesystem, so a [`crate::supervisor`] demotion to a `.prev` binary that had
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

use super::{Paths, ServerId, Source};
use crate::claude_roots::ClaudeRoot;
use crate::db::Db;

/// The ids one instance claims, per CLI, each with the server that made it.
///
/// A map rather than a set since the rows carry a `server_id`: the value is the
/// server the row was recorded on, which today is always the one reading it and
/// tomorrow need not be.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OwnedSessions {
    claude: BTreeMap<String, ServerId>,
    codex: BTreeMap<String, ServerId>,
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
        self.claude.is_empty() && self.codex.is_empty()
    }

    /// Every id recorded for one CLI, for a pass that has to visit them all.
    pub fn ids(&self, source: Source) -> impl Iterator<Item = &str> {
        self.set(source).keys().map(String::as_str)
    }

    fn set(&self, source: Source) -> &BTreeMap<String, ServerId> {
        match source {
            Source::Claude => &self.claude,
            Source::Codex => &self.codex,
            // Owned by where its transcript is kept, not by a record here; see
            // `crate::sessions::acp_transcript`.
            Source::Acp => {
                static NONE: BTreeMap<String, ServerId> = BTreeMap::new();
                &NONE
            }
        }
    }
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
    crate::db::at(paths.instance_db())
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
/// ([`super::Paths::instance_db`]), it is never read across instances (the
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
            match source.as_str() {
                "claude" => owned.claude.insert(id, server),
                "codex" => owned.codex.insert(id, server),
                _ => continue,
            };
        }
        Ok(owned)
    })
    .unwrap_or_default()
}

/// Record that this instance created `id`.
///
/// For codex this is called on the *successful* end of the handshake, once the
/// CLI has announced the thread id it actually used; recording the provisional
/// id would own a session that never existed. For claude the id is minted before
/// the spawn, so it is recorded before it — a crash mid-launch leaves an entry
/// for a session with no transcript, which nothing lists and nothing resumes.
///
/// A codex thread is claimed in the shared `<data>/codex-claims` at the same
/// time, and the claim is written *here* rather than at the call sites so the
/// two cannot drift: a registry entry with no claim is exactly a thread the
/// sibling instances go on listing. See [`super::codex_claims`].
pub async fn record(paths: &Paths, source: Source, id: &str) -> std::io::Result<()> {
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
    let outcome = store(paths)
        .write(|tx| {
            tx.execute(
                "INSERT OR IGNORE INTO owned_sessions (source, session_id, server_id) VALUES (?1, ?2, ?3)",
                (source.as_str(), id, server.as_str()),
            )
        })
        .map(|_| ())
        .map_err(std::io::Error::other);
    // After the registry: the claim is a separate, best-effort copy of what was
    // just recorded, and it is re-attempted on a second `record` of the same id
    // precisely so a claim that failed to land once is not permanent.
    if source == Source::Codex {
        super::codex_claims::claim(paths, id).await;
    }
    outcome
}

/// Drop `id` from the registry, because the session no longer needs to be in it.
///
/// The caller is [`prune`]: an entry for a transcript that is in neither root
/// stands for a session nothing lists or resumes. (Adoption in earlier builds
/// was the other caller, retiring the entry once the transcript had been copied
/// into this server's own root, where the directory answers instead.)
pub async fn forget(paths: &Paths, source: Source, id: &str) -> std::io::Result<()> {
    store(paths)
        .write(|tx| tx.execute("DELETE FROM owned_sessions WHERE source = ?1 AND session_id = ?2", (source.as_str(), id)))
        .map(|_| ())
        .map_err(std::io::Error::other)
}

/// Drop claude entries whose transcript is not in any root this server reads.
///
/// Two things put one there. A claude id is recorded *before* the spawn, so a
/// launch that failed leaves an entry for a session that never existed; and a
/// transcript the user deletes leaves one for a session that no longer does.
/// Neither is harmful — nothing lists an id with no file — but the registry is
/// the one piece of ownership state that cannot be rebuilt from the filesystem,
/// so it should not accumulate forever either.
///
/// **Boot only, and under the instance lock**, rather than on every load: the
/// check is a directory scan per id, and the read path runs once per list
/// refresh for every attached client. There is a narrow race — a server
/// restarted in the moment between a claude id being recorded and its CLI
/// writing the first line would prune it — and the cost of losing it is that the
/// session lists as external: still readable here, and still the user's own to
/// continue with `claude --resume`, but no longer sendable from this server.
///
/// Codex entries are deliberately left alone. `~/.codex` hides archived and
/// subagent threads from every lookup cheap enough to run here, so "not found"
/// there does not mean "gone", and disowning a thread the user merely archived
/// would be worse than keeping a stale line.
pub async fn prune(paths: &Paths) {
    let owned = read(paths).await;
    if owned.claude.is_empty() {
        return;
    }
    let mut stale: Vec<String> = Vec::new();
    for id in owned.claude.keys() {
        let found = super::claude::find_in_root(paths, ClaudeRoot::Managed, id).await.is_some()
            || super::claude::find_in_root(paths, ClaudeRoot::Local, id).await.is_some();
        if !found {
            stale.push(id.clone());
        }
    }
    if stale.is_empty() {
        return;
    }
    for id in &stale {
        if let Err(error) = forget(paths, Source::Claude, id).await {
            tracing::warn!(session = %id, %error, "cannot drop a stale ownership entry");
            return;
        }
    }
    tracing::info!(
        dropped = stale.len(),
        "dropped ownership entries for claude sessions that no longer exist"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

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
        record(&paths, Source::Codex, "thread-a").await.expect("record");
        record(&paths, Source::Claude, "session-a").await.expect("record");
        record(&paths, Source::Codex, "thread-a").await.expect("recording twice is fine");

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
        record(&paths, Source::Codex, "thread-a").await.expect("record");
        assert!(!paths.owned_sessions_file().exists(), "recording writes the table and nothing else");
        assert!(!paths.owned_sessions_file().with_extension("json.tmp").exists());

        // A registry that appears afterwards is left exactly as found: nothing
        // reads it, and there is no writer left to overwrite it.
        let file = paths.owned_sessions_file();
        std::fs::create_dir_all(file.parent().expect("parent")).expect("instance dir");
        let planted = r#"{"claude":["planted"],"codex":[]}"#;
        std::fs::write(&file, planted).expect("plant");
        record(&paths, Source::Claude, "session-b").await.expect("record");
        forget(&paths, Source::Codex, "thread-a").await.expect("forget");
        assert!(!read(&paths).await.contains(Source::Claude, "planted"), "and it is not adopted");
        assert_eq!(std::fs::read_to_string(&file).expect("read"), planted);
    }

    /// A claude id is recorded *before* its spawn, so a launch that failed
    /// leaves an entry for a session that never existed — and a transcript the
    /// user deletes leaves one for a session that no longer does.
    #[tokio::test]
    async fn the_prune_drops_claude_ids_with_no_transcript_and_keeps_the_rest() {
        use crate::claude_roots;

        let dir = tempfile::tempdir().expect("tempdir");
        let paths = paths(&dir);
        let project = claude_roots::projects_dir(&paths, ClaudeRoot::Local).join("-w");
        std::fs::create_dir_all(&project).expect("project dir");
        std::fs::write(project.join("alive.jsonl"), "{}\n").expect("a session that exists");

        record(&paths, Source::Claude, "alive").await.expect("record");
        record(&paths, Source::Claude, "never-launched").await.expect("record");
        // Codex is left alone whatever the filesystem says: `~/.codex` hides
        // archived threads from every lookup cheap enough to run at boot, so
        // "not found" there is not "gone".
        record(&paths, Source::Codex, "archived-thread").await.expect("record");

        prune(&paths).await;

        let owned = read(&paths).await;
        assert!(owned.contains(Source::Claude, "alive"));
        assert!(!owned.contains(Source::Claude, "never-launched"), "the failed launch is dropped");
        assert!(owned.contains(Source::Codex, "archived-thread"), "codex entries are never pruned");

        // Idempotent, and a no-op on a registry with nothing to drop.
        prune(&paths).await;
        assert_eq!(read(&paths).await, owned);
    }

    /// The registry is the one piece of ownership state that cannot be rebuilt
    /// from the filesystem, so surviving a restart is the whole of its job.
    #[tokio::test]
    async fn an_entry_survives_the_database_being_reopened() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = paths(&dir);
        record(&paths, Source::Codex, "thread-a").await.expect("record");

        let reopened = crate::db::Db::open(paths.instance_db());
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
        record(&paths, Source::Claude, "shared-id").await.expect("record");
        record(&paths, Source::Codex, "shared-id").await.expect("record");

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
            for step in &crate::db::SCHEMA[..2] {
                conn.execute_batch(step).expect("the previous release's ladder");
            }
            conn.pragma_update(None, "user_version", 2).expect("version");
            conn.execute("INSERT INTO owned_sessions (source, session_id) VALUES ('claude', 'made-before')", [])
                .expect("a session that release started");
            conn.execute("INSERT INTO owned_sessions (source, session_id) VALUES ('codex', 'thread-before')", [])
                .expect("and a thread");
        }

        // Opening migrates; recording is what this release does differently.
        record(&paths, Source::Claude, "made-after").await.expect("record");

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

        let db = crate::db::at(paths.instance_db());
        assert_eq!(db.user_version(), crate::db::SCHEMA.len() as i64, "the ladder ran");
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
        record(&a, Source::Codex, "thread-a").await.expect("record");
        record(&b, Source::Codex, "thread-b").await.expect("record");

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
        record(&a, Source::Claude, "session-a").await.expect("record");
        assert!(read(&a).await.contains(Source::Claude, "session-a"));
        assert!(read(&b).await.is_empty(), "a sibling's registry is not this one's");
    }
}
