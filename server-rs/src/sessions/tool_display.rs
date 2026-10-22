//! How a Claude tool call and its result read on screen.
//!
//! Port of the former TypeScript server's `tool-display.ts`. A Claude
//! `tool_use` block carries a parsed JSON object, so everything here starts
//! from one. Codex hands over the *source text* of a JavaScript cell or a
//! JSON-encoded argument string, and records what actually ran somewhere else
//! entirely; that half lives in [`super::codex_tools`], which borrows the
//! generic helpers below so that a tool nothing knows by name reads the same
//! from either CLI.

use serde_json::{Map, Value};

use super::util::{first_line_of, QuestionEntry, QuestionOption, ToolDisplay};
use crate::openapi_ext::Nullable;

type Json = Map<String, Value>;

/// Structured questions/options for interactive rendering.
pub fn extract_questions(input: &Json) -> Option<Vec<QuestionEntry>> {
    let questions = input.get("questions").and_then(Value::as_array)?;
    let mut out = Vec::new();
    for question in questions {
        let Some(question) = question.as_object() else { continue };
        let options = question.get("options").and_then(Value::as_array).cloned().unwrap_or_default();
        out.push(QuestionEntry {
            question: as_display_string(question.get("question")),
            header: Nullable(question.get("header").and_then(Value::as_str).map(str::to_owned)),
            multi_select: question.get("multiSelect") == Some(&Value::Bool(true)),
            options: options
                .iter()
                .filter_map(Value::as_object)
                .map(|option| QuestionOption {
                    label: as_display_string(option.get("label")),
                    description: Nullable(
                        option
                            .get("description")
                            .and_then(Value::as_str)
                            .filter(|d| !d.is_empty())
                            .map(str::to_owned),
                    ),
                })
                .collect(),
        });
    }
    (!out.is_empty()).then_some(out)
}

