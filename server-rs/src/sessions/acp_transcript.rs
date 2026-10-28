//! Sessions driven over ACP: the transcript this server keeps for them, and
//! their discovery.
//!
//! An agent driven over the Agent Client Protocol ([`crate::inject::acp`])
//! streams a turn as `session/update` notifications and keeps nothing a client
//! can read back later except by `session/load`, which means starting the
//! agent. Its own history is its format, not this server's
//! contract. So the session view reads a transcript *this server* writes, one
//! per session, under [`Paths::acp_sessions`]`/<agent id>/<session id>.jsonl`:
//! every update the driver hears is translated by [`Translator`] into the
//! small, stable record set below and appended. The feed watches that file the
//! way it watches a claude or codex transcript, which is what makes streamed
//! output show up as it arrives.
//!
//! The session id is this server's own, a UUID minted when the session is
//! created, not the agent's: an agent chooses its ids, and two agents are free
//! to choose the same one, while every route, feed and store here keys a
//! session by `(source, id)`. The agent's id is kept in the header and is what
//! the driver speaks to the agent with. The directory says which agent the
//! session runs on, and is the only agent it is ever reopened with.
//!
//! The records, one JSON object per line, each with a `timestamp`:
//!
//! * `session` — the first line: `id` (this server's), `agent` (the agent's id,
//!   [`crate::acp_agent`]), `agentSessionId` (the id the agent gave it) and
//!   `cwd`.
//! * `user` — a message this server sent: `text`, and `images` / `files` paths.
//! * `delta` — a streamed piece of the agent's answer (`kind` `text`) or of its
//!   reasoning (`kind` `thinking`).
//! * `assistant` / `thinking` — the whole of a run of deltas, written as soon
//!   as the run ends. It supersedes the deltas before it, so a finished
//!   transcript reads as messages and only a turn still streaming reads as
//!   deltas. It is also the one line [`crate::attention`] counts as the agent
//!   speaking, once per message rather than once per token.
//! * `tool_call` — `id`, `title`, `kind` and `input`; `tool_result` — `id`,
//!   `status` and `output`.
//! * `compact` — the agent handed its context off to a fresh one.
//! * `usage` — the context window reading: `used` of `size` tokens.
//! * `config` — the `model` and `effort` the session runs on from here, written
//!   when they change.
//! * `turn_end` — `stopReason`, as ACP spells it; `error` — `message`.
//!
//! Every session in an agent's directory was started by this server instance,
//! so every one is owned: there is no external ACP session to browse yet.
//! Listing an agent's own sessions (`session/list`) is left for when it can be
//! asked without starting a process per listing. A session whose agent was
//! removed stays listed under the id it recorded ([`crate::acp_agent`] says what
//! that means for sending to it).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use super::transcript::title_line;
use super::util::{
    first_line_of, make_item, make_tool_item, read_head_lines, read_tail_lines, try_parse, FileRef, ItemKind, Role, ToolDisplay, TranscriptItem,
};
use super::{iso8601, Paths, ServerId, SessionActivity, SessionScope, SessionStatus, SessionSummary, Source};
use crate::acp::{ContentBlock, SessionUpdate, ToolCallContent, ToolCallStatus, ToolKind};
use crate::openapi_ext::Nullable;

/// How much of a transcript's start is read for its working directory and title.
const HEAD_BYTES: usize = 64 * 1024;
/// How long a tool's one-line summary may be.
const LINE: usize = 180;

/// The directory one agent's transcripts are kept in, or `None` for an id that
/// cannot name one ([`crate::acp_agent::valid_id`]).
pub fn agent_dir(paths: &Paths, agent: &str) -> Option<PathBuf> {
    crate::acp_agent::valid_id(agent).then(|| paths.acp_sessions().join(agent))
}

