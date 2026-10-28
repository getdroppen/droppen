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

use crate::sessions::transcript::content_blocks;
use crate::sessions::util::{read_tail_lines, try_parse};
use crate::sessions::Source;

/// How much of a transcript's end is read. `TAIL_BYTES` in
/// [`crate::sessions::claude`] and [`crate::sessions::codex`], for the same
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

/// The texts in one transcript entry that are the agent *speaking to the user*,
/// trimmed, in the order they were written.
///
/// **The one definition** behind the badge, the unread count and the push
/// notification ([`crate::attention::unread`] counts these; [`claude_echo`] and
/// [`codex_echo`] take the newest), so the three can never disagree about
/// whether something was said. One entry per transcript row the session view
/// draws for it:
///
/// * claude: each non-empty `text` block of a main-chain `assistant` entry.
///   `isSidechain` is a subagent talking to itself, and `thinking` / `tool_use`
///   are the model working. A `<synthetic>` entry — the CLI writing in the
///   model's voice: a rate-limit wall, an API error — *does* count: it is the
///   last thing the session said, and a session that stopped on a limit is
///   exactly one the user needs to see has stopped.
/// * codex: each non-empty text part of an assistant `message` payload, minus
///   the injected wrappers (`<environment_context>`, `AGENTS.md`, …), which are
///   instructions rather than speech. `reasoning` and tool calls are not
///   messages, and an `event_msg` is an echo of a `response_item` already read.
///
/// remote-hand's own rows (task notifications, hand-backs) are user-side
/// entries and never reach this test.
pub fn spoken_texts(source: Source, entry: &serde_json::Map<String, Value>) -> Vec<String> {
    let keep = |text: &str| {
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    };
    match source {
        Source::Acp => crate::sessions::acp_transcript::spoken_texts(entry),
        Source::Claude => {
            if entry.get("isSidechain") == Some(&Value::Bool(true)) || entry.get("type").and_then(Value::as_str) != Some("assistant") {
                return Vec::new();
            }
            content_blocks(entry.get("message"))
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str).and_then(keep))
                .collect()
        }
        Source::Codex => {
            if entry.get("type").and_then(Value::as_str) != Some("response_item") {
                return Vec::new();
            }
            let Some(payload) = entry.get("payload").and_then(Value::as_object) else {
                return Vec::new();
            };
            if payload.get("type").and_then(Value::as_str) != Some("message") || payload.get("role").and_then(Value::as_str) != Some("assistant") {
                return Vec::new();
            }
            payload
                .get("content")
                .and_then(Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|part| part.get("text").and_then(Value::as_str))
                        .filter(|text| !crate::sessions::codex::is_wrapper_text(text))
                        .filter_map(keep)
                        .collect()
                })
                .unwrap_or_default()
        }
    }
}

/// The newest thing said in a tail, by [`spoken_texts`].
fn newest_echo(source: Source, tail: &[String]) -> Option<AssistantEcho> {
    for line in tail.iter().rev() {
        let Some(entry) = try_parse(line) else { continue };
        if let Some(text) = spoken_texts(source, &entry).pop() {
            return Some(AssistantEcho {
                at: entry.get("timestamp").and_then(Value::as_str).map(str::to_owned),
                text,
            });
        }
    }
    None
}

/// The newest assistant text in a Claude transcript tail.
///
/// Walks back past every entry that says nothing — a turn that ends mid-tool is
/// the session working, and the newest text before it is still the last thing
/// the user was told. What counts as saying something is [`spoken_texts`].
pub fn claude_echo(tail: &[String]) -> Option<AssistantEcho> {
    newest_echo(Source::Claude, tail)
}

/// The newest assistant text in a Codex rollout tail, by [`spoken_texts`].
///
/// Codex has no sidechain of its own — a subagent thread is a separate rollout
/// file, hidden from the list by discovery.
pub fn codex_echo(tail: &[String]) -> Option<AssistantEcho> {
    newest_echo(Source::Codex, tail)
}

