//! Claude JSONL -> normalized transcript items.
//!
//! Port of `parseClaudeTranscript` in the former TypeScript server's
//! `claude.ts`. The parse is deliberately tolerant: every line goes through
//! `serde_json::Value`, so an unknown block type is skipped rather than failing
//! the file, and a line that does not parse at all (a half-written tail while
//! the CLI is mid-write) is ignored exactly as the TypeScript does.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::tool_display::{display_claude_tool_result, display_claude_tool_use, extract_questions, shorten_tool_name};
use super::util::{
    clip, first_line_of, is_image_ext, make_item, make_tool_item, parse_attachment_suffix, try_parse, ItemKind, LossyLines, PatchHunk, Role,
    ToolDisplay, TranscriptItem, DETAIL_LIMIT,
};
use crate::openapi_ext::Nullable;

type Json = Map<String, Value>;

/// Content blocks of a `message`, normalizing the string shorthand.
pub fn content_blocks(message: Option<&Value>) -> Vec<Json> {
    let Some(Value::Object(message)) = message else { return vec![] };
    match message.get("content") {
        Some(Value::String(text)) => {
            let mut block = Map::new();
            block.insert("type".into(), Value::String("text".into()));
            block.insert("text".into(), Value::String(text.clone()));
            vec![block]
        }
        Some(Value::Array(blocks)) => blocks.iter().filter_map(Value::as_object).cloned().collect(),
        _ => vec![],
    }
}

