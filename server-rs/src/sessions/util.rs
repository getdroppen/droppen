//! Transcript item shape and the text/file helpers the normalizers share.
//!
//! Mirrors the former TypeScript server's `util.ts`. The types here are both
//! the wire shape and the OpenAPI schema, so `TranscriptItem` is named after
//! its `.openapi()` name in the former TypeScript server's `schemas.ts` rather
//! than after the internal `NormalizedItem`.

use std::path::Path;

use gotcha::Schematic;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::openapi_ext::Nullable;

/// Body text must never be cut in practice — there is no way to read the rest.
/// These are emergency backstops for pathological items, not display budgets.
pub const TEXT_LIMIT: usize = 60_000;
/// Backstop for the expanded body of a tool item.
pub const DETAIL_LIMIT: usize = 30_000;

/// Who produced a transcript item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// The person driving the session.
    User,
    /// The model.
    Assistant,
    /// remote-hand's own quiet annotations (slash commands, interrupts, notices).
    System,
}

/// What kind of content an item carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// Plain body text.
    Text,
    /// Extended thinking.
    Thinking,
    /// A tool invocation.
    ToolUse,
    /// A tool's result.
    ToolResult,
    /// The CLI dropped the conversation and replaced it with a summary.
    ///
    /// `detail` carries that summary, so the UI can offer it folded. Without
    /// this kind the boundary line is not in the transcript at all and the
    /// summary arrives as an ordinary user entry — a wall of text the user
    /// never typed, with nothing saying where it came from.
    Compact,
    /// A slash command the user ran. `toolName` is the command (`/compact`),
    /// `text` its arguments, `detail` whatever it printed.
    Command,
}

/// One choice offered by an `AskUserQuestion`.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct QuestionOption {
    /// The option's short label.
    pub label: String,
    /// Longer explanation, when the model supplied one.
    pub description: Nullable<String>,
}

/// A question the model is waiting on an answer for.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct QuestionEntry {
    /// The question text.
    pub question: String,
    /// Short header shown above the options.
    pub header: Nullable<String>,
    /// Whether more than one option may be picked.
    pub multi_select: bool,
    /// The options offered.
    pub options: Vec<QuestionOption>,
}

/// A file the item points at, offered to the UI as a download chip.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct FileRef {
    /// Base name, shown on the chip.
    pub name: String,
    /// Absolute path on the server.
    pub path: String,
}

/// One hunk of a unified diff, as the CLI records it on an edit's result.
///
/// The shape is Claude Code's own: the top-level `toolUseResult.structuredPatch`
/// of the entry carrying an `Edit`/`MultiEdit`/`Write` result. `lines` holds the
/// raw diff rows, each already prefixed with `-`, `+` or a space, so a renderer
/// can colour them without re-deriving the diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct PatchHunk {
    /// First line of the hunk in the file as it was.
    pub old_start: u32,
    /// How many lines the hunk covered before the edit.
    pub old_lines: u32,
    /// First line of the hunk in the file as it now is.
    pub new_start: u32,
    /// How many lines the hunk covers after the edit.
    pub new_lines: u32,
    /// The diff rows, `-`/`+`/space prefixed.
    pub lines: Vec<String>,
}

