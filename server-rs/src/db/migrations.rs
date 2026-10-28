//! The schema, as an append-only list of steps.
//!
//! # The rule that matters
//!
//! **Every step is additive: new tables, new nullable columns, new indexes.**
//! Nothing is ever dropped, renamed or retyped.
//!
//! That is not tidiness, it is the only thing standing between a bad upgrade and
//! a machine that never runs remote-hand again. [`crate::supervisor`] steps down
//! after three fast failures: the new binary's replacement is `.prev`, and after
//! that the baked one, and the backoff between attempts is deliberately never
//! reset. So consider a release that migrates the database and then crashes for
//! some reason of its own. The supervisor falls back to `.prev` — a binary that
//! has never heard of the newest step — and if that one *refused to run against
//! a database it does not fully understand*, it would fail three times too, step
//! down to the baked binary, and fail there for ever.
//!
//! An older binary therefore reads a higher [`user_version`] and simply carries
//! on. It can, because additive-only means everything it knows about is still
//! there and still means what it meant. The columns it has never heard of go
//! unread; the rows it writes leave them at their defaults. This is the same
//! doctrine the JSON envelopes already carried — "only ever written, never
//! branched on" (`crate::schedule`) — pointed at a database instead of a file.
//!
//! # What must never migrate
//!
//! A schema step runs when a database is opened, so the question "who opens a
//! database?" is a security question. One caller deserves naming: `preflight`,
//! the subcommand [`crate::selfupdate`] makes an *uploaded, not-yet-trusted*
//! binary run against this server's real configuration before promoting it. If
//! that binary opened the state database it would migrate it — and a rollback
//! would then hand the old binary a schema written by a build that was never
//! accepted.
//!
//! It does not, and that is checked rather than assumed: `preflight` loads the
//! configuration, prints a version, and exits (`main.rs`). It builds no
//! `Paths`, touches no store. **Anything added to it later must keep that
//! property.**
//!
//! If a change ever genuinely cannot be additive, the answer is not to break
//! this rule. It is to write the new shape *beside* the old one, teach the new
//! binary to prefer it, and let the next release — once no `.prev` in the field
//! predates it — stop writing the old.
//!
//! [`user_version`]: https://sqlite.org/pragma.html#pragma_user_version

use rusqlite::Connection;

/// Identifies the file as remote-hand's, for anyone holding `file(1)` or a hex
/// editor. Arbitrary, fixed for ever: `0x52 0x48 0x44 0x42` — "RHDB".
pub const APPLICATION_ID: i32 = 0x5248_4442;