/// The text a block carries, following the `text` / `content` shapes Claude emits.
pub fn block_text(block: &Json) -> String {
    if let Some(text) = block.get("text").and_then(Value::as_str) {
        return text.to_string();
    }
    match block.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .map(|part| part.as_object().and_then(|p| p.get("text")).and_then(Value::as_str).unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn tag_value(text: &str, name: &str) -> Option<String> {
    let re = regex::Regex::new(&format!(r"(?s)<{name}>(.*?)</{name}>")).ok()?;
    let caps = re.captures(text)?;
    let value = caps[1].trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// A background task (background bash, async agent) finished.
///
/// The payload is a `<task-notification>` envelope: routing metadata — the ids
/// that say *which* task, where its output went — wrapped around a `<summary>`
/// and the child's whole final report in `<result>`.
///
/// Both halves are kept, in different places. The ids become fields, because
/// stripping them (which is what this used to do) left the notice unable to
/// point at anything: it arrives as a plain `user` entry minutes after the call
/// it answers, so without `taskToolUseId` a client cannot put "this finished"
/// back beside the launch that started it, and the completion of every async
/// agent read as a sentence from nowhere. The report becomes `detail`, so the
/// row itself stays one line — these bodies run to twenty thousand characters
/// of Markdown tables — and a reader can open it.
fn task_notification_item(text: &str, timestamp: Option<&str>) -> TranscriptItem {
    static ROUTING: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static ENVELOPE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static BLANKS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let routing = ROUTING.get_or_init(|| {
        regex::Regex::new(r"(?s)<(usage|task-id|tool-use-id|prompt-id|agent-id|output-file|note|status|summary)>.*?</(?:usage|task-id|tool-use-id|prompt-id|agent-id|output-file|note|status|summary)>")
            .expect("valid routing pattern")
    });
    let envelope = ENVELOPE.get_or_init(|| regex::Regex::new(r"</?(?:task-notification|result|task|output)>").expect("valid envelope pattern"));
    let blanks = BLANKS.get_or_init(|| regex::Regex::new(r"\n{3,}").expect("valid blank-run pattern"));

    let status = tag_value(text, "status");
    let summary = tag_value(text, "summary");
    let result = tag_value(text, "result");
    let headline = summary.unwrap_or_else(|| "background task finished".to_string());
    let label = match status.as_deref() {
        Some(status) if status != "completed" => format!("{headline} · {status}"),
        _ => headline,
    };
    let body = result.unwrap_or_else(|| {
        // Unknown shape: keep everything except the routing metadata rather than drop it.
        let stripped = routing.replace_all(text, "");
        let stripped = envelope.replace_all(&stripped, "");
        blanks.replace_all(&stripped, "\n\n").trim().to_string()
    });
    let mut item = make_tool_item(
        Role::System,
        ItemKind::Text,
        ToolDisplay {
            text: format!("⏱ {label}"),
            detail: (!body.is_empty()).then_some(body),
            ..ToolDisplay::default()
        },
        timestamp,
    );
    item.background = Some(true);
    item.task_id = tag_value(text, "task-id");
    item.task_tool_use_id = tag_value(text, "tool-use-id");
    item.task_agent_id = tag_value(text, "agent-id");
    item.task_status = status;
    item
}

/// A report another agent handed back to this session.
///
/// It reaches the CLI through the same queue a person's mid-turn message does —
/// a `queued_command` attachment with `commandMode: "prompt"` — so the mode
/// alone cannot tell them apart, and every subagent hand-back rendered as a
/// bubble the user had typed. What does tell them apart is the attachment's
/// `origin.kind: "peer"` (it also carries `isMeta: true`) and the
/// `<agent-message from="…">` envelope around the text. It becomes a quiet
/// system row, like a task notification: one line naming the sender, the
/// report itself as `detail`, which the harness indents by two spaces under a
/// fixed preamble that ends in "The report follows:".
fn peer_message_item(text: &str, from: Option<&str>, timestamp: Option<&str>) -> TranscriptItem {
    static FROM: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static ENVELOPE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let from_attr = FROM.get_or_init(|| regex::Regex::new(r#"^\s*<agent-message\s+from="([^"]+)""#).expect("valid from pattern"));
    let envelope =
        ENVELOPE.get_or_init(|| regex::Regex::new(r"(?s)^\s*<agent-message[^>]*>\n?|\n?</agent-message>\s*$").expect("valid envelope pattern"));

    let sender = from
        .map(str::to_string)
        .or_else(|| from_attr.captures(text).map(|caps| caps[1].to_string()));
    let inner = envelope.replace_all(text, "").to_string();
    let hand_back = inner.trim_start().starts_with("[Subagent hand-back]");
    let body = match inner.find("The report follows:") {
        Some(at) if hand_back => {
            let after = &inner[at + "The report follows:".len()..];
            after
                .trim_start_matches('\n')
                .lines()
                .map(|line| line.strip_prefix("  ").unwrap_or(line))
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string()
        }
        _ => inner.trim().to_string(),
    };
    let headline = if hand_back { "subagent report" } else { "message from another agent" };
    let label = match sender.as_deref() {
        Some(sender) => format!("↩ {headline} · {sender}"),
        None => format!("↩ {headline}"),
    };
    let mut item = make_tool_item(
        Role::System,
        ItemKind::Text,
        ToolDisplay {
            text: label,
            detail: (!body.is_empty()).then_some(body),
            ..ToolDisplay::default()
        },
        timestamp,
    );
    item.background = Some(true);
    item.task_agent_id = sender;
    item
}

/// The CLI dropped the conversation and replaced it with a summary.
///
/// The boundary entry is the only thing in the file that says a compact
/// happened — the summary that follows it is an ordinary-looking `user` entry —
/// so it becomes an item of its own and [`attach_compact_summary`] hangs the
/// summary off it rather than letting it render as a turn the user never typed.
fn compact_boundary_item(entry: &Json, timestamp: Option<&str>) -> TranscriptItem {
    let label = entry
        .get("content")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|content| !content.is_empty())
        .unwrap_or("Conversation compacted");
    let meta = entry.get("compactMetadata").and_then(Value::as_object).map(|metadata| {
        let trigger = metadata.get("trigger").and_then(Value::as_str).unwrap_or("compacted");
        let tokens = |key: &str| metadata.get(key).and_then(Value::as_u64);
        match (tokens("preTokens"), tokens("postTokens")) {
            (Some(before), Some(after)) => format!("{trigger} · {before} → {after} tokens"),
            _ => trigger.to_string(),
        }
    });
    make_tool_item(
        Role::System,
        ItemKind::Compact,
        ToolDisplay {
            text: label.to_string(),
            meta,
            ..ToolDisplay::default()
        },
        timestamp,
    )
}

/// The turn moved to another model mid-flight.
///
/// Claude Code writes it as a content block of its own —
/// `{"type":"fallback","from":{"model":…},"to":{"model":…}}` — carrying no text,
/// which is why the block whitelist dropped it: in `9e3ca634` one such block at
/// line 4508 is the entire record of the next hundred and five answers coming
/// from `claude-opus-4-8` instead of the model that was asked for. A quiet
/// system line is the smallest thing that makes it visible.
fn model_fallback_item(block: &Json, timestamp: Option<&str>) -> TranscriptItem {
    let model = |key: &str| {
        block
            .get(key)
            .and_then(Value::as_object)
            .and_then(|side| side.get("model"))
            .and_then(Value::as_str)
    };
    let text = match (model("from"), model("to")) {
        (Some(from), Some(to)) => format!("model fallback: {from} → {to}"),
        (None, Some(to)) => format!("model fallback: → {to}"),
        (Some(from), None) => format!("model fallback: {from} → ?"),
        (None, None) => "model fallback".to_string(),
    };
    make_item(Role::System, ItemKind::Text, &text, timestamp)
}

/// A timestamp that cannot be earlier than the item before it.
///
/// The CLI stamps a `compact_boundary` with the moment it *decided* to compact,
/// not the moment it wrote the entry, and the two are minutes apart: in
/// `76f842ae` the boundary is 135 seconds behind the entry directly above it,
/// and `9e3ca634` has seven such inversions. Every renderer that groups items by
/// time — bubbles, day separators, "n minutes later" captions — reads that as the
/// conversation running backwards. Clamping costs the boundary nothing, since it
/// marks a seam rather than an event with a duration, and keeps the stream
/// monotonic for everyone downstream.
///
/// Only compares timestamps written the same way (UTC, `Z`-suffixed), which is
/// the only shape Claude Code writes; anything else is left exactly as it came.
fn clamped_timestamp(items: &[TranscriptItem], timestamp: Option<&str>) -> Option<String> {
    let utc = |value: &str| value.ends_with('Z');
    let previous = items.iter().rev().find_map(|item| item.timestamp.as_ref()).filter(|value| utc(value));
    match (timestamp, previous) {
        (Some(now), Some(before)) if utc(now) && now < before.as_str() => Some(before.clone()),
        (now, _) => now.map(str::to_owned),
    }
}

/// Fold a compact summary into the boundary entry it belongs to.
///
/// The two are separate entries and the summary carries no marker in its own
/// content, so on its own it is indistinguishable from a message the user
/// typed — which is how a compact used to arrive as an unexplained wall of
/// English. Falls back to a boundary of its own when the real one is not in
/// this window, because dropping it would lose the summary entirely.
fn attach_compact_summary(items: &mut [TranscriptItem], summary: &str, timestamp: Option<&str>) -> Option<TranscriptItem> {
    let (detail, truncated) = clip(summary.trim(), DETAIL_LIMIT);
    if let Some(last) = items.last_mut() {
        if last.kind == ItemKind::Compact && last.detail.0.is_none() {
            last.detail = Nullable(Some(detail));
            last.truncated = truncated;
            return None;
        }
    }
    Some(make_tool_item(
        Role::System,
        ItemKind::Compact,
        ToolDisplay {
            text: "Conversation compacted".to_string(),
            detail: Some(summary.to_string()),
            ..ToolDisplay::default()
        },
        timestamp,
    ))
}

/// What one text block of a `user` entry turned into.
enum UserLine {
    /// A finished item, ready to append. Boxed because it dwarfs the other
    /// variant, and every user text block goes through this type.
    Item(Box<TranscriptItem>),
    /// `<local-command-stdout>`: the output of the command entry before it,
    /// which the CLI writes as a separate entry of its own.
    CommandOutput(String),
}

/// Hang a command's stdout off the command it answers.
///
/// Two items would put the output on a row that never says which command
/// printed it; folded into the command's own chip it reads as one event.
fn attach_command_output(items: &mut [TranscriptItem], body: &str, timestamp: Option<&str>) -> Option<TranscriptItem> {
    let (detail, truncated) = clip(body.trim(), DETAIL_LIMIT);
    if let Some(last) = items.last_mut() {
        if last.kind == ItemKind::Command && last.detail.0.is_none() {
            last.detail = Nullable(Some(detail));
            last.truncated = truncated;
            return None;
        }
    }
    // Output whose command fell outside this window still has to show up.
    Some(make_item(Role::System, ItemKind::Text, body, timestamp))
}

/// The text of a message that was sitting in the CLI's input queue.
///
/// `prompt` is either a bare string or the block list of a user message, and a
/// block list may carry inline base64 images beside the text. Only the text
/// blocks are joined: the images are megabytes of data URI that no transcript
/// row could show, and the CLI already writes an `[Image #N]` marker into the
/// text block for each one, so nothing the reader can see is lost.
fn queued_prompt_text(attachment: &Json) -> Option<String> {
    match attachment.get("prompt")? {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => Some(
            blocks
                .iter()
                .filter_map(Value::as_object)
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .map(block_text)
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        _ => None,
    }
}

/// A message that reached the CLI while a turn was already in flight.
///
/// The CLI absorbs such a message into the running turn as a steer, and never
/// writes a `user` entry for it: the queue records `enqueue` then `remove`, and
/// this `attachment` is the only place the text survives. Skipping it — which
/// is what the blanket "attachments are chatter" rule did — deleted the message
/// from the rendered transcript while the turn's answer still reflected it, so
/// the user watched their own words vanish. It renders where it sits, which is
/// mid-turn, because mid-turn is where it landed.
///
/// A message that instead *waited* for the turn to finish is `dequeue`d and
/// arrives as a real `user` entry, and no attachment is written for it — so
/// materializing every queued command cannot double-render one. The CLI feeds
/// its own task-notifications through the same queue and does write attachments
/// for them; those carry `commandMode: "task-notification"` and are the one
/// thing here that must stay invisible.
fn queued_command_line(entry: &Json, timestamp: Option<&str>) -> Option<UserLine> {
    let attachment = entry.get("attachment").and_then(Value::as_object)?;
    if attachment.get("type").and_then(Value::as_str) != Some("queued_command") {
        return None;
    }
    if attachment.get("commandMode").and_then(Value::as_str) == Some("task-notification") {
        return None;
    }
    let text = queued_prompt_text(attachment)?;
    // Another agent's report rides the same queue with the same mode; its
    // `origin` is what says it is not the user. See `peer_message_item`.
    let peer = attachment
        .get("origin")
        .and_then(Value::as_object)
        .filter(|origin| origin.get("kind").and_then(Value::as_str) == Some("peer"));
    if peer.is_some() || text.trim_start().starts_with("<agent-message") {
        let from = peer.and_then(|origin| origin.get("from")).and_then(Value::as_str);
        return Some(UserLine::Item(Box::new(peer_message_item(&text, from, timestamp))));
    }
    // The queue also carries slash commands and, defensively, notification text
    // under some other mode; `claude_user_item` already classifies all of those.
    claude_user_item(&text, timestamp)
}

/// The words of a person's message the CLI folded into a running turn, when
/// `entry` is the record of one.
///
/// The same `queued_command` attachment [`queued_command_line`] renders as the
/// user speaking mid-turn, minus everything else that rides the CLI's input
/// queue without being somebody's message: its own task notifications, another
/// agent's hand-back, and meta prompts. A sidechain entry is a subagent's, not
/// the session's.
///
/// What the resident driver reads to learn that a steered message has been
/// *taken in* rather than left waiting for a turn of its own: claude writes
/// nothing on stdout when it folds one, and this entry is the only record of
/// it — see `inject::resident::Driver::absorb`.
pub(crate) fn absorbed_prompt(entry: &Value) -> Option<String> {
    let entry = entry.as_object()?;
    if entry.get("type").and_then(Value::as_str) != Some("attachment") || entry.get("isSidechain") == Some(&Value::Bool(true)) {
        return None;
    }
    let attachment = entry.get("attachment").and_then(Value::as_object)?;
    if attachment.get("type").and_then(Value::as_str) != Some("queued_command")
        || attachment.get("commandMode").and_then(Value::as_str) == Some("task-notification")
        || attachment.get("isMeta") == Some(&Value::Bool(true))
    {
        return None;
    }
    let peer = attachment
        .get("origin")
        .and_then(Value::as_object)
        .and_then(|origin| origin.get("kind"))
        .and_then(Value::as_str)
        == Some("peer");
    let text = queued_prompt_text(attachment)?;
    (!peer && !text.trim_start().starts_with("<agent-message")).then_some(text)
}

/// User entries carry several kinds of machine chatter (slash-command metadata,
/// task notifications, interrupt markers, attached-image suffixes). Turn them
/// into quiet system lines / thumbnails instead of raw XML walls.
fn claude_user_item(raw_text: &str, timestamp: Option<&str>) -> Option<UserLine> {
    static COMMAND: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static COMMAND_STDOUT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static INTERRUPT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let command = COMMAND.get_or_init(|| {
        regex::Regex::new(
            r"(?s)^<command-name>(.*?)</command-name>\s*(?:<command-message>.*?</command-message>)?\s*(?:<command-args>(.*?)</command-args>)?\s*$",
        )
        .expect("valid command pattern")
    });
    let command_stdout = COMMAND_STDOUT
        .get_or_init(|| regex::Regex::new(r"(?s)^<local-command-stdout>(.*?)</local-command-stdout>\s*$").expect("valid stdout pattern"));
    let interrupt = INTERRUPT.get_or_init(|| regex::Regex::new(r"^\[Request interrupted by user[^\]]*\]$").expect("valid interrupt pattern"));

    let text = raw_text.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(caps) = command.captures(text) {
        return Some(UserLine::Item(Box::new(make_tool_item(
            Role::System,
            ItemKind::Command,
            ToolDisplay {
                tool_name: Some(caps[1].trim().to_string()),
                text: caps.get(2).map(|args| args.as_str().trim()).unwrap_or("").to_string(),
                ..ToolDisplay::default()
            },
            timestamp,
        ))));
    }
    if let Some(caps) = command_stdout.captures(text) {
        let body = caps[1].trim();
        return (!body.is_empty()).then(|| UserLine::CommandOutput(body.to_string()));
    }
    if interrupt.is_match(text) {
        let inner = &text[1..text.len() - 1];
        return Some(UserLine::Item(Box::new(make_item(Role::System, ItemKind::Text, inner, timestamp))));
    }
    if text.starts_with("<agent-message") {
        return Some(UserLine::Item(Box::new(peer_message_item(text, None, timestamp))));
    }
    if text.starts_with("<task-notification>") {
        return Some(UserLine::Item(Box::new(task_notification_item(text, timestamp))));
    }
    let split = parse_attachment_suffix(raw_text);
    if !split.images.is_empty() || !split.files.is_empty() {
        let mut item = make_item(Role::User, ItemKind::Text, &split.text, timestamp);
        if !split.images.is_empty() {
            item.images = Some(split.images);
        }
        if !split.files.is_empty() {
            item.files = Some(split.files);
        }
        return Some(UserLine::Item(Box::new(item)));
    }
    Some(UserLine::Item(Box::new(make_item(Role::User, ItemKind::Text, raw_text, timestamp))))
}

/// The diff hunks a transcript entry recorded alongside its tool result.
///
/// `toolUseResult` is a sibling of `message`, not part of the result block, so it
/// describes the entry as a whole — which is only unambiguous when the entry
/// carries exactly one result. The caller enforces that; this only reads.
///
/// A malformed hunk is dropped rather than failing the entry, and a `Write` that
/// creates a file records an empty patch, which is `None` here: there is no diff
/// to show, and an empty list would only make every renderer test for it.
fn structured_patch(entry: &Json) -> Option<Vec<PatchHunk>> {
    let rows = entry
        .get("toolUseResult")
        .and_then(Value::as_object)?
        .get("structuredPatch")
        .and_then(Value::as_array)?;
    let hunks: Vec<PatchHunk> = rows
        .iter()
        .filter_map(|row| serde_json::from_value::<PatchHunk>(row.clone()).ok())
        .collect();
    (!hunks.is_empty()).then_some(hunks)
}

/// Metadata a subagent transcript writes next to the session file.
#[derive(Debug, Clone)]
pub struct SubagentMeta {
    /// The subagent's id.
    pub agent_id: String,
    /// The `tool_use` id of the Task call that spawned it.
    pub tool_use_id: Option<String>,
    /// What the Task call asked for.
    pub description: Option<String>,
    /// Which agent type answered.
    pub agent_type: Option<String>,
    /// How deep the spawn chain is: `1` for a child of the session itself, `2`
    /// for a child of a child.
    ///
    /// The field that explains an unattachable `toolUseId`: at depth 2 and below
    /// the id names a `tool_use` in the *parent agent's* transcript, not in the
    /// session's, so looking for it in the main file finds nothing and the whole
    /// subtree goes missing. One real session (`b16232e3`) had eleven agents and
    /// 2.5 MB of work invisible for exactly that reason.
    pub spawn_depth: Option<u32>,
    /// The agent that spawned it, for anything deeper than the session.
    pub parent_agent_id: Option<String>,
    /// Which model the child ran on, when the meta says.
    pub model: Option<String>,
}

/// Subagent transcripts live next to the session file: `<dir>/<sid>/subagents/agent-<id>.jsonl`.
pub fn subagent_dir(session_file: &Path) -> PathBuf {
    let stem = session_file.to_string_lossy();
    let base = stem.strip_suffix(".jsonl").unwrap_or(&stem).to_string();
    PathBuf::from(base).join("subagents")
}

/// One subagent's transcript file.
pub fn subagent_path(session_file: &Path, agent_id: &str) -> PathBuf {
    subagent_dir(session_file).join(format!("agent-{agent_id}.jsonl"))
}

/// The transcript of one agent, wherever under `subagents/` it was written.
///
/// Normally that is the directory itself, but the CLI also files an agent under
/// a subdirectory of its own — `subagents/workflows/agent-<id>.jsonl` for one
/// spawned by a workflow — and a reader that only looks at the top level answers
/// "no such agent" for work that is sitting right there, silently skipping a
/// whole class of children. One level of subdirectory is what the CLI uses and what this searches.
pub async fn find_subagent_file(session_file: &Path, agent_id: &str) -> Option<PathBuf> {
    let name = format!("agent-{agent_id}.jsonl");
    let dir = subagent_dir(session_file);
    let direct = dir.join(&name);
    if tokio::fs::metadata(&direct).await.is_ok() {
        return Some(direct);
    }
    let mut entries = tokio::fs::read_dir(&dir).await.ok()?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.file_type().await.map(|kind| kind.is_dir()).unwrap_or(false) {
            let nested = entry.path().join(&name);
            if tokio::fs::metadata(&nested).await.is_ok() {
                return Some(nested);
            }
        }
    }
    None
}

/// Read the agent metas in one directory.
///
/// Split out from [`list_subagents`] because a *subagent's* children are metas
/// in the same directory as the subagent itself — they name it in
/// `parentAgentId` rather than living under a directory of their own — so
/// tagging a subagent's transcript reads the same folder from a different file.
pub async fn list_agent_metas(dir: &Path) -> Vec<SubagentMeta> {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return vec![];
    };
    let mut metas = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(agent_id) = name.strip_prefix("agent-").and_then(|n| n.strip_suffix(".meta.json")) else {
            continue;
        };
        // Half-written meta: the next parse picks it up.
        let Ok(raw) = tokio::fs::read_to_string(entry.path()).await else {
            continue;
        };
        let Ok(Value::Object(meta)) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let string = |key: &str| meta.get(key).and_then(Value::as_str).map(str::to_owned);
        metas.push(SubagentMeta {
            agent_id: agent_id.to_string(),
            tool_use_id: string("toolUseId"),
            description: string("description"),
            agent_type: string("agentType"),
            spawn_depth: meta.get("spawnDepth").and_then(Value::as_u64).map(|depth| depth as u32),
            parent_agent_id: string("parentAgentId"),
            model: string("model"),
        });
    }
    metas
}

/// Read the subagent metas that sit beside a session file.
pub async fn list_subagents(session_file: &Path) -> Vec<SubagentMeta> {
    list_agent_metas(&subagent_dir(session_file)).await
}

/// Every agent that ever wrote under a session, however deep and however filed.
///
/// [`list_subagents`] answers "which metas are here"; this answers "what work
/// exists", which is a different question in three ways that each cost a real
/// session its visibility:
///
/// * an agent with a transcript and **no meta at all** (a Skill spawns these) is
///   listed, because 800 KB of work with no sidecar is still work;
/// * an agent under a **subdirectory** of `subagents/` is listed, for the same
///   reason [`find_subagent_file`] looks there;
/// * every agent is listed **whatever its depth**, since a depth-2 child cannot
///   be reached from the main transcript by its `toolUseId` and would otherwise
///   have no entry point anywhere in the UI.
///
/// Cheap by construction — one `read_dir`, one `stat` and one small meta read
/// per agent, no transcript parsing — because it is the index a client opens
/// before deciding which child is worth loading.
pub async fn list_session_agents(session_file: &Path) -> Vec<AgentEntry> {
    let dir = subagent_dir(session_file);
    let mut found: HashMap<String, AgentEntry> = HashMap::new();
    let mut dirs = vec![dir.clone()];
    // One level down: `subagents/workflows/agent-*.jsonl` and its like.
    if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.file_type().await.map(|kind| kind.is_dir()).unwrap_or(false) {
                dirs.push(entry.path());
            }
        }
    }
    for dir in dirs {
        for meta in list_agent_metas(&dir).await {
            let entry = found.entry(meta.agent_id.clone()).or_insert_with(|| AgentEntry::new(&meta.agent_id));
            entry.absorb(meta);
        }
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else { continue };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(agent_id) = name.strip_prefix("agent-").and_then(|n| n.strip_suffix(".jsonl")) else {
                continue;
            };
            let stat = entry.metadata().await.ok();
            let found = found.entry(agent_id.to_string()).or_insert_with(|| AgentEntry::new(agent_id));
            found.has_transcript = true;
            found.bytes = stat.as_ref().map(std::fs::Metadata::len).unwrap_or(0);
            found.updated_at = stat.and_then(|stat| stat.modified().ok()).map(crate::sessions::iso8601);
        }
    }
    let mut agents: Vec<AgentEntry> = found.into_values().collect();
    // Newest work first, and stable for agents whose mtime is unreadable.
    agents.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| a.agent_id.cmp(&b.agent_id)));
    agents
}

