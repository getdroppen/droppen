//! Where the agent remembers its streams: `<instance dir>/agent/streams.db`.
//!
//! One row per stream with the writer's acknowledged position (`next_seq`,
//! `link`, `epoch`) **and** the local position it stands for (`cursor`: how
//! far into the transcript, which index rows, which status), changed only
//! together, in one transaction, when the hub's `ACK` is handled: a cursor
//! saved apart from the position it describes is how a restart skips or
//! repeats work.
//!
//! An append is written ahead: the encoded records go into `pending` before
//! they are sent, with the position and cursor they lead to. A restart finds
//! them and sends the very same bytes again, which the hub acknowledges
//! whether or not it had already committed them (a byte-identical retry), so
//! a crash between the hub's commit and our `ACK` neither loses nor
//! duplicates a record.
//!
//! The index stream's acknowledged rows live in `index_rows`, updated in the
//! same transaction as the index stream's position.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rh_proto::record::{SealedRecord, StreamId};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::{Db, Durability};
use crate::sessions::Paths;

pub const SCHEMA: &[&str] = &[
    "
    CREATE TABLE streams (
        stream        BLOB PRIMARY KEY,
        kind          TEXT NOT NULL,
        source        TEXT NOT NULL,
        id            TEXT NOT NULL,
        epoch         INTEGER NOT NULL,
        next_seq      INTEGER NOT NULL,
        link          BLOB NOT NULL,
        cursor        TEXT NOT NULL,
        pending       BLOB,
        pending_next  INTEGER,
        pending_link  BLOB,
        pending_cursor TEXT,
        pending_diff  TEXT,
        manifest_upto INTEGER,
        pages         TEXT,
        tail_records  INTEGER NOT NULL DEFAULT 0,
        tail_bytes    INTEGER NOT NULL DEFAULT 0,
        state_bytes   INTEGER NOT NULL DEFAULT 0,
        checkpoint_ms INTEGER,
        updated_ms    INTEGER NOT NULL
    );
    CREATE INDEX streams_by_name ON streams(kind, source, id);
    CREATE TABLE index_rows (
        key   TEXT PRIMARY KEY,
        json  TEXT NOT NULL
    );
",
    "
    -- Streams still to be rebuilt under a new machine key (and so their old
    -- ciphertext deleted from the hub). Kept until the hub acknowledges the
    -- rebuilt copy, so a restart, a lost connection or a refused rebuild
    -- picks it up again (M2 review).
    CREATE TABLE reencrypt (
        stream  BLOB PRIMARY KEY,
        epoch   INTEGER NOT NULL
    );
",
    "
    -- When the oldest record no checkpoint covers was acknowledged: the hub
    -- keeps tails in Redis, so they are compacted within half an hour.
    ALTER TABLE streams ADD COLUMN tail_since_ms INTEGER;
",
    "
    -- What the agent keeps besides its streams: the account's plan as the hub
    -- last told it (`plan`), so a restart keeps the retention window.
    CREATE TABLE kv (
        key    TEXT PRIMARY KEY,
        value  TEXT NOT NULL
    );
    -- Sessions whose copy was deleted to make space (relay-and-plans §3.5):
    -- left out of the copy until the session is updated after `updated_at`.
    CREATE TABLE evicted (
        key         TEXT PRIMARY KEY,
        updated_at  TEXT NOT NULL,
        at_ms       INTEGER NOT NULL
    );
",
    "
    -- The machine-key epoch a stream's name derives from (relay-and-plans §5):
    -- the epoch it was created or migrated in. NULL: named under the first
    -- epoch, as every stream was before `streams/2`.
    ALTER TABLE streams ADD COLUMN name_epoch INTEGER;
",
];

impl Paths {
    /// `<instance dir>/agent/streams.db`.
    pub fn streams_db_path(&self) -> PathBuf {
        self.instance_dir().join("agent").join("streams.db")
    }
}

/// Which stream a row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Index,
    Status,
    Session,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Index => "index",
            Kind::Status => "status",
            Kind::Session => "session",
        }
    }

    fn parse(s: &str) -> Kind {
        match s {
            "index" => Kind::Index,
            "status" => Kind::Status,
            _ => Kind::Session,
        }
    }
}

