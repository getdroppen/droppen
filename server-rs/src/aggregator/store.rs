//! `<instance>/aggregator/copies.db`: the other devices' copies this computer
//! follows (local-aggregator §5.5, S1-5), decrypted and folded.
//!
//! One row per stream followed: a host's index, its status, and at most
//! [`super::access::SESSIONS_PER_HOST`] of its sessions. `state` is the
//! folded **plaintext** (the index rows, the whole status, a session's
//! entries); `head` the machine-signed head it was released up to — the
//! cursor the next run carries on from. It is a cache: everything in it
//! can be read again from the hub with the keys in `keys.json`, which lies
//! next to it with the same permissions (so encrypting it again would keep
//! nobody out who could not read the keys anyway).
//!
//! - **Private**: the directory 0700; the file is made 0600 before SQLite
//!   opens it, so its `-wal` and `-shm` (which SQLite creates with the
//!   database's own mode) are 0600 too. A file others can read is deleted
//!   and made again — it is a cache. `synchronous = NORMAL`.
//! - **Bound** to the account and this device (`meta`): rows another
//!   enrollment left are deleted on open.
//! - **A deleted file stays deleted**, as `keys.json` (security review of
//!   S1-4, S1): every write happens under the directory's lock
//!   ([`super::with_lock`], which [`super::forget_all`] takes too), only while
//!   the holder's guard says it may (the aggregator on, the link not
//!   stopped), and never once the file is gone ([`OFF`]): the holder then
//!   drops the store and what it held in memory.
//! - Nothing here logs content or keys.

use std::path::{Path, PathBuf};

use rh_proto::record::{SignedHead, StreamId};
use rusqlite::{params, Connection, OptionalExtension};

use super::keys::Guard;
use crate::sessions::Paths;

/// The error of a write that may not happen (as [`super::keys::OFF`]).
pub const OFF: &str = super::keys::OFF;

/// `<instance>/aggregator/copies.db`.
pub fn path(paths: &Paths) -> PathBuf {
    super::dir(paths).join("copies.db")
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS copies (
    host TEXT NOT NULL,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    stream BLOB NOT NULL,
    epoch INTEGER NOT NULL,
    head BLOB,
    state TEXT,
    too_large INTEGER NOT NULL DEFAULT 0,
    updated_ms INTEGER NOT NULL,
    head_key BLOB,
    PRIMARY KEY (host, kind, name)
);
";

/// `head_key` for a file made before it was kept (a cache: added in place).
const ADD_HEAD_KEY: &str = "ALTER TABLE copies ADD COLUMN head_key BLOB";

/// What a row copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// The host's session list.
    Index,
    /// The host's status.
    Status,
    /// One of its sessions.
    Session,
}

impl Kind {
    /// As stored.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Index => "index",
            Kind::Status => "status",
            Kind::Session => "session",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "index" => Some(Kind::Index),
            "status" => Some(Kind::Status),
            "session" => Some(Kind::Session),
            _ => None,
        }
    }
}

/// One copy as stored.
#[derive(Clone)]
pub struct Row {
    /// The host it is of.
    pub host: String,
    /// What it copies.
    pub kind: Kind,
    /// `source/id` for a session; empty otherwise.
    pub name: String,
    /// The stream it was read from.
    pub stream: StreamId,
    /// The machine-key epoch that stream is named under.
    pub epoch: u32,
    /// The machine-signed head it was released up to (the cursor).
    pub head: Option<SignedHead>,
    /// The host's key that head was verified under when it was stored:
    /// exactly that key verifies it when the copy carries on, even after the
    /// host renewed its identity key (`docs/host-identity-rotation.md`
    /// §5.4). `None` for a row from before it was kept: then only the host's
    /// key now does.
    pub head_key: Option<[u8; 32]>,
    /// The folded state (JSON); `None` when too large to keep.
    pub state: Option<String>,
    /// A session over [`super::copies::MAX_SESSION_BYTES`]: not kept.
    pub too_large: bool,
    /// When it was written (ms).
    pub updated_ms: u64,
}

impl std::fmt::Debug for Row {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Row")
            .field("host", &self.host)
            .field("kind", &self.kind)
            .field("epoch", &self.epoch)
            .field("state_bytes", &self.state.as_ref().map(String::len))
            .finish_non_exhaustive()
    }
}

