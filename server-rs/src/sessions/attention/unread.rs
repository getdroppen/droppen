//! How many messages the user has not read yet.
//!
//! `unread` says *whether* the assistant spoke since the user last looked; this
//! says *how much*. One number per row, capped at [`CAP`] — past that the
//! client shows `99+` and the exact figure stops being information.
//!
//! # What is kept, and why that is enough
//!
//! The count depends on two things that move independently: the transcript,
//! which only grows, and the read marker, which the user moves forward every
//! time they look. So the thing worth remembering is not a count but a
//! [`Ledger`]: the timestamps of the newest assistant messages (at most
//! [`CAP`] of them). Counting is then a compare against whatever the read
//! marker says *now*, and costs nothing — no transcript is opened to answer it.
//!
//! The ledger is derived once per transcript and then kept up to date by
//! reading only what was appended since. It lives in two places: a small
//! in-memory memo ([`super`]) for the five-second re-list, and the
//! per-instance `index.db` ([`crate::sessions::index`]) so a restart does not mean
//! re-reading every transcript — the largest of them run to a hundred
//! megabytes.
//!
//! # The cold scan stops early
//!
//! The first derivation walks *backwards* from the end and stops at whichever
//! comes first:
//!
//! * [`CAP`] messages found — any read marker older than all of them reads
//!   `99+`, so nothing further back can change an answer;
//! * a message at or before the current read marker — every message before it
//!   has been read too, and the read marker only ever moves forward. The ledger
//!   records where it stopped ([`Ledger::floor`]) and is only trusted for a read
//!   marker at or after that point; one from before it (a seen entry evicted from
//!   its store) costs a re-scan, never a wrong number;
//! * the start of the file;
//! * [`COLD_BUDGET`] bytes — a safety valve for a transcript that is almost all
//!   tool output. The count then under-reports, the same safe direction the
//!   attention clock already takes: a badge that says too little, not one that
//!   will not go away.
//!
//! Timestamps are compared as strings, like everything else in [`super`]: both
//! CLIs stamp ISO-8601 UTC with milliseconds, and an entry is appended after
//! the ones before it, so a transcript's stamps are non-decreasing.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use super::derive::spoken;
use crate::sessions::util::try_parse;
use crate::sessions::Transcripts;

/// The most the count ever says. The client renders this as `99+`.
pub const CAP: usize = 99;

/// The most a cold scan reads before it settles for what it found.
pub const COLD_BUDGET: u64 = 32 * 1024 * 1024;

/// The most an incremental read takes on. A transcript that grew by more than
/// this since it was last seen is cheaper to walk back from the end — the walk
/// stops at the read marker or the cap — than to read forward in full.
const FORWARD_BUDGET: u64 = 8 * 1024 * 1024;

/// The window searched for the end of the last complete line.
const END_PROBE: u64 = 64 * 1024;

/// The newest assistant messages in one transcript, by timestamp.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    /// One timestamp per message, oldest first, at most [`CAP`] of them.
    pub stamps: Vec<String>,
    /// Where a cold scan stopped because it reached the read marker.
    ///
    /// `None` when the ledger is complete — it reached the file's start, the
    /// cap, or its budget — and any read marker can be counted against it. With
    /// a floor, only a read marker at or after it can: the messages before the
    /// floor were never looked at.
    pub floor: Option<String>,
    /// How many bytes of the file the ledger covers: the end of the last
    /// complete line read. A line still being written is left for next time.
    pub end: u64,
    /// A fingerprint of the file's first line, so a transcript *replaced* by
    /// one of equal or greater length (an adoption copies one over another) is
    /// recognised as a different file rather than resumed at a stale offset.
    /// `None` until the file has a complete first line.
    #[serde(default)]
    pub head: Option<Head>,
}

/// The first line of a file (at most [`HEAD_MAX`] bytes of it), fingerprinted.
///
/// A transcript is append-only, so once its first line is complete those bytes
/// never change; a file whose first line differs is another file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Head {
    /// How many bytes were hashed.
    pub len: u64,
    /// Their FNV-1a hash.
    pub hash: u64,
}

/// The most of a first line the fingerprint reads.
const HEAD_MAX: usize = 4096;