/// Whether `id` can name a session: only the ids that cannot leave the
/// directory are let near a path. This server mints UUIDs; the agent's own
/// ids are held to the same rule, so nothing odd reaches a header either.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Where the transcript of session `id` is, and which agent's directory it is
/// in; `None` when there is no such session.
pub async fn locate(paths: &Paths, id: &str) -> Option<(String, PathBuf)> {
    if !valid_id(id) {
        return None;
    }
    for agent in agent_dirs(paths).await {
        let file = paths.acp_sessions().join(&agent).join(format!("{id}.jsonl"));
        if tokio::fs::metadata(&file).await.is_ok_and(|metadata| metadata.is_file()) {
            return Some((agent, file));
        }
    }
    None
}

/// Every agent directory there is, by agent id, in id order.
async fn agent_dirs(paths: &Paths) -> Vec<String> {
    let Ok(mut entries) = tokio::fs::read_dir(paths.acp_sessions()).await else {
        return Vec::new();
    };
    let mut agents = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        if crate::acp_agent::valid_id(&name) && entry.file_type().await.is_ok_and(|kind| kind.is_dir()) {
            agents.push(name);
        }
    }
    agents.sort();
    agents
}

/// What a transcript's `session` record says about the session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Header {
    /// The id the agent gave the session; what it is reopened and spoken to by.
    pub agent_session_id: Option<String>,
    /// Where it runs.
    pub cwd: Option<String>,
}

/// The `session` record of a transcript, if it has one.
pub async fn read_header(file_path: &Path) -> Option<Header> {
    let head = read_head_lines(file_path, HEAD_BYTES).await.ok()?;
    head.iter().filter_map(|line| try_parse(line)).find_map(|entry| {
        if entry.get("type").and_then(Value::as_str) != Some("session") {
            return None;
        }
        let field = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_owned);
        Some(Header {
            agent_session_id: field("agentSessionId").filter(|id| valid_id(id)),
            cwd: field("cwd"),
        })
    })
}

