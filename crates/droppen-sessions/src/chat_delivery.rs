//! What a chat message attached, recorded where the transcript parser can
//! find it again.
//!
//! A tool-less claude chat's images travel as base64 blocks with no path, and
//! its text files are inlined with only a delimiter to show where they were.
//! The transcript is therefore no longer enough on its own to say what a
//! message carried: it cannot point a thumbnail at an upload, and a line that
//! happens to look like an inlined block must never become a chip.
//!
//! So the driver records the link at delivery, while it still holds the paths:
//!
//! * **key:** the source, session id, a SHA-256 of the exact text block written
//!   to the CLI, and an occurrence number for that text within the session;
//! * **value:** the upload paths, split into images and files.
//!
//! The text hash is stable because the text block is what the CLI writes into
//! its transcript. The one transformation known is the `[Image #N]` marker the
//! CLI prefixes for each image; [`ChatDeliveries::lookup`] also tries the text
//! with those markers removed. The occurrence is stable because a transcript
//! is parsed from its first line, so messages carrying identical text are
//! matched in the order they were delivered. A `--session-id` / `--resume`
//! keeps the id, so a restart writes the same key.
//!
//! Ordinary sessions write no rows and — because the parser only constructs a
//! [`ChatDeliveries`] for a transcript whose cwd is the chats directory — do no
//! database work at all.

use std::collections::HashMap;
use std::sync::Arc;

use rusqlite::params;
use sha2::{Digest, Sha256};

use droppen_instance::db::Db;
use droppen_instance::paths::Paths;

use crate::Source;

/// The `kv` scope these rows live under.
const SCOPE: &str = "chat-delivery";

/// What one delivered chat message attached, as upload paths.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeliveredAttachments {
    /// Image upload paths, in the order the blocks were written.
    pub images: Vec<String>,
    /// Non-image upload paths, in the order they were inlined.
    pub files: Vec<String>,
}

/// Record what `text` carried, for the transcript parser to find later.
///
/// Failures are logged and dropped: a message whose bookkeeping could not be
/// written is still a message, and its attachments still reach the model. The
/// transcript simply shows no chip or thumbnail for it.
pub fn record(paths: &Paths, source: Source, session: &str, text: &str, images: &[String], files: &[String]) {
    // Every chat message gets a row, even one with nothing attached. The
    // occurrence numbers are then the transcript's own message order, so two
    // messages with identical text (one with an attachment, one without) are
    // never matched to each other's delivery. The driver only calls this for a
    // chat, so ordinary sessions still write nothing.
    let db = droppen_instance::db::at(paths.instance_db());
    let hash = text_hash(text);
    let prefix = format!("{}/{session}/{hash}/", source.as_str());
    let value = DeliveredAttachments {
        images: images.to_vec(),
        files: files.to_vec(),
    };
    let Ok(value) = serde_json::to_string(&value) else {
        return;
    };
    // The count and the insert in one write lock: two deliveries of the same
    // text must get different occurrence numbers, not overwrite one row.
    if let Err(error) = db.write_immediate(|tx| {
        let occurrence: i64 = tx.query_row(
            "SELECT count(*) FROM kv WHERE scope = ?1 AND key LIKE ?2",
            params![SCOPE, format!("{prefix}%")],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO kv (scope, key, value) VALUES (?1, ?2, ?3)",
            params![SCOPE, format!("{prefix}{occurrence}"), value],
        )?;
        Ok(())
    }) {
        tracing::warn!(session, %error, "cannot record a chat message's attachments");
    }
}

/// One transcript's view of the delivery rows, consumed in file order.
///
/// The occurrence counters live here rather than in a query because the parser
/// walks the transcript once from the start: the nth message with a given text
/// is the nth delivery of it.
pub struct ChatDeliveries {
    db: Arc<Db>,
    source: Source,
    session: String,
    counts: HashMap<String, usize>,
}