/// Every migration ever written, in order. The database's `user_version` is the
/// count of these that have been applied, so appending to this list is the whole
/// of shipping a schema change.
pub const SCHEMA: &[&str] = &[
    // 1 — the shape every small store folds into.
    //
    // Most of what this module replaces is a map with a handful of keys that is
    // read whole and written whole: the preferences, the archived list, the
    // titles, the seen marks. Those do not each want a bespoke table with a
    // bespoke migration; they want somewhere to put a value and get it back
    // transactionally, next to the other stores, under one `fsync`. `scope`
    // names the store, `key` names the row within it.
    //
    // Where a store has real structure — a run in flight, a scheduled task, an
    // account — it gets a table of its own in a later step, because those are
    // the ones something will eventually want to query rather than fetch.
    "CREATE TABLE IF NOT EXISTS kv (
         scope      TEXT    NOT NULL,
         key        TEXT    NOT NULL,
         value      TEXT    NOT NULL,
         updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
         PRIMARY KEY (scope, key)
     ) STRICT, WITHOUT ROWID",
    // 2 — the stores with enough structure to be asked a question.
    //
    // The column-versus-payload split here is deliberate and worth stating,
    // because the obvious reading of "migrate to SQLite" is to shred every
    // struct into columns and it would be a mistake. **A field becomes a column
    // when something queries, sorts or constrains on it; everything else stays
    // in `payload` as the JSON the store already serialises.**
    //
    // The reason is blast radius. These records are `serde` types with a decade
    // of accumulated `#[serde(default)]` and tolerant-reader behaviour that is
    // load-bearing — `RunRecord`'s fingerprint field cost a P0 the last time its
    // absence was read as a value rather than as silence. Shredding them into
    // columns would rewrite all of that by hand, in a change that cannot be
    // tested on this machine. Keeping the payload opaque means the record's
    // meaning is still decided by exactly one piece of code, the one that has
    // always decided it, and this module only supplies the transaction and the
    // key.
    //
    // It also keeps the promise the module note makes: a column nobody selects
    // is a column an older binary need never have heard of.
    "CREATE TABLE IF NOT EXISTS accounts (
         id      TEXT NOT NULL PRIMARY KEY,
         payload TEXT NOT NULL
     ) STRICT;

     -- Which account each session is bound to: 35 rows today, one more per
     -- session for ever. A row rather than a key in the accounts document,
     -- because binding a session must not rewrite every account's secrets.
     CREATE TABLE IF NOT EXISTS session_accounts (
         session_id TEXT NOT NULL PRIMARY KEY,
         account_id TEXT NOT NULL
     ) STRICT;

     -- Local login. The one store with a genuinely unguarded cross-process
     -- writer today: `remote-hand-server user add` runs while a server holds
     -- the instance lock, by design, and neither takes anything the other
     -- respects.
     CREATE TABLE IF NOT EXISTS users (
         name       TEXT    NOT NULL PRIMARY KEY,
         hash       TEXT    NOT NULL,
         created_at INTEGER NOT NULL DEFAULT (unixepoch())
     ) STRICT;

     -- Issued bearer tokens, by digest. `expires_at` is a column and not part
     -- of the payload because expiry is the one thing this table is asked in
     -- bulk: `DELETE FROM auth_sessions WHERE expires_at < ?` replaces reading
     -- every session on every request to prune the dead ones.
     CREATE TABLE IF NOT EXISTS auth_sessions (
         digest     TEXT    NOT NULL PRIMARY KEY,
         expires_at INTEGER NOT NULL,
         payload    TEXT    NOT NULL
     ) STRICT;
     CREATE INDEX IF NOT EXISTS auth_sessions_by_expiry ON auth_sessions (expires_at);

     -- Runs in flight. `session_id` is a column because adoption asks for a
     -- session's runs by name; `ordinal` preserves the file's order, which
     -- `RunsFile` made meaningful when it moved a re-added record to the end.
     CREATE TABLE IF NOT EXISTS runs (
         run_id     TEXT    NOT NULL PRIMARY KEY,
         session_id TEXT    NOT NULL,
         ordinal    INTEGER NOT NULL,
         payload    TEXT    NOT NULL
     ) STRICT;
     CREATE INDEX IF NOT EXISTS runs_by_session ON runs (session_id);
     CREATE INDEX IF NOT EXISTS runs_by_ordinal ON runs (ordinal);

     CREATE TABLE IF NOT EXISTS apps (
         id      TEXT    NOT NULL PRIMARY KEY,
         ordinal INTEGER NOT NULL,
         payload TEXT    NOT NULL
     ) STRICT;

     -- The sessions this instance made whose transcripts live somewhere it does
     -- not own. A composite key, so 'the same id under two sources' is not a
     -- thing that can be written.
     CREATE TABLE IF NOT EXISTS owned_sessions (
         source     TEXT NOT NULL,
         session_id TEXT NOT NULL,
         PRIMARY KEY (source, session_id)
     ) STRICT, WITHOUT ROWID;

     -- Pane layouts. These were machine-wide once and two instances erased each
     -- other's; the file is per-instance now and so is this table, which is the
     -- same fix twice over.
     CREATE TABLE IF NOT EXISTS screens (
         id      TEXT    NOT NULL PRIMARY KEY,
         ordinal INTEGER NOT NULL,
         payload TEXT    NOT NULL
     ) STRICT;

     CREATE TABLE IF NOT EXISTS push_subscriptions (
         endpoint TEXT NOT NULL PRIMARY KEY,
         payload  TEXT NOT NULL
     ) STRICT;

     CREATE TABLE IF NOT EXISTS scheduled_tasks (
         id      TEXT NOT NULL PRIMARY KEY,
         payload TEXT NOT NULL
     ) STRICT",
    // 3 — which server a session was made on, and who this server is.
    //
    // `server_id` is nullable with no default, which is the additive shape this
    // module insists on: the column is unread by every binary that predates it,
    // and the rows those binaries write leave it `NULL`. A `NULL` is not a gap —
    // [`crate::sessions::owned_sessions`] resolves it as *this* instance's name,
    // and says there why that is a fact rather than a guess.
    //
    // `ADD COLUMN` on a `STRICT, WITHOUT ROWID` table is legal because the
    // column is nullable and no part of the primary key; SQLite only refuses
    // one that is `NOT NULL` without a default, `UNIQUE`, or `PRIMARY KEY`.
    //
    // Both statements are harmless in the database they do not concern. The
    // ladder is applied to the config database as well as the instance one, and
    // step 2 created `owned_sessions` in both — the config copy has simply never
    // held a row, so the column lands on an empty table. `server_identity` is
    // minted into the instance database alone ([`crate::identity`]); the config
    // copy of the table stays empty for the same reason.
    "ALTER TABLE owned_sessions ADD COLUMN server_id TEXT;

     -- This server's durable name for itself. One row, minted at the first boot
     -- that finds the table empty: a uuid that outlives a rename, plus the name
     -- the instance currently answers to. Nothing reads the uuid yet — it exists
     -- so a later slice can prove which server a session came from instead of
     -- trusting a name two machines could both choose.
     CREATE TABLE IF NOT EXISTS server_identity (
         uuid       TEXT    NOT NULL PRIMARY KEY,
         name       TEXT    NOT NULL,
         created_at INTEGER NOT NULL DEFAULT (unixepoch())
     ) STRICT",
    // 4 — the limit log, and which accounts are resting.
    //
    // Phase 1 of automatic account rotation: recorded, never acted on (see
    // [`crate::accounts::rest`]). Instance tables, deliberately — the accounts
    // are machine-wide, but what one instance has seen them do is its own. Like
    // step 3's tables, the config database gets empty copies it never writes.
    //
    // `account_id` is an account uuid or `local:claude` / `local:codex`, the
    // machine's own login, which has no `accounts` row to hang a rest on. Epoch
    // milliseconds throughout, with a `NULL` `until_ms` meaning "until cleared".
    "CREATE TABLE IF NOT EXISTS account_rest (
         account_id  TEXT    NOT NULL,
         scope       TEXT    NOT NULL,
         class       TEXT    NOT NULL,
         until_ms    INTEGER,
         reason      TEXT    NOT NULL,
         session_id  TEXT,
         source      TEXT    NOT NULL,
         observed_ms INTEGER NOT NULL,
         PRIMARY KEY (account_id, scope)
     ) STRICT;

     -- Capped at 500 rows by the writer, oldest first. `raw` is what the CLI
     -- said, redacted before it gets here and cut to 500 characters.
     CREATE TABLE IF NOT EXISTS account_limit_events (
         id          INTEGER PRIMARY KEY,
         observed_ms INTEGER NOT NULL,
         account_id  TEXT,
         session_id  TEXT,
         source      TEXT    NOT NULL,
         model       TEXT,
         class       TEXT    NOT NULL,
         scope       TEXT    NOT NULL,
         until_ms    INTEGER,
         raw         TEXT    NOT NULL
     ) STRICT;
     CREATE INDEX IF NOT EXISTS account_limit_events_by_account ON account_limit_events (account_id, id)",
    // 5 — take back the rests step 4's first release wrote from a misreading.
    //
    // Not a schema change, and the one kind of data change this ladder allows:
    // a repair, idempotent, of rows only a known bug wrote. v0.2.69 read a
    // `rate_limit_event` for the window `seven_day_overage_included` as the
    // whole account's weekly limit, when that window does not say which models
    // it covers — the first real one was a Fable limit with the general weekly
    // window at 82 % and Opus still usable, and it rested the account for
    // three days. Those rests are exactly the `weekly` / `all` rows whose
    // reason names that window; the events they came from stay, because they
    // are still true. An older binary reading this database finds fewer rows
    // in a table it knows, which is a state it already handles.
    "DELETE FROM account_rest
      WHERE class = 'weekly' AND scope = 'all' AND reason LIKE '%seven_day_overage_included%'",
    // 6 — the rotation log.
    //
    // Phase 2 of account rotation: every time a session's own turn ends in a
    // limit and the driver decides where it goes — another account, or nowhere
    // (see [`crate::accounts::rotation`]). An instance table like the rests,
    // capped at twenty rows per session by the writer. The labels are copied
    // in, so a decision still names the accounts it was between after one of
    // them is renamed or deleted. `to_account` is the account the session was
    // left on, which is `from_account` again when it went nowhere.
    "CREATE TABLE IF NOT EXISTS session_rotations (
         id                INTEGER PRIMARY KEY,
         session_id        TEXT    NOT NULL,
         at_ms             INTEGER NOT NULL,
         from_account      TEXT    NOT NULL,
         from_label        TEXT    NOT NULL,
         to_account        TEXT    NOT NULL,
         to_label          TEXT    NOT NULL,
         class             TEXT    NOT NULL,
         scope             TEXT    NOT NULL,
         until_ms          INTEGER,
         interrupted_tasks INTEGER NOT NULL,
         outcome           TEXT    NOT NULL
     ) STRICT;
     CREATE INDEX IF NOT EXISTS session_rotations_by_session ON session_rotations (session_id, id)",
    // 7 — idempotent sends: one row per message a client named.
    //
    // A client mints a `clientMessageId` when a message is created and sends it
    // with every attempt, so a POST whose answer was lost on a flaky network
    // can be retried without the message running twice. The row is inserted
    // *before* the message is enqueued, under the primary key, which is what
    // makes two concurrent attempts unable to both enqueue; see
    // [`crate::inject::receipts`].
    //
    // `response_json` is `NULL` while the first attempt is still running and
    // holds exactly what it answered once it has; a retry answers the same.
    // `status` is the receipt the client is shown: `pending` (in flight),
    // `queued`, `sent` (taken into a turn) or `failed` (dropped unsent), and
    // the drivers move it by `qid`, the queue id the message travels under.
    // `kind` is `message` or `new`; a `new` row learns its `session_id` when
    // the session it started does. Pruned after seven days by the writer.
    // Instance tables, like the rests; the config database gets empty copies.
    "CREATE TABLE IF NOT EXISTS client_messages (
         client_message_id TEXT    NOT NULL PRIMARY KEY,
         kind              TEXT    NOT NULL,
         session_source    TEXT    NOT NULL,
         session_id        TEXT,
         qid               TEXT,
         status            TEXT    NOT NULL,
         response_json     TEXT,
         created_ms        INTEGER NOT NULL,
         updated_ms        INTEGER NOT NULL
     ) STRICT;
     CREATE INDEX IF NOT EXISTS client_messages_by_qid ON client_messages (qid);
     CREATE INDEX IF NOT EXISTS client_messages_by_session ON client_messages (session_source, session_id, updated_ms);
     CREATE INDEX IF NOT EXISTS client_messages_by_created ON client_messages (created_ms)",
    // 8 — a revision per screen, so two devices cannot overwrite each other.
    //
    // A layout save names the revision it was based on and is refused when the
    // row has moved on since (see [`crate::screens`]). Every row already here
    // starts at 1, which is what a client that read it before this step would
    // have been told had it been asked. `NOT NULL DEFAULT 1` is an `ADD COLUMN`
    // SQLite accepts, and it is what keeps the `.prev` binary's inserts — which
    // name only `id, ordinal, payload` — valid. The list's own revision is a
    // `kv` row under [`crate::db::scope::SCREENS`], so it needs no step.
    "ALTER TABLE screens ADD COLUMN revision INTEGER NOT NULL DEFAULT 1",
    // 9 — what set a rotation off.
    //
    // A session can now be moved *ahead* of a limit, at a quiet point, when a
    // window of its account is about to run out (see
    // [`crate::accounts::rotation::plan_ahead`]). `trigger_kind` says which —
    // `limit` or `ahead` — and is `NULL` on every row written before it, which
    // reads as `limit`, the only kind there was. `used_percent` is the near
    // window's, for an `ahead` row. Not `trigger`: that is an SQL keyword.
    "ALTER TABLE session_rotations ADD COLUMN trigger_kind TEXT;
     ALTER TABLE session_rotations ADD COLUMN used_percent REAL",
    // 10 — the questions an agent stopped to ask.
    //
    // A resident claude now offers `AskUserQuestion` and blocks its turn on the
    // answer (see [`crate::inject::questions`]). One row per question, keyed by
    // the CLI's own `request_id`, so a second device answering finds the row the
    // first already moved: an answer is a compare-and-set out of `pending`,
    // taken before anything is written to the CLI. `state` is `pending`,
    // `answered`, `dismissed`, `cancelled` or `expired`. `questions` is the tool
    // input's array verbatim, because the answer has to echo it unchanged;
    // `answer` is what the user chose and `delivery` the permission response
    // written for it, kept so a server that died between the two can write it
    // again. Settled rows are pruned after a week by the writer. An instance
    // table like the rest; the config database gets an empty copy.
    "CREATE TABLE IF NOT EXISTS session_questions (
         request_id  TEXT    NOT NULL PRIMARY KEY,
         session_id  TEXT    NOT NULL,
         tool_use_id TEXT,
         questions   TEXT    NOT NULL,
         state       TEXT    NOT NULL,
         answer      TEXT,
         delivery    TEXT,
         answered_by TEXT,
         answered_ms INTEGER,
         created_ms  INTEGER NOT NULL,
         updated_ms  INTEGER NOT NULL
     ) STRICT;
     CREATE INDEX IF NOT EXISTS session_questions_by_session ON session_questions (session_id, created_ms);
     CREATE INDEX IF NOT EXISTS session_questions_by_state ON session_questions (state)",
];