/// A record, stamped now.
pub fn record(kind: &str, fields: Value) -> Value {
    let mut record = match fields {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    record.insert("type".into(), json!(kind));
    record.insert("timestamp".into(), json!(iso8601(std::time::SystemTime::now())));
    Value::Object(record)
}

/// Turns ACP `session/update`s into transcript records.
///
/// Holds the run of deltas in progress, so the record that closes it can carry
/// the whole text.
#[derive(Debug, Default)]
pub struct Translator {
    /// `("text" | "thinking", everything streamed so far)`.
    run: Option<(&'static str, String)>,
}

impl Translator {
    /// The records one update adds.
    ///
    /// A user message chunk is dropped: the message this server sent is
    /// recorded when it is sent, with its attachments, and an echo of it would
    /// be the same message twice. [`Translator::replayed`] is the exception.
    pub fn update(&mut self, update: &SessionUpdate) -> Vec<Value> {
        let mut out = Vec::new();
        match update {
            SessionUpdate::AgentMessageChunk(chunk) => self.stream("text", &content_text(&chunk.content), &mut out),
            SessionUpdate::AgentThoughtChunk(chunk) => self.stream("thinking", &content_text(&chunk.content), &mut out),
            SessionUpdate::ToolCall(call) => {
                out.extend(self.flush());
                if !is_compaction(call.meta.as_ref()) {
                    out.push(record(
                        "tool_call",
                        json!({
                            "id": call.tool_call_id,
                            "title": call.title,
                            "kind": kind_name(call.kind),
                            "input": call.raw_input.clone().unwrap_or(Value::Null),
                        }),
                    ));
                }
            }
            SessionUpdate::ToolCallUpdate(update) => {
                let status = match update.status {
                    Some(ToolCallStatus::Completed) => "completed",
                    Some(ToolCallStatus::Failed) => "failed",
                    _ => return out,
                };
                out.extend(self.flush());
                if is_compaction(update.meta.as_ref()) {
                    out.push(record("compact", json!({})));
                    return out;
                }
                let mut output = update.content.as_deref().map(tool_content_text).unwrap_or_default();
                if output.is_empty() {
                    output = match &update.raw_output {
                        Some(Value::String(text)) => text.clone(),
                        Some(Value::Null) | None => String::new(),
                        Some(other) => serde_json::to_string_pretty(other).unwrap_or_default(),
                    };
                }
                out.push(record(
                    "tool_result",
                    json!({"id": update.tool_call_id, "status": status, "output": output}),
                ));
            }
            SessionUpdate::UsageUpdate(usage) => out.push(record("usage", json!({"used": usage.used, "size": usage.size}))),
            _ => {}
        }
        out
    }

    /// The records an update replayed by `session/load` adds.
    ///
    /// The same as a live one, except that the user's own messages are kept:
    /// a replay is the only copy of them this server will ever see.
    pub fn replayed(&mut self, update: &SessionUpdate) -> Vec<Value> {
        if let SessionUpdate::UserMessageChunk(chunk) = update {
            let text = content_text(&chunk.content);
            let mut out = self.flush();
            if !text.trim().is_empty() {
                out.push(record("user", json!({ "text": text })));
            }
            return out;
        }
        self.update(update)
    }

    /// Add a streamed piece to the run of its kind, closing a run of the other.
    fn stream(&mut self, kind: &'static str, text: &str, out: &mut Vec<Value>) {
        if text.is_empty() {
            return;
        }
        if self.run.as_ref().is_some_and(|(running, _)| *running != kind) {
            out.extend(self.flush());
        }
        self.run.get_or_insert((kind, String::new())).1.push_str(text);
        out.push(record("delta", json!({"kind": kind, "text": text})));
    }

    /// The record that closes the run of deltas in progress, if there is one.
    pub fn flush(&mut self) -> Vec<Value> {
        match self.run.take() {
            Some((kind, text)) if !text.trim().is_empty() => {
                vec![record(if kind == "text" { "assistant" } else { "thinking" }, json!({ "text": text }))]
            }
            _ => Vec::new(),
        }
    }
}

/// Whether an update is a context hand-off rather than a tool the user asked
/// for, as an agent may mark one in `_meta`.
fn is_compaction(meta: Option<&serde_json::Map<String, Value>>) -> bool {
    meta.and_then(|meta| meta.get("contextCompaction")) == Some(&Value::Bool(true))
}

/// The text of one ACP content block.
fn content_text(content: &ContentBlock) -> String {
    match content {
        ContentBlock::Text { text } => text.clone(),
        ContentBlock::ResourceLink { name } => format!("[{name}]"),
        ContentBlock::Image {} => "[image]".to_string(),
        ContentBlock::Other => String::new(),
    }
}

/// The readable text of a tool call's `content` list.
fn tool_content_text(content: &[ToolCallContent]) -> String {
    let parts: Vec<String> = content
        .iter()
        .filter_map(|part| match part {
            ToolCallContent::Content { content } => Some(content_text(content)),
            ToolCallContent::Diff { path } => Some(format!("edited {}", path.display())),
            ToolCallContent::Other => None,
        })
        .filter(|text| !text.is_empty())
        .collect();
    parts.join("\n")
}

/// ACP's name for a tool kind, as the transcript records it.
fn kind_name(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Edit => "edit",
        ToolKind::Delete => "delete",
        ToolKind::Move => "move",
        ToolKind::Search => "search",
        ToolKind::Execute => "execute",
        ToolKind::Think => "think",
        ToolKind::Fetch => "fetch",
        ToolKind::Other => "other",
    }
}

/// The tool label for a recorded tool kind, as the session view shows it.
fn tool_name(kind: &str) -> &'static str {
    match kind {
        "execute" => "shell",
        "read" => "read",
        "edit" => "edit",
        "delete" => "delete",
        "move" => "move",
        "search" => "search",
        "fetch" => "fetch",
        "think" => "think",
        _ => "tool",
    }
}

/// Read a transcript into the items the session view draws.
pub async fn parse_transcript(file_path: &Path) -> std::io::Result<Vec<TranscriptItem>> {
    let text = tokio::fs::read(file_path).await?;
    Ok(parse_lines(String::from_utf8_lossy(&text).lines()))
}

