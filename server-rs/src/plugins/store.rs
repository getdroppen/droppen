//! One plugin's document store: a SQLite file of named JSON documents with a
//! revision each, written by compare-and-set.
//!
//! The wire contract is `docs/plugins/storage-protocol.md`; this is the
//! transport-agnostic half of it. The HTTP routes ([`super::routes`]) and the
//! CLI ([`super::cli`]) are both adapters over the same calls, so a Tauri shell
//! can add a third without touching what is here.
//!
//! The store never interprets a value. It checks names, sizes and revisions,
//! keeps a bounded history of replaced versions, and that is all: a plugin's
//! data format, and its migrations, are the plugin's.
//!
//! # What counts against the limit
//!
//! [`MAX_STORE_BYTES`] is the whole store: every live document's name and
//! value **and** every kept past version's. History is a safety net, so it gives
//! way first: when a write would take the store over, the oldest past versions
//! (of any document) are forgotten until it fits, and only a store whose live
//! documents alone are over the limit refuses the write. Per document, history
//! keeps the newest [`HISTORY_KEPT`] versions and at most
//! [`MAX_HISTORY_BYTES_PER_DOC`] of them.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use droppen_plugin_sdk::{valid_doc_name, Doc, DocMeta, DocVersion, DocWrite, PluginId, StoreExport, EXPORT_FORMAT};
use rusqlite::{OptionalExtension, Transaction};
use serde_json::Value;

use crate::db::{Db, Durability};

/// The store's schema, under the same append-only rule as [`crate::db::SCHEMA`]:
/// an older binary must be able to open what a newer one wrote.
///
/// `doc_history` is a rowid table on purpose: "the newest fifty" is insertion
/// order, which a revision alone cannot say once an import has replaced a
/// document.
pub const STORE_SCHEMA: &[&str] = &["CREATE TABLE IF NOT EXISTS docs (
         name       TEXT    NOT NULL PRIMARY KEY,
         revision   INTEGER NOT NULL,
         value      TEXT    NOT NULL,
         updated_at TEXT    NOT NULL,
         updated_by TEXT
     ) STRICT;
     CREATE TABLE IF NOT EXISTS doc_history (
         name       TEXT    NOT NULL,
         revision   INTEGER NOT NULL,
         value      TEXT    NOT NULL,
         updated_at TEXT    NOT NULL,
         updated_by TEXT
     ) STRICT;
     CREATE INDEX IF NOT EXISTS doc_history_by_name ON doc_history (name);
     CREATE TABLE IF NOT EXISTS meta (
         key   TEXT NOT NULL PRIMARY KEY,
         value TEXT NOT NULL
     ) STRICT"];

/// One document's value, as JSON.
pub const MAX_DOC_BYTES: u64 = 4 * 1024 * 1024;
/// One request's body.
pub const MAX_REQUEST_BYTES: u64 = 8 * 1024 * 1024;
/// One plugin's whole store: live documents and their history, names included.
pub const MAX_STORE_BYTES: u64 = 256 * 1024 * 1024;
/// Replaced versions kept per document, at most.
pub const HISTORY_KEPT: usize = 50;
/// One document's kept versions together, at most (four of the largest).
pub const MAX_HISTORY_BYTES_PER_DOC: u64 = 16 * 1024 * 1024;
/// Versions one history page holds when the caller does not say.
pub const HISTORY_PAGE: usize = 10;
/// One history page's values together, at most — past its first version,
/// which is always included however large.
pub const MAX_HISTORY_PAGE_BYTES: u64 = 8 * 1024 * 1024;

/// Why a store call did not do what it was asked.
#[derive(Debug, Clone, thiserror::Error)]
pub enum StoreError {
    /// A name or body the store will not accept (`400 invalid`).
    #[error("{0}")]
    Invalid(String),
    /// The document does not exist (`404 doc_not_found`).
    #[error("document {0:?} does not exist")]
    DocNotFound(String),
    /// A write named a revision that is not the current one (`409 conflict`).
    #[error("document {doc:?} is not at the revision the write expected")]
    Conflict {
        /// The first document that conflicted.
        doc: String,
        /// That document as it is now; `None` when it does not exist.
        current: Option<Doc>,
    },
    /// Over one of the limits (`413 too_large`).
    #[error("{message}")]
    TooLarge {
        /// Which limit, and by how much.
        message: String,
        /// The limit, in bytes.
        limit: u64,
    },
    /// An import into a store that already holds documents, without `replace`.
    #[error("the store already holds {0} documents")]
    NotEmpty(usize),
    /// An import whose read-back did not match what was written; rolled back.
    #[error("{0}")]
    Rejected(String),
    /// SQLite, or a stored value that no longer parses.
    #[error("{0}")]
    Failed(String),
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        lift(error)
    }
}