/// A page of the last acknowledged manifest, for reuse by the next one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRef {
    pub hash: String,
    /// The items `[from, to)` it holds (session streams).
    #[serde(default)]
    pub from: usize,
    #[serde(default)]
    pub to: usize,
    /// The transcript fingerprint at `to`: the page is still right while it holds.
    #[serde(default)]
    pub fp: String,
    /// The machine-key epoch it is sealed in (only reused within it).
    #[serde(default)]
    pub epoch: u32,
    /// The window-start page of a copy the retention window trimmed (its
    /// `from` and `to` are both where the copy starts).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub window: bool,
    /// The newest dated entry it holds (ms), for telling when it is all older
    /// than the retention window (`None`: undated, or from before it was kept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ms: Option<u64>,
    /// Its size as uploaded (0: from before it was kept).
    #[serde(default)]
    pub size: u64,
}

/// An append written ahead of sending.
#[derive(Debug, Clone)]
pub struct Pending {
    /// `expect_seq` and the records, exactly as sent.
    pub expect_seq: u64,
    pub records: Vec<SealedRecord>,
    pub next_seq: u64,
    pub link: [u8; 32],
    pub cursor: String,
    /// Index streams: the rows the append changes (`{upserts, removed}`).
    pub diff: Option<String>,
    pub bytes: i64,
}

/// One stream as remembered.
#[derive(Debug, Clone)]
pub struct Row {
    pub stream: StreamId,
    pub kind: Kind,
    pub source: String,
    pub id: String,
    /// The epoch its last record (or checkpoint) is sealed in.
    pub epoch: u32,
    /// The epoch its name derives from (`None`: the first, from before `streams/2`).
    pub name_epoch: Option<u32>,
    pub next_seq: u64,
    pub link: [u8; 32],
    pub cursor: String,
    pub pending: Option<Pending>,
    pub manifest_upto: Option<u64>,
    pub pages: Vec<PageRef>,
    pub tail_records: u64,
    pub tail_bytes: i64,
    pub state_bytes: i64,
    pub checkpoint_ms: Option<u64>,
    /// When the oldest record no checkpoint covers was acknowledged.
    pub tail_since_ms: Option<u64>,
    /// When the stream last moved (an acknowledged append or checkpoint).
    pub updated_ms: u64,
}

/// The rows an index append adds or removes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexDiff {
    pub upserts: Vec<(String, String)>,
    pub removed: Vec<String>,
}

fn encode_records(records: &[SealedRecord]) -> Vec<u8> {
    let mut out = Vec::new();
    for r in records {
        let one = r.encode();
        out.extend_from_slice(&(one.len() as u32).to_be_bytes());
        out.extend_from_slice(&one);
    }
    out
}

fn decode_records(mut bytes: &[u8]) -> Option<Vec<SealedRecord>> {
    let mut out = Vec::new();
    while !bytes.is_empty() {
        let len = u32::from_be_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
        let one = bytes.get(4..4 + len)?;
        out.push(SealedRecord::decode(one).ok()?);
        bytes = &bytes[4 + len..];
    }
    Some(out)
}

fn arr32(v: Vec<u8>) -> [u8; 32] {
    v.try_into().unwrap_or([0; 32])
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    let pending_blob: Option<Vec<u8>> = r.get("pending")?;
    let pending = match pending_blob {
        Some(blob) if blob.len() >= 8 => {
            let expect_seq = u64::from_be_bytes(blob[..8].try_into().expect("8 bytes"));
            decode_records(&blob[8..]).map(|records| Pending {
                expect_seq,
                bytes: records.iter().map(|r| r.ct.len() as i64).sum(),
                records,
                next_seq: r.get::<_, Option<i64>>("pending_next").ok().flatten().unwrap_or(0) as u64,
                link: arr32(r.get::<_, Option<Vec<u8>>>("pending_link").ok().flatten().unwrap_or_default()),
                cursor: r.get::<_, Option<String>>("pending_cursor").ok().flatten().unwrap_or_default(),
                diff: r.get::<_, Option<String>>("pending_diff").ok().flatten(),
            })
        }
        _ => None,
    };
    let pages: Option<String> = r.get("pages")?;
    Ok(Row {
        stream: StreamId(arr32(r.get("stream")?)),
        kind: Kind::parse(&r.get::<_, String>("kind")?),
        source: r.get("source")?,
        id: r.get("id")?,
        epoch: r.get::<_, i64>("epoch")? as u32,
        name_epoch: r.get::<_, Option<i64>>("name_epoch")?.map(|v| v as u32),
        next_seq: r.get::<_, i64>("next_seq")? as u64,
        link: arr32(r.get("link")?),
        cursor: r.get("cursor")?,
        pending,
        manifest_upto: r.get::<_, Option<i64>>("manifest_upto")?.map(|v| v as u64),
        pages: pages.and_then(|p| serde_json::from_str(&p).ok()).unwrap_or_default(),
        tail_records: r.get::<_, i64>("tail_records")? as u64,
        tail_bytes: r.get("tail_bytes")?,
        state_bytes: r.get("state_bytes")?,
        checkpoint_ms: r.get::<_, Option<i64>>("checkpoint_ms")?.map(|v| v as u64),
        tail_since_ms: r.get::<_, Option<i64>>("tail_since_ms")?.map(|v| v as u64),
        updated_ms: r.get::<_, i64>("updated_ms")? as u64,
    })
}