/// The copies database.
pub struct CopyStore {
    path: PathBuf,
    conn: Connection,
    guard: Guard,
}

#[cfg(unix)]
fn private_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mode = std::fs::metadata(dir).map_err(|e| e.to_string())?.permissions().mode();
    if mode & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn private_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))
}

/// The database and its sidecars.
fn files(path: &Path) -> [PathBuf; 3] {
    ["", "-wal", "-shm"].map(|suffix| PathBuf::from(format!("{}{suffix}", path.display())))
}

fn remove_files(path: &Path) {
    for file in files(path) {
        let _ = std::fs::remove_file(file);
    }
}

/// Make the file 0600 before SQLite opens it (its sidecars take its mode);
/// one others can read is a cache made again.
fn private_file(path: &Path) -> Result<(), String> {
    if path.exists() {
        let open_to_others = files(path).iter().filter(|f| f.exists()).any(|f| crate::agent::check_private(f).is_err());
        if !open_to_others {
            return Ok(());
        }
        tracing::warn!(path = %path.display(), "the other devices' copies were readable by others: starting them again");
        remove_files(path);
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path).map(|_| ()).map_err(|e| format!("{}: {e}", path.display()))
}

/// What the write-ahead log is cut back to after a checkpoint.
pub const JOURNAL_SIZE_LIMIT: i64 = 1024 * 1024;

fn open_conn(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    // A cache: losing the last commits to a power cut costs a read from the hub.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    // What is deleted is overwritten, not left in free pages.
    conn.pragma_update(None, "secure_delete", "ON")?;
    // Closing never writes the log back into the database: a connection
    // still open on a directory being deleted ([`super::forget_all`]) must
    // not put plaintext back into a file being zeroed.
    conn.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)?;
    // The log never keeps more than this once checkpointed (its stale frames
    // with it); and it is truncated whenever the store is let go (`Drop`).
    let _: i64 = conn.query_row(&format!("PRAGMA journal_size_limit = {JOURNAL_SIZE_LIMIT}"), [], |row| row.get(0))?;
    conn.pragma_update(None, "busy_timeout", 5_000)?;
    conn.execute_batch(SCHEMA)?;
    let has_head_key = conn
        .prepare("SELECT head_key FROM copies LIMIT 0")
        .is_ok();
    if !has_head_key {
        conn.execute_batch(ADD_HEAD_KEY)?;
    }
    Ok(conn)
}

fn sql(e: rusqlite::Error) -> String {
    e.to_string()
}

/// Checkpoint the write-ahead log into the database and truncate it to
/// nothing (what it held of deleted rows goes with it). A checkpoint another
/// connection holds back is tried once more, then said.
fn truncate_wal(conn: &Connection) {
    for attempt in 0..2 {
        match conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get::<_, i64>(0)) {
            Ok(0) => return,
            Ok(_) if attempt == 0 => std::thread::sleep(std::time::Duration::from_millis(50)),
            Ok(_) => tracing::warn!("the copies' write-ahead log is busy: not truncated"),
            Err(error) => {
                tracing::warn!(%error, "cannot truncate the copies' write-ahead log");
                return;
            }
        }
    }
}

impl CopyStore {
    /// Open (or make) the copies of `account` held by `device`, writing only
    /// while `guard` says so. Rows another enrollment left are deleted. A
    /// database that will not open is deleted and made again (a cache).
    pub fn open(paths: &Paths, account: &str, device: &str, guard: Guard) -> Result<Self, String> {
        let path = path(paths);
        super::sweep_deleted(paths);
        let conn = super::with_lock(|| -> Result<Connection, String> {
            if !guard() {
                return Err(OFF.to_string());
            }
            private_dir(&super::dir(paths))?;
            super::exclude_from_backups(&super::dir(paths));
            private_file(&path)?;
            match open_conn(&path) {
                Ok(conn) => Ok(conn),
                Err(error) => {
                    tracing::warn!(%error, "the other devices' copies do not open: starting them again");
                    remove_files(&path);
                    private_file(&path)?;
                    open_conn(&path).map_err(sql)
                }
            }
        })?;
        let mut store = Self { path, conn, guard };
        let bound = |key: &str| -> Result<Option<String>, String> {
            store
                .conn
                .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
                .optional()
                .map_err(sql)
        };
        if bound("account")?.as_deref() != Some(account) || bound("device")?.as_deref() != Some(device) {
            store.write(|tx| {
                tx.execute("DELETE FROM copies", [])?;
                tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('account', ?1)", [account])?;
                tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('device', ?1)", [device])?;
                Ok(())
            })?;
        }
        Ok(store)
    }