/// A single normalized transcript entry, as the web transcript renderer expects it.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptItem {
    /// Who produced the item.
    pub role: Role,
    /// What kind of content it carries.
    pub kind: ItemKind,
    /// Body for text/thinking items; a one-line summary for tool items.
    pub text: String,
    /// Expanded content for tool items (command source, diff, full output).
    pub detail: Nullable<String>,
    /// Display name of the tool, for tool items.
    pub tool_name: Nullable<String>,
    /// Small caption: wall time, exit code, description, workdir…
    pub meta: Nullable<String>,
    /// Whether the item represents a failure.
    pub is_error: bool,
    /// Structured options for an `AskUserQuestion` that is still unanswered.
    pub question: Nullable<Vec<QuestionEntry>>,
    /// ISO-8601 timestamp from the transcript entry.
    pub timestamp: Nullable<String>,
    /// Whether the body hit one of the emergency limits.
    pub truncated: bool,
    /// Subagent id on the Task tool_use that spawned it (claude only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Background work: a `run_in_background` launch, or its completion notice.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
    /// Attached image paths on a user message (rendered as thumbnails).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<String>>,
    /// Downloadable files: non-image attachments, or files the agent produced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<FileRef>>,
    /// A tool call's own id, set on `tool_use` items (claude only).
    ///
    /// Paired with `resultOf` on the matching result. Both items stay in the
    /// stream — a renderer that merges them visually does so by id, because
    /// dropping one would move every later index and invalidate the paging
    /// window and signature the client already holds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    /// The `tool_use` id this result answers, set on `tool_result` items.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_of: Option<String>,
    /// Diff hunks the CLI recorded for an `Edit`/`MultiEdit`/`Write` result.
    ///
    /// Absent unless the entry carried exactly one result: the field sits at the
    /// top level of the transcript entry, not inside the result block, so with
    /// several results in one entry there is no way to say which it describes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_patch: Option<Vec<PatchHunk>>,
    /// The CLI's `task_id` a background-task notification is about.
    ///
    /// Set only on the notification item. Together with the three fields below
    /// it is what lets a client put "this finished" back beside the call that
    /// started it: the notification arrives as a `user` entry minutes later and
    /// anywhere in the file, so without the ids it is a floating sentence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// The `tool_use` id of the call that launched the task the notice is about.
    ///
    /// The join key to the launching item's `toolUseId`. Absent on a
    /// notification the CLI wrote without one — the "no completion record was
    /// found for the previous session" shape carries no `tool-use-id` — which
    /// is why it is optional rather than the identity of the item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_tool_use_id: Option<String>,
    /// The subagent id the notice is about, when the CLI named one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_agent_id: Option<String>,
    /// How the task ended: `completed`, `failed`, `stopped`, or whatever the
    /// CLI says next. Passed through rather than parsed into an enum.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_status: Option<String>,
    /// The CLI wrote this message itself, in the model's voice.
    ///
    /// `message.model` was `<synthetic>`: a rate-limit wall, a "Not logged in",
    /// an API error the CLI phrased as an assistant turn. It is the difference
    /// between the model saying something and the CLI apologising on its behalf,
    /// and nothing else in the entry distinguishes the two.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synthetic: Option<bool>,
    /// The model that wrote the item, when the transcript names one.
    ///
    /// Read per item rather than per session, because both CLIs downgrade
    /// silently — a rate-limit fallback, a `turn_context` that suddenly names
    /// something smaller — and the session's own `cliChoice` then describes only
    /// its last turn. Claude supplies `message.model` on every assistant entry;
    /// Codex supplies it once per turn in a `turn_context`, which is why an item
    /// takes the newest one above it.
    ///
    /// Set only on the agent's own items (text, thinking, tool calls) — never on
    /// a user turn, a tool result, or one of remote-hand's own system lines — and
    /// never on a `<synthetic>` message, where the CLI rather than a model wrote
    /// the words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// What a tool call or its result should look like on screen.
///
/// The TypeScript splits this into `ToolUseDisplay` and `ToolResultDisplay`; the
/// two differ only in which fields they happen to fill, so one struct carries
/// both here and `make_tool_item` takes it whole rather than as eight arguments.
#[derive(Debug, Clone, Default)]
pub struct ToolDisplay {
    /// Display name of the tool. A result inherits it from its call.
    pub tool_name: Option<String>,
    /// One-line summary or verdict.
    pub text: String,
    /// Small caption.
    pub meta: Option<String>,
    /// Expanded body.
    pub detail: Option<String>,
    /// Whether the tool reported a failure.
    pub is_error: bool,
}

/// Build a text/thinking item, applying the emergency clip.
pub fn make_item(role: Role, kind: ItemKind, raw_text: &str, timestamp: Option<&str>) -> TranscriptItem {
    let (text, truncated) = clip(raw_text.trim(), TEXT_LIMIT);
    TranscriptItem {
        role,
        kind,
        text,
        detail: Nullable(None),
        tool_name: Nullable(None),
        meta: Nullable(None),
        is_error: false,
        question: Nullable(None),
        timestamp: Nullable(timestamp.map(str::to_owned)),
        truncated,
        agent: None,
        background: None,
        images: None,
        files: None,
        tool_use_id: None,
        result_of: None,
        structured_patch: None,
        task_id: None,
        task_tool_use_id: None,
        task_agent_id: None,
        task_status: None,
        synthetic: None,
        model: None,
    }
}