/// [`parse_transcript`] over lines already read.
fn parse_lines<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<TranscriptItem> {
    let mut items = Vec::new();
    // The run of deltas no closing record has superseded yet.
    let mut run: Option<(String, String, Option<String>)> = None;
    let mut tools: HashMap<String, String> = HashMap::new();
    let close_run = |run: &mut Option<(String, String, Option<String>)>, items: &mut Vec<TranscriptItem>| {
        if let Some((kind, text, at)) = run.take() {
            items.push(agent_item(&kind, &text, at.as_deref()));
        }
    };
    for line in lines {
        let Some(entry) = try_parse(line) else { continue };
        let at = entry.get("timestamp").and_then(Value::as_str);
        let field = |key: &str| entry.get(key).and_then(Value::as_str).unwrap_or_default();
        match field("type") {
            "delta" => {
                let kind = if field("kind") == "thinking" { "thinking" } else { "text" };
                if run.as_ref().is_some_and(|(running, _, _)| running != kind) {
                    close_run(&mut run, &mut items);
                }
                run.get_or_insert_with(|| (kind.to_string(), String::new(), at.map(str::to_owned)))
                    .1
                    .push_str(field("text"));
            }
            kind @ ("assistant" | "thinking") => {
                let kind = if kind == "assistant" { "text" } else { "thinking" };
                // The record is the whole run: its deltas are dropped, and kept
                // only if they were a different kind it does not account for.
                let started = match run.take() {
                    Some((running, _, started)) if running == kind => started,
                    other => {
                        run = other;
                        close_run(&mut run, &mut items);
                        None
                    }
                };
                items.push(agent_item(kind, field("text"), started.as_deref().or(at)));
            }
            // Bookkeeping, not conversation: an agent reports its context usage in
            // the middle of a streamed answer, and it must not split one.
            "session" | "usage" | "config" => {}
            other => {
                close_run(&mut run, &mut items);
                match other {
                    "user" => items.push(user_item(&entry, at)),
                    "tool_call" => {
                        let id = field("id").to_string();
                        let name = tool_name(field("kind"));
                        tools.insert(id.clone(), name.to_string());
                        let input = entry.get("input").filter(|input| !input.is_null());
                        let detail = input.map(|input| match input {
                            Value::String(text) => text.clone(),
                            other => serde_json::to_string_pretty(other).unwrap_or_default(),
                        });
                        let title = field("title");
                        let mut item = make_tool_item(
                            Role::Assistant,
                            ItemKind::ToolUse,
                            ToolDisplay {
                                tool_name: Some(name.to_string()),
                                text: if title.is_empty() {
                                    name.to_string()
                                } else {
                                    first_line_of(title, LINE)
                                },
                                detail: detail.filter(|detail| !detail.is_empty()),
                                ..ToolDisplay::default()
                            },
                            at,
                        );
                        item.tool_use_id = Some(id).filter(|id| !id.is_empty());
                        items.push(item);
                    }
                    "tool_result" => {
                        let id = field("id").to_string();
                        let output = field("output").replace("\r\n", "\n");
                        let output = output.trim();
                        let headline = first_line_of(output, LINE);
                        let mut item = make_tool_item(
                            Role::User,
                            ItemKind::ToolResult,
                            ToolDisplay {
                                tool_name: tools.get(&id).cloned(),
                                text: if headline.is_empty() { "no output".to_string() } else { headline },
                                detail: (!output.is_empty()).then(|| output.to_string()),
                                is_error: field("status") == "failed",
                                ..ToolDisplay::default()
                            },
                            at,
                        );
                        item.result_of = Some(id).filter(|id| !id.is_empty());
                        items.push(item);
                    }
                    "compact" => items.push(make_item(Role::System, ItemKind::Compact, "Context compacted", at)),
                    "turn_end" => match field("stopReason") {
                        "cancelled" => items.push(make_item(Role::System, ItemKind::Text, "Interrupted", at)),
                        "max_tokens" => items.push(make_item(
                            Role::System,
                            ItemKind::Text,
                            "Stopped: the model's output limit was reached",
                            at,
                        )),
                        "max_turn_requests" => items.push(make_item(
                            Role::System,
                            ItemKind::Text,
                            "Stopped: too many model requests in one turn",
                            at,
                        )),
                        "refusal" => items.push(make_item(Role::System, ItemKind::Text, "The agent refused to continue", at)),
                        _ => {}
                    },
                    "error" => {
                        let mut item = make_item(Role::System, ItemKind::Text, field("message"), at);
                        item.is_error = true;
                        items.push(item);
                    }
                    // A record a newer build wrote: shown as nothing, and it
                    // still ends the answer before it, which is the safe side.
                    _ => {}
                }
            }
        }
    }
    close_run(&mut run, &mut items);
    items
}