impl Ledger {
    /// The unread count against this read marker, or `None` when the ledger
    /// cannot answer for it (see [`Ledger::floor`]).
    pub fn count(&self, read_at: Option<&str>) -> Option<u32> {
        if let Some(floor) = &self.floor {
            if read_at.is_none_or(|read_at| read_at < floor.as_str()) {
                return None;
            }
        }
        let unread = self
            .stamps
            .iter()
            .filter(|stamp| read_at.is_none_or(|read_at| stamp.as_str() > read_at))
            .count();
        Some(unread.min(CAP) as u32)
    }

    /// When the assistant last spoke, by the newest message in the ledger.
    pub fn newest(&self) -> Option<&str> {
        self.stamps.last().map(String::as_str)
    }

    /// Take messages read forward from [`Ledger::end`], oldest first.
    fn extend(&mut self, newer: Vec<String>, end: u64) {
        self.stamps.extend(newer);
        if self.stamps.len() >= CAP {
            let excess = self.stamps.len() - CAP;
            self.stamps.drain(..excess);
            // A full ledger answers any read marker: older than all of it is
            // `99+`, inside it is exact. Whatever the floor guarded is moot.
            self.floor = None;
        }
        self.end = end;
    }
}

/// The ledger for a transcript, brought up to date.
///
/// `known` is the last ledger derived for this file, if any; `size` is the
/// file's length as the session row reports it. `fallback_at` stands in for an
/// entry with no timestamp of its own — the row's mtime, the same stand-in the
/// attention clock has always used.
///
/// `None` only for an unreadable file.
pub async fn refresh(
    reader: &dyn Transcripts,
    path: &Path,
    known: Option<Ledger>,
    size: u64,
    read_at: Option<&str>,
    fallback_at: &str,
) -> Option<Ledger> {
    if let Some(mut ledger) = known {
        let usable = ledger.count(read_at).is_some();
        // A transcript only grows. One that is shorter than what was read, or
        // whose first line or last-read newline is not where the ledger left
        // them, has been replaced; one that grew enormously is cheaper to walk
        // back.
        if usable && size >= ledger.end && size - ledger.end <= FORWARD_BUDGET && still_the_same(path, &ledger).await.unwrap_or(false) {
            if size > ledger.end {
                let (newer, end) = read_forward(reader, path, ledger.end, fallback_at).await.ok()?;
                ledger.extend(newer, end);
            }
            if ledger.head.is_none() && ledger.end > 0 {
                ledger.head = head_of_file(path).await.ok().flatten();
            }
            return Some(ledger);
        }
    }
    scan_back(reader, path, read_at, fallback_at, COLD_BUDGET).await.ok()
}

/// Whether `path` is still the file the ledger was read from: the same first
/// line, and a line ending exactly where the ledger stopped reading.
///
/// Two small reads, and only on the path that is about to resume reading — a
/// row whose watermark still matches never gets here.
async fn still_the_same(path: &Path, ledger: &Ledger) -> std::io::Result<bool> {
    if ledger.end == 0 {
        // Nothing read yet, so nothing for a replacement to contradict.
        return Ok(true);
    }
    let Some(head) = ledger.head else {
        return Ok(false);
    };
    let mut file = tokio::fs::File::open(path).await?;
    let mut prefix = vec![0u8; usize::try_from(head.len).unwrap_or(HEAD_MAX)];
    if file.read_exact(&mut prefix).await.is_err() || fnv1a(&prefix) != head.hash {
        return Ok(false);
    }
    file.seek(std::io::SeekFrom::Start(ledger.end - 1)).await?;
    let mut last = [0u8; 1];
    if file.read_exact(&mut last).await.is_err() {
        return Ok(false);
    }
    Ok(last[0] == b'\n')
}

/// The fingerprint of a file's first line, or `None` while it has none whole.
pub(super) async fn head_of_file(path: &Path) -> std::io::Result<Option<Head>> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut prefix = Vec::with_capacity(HEAD_MAX);
    (&mut file).take(HEAD_MAX as u64).read_to_end(&mut prefix).await?;
    Ok(match memchr::memchr(b'\n', &prefix) {
        Some(at) => Some(Head {
            len: at as u64 + 1,
            hash: fnv1a(&prefix[..=at]),
        }),
        // A first line longer than the window: its first bytes are settled once
        // anything follows it, and the caller only asks when something does.
        None if prefix.len() == HEAD_MAX => Some(Head {
            len: HEAD_MAX as u64,
            hash: fnv1a(&prefix),
        }),
        None => None,
    })
}