    /// One change, in a transaction — under the directory's lock, only while
    /// the guard says so and the file is still there ([`OFF`]).
    fn write<T>(&mut self, work: impl FnOnce(&rusqlite::Transaction<'_>) -> rusqlite::Result<T>) -> Result<T, String> {
        let (path, guard, conn) = (&self.path, &self.guard, &mut self.conn);
        super::with_lock(|| {
            if !guard() || !path.exists() {
                return Err(OFF.to_string());
            }
            let tx = conn.transaction().map_err(sql)?;
            let out = work(&tx).map_err(sql)?;
            tx.commit().map_err(sql)?;
            Ok(out)
        })
    }

    /// Whether writes may still happen (the file is there and the guard says yes).
    pub fn usable(&self) -> bool {
        super::with_lock(|| (self.guard)() && self.path.exists())
    }

    /// Keep `row` (replacing the one of the same host, kind and name).
    pub fn put(&mut self, row: &Row) -> Result<(), String> {
        self.write(|tx| {
            tx.execute(
                "INSERT OR REPLACE INTO copies (host, kind, name, stream, epoch, head, state, too_large, updated_ms, head_key)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    row.host,
                    row.kind.as_str(),
                    row.name,
                    row.stream.0.to_vec(),
                    row.epoch,
                    row.head.as_ref().map(SignedHead::encode),
                    row.state,
                    row.too_large,
                    row.updated_ms as i64,
                    row.head_key.map(|k| k.to_vec()),
                ],
            )?;
            Ok(())
        })
    }

    /// Move the head of an unchanged copy (a heartbeat: the same place in
    /// the stream, signed later), if the row still copies `stream` and its
    /// head stands at that very place — a head further on would put the
    /// cursor past records the stored state lacks (a write of them that
    /// failed), and the next run would skip them. `true` when moved.
    pub fn touch(
        &mut self,
        host: &str,
        kind: Kind,
        name: &str,
        stream: &StreamId,
        head: &SignedHead,
        head_key: &[u8; 32],
        updated_ms: u64,
    ) -> Result<bool, String> {
        self.write(|tx| {
            let held: Option<Option<Vec<u8>>> = tx
                .query_row(
                    "SELECT head FROM copies WHERE host = ?1 AND kind = ?2 AND name = ?3 AND stream = ?4 AND too_large = 0",
                    params![host, kind.as_str(), name, stream.0.to_vec()],
                    |r| r.get(0),
                )
                .optional()?;
            let same_place = held
                .flatten()
                .and_then(|h| SignedHead::decode(&h).ok())
                .is_some_and(|h| h.head.next_seq == head.head.next_seq);
            if !same_place {
                return Ok(false);
            }
            tx.execute(
                "UPDATE copies SET head = ?1, updated_ms = ?2, head_key = ?7 WHERE host = ?3 AND kind = ?4 AND name = ?5 AND stream = ?6",
                params![head.encode(), updated_ms as i64, host, kind.as_str(), name, stream.0.to_vec(), head_key.to_vec()],
            )?;
            Ok(true)
        })
    }

    /// Delete one row.
    pub fn delete(&mut self, host: &str, kind: Kind, name: &str) -> Result<bool, String> {
        self.write(|tx| {
            tx.execute(
                "DELETE FROM copies WHERE host = ?1 AND kind = ?2 AND name = ?3",
                params![host, kind.as_str(), name],
            )
            .map(|n| n > 0)
        })
    }

    /// Delete every row of a host `keep` says no to: the hosts deleted. Its
    /// plaintext goes from the file too: overwritten where it lay
    /// (`secure_delete`), and the write-ahead log, which still held it, is
    /// checkpointed and truncated to nothing.
    pub fn retain_hosts(&mut self, keep: impl Fn(&str) -> bool) -> Result<Vec<String>, String> {
        let gone: Vec<String> = self.hosts()?.into_iter().filter(|h| !keep(h)).collect();
        if gone.is_empty() {
            return Ok(gone);
        }
        self.write(|tx| {
            for host in &gone {
                tx.execute("DELETE FROM copies WHERE host = ?1", [host])?;
            }
            Ok(())
        })?;
        let (path, guard, conn) = (&self.path, &self.guard, &self.conn);
        super::with_lock(|| {
            if guard() && path.exists() {
                truncate_wal(conn);
            }
        });
        Ok(gone)
    }