/// Abandon a transaction with `error`: the closure's `Err` is what rolls it back.
/// Carried in the one boxed-error variant rusqlite has without extra features.
fn abort(error: StoreError) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

/// The other half of [`abort`].
fn lift(error: rusqlite::Error) -> StoreError {
    match error {
        rusqlite::Error::ToSqlConversionFailure(boxed) => match boxed.downcast::<StoreError>() {
            Ok(store) => *store,
            Err(other) => StoreError::Failed(other.to_string()),
        },
        other => StoreError::Failed(other.to_string()),
    }
}

/// Now, as the store writes `updated_at`.
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// A document as a row holds it, before its value is parsed.
struct Row {
    revision: u64,
    value: String,
    updated_at: String,
    updated_by: Option<String>,
}

impl Row {
    fn into_doc(self, name: &str) -> Result<Doc, StoreError> {
        Ok(Doc {
            name: name.to_string(),
            revision: self.revision,
            updated_at: self.updated_at,
            value: parse(name, &self.value)?,
        })
    }
}

fn parse(name: &str, text: &str) -> Result<Value, StoreError> {
    serde_json::from_str(text).map_err(|error| StoreError::Failed(format!("document {name:?} holds a value that is not JSON: {error}")))
}

fn select(tx: &rusqlite::Connection, name: &str) -> rusqlite::Result<Option<Row>> {
    tx.query_row(
        "SELECT revision, value, updated_at, updated_by FROM docs WHERE name = ?1",
        (name,),
        |row| {
            Ok(Row {
                revision: row.get::<_, i64>(0)?.max(0) as u64,
                value: row.get(1)?,
                updated_at: row.get(2)?,
                updated_by: row.get(3)?,
            })
        },
    )
    .optional()
}

/// Bytes of every live document, name and value. (`length()` of TEXT counts
/// characters; the limit is bytes.)
fn live_bytes(tx: &rusqlite::Connection) -> rusqlite::Result<u64> {
    tx.query_row(
        "SELECT COALESCE(SUM(length(CAST(value AS BLOB)) + length(CAST(name AS BLOB))), 0) FROM docs",
        [],
        |row| row.get::<_, i64>(0),
    )
    .map(|bytes| bytes.max(0) as u64)
}

/// Bytes of every kept past version, name and value.
fn history_bytes(tx: &rusqlite::Connection) -> rusqlite::Result<u64> {
    tx.query_row(
        "SELECT COALESCE(SUM(length(CAST(value AS BLOB)) + length(CAST(name AS BLOB))), 0) FROM doc_history",
        [],
        |row| row.get::<_, i64>(0),
    )
    .map(|bytes| bytes.max(0) as u64)
}

/// Keep `old` as a past version of `name`, and forget all but the newest
/// [`HISTORY_KEPT`] — and, of those, all but the newest that fit in
/// [`MAX_HISTORY_BYTES_PER_DOC`].
fn remember(tx: &Transaction<'_>, name: &str, old: &Row) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO doc_history (name, revision, value, updated_at, updated_by) VALUES (?1, ?2, ?3, ?4, ?5)",
        (name, old.revision as i64, &old.value, &old.updated_at, &old.updated_by),
    )?;
    tx.execute(
        "DELETE FROM doc_history WHERE name = ?1 AND rowid NOT IN
             (SELECT rowid FROM doc_history WHERE name = ?1 ORDER BY rowid DESC LIMIT ?2)",
        (name, HISTORY_KEPT as i64),
    )?;
    let sizes: Vec<(i64, u64)> = {
        let mut statement = tx.prepare("SELECT rowid, length(CAST(value AS BLOB)) FROM doc_history WHERE name = ?1 ORDER BY rowid DESC")?;
        let rows = statement.query_map((name,), |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?.max(0) as u64)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut kept = 0u64;
    for (index, (rowid, size)) in sizes.into_iter().enumerate() {
        kept += size;
        if index > 0 && kept > MAX_HISTORY_BYTES_PER_DOC {
            tx.execute("DELETE FROM doc_history WHERE rowid = ?1", (rowid,))?;
        }
    }
    Ok(())
}