/// The agent's stream state.
#[derive(Clone)]
pub struct Store {
    db: Arc<Db>,
}

impl Store {
    pub fn open(paths: &Paths) -> Self {
        Self::at(&paths.streams_db_path())
    }

    pub fn at(file: &Path) -> Self {
        Self {
            db: crate::db::at_with(file, SCHEMA, Durability::Committed),
        }
    }

    pub fn get(&self, stream: &StreamId) -> Option<Row> {
        self.db
            .read(|c| {
                c.query_row("SELECT * FROM streams WHERE stream = ?1", params![&stream.0[..]], row)
                    .optional()
            })
            .ok()
            .flatten()
    }

    pub fn all(&self) -> Vec<Row> {
        self.try_all().unwrap_or_default()
    }

    /// Every stream, or the error that kept them from being read — for a
    /// decision that must not take an unreadable store for an empty one (a
    /// delete).
    pub fn try_all(&self) -> rusqlite::Result<Vec<Row>> {
        self.db.read(|c| {
            let mut stmt = c.prepare("SELECT * FROM streams")?;
            let rows = stmt.query_map([], row)?.collect::<rusqlite::Result<Vec<_>>>();
            rows
        })
    }

    /// Mark `streams` for re-encryption under machine key `epoch` — once
    /// streams are named per key epoch (relay-and-plans §5), for a move to a
    /// name of that epoch, kept until the old stream is deleted.
    pub fn mark_reencrypt(&self, streams: &[StreamId], epoch: u32) {
        self.db.commit("streams to re-encrypt", |tx| {
            for stream in streams {
                tx.execute(
                    "INSERT INTO reencrypt (stream, epoch) VALUES (?1, ?2) ON CONFLICT (stream) DO UPDATE SET epoch = MAX(epoch, excluded.epoch)",
                    params![&stream.0[..], epoch],
                )?;
            }
            Ok(())
        });
    }

    /// The streams still to re-encrypt, and the epoch each must reach.
    pub fn reencrypt_list(&self) -> Vec<(StreamId, u32)> {
        self.db
            .read(|c| {
                let mut stmt = c.prepare("SELECT stream, epoch FROM reencrypt")?;
                let rows = stmt
                    .query_map([], |r| Ok((StreamId(arr32(r.get(0)?)), r.get::<_, i64>(1)? as u32)))?
                    .collect::<rusqlite::Result<Vec<_>>>();
                rows
            })
            .unwrap_or_default()
    }

    /// `stream` is re-encrypted (the hub acknowledged its rebuilt copy).
    pub fn clear_reencrypt(&self, stream: &StreamId) {
        self.db.commit("a stream re-encrypted", |tx| {
            tx.execute("DELETE FROM reencrypt WHERE stream = ?1", params![&stream.0[..]])?;
            Ok(())
        });
    }

    /// Forget a stream altogether (the hub deleted it).
    pub fn forget(&self, stream: &StreamId) {
        self.db.commit("a stream forgotten", |tx| {
            tx.execute("DELETE FROM streams WHERE stream = ?1", params![&stream.0[..]])?;
            tx.execute("DELETE FROM reencrypt WHERE stream = ?1", params![&stream.0[..]])?;
            Ok(())
        });
    }