/// One message, or one run of reasoning, from the agent.
fn agent_item(kind: &str, text: &str, at: Option<&str>) -> TranscriptItem {
    make_item(
        Role::Assistant,
        if kind == "thinking" { ItemKind::Thinking } else { ItemKind::Text },
        text,
        at,
    )
}

/// A message this server sent, with what was attached to it.
fn user_item(entry: &Map<String, Value>, at: Option<&str>) -> TranscriptItem {
    let mut item = make_item(
        Role::User,
        ItemKind::Text,
        entry.get("text").and_then(Value::as_str).unwrap_or_default(),
        at,
    );
    let paths = |key: &str| -> Vec<String> {
        entry
            .get(key)
            .and_then(Value::as_array)
            .map(|paths| paths.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default()
    };
    let images = paths("images");
    let files: Vec<FileRef> = paths("files")
        .into_iter()
        .map(|path| FileRef {
            name: Path::new(&path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone()),
            path,
        })
        .collect();
    item.images = (!images.is_empty()).then_some(images);
    item.files = (!files.is_empty()).then_some(files);
    item
}

/// The texts in one transcript line that are the agent speaking to the user.
///
/// The ACP arm of [`crate::attention::derive::spoken_texts`]: the record that
/// closes a run of answer text, never a delta.
pub fn spoken_texts(entry: &Map<String, Value>) -> Vec<String> {
    if entry.get("type").and_then(Value::as_str) != Some("assistant") {
        return Vec::new();
    }
    entry
        .get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| vec![text.to_string()])
        .unwrap_or_default()
}

/// The model and effort the session last ran on: its newest `config` record.
pub async fn read_cli_choice(file_path: &Path) -> Option<super::CliChoice> {
    let lines = read_tail_lines(file_path, HEAD_BYTES as u64).await.ok()?;
    lines.iter().rev().filter_map(|line| try_parse(line)).find_map(|entry| {
        if entry.get("type").and_then(Value::as_str) != Some("config") {
            return None;
        }
        let field = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_owned);
        Some(super::CliChoice {
            model: Nullable(field("model")),
            effort: Nullable(field("effort")),
            at: Nullable(field("timestamp")),
        })
    })
}

/// Whether a transcript records any conversation at all.
///
/// A session reopened by `session/load` is given the agent's own history only
/// when this says there is none here: replaying it over a transcript that already
/// has it would be every message twice.
pub async fn has_conversation(file_path: &Path) -> bool {
    let Ok(lines) = read_tail_lines(file_path, HEAD_BYTES as u64).await else {
        return false;
    };
    lines
        .iter()
        .filter_map(|line| try_parse(line))
        .any(|entry| entry.get("type").and_then(Value::as_str) != Some("session"))
}

/// Every ACP session this server made, on every agent, configured or not; the
/// order is the caller's.
pub async fn list_sessions(paths: &Paths) -> Vec<SessionSummary> {
    let titles = super::custom_titles(paths, Source::Acp).await;
    let mut out = Vec::new();
    for agent in agent_dirs(paths).await {
        let dir = paths.acp_sessions().join(&agent);
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else { continue };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_suffix(".jsonl").filter(|id| valid_id(id)) else { continue };
            if let Some(summary) = summary_of(paths, &agent, &dir.join(&name), id, &titles).await {
                out.push(summary);
            }
        }
    }
    out
}