/// 64-bit FNV-1a: a fingerprint, not a defence — nobody is forging transcripts
/// at this server; it only has to tell two different files apart.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The messages on the complete lines after `from`, and where those lines end.
async fn read_forward(reader: &dyn Transcripts, path: &Path, from: u64, fallback_at: &str) -> std::io::Result<(Vec<String>, u64)> {
    let mut file = tokio::fs::File::open(path).await?;
    file.seek(std::io::SeekFrom::Start(from)).await?;
    let mut buf = Vec::new();
    file.take(FORWARD_BUDGET + 1).read_to_end(&mut buf).await?;
    // Only up to the last newline: anything after it is a line still being
    // written, and the next read starts there.
    let Some(last_newline) = buf.iter().rposition(|byte| *byte == b'\n') else {
        return Ok((Vec::new(), from));
    };
    let complete = &buf[..=last_newline];
    let mut stamps = Vec::new();
    for line in complete.split(|byte| *byte == b'\n') {
        stamps.extend(messages_on(reader, line, fallback_at));
    }
    Ok((stamps, from + last_newline as u64 + 1))
}

/// How many cold scans each path has had, so a test can prove one was avoided.
#[cfg(test)]
pub(super) static COLD_SCANS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, usize>>> =
    std::sync::LazyLock::new(Default::default);

/// Walk back from the end until the answer cannot change; see the module note.
async fn scan_back(reader: &dyn Transcripts, path: &Path, read_at: Option<&str>, fallback_at: &str, budget: u64) -> std::io::Result<Ledger> {
    #[cfg(test)]
    {
        *COLD_SCANS.lock().expect("scan counter").entry(path.to_path_buf()).or_default() += 1;
    }
    let end = complete_end(path).await?;
    let mut lines = BackwardBytes::open(path, end, budget).await?;
    // Newest first while walking; reversed once at the end.
    let mut newest_first: Vec<String> = Vec::new();
    let mut floor = None;
    while let Some(found) = lines.next_spoken(reader, fallback_at).await? {
        let at = found[0].clone();
        newest_first.extend(found);
        if newest_first.len() >= CAP {
            break;
        }
        if read_at.is_some_and(|read_at| at.as_str() <= read_at) {
            floor = Some(at);
            break;
        }
    }
    newest_first.truncate(CAP);
    if newest_first.len() >= CAP {
        floor = None;
    }
    newest_first.reverse();
    let head = if end > 0 { head_of_file(path).await? } else { None };
    Ok(Ledger {
        stamps: newest_first,
        floor,
        end,
        head,
    })
}

/// The first chunk a backward scan reads, and the most it doubles to.
const BACKWARD_FIRST_CHUNK: usize = 64 * 1024;
const BACKWARD_MAX_CHUNK: usize = 4 * 1024 * 1024;

/// Complete lines from `end` backwards, as raw bytes.
///
/// [`crate::sessions::util::BackwardLines`] answers the same question with a
/// decoded `String` per line, which is right for a walk that stops after a
/// handful of entries and wrong for this one: a cold scan crosses megabytes of
/// tool output to find a hundred messages, and decoding every line it steps
/// over was most of the cost. Here a line is only ever a slice of the chunk,
/// and only the ones [`might_speak`] lets through are decoded and parsed.
struct BackwardBytes {
    file: tokio::fs::File,
    /// Offset of the earliest byte read so far.
    cursor: u64,
    /// The chunk being walked, with the head of the newer chunk's first line
    /// appended so that line is whole.
    buf: Vec<u8>,
    /// Where in `buf` the lines not yet handed out end.
    upto: usize,
    scanned: u64,
    budget: u64,
    step: usize,
}

impl BackwardBytes {
    async fn open(path: &Path, end: u64, budget: u64) -> std::io::Result<Self> {
        let file = tokio::fs::File::open(path).await?;
        let size = file.metadata().await?.len().min(end);
        Ok(Self {
            file,
            cursor: size,
            buf: Vec::new(),
            upto: 0,
            scanned: 0,
            budget,
            step: BACKWARD_FIRST_CHUNK,
        })
    }