/// Build a tool_use/tool_result item from a display description.
pub fn make_tool_item(role: Role, kind: ItemKind, display: ToolDisplay, timestamp: Option<&str>) -> TranscriptItem {
    let detail_clip = display.detail.as_deref().map(|d| clip(d, DETAIL_LIMIT));
    TranscriptItem {
        role,
        kind,
        text: strip_ansi(&display.text),
        detail: Nullable(detail_clip.as_ref().map(|(t, _)| t.clone())),
        tool_name: Nullable(display.tool_name),
        meta: Nullable(display.meta),
        is_error: display.is_error,
        question: Nullable(None),
        timestamp: Nullable(timestamp.map(str::to_owned)),
        truncated: detail_clip.map(|(_, t)| t).unwrap_or(false),
        agent: None,
        background: None,
        images: None,
        files: None,
        tool_use_id: None,
        result_of: None,
        structured_patch: None,
        task_id: None,
        task_tool_use_id: None,
        task_agent_id: None,
        task_status: None,
        synthetic: None,
        model: None,
    }
}

/// Extensions a browser draws in an `<img>`, and so the ones that may render as
/// a thumbnail rather than a download chip.
///
/// An allowlist rather than a "not pdf" test, because the failure mode is a
/// broken picture on the page and the next format the CLI learns to rasterise
/// (`Read` answers a PDF with page images) should default to the working
/// fallback, not to the broken one.
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "avif", "ico"];

/// Whether an extension, without the dot, is one a browser draws.
///
/// The three call sites keep their own way of finding the extension — a name's
/// tail, a `Path::extension`, an upload name — because those disagree at the
/// edges (a dotfile, a name with no dot) and each disagreement is pinned by a
/// test at the site. This is only the list.
pub fn is_image_ext(ext: &str) -> bool {
    IMAGE_EXTS.contains(&ext.to_ascii_lowercase().as_str())
}

/// Whether a name looks like an image: the tail after its last `.`, as
/// `name.rsplit('.')` finds it, so a name with no dot is not one.
pub fn is_image_name(name: &str) -> bool {
    match name.rsplit('.').next() {
        Some(ext) if name.contains('.') => is_image_ext(ext),
        _ => false,
    }
}

/// Strip CSI escape sequences, which leak in from CLI stdout captures.
pub fn strip_ansi(text: &str) -> String {
    static ANSI: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = ANSI.get_or_init(|| regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").expect("valid ANSI pattern"));
    re.replace_all(text, "").into_owned()
}

/// Strip ANSI and cut to `limit` characters, reporting whether anything was lost.
pub fn clip(text: &str, limit: usize) -> (String, bool) {
    let text = strip_ansi(text);
    if text.chars().count() <= limit {
        return (text, false);
    }
    (text.chars().take(limit).collect(), true)
}

/// First line of `text`, ellipsised past `limit` characters.
pub fn first_line_of(text: &str, limit: usize) -> String {
    let line = text.trim_start().split('\n').next().unwrap_or("");
    if line.chars().count() > limit {
        format!("{}…", line.chars().take(limit).collect::<String>())
    } else {
        line.to_string()
    }
}

/// What an attachment suffix on a user message decomposes into.
#[derive(Debug, Default)]
pub struct AttachmentSplit {
    /// The message with the suffix removed.
    pub text: String,
    /// Attached image paths.
    pub images: Vec<String>,
    /// Attached non-image files.
    pub files: Vec<FileRef>,
}

/// Strip the attachment suffix from a message and classify its paths.
///
/// The suffix is the contract between the injector and the transcript parser;
/// the older "image file(s)" wording still parses.
pub fn parse_attachment_suffix(raw_text: &str) -> AttachmentSplit {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"(?s)\n\n\[\d+ attached (?:image )?file\(s\) — view with the Read tool:\]\n((?:- .+\n?)+)$")
            .expect("valid attachment pattern")
    });
    let Some(caps) = re.captures(raw_text) else {
        return AttachmentSplit {
            text: raw_text.to_string(),
            ..Default::default()
        };
    };
    let whole = caps.get(0).expect("group 0 always present");
    let mut images = Vec::new();
    let mut files = Vec::new();
    for line in caps[1].split('\n') {
        let path = line.strip_prefix("- ").unwrap_or(line).trim();
        if path.is_empty() {
            continue;
        }
        if is_image_name(path) {
            images.push(path.to_string());
        } else {
            files.push(FileRef {
                name: Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                path: path.to_string(),
            });
        }
    }
    AttachmentSplit {
        text: raw_text[..whole.start()].to_string(),
        images,
        files,
    }
}

