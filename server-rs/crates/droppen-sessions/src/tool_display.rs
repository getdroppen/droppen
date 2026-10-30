//! What a tool call and its result read as on screen, whichever CLI ran it:
//! the helpers each CLI's own cards borrow, so that a tool nothing knows by
//! name reads the same from any of them. The cards themselves are each
//! reader's, in the server.

use serde_json::{Map, Value};

use super::util::{QuestionEntry, QuestionOption};
use droppen_http::openapi_ext::Nullable;

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
pub fn as_display_string(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
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

/// The first non-blank string among the keys a one-line summary prefers, in order.
pub fn pick_preferred(input: &Json) -> Option<String> {
    PREFERRED_KEYS.iter().find_map(|key| {
        input
            .get(*key)
            .and_then(Value::as_str)
            .filter(|v| !v.trim().is_empty())
            .map(str::to_owned)
    })
}

/// Fernet blobs in agent-orchestration messages are unreadable; blank them out.
pub fn redact_encrypted(input: &Json) -> Json {
    let mut out = input.clone();
    if let Some(Value::String(v)) = out.get("message") {
        if v.starts_with("gAAAAA") {
            let replacement = Value::String(format!("<encrypted {} chars>", v.chars().count()));
            out.insert("message".to_string(), replacement);
        }
    }
    out
}

/// `input` as indented JSON, for a tool call shown in full.
pub fn pretty(input: &Json) -> String {
    serde_json::to_string_pretty(input).unwrap_or_default()
}

/// `JSON.stringify(input)` for the one-line fallback summary.
pub fn compact(input: &Json) -> String {
    serde_json::to_string(input).unwrap_or_default()
}

/// `old_text` and `new_text` as one diff: every old line as `- `, then every new
/// line as `+ `.
pub fn diff_of(old_text: &str, new_text: &str) -> String {
    let removed = old_text.split('\n').map(|l| format!("- {l}"));
    let added = new_text.split('\n').map(|l| format!("+ {l}"));
    removed.chain(added).collect::<Vec<_>>().join("\n")
}

/// The fold of a checklist tool: one line per item, `[x]` when done, `[~]` for
/// one of `in_progress` and `[ ]` otherwise.
///
/// Claude's `TodoWrite` and Codex's `update_plan` are the same board in two
/// shapes and spell the in-progress state differently, so each caller names the
/// statuses its CLI writes.
pub trait ChecklistLinesExt: Iterator + Sized {
    /// The board, one line per `(status, text)` item.
    fn checklist_lines(self, in_progress: &[&str]) -> String;
}

impl<S: AsRef<str>, T: AsRef<str>, I: Iterator<Item = (S, T)>> ChecklistLinesExt for I {
    fn checklist_lines(self, in_progress: &[&str]) -> String {
        self.map(|(status, text)| {
            let mark = match status.as_ref() {
                "completed" => "[x]",
                status if in_progress.contains(&status) => "[~]",
                _ => "[ ]",
            };
            format!("{mark} {}", text.as_ref())
        })
        .collect::<Vec<_>>()
        .join("\n")
    }
}

/// The fold of a question tool: each question's title, then one `◦` line per
/// option, a blank line between questions.
///
/// Claude's `AskUserQuestion` and Codex's `request_user_input` spell the
/// question and its options differently — Claude shows an option's description,
/// Codex does not — so each caller renders both through its own closures.
pub trait QuestionLinesExt<'a>: Iterator<Item = &'a Json> + Sized {
    /// Every question's lines, its title and options rendered by the caller's closures.
    fn question_lines(self, title_of: impl Fn(&Json) -> String, option_of: impl Fn(&Value) -> Option<String>) -> Vec<String>;
}

impl<'a, I: Iterator<Item = &'a Json>> QuestionLinesExt<'a> for I {
    fn question_lines(self, title_of: impl Fn(&Json) -> String, option_of: impl Fn(&Value) -> Option<String>) -> Vec<String> {
        let mut lines = Vec::new();
        for question in self {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push(title_of(question));
            for option in question.get("options").and_then(Value::as_array).into_iter().flatten() {
                if let Some(line) = option_of(option) {
                    lines.push(line);
                }
            }
        }
        lines
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
}