    /// The hosts with any row.
    pub fn hosts(&self) -> Result<Vec<String>, String> {
        let mut stmt = self.conn.prepare("SELECT DISTINCT host FROM copies ORDER BY host").map_err(sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(sql)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(sql)
    }

    /// Every row.
    pub fn rows(&self) -> Result<Vec<Row>, String> {
        self.select(
            "SELECT host, kind, name, stream, epoch, head, state, too_large, updated_ms, head_key FROM copies ORDER BY host, kind, name",
            &[],
        )
    }

    /// One row.
    pub fn row(&self, host: &str, kind: Kind, name: &str) -> Result<Option<Row>, String> {
        Ok(self
            .select(
                "SELECT host, kind, name, stream, epoch, head, state, too_large, updated_ms, head_key FROM copies WHERE host = ?1 AND kind = ?2 AND name = ?3",
                &[&host, &kind.as_str(), &name],
            )?
            .pop())
    }

    fn select(&self, query: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<Row>, String> {
        let mut stmt = self.conn.prepare(query).map_err(sql)?;
        let rows = stmt
            .query_map(args, |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Vec<u8>>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, Option<Vec<u8>>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, bool>(7)?,
                    r.get::<_, i64>(8)?,
                    r.get::<_, Option<Vec<u8>>>(9)?,
                ))
            })
            .map_err(sql)?;
        let mut out = Vec::new();
        for row in rows {
            let (host, kind, name, stream, epoch, head, state, too_large, updated_ms, head_key) = row.map_err(sql)?;
            // A row this release cannot read is skipped (it is read again from the hub).
            let (Some(kind), Ok(stream), Ok(epoch)) = (Kind::parse(&kind), <[u8; 32]>::try_from(stream), u32::try_from(epoch)) else {
                continue;
            };
            out.push(Row {
                host,
                kind,
                name,
                stream: StreamId(stream),
                epoch,
                head: head.and_then(|h| SignedHead::decode(&h).ok()),
                head_key: head_key.and_then(|k| <[u8; 32]>::try_from(k).ok()),
                state,
                too_large,
                updated_ms: u64::try_from(updated_ms).unwrap_or(0),
            });
        }
        Ok(out)
    }
}