/// Forget the oldest past versions, of any document, until the whole store is
/// within `limit`. The live documents are the caller's to have checked.
fn fit_history(tx: &Transaction<'_>, limit: u64) -> rusqlite::Result<()> {
    let live = live_bytes(tx)?;
    let mut over = (live + history_bytes(tx)?).saturating_sub(limit);
    while over > 0 {
        let oldest: Vec<(i64, u64)> = {
            let mut statement =
                tx.prepare("SELECT rowid, length(CAST(value AS BLOB)) + length(CAST(name AS BLOB)) FROM doc_history ORDER BY rowid LIMIT 256")?;
            let rows = statement.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?.max(0) as u64)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        if oldest.is_empty() {
            break;
        }
        for (rowid, size) in oldest {
            tx.execute("DELETE FROM doc_history WHERE rowid = ?1", (rowid,))?;
            over = over.saturating_sub(size);
            if over == 0 {
                break;
            }
        }
    }
    Ok(())
}

fn upsert(tx: &Transaction<'_>, name: &str, revision: u64, value: &str, updated_at: &str, by: Option<&str>) -> rusqlite::Result<()> {
    let revision = i64::try_from(revision).map_err(|_| abort(StoreError::Invalid("revision out of range".into())))?;
    tx.execute(
        "INSERT INTO docs (name, revision, value, updated_at, updated_by) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (name) DO UPDATE SET revision = excluded.revision, value = excluded.value,
             updated_at = excluded.updated_at, updated_by = excluded.updated_by",
        (name, revision, value, updated_at, by),
    )?;
    Ok(())
}

fn check_name(name: &str) -> Result<(), StoreError> {
    if valid_doc_name(name) {
        Ok(())
    } else {
        Err(StoreError::Invalid(format!(
            "{name:?} is not a document name: 1-200 characters of A-Z a-z 0-9 . _ - in /-separated segments, none empty, . or .."
        )))
    }
}

/// Encode a value and hold it to [`MAX_DOC_BYTES`].
fn encode(name: &str, value: &Value) -> Result<String, StoreError> {
    let text = serde_json::to_string(value).map_err(|error| StoreError::Invalid(error.to_string()))?;
    if text.len() as u64 > MAX_DOC_BYTES {
        return Err(StoreError::TooLarge {
            message: format!("document {name:?} is {} bytes of JSON; one document may hold {MAX_DOC_BYTES}", text.len()),
            limit: MAX_DOC_BYTES,
        });
    }
    Ok(text)
}

fn over_store_limit(total: u64, limit: u64) -> StoreError {
    StoreError::TooLarge {
        message: format!("the plugin's documents (names and values) would come to {total} bytes; a plugin may keep {limit}"),
        limit,
    }
}

/// One page of a document's past versions.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryPage {
    /// Newest first.
    pub versions: Vec<DocVersion>,
    /// Older versions remain past this page.
    pub more: bool,
}

/// One plugin's store on one instance.
#[derive(Debug, Clone)]
pub struct Store {
    path: PathBuf,
    db: Arc<Db>,
    /// [`MAX_STORE_BYTES`], but for the test that fills it.
    store_limit: u64,
}