    /// The messages on the next line back that holds any; `None` at the start
    /// of the file or at the budget.
    async fn next_spoken(&mut self, reader: &dyn Transcripts, fallback_at: &str) -> std::io::Result<Option<Vec<String>>> {
        loop {
            // Every complete line left in this chunk, newest first. The piece
            // before the first newline is only whole at the start of the file.
            while let Some(newline) = memchr::memrchr(b'\n', &self.buf[..self.upto]) {
                let line = &self.buf[newline + 1..self.upto];
                self.upto = newline;
                let found = messages_on(reader, line, fallback_at);
                if !found.is_empty() {
                    return Ok(Some(found));
                }
            }
            if self.cursor == 0 {
                let first = &self.buf[..self.upto];
                self.upto = 0;
                let found = messages_on(reader, first, fallback_at);
                return Ok((!found.is_empty()).then_some(found));
            }
            if self.scanned >= self.budget {
                return Ok(None);
            }
            // Read the next chunk below, and carry the unfinished head along.
            let step = self.step.min(usize::try_from(self.budget - self.scanned).unwrap_or(usize::MAX)).max(1);
            let start = self.cursor.saturating_sub(step as u64);
            let len = (self.cursor - start) as usize;
            let mut chunk = vec![0u8; len];
            self.file.seek(std::io::SeekFrom::Start(start)).await?;
            self.file.read_exact(&mut chunk).await?;
            chunk.extend_from_slice(&self.buf[..self.upto]);
            self.upto = chunk.len();
            self.buf = chunk;
            self.cursor = start;
            self.scanned += len as u64;
            self.step = self.step.saturating_mul(2).min(BACKWARD_MAX_CHUNK);
        }
    }
}

/// A cheap pre-test on raw bytes, so a scan only decodes and parses the lines
/// that could possibly hold a message.
///
/// Both CLIs write compact JSON, and an entry that speaks says `"assistant"`
/// unescaped — as its `type` (claude) or its `role` (codex) — and carries
/// either a `"text"` key or claude's plain-string shorthand, `"content":"…"`.
/// A quoted string inside some other entry's content is escaped
/// (`\"assistant\"`) and cannot match, so what gets through is the assistant
/// entries plus the rare entry with a nested assistant object, which the full
/// test in [`spoken`] then rejects. What is kept out is the tool results: most
/// of a transcript's bytes, and none of its messages.
fn might_speak(line: &[u8]) -> bool {
    memchr::memmem::find(line, b"\"assistant\"").is_some()
        && (memchr::memmem::find(line, b"\"text\"").is_some() || memchr::memmem::find(line, b"\"content\":\"").is_some())
}

/// One timestamp per message on a line; empty for a line that holds none.
fn messages_on(reader: &dyn Transcripts, line: &[u8], fallback_at: &str) -> Vec<String> {
    if !might_speak(line) {
        return Vec::new();
    }
    let Some(entry) = try_parse(&String::from_utf8_lossy(line)) else {
        return Vec::new();
    };
    let Some(said) = spoken(reader, &entry) else {
        return Vec::new();
    };
    let at = said.at.unwrap_or_else(|| fallback_at.to_string());
    vec![at; said.messages]
}