/// Read at most `max_bytes` from the tail of a file and return complete JSONL lines.
pub async fn read_tail_lines(path: &Path, max_bytes: u64) -> std::io::Result<Vec<String>> {
    let mut file = tokio::fs::File::open(path).await?;
    let size = file.metadata().await?.len();
    let start = size.saturating_sub(max_bytes);
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let mut buf = Vec::with_capacity((size - start) as usize);
    file.read_to_end(&mut buf).await?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.split('\n').collect();
    // The first line is almost certainly cut in half unless we read from 0.
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    Ok(lines.into_iter().filter(|l| !l.trim().is_empty()).map(str::to_owned).collect())
}

/// The first chunk [`BackwardLines`] reads, and the size every fixed-window
/// tail reader in this crate uses: the common case is one read of this much.
const BACKWARD_FIRST_CHUNK: usize = 64 * 1024;
/// The ceiling the chunk size doubles to once the first one was not enough.
const BACKWARD_MAX_CHUNK: usize = 1024 * 1024;

/// Complete JSONL lines from the end of a file backwards, newest first.
///
/// A fixed-size tail window answers "what happened last" only when the last
/// interesting entry is *small*. Real Claude transcripts are not: a single
/// `tool_result` line can be 1.35 MB and a pair of `queue-operation` entries can
/// be 88 KB, so any question answered by scanning back to the first entry that
/// settles it needs a window that grows until the answer is found rather than
/// one measured in bytes.
///
/// The read policy is what keeps that affordable. The first chunk is
/// [`BACKWARD_FIRST_CHUNK`] — the same single read a 64 KiB tail costs — and
/// only a caller whose answer is *not* in it pays for a second, doubling to
/// [`BACKWARD_MAX_CHUNK`]. `budget` caps the total read; once it is spent
/// [`BackwardLines::next_line`] answers `None` and [`BackwardLines::capped`]
/// says why, so a caller can tell "the file ended" from "I stopped looking".
pub struct BackwardLines {
    file: tokio::fs::File,
    /// Offset of the earliest byte read so far; everything below is unread.
    cursor: u64,
    /// The leading fragment of the last chunk, whose start lies below `cursor`.
    pending: Vec<u8>,
    /// Complete lines from the last chunk, newest first, not yet handed out.
    ready: Vec<String>,
    /// Total bytes read, against `budget`.
    scanned: u64,
    budget: u64,
    /// Set when `budget` ran out with the file not fully read.
    capped: bool,
    step: usize,
}

impl BackwardLines {
    /// Open `path` for backwards reading, reading at most `budget` bytes.
    pub async fn open(path: &Path, budget: u64) -> std::io::Result<Self> {
        let file = tokio::fs::File::open(path).await?;
        let size = file.metadata().await?.len();
        Ok(Self {
            file,
            cursor: size,
            pending: Vec::new(),
            ready: Vec::new(),
            scanned: 0,
            budget,
            capped: false,
            step: BACKWARD_FIRST_CHUNK,
        })
    }

    /// Whether the walk stopped at its byte budget rather than at the file's start.
    pub fn capped(&self) -> bool {
        self.capped
    }

    /// How many bytes have been read so far.
    pub fn scanned(&self) -> u64 {
        self.scanned
    }