    /// A stream the hub has never seen: at seq 0 with an empty cursor, its
    /// name derived under `name_epoch` (`None`: the first epoch).
    #[allow(clippy::too_many_arguments)]
    pub fn create(&self, stream: &StreamId, kind: Kind, source: &str, id: &str, epoch: u32, name_epoch: Option<u32>, now: u64) -> Row {
        self.db.commit("a new stream", |tx| {
            tx.execute(
                "INSERT OR IGNORE INTO streams (stream, kind, source, id, epoch, name_epoch, next_seq, link, cursor, updated_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, '', ?8)",
                params![&stream.0[..], kind.as_str(), source, id, epoch, name_epoch, &[0u8; 32][..], now as i64],
            )?;
            Ok(())
        });
        self.get(stream).expect("just created")
    }

    /// Write an append ahead of sending it. Nothing else may be pending.
    pub fn write_ahead(&self, stream: &StreamId, pending: &Pending) -> rusqlite::Result<()> {
        let mut blob = pending.expect_seq.to_be_bytes().to_vec();
        blob.extend_from_slice(&encode_records(&pending.records));
        self.db.write(|tx| {
            tx.execute(
                "UPDATE streams SET pending = ?2, pending_next = ?3, pending_link = ?4, pending_cursor = ?5, pending_diff = ?6 WHERE stream = ?1",
                params![
                    &stream.0[..],
                    blob,
                    pending.next_seq as i64,
                    &pending.link[..],
                    pending.cursor,
                    pending.diff
                ],
            )?;
            Ok(())
        })
    }

    /// The hub acknowledged the pending append: its position and cursor (and,
    /// for the index, its rows) become the stream's, all at once.
    pub fn commit_pending(&self, stream: &StreamId, now: u64) -> rusqlite::Result<bool> {
        let Some(row) = self.get(stream) else { return Ok(false) };
        let Some(p) = row.pending else { return Ok(false) };
        self.db.write(|tx| {
            if let Some(diff) = p.diff.as_deref().and_then(|d| serde_json::from_str::<IndexDiff>(d).ok()) {
                for key in &diff.removed {
                    tx.execute("DELETE FROM index_rows WHERE key = ?1", params![key])?;
                }
                for (key, json) in &diff.upserts {
                    tx.execute(
                        "INSERT INTO index_rows (key, json) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET json = excluded.json",
                        params![key, json],
                    )?;
                }
            }
            // The stream's epoch is its last record's: what the next head says.
            let epoch = p.records.last().map_or(row.epoch, |r| r.epoch);
            tx.execute(
                "UPDATE streams SET next_seq = ?2, link = ?3, cursor = ?4, pending = NULL, pending_next = NULL, pending_link = NULL,
                     pending_cursor = NULL, pending_diff = NULL, tail_records = tail_records + ?5, tail_bytes = tail_bytes + ?6, updated_ms = ?7,
                     epoch = ?8, tail_since_ms = CASE WHEN tail_records = 0 OR tail_since_ms IS NULL THEN ?7 ELSE tail_since_ms END
                 WHERE stream = ?1",
                params![
                    &stream.0[..],
                    p.next_seq as i64,
                    &p.link[..],
                    p.cursor,
                    p.records.len() as i64,
                    p.bytes,
                    now as i64,
                    epoch
                ],
            )?;
            Ok(true)
        })
    }

    /// Drop a pending append the hub refused for good (a CAS loss).
    pub fn drop_pending(&self, stream: &StreamId) {
        self.db.commit("dropping a pending append", |tx| {
            tx.execute(
                "UPDATE streams SET pending = NULL, pending_next = NULL, pending_link = NULL, pending_cursor = NULL, pending_diff = NULL WHERE stream = ?1",
                params![&stream.0[..]],
            )?;
            Ok(())
        });
    }