/// One agent found on disk under a session, meta or no meta.
#[derive(Debug, Clone, Default)]
pub struct AgentEntry {
    /// The agent's id, which is what `/agents/{agentId}` takes.
    pub agent_id: String,
    /// Which agent type answered, from the meta.
    pub agent_type: Option<String>,
    /// What the spawning call asked for, from the meta.
    pub description: Option<String>,
    /// The `tool_use` id the meta recorded, if any.
    pub tool_use_id: Option<String>,
    /// The agent that spawned it, for anything deeper than the session.
    pub parent_agent_id: Option<String>,
    /// How deep the spawn chain is; `1` is a direct child of the session.
    pub spawn_depth: Option<u32>,
    /// Which model it ran on.
    pub model: Option<String>,
    /// Whether a transcript file exists beside the meta.
    pub has_transcript: bool,
    /// Size of that transcript, in bytes.
    pub bytes: u64,
    /// When it was last written, by the filesystem's clock.
    pub updated_at: Option<String>,
}

impl AgentEntry {
    fn new(agent_id: &str) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            ..Self::default()
        }
    }

    fn absorb(&mut self, meta: SubagentMeta) {
        self.agent_type = meta.agent_type;
        self.description = meta.description;
        self.tool_use_id = meta.tool_use_id;
        self.parent_agent_id = meta.parent_agent_id;
        self.spawn_depth = meta.spawn_depth;
        self.model = meta.model;
    }
}

/// The file to show as a thumbnail for a result that came back as an image.
///
/// The transcript's own image blocks are inline base64 — a quarter of a megabyte
/// each, with no path anywhere in the entry — so they can be counted but not
/// sent. What *can* be shown is the file the call named: every image result on
/// record (166 of them, all `Read`) answers a call carrying the `file_path` of
/// the image itself, and `/api/fs/download` serves that file already.
///
/// One entry per file rather than per block, because a `Read` of a multi-page
/// document answers with many images from one path and a list of identical
/// thumbnails is not a better answer than one.
///
/// `None` whenever the path cannot be offered — no call in this window, no path
/// on it, a path outside the home directory (which the download route refuses),
/// or a file no `<img>` can render — and the display's own `[N image]` note
/// stays as the fallback.
fn result_images(
    content: Option<&Value>,
    tool_use_id: Option<&str>,
    tool_paths: &HashMap<String, String>,
    home: &Path,
    cwd: Option<&str>,
) -> Option<Vec<String>> {
    if super::tool_display::result_image_count(content) == 0 {
        return None;
    }
    let named = tool_paths.get(tool_use_id?)?;
    if !is_renderable_image(Path::new(named)) {
        return None;
    }
    let resolved = super::mentions::resolve_within_home(home, named, cwd.map(Path::new))?;
    Some(vec![resolved.to_string_lossy().into_owned()])
}

/// Whether a browser would draw this file in an `<img>`.
///
/// The count of image blocks in a result is not enough on its own to name the
/// call's file as a thumbnail. `Read` answers with image blocks for a *PDF*
/// too — it rasterises the pages — and the path on that call is the `.pdf`,
/// which an `<img>` renders as a broken-image icon. That is strictly worse than
/// the `[N images]` line it replaced, so the extension has to agree before a
/// path is offered.
///
/// The extraction is `Path::extension`, which has no extension for a dotfile
/// (`.png`); the allowlist is shared with the other image sites.
pub(super) fn is_renderable_image(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()).is_some_and(is_image_ext)
}

/// The metas describing the children of the transcript being parsed.
///
/// Two different questions behind one name: for a session file it is "which
/// agents did this session spawn", answered by the metas under its own
/// `subagents/` directory; for a subagent's file it is "which agents did *this
/// agent* spawn", answered by the metas beside it that name it as their parent.
async fn agent_metas_for(file_path: &Path, sidechain: bool) -> Vec<SubagentMeta> {
    if !sidechain {
        return list_subagents(file_path).await;
    }
    let name = file_path.file_name().map(|name| name.to_string_lossy().into_owned());
    let Some(me) = name
        .as_deref()
        .and_then(|name| name.strip_prefix("agent-"))
        .and_then(|name| name.strip_suffix(".jsonl"))
    else {
        return vec![];
    };
    let Some(dir) = file_path.parent() else { return vec![] };
    list_agent_metas(dir)
        .await
        .into_iter()
        .filter(|meta| meta.parent_agent_id.as_deref() == Some(me))
        .collect()
}