    /// The next line back from the end, without its terminator.
    ///
    /// `None` means either the start of the file or the budget; ask
    /// [`BackwardLines::capped`] which. Blank lines are skipped, matching
    /// [`read_tail_lines`].
    pub async fn next_line(&mut self) -> std::io::Result<Option<String>> {
        loop {
            if let Some(line) = self.ready.pop() {
                return Ok(Some(line));
            }
            if self.cursor == 0 {
                // The last thing left is the file's first line, which is whole.
                let first = std::mem::take(&mut self.pending);
                if first.iter().any(|byte| !byte.is_ascii_whitespace()) {
                    return Ok(Some(String::from_utf8_lossy(&first).into_owned()));
                }
                return Ok(None);
            }
            if self.scanned >= self.budget {
                self.capped = true;
                return Ok(None);
            }
            self.read_chunk().await?;
        }
    }

    /// Read one more chunk below `cursor` and split it into lines.
    async fn read_chunk(&mut self) -> std::io::Result<()> {
        let step = self.step.min(usize::try_from(self.budget - self.scanned).unwrap_or(usize::MAX)).max(1);
        let start = self.cursor.saturating_sub(step as u64);
        let len = (self.cursor - start) as usize;
        self.file.seek(std::io::SeekFrom::Start(start)).await?;
        let mut buf = vec![0u8; len];
        self.file.read_exact(&mut buf).await?;
        self.cursor = start;
        self.scanned += len as u64;
        self.step = self.step.saturating_mul(2).min(BACKWARD_MAX_CHUNK);

        // The fragment held from last time completes the chunk's final line: it
        // ran up to a newline that was consumed when that chunk was split.
        buf.append(&mut self.pending);
        let mut parts = buf.split(|byte| *byte == b'\n');
        // The first piece starts below `start`, so it is only whole at offset 0.
        let parts_head = parts.next().unwrap_or_default().to_vec();
        // The rest are kept in file order: `next_line` pops from the end, the
        // newest of them. The fragment is held back — at offset 0 it is the
        // file's own first line and `next_line` yields it last of all.
        self.ready = parts
            .filter(|part| part.iter().any(|byte| !byte.is_ascii_whitespace()))
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect();
        self.pending = parts_head;
        Ok(())
    }
}

/// Read at most `max_bytes` from the head of a file and return complete JSONL lines.
pub async fn read_head_lines(path: &Path, max_bytes: usize) -> std::io::Result<Vec<String>> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut buf = vec![0u8; max_bytes];
    let mut read = 0usize;
    while read < max_bytes {
        let n = file.read(&mut buf[read..]).await?;
        if n == 0 {
            break;
        }
        read += n;
    }
    buf.truncate(read);
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.split('\n').collect();
    // Drop the final partial line unless we read the whole file.
    if read == max_bytes {
        lines.pop();
    }
    Ok(lines.into_iter().filter(|l| !l.trim().is_empty()).map(str::to_owned).collect())
}

/// A JSONL reader that decodes lossily, so bad bytes cost one line, not the file.
///
/// [`tokio::io::AsyncBufReadExt::lines`] answers `Err(InvalidData)` for a line
/// that is not valid UTF-8, and every caller propagates that with `?` — so a
/// single stray byte anywhere in a transcript abandons the rest of the walk and
/// blanks the session. Reading raw bytes and decoding each line with
/// [`String::from_utf8_lossy`] is what [`read_tail_lines`] and
/// [`read_head_lines`] already do; this is the streaming form of it, for files
/// far too large to hold in memory.
///
/// Only a genuine I/O error is still an error.
pub struct LossyLines<R> {
    reader: R,
    buffer: Vec<u8>,
}

impl LossyLines<tokio::io::BufReader<tokio::fs::File>> {
    /// Open `path` for line-by-line lossy reading.
    pub async fn open(path: &Path) -> std::io::Result<Self> {
        let file = tokio::fs::File::open(path).await?;
        Ok(Self {
            reader: tokio::io::BufReader::new(file),
            buffer: Vec::new(),
        })
    }
}