    /// A checkpoint the hub acknowledged: the stream stands at `next_seq`/`link`
    /// (unchanged for a compaction, new for a rebase), its cursor is
    /// `cursor`, and its manifest names `pages`. For the index, `rows` replace
    /// the acknowledged rows.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_checkpoint(
        &self,
        stream: &StreamId,
        epoch: u32,
        next_seq: u64,
        link: &[u8; 32],
        cursor: &str,
        pages: &[PageRef],
        state_bytes: i64,
        rows: Option<&[(String, String)]>,
        now: u64,
    ) -> rusqlite::Result<()> {
        let pages = serde_json::to_string(pages).unwrap_or_else(|_| "[]".into());
        self.db.write(|tx| {
            if let Some(rows) = rows {
                tx.execute("DELETE FROM index_rows", [])?;
                for (key, json) in rows {
                    tx.execute("INSERT INTO index_rows (key, json) VALUES (?1, ?2)", params![key, json])?;
                }
            }
            tx.execute(
                "UPDATE streams SET epoch = ?2, next_seq = ?3, link = ?4, cursor = ?5, pages = ?6, manifest_upto = ?3, tail_records = 0, tail_bytes = 0,
                     tail_since_ms = NULL, state_bytes = ?7, checkpoint_ms = ?8, pending = NULL, pending_next = NULL, pending_link = NULL, pending_cursor = NULL,
                     pending_diff = NULL, updated_ms = ?8
                 WHERE stream = ?1",
                params![&stream.0[..], epoch, next_seq as i64, &link[..], cursor, pages, state_bytes, now as i64],
            )?;
            Ok(())
        })
    }

    /// Note a checkpoint attempt (so a failing one is not retried every tick).
    pub fn touch_checkpoint(&self, stream: &StreamId, now: u64) {
        self.db.commit("a checkpoint attempt", |tx| {
            tx.execute(
                "UPDATE streams SET checkpoint_ms = ?2 WHERE stream = ?1",
                params![&stream.0[..], now as i64],
            )?;
            Ok(())
        });
    }

    /// A value kept beside the streams.
    pub fn get_kv(&self, key: &str) -> Option<String> {
        self.db
            .read(|c| c.query_row("SELECT value FROM kv WHERE key = ?1", params![key], |r| r.get(0)).optional())
            .ok()
            .flatten()
    }

    pub fn set_kv(&self, key: &str, value: &str) {
        self.db.commit("a value beside the streams", |tx| {
            tx.execute(
                "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
            Ok(())
        });
    }

    /// The sessions deleted for space: `source/id` → (their `updated_at` then, when).
    pub fn evicted(&self) -> std::collections::HashMap<String, (String, u64)> {
        self.db
            .read(|c| {
                let mut stmt = c.prepare("SELECT key, updated_at, at_ms FROM evicted")?;
                let rows = stmt
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, String>(1)?, r.get::<_, i64>(2)? as u64))))?
                    .collect::<rusqlite::Result<_>>();
                rows
            })
            .unwrap_or_default()
    }

    /// Session `key`'s copy was deleted for space, as of its `updated_at`.
    pub fn mark_evicted(&self, key: &str, updated_at: &str, now: u64) {
        self.db.commit("a session evicted for space", |tx| {
            tx.execute(
                "INSERT INTO evicted (key, updated_at, at_ms) VALUES (?1, ?2, ?3)
                 ON CONFLICT (key) DO UPDATE SET updated_at = excluded.updated_at, at_ms = excluded.at_ms",
                params![key, updated_at, now as i64],
            )?;
            Ok(())
        });
    }

    /// Session `key` may be copied again.
    pub fn clear_evicted(&self, key: &str) {
        self.db.commit("an evicted session back", |tx| {
            tx.execute("DELETE FROM evicted WHERE key = ?1", params![key])?;
            Ok(())
        });
    }

    /// The index rows the hub holds (as last acknowledged): key → JSON.
    pub fn index_rows(&self) -> std::collections::BTreeMap<String, String> {
        self.try_index_rows().unwrap_or_default()
    }

    /// The acknowledged index rows, or the error that kept them from being read.
    pub fn try_index_rows(&self) -> rusqlite::Result<std::collections::BTreeMap<String, String>> {
        self.db.read(|c| {
            let mut stmt = c.prepare("SELECT key, json FROM index_rows")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<_>>();
            rows
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;
    use rh_proto::record::{self, Compression, LogWriter};

    #[test]
    fn a_pending_append_survives_a_restart_and_commits_with_its_cursor_and_rows_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("streams.db");
        let mk = [3; 32];
        let stream = record::stream_id(&mk, "rh.index", "").unwrap();
        let store = Store::at(&file);
        store.create(&stream, Kind::Index, "rh.index", "", 0, None, 1);
        let mut w = LogWriter::new(&mk, 0, stream, 0, [0; 32]);
        let rec = w.append(2, b"{}", Compression::None, &mut OsRng).unwrap();
        let diff = IndexDiff {
            upserts: vec![("claude/a".into(), "{\"id\":\"a\"}".into())],
            removed: vec![],
        };
        let pending = Pending {
            expect_seq: 0,
            records: vec![rec.clone()],
            next_seq: 1,
            link: rec.link(&stream, &[0; 32]),
            cursor: "{\"rows\":1}".into(),
            diff: Some(serde_json::to_string(&diff).unwrap()),
            bytes: rec.ct.len() as i64,
        };
        store.write_ahead(&stream, &pending).unwrap();
        // "Crash": a fresh handle on the same file (the registry hands back the
        // same connection in-process; the row is what a new process would read).
        let again = Store::at(&file);
        let row = again.get(&stream).unwrap();
        let p = row.pending.expect("still pending");
        assert_eq!(p.records, vec![rec], "the very same bytes are re-sent");
        assert_eq!((row.next_seq, row.cursor.as_str()), (0, ""), "nothing moved before the ACK");
        assert!(again.index_rows().is_empty());
        assert!(again.commit_pending(&stream, 2).unwrap());
        let row = again.get(&stream).unwrap();
        assert_eq!((row.next_seq, row.cursor.as_str(), row.tail_records), (1, "{\"rows\":1}", 1));
        assert_eq!((row.tail_since_ms, row.updated_ms), (Some(2), 2), "the tail's oldest record is dated");
        assert!(row.pending.is_none());
        assert_eq!(again.index_rows().get("claude/a").map(String::as_str), Some("{\"id\":\"a\"}"));
        assert!(!again.commit_pending(&stream, 3).unwrap(), "nothing pending any more");
        again.commit_checkpoint(&stream, 0, 1, &row.link, &row.cursor, &[], 10, None, 4).unwrap();
        let row = again.get(&stream).unwrap();
        assert_eq!(
            (row.tail_records, row.tail_since_ms, row.updated_ms),
            (0, None, 4),
            "a checkpoint covers the tail"
        );
    }

    #[test]
    fn the_plan_and_the_sessions_evicted_for_space_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(&dir.path().join("streams.db"));
        assert_eq!(store.get_kv("plan"), None);
        store.set_kv("plan", "{\"retention_days\":7}");
        store.set_kv("plan", "{\"retention_days\":30}");
        assert_eq!(store.get_kv("plan").as_deref(), Some("{\"retention_days\":30}"));
        store.mark_evicted("claude/a", "2026-10-01T00:00:00.000Z", 5);
        store.mark_evicted("claude/b", "2026-10-02T00:00:00.000Z", 6);
        store.mark_evicted("claude/a", "2026-10-03T00:00:00.000Z", 7);
        store.clear_evicted("claude/b");
        let evicted = store.evicted();
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted["claude/a"], ("2026-10-03T00:00:00.000Z".to_string(), 7));
    }

    #[test]
    fn a_stream_remembers_the_epoch_its_name_derives_from() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(&dir.path().join("streams.db"));
        let legacy = StreamId([1; 32]);
        let moved = StreamId([2; 32]);
        store.create(&legacy, Kind::Session, "claude", "a", 0, None, 1);
        store.create(&moved, Kind::Session, "claude", "a", 3, Some(3), 2);
        assert_eq!(
            store.get(&legacy).unwrap().name_epoch,
            None,
            "named before epoch names: the first epoch's"
        );
        let row = store.get(&moved).unwrap();
        assert_eq!((row.name_epoch, row.epoch), (Some(3), 3));
        // Marked to move, kept until the old one is deleted.
        store.mark_reencrypt(&[legacy], 3);
        assert_eq!(store.reencrypt_list(), vec![(legacy, 3)]);
        store.forget(&legacy);
        assert!(store.reencrypt_list().is_empty());
        assert_eq!(store.all().len(), 1);
    }

    #[test]
    fn page_refs_from_before_the_window_read_as_undated_and_unsized() {
        let old: Vec<PageRef> = serde_json::from_str(r#"[{"hash":"ab","from":0,"to":7,"fp":"f","epoch":0}]"#).unwrap();
        assert_eq!((old[0].window, old[0].last_ms, old[0].size), (false, None, 0));
        let new = PageRef {
            window: true,
            last_ms: Some(9),
            size: 3,
            ..old[0].clone()
        };
        let text = serde_json::to_string(std::slice::from_ref(&new)).unwrap();
        assert_eq!(serde_json::from_str::<Vec<PageRef>>(&text).unwrap(), vec![new]);
        assert!(!serde_json::to_string(&old).unwrap().contains("window"), "only a window page says so");
    }
}
