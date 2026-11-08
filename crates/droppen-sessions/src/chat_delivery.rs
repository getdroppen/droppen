//! What a chat message attached, recorded where the transcript parser can
//! find it again.
//!
//! A tool-less claude chat's images travel as base64 blocks with no path, and
//! its text files are inlined with only a delimiter to show where they were.
//! The transcript is therefore not enough on its own to say what a message
//! carried: it cannot point a thumbnail at an upload, and a line that happens
//! to look like an inlined block must never become a chip.
//!
//! So the driver records the link in the instance database **after** the line
//! has been written to the CLI:
//!
//! * a row's identity is the delivery's own queue id (`chat_deliveries`'
//!   primary key), so a message retried under the same id overwrites its row
//!   instead of adding another;
//! * the row carries the SHA-256 of the exact text block written, the instant
//!   it was written, and the upload paths, split into images and files.
//!
//! The parser matches an attachment row to a user entry by the text hash and
//! by time: only rows written at or before the entry's own timestamp are
//! candidates, and the latest such row wins. That is what keeps a transcript
//! from before the feature (whose entries have no rows) from claiming a later
//! row, and what a retry or a write that never became an entry cannot shift:
//! the row either replaced its predecessor or sits after every entry it could
//! wrongly match. The one CLI transformation known is the `[Image #N]` marker
//! it prefixes for each image; [`ChatDeliveries::lookup`] also tries the text
//! with those markers removed.
//!
//! Ordinary sessions write no rows and — because the parser only constructs a
//! [`ChatDeliveries`] for a transcript whose cwd is the chats directory — do no
//! database work at all.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use rusqlite::params;
use sha2::{Digest, Sha256};

use droppen_base::time::{now_ms, parse_iso_ms};
use droppen_instance::db::Db;
use droppen_instance::paths::Paths;

use crate::Source;

/// What one delivered chat message attached, as upload paths.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeliveredAttachments {
    /// Image upload paths, in the order the blocks were written.
    pub images: Vec<String>,
    /// Non-image upload paths, in the order they were inlined.
    pub files: Vec<String>,
}

/// Record what the message `delivery_id` carried, after its line was written.
///
/// A retry travels under the same delivery id and replaces the row, so the
/// recorded instant is the write that actually happened. A failure is logged
/// and dropped: the message still reached the CLI; the transcript simply shows
/// no chip or thumbnail for it.
pub fn record(paths: &Paths, source: Source, session: &str, delivery_id: &str, text: &str, images: &[String], files: &[String]) {
    let Ok(images) = serde_json::to_string(images) else { return };
    let Ok(files) = serde_json::to_string(files) else { return };
    let db = droppen_instance::db::at(paths.instance_db());
    if let Err(error) = db.write(|tx| {
        tx.execute(
            "INSERT INTO chat_deliveries (source, session_id, delivery_id, text_hash, at_ms, images, files)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(source, session_id, delivery_id) DO UPDATE SET
                 text_hash = excluded.text_hash,
                 at_ms = excluded.at_ms,
                 images = excluded.images,
                 files = excluded.files",
            params![source.as_str(), session, delivery_id, text_hash(text), now_ms(), images, files],
        )?;
        Ok(())
    }) {
        tracing::warn!(session, %error, "cannot record a chat message's attachments");
    }
}

/// Delete rows whose uploads are all gone, after the upload sweep.
///
/// A row with no attachments is kept: it is what tells the parser that a later
/// message with the same text had nothing attached. A row whose files are all
/// gone can show neither thumbnails nor chips, and its inlined block has
/// already served its purpose.
pub fn prune_missing(paths: &Paths) {
    let db = droppen_instance::db::at(paths.instance_db());
    let rows = db
        .read(|conn| {
            let mut statement = conn.prepare("SELECT source, session_id, delivery_id, images, files FROM chat_deliveries")?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?;
            Ok(rows.filter_map(Result::ok).collect::<Vec<_>>())
        })
        .unwrap_or_default();
    let gone: Vec<(String, String, String)> = rows
        .into_iter()
        .filter(|(_, _, _, images, files)| {
            let images: Vec<String> = serde_json::from_str(images).unwrap_or_default();
            let files: Vec<String> = serde_json::from_str(files).unwrap_or_default();
            let attached: Vec<&String> = images.iter().chain(files.iter()).collect();
            !attached.is_empty() && attached.iter().all(|path| !Path::new(path).is_file())
        })
        .map(|(source, session, delivery, _, _)| (source, session, delivery))
        .collect();
    if gone.is_empty() {
        return;
    }
    if let Err(error) = db.write(|tx| {
        let mut statement = tx.prepare("DELETE FROM chat_deliveries WHERE source = ?1 AND session_id = ?2 AND delivery_id = ?3")?;
        for (source, session, delivery) in &gone {
            statement.execute(params![source, session, delivery])?;
        }
        Ok(())
    }) {
        tracing::warn!(%error, "cannot prune swept chat deliveries");
    }
}