impl<R: tokio::io::AsyncBufRead + Unpin> LossyLines<R> {
    /// The next line without its terminator, or `None` at end of file.
    pub async fn next_line(&mut self) -> std::io::Result<Option<String>> {
        use tokio::io::AsyncBufReadExt;

        self.buffer.clear();
        if self.reader.read_until(b'\n', &mut self.buffer).await? == 0 {
            return Ok(None);
        }
        let mut end = self.buffer.len();
        if end > 0 && self.buffer[end - 1] == b'\n' {
            end -= 1;
        }
        // A file written on Windows keeps the carriage return out of the JSON.
        if end > 0 && self.buffer[end - 1] == b'\r' {
            end -= 1;
        }
        Ok(Some(String::from_utf8_lossy(&self.buffer[..end]).into_owned()))
    }
}

/// Parse one JSONL line into an object, or `None` for anything else.
///
/// Unparseable lines are skipped rather than failing the transcript: a
/// half-written tail line is normal while a CLI is mid-write.
pub fn try_parse(line: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    match serde_json::from_str::<serde_json::Value>(line) {
        Ok(serde_json::Value::Object(map)) => Some(map),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn backwards(bytes: &[u8], budget: u64) -> (Vec<String>, bool) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("f.jsonl");
        tokio::fs::write(&path, bytes).await.expect("write");
        let mut scan = BackwardLines::open(&path, budget).await.expect("open");
        let mut lines = Vec::new();
        while let Some(line) = scan.next_line().await.expect("read") {
            lines.push(line);
        }
        (lines, scan.capped())
    }

    /// The chunk boundary is the whole risk: a line the reader met in halves has
    /// to come back out in one piece, in the right place in the order.
    #[tokio::test]
    async fn backward_lines_reassemble_across_chunks_newest_first() {
        // Each line is far bigger than the first chunk, so every one of them is
        // split by the reader and rejoined from two reads.
        let body: String = (0..4).map(|n| format!("{}{n}\n", "x".repeat(200_000))).collect();
        let (lines, capped) = backwards(body.as_bytes(), u64::MAX).await;
        assert!(!capped, "the whole file fits in an unbounded budget");
        assert_eq!(lines.len(), 4);
        for (index, line) in lines.iter().enumerate() {
            assert_eq!(line.len(), 200_001, "line {index} came back whole");
            assert!(line.ends_with(&(3 - index).to_string()), "newest first");
        }
    }

    #[tokio::test]
    async fn backward_lines_handle_the_small_and_the_empty() {
        let (lines, _) = backwards(b"a\nb\nc", u64::MAX).await;
        assert_eq!(lines, vec!["c", "b", "a"], "an unterminated last line still counts");

        let (lines, _) = backwards(b"only\n", u64::MAX).await;
        assert_eq!(lines, vec!["only"]);

        let (lines, capped) = backwards(b"", u64::MAX).await;
        assert!(lines.is_empty() && !capped, "an empty file is not a capped one");

        let (lines, _) = backwards(b"a\n\n\nb\n", u64::MAX).await;
        assert_eq!(lines, vec!["b", "a"], "blank lines are skipped like the tail reader's");
    }

    /// The transcript splitter's extraction is the tail after the last dot.
    #[test]
    fn an_image_name_keeps_its_own_extraction() {
        assert!(is_image_name("photo.PNG"));
        assert!(is_image_name("dir/photo.png"));
        assert!(is_image_name(".png"), "the tail after the last dot is png");
        // No dot: not an image here. (The upload route extracts differently and
        // does count a name that is only an extension.)
        assert!(!is_image_name("png"));
        // A trailing separator, or `/.`, leaves a tail that is not an extension.
        assert!(!is_image_name("dir/"));
        assert!(!is_image_name("dir/."));
    }

    /// The budget stops the walk without pretending the file ended.
    #[tokio::test]
    async fn backward_lines_report_a_spent_budget() {
        let body: String = (0..40).map(|n| format!("{}{n}\n", "y".repeat(50_000))).collect();
        let (lines, capped) = backwards(body.as_bytes(), 200_000).await;
        assert!(capped, "the file is 2 MB and the budget was 200 KB");
        assert!(!lines.is_empty() && lines.len() < 40, "some of the tail, not all of the file");
        assert!(lines[0].ends_with("39"), "and it started from the end");
    }
}