impl Store {
    /// Open — or make — the store at `path`, once per process.
    ///
    /// Through [`crate::db::at_with`], so it is WAL, `synchronous = FULL`,
    /// `0600`, and a file that will not open is set aside as
    /// `store.corrupt-<time>` rather than refused.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let db = crate::db::at_with(&path, STORE_SCHEMA, Durability::Committed);
        Self {
            path,
            db,
            store_limit: MAX_STORE_BYTES,
        }
    }

    /// The same store held to a smaller total, so a test can fill it.
    #[cfg(test)]
    pub(crate) fn with_store_limit(mut self, limit: u64) -> Self {
        self.store_limit = limit;
        self
    }

    /// The file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// One document, or `None`.
    pub fn get(&self, name: &str) -> Result<Option<Doc>, StoreError> {
        check_name(name)?;
        let row = self.db.read(|conn| select(conn, name))?;
        row.map(|row| row.into_doc(name)).transpose()
    }

    /// Every document whose name starts with `prefix` (all of them for `""`),
    /// without values, by name.
    pub fn list(&self, prefix: &str) -> Result<Vec<DocMeta>, StoreError> {
        Ok(self.db.read(|conn| {
            let mut statement = conn.prepare("SELECT name, revision, updated_at FROM docs WHERE substr(name, 1, length(?1)) = ?1 ORDER BY name")?;
            let rows = statement.query_map((prefix,), |row| {
                Ok(DocMeta {
                    name: row.get(0)?,
                    revision: row.get::<_, i64>(1)?.max(0) as u64,
                    updated_at: row.get(2)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })?)
    }

    /// Every document, values included, by name.
    pub fn all(&self) -> Result<Vec<Doc>, StoreError> {
        let rows = self.db.read(|conn| {
            let mut statement = conn.prepare("SELECT name, revision, value, updated_at, updated_by FROM docs ORDER BY name")?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    Row {
                        revision: row.get::<_, i64>(1)?.max(0) as u64,
                        value: row.get(2)?,
                        updated_at: row.get(3)?,
                        updated_by: row.get(4)?,
                    },
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })?;
        rows.into_iter().map(|(name, row)| row.into_doc(&name)).collect()
    }

    /// How many documents there are.
    pub fn count(&self) -> Result<usize, StoreError> {
        Ok(self
            .db
            .read(|conn| conn.query_row("SELECT count(*) FROM docs", [], |row| row.get::<_, i64>(0)))?
            .max(0) as usize)
    }

    /// The bytes of JSON every live document holds together.
    pub fn bytes(&self) -> Result<u64, StoreError> {
        Ok(self.db.read(live_bytes)?)
    }

    /// Write one document by compare-and-set: a batch of one.
    pub fn put(&self, name: &str, expected_revision: Option<u64>, value: Value, by: Option<&str>) -> Result<Doc, StoreError> {
        let mut docs = self.batch(
            vec![DocWrite {
                name: name.to_string(),
                expected_revision,
                value,
            }],
            by,
        )?;
        docs.pop().ok_or_else(|| StoreError::Failed("the write answered nothing".into()))
    }

    /// Write every one of `writes`, or none of them.
    ///
    /// In one immediate transaction: every name and size is checked, then every
    /// revision, then the plugin's total, and only then is anything written. A
    /// write expecting `None` needs the document not to exist; one expecting
    /// `n` needs it to be at exactly `n`, and leaves it at `n + 1`. The first
    /// write that does not hold is the [`StoreError::Conflict`], carrying that
    /// document as it is now.
    pub fn batch(&self, writes: Vec<DocWrite>, by: Option<&str>) -> Result<Vec<Doc>, StoreError> {
        let mut seen = HashSet::new();
        for write in &writes {
            check_name(&write.name)?;
            if !seen.insert(write.name.as_str()) {
                return Err(StoreError::Invalid(format!("document {:?} appears twice in one batch", write.name)));
            }
        }
        let encoded = writes
            .into_iter()
            .map(|write| encode(&write.name, &write.value).map(|text| (write, text)))
            .collect::<Result<Vec<_>, _>>()?;
        if encoded.is_empty() {
            return Ok(Vec::new());
        }
        let at = now();
        let docs = self.db.write_immediate(|tx| {
            let mut olds = Vec::with_capacity(encoded.len());
            for (write, _) in &encoded {
                let current = select(tx, &write.name)?;
                let holds = match (write.expected_revision, &current) {
                    (None, None) => true,
                    (Some(expected), Some(current)) => current.revision == expected,
                    _ => false,
                };
                if !holds {
                    let current = current.map(|row| row.into_doc(&write.name)).transpose().map_err(abort)?;
                    return Err(abort(StoreError::Conflict {
                        doc: write.name.clone(),
                        current,
                    }));
                }
                olds.push(current);
            }
            let replaced: u64 = encoded
                .iter()
                .zip(&olds)
                .filter_map(|((write, _), old)| old.as_ref().map(|old| (old.value.len() + write.name.len()) as u64))
                .sum();
            let added: u64 = encoded.iter().map(|(write, text)| (text.len() + write.name.len()) as u64).sum();
            let total = live_bytes(tx)?.saturating_sub(replaced) + added;
            if total > self.store_limit {
                return Err(abort(over_store_limit(total, self.store_limit)));
            }
            let mut docs = Vec::with_capacity(encoded.len());
            for ((write, text), old) in encoded.iter().zip(olds) {
                let revision = match write.expected_revision {
                    None => 1,
                    Some(expected) => expected
                        .checked_add(1)
                        .ok_or_else(|| abort(StoreError::Invalid("revision out of range".into())))?,
                };
                if let Some(old) = &old {
                    remember(tx, &write.name, old)?;
                }
                upsert(tx, &write.name, revision, text, &at, by)?;
                docs.push(Doc {
                    name: write.name.clone(),
                    revision,
                    updated_at: at.clone(),
                    value: write.value.clone(),
                });
            }
            fit_history(tx, self.store_limit)?;
            Ok(docs)
        })?;
        Ok(docs)
    }

    /// The versions `name` has been replaced from, newest first: `limit`
    /// (default [`HISTORY_PAGE`], at most [`HISTORY_KEPT`]) of them after
    /// skipping `offset`, cut short once their values pass
    /// [`MAX_HISTORY_PAGE_BYTES`]. The current version is [`Store::get`]'s.
    pub fn history(&self, name: &str, offset: usize, limit: Option<usize>) -> Result<HistoryPage, StoreError> {
        check_name(name)?;
        let limit = limit.unwrap_or(HISTORY_PAGE).clamp(1, HISTORY_KEPT);
        let rows = self.db.read(|conn| {
            let mut statement = conn
                .prepare("SELECT revision, value, updated_at, updated_by FROM doc_history WHERE name = ?1 ORDER BY rowid DESC LIMIT ?2 OFFSET ?3")?;
            // One more than asked, to say whether there is more.
            let rows = statement.query_map((name, limit as i64 + 1, offset as i64), |row| {
                Ok(Row {
                    revision: row.get::<_, i64>(0)?.max(0) as u64,
                    value: row.get(1)?,
                    updated_at: row.get(2)?,
                    updated_by: row.get(3)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })?;
        let mut more = rows.len() > limit;
        let mut versions = Vec::new();
        let mut bytes = 0u64;
        for row in rows.into_iter().take(limit) {
            bytes += row.value.len() as u64;
            if !versions.is_empty() && bytes > MAX_HISTORY_PAGE_BYTES {
                more = true;
                break;
            }
            versions.push(DocVersion {
                revision: row.revision,
                updated_at: row.updated_at,
                updated_by: row.updated_by,
                value: parse(name, &row.value)?,
            });
        }
        Ok(HistoryPage { versions, more })
    }

    /// Make the store hold exactly `docs`, in one transaction: the export
    /// import path, never a plugin's.
    ///
    /// Without `replace`, a store that already holds a document is refused.
    /// With it, every document that is replaced or dropped goes into its
    /// history first, and a replaced document's revision only ever moves
    /// forward (`max(imported, current + 1)`), so a client still holding the
    /// old revision gets a conflict rather than a silent overwrite. A document
    /// new to the store keeps the revision it was imported with.
    ///
    /// Before it commits, every document is read back and handed to `verify`;
    /// an `Err` from it rolls the whole import back as [`StoreError::Rejected`].
    pub fn replace_all(
        &self,
        docs: Vec<Doc>,
        replace: bool,
        by: Option<&str>,
        verify: impl FnOnce(&[Doc]) -> Result<(), String>,
    ) -> Result<Vec<Doc>, StoreError> {
        let mut seen = HashSet::new();
        let mut encoded = Vec::with_capacity(docs.len());
        let mut total = 0u64;
        for doc in docs {
            check_name(&doc.name)?;
            if !seen.insert(doc.name.clone()) {
                return Err(StoreError::Invalid(format!("document {:?} appears twice in the import", doc.name)));
            }
            let text = encode(&doc.name, &doc.value)?;
            total += (text.len() + doc.name.len()) as u64;
            encoded.push((doc, text));
        }
        if total > self.store_limit {
            return Err(over_store_limit(total, self.store_limit));
        }
        self.db
            .write_immediate(|tx| {
                let existing: BTreeMap<String, Row> = {
                    let mut statement = tx.prepare("SELECT name, revision, value, updated_at, updated_by FROM docs")?;
                    let rows = statement.query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            Row {
                                revision: row.get::<_, i64>(1)?.max(0) as u64,
                                value: row.get(2)?,
                                updated_at: row.get(3)?,
                                updated_by: row.get(4)?,
                            },
                        ))
                    })?;
                    rows.collect::<rusqlite::Result<_>>()?
                };
                if !replace && !existing.is_empty() {
                    return Err(abort(StoreError::NotEmpty(existing.len())));
                }
                for (name, old) in &existing {
                    if !seen.contains(name) {
                        remember(tx, name, old)?;
                        tx.execute("DELETE FROM docs WHERE name = ?1", (name,))?;
                    }
                }
                for (doc, text) in &encoded {
                    let revision = match existing.get(&doc.name) {
                        Some(old) => {
                            remember(tx, &doc.name, old)?;
                            doc.revision.max(old.revision + 1)
                        }
                        None => doc.revision,
                    };
                    upsert(tx, &doc.name, revision, text, &doc.updated_at, by)?;
                }
                let live = live_bytes(tx)?;
                if live > self.store_limit {
                    return Err(abort(over_store_limit(live, self.store_limit)));
                }
                fit_history(tx, self.store_limit)?;
                let stored = {
                    let mut statement = tx.prepare("SELECT name, revision, value, updated_at, updated_by FROM docs ORDER BY name")?;
                    let rows = statement.query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            Row {
                                revision: row.get::<_, i64>(1)?.max(0) as u64,
                                value: row.get(2)?,
                                updated_at: row.get(3)?,
                                updated_by: row.get(4)?,
                            },
                        ))
                    })?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()?
                };
                let stored = stored
                    .into_iter()
                    .map(|(name, row)| row.into_doc(&name))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(abort)?;
                verify(&stored).map_err(|why| abort(StoreError::Rejected(why)))?;
                Ok(stored)
            })
            .map_err(lift)
    }

    /// Everything, as `plugin export` writes it.
    pub fn export(&self, plugin: &PluginId) -> Result<StoreExport, StoreError> {
        Ok(StoreExport {
            format: EXPORT_FORMAT.to_string(),
            plugin: plugin.clone(),
            exported_at: now(),
            docs: self.all()?,
        })
    }

    /// `PRAGMA integrity_check`'s answer, `ok` when the file is sound.
    pub fn integrity(&self) -> Result<String, StoreError> {
        Ok(self.db.read(|conn| conn.query_row("PRAGMA integrity_check", [], |row| row.get(0)))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path().join("plugins").join("demo").join("store.db"));
        (dir, store)
    }

    fn write(name: &str, expected: Option<u64>, value: Value) -> DocWrite {
        DocWrite {
            name: name.into(),
            expected_revision: expected,
            value,
        }
    }

    #[test]
    fn a_document_is_created_once_and_then_moved_forward_by_its_revision() {
        let (_dir, store) = store();
        assert_eq!(store.get("state").unwrap(), None);
        let first = store.put("state", None, json!({"a": 1}), Some("ada")).unwrap();
        assert_eq!(first.revision, 1, "the first write makes it 1");
        let again = store.put("state", None, json!({"a": 2}), None).unwrap_err();
        match again {
            StoreError::Conflict { doc, current } => {
                assert_eq!(doc, "state");
                assert_eq!(current.unwrap().value, json!({"a": 1}), "the conflict carries the document as it is");
            }
            other => panic!("expected a conflict, got {other:?}"),
        }
        let second = store.put("state", Some(1), json!({"a": 2}), None).unwrap();
        assert_eq!(second.revision, 2);
        assert!(matches!(store.put("state", Some(1), json!({}), None), Err(StoreError::Conflict { .. })));
        assert!(
            matches!(
                store.put("missing", Some(3), json!({}), None),
                Err(StoreError::Conflict { current: None, .. })
            ),
            "a number means the document must exist"
        );
        let read = store.get("state").unwrap().unwrap();
        assert_eq!((read.revision, read.value), (2, json!({"a": 2})));
    }

    #[test]
    fn a_batch_is_all_or_nothing_and_names_the_first_conflict() {
        let (_dir, store) = store();
        store.put("b", None, json!("b1"), None).unwrap();
        store.put("c", None, json!("c1"), None).unwrap();
        let outcome = store.batch(
            vec![
                write("a", None, json!("a1")),
                write("b", Some(7), json!("b2")),
                write("c", Some(9), json!("c2")),
            ],
            None,
        );
        match outcome {
            Err(StoreError::Conflict { doc, current }) => {
                assert_eq!(doc, "b", "the first conflicting write is the one named");
                assert_eq!(current.unwrap().revision, 1);
            }
            other => panic!("expected a conflict, got {other:?}"),
        }
        assert_eq!(store.get("a").unwrap(), None, "nothing of a failed batch is written");
        let docs = store
            .batch(vec![write("a", None, json!("a1")), write("b", Some(1), json!("b2"))], Some("ada"))
            .unwrap();
        assert_eq!(
            docs.iter().map(|d| (d.name.as_str(), d.revision)).collect::<Vec<_>>(),
            [("a", 1), ("b", 2)]
        );
        assert!(matches!(
            store.batch(vec![write("a", Some(1), json!(1)), write("a", Some(1), json!(2))], None),
            Err(StoreError::Invalid(_))
        ));
        assert!(store.batch(vec![], None).unwrap().is_empty());
    }

    #[test]
    fn names_are_checked_and_listing_is_by_prefix_and_name() {
        let (_dir, store) = store();
        for bad in ["", "/a", "a/", "a//b", "../x", "a b", "名"] {
            assert!(matches!(store.put(bad, None, json!(1), None), Err(StoreError::Invalid(_))), "{bad:?}");
            assert!(matches!(store.get(bad), Err(StoreError::Invalid(_))), "{bad:?}");
        }
        for name in ["state", "events/2026-10", "events/2026-09", "reports/2026-10-12"] {
            store.put(name, None, json!({}), None).unwrap();
        }
        let names = |prefix: &str| store.list(prefix).unwrap().into_iter().map(|m| m.name).collect::<Vec<_>>();
        assert_eq!(names("events/"), ["events/2026-09", "events/2026-10"]);
        assert_eq!(names(""), ["events/2026-09", "events/2026-10", "reports/2026-10-12", "state"]);
        assert!(names("nope").is_empty());
    }

    #[test]
    fn history_keeps_the_replaced_values_newest_first_and_only_fifty() {
        let (_dir, store) = store();
        store.put("state", None, json!(0), Some("ada")).unwrap();
        for n in 1..=60u64 {
            store.put("state", Some(n), json!(n), Some("bob")).unwrap();
        }
        let history = store.history("state", 0, Some(HISTORY_KEPT)).unwrap().versions;
        assert_eq!(history.len(), HISTORY_KEPT);
        assert_eq!(history[0].revision, 60, "newest first: the value revision 61 replaced");
        assert_eq!(history[0].value, json!(59));
        assert_eq!(history[0].updated_by.as_deref(), Some("bob"));
        assert_eq!(history.last().unwrap().revision, 11);
        assert!(store.history("other", 0, None).unwrap().versions.is_empty());
    }

    #[test]
    fn a_document_or_a_store_over_its_limit_is_too_large() {
        let (_dir, store) = store();
        let big = "x".repeat(MAX_DOC_BYTES as usize);
        match store.put("big", None, json!(big), None) {
            Err(StoreError::TooLarge { limit, .. }) => assert_eq!(limit, MAX_DOC_BYTES, "the quotes put it over"),
            other => panic!("expected too_large, got {other:?}"),
        }
        assert_eq!(store.get("big").unwrap(), None);
        // A store's total is checked inside the transaction, so a batch that
        // fits per document but not together is refused whole — and a write
        // that replaces a document is charged only the difference.
        let store = store.with_store_limit(100);
        let chunk = "y".repeat(40);
        let writes = (0..3).map(|i| write(&format!("part/{i}"), None, json!(chunk))).collect();
        match store.batch(writes, None) {
            Err(StoreError::TooLarge { limit, .. }) => assert_eq!(limit, 100),
            other => panic!("expected too_large, got {:?}", other.map(|d| d.len())),
        }
        assert_eq!(store.count().unwrap(), 0);
        store.put("part/0", None, json!(chunk), None).unwrap();
        store.put("part/1", None, json!(chunk), None).unwrap();
        store.put("part/0", Some(1), json!(chunk), None).unwrap();
        assert!(matches!(store.put("part/2", None, json!(chunk), None), Err(StoreError::TooLarge { .. })));
        assert_eq!(store.bytes().unwrap(), 96, "names count too: two of 6 + 42 bytes");
        assert!(
            store.history("part/0", 0, None).unwrap().versions.is_empty(),
            "the replaced version did not fit beside them and gave way"
        );
    }

    #[test]
    fn history_counts_against_the_store_and_gives_way_oldest_first() {
        let (_dir, store) = store();
        // 1 + 2 + 3 + … : every name is 1 byte and every value 22 bytes of JSON.
        let store = store.with_store_limit(200);
        let value = |n: u64| json!(format!("{n:0>20}"));
        store.put("a", None, value(0), None).unwrap();
        store.put("b", None, value(0), None).unwrap();
        for n in 1..=10u64 {
            store.put("a", Some(n), value(n), None).unwrap();
        }
        let total = |store: &Store| -> u64 { store.db.read(|conn| Ok(live_bytes(conn)? + history_bytes(conn)?)).unwrap() };
        assert!(total(&store) <= 200, "history is held to the store's limit: {}", total(&store));
        let kept = store.history("a", 0, Some(HISTORY_KEPT)).unwrap().versions;
        assert_eq!(kept[0].value, value(9), "the newest past version is the one kept");
        assert_eq!(kept.len(), 6, "(200 - 2 × 23 live) / 23 per version");
        // The live documents alone are what may refuse a write.
        assert!(matches!(
            store.put("c", None, json!("x".repeat(200)), None),
            Err(StoreError::TooLarge { .. })
        ));
    }

    #[test]
    fn a_documents_history_is_held_to_its_own_byte_cap_and_answered_in_pages() {
        let (_dir, store) = store();
        let big = |n: usize| json!(format!("{n}{}", "z".repeat(3 * 1024 * 1024)));
        store.put("big", None, big(0), None).unwrap();
        for n in 1..=8 {
            store.put("big", Some(n as u64), big(n), None).unwrap();
        }
        let page = store.history("big", 0, Some(HISTORY_KEPT)).unwrap();
        // 16 MiB of 3 MiB versions: five fit.
        assert_eq!(page.versions.len(), 2, "a page stops once it passes 8 MiB");
        assert!(page.more);
        let next = store.history("big", 2, Some(HISTORY_KEPT)).unwrap();
        assert_eq!(next.versions[0].value, big(5));
        let all: usize = (0..5).map(|offset| store.history("big", offset, Some(1)).unwrap().versions.len()).sum();
        assert_eq!(all, 5, "the per-document cap keeps the newest five");
        assert!(!store.history("big", 4, Some(1)).unwrap().more);

        let (_dir, small) = self::store();
        small.put("s", None, json!(0), None).unwrap();
        for n in 1..=12u64 {
            small.put("s", Some(n), json!(n), None).unwrap();
        }
        let page = small.history("s", 0, None).unwrap();
        assert_eq!((page.versions.len(), page.more), (HISTORY_PAGE, true), "ten unless asked");
        let rest = small.history("s", HISTORY_PAGE, None).unwrap();
        assert_eq!((rest.versions.len(), rest.more), (2, false));
    }

    #[test]
    fn replacing_everything_moves_revisions_forward_and_keeps_the_old_values() {
        let (_dir, store) = store();
        store.put("state", None, json!("old"), None).unwrap();
        store.put("state", Some(1), json!("older?"), None).unwrap();
        store.put("gone", None, json!("bye"), None).unwrap();
        let doc = |name: &str, revision, value| Doc {
            name: name.into(),
            revision,
            updated_at: "2026-10-17T00:00:00.000Z".into(),
            value,
        };
        assert!(matches!(
            store.replace_all(vec![doc("state", 1, json!("new"))], false, None, |_| Ok(())),
            Err(StoreError::NotEmpty(2))
        ));
        let refused = store.replace_all(vec![doc("state", 1, json!("new"))], true, None, |_| Err("no".into()));
        assert!(matches!(refused, Err(StoreError::Rejected(_))));
        assert_eq!(
            store.get("state").unwrap().unwrap().value,
            json!("older?"),
            "a rejected import changes nothing"
        );

        let stored = store
            .replace_all(
                vec![doc("state", 1, json!("new")), doc("fresh", 5, json!("f"))],
                true,
                Some("cli"),
                |docs| {
                    assert_eq!(docs.len(), 2, "the verifier sees what was written, before it commits");
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(
            stored.iter().map(|d| (d.name.as_str(), d.revision)).collect::<Vec<_>>(),
            [("fresh", 5), ("state", 3)]
        );
        assert_eq!(store.get("gone").unwrap(), None, "a document the import does not have is dropped");
        assert_eq!(
            store.history("gone", 0, None).unwrap().versions[0].value,
            json!("bye"),
            "into its history"
        );
        assert_eq!(store.history("state", 0, None).unwrap().versions[0].value, json!("older?"));

        let export = store.export(&PluginId::new("demo").unwrap()).unwrap();
        assert_eq!(export.format, EXPORT_FORMAT);
        let (_other_dir, other) = self::store();
        other.replace_all(export.docs.clone(), false, None, |_| Ok(())).unwrap();
        assert_eq!(other.all().unwrap(), export.docs, "an export restores exactly, revisions included");
    }
}