/// Bring `conn` up to the newest step, and say nothing if it is already there.
pub fn apply(conn: &mut Connection, schema: &[&str]) -> rusqlite::Result<()> {
    // Set at every open rather than at creation: a database made by a build that
    // predates this constant should acquire it rather than stay anonymous, and
    // writing the value it already has costs nothing.
    conn.pragma_update(None, "application_id", APPLICATION_ID)?;

    let at: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let newest = schema.len() as i64;

    if at > newest {
        // The downgrade case the module note exists for. Not an error, not a
        // refusal — a line in the log and then business as usual.
        tracing::warn!(
            database_at = at,
            this_binary_knows = newest,
            "the state database was made by a newer Droppen; carrying on with the part of it this build understands"
        );
        return Ok(());
    }
    if at == newest {
        return Ok(());
    }

    // One transaction for the whole ladder: a migration interrupted half way
    // through is a database at a version that describes neither shape, and
    // recovering from that by hand is exactly the situation this replaces.
    let tx = conn.transaction()?;
    for (index, step) in schema.iter().enumerate().skip(at as usize) {
        tracing::info!(step = index + 1, "applying a schema step");
        tx.execute_batch(step)?;
    }
    // `user_version` takes no parameter binding — it is a pragma, not a
    // statement — so the value is formatted in. It is an integer from a `len()`,
    // which is the only reason that is safe.
    tx.pragma_update(None, "user_version", newest)?;
    tx.commit()
}