/// Where the last complete line of a file ends.
///
/// Usually the file's length — both CLIs write whole lines — but a scan that
/// lands while a line is half-written must stop short of it, or the line would
/// be counted as unparseable now and skipped for ever after. A trailing line
/// longer than [`END_PROBE`] with no newline in sight is taken as whole; the
/// worst that does is miss that one line.
async fn complete_end(path: &Path) -> std::io::Result<u64> {
    let mut file = tokio::fs::File::open(path).await?;
    let size = file.metadata().await?.len();
    if size == 0 {
        return Ok(0);
    }
    let start = size.saturating_sub(END_PROBE);
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let mut buf = Vec::with_capacity((size - start) as usize);
    file.read_to_end(&mut buf).await?;
    Ok(match buf.iter().rposition(|byte| *byte == b'\n') {
        Some(at) => start + at as u64 + 1,
        None if start == 0 => 0,
        None => size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::claude::ClaudeCodeSessions;
    use crate::sessions::codex::CodexSessions;

    const FALLBACK: &str = "2026-01-01T09:59:59.000Z";

    fn at(n: u32) -> String {
        format!("2026-01-01T10:{:02}:{:02}.000Z", n / 60, n % 60)
    }

    fn assistant(n: u32, text: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{}","message":{{"content":[{{"type":"text","text":"{text}"}}]}}}}"#,
            at(n)
        )
    }

    fn user(n: u32) -> String {
        format!(
            r#"{{"type":"user","timestamp":"{}","message":{{"content":[{{"type":"text","text":"go"}}]}}}}"#,
            at(n)
        )
    }

    fn write(dir: &Path, lines: &[String]) -> std::path::PathBuf {
        let path = dir.join("t.jsonl");
        let mut body = lines.join("\n");
        body.push('\n');
        std::fs::write(&path, body).expect("write");
        path
    }

    fn size(path: &Path) -> u64 {
        std::fs::metadata(path).expect("stat").len()
    }

    async fn ledger(reader: &dyn Transcripts, path: &Path, read_at: Option<&str>) -> Ledger {
        refresh(reader, path, None, size(path), read_at, FALLBACK).await.expect("a ledger")
    }

    /// The unit is a message the user reads. Thinking, tool calls, tool results
    /// and a subagent's chatter are not; the CLI's own `<synthetic>` notice — a
    /// rate-limit wall — is, because it is how the session stopped.
    #[tokio::test]
    async fn only_assistant_text_counts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(
            dir.path(),
            &[
                user(0),
                assistant(1, "first"),
                format!(
                    r#"{{"type":"assistant","timestamp":"{}","message":{{"content":[{{"type":"thinking","thinking":"hmm"}},{{"type":"tool_use","name":"Bash","input":{{}}}}]}}}}"#,
                    at(2)
                ),
                format!(
                    r#"{{"type":"user","timestamp":"{}","message":{{"content":[{{"type":"tool_result","content":"{{\"type\":\"assistant\"}}"}}]}}}}"#,
                    at(3)
                ),
                format!(
                    r#"{{"type":"assistant","isSidechain":true,"timestamp":"{}","message":{{"content":[{{"type":"text","text":"subagent"}}]}}}}"#,
                    at(4)
                ),
                format!(
                    r#"{{"type":"assistant","timestamp":"{}","message":{{"model":"<synthetic>","content":[{{"type":"text","text":"You've hit your limit"}}]}}}}"#,
                    at(5)
                ),
                format!(
                    r#"{{"type":"assistant","timestamp":"{}","message":{{"content":[{{"type":"text","text":"   "}}]}}}}"#,
                    at(6)
                ),
                format!(
                    r#"{{"type":"assistant","timestamp":"{}","message":{{"content":[{{"type":"text","text":"two"}},{{"type":"text","text":"blocks"}}]}}}}"#,
                    at(7)
                ),
            ],
        );
        let ledger = ledger(&ClaudeCodeSessions, &path, None).await;
        assert_eq!(
            ledger.stamps,
            vec![at(1), at(5), at(7), at(7)],
            "one per text block, the limit notice included"
        );
        assert_eq!(ledger.count(None), Some(4), "never read: every message counts");
        assert_eq!(ledger.newest(), Some(at(7).as_str()));
    }

    /// A session that stopped on a rate limit or an API error has said its last
    /// word, and the badge must show it — the push notification already does.
    #[tokio::test]
    async fn a_synthetic_notice_at_the_end_is_unread() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(
            dir.path(),
            &[
                assistant(1, "working on it"),
                format!(
                    r#"{{"type":"assistant","timestamp":"{}","message":{{"model":"<synthetic>","content":[{{"type":"text","text":"API Error: 529 overloaded"}}]}}}}"#,
                    at(2)
                ),
            ],
        );
        let ledger = ledger(&ClaudeCodeSessions, &path, Some(&at(1))).await;
        assert_eq!(ledger.count(Some(&at(1))), Some(1));
        assert_eq!(ledger.newest(), Some(at(2).as_str()));
    }

    /// Claude's string shorthand for a one-block message is still a message.
    #[tokio::test]
    async fn a_plain_string_message_counts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(
            dir.path(),
            &[format!(
                r#"{{"type":"assistant","timestamp":"{}","message":{{"content":"just a string"}}}}"#,
                at(1)
            )],
        );
        assert_eq!(ledger(&ClaudeCodeSessions, &path, None).await.stamps, vec![at(1)]);
    }

    /// An adoption copies one transcript over another, and the copy can be as
    /// long as or longer than what was read. Resuming at the old offset would
    /// graft the old count onto the middle of a different file; the first
    /// line's fingerprint and the newline before `end` catch it.
    #[tokio::test]
    async fn a_replacement_of_equal_or_greater_length_is_scanned_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(dir.path(), &[user(0), assistant(1, "a"), assistant(2, "b")]);
        let first = ledger(&ClaudeCodeSessions, &path, None).await;
        assert!(first.head.is_some(), "a complete first line is fingerprinted");

        // Longer, and a different first line.
        let path = write(
            dir.path(),
            &[user(10), assistant(11, "x"), assistant(12, "y"), assistant(13, "z"), assistant(14, "w")],
        );
        let again = refresh(&ClaudeCodeSessions, &path, Some(first.clone()), size(&path), None, FALLBACK)
            .await
            .expect("a ledger");
        assert_eq!(again.stamps, vec![at(11), at(12), at(13), at(14)], "a fresh scan, nothing grafted on");

        // Same first line, but the old `end` now lands mid-line.
        let path = write(dir.path(), &[user(0), assistant(1, "a much longer first answer"), assistant(3, "c")]);
        let shifted = refresh(&ClaudeCodeSessions, &path, Some(first), size(&path), None, FALLBACK)
            .await
            .expect("a ledger");
        assert_eq!(shifted.stamps, vec![at(1), at(3)]);

        // And the genuine article — the same file, appended to — still resumes.
        let path = write(dir.path(), &[user(0), assistant(1, "a")]);
        let original = ledger(&ClaudeCodeSessions, &path, None).await;
        let mut body = std::fs::read_to_string(&path).expect("read");
        body.push_str(&assistant(4, "d"));
        body.push('\n');
        std::fs::write(&path, body).expect("append");
        let grown = refresh(&ClaudeCodeSessions, &path, Some(original.clone()), size(&path), None, FALLBACK)
            .await
            .expect("a ledger");
        assert_eq!(grown.stamps, vec![at(1), at(4)]);
        assert_eq!(grown.head, original.head, "the fingerprint carries over");
    }

    #[tokio::test]
    async fn only_messages_after_the_read_marker_count() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(dir.path(), &[assistant(1, "a"), user(2), assistant(3, "b"), assistant(4, "c")]);
        let ledger = ledger(&ClaudeCodeSessions, &path, Some(&at(2))).await;
        assert_eq!(ledger.count(Some(&at(2))), Some(2));
        assert_eq!(ledger.count(Some(&at(3))), Some(1), "looked at the very moment of the message: read");
        assert_eq!(ledger.count(Some(&at(9))), Some(0), "a read marker newer than everything");
    }

    #[tokio::test]
    async fn the_count_is_capped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let lines: Vec<String> = (0..150).map(|n| assistant(n, "x")).collect();
        let path = write(dir.path(), &lines);

        let never = ledger(&ClaudeCodeSessions, &path, None).await;
        assert_eq!(never.stamps.len(), CAP, "the scan stops at the cap");
        assert_eq!(never.count(None), Some(99), "never read: counts from the start, still capped");
        assert_eq!(never.count(Some(&at(10))), Some(99), "older than everything kept is still 99+");
        assert_eq!(never.count(Some(&at(140))), Some(9), "inside the kept window is exact");
        assert_eq!(never.floor, None, "a full ledger answers any read marker");
    }

    /// A read marker stops the cold walk; the floor says what it is good for.
    #[tokio::test]
    async fn a_scan_that_stopped_at_the_read_marker_answers_only_from_there() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(dir.path(), &[assistant(1, "a"), assistant(2, "b"), assistant(3, "c"), assistant(4, "d")]);
        let ledger = ledger(&ClaudeCodeSessions, &path, Some(&at(2))).await;
        assert_eq!(ledger.stamps, vec![at(2), at(3), at(4)], "nothing older than the marker was read");
        assert_eq!(ledger.floor.as_deref(), Some(at(2).as_str()));
        assert_eq!(ledger.count(Some(&at(2))), Some(2));
        assert_eq!(ledger.count(Some(&at(3))), Some(1), "a later marker is fine");
        assert_eq!(ledger.count(Some(&at(1))), None, "an earlier one cannot be answered");
        assert_eq!(ledger.count(None), None, "nor can 'never read'");

        // …and asking anyway re-scans rather than guessing.
        let rescanned = refresh(&ClaudeCodeSessions, &path, Some(ledger), size(&path), None, FALLBACK)
            .await
            .expect("a ledger");
        assert_eq!(rescanned.count(None), Some(4));
    }

    /// The incremental path: only what was appended is read, and a line still
    /// being written is left for next time instead of being lost.
    #[tokio::test]
    async fn appended_messages_are_read_forward_and_half_lines_wait() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(dir.path(), &[assistant(1, "a")]);
        let first = ledger(&ClaudeCodeSessions, &path, None).await;
        assert_eq!(first.count(None), Some(1));

        let whole = assistant(2, "b");
        let (head, tail) = whole.split_at(20);
        let mut body = std::fs::read_to_string(&path).expect("read");
        body.push_str(head);
        std::fs::write(&path, &body).expect("half a line");
        let mid = refresh(&ClaudeCodeSessions, &path, Some(first.clone()), size(&path), None, FALLBACK)
            .await
            .expect("a ledger");
        assert_eq!(mid.count(None), Some(1), "half a line is not a message yet");
        assert_eq!(mid.end, first.end, "and is not consumed");

        body.push_str(tail);
        body.push('\n');
        std::fs::write(&path, &body).expect("rest of the line");
        let after = refresh(&ClaudeCodeSessions, &path, Some(mid), size(&path), None, FALLBACK)
            .await
            .expect("a ledger");
        assert_eq!(after.stamps, vec![at(1), at(2)]);
        assert_eq!(after.end, size(&path));
    }

    #[tokio::test]
    async fn a_cold_scan_does_not_consume_a_half_written_last_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.jsonl");
        let whole = assistant(2, "b");
        std::fs::write(&path, format!("{}\n{}", assistant(1, "a"), &whole[..20])).expect("write");
        let ledger = ledger(&ClaudeCodeSessions, &path, None).await;
        assert_eq!(ledger.count(None), Some(1));
        assert_eq!(ledger.end, assistant(1, "a").len() as u64 + 1, "it stops at the last newline");
    }

    #[tokio::test]
    async fn a_replaced_transcript_is_scanned_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(dir.path(), &[assistant(1, "a"), assistant(2, "b"), assistant(3, "c")]);
        let first = ledger(&ClaudeCodeSessions, &path, None).await;
        let path = write(dir.path(), &[assistant(5, "z")]);
        let again = refresh(&ClaudeCodeSessions, &path, Some(first), size(&path), None, FALLBACK)
            .await
            .expect("a ledger");
        assert_eq!(again.stamps, vec![at(5)], "shorter than what was read means it is a different file");
    }

    #[tokio::test]
    async fn codex_counts_assistant_text_parts_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(
            dir.path(),
            &[
                format!(
                    r#"{{"type":"response_item","timestamp":"{}","payload":{{"type":"message","role":"user","content":[{{"text":"go"}}]}}}}"#,
                    at(0)
                ),
                format!(
                    r#"{{"type":"response_item","timestamp":"{}","payload":{{"type":"message","role":"assistant","content":[{{"type":"output_text","text":"the answer"}}]}}}}"#,
                    at(1)
                ),
                format!(
                    r#"{{"type":"response_item","timestamp":"{}","payload":{{"type":"reasoning","summary":[{{"text":"thinking"}}]}}}}"#,
                    at(2)
                ),
                format!(
                    r#"{{"type":"response_item","timestamp":"{}","payload":{{"type":"function_call","name":"exec_command","arguments":"{{}}"}}}}"#,
                    at(3)
                ),
                format!(
                    r#"{{"type":"event_msg","timestamp":"{}","payload":{{"type":"agent_message","message":"the answer"}}}}"#,
                    at(4)
                ),
                format!(
                    r#"{{"type":"response_item","timestamp":"{}","payload":{{"type":"message","role":"assistant","content":[{{"text":"<environment_context>x</environment_context>"}},{{"text":"more"}}]}}}}"#,
                    at(5)
                ),
            ],
        );
        let ledger = ledger(&CodexSessions, &path, None).await;
        assert_eq!(ledger.stamps, vec![at(1), at(5)], "the event_msg echo and the wrapper are not messages");
        assert_eq!(ledger.count(Some(&at(1))), Some(1));
    }

    #[tokio::test]
    async fn an_entry_without_a_timestamp_takes_the_fallback() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(
            dir.path(),
            &[r#"{"type":"assistant","message":{"content":[{"type":"text","text":"undated"}]}}"#.to_string()],
        );
        let ledger = ledger(&ClaudeCodeSessions, &path, None).await;
        assert_eq!(ledger.stamps, vec![FALLBACK.to_string()]);
    }

    #[tokio::test]
    async fn a_missing_file_has_no_ledger() {
        assert_eq!(
            refresh(&ClaudeCodeSessions, Path::new("/nonexistent/x.jsonl"), None, 0, None, FALLBACK).await,
            None
        );
    }
}