/// One session's list row, or `None` when there is no such session.
pub async fn find_session(paths: &Paths, id: &str) -> Option<SessionSummary> {
    let (agent, file_path) = locate(paths, id).await?;
    let titles = super::custom_titles(paths, Source::Acp).await;
    summary_of(paths, &agent, &file_path, id, &titles).await
}

/// Build the row for one transcript of `agent`'s.
async fn summary_of(paths: &Paths, agent: &str, file_path: &Path, id: &str, titles: &HashMap<String, String>) -> Option<SessionSummary> {
    let metadata = tokio::fs::metadata(file_path).await.ok()?;
    if !metadata.is_file() {
        return None;
    }
    let modified = metadata.modified().map(iso8601).unwrap_or_default();
    let head = read_head_lines(file_path, HEAD_BYTES).await.unwrap_or_default();
    let mut cwd = None;
    let mut created_at = None;
    let mut first_message = None;
    for entry in head.iter().filter_map(|line| try_parse(line)) {
        let field = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_owned);
        created_at = created_at.or_else(|| field("timestamp"));
        match entry.get("type").and_then(Value::as_str) {
            Some("session") => cwd = cwd.or_else(|| field("cwd")),
            Some("user") => first_message = first_message.or_else(|| field("text").filter(|text| !text.trim().is_empty())),
            _ => {}
        }
        if cwd.is_some() && first_message.is_some() {
            break;
        }
    }
    let title = titles.get(id).cloned().or(first_message).unwrap_or_else(|| "(untitled)".to_string());
    Some(SessionSummary {
        pinned: false,
        id: id.to_string(),
        source: Source::Acp,
        scope: SessionScope::Owned,
        server: ServerId::local(paths).to_string(),
        title: title_line(&title),
        cwd: Nullable(cwd),
        repo_key: Nullable(None),
        git_branch: Nullable(None),
        updated_at: modified.clone(),
        created_at: created_at.unwrap_or(modified),
        size_bytes: metadata.len(),
        file_path: file_path.to_string_lossy().into_owned(),
        // Corrected by the driver's own answer wherever this server is running
        // the session, which is the only place one of these can be running.
        status: SessionStatus::Idle,
        activity: SessionActivity::Idle,
        resident: Nullable(None),
        claude_root: None,
        acp_agent: Some(agent.to_string()),
        account_id: None,
        attention_at: Nullable(None),
        unread: false,
        unread_count: 0,
        // Every permission request the agent makes is answered by the client,
        // so the session never stops to ask.
        asking: false,
        children: Vec::new(),
        parent_id: Nullable(None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `session/update` as the wire spells it, read through its type.
    fn update(value: Value) -> SessionUpdate {
        serde_json::from_value(value).expect("a session update")
    }

    fn chunk(kind: &str, text: &str) -> SessionUpdate {
        update(json!({"sessionUpdate": kind, "content": {"type": "text", "text": text}}))
    }

    fn lines(records: &[Value]) -> Vec<String> {
        records.iter().map(Value::to_string).collect()
    }

    fn parse(records: &[Value]) -> Vec<TranscriptItem> {
        let lines = lines(records);
        parse_lines(lines.iter().map(String::as_str))
    }

    #[test]
    fn a_streamed_answer_is_one_message_once_it_ends() {
        let mut translator = Translator::default();
        let mut records = vec![record("session", json!({"id": "s", "cwd": "/w"})), record("user", json!({"text": "hi"}))];
        records.extend(translator.update(&chunk("agent_thought_chunk", "let me ")));
        records.extend(translator.update(&chunk("agent_thought_chunk", "think")));
        records.extend(translator.update(&chunk("agent_message_chunk", "Hel")));
        records.extend(translator.update(&update(json!({"sessionUpdate": "usage_update", "used": 10, "size": 100}))));
        records.extend(translator.update(&chunk("agent_message_chunk", "lo")));
        records.extend(translator.update(&update(json!({"sessionUpdate": "usage_update", "used": 12, "size": 100}))));

        // Mid-stream: the answer so far is on screen, from its deltas.
        let streaming = parse(&records);
        assert_eq!(streaming.len(), 3, "{streaming:?}");
        assert_eq!((streaming[1].kind, streaming[1].text.as_str()), (ItemKind::Thinking, "let me think"));
        assert_eq!((streaming[2].kind, streaming[2].text.as_str()), (ItemKind::Text, "Hello"));

        records.extend(translator.flush());
        records.push(record("turn_end", json!({"stopReason": "end_turn"})));
        let done = parse(&records);
        assert_eq!(done.len(), 3, "the closing record replaces its deltas: {done:?}");
        assert_eq!((done[2].role, done[2].text.as_str()), (Role::Assistant, "Hello"));
        let spoken: Vec<_> = records.iter().filter_map(|r| r.as_object()).flat_map(spoken_texts).collect();
        assert_eq!(spoken, vec!["Hello"], "one message, counted once");
    }

    #[test]
    fn a_tool_call_and_its_result_pair_up() {
        let mut translator = Translator::default();
        let mut records = translator.update(&chunk("agent_message_chunk", "Looking."));
        records.extend(translator.update(&update(json!({
            "sessionUpdate": "tool_call", "toolCallId": "call-1", "title": "ls -la", "kind": "execute",
            "status": "in_progress", "rawInput": {"cmd": "ls -la"},
        }))));
        records.extend(translator.update(&update(
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "call-1", "status": "in_progress"}),
        )));
        records.extend(translator.update(&update(json!({
            "sessionUpdate": "tool_call_update", "toolCallId": "call-1", "status": "failed",
            "content": [{"type": "content", "content": {"type": "text", "text": "$ ls -la\n\nno such file"}}],
        }))));
        let items = parse(&records);
        assert_eq!(items.len(), 3, "{items:?}");
        assert_eq!(items[0].text, "Looking.", "a tool call closes the answer before it");
        assert_eq!((items[1].kind, items[1].tool_name.0.as_deref()), (ItemKind::ToolUse, Some("shell")));
        assert_eq!(items[1].text, "ls -la");
        assert_eq!(items[1].tool_use_id.as_deref(), Some("call-1"));
        assert_eq!((items[2].kind, items[2].result_of.as_deref()), (ItemKind::ToolResult, Some("call-1")));
        assert!(items[2].is_error);
        assert_eq!(items[2].text, "$ ls -la");
        assert_eq!(items[2].tool_name.0.as_deref(), Some("shell"));
    }

    #[test]
    fn a_replay_keeps_the_users_messages_and_a_live_turn_does_not_echo_them() {
        let mut translator = Translator::default();
        assert!(translator.update(&chunk("user_message_chunk", "hi")).is_empty());
        let mut records = translator.replayed(&chunk("user_message_chunk", "hi"));
        records.extend(translator.replayed(&chunk("agent_message_chunk", "hello")));
        records.extend(translator.replayed(&chunk("user_message_chunk", "again")));
        let items = parse(&records);
        let texts: Vec<_> = items.iter().map(|item| (item.role, item.text.as_str())).collect();
        assert_eq!(texts, vec![(Role::User, "hi"), (Role::Assistant, "hello"), (Role::User, "again")]);
    }

    #[test]
    fn interruptions_errors_and_compaction_are_said() {
        let mut translator = Translator::default();
        let mut records = translator.update(&update(
            json!({"sessionUpdate": "tool_call", "toolCallId": "t", "title": "Context compacting", "_meta": {"contextCompaction": true}}),
        ));
        records.extend(translator.update(&update(
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "t", "status": "completed", "_meta": {"contextCompaction": true}}),
        )));
        records.push(record("turn_end", json!({"stopReason": "cancelled"})));
        records.push(record("error", json!({"message": "the ACP agent exited"})));
        let items = parse(&records);
        assert_eq!(
            items.iter().map(|item| item.kind).collect::<Vec<_>>(),
            vec![ItemKind::Compact, ItemKind::Text, ItemKind::Text]
        );
        assert_eq!(items[1].text, "Interrupted");
        assert!(items[2].is_error);
    }

    #[test]
    fn attachments_are_shown_on_the_message_they_came_with() {
        let items = parse(&[record("user", json!({"text": "see", "images": ["/u/a.png"], "files": ["/u/notes.txt"]}))]);
        assert_eq!(items[0].images.as_deref(), Some(&["/u/a.png".to_string()][..]));
        assert_eq!(items[0].files.as_ref().map(|files| files[0].name.as_str()), Some("notes.txt"));
    }

    #[tokio::test]
    async fn a_transcript_lists_as_an_owned_session_of_its_agent() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        let dir = agent_dir(&paths, "mine").expect("a slug");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let file = dir.join("abc123.jsonl");
        let body = lines(&[
            record("session", json!({"id": "abc123", "agent": "mine", "agentSessionId": "agent-1", "cwd": "/work"})),
            record("user", json!({"text": "fix the build\nplease"})),
        ])
        .join("\n");
        tokio::fs::write(&file, body + "\n").await.unwrap();
        tokio::fs::write(dir.join("not an id!.jsonl"), "").await.unwrap();
        // Neither a directory an agent id cannot name, nor a stray file, is an agent.
        tokio::fs::create_dir_all(paths.acp_sessions().join("Not An Agent")).await.unwrap();
        tokio::fs::write(paths.acp_sessions().join("Not An Agent").join("zzz.jsonl"), "").await.unwrap();
        tokio::fs::write(paths.acp_sessions().join("stray.jsonl"), "").await.unwrap();

        let listed = list_sessions(&paths).await;
        assert_eq!(listed.len(), 1, "{listed:?}");
        let row = &listed[0];
        assert_eq!((row.source, row.scope), (Source::Acp, SessionScope::Owned));
        assert_eq!(row.acp_agent.as_deref(), Some("mine"));
        assert_eq!(row.title, "fix the build");
        assert_eq!(row.cwd.0.as_deref(), Some("/work"));
        assert!(has_conversation(&file).await);
        assert!(find_session(&paths, "../abc123").await.is_none());
        assert!(find_session(&paths, "zzz").await.is_none());
        assert_eq!(find_session(&paths, "abc123").await.and_then(|row| row.acp_agent), Some("mine".into()));
        let header = read_header(&file).await.expect("a header");
        assert_eq!(header.agent_session_id.as_deref(), Some("agent-1"));
        assert_eq!(header.cwd.as_deref(), Some("/work"));
        assert!(agent_dir(&paths, "../x").is_none());
    }

    #[tokio::test]
    async fn sessions_of_two_agents_are_listed_with_their_own_agent() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        for (agent, id) in [("one", "s-1"), ("two", "s-2")] {
            let dir = agent_dir(&paths, agent).unwrap();
            tokio::fs::create_dir_all(&dir).await.unwrap();
            // Both agents gave their session the same id; this server's ids differ.
            let header = record("session", json!({"id": id, "agent": agent, "agentSessionId": "same", "cwd": "/w"}));
            tokio::fs::write(dir.join(format!("{id}.jsonl")), format!("{header}\n")).await.unwrap();
        }
        let mut listed: Vec<(String, Option<String>)> = list_sessions(&paths).await.into_iter().map(|row| (row.id, row.acp_agent)).collect();
        listed.sort();
        assert_eq!(listed, vec![("s-1".into(), Some("one".into())), ("s-2".into(), Some("two".into()))]);
        assert_eq!(locate(&paths, "s-2").await.map(|(agent, _)| agent).as_deref(), Some("two"));
    }
}