/// Delete every row of one session, when the chat is archived or deleted.
pub fn prune_session(paths: &Paths, source: Source, session: &str) {
    let db = droppen_instance::db::at(paths.instance_db());
    if let Err(error) = db.write(|tx| {
        tx.execute(
            "DELETE FROM chat_deliveries WHERE source = ?1 AND session_id = ?2",
            params![source.as_str(), session],
        )?;
        Ok(())
    }) {
        tracing::warn!(session, %error, "cannot prune a chat's deliveries");
    }
}

/// One transcript's view of the delivery rows, consumed in file order.
pub struct ChatDeliveries {
    db: Arc<Db>,
    source: Source,
    session: String,
    /// Row keys already matched in this pass, so two entries at the same
    /// instant do not share one delivery.
    consumed: HashSet<String>,
}

impl ChatDeliveries {
    /// The deliveries of one session's chat transcript.
    pub fn new(paths: &Paths, source: Source, session: &str) -> Self {
        Self {
            db: droppen_instance::db::at(paths.instance_db()),
            source,
            session: session.to_string(),
            consumed: HashSet::new(),
        }
    }

    /// What the user entry whose text block is `text`, written at
    /// `timestamp`, attached.
    ///
    /// Only deliveries written at or before that instant are candidates: an
    /// entry from before this server recorded anything cannot claim a later
    /// message's row. The latest qualifying row wins; a row already matched in
    /// this pass is skipped.
    pub fn lookup(&mut self, text: &str, timestamp: Option<&str>) -> Option<DeliveredAttachments> {
        let at = entry_time_ms(timestamp?)?;
        let mut hashes = vec![text_hash(text)];
        let stripped = strip_image_markers(text);
        if stripped != text {
            hashes.push(text_hash(stripped));
        }
        for hash in hashes {
            let rows = self
                .db
                .read(|conn| {
                    let mut statement = conn.prepare(
                        "SELECT delivery_id, images, files FROM chat_deliveries
                         WHERE source = ?1 AND session_id = ?2 AND text_hash = ?3 AND at_ms <= ?4
                         ORDER BY at_ms DESC, delivery_id DESC",
                    )?;
                    let rows = statement.query_map(params![self.source.as_str(), self.session, hash, at], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    })?;
                    Ok(rows.filter_map(Result::ok).collect::<Vec<_>>())
                })
                .unwrap_or_default();
            for (delivery, images, files) in rows {
                if !self.consumed.insert(delivery) {
                    continue;
                }
                let (Ok(images), Ok(files)) = (serde_json::from_str(&images), serde_json::from_str(&files)) else {
                    continue;
                };
                return Some(DeliveredAttachments { images, files });
            }
        }
        None
    }
}

/// The transcript entry's instant, in milliseconds.
///
/// A stamp truncated to whole seconds is treated as the end of that second:
/// the CLI writes it after the delivery row, so the row's millisecond instant
/// can be later than the truncated stamp while still being before the entry.
fn entry_time_ms(timestamp: &str) -> Option<i64> {
    let base = parse_iso_ms(timestamp)?;
    Some(if timestamp.as_bytes().get(19) == Some(&b'.') { base } else { base + 999 })
}