/// Normalize a Claude session transcript into renderable items.
///
/// `sidechain` controls whether entries marked `isSidechain` are kept (they are
/// the subagent's own transcript, browsed separately).
///
/// `paths` is threaded through for the mentioned-file annotation, which resolves
/// relative paths against the session's own `cwd` and refuses anything outside
/// the home directory.
pub async fn parse_claude_transcript(paths: &super::Paths, file_path: &Path, sidechain: bool) -> std::io::Result<Vec<TranscriptItem>> {
    let mut lines = LossyLines::open(file_path).await?;

    let mut items: Vec<TranscriptItem> = Vec::new();
    let mut tool_names: HashMap<String, String> = HashMap::new();
    // tool_use id -> index of its item, so subagent metas can tag their spawning Task call.
    let mut tool_items: HashMap<String, usize> = HashMap::new();
    // The same, for the result that answers it: a subagent's verdict is a
    // `tool_result` like any other and would otherwise fold into a step group.
    let mut result_items: HashMap<String, usize> = HashMap::new();
    // Question items stay interactive until their tool_result arrives.
    let mut pending_questions: HashMap<String, usize> = HashMap::new();
    // tool_use id -> the file it named, so a result that came back as an image
    // can offer that file as a thumbnail. See `result_images`.
    let mut tool_paths: HashMap<String, String> = HashMap::new();
    // Session cwd resolves relative file mentions in assistant output.
    let mut cwd: Option<String> = None;

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let Some(entry) = try_parse(&line) else { continue };
        if entry.get("isSidechain") == Some(&Value::Bool(true)) && !sidechain {
            continue;
        }
        if cwd.is_none() {
            cwd = entry.get("cwd").and_then(Value::as_str).map(str::to_owned);
        }
        let timestamp = entry.get("timestamp").and_then(Value::as_str).map(str::to_owned);
        let timestamp = timestamp.as_deref();
        let entry_type = entry.get("type").and_then(Value::as_str).unwrap_or("");

        if entry_type == "system" {
            // The one `system` subtype with anything to show; the rest is
            // routing chatter the CLI keeps for itself.
            if entry.get("subtype").and_then(Value::as_str) == Some("compact_boundary") {
                let at = clamped_timestamp(&items, timestamp);
                items.push(compact_boundary_item(&entry, at.as_deref()));
            }
        } else if entry_type == "assistant" {
            // `<synthetic>` is the CLI writing in the model's voice: a rate-limit
            // wall, a "Not logged in", an API error. Marked rather than dropped,
            // because a reader who cannot tell those from the model's own words
            // reads a session limit as the agent giving up.
            let named = entry
                .get("message")
                .and_then(Value::as_object)
                .and_then(|message| message.get("model"))
                .and_then(Value::as_str);
            let synthetic = named == Some("<synthetic>");
            // Which model actually answered, per entry rather than per session:
            // a rate limit or a fallback moves the conversation to another model
            // mid-way and only the entries themselves say where it went. Nothing
            // to attribute on a `<synthetic>` line, since no model wrote it.
            let model = named.filter(|_| !synthetic).filter(|model| !model.trim().is_empty());
            let mark = |mut item: TranscriptItem| {
                if synthetic {
                    item.synthetic = Some(true);
                }
                // Only the agent's own items: a `tool_result` is the user turn
                // that carries it, and a system line is remote-hand's own voice.
                if item.role == Role::Assistant {
                    item.model = model.map(str::to_owned);
                }
                item
            };
            for block in content_blocks(entry.get("message")) {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let text = block_text(&block);
                        if !text.trim().is_empty() {
                            items.push(mark(make_item(Role::Assistant, ItemKind::Text, &text, timestamp)));
                        }
                    }
                    Some("thinking") => {
                        if let Some(thinking) = block.get("thinking").and_then(Value::as_str) {
                            if !thinking.trim().is_empty() {
                                items.push(mark(make_item(Role::Assistant, ItemKind::Thinking, thinking, timestamp)));
                            }
                        }
                    }
                    // The CLI silently moved the turn to another model. Its own
                    // block type, with no text in it, so the block whitelist used
                    // to drop it — and a hundred answers came back from a
                    // different model than the one that was asked, with nothing
                    // anywhere saying so.
                    Some("fallback") => items.push(model_fallback_item(&block, timestamp)),
                    Some("tool_use") => {
                        let name = block.get("name").and_then(Value::as_str).unwrap_or("tool").to_string();
                        let id = block.get("id").and_then(Value::as_str).map(str::to_owned);
                        if let Some(id) = &id {
                            tool_names.insert(id.clone(), shorten_tool_name(&name));
                        }
                        let input = block.get("input").and_then(Value::as_object).cloned().unwrap_or_default();
                        if let Some(id) = &id {
                            if let Some(named) = ["file_path", "path"]
                                .iter()
                                .find_map(|key| input.get(*key).and_then(Value::as_str))
                                .filter(|path| !path.trim().is_empty())
                            {
                                tool_paths.insert(id.clone(), named.to_string());
                            }
                        }
                        let display = display_claude_tool_use(&name, &input);
                        let mut item = make_tool_item(Role::Assistant, ItemKind::ToolUse, display, timestamp);
                        // Launched in the background: keep it visible instead of folding it
                        // into a step group, since its result lands much later.
                        if input.get("run_in_background") == Some(&Value::Bool(true)) {
                            item.background = Some(true);
                        }
                        if name == "AskUserQuestion" {
                            item.question = Nullable(extract_questions(&input));
                            if let Some(id) = &id {
                                pending_questions.insert(id.clone(), items.len());
                            }
                        }
                        if let Some(id) = &id {
                            tool_items.insert(id.clone(), items.len());
                        }
                        item.tool_use_id = id;
                        items.push(mark(item));
                    }
                    _ => {}
                }
            }
        } else if entry_type == "user" && entry.get("isMeta") != Some(&Value::Bool(true)) {
            // Written as a user turn, but nobody typed it: it is the compact's
            // own output, and belongs to the boundary entry just before it.
            if entry.get("isCompactSummary") == Some(&Value::Bool(true)) {
                let summary = content_blocks(entry.get("message")).iter().map(block_text).collect::<Vec<_>>().join("\n");
                if !summary.trim().is_empty() {
                    if let Some(item) = attach_compact_summary(&mut items, &summary, timestamp) {
                        items.push(item);
                    }
                }
                continue;
            }
            let blocks = content_blocks(entry.get("message"));
            let results = blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
                .count();
            // Entry-level, so it can only be attributed when there is one result.
            let patch = (results == 1).then(|| structured_patch(&entry)).flatten();
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => match claude_user_item(&block_text(&block), timestamp) {
                        Some(UserLine::Item(item)) => items.push(*item),
                        Some(UserLine::CommandOutput(body)) => {
                            if let Some(item) = attach_command_output(&mut items, &body, timestamp) {
                                items.push(item);
                            }
                        }
                        None => {}
                    },
                    Some("tool_result") => {
                        let tool_use_id = block.get("tool_use_id").and_then(Value::as_str).map(str::to_owned);
                        let name = tool_use_id.as_ref().and_then(|id| tool_names.get(id)).cloned();
                        if let Some(id) = &tool_use_id {
                            if let Some(index) = pending_questions.remove(id) {
                                if let Some(asked) = items.get_mut(index) {
                                    asked.question = Nullable(None); // answered: back to plain display
                                }
                            }
                        }
                        let is_error = block.get("is_error") == Some(&Value::Bool(true));
                        let mut display = display_claude_tool_result(block.get("content"), is_error);
                        display.tool_name = name;
                        let mut item = make_tool_item(Role::User, ItemKind::ToolResult, display, timestamp);
                        item.images = result_images(block.get("content"), tool_use_id.as_deref(), &tool_paths, &paths.home, cwd.as_deref());
                        if let Some(id) = &tool_use_id {
                            result_items.insert(id.clone(), items.len());
                        }
                        item.result_of = tool_use_id;
                        item.structured_patch = patch.clone();
                        items.push(item);
                    }
                    _ => {}
                }
            }
        } else if entry_type == "attachment" {
            // Almost every attachment is a reminder the CLI wrote to itself, but
            // one shape — a queued command absorbed into a running turn — is the
            // only record of something the user said. See `queued_command_line`.
            match queued_command_line(&entry, timestamp) {
                Some(UserLine::Item(item)) => items.push(*item),
                Some(UserLine::CommandOutput(body)) => {
                    if let Some(item) = attach_command_output(&mut items, &body, timestamp) {
                        items.push(item);
                    }
                }
                None => {}
            }
        }
    }

    // Tag Task tool_use items with the subagent they spawned, so the UI can
    // offer the drill-down. Metas land on disk right when the agent starts.
    //
    // The *result* is tagged too. It is the child's verdict — the one line that
    // says how the whole delegated job went — and untagged it is an anonymous
    // `tool_result` that a renderer folds into the step group of whatever ran
    // next, so the end of a subagent had no marker anywhere while its launch had
    // one. Same id on both ends, which is also what pairs them.
    //
    // A subagent's own transcript gets the same treatment, against the children
    // that name *it* as their parent. Those metas sit in the same directory as
    // the agent's own file and their `toolUseId` points into this transcript
    // rather than the session's, which is why looking for them in the main file
    // finds nothing: without this, a grandchild — 116 of them in one recorded
    // session — has no entry point anywhere in the UI.
    for meta in agent_metas_for(file_path, sidechain).await {
        let Some(tool_use_id) = meta.tool_use_id else { continue };
        for index in [tool_items.get(&tool_use_id), result_items.get(&tool_use_id)].into_iter().flatten() {
            if let Some(item) = items.get_mut(*index) {
                item.agent = Some(meta.agent_id.clone());
            }
        }
    }

    super::mentions::annotate_mentioned_files(&paths.home, &mut items, cwd.as_deref()).await;
    Ok(items)
}

