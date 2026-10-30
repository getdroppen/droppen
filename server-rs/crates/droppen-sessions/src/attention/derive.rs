//! The attention clock, read off the tail of a transcript.
//!
//! One question, asked of both CLIs' file formats: **when did the assistant last
//! actually say something to the user?** Not when it thought, not when it ran a
//! tool, not when a subagent spoke — those are the session working, and a
//! session that is working does not need anyone.
//!
//! That narrowness is the whole point. A tool-heavy turn writes a line every few
//! seconds for minutes on end; a clock that moved on any of them would mark
//! every session unread forever, and the badge would stop meaning anything. The
//! clock moves once per turn, on the sentence the user is meant to read.
//!
//! Everything here works on a *tail*: the newest entries, most-recent-first. A
//! transcript is append-only and can run to hundreds of megabytes, and the
//! answer is always near the end. The window is [`TAIL_BYTES`], the same one
//! session discovery already reads for its own purposes — so a turn whose
//! trailing tool output alone exceeds 64 KiB reads as "no assistant text", i.e.
//! not unread. Under-reporting is the safe direction: the cost is a badge that
//! does not appear, not one that will not go away.

use std::path::Path;

use serde_json::Value;

use crate::util::{read_tail_lines, try_parse};
use crate::{Source, TranscriptSources, Transcripts};

/// How much of a transcript's end is read. `TAIL_BYTES` in
/// `provider::claude_code::sessions::claude` and `droppen_provider_codex::sessions::codex`, for the same
/// reason: it is comfortably more than one turn's worth of trailing entries.
pub const TAIL_BYTES: u64 = 64 * 1024;

/// How much of the assistant's last words a notification body carries.
pub const SNIPPET_CHARS: usize = 140;

/// The last thing the assistant said on the main chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantEcho {
    /// ISO-8601 timestamp of the transcript entry it came from, when the entry
    /// carried one. Both CLIs stamp every line in practice; a fixture may not,
    /// and a caller that needs a clock regardless falls back on the file's mtime
    /// ([`crate::attention::apply`] does).
    pub at: Option<String>,
    /// The text itself, trimmed.
    pub text: String,
}

impl AssistantEcho {
    /// The text as a notification body: one clipped, single-paragraph line.
    ///
    /// Every run of whitespace — and any stray control character — is collapsed
    /// to a single space *before* the clip, so an embedded newline cannot smuggle
    /// a second line into the notification shade, and a clip never lands on a
    /// half of a control sequence.
    pub fn snippet(&self) -> String {
        let flat = collapse_whitespace(&self.text);
        let mut out: String = flat.chars().take(SNIPPET_CHARS).collect();
        if out.chars().count() < flat.chars().count() {
            out.push('…');
        }
        out
    }
}

/// Collapse whitespace and control characters into single spaces.
///
/// Leading and trailing whitespace produce no space (the callers already trim,
/// but a lossy tail read can leave a stray control byte at an edge), and an
/// interior run — spaces, tabs, newlines, a bell — becomes exactly one space.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() || ch.is_control() {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        out.push(ch);
    }
    out
}

/// The newest thing said in a tail, by [`Transcripts::spoken_texts`].
///
/// Walks back past every entry that says nothing — a turn that ends mid-tool is
/// the session working, and the newest text before it is still the last thing
/// the user was told.
pub fn echo(reader: &dyn Transcripts, tail: &[String]) -> Option<AssistantEcho> {
    for line in tail.iter().rev() {
        let Some(entry) = try_parse(line) else { continue };
        if let Some(text) = reader.spoken_texts(&entry).pop() {
            return Some(AssistantEcho {
                at: entry.get("timestamp").and_then(Value::as_str).map(str::to_owned),
                text,
            });
        }
    }
    None
}

/// What one transcript line adds to the unread count:
/// [`Transcripts::spoken_texts`], counted, with the entry's timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spoken {
    /// The entry's own timestamp, when it carried one.
    pub at: Option<String>,
    /// How many messages the entry holds; never zero.
    pub messages: usize,
}