/// The SHA-256 of a text block, hex.
fn text_hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `text` with the CLI's leading `[Image #N]` markers removed.
///
/// The CLI prefixes one per image block when it writes the user entry. Only
/// the leading run is touched, so a user who typed the same marker themselves
/// further in keeps their words; the text hashed for the record has no markers.
fn strip_image_markers(mut text: &str) -> &str {
    while let Some(rest) = text.strip_prefix("[Image #") {
        let Some(end) = rest.find(']') else { break };
        if end == 0 || !rest[..end].bytes().all(|byte| byte.is_ascii_digit()) {
            break;
        }
        let after = &rest[end + 1..];
        text = after.strip_prefix(' ').unwrap_or(after);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(home: &std::path::Path) -> Paths {
        Paths::new(home.to_path_buf())
    }

    fn upload(paths: &Paths, name: &str) -> String {
        let dir = paths.uploads_dir().join("abc");
        std::fs::create_dir_all(&dir).expect("uploads dir");
        let path = dir.join(name);
        std::fs::write(&path, b"bytes").expect("upload");
        path.to_string_lossy().into_owned()
    }

    /// A fixed entry stamp, and a delivery written before it.
    fn before(stamp: &str) -> i64 {
        entry_time_ms(stamp).expect("stamp") - 1_000
    }

    fn row(paths: &Paths, id: &str, at: i64, hash: &str, images: &[String], files: &[String]) -> String {
        let db = droppen_instance::db::at(paths.instance_db());
        db.write(|tx| {
            tx.execute(
                "INSERT INTO chat_deliveries (source, session_id, delivery_id, text_hash, at_ms, images, files) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(source, session_id, delivery_id) DO UPDATE SET text_hash = excluded.text_hash, at_ms = excluded.at_ms, images = excluded.images, files = excluded.files",
                params!["claude", "s-1", id, hash, at, serde_json::to_string(images).unwrap(), serde_json::to_string(files).unwrap()],
            )
        })
        .map(|_| id.to_string())
        .expect("row")
    }

    #[test]
    fn only_the_clis_leading_image_markers_are_stripped() {
        assert_eq!(strip_image_markers("[Image #1] look"), "look");
        assert_eq!(strip_image_markers("[Image #1] [Image #2] look"), "look");
        assert_eq!(strip_image_markers("look [Image #1] at this"), "look [Image #1] at this");
        assert_eq!(strip_image_markers("[Image #x] look"), "[Image #x] look");
    }

    /// A row is matched by its text and only when it was written before the
    /// entry; the latest such row wins.
    #[test]
    fn a_delivery_is_found_by_its_text_and_time() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = paths(home.path());
        let image = vec!["/u/a.png".to_string()];
        row(&paths, "old", 1_000, &text_hash("look"), &[], &[]);
        row(&paths, "new", 2_000, &text_hash("look"), &image, &[]);
        let stamp = "2026-10-01T10:00:03.000Z"; // 3 s after the epoch-like rows

        let mut deliveries = ChatDeliveries::new(&paths, Source::Claude, "s-1");
        assert_eq!(deliveries.lookup("look", Some(stamp)).expect("the latest").images, image);
        let mut earlier = ChatDeliveries::new(&paths, Source::Claude, "s-1");
        assert_eq!(
            earlier.lookup("look", Some("1970-01-01T00:00:00.500Z")),
            None,
            "no row was written before this entry"
        );
    }

    /// The upgrade case: an entry from before the feature has no row of its
    /// own and must not consume the next identical message's row.
    #[test]
    fn an_entry_from_before_the_feature_never_claims_a_later_row() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = paths(home.path());
        let image = vec!["/u/new.png".to_string()];
        row(&paths, "new", before("2026-10-01T10:00:10.000Z"), &text_hash("what's this?"), &image, &[]);

        let mut deliveries = ChatDeliveries::new(&paths, Source::Claude, "s-1");
        assert_eq!(
            deliveries.lookup("what's this?", Some("2026-10-01T09:00:00.000Z")),
            None,
            "an old identical text predates the row"
        );
        assert_eq!(
            deliveries.lookup("what's this?", Some("2026-10-01T10:00:11.000Z")).expect("its own row").images,
            image
        );
    }

    /// A message with nothing attached must still have a row, or the next
    /// identical text would take the previous message's image.
    #[test]
    fn an_empty_delivery_keeps_later_identical_texts_apart() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = paths(home.path());
        let image = vec!["/u/first.png".to_string()];
        row(&paths, "first", before("2026-10-01T10:00:01.000Z"), &text_hash("look"), &image, &[]);
        row(&paths, "empty", before("2026-10-01T10:00:02.000Z"), &text_hash("look"), &[], &[]);

        let mut deliveries = ChatDeliveries::new(&paths, Source::Claude, "s-1");
        assert_eq!(deliveries.lookup("look", Some("2026-10-01T10:00:00.500Z")).expect("first").images, image);
        assert_eq!(
            deliveries.lookup("look", Some("2026-10-01T10:00:02.500Z")).expect("empty"),
            DeliveredAttachments::default(),
            "the empty row, not the first message's image"
        );
    }

    /// A retry under the same delivery id replaces the row; it does not add a
    /// second one for the next identical text to shift onto.
    #[test]
    fn a_retry_overwrites_its_row() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = paths(home.path());
        record(&paths, Source::Claude, "s-1", "q-1", "look", &["/u/first.png".to_string()], &[]);
        record(&paths, Source::Claude, "s-1", "q-1", "look", &["/u/second.png".to_string()], &[]);
        let db = droppen_instance::db::at(paths.instance_db());
        let count: i64 = db
            .read(|conn| conn.query_row("SELECT count(*) FROM chat_deliveries", [], |row| row.get(0)))
            .expect("count");
        assert_eq!(count, 1, "a retry must replace, not add");
        let mut deliveries = ChatDeliveries::new(&paths, Source::Claude, "s-1");
        assert_eq!(
            deliveries.lookup("look", Some("2999-01-01T00:00:00.000Z")).expect("the retry").images,
            ["/u/second.png"]
        );
    }

    /// A row written after an entry cannot match it: a write that never became
    /// an entry (or a failed one) is inert.
    #[test]
    fn a_row_written_after_the_entry_never_matches_it() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = paths(home.path());
        row(&paths, "future", before("2026-10-01T12:00:00.000Z"), &text_hash("look"), &["/u/a.png".to_string()], &[]);
        let mut deliveries = ChatDeliveries::new(&paths, Source::Claude, "s-1");
        assert_eq!(deliveries.lookup("look", Some("2026-10-01T11:59:58.000Z")), None);
    }

    #[test]
    fn another_session_and_another_source_do_not_see_the_row() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = paths(home.path());
        record(&paths, Source::Claude, "s-1", "q-1", "look", &["/u/a.png".to_string()], &[]);
        assert!(ChatDeliveries::new(&paths, Source::Claude, "s-2").lookup("look", Some("2999-01-01T00:00:00.000Z")).is_none());
        assert!(ChatDeliveries::new(&paths, Source::Codex, "s-1").lookup("look", Some("2999-01-01T00:00:00.000Z")).is_none());
    }

    /// The prune drops rows whose uploads are all gone, keeps empty ones and
    /// removes a whole session's rows on archive.
    #[test]
    fn the_prune_drops_swept_rows_and_keeps_empty_ones() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = paths(home.path());
        let gone = paths.uploads_dir().join("abc/gone.png").to_string_lossy().into_owned();
        record(&paths, Source::Claude, "s-1", "swept", "look", &[gone], &[]);
        record(&paths, Source::Claude, "s-1", "empty", "nothing", &[], &[]);
        prune_missing(&paths);
        assert!(ChatDeliveries::new(&paths, Source::Claude, "s-1").lookup("look", Some("2999-01-01T00:00:00.000Z")).is_none());
        assert_eq!(
            ChatDeliveries::new(&paths, Source::Claude, "s-1").lookup("nothing", Some("2999-01-01T00:00:00.000Z")),
            Some(DeliveredAttachments::default()),
            "an empty row survives the sweep"
        );

        prune_session(&paths, Source::Claude, "s-1");
        assert!(ChatDeliveries::new(&paths, Source::Claude, "s-1").lookup("nothing", Some("2999-01-01T00:00:00.000Z")).is_none());
    }

    #[test]
    fn a_whole_second_stamp_is_read_as_the_end_of_its_second() {
        assert_eq!(entry_time_ms("2026-10-01T10:00:00Z"), parse_iso_ms("2026-10-01T10:00:00Z").map(|ms| ms + 999));
        assert_eq!(
            entry_time_ms("2026-10-01T10:00:00.000Z"),
            parse_iso_ms("2026-10-01T10:00:00.000Z")
        );
    }
}