/// `String(v)` in JS: a bare string stays bare, anything else is stringified,
/// and `undefined ?? ""` collapses to the empty string.
pub(super) fn as_display_string(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn str_field(input: &Json, key: &str) -> Option<String> {
    input.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// The task a call names, under either spelling the tools use: `TaskUpdate`
/// writes `taskId` and `TaskStop` writes `task_id`, on the same board.
fn task_id(input: &Json) -> Option<String> {
    str_field(input, "taskId")
        .or_else(|| str_field(input, "task_id"))
        .filter(|id| !id.trim().is_empty())
}

/// `mcp__Server__tool` / `mcp___weird-server__tool` -> `Server:tool`
pub fn shorten_tool_name(name: &str) -> String {
    if !name.starts_with("mcp_") {
        return name.to_string();
    }
    // `replace(/^mcp_+/, "")` strips `mcp` plus every following underscore.
    let rest = name.strip_prefix("mcp").expect("checked above").trim_start_matches('_');
    match rest.find("__") {
        None => rest.to_string(),
        Some(sep) => format!("{}:{}", rest[..sep].trim_start_matches('_'), &rest[sep + 2..]),
    }
}

/// What to put on the row of a tool nothing here knows by name, in order of how
/// well each key describes a call.
///
/// The tail of the list is the part that earns its keep: a tool whose input is
/// nothing but routing ids used to fall through to `JSON.stringify(input)`, so
/// the row read `{"taskId":"8","status":"in_progress"}`. An id is a poor summary
/// and a raw object is a worse one.
const PREFERRED_KEYS: &[&str] = &[
    "command",
    "cmd",
    "file_path",
    "path",
    "url",
    "query",
    "pattern",
    "subject",
    "title",
    "skill",
    // Before `description`: a tool that writes both means the summary to be the
    // short one.
    "summary",
    "description",
    "expression",
    "selector",
    "objective",
    "target",
    "app",
    "text",
    "prompt",
    "script",
    "plan",
    // Last resort, for a call that is only a handle to something else.
    "taskId",
    "task_id",
    "agentId",
    "agent_id",
    "name",
    "label",
];

pub(super) fn pick_preferred(input: &Json) -> Option<String> {
    PREFERRED_KEYS.iter().find_map(|key| {
        input
            .get(*key)
            .and_then(Value::as_str)
            .filter(|v| !v.trim().is_empty())
            .map(str::to_owned)
    })
}

/// Fernet blobs in agent-orchestration messages are unreadable; blank them out.
pub(super) fn redact_encrypted(input: &Json) -> Json {
    let mut out = input.clone();
    if let Some(Value::String(v)) = out.get("message") {
        if v.starts_with("gAAAAA") {
            let replacement = Value::String(format!("<encrypted {} chars>", v.chars().count()));
            out.insert("message".to_string(), replacement);
        }
    }
    out
}

pub(super) fn pretty(input: &Json) -> String {
    serde_json::to_string_pretty(input).unwrap_or_default()
}

/// `JSON.stringify(input)` for the one-line fallback summary.
pub(super) fn compact(input: &Json) -> String {
    serde_json::to_string(input).unwrap_or_default()
}

pub(super) fn diff_of(old_text: &str, new_text: &str) -> String {
    let removed = old_text.split('\n').map(|l| format!("- {l}"));
    let added = new_text.split('\n').map(|l| format!("+ {l}"));
    removed.chain(added).collect::<Vec<_>>().join("\n")
}

/// A `$ command` summary, with an ellipsis when more lines follow.
fn command_line(command: &str) -> String {
    format!("$ {}{}", first_line_of(command, 180), if command.contains('\n') { " ⋯" } else { "" })
}

/// Claude `tool_use` blocks: `{name, input}`.
pub fn display_claude_tool_use(name: &str, input: &Json) -> ToolDisplay {
    let tool_name = shorten_tool_name(name);
    match name {
        // A description says what the call is *for*, which reads better in a
        // collapsed list than the shell line, so it takes the row and the
        // command drops to the caption. The caption is clipped to a third of the
        // row, so the fold becomes the only place the command reads in full:
        // with a description it is always there, even for a one-liner.
        "Bash" | "Monitor" => {
            let command = str_field(input, "command").unwrap_or_default();
            match str_field(input, "description").filter(|d| !d.trim().is_empty()) {
                Some(description) => ToolDisplay {
                    tool_name: Some(tool_name),
                    text: description,
                    meta: Some(command_line(&command)),
                    detail: Some(command),
                    is_error: false,
                },
                None => ToolDisplay {
                    tool_name: Some(tool_name),
                    text: command_line(&command),
                    meta: None,
                    // Nothing to unfold: the row already shows the whole command.
                    detail: command.contains('\n').then_some(command),
                    is_error: false,
                },
            }
        }
        "Read" => {
            let has_range = input.contains_key("offset") || input.contains_key("limit");
            let range = has_range.then(|| {
                let offset = input
                    .get("offset")
                    .filter(|v| !v.is_null())
                    .map(number_text)
                    .unwrap_or_else(|| "0".to_string());
                match input.get("limit").filter(|v| !v.is_null()) {
                    Some(limit) => format!("@{offset}+{}", number_text(limit)),
                    None => format!("@{offset}"),
                }
            });
            ToolDisplay {
                tool_name: Some(tool_name),
                text: as_display_string(input.get("file_path")),
                meta: range,
                detail: None,
                is_error: false,
            }
        }
        "Write" => ToolDisplay {
            tool_name: Some(tool_name),
            text: as_display_string(input.get("file_path")),
            meta: None,
            detail: str_field(input, "content"),
            is_error: false,
        },
        "Edit" => {
            let old_string = str_field(input, "old_string").unwrap_or_default();
            let new_string = str_field(input, "new_string").unwrap_or_default();
            ToolDisplay {
                tool_name: Some(tool_name),
                text: as_display_string(input.get("file_path")),
                meta: (input.get("replace_all") == Some(&Value::Bool(true))).then(|| "replace_all".to_string()),
                detail: Some(diff_of(&old_string, &new_string)),
                is_error: false,
            }
        }
        "TodoWrite" => {
            let todos = input.get("todos").and_then(Value::as_array).cloned().unwrap_or_default();
            let lines = todos
                .iter()
                .map(|todo| {
                    let todo = todo.as_object().cloned().unwrap_or_default();
                    let mark = match todo.get("status").and_then(Value::as_str) {
                        Some("completed") => "[x]",
                        Some("in_progress") => "[~]",
                        _ => "[ ]",
                    };
                    format!("{mark} {}", as_display_string(todo.get("content")))
                })
                .collect::<Vec<_>>()
                .join("\n");
            ToolDisplay {
                tool_name: Some(tool_name),
                text: format!("{} todos", todos.len()),
                meta: None,
                detail: (!lines.is_empty()).then_some(lines),
                is_error: false,
            }
        }
        "Agent" | "Task" => {
            let parts: Vec<String> = ["subagent_type", "model"].iter().filter_map(|key| str_field(input, key)).collect();
            ToolDisplay {
                tool_name: Some(tool_name),
                text: as_display_string(input.get("description")),
                meta: (!parts.is_empty()).then(|| parts.join(" · ")),
                detail: str_field(input, "prompt"),
                is_error: false,
            }
        }
        // The agent-orchestration family: a message to a running child, and the
        // task board beside it. Every one of these calls is mostly routing — an
        // agent id, a task id, a status — with the one readable field buried in
        // the middle, so the generic key sweep found nothing and the row fell
        // through to `JSON.stringify(input)`. Measured on the recorded sessions:
        // 159 `SendMessage`, 57 `TaskUpdate` and 29 `TaskStop` rows that all read
        // `{"to":"a5ceb2dbf3bd2fb99","summary":…,"message":…}` instead of saying
        // what was sent to whom.
        "SendMessage" => {
            // `content` repeats `message` on every recorded call; either may be
            // the fernet blob `redact_encrypted` exists for.
            let redacted = redact_encrypted(input);
            let body = str_field(&redacted, "message")
                .or_else(|| str_field(&redacted, "content"))
                .unwrap_or_default();
            let summary = str_field(input, "summary")
                .filter(|summary| !summary.trim().is_empty())
                .unwrap_or_else(|| body.clone());
            ToolDisplay {
                tool_name: Some(tool_name),
                text: first_line_of(&summary, 180),
                meta: str_field(input, "to")
                    .or_else(|| str_field(input, "recipient"))
                    .map(|to| format!("→ {to}")),
                detail: (!body.is_empty()).then_some(body),
                is_error: false,
            }
        }
        "TaskCreate" => ToolDisplay {
            tool_name: Some(tool_name),
            text: first_line_of(&as_display_string(input.get("subject")), 180),
            meta: str_field(input, "activeForm"),
            detail: str_field(input, "description"),
            is_error: false,
        },
        "TaskUpdate" => {
            let status = str_field(input, "status");
            let text = match (task_id(input), &status) {
                (Some(id), Some(status)) => format!("task {id} → {status}"),
                (Some(id), None) => format!("task {id}"),
                (None, Some(status)) => status.clone(),
                (None, None) => compact(input),
            };
            ToolDisplay {
                tool_name: Some(tool_name),
                text,
                meta: None,
                // The blocking edges live nowhere else on the row.
                detail: Some(pretty(input)),
                is_error: false,
            }
        }
        "TaskStop" | "TaskGet" => ToolDisplay {
            tool_name: Some(tool_name),
            text: task_id(input).map(|id| format!("task {id}")).unwrap_or_default(),
            meta: None,
            // An id is the whole call; anything else it carries is worth a fold.
            detail: (input.len() > 1).then(|| pretty(input)),
            is_error: false,
        },
        "AskUserQuestion" => {
            let questions = input.get("questions").and_then(Value::as_array).cloned().unwrap_or_default();
            let first = questions.first().and_then(Value::as_object).cloned().unwrap_or_default();
            let fallback = str_field(input, "__unparsedToolInput").unwrap_or_default();
            let mut lines: Vec<String> = Vec::new();
            for question in questions.iter().filter_map(Value::as_object) {
                if !lines.is_empty() {
                    lines.push(String::new());
                }
                lines.push(as_display_string(question.get("question")));
                for option in question
                    .get("options")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_object)
                {
                    let description = match option.get("description").and_then(Value::as_str).filter(|d| !d.is_empty()) {
                        Some(d) => format!(" — {d}"),
                        None => String::new(),
                    };
                    lines.push(format!("◦ {}{description}", as_display_string(option.get("label"))));
                }
            }
            let headline = match first.get("question").filter(|value| !value.is_null()) {
                Some(question) => as_display_string(Some(question)),
                None => fallback.clone(),
            };
            let detail = lines.join("\n");
            ToolDisplay {
                tool_name: Some(tool_name),
                text: first_line_of(&headline, 180),
                meta: (questions.len() > 1).then(|| format!("{} questions", questions.len())),
                detail: if detail.is_empty() {
                    (!fallback.is_empty()).then_some(fallback)
                } else {
                    Some(detail)
                },
                is_error: false,
            }
        }
        _ => {
            if input.is_empty() {
                return ToolDisplay {
                    tool_name: Some(tool_name),
                    text: String::new(),
                    meta: None,
                    detail: None,
                    is_error: false,
                };
            }
            let picked = pick_preferred(input);
            ToolDisplay {
                tool_name: Some(tool_name),
                text: first_line_of(&picked.unwrap_or_else(|| compact(input)), 180),
                meta: None,
                detail: Some(pretty(&redact_encrypted(input))),
                is_error: false,
            }
        }
    }
}

/// JS renders a JSON number without a trailing `.0`.
fn number_text(value: &Value) -> String {
    match value {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// How many `image` blocks a `tool_result`'s content carries.
///
/// The blocks themselves are base64 payloads a quarter of a megabyte each — far
/// too much to put on the wire, and the transcript records no path beside them —
/// so the display keeps its `[N image]` note and the parser uses this count to
/// decide whether the *call's* file is worth offering as a thumbnail. See
/// `transcript::parse_claude_transcript`.
pub fn result_image_count(content: Option<&Value>) -> usize {
    let Some(Value::Array(blocks)) = content else { return 0 };
    blocks
        .iter()
        .filter_map(Value::as_object)
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("image"))
        .count()
}

/// Claude `tool_result` content: a string, or an array of blocks.
pub fn display_claude_tool_result(content: Option<&Value>, is_error_flag: bool) -> ToolDisplay {
    static SYSTEM_REMINDER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static TOOL_USE_ERROR: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static UPDATED: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static CREATED: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static NO_OUTPUT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let system_reminder = SYSTEM_REMINDER.get_or_init(|| regex::Regex::new(r"(?s)<system-reminder>.*?</system-reminder>").expect("valid"));
    let tool_use_error = TOOL_USE_ERROR.get_or_init(|| regex::Regex::new(r"(?s)^<tool_use_error>(.*?)</tool_use_error>$").expect("valid"));
    let updated = UPDATED.get_or_init(|| regex::Regex::new(r"^The file (\S+) has been updated").expect("valid"));
    let created = CREATED.get_or_init(|| regex::Regex::new(r"^File created successfully at: (\S+)").expect("valid"));
    let no_output = NO_OUTPUT.get_or_init(|| regex::Regex::new(r"^\(?[A-Za-z]+ completed with no output\)?\.?$").expect("valid"));

    let mut body = String::new();
    let mut extras: Vec<String> = Vec::new();
    match content {
        Some(Value::String(text)) => body = text.clone(),
        Some(Value::Array(blocks)) => {
            let mut texts: Vec<&str> = Vec::new();
            let mut tool_refs: Vec<&str> = Vec::new();
            let mut images = 0usize;
            for block in blocks.iter().filter_map(Value::as_object) {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            texts.push(text);
                        }
                    }
                    Some("tool_reference") => {
                        if let Some(name) = block.get("tool_name").and_then(Value::as_str) {
                            tool_refs.push(name);
                        }
                    }
                    Some("image") => images += 1,
                    _ => {}
                }
            }
            body = texts.join("\n");
            if !tool_refs.is_empty() {
                extras.push(format!("tools: {}", tool_refs.join(", ")));
            }
            if images > 0 {
                extras.push(format!("[{images} image{}]", if images > 1 { "s" } else { "" }));
            }
        }
        _ => {}
    }

    body = system_reminder.replace_all(&body, "").trim().to_string();
    let mut is_error = is_error_flag;
    if let Some(caps) = tool_use_error.captures(&body) {
        body = caps[1].trim().to_string();
        is_error = true;
    }
    if !extras.is_empty() {
        body = std::iter::once(body.clone())
            .chain(extras)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
    }

    // Boilerplate success messages collapse to a short verdict; full text stays in detail.
    let mut text = first_line_of(&body, 180);
    if text.is_empty() {
        text = "(empty)".to_string();
    }
    if let Some(caps) = updated.captures(&body) {
        text = format!("updated {}", caps[1].rsplit('/').next().unwrap_or(&caps[1]));
    } else if let Some(caps) = created.captures(&body) {
        text = format!("created {}", caps[1].rsplit('/').next().unwrap_or(&caps[1]));
    } else if no_output.is_match(body.trim()) {
        text = "no output".to_string();
    }

    ToolDisplay {
        // A result inherits the tool name from its call; the transcript parser
        // fills it in once it has matched the `tool_use_id`.
        tool_name: None,
        text,
        meta: None,
        detail: (!body.is_empty()).then_some(body),
        is_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(value: serde_json::Value) -> Json {
        value.as_object().cloned().expect("object literal")
    }

    #[test]
    fn mcp_tool_names_shorten() {
        assert_eq!(shorten_tool_name("mcp__Server__tool"), "Server:tool");
        assert_eq!(shorten_tool_name("mcp___weird-server__tool"), "weird-server:tool");
        assert_eq!(shorten_tool_name("Bash"), "Bash");
        assert_eq!(shorten_tool_name("mcp__nosep"), "nosep");
    }

    #[test]
    fn bash_leads_with_its_description_and_captions_the_command() {
        let display = display_claude_tool_use("Bash", &json(serde_json::json!({"command": "ls -a\ncd /tmp", "description": "look"})));
        assert_eq!(display.text, "look");
        assert_eq!(display.meta.as_deref(), Some("$ ls -a ⋯"));
        assert_eq!(display.detail.as_deref(), Some("ls -a\ncd /tmp"));
    }

    #[test]
    fn a_described_bash_call_folds_even_a_one_line_command() {
        let display = display_claude_tool_use("Bash", &json(serde_json::json!({"command": "ls -a", "description": "look"})));
        assert_eq!(display.text, "look");
        assert_eq!(display.meta.as_deref(), Some("$ ls -a"));
        assert_eq!(
            display.detail.as_deref(),
            Some("ls -a"),
            "the caption is clipped, so only the fold reads in full"
        );
    }

    #[test]
    fn an_undescribed_bash_call_keeps_the_command_on_the_row() {
        let display = display_claude_tool_use("Bash", &json(serde_json::json!({"command": "ls -a\ncd /tmp"})));
        assert_eq!(display.text, "$ ls -a ⋯");
        assert_eq!(display.meta, None);
        assert_eq!(display.detail.as_deref(), Some("ls -a\ncd /tmp"));

        let blank = display_claude_tool_use("Bash", &json(serde_json::json!({"command": "ls -a", "description": "  "})));
        assert_eq!(blank.text, "$ ls -a", "a whitespace description is no summary");
        assert_eq!(blank.detail, None, "nothing to unfold: the row holds the whole command");
    }

    #[test]
    fn read_renders_its_range() {
        let display = display_claude_tool_use("Read", &json(serde_json::json!({"file_path": "/a/b.rs", "offset": 10, "limit": 5})));
        assert_eq!(display.text, "/a/b.rs");
        assert_eq!(display.meta.as_deref(), Some("@10+5"));
    }

    #[test]
    fn tool_result_collapses_boilerplate_and_lifts_errors() {
        let ok = display_claude_tool_result(Some(&serde_json::json!("The file /x/y.ts has been updated. Here is …")), false);
        assert_eq!(ok.text, "updated y.ts");
        assert!(!ok.is_error);

        let err = display_claude_tool_result(Some(&serde_json::json!("<tool_use_error>File not found</tool_use_error>")), false);
        assert_eq!(err.text, "File not found");
        assert!(err.is_error, "a tool_use_error envelope marks the item failed even without the flag");
    }

    #[test]
    fn tool_result_strips_system_reminders_and_counts_images() {
        let result = display_claude_tool_result(
            Some(&serde_json::json!([
                {"type": "text", "text": "done<system-reminder>noise</system-reminder>"},
                {"type": "image"},
                {"type": "image"},
            ])),
            false,
        );
        assert_eq!(result.detail.as_deref(), Some("done\n[2 images]"));
    }

    #[test]
    fn questions_survive_extraction() {
        let input = json(serde_json::json!({
            "questions": [{"question": "Pick", "header": "h", "multiSelect": true,
                           "options": [{"label": "a", "description": "first"}, {"label": "b"}]}]
        }));
        let questions = extract_questions(&input).expect("one question");
        assert_eq!(questions.len(), 1);
        assert!(questions[0].multi_select);
        assert_eq!(questions[0].options[0].description.as_ref().map(String::as_str), Some("first"));
        assert!(questions[0].options[1].description.as_ref().is_none(), "a missing description stays null");
    }

    /// The shape of a real `SendMessage`, which used to render as its own JSON.
    #[test]
    fn a_message_to_an_agent_reads_as_a_summary_and_a_recipient() {
        let display = display_claude_tool_use(
            "SendMessage",
            &json(serde_json::json!({
                "to": "a5ceb2dbf3bd2fb99",
                "summary": "恢复竞品调研任务",
                "message": "进程刚才中断了,请继续。",
                "type": "message",
                "recipient": "a5ceb2dbf3bd2fb99",
                "content": "进程刚才中断了,请继续。",
            })),
        );
        assert_eq!(display.text, "恢复竞品调研任务");
        assert_eq!(display.meta.as_deref(), Some("→ a5ceb2dbf3bd2fb99"));
        assert_eq!(display.detail.as_deref(), Some("进程刚才中断了,请继续。"));
        assert!(!display.text.contains('{'), "never the raw object");
    }

    /// No summary written: the message itself is the next best line, and an
    /// encrypted body must not be it.
    #[test]
    fn a_message_without_a_summary_leads_with_its_own_text() {
        let plain = display_claude_tool_use("SendMessage", &json(serde_json::json!({"to": "a1", "message": "ship it\nplease"})));
        assert_eq!(plain.text, "ship it", "the row is one line; the fold has the rest");
        assert_eq!(plain.detail.as_deref(), Some("ship it\nplease"));

        let sealed = display_claude_tool_use("SendMessage", &json(serde_json::json!({"to": "a1", "message": "gAAAAABmZ2FrZQ"})));
        assert_eq!(sealed.text, "<encrypted 14 chars>", "a fernet blob is unreadable either way");
    }

    #[test]
    fn the_task_board_says_which_task_and_what_happened_to_it() {
        let created = display_claude_tool_use(
            "TaskCreate",
            &json(serde_json::json!({"subject": "WP-A: PWA install assets", "description": "the long brief", "activeForm": "Generating assets"})),
        );
        assert_eq!(created.text, "WP-A: PWA install assets");
        assert_eq!(created.meta.as_deref(), Some("Generating assets"));
        assert_eq!(created.detail.as_deref(), Some("the long brief"));

        let updated = display_claude_tool_use("TaskUpdate", &json(serde_json::json!({"taskId": "8", "status": "in_progress"})));
        assert_eq!(updated.text, "task 8 → in_progress");
        assert!(updated.detail.is_some(), "the blocking edges stay in the fold");

        // The same board, the other spelling of the same field.
        let stopped = display_claude_tool_use("TaskStop", &json(serde_json::json!({"task_id": "b8jt0re3r"})));
        assert_eq!(stopped.text, "task b8jt0re3r");
        assert_eq!(stopped.detail, None, "an id is the whole call");
    }

    /// A tool nothing here knows by name still has to say something: the sweep
    /// reaches an id before it gives up and prints the object.
    #[test]
    fn an_unknown_tool_prefers_a_summary_then_an_id_over_raw_json() {
        let summarised = display_claude_tool_use(
            "Whatever",
            &json(serde_json::json!({"summary": "did a thing", "description": "the long one"})),
        );
        assert_eq!(summarised.text, "did a thing", "a summary outranks a description");

        let handle = display_claude_tool_use("Whatever", &json(serde_json::json!({"taskId": "t-9", "flag": true})));
        assert_eq!(handle.text, "t-9");

        let nothing = display_claude_tool_use("Whatever", &json(serde_json::json!({"flag": true})));
        assert_eq!(nothing.text, r#"{"flag":true}"#, "with nothing readable, the object is still the truth");
    }

    #[test]
    fn image_blocks_in_a_result_are_counted_but_not_carried() {
        let content = serde_json::json!([
            {"type": "text", "text": "here it is"},
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}},
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "BBBB"}},
        ]);
        assert_eq!(result_image_count(Some(&content)), 2);
        assert_eq!(result_image_count(Some(&serde_json::json!("plain text"))), 0);
        assert_eq!(result_image_count(None), 0);

        let display = display_claude_tool_result(Some(&content), false);
        assert!(
            display.detail.as_deref().expect("a body").contains("[2 images]"),
            "the count stays as the fallback line: {display:?}"
        );
        assert!(!display.detail.as_deref().expect("a body").contains("AAAA"), "never the base64");
    }
}