/// [`Spoken`] for one parsed line, or `None` when it holds no message.
pub fn spoken(reader: &dyn Transcripts, entry: &serde_json::Map<String, Value>) -> Option<Spoken> {
    let messages = reader.spoken_texts(entry).len();
    (messages > 0).then(|| Spoken {
        at: entry.get("timestamp").and_then(Value::as_str).map(str::to_owned),
        messages,
    })
}

impl TranscriptSources {
    /// [`echo`], reading the tail of `source`'s transcript at `file_path`
    /// itself. `None` for an unreadable file or a source with no reader.
    pub async fn read_echo(&self, source: Source, file_path: &Path) -> Option<AssistantEcho> {
        let reader = self.get(source)?;
        let tail = read_tail_lines(file_path, TAIL_BYTES).await.ok()?;
        echo(reader, &tail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::test_reader::TestReader;

    /// The echo of the core's test reader; each CLI's own is pinned next to
    /// its reader.
    fn echo_of(tail: &[String]) -> Option<AssistantEcho> {
        echo(&TestReader(Source::Claude), tail)
    }

    fn lines(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|l| (*l).to_string()).collect()
    }

    #[test]
    fn assistant_text_sets_the_clock() {
        let tail = lines(&[
            r#"{"type":"user","timestamp":"2026-01-01T00:00:00.000Z","message":{"content":[{"type":"text","text":"go"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:05.000Z","message":{"content":[{"type":"text","text":"  all done  "}]}}"#,
        ]);
        let echo = echo_of(&tail).expect("an echo");
        assert_eq!(echo.at.as_deref(), Some("2026-01-01T00:00:05.000Z"));
        assert_eq!(echo.text, "all done", "the body is trimmed");
    }

    #[test]
    fn a_transcript_with_no_assistant_text_has_no_clock() {
        let tail = lines(&[r#"{"type":"user","timestamp":"2026-01-01T00:00:00.000Z","message":{"content":[{"type":"text","text":"go"}]}}"#]);
        assert_eq!(echo_of(&tail), None);
    }

    /// A half-written trailing line is normal while a CLI is mid-write, and a
    /// lossy tail read can cut a multi-byte character. Neither may blank the
    /// clock for the whole session.
    #[test]
    fn unparseable_lines_are_skipped_not_fatal() {
        let tail = lines(&[
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:05.000Z","message":{"content":[{"type":"text","text":"survived"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:0"#,
        ]);
        assert_eq!(echo_of(&tail).expect("an echo").text, "survived");
    }

    #[test]
    fn a_snippet_is_clipped_by_characters_not_bytes() {
        let echo = AssistantEcho {
            at: None,
            text: "空".repeat(SNIPPET_CHARS + 10),
        };
        let snippet = echo.snippet();
        assert_eq!(snippet.chars().count(), SNIPPET_CHARS + 1, "the ellipsis is the extra one");
        assert!(snippet.ends_with('…'));

        let short = AssistantEcho {
            at: None,
            text: "brief".to_string(),
        };
        assert_eq!(short.snippet(), "brief", "nothing lost, nothing marked");
    }

    /// A notification body is one line. An embedded newline or control character
    /// must be flattened, never carried into the shade as a second line.
    #[test]
    fn a_snippet_is_a_single_sanitised_line() {
        let echo = AssistantEcho {
            at: None,
            text: "line one\nline two\tafter\u{0007}bell".to_string(),
        };
        let snippet = echo.snippet();
        assert!(!snippet.contains('\n'), "no newline reaches the shade: {snippet:?}");
        assert!(!snippet.chars().any(char::is_control), "and no control character: {snippet:?}");
        assert_eq!(snippet, "line one line two after bell");
    }

    #[tokio::test]
    async fn a_missing_file_has_no_clock() {
        assert_eq!(
            TestReader::sources()
                .read_echo(Source::Claude, Path::new("/nonexistent/x.jsonl"))
                .await,
            None
        );
    }
}