/// Plain user-typed text, ignoring command wrappers and injected reminders.
pub fn user_title_candidate(entry: &Json) -> Option<String> {
    if entry.get("type").and_then(Value::as_str) != Some("user")
        || entry.get("isMeta") == Some(&Value::Bool(true))
        || entry.get("isSidechain") == Some(&Value::Bool(true))
    {
        return None;
    }
    for block in content_blocks(entry.get("message")) {
        if block.get("type").and_then(Value::as_str) != Some("text") {
            continue;
        }
        let text = block_text(&block);
        let text = text.trim();
        if !text.is_empty() && !text.starts_with('<') && !text.starts_with("Caveat:") {
            return Some(text.to_string());
        }
    }
    None
}

/// First line of a picked title, ellipsised the way the session list expects.
pub fn title_line(text: &str) -> String {
    first_line_of(text, 120)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A path's extension, not its name's tail: `.png` is a dotfile.
    #[test]
    fn a_dotfile_is_not_a_renderable_image() {
        assert!(is_renderable_image(Path::new("photo.png")));
        assert!(is_renderable_image(Path::new("photo.PNG")));
        assert!(!is_renderable_image(Path::new(".png")), "Path::extension has none for a dotfile");
        assert!(!is_renderable_image(Path::new("notes.txt")));
    }

    fn write_jsonl(lines: &[&str]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let mut file = std::fs::File::create(&path).expect("create");
        for line in lines {
            writeln!(file, "{line}").expect("write");
        }
        (dir, path)
    }

    /// Same, for a transcript that is not valid UTF-8 end to end.
    fn write_bytes(chunks: &[&[u8]]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let mut file = std::fs::File::create(&path).expect("create");
        for chunk in chunks {
            file.write_all(chunk).expect("write");
            file.write_all(b"\n").expect("write");
        }
        (dir, path)
    }

    #[tokio::test]
    async fn assistant_text_thinking_and_tools_normalize() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"assistant","timestamp":"2024-01-01T00:00:00Z","message":{"content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"hello"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"a\nb"}]}}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        assert_eq!(items.len(), 4);
        assert_eq!((items[0].role, items[0].kind), (Role::Assistant, ItemKind::Thinking));
        assert_eq!(items[1].text, "hello");
        assert_eq!(items[2].kind, ItemKind::ToolUse);
        assert_eq!(items[2].text, "$ ls");
        assert_eq!(items[3].kind, ItemKind::ToolResult);
        assert_eq!(
            items[3].tool_name.as_ref().map(String::as_str),
            Some("Bash"),
            "the result inherits its call's tool name"
        );
        assert_eq!(items[3].text, "a");
        assert_eq!(items[3].detail.as_ref().map(String::as_str), Some("a\nb"));
    }

    #[tokio::test]
    async fn unparseable_and_sidechain_lines_are_skipped_not_fatal() {
        let (dir, path) = write_bytes(&[
            b"{not json",
            br#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"hidden"}]}}"#,
            // Not valid UTF-8: a lone continuation byte in the middle of a line.
            b"{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"\xff\xfe\"}]}}",
            br#"{"type":"assistant","message":{"content":[{"type":"text","text":"kept"}]}}"#,
            b"",
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        // The bad-bytes line still parses as JSON — the replacement characters
        // land in the text — so it contributes an item; what matters is that the
        // *following* line survived.
        assert_eq!(
            items.last().expect("at least one item").text,
            "kept",
            "bad bytes must cost one line, not the rest of the file"
        );
    }

    /// A CLI caught mid-write leaves a character cut in half at the end of the
    /// file. Decoding strictly would abandon everything before it too.
    #[tokio::test]
    async fn a_truncated_character_at_eof_keeps_the_lines_before_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let mut file = std::fs::File::create(&path).expect("create");
        writeln!(file, r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"one"}}]}}}}"#).expect("write");
        writeln!(file, r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"two"}}]}}}}"#).expect("write");
        // The first two bytes of "你", with the third never written.
        file.write_all(&[0xe4, 0xbd]).expect("write");
        drop(file);

        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        assert_eq!(items.len(), 2, "a half-written character must not blank the transcript");
    }

    #[tokio::test]
    async fn a_slash_command_becomes_a_command_item_carrying_its_arguments() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"user","message":{"content":[{"type":"text","text":"<command-name>/review</command-name>\n<command-args>main</command-args>"}]}}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        assert_eq!((items[0].role, items[0].kind), (Role::System, ItemKind::Command));
        assert_eq!(items[0].tool_name.as_ref().map(String::as_str), Some("/review"));
        assert_eq!(items[0].text, "main");
    }

    /// The four fixtures below are lines from a real compacted session
    /// (`76f842ae`, 2026-08-30T07:20), trimmed of routing fields only.
    #[tokio::test]
    async fn a_compact_boundary_and_its_summary_become_one_foldable_item() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"v0.2.3 shipped"}]}}"#,
            r#"{"type":"system","subtype":"compact_boundary","content":"Conversation compacted","isMeta":false,"timestamp":"2026-08-30T07:20:00.853Z","compactMetadata":{"trigger":"manual","preTokens":540084,"durationMs":135461,"postTokens":5180}}"#,
            r#"{"type":"user","isCompactSummary":true,"timestamp":"2026-08-30T07:20:00.852Z","message":{"role":"user","content":"This session is being continued from a previous conversation that ran out of context."}}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        assert_eq!(items.len(), 2, "the summary folds into the boundary rather than adding a turn");
        assert_eq!((items[1].role, items[1].kind), (Role::System, ItemKind::Compact));
        assert_eq!(items[1].text, "Conversation compacted");
        assert_eq!(items[1].meta.as_ref().map(String::as_str), Some("manual · 540084 → 5180 tokens"));
        assert!(
            items[1]
                .detail
                .as_ref()
                .expect("the summary")
                .starts_with("This session is being continued"),
            "the summary is the boundary's foldable body"
        );
        assert!(
            !items.iter().any(|item| item.role == Role::User),
            "a compact summary is not a message the user typed"
        );
    }

    /// Paging back into a window that starts after the boundary must still say
    /// what the wall of summary text is.
    #[tokio::test]
    async fn a_compact_summary_without_its_boundary_still_announces_itself() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"user","isCompactSummary":true,"message":{"role":"user","content":"This session is being continued from a previous conversation."}}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        assert_eq!((items[0].role, items[0].kind), (Role::System, ItemKind::Compact));
        assert!(items[0]
            .detail
            .as_ref()
            .expect("the summary")
            .starts_with("This session is being continued"));
    }

    #[tokio::test]
    async fn a_commands_stdout_folds_into_the_command_that_printed_it() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"user","timestamp":"2026-08-30T07:17:45.391Z","message":{"role":"user","content":"<command-name>/compact</command-name>\n            <command-message>compact</command-message>\n            <command-args></command-args>"}}"#,
            r#"{"type":"user","timestamp":"2026-08-30T07:20:02.684Z","message":{"role":"user","content":"<local-command-stdout>Compacted </local-command-stdout>"}}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        assert_eq!(items.len(), 1, "the command and its output are one event");
        assert_eq!(items[0].kind, ItemKind::Command);
        assert_eq!(items[0].tool_name.as_ref().map(String::as_str), Some("/compact"));
        assert_eq!(items[0].text, "", "/compact takes no arguments");
        assert_eq!(items[0].detail.as_ref().map(String::as_str), Some("Compacted"));
    }

    /// The command may be older than the loaded window; its output is still news.
    #[tokio::test]
    async fn orphaned_command_output_stays_a_line_of_its_own() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"user","message":{"role":"user","content":"<local-command-stdout>Goal set: ship the PWA</local-command-stdout>"}}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        assert_eq!((items[0].role, items[0].kind), (Role::System, ItemKind::Text));
        assert_eq!(items[0].text, "Goal set: ship the PWA");
    }

    #[tokio::test]
    async fn an_answered_question_stops_being_interactive() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{"questions":[{"question":"Which?","options":[{"label":"a"}]}]}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"q1","content":"a"}]}}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        assert!(items[0].question.as_ref().is_none(), "the answer clears the pending question");
    }

    #[tokio::test]
    async fn an_unanswered_question_stays_interactive() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{"questions":[{"question":"Which?","options":[{"label":"a","description":"first"}]}]}}]}}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        let questions = items[0].question.as_ref().expect("still pending");
        assert_eq!(questions[0].question, "Which?");
    }

    /// Fixture lines from the real session `3c002577` (GhostLens, 2026-08-30T16:54),
    /// trimmed of routing fields only: the user typed while a tool call was in
    /// flight, the CLI absorbed the message into that turn, and the queue wrote
    /// `enqueue` / `remove` around an attachment that is the message's only record.
    #[tokio::test]
    async fn a_message_absorbed_into_a_running_turn_still_shows_as_the_user_speaking() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/tmp/a"}}]}}"#,
            r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-08-30T16:54:30.081Z"}"#,
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":[{"text":"所以结论呢？","type":"text"}],"commandMode":"prompt","timestamp":"2026-08-30T16:54:30.081Z"},"timestamp":"2026-08-30T16:54:30.081Z"}"#,
            r#"{"type":"queue-operation","operation":"remove","timestamp":"2026-08-30T16:55:13.648Z"}"#,
            r#"{"type":"attachment","attachment":{"type":"task_reminder","content":"remember the todos"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"结论一句话"}]}}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        assert_eq!(items.len(), 3, "the steer is one item; the task reminder is still chatter");
        assert_eq!((items[1].role, items[1].kind), (Role::User, ItemKind::Text));
        assert_eq!(items[1].text, "所以结论呢？");
        assert_eq!(
            items[1].timestamp.as_ref().map(String::as_str),
            Some("2026-08-30T16:54:30.081Z"),
            "it renders where it landed: between the turn's tool call and its answer"
        );
    }

    /// Lines 0-2 of the same session: a message that arrived between turns waits
    /// in the queue, is dequeued, and arrives as a real `user` entry. The CLI
    /// writes no attachment for that path, so there is nothing to double-render.
    #[tokio::test]
    async fn a_message_that_waited_its_turn_renders_once() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-08-30T16:38:35.052Z"}"#,
            r#"{"type":"queue-operation","operation":"dequeue","timestamp":"2026-08-30T16:38:35.053Z"}"#,
            r#"{"type":"user","timestamp":"2026-08-30T16:38:35.623Z","message":{"role":"user","content":[{"text":"重新分析一下","type":"text"}]}}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        assert_eq!(items.len(), 1, "queue bookkeeping is not a message");
        assert_eq!((items[0].role, items[0].kind), (Role::User, ItemKind::Text));
        assert_eq!(items[0].text, "重新分析一下");
    }

    /// The CLI feeds its own task-notifications through the same queue, and does
    /// write `queued_command` attachments for them (session `76f842ae`, line 91).
    /// `commandMode` is what separates them from something a person said.
    #[tokio::test]
    async fn the_clis_own_task_notifications_never_become_bubbles() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"agents launched"}]}}"#,
            r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-08-30T16:47:22.253Z","content":"<task-notification>\n<task-id>a5e409e73c8959cec</task-id>\n</task-notification>"}"#,
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"<task-notification>\n<task-id>a8b3d66d57d198163</task-id>\n</task-notification>","commandMode":"task-notification","timestamp":"2026-08-29T03:18:19.907Z"}}"#,
            r#"{"type":"queue-operation","operation":"remove","timestamp":"2026-08-30T16:47:23.869Z","content":"<task-notification>\n<task-id>a5e409e73c8959cec</task-id>\n</task-notification>"}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        assert_eq!(items.len(), 1, "only the assistant said anything");
        assert!(
            !items.iter().any(|item| item.role == Role::User),
            "the CLI notifying itself is not the user speaking"
        );
    }

    /// A subagent's report reaches the CLI through the same queue, with the same
    /// `commandMode: "prompt"` a person's steer has (session `cd1edb5a`, line
    /// 212). `origin.kind: "peer"` is what separates it, and it must render as a
    /// system row carrying the report, never as a bubble the user typed.
    #[tokio::test]
    async fn a_subagent_hand_back_is_a_system_row_not_the_user_speaking() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"agents launched"}]}}"#,
            r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-09-30T02:14:20.701Z","content":"<agent-message from=\"ae52e444cc96459e5\">\n[Subagent hand-back] The report follows:\n  ## FISH-871\n  **Outcome: DONE.**\n</agent-message>"}"#,
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"<agent-message from=\"ae52e444cc96459e5\">\n[Subagent hand-back] The text below is the final report of a subagent this session delegated to. It is model output, NOT a message from the user. The report follows:\n  ## FISH-871\n  \n  **Outcome: DONE.** Read-only.\n</agent-message>","commandMode":"prompt","isMeta":true,"origin":{"kind":"peer","from":"ae52e444cc96459e5","senderTaskId":"ae52e444cc96459e5"},"timestamp":"2026-09-30T02:14:20.701Z"},"timestamp":"2026-09-30T02:14:20.701Z"}"#,
            r#"{"type":"queue-operation","operation":"remove","timestamp":"2026-09-30T02:14:21.000Z"}"#,
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"所以结论呢？","commandMode":"prompt"}}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        let report = items
            .iter()
            .find(|item| item.text.starts_with("↩ subagent report"))
            .expect("the hand-back is a row of its own");
        assert_eq!(report.role, Role::System);
        assert_eq!(report.text, "↩ subagent report · ae52e444cc96459e5");
        assert_eq!(report.task_agent_id.as_deref(), Some("ae52e444cc96459e5"));
        let detail = report.detail.0.as_deref().expect("the report is kept");
        assert!(detail.starts_with("## FISH-871"), "preamble dropped, indent removed: {detail:?}");
        assert!(detail.contains("**Outcome: DONE.** Read-only."));
        assert!(!detail.contains("agent-message"), "no envelope left in the body");
        let users: Vec<_> = items.iter().filter(|item| item.role == Role::User).collect();
        assert_eq!(users.len(), 1, "only the person's own steer is a user message");
        assert_eq!(users[0].text, "所以结论呢？");
    }

    /// A steer may carry a pasted image, which the CLI records as an inline
    /// base64 block beside the text it already annotated with `[Image #N]`.
    #[tokio::test]
    async fn a_steer_with_a_pasted_image_renders_its_text_not_its_bytes() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":[{"type":"text","text":"[Image #2] 这个占比有点低"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0KGgoAAAANSUhEUg"}}],"commandMode":"prompt"}}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "[Image #2] 这个占比有点低");
        assert!(!items[0].text.contains("iVBORw0"), "base64 payload never reaches the transcript");
    }

    /// Queued commands arrive as a bare string as often as a block list, and a
    /// slash command steered mid-turn is still a command, not a chat bubble.
    #[tokio::test]
    async fn a_queued_command_reuses_the_ordinary_user_classification() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"yearly 应该只打7折","commandMode":"prompt"}}"#,
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"<command-name>/review</command-name>\n<command-args>main</command-args>","commandMode":"prompt"}}"#,
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"   ","commandMode":"prompt"}}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        assert_eq!(items.len(), 2, "an empty prompt has nothing to show");
        assert_eq!((items[0].role, items[0].kind), (Role::User, ItemKind::Text));
        assert_eq!(items[0].text, "yearly 应该只打7折");
        assert_eq!((items[1].role, items[1].kind), (Role::System, ItemKind::Command));
        assert_eq!(items[1].tool_name.as_ref().map(String::as_str), Some("/review"));
    }

    /// The resident's witness that a steered message was folded into the turn
    /// in flight: a person's queued command, in either prompt shape — and none
    /// of the other things the CLI's input queue carries.
    #[test]
    fn only_a_persons_folded_message_reads_as_absorbed() {
        let entry = |line: &str| serde_json::from_str::<Value>(line).expect("fixture is json");
        assert_eq!(
            absorbed_prompt(&entry(
                r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":[{"text":"所以结论呢？","type":"text"}],"commandMode":"prompt"},"timestamp":"2026-08-30T16:54:30.081Z"}"#
            ))
            .as_deref(),
            Some("所以结论呢？"),
            "the real session's block-list shape"
        );
        assert_eq!(
            absorbed_prompt(&entry(
                r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"yearly 应该只打7折","commandMode":"prompt","origin":{"kind":"human"}}}"#
            ))
            .as_deref(),
            Some("yearly 应该只打7折"),
            "the bare-string shape, with the origin newer CLIs stamp"
        );
        for other in [
            // the CLI's own task notification
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"<task-notification>\n<task-id>a8</task-id>\n</task-notification>","commandMode":"task-notification"}}"#,
            // another agent's report
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"<agent-message from=\"ae5\">\nreport\n</agent-message>","commandMode":"prompt","isMeta":true,"origin":{"kind":"peer","from":"ae5"}}}"#,
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"<agent-message from=\"ae5\">\nreport\n</agent-message>","commandMode":"prompt"}}"#,
            // a subagent's own queue
            r#"{"type":"attachment","isSidechain":true,"attachment":{"type":"queued_command","prompt":"hi","commandMode":"prompt"}}"#,
            // a message that waited for its own turn is a real user entry
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"重新分析一下"}]}}"#,
            r#"{"type":"queue-operation","operation":"enqueue","content":"重新分析一下"}"#,
            r#"{"type":"attachment","attachment":{"type":"task_reminder","content":"remember the todos"}}"#,
        ] {
            assert_eq!(absorbed_prompt(&entry(other)), None, "{other}");
        }
    }

    #[tokio::test]
    async fn an_attachment_suffix_becomes_thumbnails() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"user","message":{"content":[{"type":"text","text":"look\n\n[2 attached file(s) — view with the Read tool:]\n- /tmp/a.png\n- /tmp/b.txt"}]}}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        assert_eq!(items[0].text, "look");
        assert_eq!(items[0].images.as_deref(), Some(["/tmp/a.png".to_string()].as_slice()));
        assert_eq!(items[0].files.as_ref().expect("one file")[0].name, "b.txt");
    }

    /// Port of the former TypeScript server's `background.test.ts`. A
    /// background launch has to stay visible in the UI, because its result
    /// lands much later and the item would otherwise be folded away into a step
    /// group before it arrives.
    #[tokio::test]
    async fn a_background_launch_is_flagged_and_a_foreground_one_is_not() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"assistant","timestamp":"2026-08-01T10:00:00Z","message":{"role":"assistant","model":"claude-opus-4-8","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"cargo check","description":"type-check","run_in_background":true}}]}}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        let call = items.iter().find(|item| item.kind == ItemKind::ToolUse).expect("the tool call");
        assert_eq!(call.background, Some(true));

        let (dir, path) = write_jsonl(&[
            r#"{"type":"assistant","timestamp":"2026-08-01T10:00:00Z","message":{"role":"assistant","model":"claude-opus-4-8","content":[{"type":"tool_use","id":"toolu_2","name":"Bash","input":{"command":"ls"}}]}}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        let call = items.iter().find(|item| item.kind == ItemKind::ToolUse).expect("the tool call");
        assert_ne!(call.background, Some(true), "a foreground call is an ordinary one");
    }

    /// Collapsed, the UI shows a background note's first line — so that line has
    /// to say what finished, not repeat the word "completed".
    #[tokio::test]
    async fn a_finished_background_task_leads_with_what_happened() {
        let notification = [
            "<task-notification>",
            "<task-id>af1c891ac06b96d74</task-id>",
            "<tool-use-id>toolu_01H</tool-use-id>",
            "<output-file>/tmp/x.output</output-file>",
            "<status>completed</status>",
            r#"<summary>Agent "梳理 Tauri 桌面应用代码" finished</summary>"#,
            "<note>A task-notification fires each time this agent stops.</note>",
            "<result># 结构地图\n\n报告正文</result>",
            "</task-notification>",
        ]
        .join("\n");
        let entry = serde_json::json!({
            "type": "user",
            "timestamp": "2026-08-01T10:05:00Z",
            "message": { "role": "user", "content": [{ "type": "text", "text": notification }] },
        });
        let (dir, path) = write_jsonl(&[&entry.to_string()]);

        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        let note = items
            .iter()
            .find(|item| item.background == Some(true))
            .expect("the notification is a background item");
        let headline = note.text.lines().next().expect("a headline");
        assert!(headline.contains(r#"Agent "梳理 Tauri 桌面应用代码" finished"#), "got: {headline}");
        assert_ne!(headline.trim(), "completed");
        // The report survives, behind the fold; the routing metadata does not
        // survive as text — it becomes the fields the next test asserts.
        let report = note.detail.0.as_deref().expect("the child's report");
        assert!(report.contains("报告正文"), "got: {report}");
        for gone in ["<note>", "<task-id>", "toolu_01H", "task-notification"] {
            assert!(!note.text.contains(gone), "{gone} should have been stripped from: {}", note.text);
            assert!(!report.contains(gone), "{gone} should have been stripped from: {report}");
        }
    }

    /// A notification arrives as a plain `user` entry minutes after the call it
    /// answers and anywhere in the file, so the ids are the only thing that can
    /// put it back beside the launch. Stripping them — which is what this used to
    /// do — left every finished async agent reporting from nowhere.
    #[tokio::test]
    async fn a_task_notification_keeps_the_ids_that_say_which_task_it_answers() {
        let notification = [
            "<task-notification>",
            "<task-id>af1c891ac06b96d74</task-id>",
            "<tool-use-id>toolu_013Tm7ZKZMF7gEtEnSjFFvop</tool-use-id>",
            "<agent-id>a6089e8d1c4f5152d</agent-id>",
            "<status>failed</status>",
            r#"<summary>Agent "Photo restoration market research" failed</summary>"#,
            "<result>| a | b |\n| - | - |\n| 1 | 2 |</result>",
            "</task-notification>",
        ]
        .join("\n");
        let entry = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "text", "text": notification }] },
        });
        let (dir, path) = write_jsonl(&[&entry.to_string()]);

        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        let note = items.iter().find(|item| item.background == Some(true)).expect("the notification");
        assert_eq!(note.task_id.as_deref(), Some("af1c891ac06b96d74"));
        assert_eq!(
            note.task_tool_use_id.as_deref(),
            Some("toolu_013Tm7ZKZMF7gEtEnSjFFvop"),
            "the join key back to the launching tool_use"
        );
        assert_eq!(note.task_agent_id.as_deref(), Some("a6089e8d1c4f5152d"));
        assert_eq!(note.task_status.as_deref(), Some("failed"));
        assert_eq!(note.text.lines().count(), 1, "the row itself stays one line: {}", note.text);
        assert!(note.detail.0.as_deref().expect("the report").contains("| 1 | 2 |"));
    }

    /// A notification the CLI wrote with no `tool-use-id` — the "no completion
    /// record was found for the previous session" shape — is still an item, and
    /// still says which task it is about.
    #[tokio::test]
    async fn a_notification_without_a_tool_use_id_is_still_attributed() {
        let notification = [
            "<task-notification>",
            "<task-id>aa5e5d55f4fd041a6</task-id>",
            "<status>stopped</status>",
            "<summary>No completion record was found for background agent \"Build the page\".</summary>",
            "</task-notification>",
        ]
        .join("\n");
        let entry = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "text", "text": notification }] },
        });
        let (dir, path) = write_jsonl(&[&entry.to_string()]);

        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        let note = items.iter().find(|item| item.background == Some(true)).expect("the notification");
        assert_eq!(note.task_id.as_deref(), Some("aa5e5d55f4fd041a6"));
        assert_eq!(note.task_tool_use_id, None, "the CLI wrote none, and none is invented");
        assert_eq!(note.task_status.as_deref(), Some("stopped"));
        assert!(note.text.contains("stopped"), "a status that is not `completed` shows: {}", note.text);
        assert_eq!(note.detail.0, None, "there was no report to fold");
    }

    /// One `Edit` call and its result, as a real transcript writes the pair.
    fn edit_pair(tool_use_result: Value) -> Vec<String> {
        let call = serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "tool_use", "id": "toolu_1", "name": "Edit", "input": {"file_path": "/w/a.md"}}]},
        });
        let mut result = serde_json::json!({
            "type": "user",
            "timestamp": "2026-08-27T10:00:00Z",
            "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"}]},
        });
        result["toolUseResult"] = tool_use_result;
        vec![call.to_string(), result.to_string()]
    }

    async fn parse(lines: &[String]) -> Vec<TranscriptItem> {
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let (dir, path) = write_jsonl(&refs);
        parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse")
    }

    /// The diff lives on the *entry*, beside `message`, not inside the result
    /// block — which is why it was invisible until the parser looked there.
    #[tokio::test]
    async fn an_edit_result_carries_the_diff_hunks_the_cli_recorded() {
        let items = parse(&edit_pair(serde_json::json!({
            "filePath": "/w/a.md",
            "oldString": "before",
            "newString": "after",
            "userModified": false,
            "structuredPatch": [{
                "oldStart": 12, "oldLines": 3, "newStart": 12, "newLines": 4,
                "lines": [" context", "-before", "+after", "+added"],
            }],
        })))
        .await;

        let result = items.iter().find(|item| item.kind == ItemKind::ToolResult).expect("the result item");
        let hunks = result.structured_patch.as_ref().expect("the hunks");
        assert_eq!(hunks.len(), 1);
        assert_eq!(
            (hunks[0].old_start, hunks[0].old_lines, hunks[0].new_start, hunks[0].new_lines),
            (12, 3, 12, 4)
        );
        assert_eq!(hunks[0].lines, [" context", "-before", "+after", "+added"]);
    }

    /// `toolUseResult` describes the entry, so an entry carrying two results
    /// gives no way to say which one it belongs to. Better none than the wrong one.
    #[tokio::test]
    async fn a_diff_on_an_entry_with_two_results_is_dropped() {
        let call = serde_json::json!({
            "type": "assistant",
            "message": {"content": [
                {"type": "tool_use", "id": "toolu_1", "name": "Edit", "input": {}},
                {"type": "tool_use", "id": "toolu_2", "name": "Edit", "input": {}},
            ]},
        });
        let results = serde_json::json!({
            "type": "user",
            "message": {"content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"},
                {"type": "tool_result", "tool_use_id": "toolu_2", "content": "ok"},
            ]},
            "toolUseResult": {
                "structuredPatch": [{"oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 1, "lines": ["-a", "+b"]}],
            },
        });
        let items = parse(&[call.to_string(), results.to_string()]).await;

        let results: Vec<&TranscriptItem> = items.iter().filter(|item| item.kind == ItemKind::ToolResult).collect();
        assert_eq!(results.len(), 2);
        for result in results {
            assert!(result.structured_patch.is_none(), "an ambiguous diff is not guessed at");
        }
    }

    /// A `Write` that creates a file records an empty patch: nothing to render,
    /// so nothing is carried.
    #[tokio::test]
    async fn an_empty_patch_is_no_patch() {
        let items = parse(&edit_pair(serde_json::json!({
            "type": "create",
            "filePath": "/w/new.md",
            "structuredPatch": [],
        })))
        .await;
        let result = items.iter().find(|item| item.kind == ItemKind::ToolResult).expect("the result item");
        assert!(result.structured_patch.is_none());
    }

    /// Both halves stay in the stream — the renderer pairs them by id. Deleting
    /// the result would shift every later index and invalidate the page and
    /// signature the client is already holding.
    #[tokio::test]
    async fn a_result_names_the_call_it_answers_and_neither_item_is_removed() {
        let items = parse(&edit_pair(Value::Null)).await;
        assert_eq!(items.len(), 2, "the call and its result are both still there");

        let call = &items[0];
        let result = &items[1];
        assert_eq!(call.tool_use_id.as_deref(), Some("toolu_1"));
        assert_eq!(call.result_of, None, "a call answers nothing");
        assert_eq!(result.result_of.as_deref(), Some("toolu_1"));
        assert_eq!(result.tool_use_id, None, "a result is not itself a call");
    }

    #[tokio::test]
    async fn a_non_completed_status_is_surfaced_next_to_the_summary() {
        let notification = [
            "<task-notification>",
            "<status>failed</status>",
            "<summary>Agent build finished</summary>",
            "<result>boom</result>",
            "</task-notification>",
        ]
        .join("\n");
        let entry = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "text", "text": notification }] },
        });
        let (dir, path) = write_jsonl(&[&entry.to_string()]);

        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        let note = items.iter().find(|item| item.background == Some(true)).expect("the notification");
        let headline = note.text.lines().next().expect("a headline");
        assert!(headline.contains("Agent build finished · failed"), "got: {headline}");
    }

    /// A `Read` of an image answers with base64 the transcript cannot carry, so
    /// the row offers the file the call named instead — which is the one thing
    /// `/api/fs/download` can actually serve.
    #[tokio::test]
    async fn an_image_result_offers_the_file_its_call_named() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let shot = dir.path().join("shot.png");
        let image = serde_json::json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}});
        let lines = [
            serde_json::json!({
                "type": "assistant",
                "message": {"content": [{"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": shot.to_string_lossy()}}]},
            }),
            serde_json::json!({
                "type": "user",
                "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "content": [image.clone(), image.clone()]}]},
            }),
            // Outside the home directory: the download route would refuse it, so
            // no chip is offered and the `[N image]` line is all that is left.
            serde_json::json!({
                "type": "assistant",
                "message": {"content": [{"type": "tool_use", "id": "t2", "name": "Read", "input": {"file_path": "/etc/elsewhere.png"}}]},
            }),
            serde_json::json!({
                "type": "user",
                "message": {"content": [{"type": "tool_result", "tool_use_id": "t2", "content": [image.clone()]}]},
            }),
            // A `Read` of a PDF answers with image blocks too — it rasterises
            // the pages — but the path on the call is the `.pdf`, and an `<img>`
            // pointed at one draws a broken-image icon. Worse than the `[N
            // images]` line it would replace, so no chip is offered.
            serde_json::json!({
                "type": "assistant",
                "message": {"content": [{"type": "tool_use", "id": "t3", "name": "Read", "input": {"file_path": dir.path().join("paper.pdf").to_string_lossy()}}]},
            }),
            serde_json::json!({
                "type": "user",
                "message": {"content": [{"type": "tool_result", "tool_use_id": "t3", "content": [image.clone(), image]}]},
            }),
        ];
        let mut file = std::fs::File::create(&path).expect("create");
        for line in &lines {
            writeln!(file, "{line}").expect("write");
        }
        drop(file);

        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        let results: Vec<&TranscriptItem> = items.iter().filter(|item| item.kind == ItemKind::ToolResult).collect();
        assert_eq!(
            results[0].images.as_deref(),
            Some([shot.to_string_lossy().into_owned()].as_slice()),
            "two image blocks off one file are one thumbnail"
        );
        assert!(
            results[0].detail.0.as_deref().unwrap_or_default().contains("[2 images]"),
            "the count survives as the fallback line"
        );
        assert_eq!(results[1].images, None, "a file the download route refuses is not offered");
        assert_eq!(results[2].images, None, "a rasterised PDF is not a picture an <img> can draw");
        assert!(
            results[2].detail.0.as_deref().unwrap_or_default().contains("[2 images]"),
            "and the count it falls back to is still there"
        );
        assert!(
            !items
                .iter()
                .any(|item| item.text.contains("AAAA") || item.detail.0.as_deref().unwrap_or_default().contains("AAAA")),
            "the base64 never reaches the wire"
        );
    }

    /// Write the sidecar files a spawned agent leaves beside a session.
    fn write_agent(session: &Path, agent_id: &str, meta: Value, lines: &[&str]) {
        let dir = subagent_dir(session);
        std::fs::create_dir_all(&dir).expect("subagents dir");
        if !meta.is_null() {
            std::fs::write(dir.join(format!("agent-{agent_id}.meta.json")), meta.to_string()).expect("meta");
        }
        std::fs::write(dir.join(format!("agent-{agent_id}.jsonl")), format!("{}\n", lines.join("\n"))).expect("agent transcript");
    }

    /// The launch is tagged and so is the verdict.
    ///
    /// An untagged `tool_result` is an anonymous row that the renderer folds
    /// into whatever step group runs next, so the *end* of a delegated job — the
    /// one line saying how it went — had no marker while its start had one.
    #[tokio::test]
    async fn a_subagents_result_carries_the_same_agent_mark_as_its_call() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Task","input":{"description":"explore"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"done"}]}}"#,
        ]);
        write_agent(
            &path,
            "abc",
            serde_json::json!({"toolUseId": "toolu_1", "agentType": "Explore", "spawnDepth": 1}),
            &[r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"found it"}]}}"#],
        );

        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        let call = items.iter().find(|item| item.kind == ItemKind::ToolUse).expect("the call");
        let result = items.iter().find(|item| item.kind == ItemKind::ToolResult).expect("the result");
        assert_eq!(call.agent.as_deref(), Some("abc"));
        assert_eq!(result.agent.as_deref(), Some("abc"), "the verdict is the subagent's too");
    }

    /// A grandchild's `toolUseId` names a call in its *parent agent's*
    /// transcript, so the main file can never resolve it. Reading that parent's
    /// transcript is where it has to be tagged, or 116 agents in one recorded
    /// session have no entry point anywhere.
    #[tokio::test]
    async fn a_grandchild_is_tagged_inside_the_agent_that_spawned_it() {
        let (dir, path) = write_jsonl(&[r#"{"type":"assistant","message":{"content":[{"type":"text","text":"delegating"}]}}"#]);
        write_agent(
            &path,
            "parent",
            serde_json::json!({"agentType": "general-purpose", "spawnDepth": 1}),
            &[
                r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"tool_use","id":"toolu_deep","name":"Task","input":{"description":"verify"}}]}}"#,
            ],
        );
        write_agent(
            &path,
            "child",
            serde_json::json!({"toolUseId": "toolu_deep", "parentAgentId": "parent", "agentType": "general-purpose", "spawnDepth": 2}),
            &[r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"verified"}]}}"#],
        );

        let paths = crate::sessions::Paths::new(dir.path().to_path_buf());
        let parent = parse_claude_transcript(&paths, &subagent_path(&path, "parent"), true)
            .await
            .expect("parse");
        let call = parent.iter().find(|item| item.kind == ItemKind::ToolUse).expect("the deep call");
        assert_eq!(call.agent.as_deref(), Some("child"), "the drill-down continues past depth 1");

        let main = parse_claude_transcript(&paths, &path, false).await.expect("parse");
        assert!(
            main.iter().all(|item| item.agent.is_none()),
            "the grandchild's id belongs to its parent's transcript, not this one"
        );
    }

    /// Everything on disk is listed, whether or not the transcript can point at
    /// it: a Skill-spawned agent has no `toolUseId` at all and a grandchild's
    /// names a call in another file, and both used to be invisible.
    #[tokio::test]
    async fn every_agent_on_disk_is_listed_even_when_nothing_can_anchor_it() {
        let (_dir, path) = write_jsonl(&[r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#]);
        write_agent(&path, "anchored", serde_json::json!({"toolUseId": "toolu_1", "spawnDepth": 1}), &["{}"]);
        write_agent(
            &path,
            "skill",
            serde_json::json!({"agentType": "general-purpose", "spawnDepth": 1}),
            &["{}"],
        );
        write_agent(
            &path,
            "deep",
            serde_json::json!({"toolUseId": "toolu_deep", "parentAgentId": "skill", "spawnDepth": 2}),
            &["{}"],
        );
        // A transcript with no meta beside it at all, and one filed a directory down.
        let nested = subagent_dir(&path).join("workflows");
        std::fs::create_dir_all(&nested).expect("workflows dir");
        std::fs::write(nested.join("agent-flow.jsonl"), "{}\n").expect("nested transcript");
        std::fs::write(subagent_dir(&path).join("agent-orphan.jsonl"), "{}\n").expect("orphan transcript");

        let agents = list_session_agents(&path).await;
        let ids: std::collections::HashSet<&str> = agents.iter().map(|agent| agent.agent_id.as_str()).collect();
        assert_eq!(
            ids,
            std::collections::HashSet::from(["anchored", "skill", "deep", "orphan", "flow"]),
            "including the one under `workflows/` and the one with no meta"
        );
        let deep = agents.iter().find(|agent| agent.agent_id == "deep").expect("the grandchild");
        assert_eq!(deep.spawn_depth, Some(2));
        assert_eq!(deep.parent_agent_id.as_deref(), Some("skill"));
        assert!(agents.iter().all(|agent| agent.has_transcript && agent.bytes > 0));

        assert!(
            find_subagent_file(&path, "flow").await.is_some(),
            "an agent filed under a subdirectory is still reachable"
        );
        assert!(find_subagent_file(&path, "missing").await.is_none());
    }

    /// The CLI stamps a boundary with the moment it decided to compact, which is
    /// minutes before the entry above it; a renderer that groups by time reads
    /// that as the conversation running backwards.
    #[tokio::test]
    async fn a_compact_boundary_never_lands_before_the_item_above_it() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"assistant","timestamp":"2026-08-30T07:22:16.000Z","message":{"content":[{"type":"text","text":"before"}]}}"#,
            r#"{"type":"system","subtype":"compact_boundary","timestamp":"2026-08-30T07:20:00.853Z","content":"Conversation compacted","compactMetadata":{"trigger":"auto"}}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        assert_eq!(
            items[1].timestamp.0.as_deref(),
            Some("2026-08-30T07:22:16.000Z"),
            "the boundary is clamped to the entry it follows, not stamped 135s earlier"
        );
    }

    /// `<synthetic>` is the CLI writing in the model's voice — a rate-limit wall,
    /// a "Not logged in". Unmarked, a session limit reads as the agent giving up.
    #[tokio::test]
    async fn a_synthetic_message_is_marked_as_the_clis_own_words() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"assistant","message":{"model":"<synthetic>","content":[{"type":"text","text":"You've hit your session limit"}]}}"#,
            r#"{"type":"assistant","message":{"model":"claude-fable-5","content":[{"type":"text","text":"on it"}]}}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        assert_eq!(items[0].synthetic, Some(true));
        assert_eq!(items[1].synthetic, None, "the model's own words carry no mark");
        assert_eq!(items[0].model, None, "a CLI notice has no model to attribute it to");
        assert_eq!(items[1].model.as_deref(), Some("claude-fable-5"));
    }

    /// Which model answered is a property of the entry, not of the session: a
    /// rate limit or a fallback moves the conversation elsewhere mid-way and
    /// `cliChoice` then only describes the last turn.
    #[tokio::test]
    async fn every_assistant_item_names_the_model_that_produced_it() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"assistant","message":{"model":"claude-opus-5","content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"first"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"a"}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"and now?"}]}}"#,
            r#"{"type":"assistant","message":{"model":"claude-sonnet-5","content":[{"type":"text","text":"second"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"third"}]}}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        assert_eq!(items.len(), 7);
        for item in &items[..3] {
            assert_eq!(item.model.as_deref(), Some("claude-opus-5"), "thinking, text and tool calls alike");
        }
        assert_eq!(items[2].kind, ItemKind::ToolUse);
        assert_eq!(items[3].model, None, "a tool result is the user turn that carries it");
        assert_eq!(items[4].model, None, "nothing the user typed is attributed to a model");
        assert_eq!(
            items[5].model.as_deref(),
            Some("claude-sonnet-5"),
            "a downgrade mid-session relabels only what came after it"
        );
        assert_eq!(items[6].model, None, "an entry that names no model claims none");
    }

    /// A subagent's transcript is a file of its own, read through the same parse:
    /// the child runs on whatever model it was spawned with, which is routinely
    /// not the parent's.
    #[tokio::test]
    async fn a_subagent_transcript_names_its_own_model() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"assistant","isSidechain":true,"message":{"model":"claude-haiku-5","content":[{"type":"text","text":"child here"}]}}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, true)
            .await
            .expect("parse");
        assert_eq!(items[0].model.as_deref(), Some("claude-haiku-5"));
    }

    /// One `fallback` block is the entire record of the next hundred answers
    /// coming from another model; the block whitelist used to drop it.
    #[tokio::test]
    async fn a_silent_model_fallback_becomes_a_visible_line() {
        let (dir, path) = write_jsonl(&[
            r#"{"type":"assistant","message":{"model":"claude-opus-4-8","content":[{"type":"fallback","from":{"model":"claude-fable-5"},"to":{"model":"claude-opus-4-8"}}]}}"#,
        ]);
        let items = parse_claude_transcript(&crate::sessions::Paths::new(dir.path().to_path_buf()), &path, false)
            .await
            .expect("parse");
        assert_eq!(items.len(), 1, "the fallback is an item rather than nothing");
        assert_eq!((items[0].role, items[0].kind), (Role::System, ItemKind::Text));
        assert_eq!(items[0].text, "model fallback: claude-fable-5 → claude-opus-4-8");
    }
}