/// Let go of the store: its write-ahead log is checkpointed and truncated
/// (closing never does it: `NO_CKPT_ON_CLOSE`), unless writing is refused or
/// the file is gone — then nothing more is written into it.
impl Drop for CopyStore {
    fn drop(&mut self) {
        let (path, guard, conn) = (&self.path, &self.guard, &self.conn);
        super::with_lock(|| {
            if guard() && path.exists() {
                truncate_wal(conn);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn paths() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "copiestest");
        (dir, paths)
    }

    fn open(paths: &Paths, account: &str, device: &str) -> CopyStore {
        CopyStore::open(paths, account, device, Box::new(|| true)).unwrap()
    }

    fn row(host: &str, kind: Kind, name: &str, state: &str) -> Row {
        let machine = ed25519_dalek::SigningKey::from_bytes(&[4; 32]);
        let stream = rh_proto::record::stream_id(&[1; 32], "claude", name).unwrap();
        let head = rh_proto::record::StreamHead {
            stream,
            next_seq: 3,
            epoch: 0,
            link: [5; 32],
            time_ms: 77,
        }
        .sign(&machine);
        Row {
            host: host.into(),
            kind,
            name: name.into(),
            stream,
            epoch: 0,
            head: Some(head),
            head_key: Some(machine.verifying_key().to_bytes()),
            state: Some(state.into()),
            too_large: false,
            updated_ms: 9,
        }
    }

    #[test]
    fn copies_are_private_bound_and_kept_by_host_kind_and_name() {
        let (_dir, paths) = paths();
        let mut store = open(&paths, "a-1", "m-me");
        store.put(&row("m-2", Kind::Index, "", r#"{"rows":{}}"#)).unwrap();
        store.put(&row("m-2", Kind::Session, "claude/s1", r#"{"items":[1]}"#)).unwrap();
        store.put(&row("m-2", Kind::Session, "claude/s1", r#"{"items":[1,2]}"#)).unwrap();
        store.put(&row("m-3", Kind::Status, "", r#"{"asOf":1}"#)).unwrap();
        let rows = store.rows().unwrap();
        assert_eq!(rows.len(), 3);
        let s1 = rows.iter().find(|r| r.kind == Kind::Session).unwrap();
        assert_eq!(s1.state.as_deref(), Some(r#"{"items":[1,2]}"#));
        assert_eq!(s1.head.as_ref().unwrap().head.next_seq, 3, "the cursor comes back");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&super::super::dir(&paths)), 0o700);
            for file in files(&path(&paths)) {
                if file.exists() {
                    assert_eq!(mode(&file), 0o600, "{}", file.display());
                }
            }
            assert!(files(&path(&paths))[1].exists(), "WAL is on, and private");
        }
        drop(store);
        // Reopened as the same enrollment: kept. As another: started again.
        assert_eq!(open(&paths, "a-1", "m-me").rows().unwrap().len(), 3);
        assert!(open(&paths, "a-1", "m-other").rows().unwrap().is_empty());
        let mut again = open(&paths, "a-1", "m-me");
        assert!(again.rows().unwrap().is_empty(), "bound to the last enrollment that opened it");
        again.put(&row("m-2", Kind::Index, "", "{}")).unwrap();
        // A file others can read is a cache made again.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            drop(again);
            std::fs::set_permissions(path(&paths), std::fs::Permissions::from_mode(0o644)).unwrap();
            let fresh = open(&paths, "a-1", "m-me");
            assert!(fresh.rows().unwrap().is_empty());
            assert_eq!(std::fs::metadata(path(&paths)).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn a_host_that_no_longer_admits_this_device_loses_its_rows() {
        let (_dir, paths) = paths();
        let mut store = open(&paths, "a-1", "m-me");
        store.put(&row("m-2", Kind::Index, "", "{}")).unwrap();
        store
            .put(&row("m-2", Kind::Session, "claude/a", r#"{"items":["M2-PLAINTEXT-CANARY"]}"#))
            .unwrap();
        store.put(&row("m-3", Kind::Index, "", "{}")).unwrap();
        assert_eq!(store.retain_hosts(|h| h == "m-3").unwrap(), vec!["m-2".to_string()]);
        // Its plaintext is gone from the files, not only from the table.
        let on_disk: Vec<u8> = files(&path(&paths)).iter().filter_map(|f| std::fs::read(f).ok()).flatten().collect();
        assert!(
            !on_disk.windows(19).any(|w| w == b"M2-PLAINTEXT-CANARY"),
            "a purged host's copy lingers in the database or its log"
        );
        assert_eq!(store.hosts().unwrap(), vec!["m-3".to_string()]);
        assert!(store.delete("m-3", Kind::Index, "").unwrap());
        assert!(!store.delete("m-3", Kind::Index, "").unwrap());
    }

    /// As keys.json (security review of S1-4, S1): off, stopped or deleted,
    /// nothing is written back.
    #[test]
    fn a_deleted_store_is_never_written_back() {
        let (_dir, paths) = paths();
        let on = Arc::new(AtomicBool::new(true));
        let gate = on.clone();
        let mut store = CopyStore::open(&paths, "a-1", "m-me", Box::new(move || gate.load(Ordering::SeqCst))).unwrap();
        store.put(&row("m-2", Kind::Index, "", "{}")).unwrap();
        on.store(false, Ordering::SeqCst);
        assert_eq!(store.put(&row("m-2", Kind::Status, "", "{}")).unwrap_err(), OFF);
        assert!(!store.usable());
        on.store(true, Ordering::SeqCst);
        super::super::forget_all(&paths).unwrap();
        assert_eq!(store.put(&row("m-2", Kind::Status, "", "{}")).unwrap_err(), OFF);
        assert_eq!(store.retain_hosts(|_| false).unwrap_or_default(), Vec::<String>::new());
        assert!(!super::super::dir(&paths).exists(), "the directory stays deleted");
        // Off: not even opened.
        on.store(false, Ordering::SeqCst);
        let gate = on.clone();
        assert!(CopyStore::open(&paths, "a-1", "m-me", Box::new(move || gate.load(Ordering::SeqCst))).is_err());
        assert!(!super::super::dir(&paths).exists());
    }

    /// Re-review of S1-5, M2: the log is kept small, and empty once the
    /// store is let go.
    #[test]
    fn the_log_is_cut_back_and_emptied_when_the_store_is_let_go() {
        let (_dir, paths) = paths();
        let mut store = open(&paths, "a-1", "m-me");
        let limit: i64 = store.conn.query_row("PRAGMA journal_size_limit", [], |r| r.get(0)).unwrap();
        assert_eq!(limit, JOURNAL_SIZE_LIMIT);
        for i in 0..20 {
            store
                .put(&row("m-2", Kind::Session, &format!("claude/{i}"), &"x".repeat(50_000)))
                .unwrap();
        }
        let wal = files(&path(&paths))[1].clone();
        assert!(std::fs::metadata(&wal).unwrap().len() > 0);
        drop(store);
        assert_eq!(std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0), 0, "the log is empty");
        assert_eq!(open(&paths, "a-1", "m-me").rows().unwrap().len(), 20, "and nothing was lost");
    }

    /// Re-review of S1-5, M3: a heartbeat moves the cursor only where the
    /// stored state stands — never past records it lacks.
    #[test]
    fn a_heartbeat_never_moves_the_cursor_past_the_state() {
        let (_dir, paths) = paths();
        let mut store = open(&paths, "a-1", "m-me");
        let at10 = row("m-2", Kind::Status, "", r#"{"asOf":10}"#);
        store.put(&at10).unwrap();
        let machine = ed25519_dalek::SigningKey::from_bytes(&[4; 32]);
        let head = |next_seq, time_ms| {
            rh_proto::record::StreamHead {
                stream: at10.stream,
                next_seq,
                epoch: 0,
                link: [5; 32],
                time_ms,
            }
            .sign(&machine)
        };
        let stored_head = |store: &CopyStore| store.row("m-2", Kind::Status, "").unwrap().unwrap().head.unwrap().head;
        // The row's head is at 3 (`row`): a heartbeat at 12 (a state at 12 whose write failed) moves nothing.
        assert!(!store.touch("m-2", Kind::Status, "", &at10.stream, &head(12, 9_000), &[4; 32], 5).unwrap());
        assert_eq!(stored_head(&store).next_seq, 3);
        // At the same place, signed later: moved.
        assert!(store.touch("m-2", Kind::Status, "", &at10.stream, &head(3, 9_000), &[4; 32], 5).unwrap());
        assert_eq!((stored_head(&store).next_seq, stored_head(&store).time_ms), (3, 9_000));
        // Another stream's heartbeat: nothing.
        let other = rh_proto::stream::index_stream(&[9; 32]);
        assert!(!store.touch("m-2", Kind::Status, "", &other, &head(3, 10_000), &[4; 32], 5).unwrap());
    }

    /// Host identity renewal (§5.4, R6 review): the host key a stored head
    /// was verified under is kept with it — moved by a heartbeat too — and a
    /// file from before the column takes it in place, its rows saying none.
    #[test]
    fn the_key_a_head_was_verified_under_is_kept_with_it() {
        let (_dir, paths) = paths();
        let mut store = open(&paths, "a-1", "m-me");
        let at = row("m-2", Kind::Status, "", "{}");
        store.put(&at).unwrap();
        assert_eq!(store.row("m-2", Kind::Status, "").unwrap().unwrap().head_key, at.head_key);
        let head = at.head.clone().unwrap();
        assert!(store.touch("m-2", Kind::Status, "", &at.stream, &head, &[6; 32], 5).unwrap());
        assert_eq!(store.row("m-2", Kind::Status, "").unwrap().unwrap().head_key, Some([6; 32]));
        drop(store);
        // A file from an older release: no such column.
        let conn = rusqlite::Connection::open(path(&paths)).unwrap();
        conn.execute_batch("ALTER TABLE copies DROP COLUMN head_key").unwrap();
        drop(conn);
        let store = open(&paths, "a-1", "m-me");
        let old = store.row("m-2", Kind::Status, "").unwrap().unwrap();
        assert_eq!(old.head_key, None, "a row from before says none");
        assert!(old.head.is_some());
    }
}