impl super::Db {
    /// Apply everything outstanding, or carry on having failed to.
    ///
    /// A migration that cannot be applied leaves a database at the version it
    /// was, which the stores above then find missing the table they wanted. That
    /// is survivable in exactly the way a missing JSON file was — the store
    /// reads as empty — and it is far better than a server that will not start.
    pub(super) fn migrate(&self) {
        let mut conn = self.lock();
        let schema = self.schema;
        if let Err(error) = apply(&mut conn, schema) {
            tracing::error!(path = %self.path.display(), %error, "cannot bring the state database's schema up to date");
        }
    }

    /// The schema step this database has reached. Tests, and the log line that
    /// reports what a boot found.
    pub fn user_version(&self) -> i64 {
        self.read(|conn| conn.query_row("PRAGMA user_version", [], |row| row.get(0)))
            .unwrap_or(-1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    /// A database exactly as the release before this one left it: the ladder up
    /// to `at`, and `user_version` to match. `SCHEMA` is append-only, so its
    /// first `at` entries *are* what that binary shipped.
    fn database_at(path: &std::path::Path, at: usize) -> Connection {
        let conn = Connection::open(path).expect("open");
        for step in &SCHEMA[..at] {
            conn.execute_batch(step).expect("a step this binary already shipped");
        }
        conn.pragma_update(None, "user_version", at as i64).expect("version");
        conn
    }

    fn version(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA user_version", [], |row| row.get(0)).expect("version")
    }

    /// The upgrade this slice ships, on a database with rows already in it.
    ///
    /// `owned_sessions` is `STRICT, WITHOUT ROWID`, which is the declaration
    /// worth naming: SQLite refuses `ADD COLUMN` for a column that is `NOT NULL`
    /// without a default, `UNIQUE` or part of the primary key, and `server_id`
    /// is none of those — so the rows already there keep their bytes and gain a
    /// `NULL`.
    #[test]
    fn a_database_at_step_two_gains_the_column_and_keeps_its_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.db");
        {
            let conn = database_at(&path, 2);
            conn.execute("INSERT INTO owned_sessions (source, session_id) VALUES ('claude', 'before')", [])
                .expect("a row the previous release wrote");
            assert_eq!(version(&conn), 2);
        }

        let db = Db::open(&path);
        assert_eq!(db.user_version(), SCHEMA.len() as i64, "the ladder ran to the top");

        let (id, server): (String, Option<String>) = db
            .read(|conn| {
                conn.query_row("SELECT session_id, server_id FROM owned_sessions", [], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
            })
            .expect("the row is still there");
        assert_eq!(id, "before", "the migration moved nothing");
        assert_eq!(server, None, "and left the new column empty rather than guessing");

        // The identity table arrives empty; `crate::identity` is what fills it.
        let identities: i64 = db
            .read(|conn| conn.query_row("SELECT count(*) FROM server_identity", [], |row| row.get(0)))
            .expect("count");
        assert_eq!(identities, 0);
    }

    /// Step 4 lands the limit log and the rests on a database the previous
    /// release left at step 3, and touches nothing it already held.
    #[test]
    fn a_database_at_step_three_gains_the_limit_tables() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.db");
        {
            let conn = database_at(&path, 3);
            conn.execute("INSERT INTO kv (scope, key, value) VALUES ('t', 'k', 'v')", [])
                .expect("a row the previous release wrote");
        }
        let db = Db::open(&path);
        assert_eq!(db.user_version(), SCHEMA.len() as i64);
        let (rests, events, kept): (i64, i64, i64) = db
            .read(|conn| {
                Ok((
                    conn.query_row("SELECT count(*) FROM account_rest", [], |row| row.get(0))?,
                    conn.query_row("SELECT count(*) FROM account_limit_events", [], |row| row.get(0))?,
                    conn.query_row("SELECT count(*) FROM kv", [], |row| row.get(0))?,
                ))
            })
            .expect("both tables exist");
        assert_eq!((rests, events, kept), (0, 0, 1));
    }

    /// Step 5 removes the rests v0.2.69 wrote from a `seven_day_overage_included`
    /// event as if it covered the whole account, and nothing else.
    #[test]
    fn a_database_at_step_four_loses_only_the_misread_weekly_rests() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.db");
        {
            let conn = database_at(&path, 4);
            for (account, scope, class, reason) in [
                ("a", "all", "weekly", "rate_limit_event: seven_day_overage_included window rejected"),
                ("b", "all", "weekly", "rate_limit_event: seven_day window rejected"),
                ("a", "fable", "weekly", "You've reached your Fable limit."),
                ("c", "all", "session", "rate_limit_event: seven_day_overage_included window rejected"),
            ] {
                conn.execute(
                    "INSERT INTO account_rest (account_id, scope, class, until_ms, reason, source, observed_ms)
                     VALUES (?1, ?2, ?3, 1, ?4, 'claude:rate_limit_event', 1)",
                    (account, scope, class, reason),
                )
                .expect("a rest v0.2.69 wrote");
            }
        }
        let db = Db::open(&path);
        let left: Vec<(String, String, String)> = db
            .read(|conn| {
                let mut stmt = conn.prepare("SELECT account_id, scope, class FROM account_rest ORDER BY account_id, scope")?;
                let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
                rows.collect()
            })
            .expect("read");
        let left: Vec<(&str, &str, &str)> = left.iter().map(|(a, s, c)| (a.as_str(), s.as_str(), c.as_str())).collect();
        assert_eq!(
            left,
            vec![("a", "fable", "weekly"), ("b", "all", "weekly"), ("c", "all", "session")],
            "only the weekly/all rest that names the overage window goes"
        );
    }