/// What one transcript line adds to the unread count: [`spoken_texts`],
/// counted, with the entry's timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spoken {
    /// The entry's own timestamp, when it carried one.
    pub at: Option<String>,
    /// How many messages the entry holds; never zero.
    pub messages: usize,
}

/// [`Spoken`] for one parsed line, or `None` when it holds no message.
pub fn spoken(source: Source, entry: &serde_json::Map<String, Value>) -> Option<Spoken> {
    let messages = spoken_texts(source, entry).len();
    (messages > 0).then(|| Spoken {
        at: entry.get("timestamp").and_then(Value::as_str).map(str::to_owned),
        messages,
    })
}

/// [`claude_echo`] or [`codex_echo`], by source.
pub fn echo(source: Source, tail: &[String]) -> Option<AssistantEcho> {
    match source {
        Source::Claude => claude_echo(tail),
        Source::Codex => codex_echo(tail),
        Source::Acp => newest_echo(Source::Acp, tail),
    }
}

/// [`echo`], reading the tail itself. `None` for an unreadable file.
pub async fn read_echo(source: Source, file_path: &Path) -> Option<AssistantEcho> {
    let tail = read_tail_lines(file_path, TAIL_BYTES).await.ok()?;
    echo(source, &tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|l| (*l).to_string()).collect()
    }

    #[test]
    fn assistant_text_sets_the_clock() {
        let tail = lines(&[
            r#"{"type":"user","timestamp":"2026-01-01T00:00:00.000Z","message":{"content":[{"type":"text","text":"go"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:05.000Z","message":{"content":[{"type":"text","text":"  all done  "}]}}"#,
        ]);
        let echo = claude_echo(&tail).expect("an echo");
        assert_eq!(echo.at.as_deref(), Some("2026-01-01T00:00:05.000Z"));
        assert_eq!(echo.text, "all done", "the body is trimmed");
    }

    /// The clock is about the assistant *speaking*. A turn that ends mid-tool is
    /// the session working, and the newest text before it is still the last
    /// thing the user was told.
    #[test]
    fn thinking_and_tool_entries_do_not_move_the_clock() {
        let tail = lines(&[
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:05.000Z","message":{"content":[{"type":"text","text":"working on it"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:06.000Z","message":{"content":[{"type":"thinking","thinking":"hmm"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:07.000Z","message":{"content":[{"type":"tool_use","name":"Bash","input":{}}]}}"#,
            r#"{"type":"user","timestamp":"2026-01-01T00:00:08.000Z","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
        ]);
        let echo = claude_echo(&tail).expect("an echo");
        assert_eq!(echo.at.as_deref(), Some("2026-01-01T00:00:05.000Z"));
        assert_eq!(echo.text, "working on it");
    }

    /// A Task subagent talks for as long as it runs, and the user never sees a
    /// word of it. A clock that moved on those would mark a busy session unread
    /// the whole time it was busy.
    #[test]
    fn a_sidechain_never_moves_the_clock() {
        let tail = lines(&[
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:05.000Z","message":{"content":[{"type":"text","text":"delegating"}]}}"#,
            r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-01-01T00:00:09.000Z","message":{"content":[{"type":"text","text":"subagent chatter"}]}}"#,
        ]);
        let echo = claude_echo(&tail).expect("an echo");
        assert_eq!(echo.at.as_deref(), Some("2026-01-01T00:00:05.000Z"));
        assert_eq!(echo.text, "delegating");
    }

    /// The CLI writing in the model's voice — a rate-limit wall, an API error —
    /// is the last thing the session said, and both the echo and the count
    /// read it as such. They share one predicate, so they cannot drift apart.
    #[test]
    fn a_synthetic_notice_is_what_the_session_last_said() {
        let tail = lines(&[
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:05.000Z","message":{"content":[{"type":"text","text":"on it"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:06.000Z","message":{"model":"<synthetic>","content":[{"type":"text","text":"You've hit your limit"}]}}"#,
        ]);
        let echo = claude_echo(&tail).expect("an echo");
        assert_eq!(echo.at.as_deref(), Some("2026-01-01T00:00:06.000Z"));
        assert_eq!(echo.text, "You've hit your limit");
        let entry = try_parse(&tail[1]).expect("parses");
        assert_eq!(spoken(Source::Claude, &entry).map(|said| said.messages), Some(1));
    }

    /// A codex wrapper is instructions, for the echo as for the count.
    #[test]
    fn a_codex_wrapper_is_not_speech_for_either_reader() {
        let tail = lines(&[
            r#"{"type":"response_item","timestamp":"2026-01-01T00:00:01.000Z","payload":{"type":"message","role":"assistant","content":[{"text":"real"}]}}"#,
            r#"{"type":"response_item","timestamp":"2026-01-01T00:00:02.000Z","payload":{"type":"message","role":"assistant","content":[{"text":"<environment_context>x</environment_context>"}]}}"#,
        ]);
        assert_eq!(codex_echo(&tail).expect("an echo").text, "real");
        assert_eq!(spoken(Source::Codex, &try_parse(&tail[1]).expect("parses")), None);
    }

    #[test]
    fn an_empty_text_block_is_not_speech() {
        let tail = lines(&[
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:05.000Z","message":{"content":[{"type":"text","text":"real"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:06.000Z","message":{"content":[{"type":"text","text":"   "}]}}"#,
        ]);
        assert_eq!(claude_echo(&tail).expect("an echo").text, "real");
    }

    #[test]
    fn a_transcript_with_no_assistant_text_has_no_clock() {
        let tail = lines(&[r#"{"type":"user","timestamp":"2026-01-01T00:00:00.000Z","message":{"content":[{"type":"text","text":"go"}]}}"#]);
        assert_eq!(claude_echo(&tail), None);
        assert_eq!(codex_echo(&tail), None);
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
        assert_eq!(claude_echo(&tail).expect("an echo").text, "survived");
    }

    #[test]
    fn codex_reads_assistant_messages_and_ignores_the_rest() {
        let tail = lines(&[
            r#"{"type":"response_item","timestamp":"2026-01-01T00:00:01.000Z","payload":{"type":"message","role":"assistant","content":[{"text":"the answer"}]}}"#,
            r#"{"type":"response_item","timestamp":"2026-01-01T00:00:02.000Z","payload":{"type":"reasoning","summary":[{"text":"thinking"}]}}"#,
            r#"{"type":"response_item","timestamp":"2026-01-01T00:00:03.000Z","payload":{"type":"function_call","name":"exec_command","arguments":"{}"}}"#,
            r#"{"type":"event_msg","timestamp":"2026-01-01T00:00:04.000Z","payload":{"type":"token_count"}}"#,
        ]);
        let echo = codex_echo(&tail).expect("an echo");
        assert_eq!(echo.at.as_deref(), Some("2026-01-01T00:00:01.000Z"));
        assert_eq!(echo.text, "the answer");
    }

    #[test]
    fn a_codex_user_message_is_not_the_assistant() {
        let tail = lines(&[
            r#"{"type":"response_item","timestamp":"2026-01-01T00:00:01.000Z","payload":{"type":"message","role":"assistant","content":[{"text":"mine"}]}}"#,
            r#"{"type":"response_item","timestamp":"2026-01-01T00:00:02.000Z","payload":{"type":"message","role":"user","content":[{"text":"yours"}]}}"#,
        ]);
        assert_eq!(codex_echo(&tail).expect("an echo").text, "mine");
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
        assert_eq!(read_echo(Source::Claude, Path::new("/nonexistent/x.jsonl")).await, None);
    }
}