impl ChatDeliveries {
    /// The deliveries of one session's chat transcript.
    pub fn new(paths: &Paths, source: Source, session: &str) -> Self {
        Self {
            db: droppen_instance::db::at(paths.instance_db()),
            source,
            session: session.to_string(),
            counts: HashMap::new(),
        }
    }

    /// What the message whose text block is `text` attached, if anything.
    pub fn lookup(&mut self, text: &str) -> Option<DeliveredAttachments> {
        let mut hashes = vec![text_hash(text)];
        let stripped = strip_image_markers(text);
        if stripped != text {
            hashes.push(text_hash(stripped));
        }
        for hash in hashes {
            let next = self.counts.entry(hash.clone()).or_insert(0);
            let occurrence = *next;
            *next += 1;
            let key = format!("{}/{}/{hash}/{occurrence}", self.source.as_str(), self.session);
            let found = self
                .db
                .read(|conn| {
                    conn.query_row("SELECT value FROM kv WHERE scope = ?1 AND key = ?2", params![SCOPE, key], |row| {
                        row.get::<_, String>(0)
                    })
                })
                .ok()
                .and_then(|value| serde_json::from_str::<DeliveredAttachments>(&value).ok());
            if found.is_some() {
                return found;
            }
        }
        None
    }
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

    #[test]
    fn only_the_clis_leading_image_markers_are_stripped() {
        assert_eq!(strip_image_markers("[Image #1] look"), "look");
        assert_eq!(strip_image_markers("[Image #1] [Image #2] look"), "look");
        assert_eq!(strip_image_markers("look [Image #1] at this"), "look [Image #1] at this");
        assert_eq!(strip_image_markers("[Image #x] look"), "[Image #x] look");
    }

    #[test]
    fn a_delivery_is_found_by_its_text_and_occurrence() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = paths(home.path());
        let images = vec!["/u/a.png".to_string()];
        record(&paths, Source::Claude, "s-1", "look", &images, &[]);
        record(&paths, Source::Claude, "s-1", "look", &images, &[]);

        let mut deliveries = ChatDeliveries::new(&paths, Source::Claude, "s-1");
        assert_eq!(deliveries.lookup("look"), Some(DeliveredAttachments { images: images.clone(), files: vec![] }));
        assert_eq!(deliveries.lookup("look"), Some(DeliveredAttachments { images, files: vec![] }));
        assert_eq!(deliveries.lookup("look"), None, "no third delivery");

        // The CLI prefixes a marker per image; the record still matches.
        let mut after_restart = ChatDeliveries::new(&paths, Source::Claude, "s-1");
        assert!(after_restart.lookup("[Image #1] look").is_some(), "a restart starts the counters again");
    }

    #[test]
    fn another_session_and_another_source_do_not_see_the_row() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = paths(home.path());
        record(&paths, Source::Claude, "s-1", "look", &["/u/a.png".to_string()], &[]);
        assert!(ChatDeliveries::new(&paths, Source::Claude, "s-2").lookup("look").is_none());
        assert!(ChatDeliveries::new(&paths, Source::Codex, "s-1").lookup("look").is_none());
    }

    /// Every chat message is a row, so identical text interleaved with and
    /// without attachments keeps its own delivery:
    /// attached(1) → empty → attached(2) must not hand the third message the
    /// first one's image.
    #[test]
    fn an_empty_delivery_keeps_the_occurrence_order() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = paths(home.path());
        let first = vec!["/u/first.png".to_string()];
        let second = vec!["/u/second.png".to_string()];
        record(&paths, Source::Claude, "s-1", "look", &first, &[]);
        record(&paths, Source::Claude, "s-1", "look", &[], &[]);
        record(&paths, Source::Claude, "s-1", "look", &second, &[]);

        let mut deliveries = ChatDeliveries::new(&paths, Source::Claude, "s-1");
        assert_eq!(deliveries.lookup("look").expect("first").images, first);
        assert_eq!(deliveries.lookup("look").expect("second"), DeliveredAttachments::default());
        assert_eq!(deliveries.lookup("look").expect("third").images, second);
        assert_eq!(deliveries.lookup("look"), None, "no fourth delivery");
    }
}