    /// Step 6 adds the rotation log to a database the previous release left at
    /// step 5, and leaves the rests it already held alone.
    #[test]
    fn a_database_at_step_five_gains_the_rotation_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.db");
        {
            let conn = database_at(&path, 5);
            conn.execute(
                "INSERT INTO account_rest (account_id, scope, class, until_ms, reason, source, observed_ms)
                 VALUES ('a', 'all', 'session', 1, 'You''ve hit your session limit', 'claude:result', 1)",
                [],
            )
            .expect("a rest the previous release wrote");
        }
        let db = Db::open(&path);
        assert_eq!(db.user_version(), SCHEMA.len() as i64);
        let (rotations, rests): (i64, i64) = db
            .read(|conn| {
                Ok((
                    conn.query_row("SELECT count(*) FROM session_rotations", [], |row| row.get(0))?,
                    conn.query_row("SELECT count(*) FROM account_rest", [], |row| row.get(0))?,
                ))
            })
            .expect("the new table exists");
        assert_eq!((rotations, rests), (0, 1));
    }

    /// Step 7 adds the idempotent-send table to a database the previous
    /// release left at step 6, and leaves the rotation log alone.
    #[test]
    fn a_database_at_step_six_gains_the_client_message_table() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.db");
        {
            let conn = database_at(&path, 6);
            conn.execute(
                "INSERT INTO session_rotations
                     (session_id, at_ms, from_account, from_label, to_account, to_label,
                      class, scope, until_ms, interrupted_tasks, outcome)
                 VALUES ('s', 1, 'a', 'A', 'b', 'B', 'session', 'all', NULL, 0, 'switched')",
                [],
            )
            .expect("a rotation the previous release wrote");
        }
        let db = Db::open(&path);
        assert_eq!(db.user_version(), SCHEMA.len() as i64);
        let (messages, rotations): (i64, i64) = db
            .read(|conn| {
                Ok((
                    conn.query_row("SELECT count(*) FROM client_messages", [], |row| row.get(0))?,
                    conn.query_row("SELECT count(*) FROM session_rotations", [], |row| row.get(0))?,
                ))
            })
            .expect("the new table exists");
        assert_eq!((messages, rotations), (0, 1));
    }

    /// Step 8 gives every screen a revision, starting the rows the previous
    /// release wrote at 1, and an insert that does not name the column — the
    /// one the `.prev` binary makes — still lands, at 1.
    #[test]
    fn a_database_at_step_seven_gains_screen_revisions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.db");
        {
            let conn = database_at(&path, 7);
            conn.execute("INSERT INTO screens (id, ordinal, payload) VALUES ('old', 0, '{}')", [])
                .expect("a screen the previous release wrote");
        }
        let db = Db::open(&path);
        assert_eq!(db.user_version(), SCHEMA.len() as i64);
        db.commit("a row from an older binary", |tx| {
            tx.execute("INSERT INTO screens (id, ordinal, payload) VALUES ('older', 1, '{}')", [])?;
            Ok(())
        });
        let revisions: Vec<(String, i64)> = db
            .read(|conn| {
                let mut stmt = conn.prepare("SELECT id, revision FROM screens ORDER BY ordinal")?;
                let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
                rows.collect()
            })
            .expect("the column exists");
        assert_eq!(revisions, vec![("old".to_string(), 1), ("older".to_string(), 1)]);
    }

    /// Step 9 gives the rotation log its trigger, and a row the previous
    /// release wrote reads back as the limit it was.
    #[test]
    fn a_database_at_step_eight_gains_the_rotation_trigger_and_old_rows_read_as_limits() {
        use crate::accounts::rotation::RotationLog;
        use crate::sessions::account::RotationTrigger;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.db");
        {
            let conn = database_at(&path, 8);
            conn.execute(
                "INSERT INTO session_rotations
                     (session_id, at_ms, from_account, from_label, to_account, to_label,
                      class, scope, until_ms, interrupted_tasks, outcome)
                 VALUES ('s', 1, 'a', 'A', 'b', 'B', 'session', 'all', NULL, 0, 'switched')",
                [],
            )
            .expect("a rotation the previous release wrote");
        }
        let db = std::sync::Arc::new(Db::open(&path));
        assert_eq!(db.user_version(), SCHEMA.len() as i64);
        let latest = RotationLog::new(db).latest("s").expect("the old row reads back");
        assert_eq!(latest.trigger, RotationTrigger::Limit);
        assert_eq!(latest.used_percent, None);
        assert_eq!(latest.to.label, "B");
    }

    /// Step 10 adds the question table to a database the previous release left
    /// at step 9, and leaves the rotation log alone.
    #[test]
    fn a_database_at_step_nine_gains_the_question_table() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.db");
        {
            let conn = database_at(&path, 9);
            conn.execute(
                "INSERT INTO session_rotations
                     (session_id, at_ms, from_account, from_label, to_account, to_label,
                      class, scope, until_ms, interrupted_tasks, outcome, trigger_kind)
                 VALUES ('s', 1, 'a', 'A', 'b', 'B', 'session', 'all', NULL, 0, 'switched', 'ahead')",
                [],
            )
            .expect("a rotation the previous release wrote");
        }
        let db = Db::open(&path);
        assert_eq!(db.user_version(), SCHEMA.len() as i64);
        let (questions, rotations): (i64, i64) = db
            .read(|conn| {
                Ok((
                    conn.query_row("SELECT count(*) FROM session_questions", [], |row| row.get(0))?,
                    conn.query_row("SELECT count(*) FROM session_rotations", [], |row| row.get(0))?,
                ))
            })
            .expect("the new table exists");
        assert_eq!((questions, rotations), (0, 1));
    }

    /// The release gate: the binary this one replaces must still work against
    /// the database this one migrated.
    ///
    /// [`crate::supervisor`] demotes to `.prev` after three fast failures, so a
    /// migration that made the old binary refuse to run would take the whole
    /// machine down rather than one release. The old binary is modelled by its
    /// own schema — the first two steps — which is exactly what it holds.
    #[test]
    fn the_previous_release_still_reads_a_database_this_one_migrated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.db");
        Db::open(&path).commit("a row from the new binary", |tx| {
            tx.execute(
                "INSERT INTO owned_sessions (source, session_id, server_id) VALUES ('claude', 'after', 'alice')",
                [],
            )?;
            Ok(())
        });

        let mut old = Connection::open(&path).expect("open");
        let previous = &SCHEMA[..2];
        apply(&mut old, previous).expect("the older binary opens it rather than refusing");
        assert_eq!(version(&old), SCHEMA.len() as i64, "and leaves the version where it found it");

        // Its own query, spelled as it spells it: no `server_id`, so the column
        // it has never heard of goes unread.
        let ids: Vec<String> = old
            .prepare("SELECT session_id FROM owned_sessions WHERE source = 'claude'")
            .and_then(|mut stmt| stmt.query_map([], |row| row.get(0)).map(|rows| rows.filter_map(Result::ok).collect()))
            .expect("read");
        assert_eq!(ids, vec!["after".to_string()], "the row this binary wrote reads back whole");

        // And it can still write one, leaving the column it does not know about
        // to its default.
        old.execute(
            "INSERT INTO owned_sessions (source, session_id) VALUES ('codex', 'from-the-old-binary')",
            [],
        )
        .expect("the old binary's insert names the columns it knows");
        let null: Option<String> = old
            .query_row(
                "SELECT server_id FROM owned_sessions WHERE session_id = 'from-the-old-binary'",
                [],
                |row| row.get(0),
            )
            .expect("read");
        assert_eq!(null, None);
    }
}
